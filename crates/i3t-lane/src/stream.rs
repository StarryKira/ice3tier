//! 一条 lane 上的一条流：[`LaneSend`] 和 [`LaneRecv`]，实现连接层契约里的 [`SendStream`] 和 [`RecvStream`]。
//!
//! # 两种负载方式
//!
//! 每个方向开始时都在 lane 的 TLS 里面（帧见 crate 文档）。发起方在头写完时（[`LaneSend::end_header`]）选定负载方式：
//!
//! - sealed：两个方向一直在 TLS 里，数据是 DATA 帧，正常结束是 close_notify 加 FIN。没有 close_notify 的 FIN 是截断，按异常结束算。
//! - passthrough：每个方向在自己的 SWITCH 所在的记录之后离开 TLS。接受方在 REPLY 之后、确认可以不再加密时调
//!   [`LaneSend::end_header`] 发出它的（流层是在目标的 TLS 1.3 ServerHello 之后）；发起方收到它时回一个，自己的方向跟着离开
//!   （[`Mode::Mutual`]）。接受方不发，两个方向就一直留在 TLS 里，对端照样读得懂：目标回的是 HelloRetryRequest、TLS 1.2
//!   或者根本不是 TLS 时，客户端接下来发的（第二个 ClientHello、客户端证书、明文）都还加密（issue #34）。
//!   离开之后是用户的原始字节，FIN 就是结束，RST 或者出错就是异常结束。
//!
//! 对端是没升级的节点（lane 谈定的是 `i3t-lane/1`）时，本端发起的流一律 sealed：那边不认"等 SWITCH"。
//! 那边发起的 passthrough 流照旧：它的方向在 OPENING 所在的记录之后就离开 TLS（[`Mode::Early`]），只有接受方发 SWITCH。
//!
//! 流层的头（OPEN、REPLY）决定连谁、连上了没有，只能来自认证过的对端，所以它们必须完整地在 TLS 里：
//! 收的一方读完对端的头（[`RecvStream::end_header`]）之前，接收方向只交出 TLS 里收到的字节；对端的方向离开了 TLS、
//! 头却还没读完时读出错（对端不守协议），不会拿后面未经认证的原始字节去凑。
//!
//! # 异常结束
//!
//! lane 是一整条只属于这条流的 TCP，所以任何一个方向的异常结束都让整条 lane 异常结束：
//!
//! | 本端做了什么 | 对端看到什么 |
//! |---|---|
//! | [`SendStream::reset`]，这个方向还在 TLS 里 | ABORT 帧（带原因码），随后 FIN；两个方向都丢弃之后等对端关掉再关，那时发 RST |
//! | [`SendStream::reset`]，这个方向已经是原始字节 | RST（Linux 上立刻发，别处在关闭时） |
//! | [`RecvStream::stop`]，本端的发送方向还在 TLS 里 | STOP 帧（带原因码），对端之后的写出错；lane 不断，本端的发送照常 |
//! | [`RecvStream::stop`]，本端的发送方向已经不在 TLS 里 | RST |
//! | 没有 `shutdown` 就丢弃发送方向 | 同 `reset` |
//!
//! lane 从开流起就设成"关闭时发 RST"，只有两个方向都正常结束才改回去，所以任务被取消、进程退出也一样。
//! macOS 上是在发送方向正常结束、发 FIN 时就改回去：那里两个方向都关了之后改不了（见 `i3t_underlay::tcp_reset`）。
//! 收到的 RST 在读写时是一个不带原因码的连接错误（分不清是对端重置了流，还是路上断了），ABORT 帧是带原因码的
//! [`StreamAborted::Reset`]，STOP 帧让本端之后的写返回 [`StreamAborted::Stopped`]。
//!
//! 发了 ABORT 帧的一端不在丢弃时马上关：Windows 收到 RST 时把还没读走的数据扔掉，紧跟在 ABORT 后面的 RST
//! 会让对端连 ABORT 和原因码一起丢掉。所以它先发 FIN，把对端还在路上的数据收下扔掉，等对端关掉（最多
//! [`LINGER_AFTER_STOP`]）再关。ABORT 后面的 FIN 不会被当成正常结束：sealed 的方向上正常结束要有 close_notify。
//!
//! # 写不需要 flush
//!
//! 流层是照 QUIC 的流写的，不调 `flush`。所以 [`LaneSend`] 返回"写进去了"之后，密文要么已经交给内核，
//! 要么由一个短命的后台任务等 socket 可写时送出，不会停在缓冲区里等下一次写。

use std::fmt;
use std::io;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::{Context, Poll, Waker, ready};
use std::time::Duration;

use bytes::{Buf, Bytes, BytesMut};
use i3t_conn::contract::wait_revoked;
use i3t_conn::{Payload, RawRead, RawWrite, RecvStream, SendStream, StreamAborted, StreamCode};
use i3t_proto::NodeId;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio::sync::{Notify, watch};

use crate::frame::{Frame, MAX_DATA_LEN, MAX_HEADER_LEN, Mode, data_header};
use crate::lane::{Lane, Version};
use crate::sealed::{Next, Sealed};
use crate::socket;
use crate::status::{Book, Gauge};

/// 发送方向没有正常结束就被丢弃时，对端看到的原因码。和 QUIC 的实现取同一个数。
const DROPPED_SEND: StreamCode = StreamCode(u32::MAX);
/// 接收方向没读完就被丢弃时，对端看到的原因码。同上。
const DROPPED_RECV: StreamCode = StreamCode(0);

/// 一次从 socket 读多少（sealed 的方向）。
const READ_CHUNK: usize = 64 * 1024;

/// 本端叫停了对端、自己这一侧已经正常写完，或者本端发了 ABORT 帧时，关闭之前最多再收多久（收到就扔）：
/// 关闭时接收缓冲里还有没读的数据，内核会发 RST 并丢掉还没发出去的数据，刚写完的 REPLY 可能就此丢了；
/// 对端是 Windows 时，它收到 RST 也会丢掉还没读走的 ABORT。
const LINGER_AFTER_STOP: Duration = Duration::from_secs(5);

/// 这一端在流里是哪一方。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    Initiator,
    Responder,
}

type RevocationWait = Pin<Box<dyn Future<Output = ()> + Send + Sync>>;

/// 一条流的发送方向。实现 [`SendStream`]。
pub struct LaneSend {
    shared: Arc<Shared>,
    wait_revoked: RevocationWait,
}

/// 一条流的接收方向。实现 [`RecvStream`]。
pub struct LaneRecv {
    shared: Arc<Shared>,
    wait_revoked: RevocationWait,
}

impl fmt::Debug for LaneSend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LaneSend").field("peer", &self.shared.peer).finish_non_exhaustive()
    }
}

impl fmt::Debug for LaneRecv {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LaneRecv").field("peer", &self.shared.peer).finish_non_exhaustive()
    }
}

struct Shared {
    socket: Arc<TcpStream>,
    peer: NodeId,
    state: Mutex<State>,
    revoked: watch::Receiver<bool>,
    /// 两个方向都被丢弃了：后台的发送任务不用再等。
    closed: Notify,
    book: Book,
    _in_use: [Gauge; 2],
}

impl Shared {
    fn is_revoked(&self) -> bool {
        *self.revoked.borrow()
    }

    fn check_revoked(&self, wait: &mut RevocationWait, cx: &mut Context<'_>) -> io::Result<()> {
        if self.is_revoked() || wait.as_mut().poll(cx).is_ready() {
            return Err(io::Error::new(io::ErrorKind::ConnectionReset, "对端的 lane 授权已经吊销"));
        }
        Ok(())
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().expect("lane stream mutex poisoned")
    }
}

enum SendState {
    /// 发起方还在写头（OPEN 连同首批数据），攒在这里，头写完时作为一个 OPENING 帧发出。
    Header(Vec<u8>),
    Sealed,
    Raw,
    /// 已经开始正常结束：close_notify（sealed 时）已经排进待发的密文，等它们发完再发 FIN。
    Closing,
    Finished,
    Aborted,
    /// 已经用 [`LaneSend::take_raw`] 交出去了。
    Handed,
}

enum RecvState {
    /// 在 TLS 里，里面是收到了、还没交出去的数据。
    Sealed(Bytes),
    /// 对端的方向已经离开 TLS，本端还没读完对端的头（[`RecvStream::end_header`]）。`sealed` 是 TLS 里收到、还没交出去的最后一点，
    /// `raw` 是那之后已经从 socket 读进来的原始字节。这时只交出 `sealed`：它读完了头还没完，就是对端不守协议，
    /// 不能拿未经认证的原始字节去凑对端的头。头读完时并成 [`RecvState::Raw`]。
    Leaving {
        sealed: Bytes,
        raw: Bytes,
    },
    /// 原始字节，里面是已经从 socket 读进来、还没交出去的部分。
    Raw(Bytes),
    Eof,
    /// 本端叫停了，之后收到的都扔掉。
    Stopped,
    Failed(Failure),
    Handed,
}

/// 接收方向异常结束的原因。`io::Error` 不能复制，这里存够重建它的东西。
#[derive(Debug, Clone)]
enum Failure {
    Reset(StreamCode),
    Protocol(String),
    Truncated,
    Io(io::ErrorKind, String),
}

impl Failure {
    fn to_io(&self) -> io::Error {
        match self {
            Self::Reset(code) => StreamAborted::Reset(*code).into(),
            Self::Protocol(what) => io::Error::new(io::ErrorKind::InvalidData, format!("lane 上的对端不守协议：{what}")),
            Self::Truncated => io::Error::new(io::ErrorKind::UnexpectedEof, "lane 在 close_notify 之前就结束了：数据被截断"),
            Self::Io(kind, message) => io::Error::new(*kind, message.clone()),
        }
    }

    fn of(error: &io::Error) -> Self {
        Self::Io(error.kind(), error.to_string())
    }
}

struct State {
    role: Role,
    /// 这条流的负载方式。发起方在头写完之前还没定。
    mode: Option<Mode>,
    /// lane 谈定的版本。发起方按它选负载方式。
    version: Version,
    /// 本端发过 SWITCH。接受方要先发过，才收得了发起方的 SWITCH（那是回应）。
    switched: bool,
    /// TLS 状态。两个方向都离开 TLS 之后就扔掉，静默的 passthrough 流不再占它的内存。
    sealed: Option<Sealed>,
    /// 待发的密文。
    out: BytesMut,
    send: SendState,
    recv: RecvState,
    /// 本端读完了对端的头（[`RecvStream::end_header`]，或者交出了原始的接收方向）：之后接收方向离开 TLS 时可以接着读原始字节。
    header_read: bool,
    /// 对端叫停了本端的发送（STOP），或者异常结束了整条流（ABORT）：本端之后的写都返回它。
    send_refused: Option<StreamAborted>,
    /// 写在等的任务。对端叫停时要叫醒它，不然它会一直等一个不会再变得可写的 socket。
    send_waker: Option<Waker>,
    flushing: bool,
    halves: u8,
    raw_taken: bool,
    /// 本端或对端异常结束了这条流：关闭时发 RST。
    aborted: bool,
    /// 本端用 ABORT 帧重置了发送方向，这个帧已经全部交给了内核：关闭之前要等对端读到它（[`close_after_abort`]）。
    abort_sent: bool,
    heard_from_peer: bool,
    /// 往 socket 写的时候出过错：连接已经断了，多半是对端重置了。Linux 上一次重置只报一次错，被写拿走之后读只看到 EOF，
    /// 所以记在这里：之后读到 EOF 时按它异常结束，不当作对端正常结束了（见 [`State::drain_now`]）。
    write_failed: Option<Failure>,
}

impl LaneSend {
    /// 对端节点。
    #[must_use]
    pub fn peer(&self) -> NodeId {
        self.shared.peer
    }
}

impl LaneRecv {
    pub(crate) fn is_revoked(&self) -> bool {
        self.shared.is_revoked()
    }

    /// 这个方向已经离开 TLS 时，把它交出去，让调用方直接读 socket（见 [`RawRead`]）。还在 TLS 里、已经结束或者已经交出去时返回 `None`。
    /// 不等待；要等对端的切换用 [`RecvStream::wait_raw`]。
    ///
    /// 交出去之后这个 [`LaneRecv`] 不再能读，丢弃它也不会叫停流。
    ///
    /// 已经收到、还没处理的记录先处理掉（不等待）：接受方的 REPLY 和 SWITCH 往往一起到，读完 REPLY 时 SWITCH 已经在缓冲里了。
    ///
    /// 要原始字节说明调用方已经读完了对端的头：交得出来时同时算作 [`RecvStream::end_header`]。
    pub fn take_raw(&mut self) -> Option<RawRead> {
        let shared = &self.shared;
        let mut state = shared.lock();
        if shared.is_revoked() {
            return None;
        }
        while matches!(&state.recv, RecvState::Sealed(data) if data.is_empty()) {
            if !matches!(state.poll_sealed_step(&shared.socket, Read::Now), Poll::Ready(Ok(()))) {
                break;
            }
        }
        if !state.out.is_empty() {
            push(shared, &mut state);
        }
        if matches!(state.recv, RecvState::Leaving { .. }) {
            state.finish_header();
        }
        // 写的时候已经知道连接断了：交出去的话，搬运的一方读到的只是 EOF。留在这里读，读到 EOF 时报那个错误。
        if state.write_failed.is_some() {
            return None;
        }
        let RecvState::Raw(pending) = &mut state.recv else { return None };
        let received = std::mem::take(pending);
        state.recv = RecvState::Handed;
        state.raw_taken = true;
        state.drop_tls_if_unused();
        Some(RawRead::new(Arc::clone(&shared.socket), received, shared.book.spliced()).with_revocation(shared.revoked.clone()))
    }

    /// 这条流的负载方式。发起方在头写完之前是 `None`。
    #[must_use]
    pub fn payload(&self) -> Option<Payload> {
        self.shared.lock().mode.map(Mode::payload)
    }
}

/// 把一条刚取到的 lane 变成本端发起的一条流。
pub(crate) fn initiate(lane: Lane, book: Book, revoked: watch::Receiver<bool>) -> (LaneSend, LaneRecv) {
    let Lane { socket, mut sealed, out, peer, version, .. } = lane;
    sealed.shrink();
    socket::rst_on_close(&socket);
    socket::set_user_timeout(&socket, socket::OPENING_USER_TIMEOUT);
    let state = State {
        role: Role::Initiator,
        mode: None,
        version,
        switched: false,
        sealed: Some(sealed),
        out,
        send: SendState::Header(Vec::new()),
        recv: RecvState::Sealed(Bytes::new()),
        header_read: false,
        send_refused: None,
        send_waker: None,
        flushing: false,
        halves: 2,
        raw_taken: false,
        aborted: false,
        abort_sent: false,
        heard_from_peer: false,
        write_failed: None,
    };
    split(socket, peer, state, book, revoked)
}

/// 对端在一条 lane 上开过来的流：它的 OPENING 已经读到了，`header` 是里面流的头。
///
/// [`Mode::Early`] 时 OPENING 所在的记录之后就是原始字节，调用方已经确认这条记录在这个帧的末尾结束。
/// 流层的 OPEN 只能从 `header` 里读，读完之前不接着读后面的原始字节（[`RecvState::Leaving`]）。
pub(crate) fn respond(lane: Lane, mode: Mode, header: &[u8], book: Book, revoked: watch::Receiver<bool>) -> (LaneSend, LaneRecv) {
    let Lane { socket, mut sealed, out, peer, version, .. } = lane;
    socket::rst_on_close(&socket);
    socket::set_user_timeout(&socket, socket::STREAM_USER_TIMEOUT);
    let recv = match mode {
        Mode::Sealed | Mode::Mutual => RecvState::Sealed(compact(header)),
        Mode::Early => RecvState::Leaving { sealed: compact(header), raw: compact(&sealed.take_inbound()) },
    };
    let state = State {
        role: Role::Responder,
        mode: Some(mode),
        version,
        switched: false,
        sealed: Some(sealed),
        out,
        send: SendState::Sealed,
        recv,
        header_read: false,
        send_refused: None,
        send_waker: None,
        flushing: false,
        halves: 2,
        raw_taken: false,
        aborted: false,
        abort_sent: false,
        heard_from_peer: true,
        write_failed: None,
    };
    split(socket, peer, state, book, revoked)
}

fn split(socket: Arc<TcpStream>, peer: NodeId, state: State, book: Book, revoked: watch::Receiver<bool>) -> (LaneSend, LaneRecv) {
    let shared = Arc::new(Shared {
        socket,
        revoked,
        peer,
        state: Mutex::new(state),
        closed: Notify::new(),
        _in_use: book.enter(|tally| &tally.in_use),
        book,
    });
    let send = LaneSend { shared: Arc::clone(&shared), wait_revoked: Box::pin(wait_revoked(shared.revoked.clone())) };
    let recv = LaneRecv { wait_revoked: Box::pin(wait_revoked(shared.revoked.clone())), shared };
    (send, recv)
}

impl State {
    fn end_header(&mut self, payload: Payload) {
        match (&mut self.send, self.role) {
            (SendState::Header(header), Role::Initiator) => {
                let header = Bytes::from(std::mem::take(header));
                // 没升级的接受方不认"等 SWITCH"，passthrough 在那里只能是 OPENING 之后马上离开 TLS：不那样做，见 [`Mode::Early`]。
                let mode = match (payload, self.version) {
                    (Payload::Passthrough, Version::V2) => Mode::Mutual,
                    (Payload::Passthrough, Version::V1) | (Payload::Sealed, _) => Mode::Sealed,
                };
                self.mode = Some(mode);
                let sealed = self.sealed.as_mut().expect("头还没发出去时 TLS 状态一定还在");
                sealed.seal_frame(&Frame::Opening { mode, header });
                sealed.take_output(&mut self.out);
                self.send = SendState::Sealed;
            }
            (SendState::Sealed, Role::Responder)
                if payload == Payload::Passthrough && self.mode.is_some_and(|mode| mode != Mode::Sealed) =>
            {
                self.switch();
            }
            _ => {}
        }
    }

    /// 发 SWITCH：这个方向在它所在的记录之后离开 TLS。调用方确认过这个方向还在 TLS 里（[`SendState::Sealed`]）。
    fn switch(&mut self) {
        let sealed = self.sealed.as_mut().expect("sealed 的方向一定还有 TLS 状态");
        sealed.seal_frame(&Frame::Switch);
        sealed.take_output(&mut self.out);
        self.send = SendState::Raw;
        self.switched = true;
        self.drop_tls_if_unused();
    }

    /// 现在收到对端的 SWITCH 合不合协议。只有 [`Mode::Mutual`] 的流上会有：接受方的随时可以来（本端发起的流不再用 [`Mode::Early`]），
    /// 发起方的只能是对本端那个的回应。
    fn expects_switch(&self) -> bool {
        self.mode == Some(Mode::Mutual) && (self.role == Role::Initiator || self.switched)
    }

    /// 本端读完了对端的头：之后可以从 TLS 里收到的字节接着读原始字节。
    fn finish_header(&mut self) {
        self.header_read = true;
        if let RecvState::Leaving { sealed, raw } = &mut self.recv {
            let pending = match (sealed.is_empty(), raw.is_empty()) {
                (true, _) => std::mem::take(raw),
                (false, true) => std::mem::take(sealed),
                (false, false) => compact(&[&sealed[..], &raw[..]].concat()),
            };
            self.recv = RecvState::Raw(pending);
        }
    }

    /// 两个方向都不再需要 TLS 时扔掉它。
    fn drop_tls_if_unused(&mut self) {
        let send_needs = matches!(self.send, SendState::Header(_) | SendState::Sealed);
        let recv_needs = matches!(self.recv, RecvState::Sealed(_));
        if !send_needs && !recv_needs {
            self.sealed = None;
        }
    }

    /// 尽量把待发的密文交给内核，不等待。全部交完返回 `true`。
    ///
    /// 写出错时记下来（[`State::write_failed`]）：这里的写有时是读的那一侧顺手做的（回应对端的 SWITCH），或者在后台，
    /// 错误没有人接，读的那一侧却要靠它知道连接是被重置的。
    fn drain_now(&mut self, socket: &TcpStream) -> io::Result<bool> {
        while !self.out.is_empty() {
            match socket.try_write(&self.out) {
                Ok(written) => self.out.advance(written),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(false),
                Err(error) => {
                    self.write_failed.get_or_insert_with(|| Failure::of(&error));
                    return Err(error);
                }
            }
        }
        if self.out.capacity() > 0 {
            self.out = BytesMut::new();
        }
        Ok(true)
    }

    /// 把待发的密文交给内核，交不完就等 socket 可写。
    fn poll_drain(&mut self, socket: &TcpStream, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        loop {
            if self.drain_now(socket)? {
                return Poll::Ready(Ok(()));
            }
            self.send_waker = Some(cx.waker().clone());
            ready!(socket.poll_write_ready(cx))?;
        }
    }

    /// 第一次听到对端（发起方开流之后）：确认 lane 是活的，换成流里的 `TCP_USER_TIMEOUT`。
    fn heard(&mut self, socket: &TcpStream) {
        if !self.heard_from_peer {
            self.heard_from_peer = true;
            socket::set_user_timeout(socket, socket::STREAM_USER_TIMEOUT);
        }
    }

    fn fail_recv(&mut self, failure: Failure) -> io::Error {
        let error = failure.to_io();
        self.recv = RecvState::Failed(failure);
        self.aborted = true;
        error
    }

    fn wake_sender(&mut self) {
        if let Some(waker) = self.send_waker.take() {
            waker.wake();
        }
    }

    /// TLS 处理收到的记录时可能要回点什么（比如更新密钥）。本端的发送方向还在 TLS 里才发，否则扔掉：
    /// 原始字节的方向上不能插进 TLS 的字节。
    fn collect_tls_output(&mut self) {
        let Some(sealed) = self.sealed.as_mut() else { return };
        if matches!(self.send, SendState::Header(_) | SendState::Sealed | SendState::Closing) {
            sealed.take_output(&mut self.out);
        } else {
            sealed.take_output(&mut BytesMut::new());
        }
    }

    /// sealed 的接收方向往前走一步：交出数据、处理一个帧，或者从 socket 读更多。
    fn poll_sealed_step(&mut self, socket: &TcpStream, read: Read<'_, '_>) -> Poll<io::Result<()>> {
        let sealed = self.sealed.as_mut().expect("sealed 的方向一定还有 TLS 状态");
        let next = sealed.next_frame();
        let at_boundary = sealed.at_record_boundary();
        self.collect_tls_output();
        match next {
            Ok(Next::Frame(Frame::Data(data))) => {
                self.heard(socket);
                self.recv = RecvState::Sealed(data);
            }
            Ok(Next::Frame(Frame::Switch)) => {
                if !self.expects_switch() || !at_boundary {
                    return Poll::Ready(Err(self.fail_recv(Failure::Protocol("不该出现的 SWITCH".to_owned()))));
                }
                self.heard(socket);
                let leftover = self.sealed.as_mut().map(Sealed::take_inbound).unwrap_or_default();
                // 流层的 REPLY 要在 SWITCH 之前完整地送到：还没读完它时，后面的原始字节先不交出去。
                self.recv = RecvState::Leaving { sealed: Bytes::new(), raw: compact(&leftover) };
                if self.header_read {
                    self.finish_header();
                }
                // 接受方看过目标的回答、先切换了：发起方回一个 SWITCH，自己的方向跟着离开 TLS。已经在结束、已经重置的方向不用跟。
                // 回应排在待发的密文里，调用方随后送出（[`push`]）。
                if self.role == Role::Initiator && matches!(self.send, SendState::Sealed) {
                    self.switch();
                }
                self.drop_tls_if_unused();
            }
            Ok(Next::Frame(Frame::Stop(code))) => {
                self.heard(socket);
                self.send_refused = Some(StreamAborted::Stopped(StreamCode(code)));
                self.wake_sender();
            }
            Ok(Next::Frame(Frame::Abort(code))) => {
                self.heard(socket);
                self.send_refused = Some(StreamAborted::Stopped(StreamCode(code)));
                self.wake_sender();
                return Poll::Ready(Err(self.fail_recv(Failure::Reset(StreamCode(code)))));
            }
            Ok(Next::Frame(other)) => {
                return Poll::Ready(Err(self.fail_recv(Failure::Protocol(format!("流上不该出现的帧 {other:?}")))));
            }
            Ok(Next::Closed) => {
                self.heard(socket);
                self.recv = RecvState::Eof;
                self.drop_tls_if_unused();
            }
            Ok(Next::More) => {
                let sealed = self.sealed.as_mut().expect("同上");
                sealed.inbound.reserve(READ_CHUNK);
                match socket.try_read_buf(&mut sealed.inbound) {
                    Ok(0) => return Poll::Ready(Err(self.fail_recv(Failure::Truncated))),
                    Ok(_) => {}
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        sealed.shrink();
                        let Read::Wait(cx) = read else { return Poll::Pending };
                        ready!(socket.poll_read_ready(cx)).map_err(|error| self.fail_recv(Failure::of(&error)))?;
                    }
                    Err(error) => return Poll::Ready(Err(self.fail_recv(Failure::of(&error)))),
                }
            }
            Err(error) => return Poll::Ready(Err(self.fail_recv(Failure::Protocol(error.to_string())))),
        }
        Poll::Ready(Ok(()))
    }
}

/// 从 `data` 开头取出能放进 `buf` 的字节。取空了就换成一个空的 `Bytes`：
/// 切出来的 `Bytes` 和整块读缓冲共用内存，空着也会让那一整块（几十 KB）一直留着，几千条静默的流就是几百 MB。
fn take_into(data: &mut Bytes, buf: &mut ReadBuf<'_>) -> usize {
    let len = data.len().min(buf.remaining());
    buf.put_slice(&data.split_to(len));
    if data.is_empty() {
        *data = Bytes::new();
    }
    len
}

/// 切换成原始字节时多读进来的那一点，拷进一块大小正好的内存，不留着整块读缓冲。
fn compact(leftover: &[u8]) -> Bytes {
    if leftover.is_empty() { Bytes::new() } else { Bytes::copy_from_slice(leftover) }
}

/// 读 socket 读不到东西时怎么办。
enum Read<'a, 'b> {
    /// 登记这个任务，等 socket 可读。
    Wait(&'a mut Context<'b>),
    /// 不等，就此返回 `Pending`（不登记任何人）。
    Now,
}

/// 把待发的密文交给内核；交不完时让后台任务等 socket 可写再送，调用方不必再管。
fn push(shared: &Arc<Shared>, state: &mut State) {
    if shared.is_revoked() {
        return;
    }
    match state.drain_now(&shared.socket) {
        Ok(true) => {}
        Ok(false) => {
            if !state.flushing
                && let Ok(runtime) = tokio::runtime::Handle::try_current()
            {
                state.flushing = true;
                drop(runtime.spawn(flush_later(Arc::clone(shared))));
            }
        }
        // 写出错时连接已经坏了：下一次读写会看到错误，这里不用做什么。
        Err(error) => tracing::debug!(peer = %shared.peer, %error, "lane 的密文没有写出去"),
    }
}

/// 后台送出待发的密文。只在写的一方已经返回、没有人会再来推一把时才需要。
async fn flush_later(shared: Arc<Shared>) {
    let revoked = wait_revoked(shared.revoked.clone());
    tokio::pin!(revoked);
    loop {
        {
            let mut state = shared.lock();
            if shared.is_revoked() || state.halves == 0 || !matches!(state.drain_now(&shared.socket), Ok(false)) {
                break;
            }
        }
        tokio::select! {
            biased;
            () = &mut revoked => break,
            () = shared.closed.notified() => break,
            ready = shared.socket.writable() => if ready.is_err() { break; },
        }
    }
    let mut state = shared.lock();
    state.flushing = false;
    state.wake_sender();
}

impl AsyncRead for LaneRecv {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let shared = &this.shared;
        let mut state = shared.lock();
        shared.check_revoked(&mut this.wait_revoked, cx)?;
        if state.role == Role::Initiator && matches!(state.send, SendState::Header(_)) {
            // 发起方开始等回复了，头一定已经写完。
            state.end_header(Payload::Sealed);
            push(shared, &mut state);
        }
        loop {
            match &mut state.recv {
                RecvState::Failed(failure) => return Poll::Ready(Err(failure.to_io())),
                RecvState::Eof => return Poll::Ready(Ok(())),
                RecvState::Stopped => return Poll::Ready(Err(io::Error::new(io::ErrorKind::BrokenPipe, "这个方向已经叫停"))),
                RecvState::Handed => return Poll::Ready(Err(io::Error::other("这个方向已经交给调用方直接读"))),
                RecvState::Sealed(data) if !data.is_empty() => {
                    let len = take_into(data, buf);
                    shared.book.add(|tally| &tally.sealed_bytes, len);
                    return Poll::Ready(Ok(()));
                }
                RecvState::Leaving { sealed, .. } if !sealed.is_empty() => {
                    let len = take_into(sealed, buf);
                    shared.book.add(|tally| &tally.sealed_bytes, len);
                    return Poll::Ready(Ok(()));
                }
                // 本端还在读对端的头，TLS 里的字节却已经读完了：头没有完整地在 TLS 里。
                RecvState::Leaving { .. } => {
                    return Poll::Ready(Err(state.fail_recv(Failure::Protocol("流的头还没完，对端的方向就离开了 TLS".to_owned()))));
                }
                RecvState::Raw(pending) if !pending.is_empty() => {
                    let len = take_into(pending, buf);
                    shared.book.add(|tally| &tally.raw_bytes, len);
                    return Poll::Ready(Ok(()));
                }
                RecvState::Raw(_) => match shared.socket.try_read(buf.initialize_unfilled()) {
                    Ok(0) => {
                        if let Some(failure) = state.write_failed.clone() {
                            return Poll::Ready(Err(state.fail_recv(failure)));
                        }
                        state.recv = RecvState::Eof;
                        return Poll::Ready(Ok(()));
                    }
                    Ok(len) => {
                        buf.advance(len);
                        state.heard(&shared.socket);
                        shared.book.add(|tally| &tally.raw_bytes, len);
                        return Poll::Ready(Ok(()));
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        ready!(shared.socket.poll_read_ready(cx)).map_err(|error| state.fail_recv(Failure::of(&error)))?;
                    }
                    Err(error) => return Poll::Ready(Err(state.fail_recv(Failure::of(&error)))),
                },
                RecvState::Sealed(_) => {
                    ready!(state.poll_sealed_step(&shared.socket, Read::Wait(cx)))?;
                    if !state.out.is_empty() {
                        push(shared, &mut state);
                    }
                }
            }
        }
    }
}

impl AsyncWrite for LaneSend {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, data: &[u8]) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        let shared = &this.shared;
        let mut state = shared.lock();
        shared.check_revoked(&mut this.wait_revoked, cx)?;
        if let Some(refused) = state.send_refused {
            return Poll::Ready(Err(refused.into()));
        }
        if data.is_empty() {
            return Poll::Ready(Ok(0));
        }
        loop {
            match &mut state.send {
                SendState::Header(header) => {
                    if header.len() + data.len() <= MAX_HEADER_LEN {
                        header.extend_from_slice(data);
                        return Poll::Ready(Ok(data.len()));
                    }
                    // 头不会这么长：按 sealed 结束它，后面的照常作为数据发。
                    state.end_header(Payload::Sealed);
                }
                SendState::Sealed => {
                    ready!(state.poll_drain(&shared.socket, cx))?;
                    let len = data.len().min(MAX_DATA_LEN);
                    let State { sealed, out, .. } = &mut *state;
                    let sealed = sealed.as_mut().expect("sealed 的方向一定还有 TLS 状态");
                    sealed.seal(&[&data_header(len), &data[..len]]);
                    sealed.take_output(out);
                    shared.book.add(|tally| &tally.sealed_bytes, len);
                    push(shared, &mut state);
                    return Poll::Ready(Ok(len));
                }
                SendState::Raw => {
                    ready!(state.poll_drain(&shared.socket, cx))?;
                    match shared.socket.try_write(data) {
                        Ok(len) => {
                            shared.book.add(|tally| &tally.raw_bytes, len);
                            return Poll::Ready(Ok(len));
                        }
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                            state.send_waker = Some(cx.waker().clone());
                            ready!(shared.socket.poll_write_ready(cx))?;
                        }
                        Err(error) => {
                            state.write_failed.get_or_insert_with(|| Failure::of(&error));
                            return Poll::Ready(Err(error));
                        }
                    }
                }
                SendState::Closing | SendState::Finished => {
                    return Poll::Ready(Err(io::Error::new(io::ErrorKind::BrokenPipe, "这个方向已经正常结束")));
                }
                SendState::Aborted => return Poll::Ready(Err(io::Error::new(io::ErrorKind::BrokenPipe, "这个方向已经重置"))),
                SendState::Handed => return Poll::Ready(Err(io::Error::other("这个方向已经交给调用方直接写"))),
            }
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let shared = &this.shared;
        let mut state = shared.lock();
        shared.check_revoked(&mut this.wait_revoked, cx)?;
        if matches!(state.send, SendState::Header(_)) {
            state.end_header(Payload::Sealed);
        }
        state.poll_drain(&shared.socket, cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let shared = &this.shared;
        let mut state = shared.lock();
        shared.check_revoked(&mut this.wait_revoked, cx)?;
        loop {
            match state.send {
                SendState::Header(_) => state.end_header(Payload::Sealed),
                SendState::Sealed => {
                    let State { sealed, out, .. } = &mut *state;
                    let sealed = sealed.as_mut().expect("sealed 的方向一定还有 TLS 状态");
                    sealed.close_notify();
                    sealed.take_output(out);
                    state.send = SendState::Closing;
                }
                SendState::Raw => state.send = SendState::Closing,
                SendState::Closing => {
                    ready!(state.poll_drain(&shared.socket, cx))?;
                    shared.check_revoked(&mut this.wait_revoked, cx)?;
                    socket::finish_write(&shared.socket)?;
                    state.send = SendState::Finished;
                    state.drop_tls_if_unused();
                    return Poll::Ready(Ok(()));
                }
                SendState::Finished => return Poll::Ready(Ok(())),
                SendState::Aborted => return Poll::Ready(Err(io::Error::new(io::ErrorKind::BrokenPipe, "这个方向已经重置"))),
                SendState::Handed => return Poll::Ready(Err(io::Error::other("这个方向已经交给调用方直接写"))),
            }
        }
    }
}

impl SendStream for LaneSend {
    fn reset(&mut self, code: StreamCode) {
        let shared = &self.shared;
        let mut state = shared.lock();
        reset(shared, &mut state, code);
    }

    /// 头写完了，或者接受方这个方向可以不再加密了。
    ///
    /// - 发起方：OPEN 连同首批数据写完了。`payload` 选定这条流的负载方式，攒下的头作为一个 OPENING 帧送出。
    ///   选了 [`Payload::Passthrough`] 时这个方向还留在 TLS 里，等接受方的 SWITCH：收到时（在读的那一侧）回一个 SWITCH，
    ///   之后这个方向是原始字节，[`take_raw`](SendStream::take_raw) 交得出来。对端是没升级的节点时按 sealed 送出。
    /// - 接受方：发起方选了 passthrough、并且这里也传 [`Payload::Passthrough`] 时，发一个 SWITCH，之后这个方向是原始字节；
    ///   别的情况什么都不做，这个方向留在 TLS 里。在这之前可以已经发过任意多的数据（流层先加密地送出目标的 ServerHello）。
    ///
    /// 不用等：密文立刻交给内核，交不完的由后台送出。头已经结束之后再调用没有效果。
    ///
    /// 发起方不调它也能用，只是一直是 sealed：开始读（等回复）、`flush`、`shutdown`，或者头超过上限时，
    /// 攒下的头按 sealed 送出。
    fn end_header(&mut self, payload: Payload) {
        let mut state = self.shared.lock();
        if self.shared.is_revoked() {
            return;
        }
        state.end_header(payload);
        push(&self.shared, &mut state);
    }

    /// 这个方向已经离开 TLS 时，把它交出去，让调用方直接写 socket（见 [`RawWrite`]）。还在 TLS 里、已经结束或者已经交出去时返回 `None`。
    ///
    /// 交出去之后这个 [`LaneSend`] 不再能写，丢弃它也不会重置流。
    fn take_raw(&mut self) -> Option<RawWrite> {
        let shared = &self.shared;
        let mut state = shared.lock();
        if shared.is_revoked() || !matches!(state.send, SendState::Raw) || state.aborted {
            return None;
        }
        state.send = SendState::Handed;
        state.raw_taken = true;
        let unsent = std::mem::take(&mut state.out).freeze();
        state.drop_tls_if_unused();
        Some(RawWrite::new(Arc::clone(&shared.socket), unsent, shared.book.spliced()).with_revocation(shared.revoked.clone()))
    }
}

fn reset(shared: &Shared, state: &mut State, code: StreamCode) {
    if shared.is_revoked() {
        return;
    }
    let raw = match state.send {
        SendState::Finished | SendState::Aborted | SendState::Handed => return,
        // 头还没发出去：对端还不知道有这条流，重置连接就够了。
        SendState::Header(_) | SendState::Closing => false,
        SendState::Sealed => {
            let sealed = state.sealed.as_mut().expect("sealed 的方向一定还有 TLS 状态");
            sealed.seal_frame(&Frame::Abort(code.0));
            sealed.take_output(&mut state.out);
            // 尽力而为：ABORT 进得了内核就送得到，进不去对端也会看到 RST。
            state.abort_sent = matches!(state.drain_now(&shared.socket), Ok(true));
            false
        }
        SendState::Raw => true,
    };
    state.send = SendState::Aborted;
    state.aborted = true;
    socket::rst_on_close(&shared.socket);
    if raw {
        socket::reset_now(&shared.socket);
    }
}

impl RecvStream for LaneRecv {
    fn stop(&mut self, code: StreamCode) {
        let shared = &self.shared;
        let mut state = shared.lock();
        stop(shared, &mut state, code);
    }

    fn revocation(&self) -> Option<watch::Receiver<bool>> {
        Some(self.shared.revoked.clone())
    }

    /// 读完了对端的头（OPEN 或 REPLY）。在这之前接收方向只交出 TLS 里收到的字节，见 [`RecvState::Leaving`]。
    fn end_header(&mut self) {
        self.shared.lock().finish_header();
    }

    /// 等到知道这个方向接下来是什么：离开了 TLS（对端发了 SWITCH，或者没升级的发起方选了 passthrough）就交出来，和
    /// [`take_raw`](Self::take_raw) 一样；接下来是 TLS 里的数据、或者这个方向结束了、出错了，返回 `None`，调用方照常读。
    ///
    /// 发起方读完 REPLY 之后用它：接受方紧接着发 SWITCH，但它可能还在路上。收到时发起方的发送方向也跟着切换（见
    /// [`LaneSend::end_header`]）。接受方在它自己的 SWITCH 之后用它等发起方的回应。
    async fn wait_raw(&mut self) -> Option<RawRead> {
        // 一直加密的流，哪个方向都不会离开 TLS：直接返回，不去推进 TLS、也不再为 `take_raw` 拿一次锁。
        // 流层每读一段之前都会问一次（i3t-flow 的 `flow_to_tcp`），这样加密的流上每段只多拿一次锁。
        if self.shared.lock().mode == Some(Mode::Sealed) {
            return None;
        }
        std::future::poll_fn(|cx| {
            let shared = &self.shared;
            let mut state = shared.lock();
            if shared.check_revoked(&mut self.wait_revoked, cx).is_err() {
                return Poll::Ready(());
            }
            loop {
                match &state.recv {
                    RecvState::Raw(_) | RecvState::Leaving { .. } => break,
                    RecvState::Sealed(data) if data.is_empty() => {
                        let step = state.poll_sealed_step(&shared.socket, Read::Wait(cx));
                        if !state.out.is_empty() {
                            push(shared, &mut state);
                        }
                        match step {
                            Poll::Ready(Ok(())) => {}
                            Poll::Ready(Err(_)) => return Poll::Ready(()),
                            Poll::Pending => return Poll::Pending,
                        }
                    }
                    _ => return Poll::Ready(()),
                }
            }
            Poll::Ready(())
        })
        .await;
        self.take_raw()
    }
}

fn stop(shared: &Arc<Shared>, state: &mut State, code: StreamCode) {
    if shared.is_revoked() || matches!(state.recv, RecvState::Eof | RecvState::Stopped | RecvState::Failed(_) | RecvState::Handed) {
        return;
    }
    state.recv = RecvState::Stopped;
    match state.send {
        SendState::Sealed => {
            let sealed = state.sealed.as_mut().expect("sealed 的方向一定还有 TLS 状态");
            sealed.seal_frame(&Frame::Stop(code.0));
            sealed.take_output(&mut state.out);
            push(shared, state);
        }
        // 对端还不知道有这条流。
        SendState::Header(_) | SendState::Aborted | SendState::Handed => {}
        // 没有办法在流里告诉对端了：只能重置整条 lane。
        SendState::Raw | SendState::Closing | SendState::Finished => {
            state.aborted = true;
            socket::reset_now(&shared.socket);
        }
    }
    state.drop_tls_if_unused();
}

impl Drop for LaneSend {
    fn drop(&mut self) {
        let shared = &self.shared;
        let mut state = shared.lock();
        if !matches!(state.send, SendState::Finished | SendState::Aborted | SendState::Handed) {
            reset(shared, &mut state, DROPPED_SEND);
        }
        release(shared, &mut state);
    }
}

impl Drop for LaneRecv {
    fn drop(&mut self) {
        let shared = &self.shared;
        let mut state = shared.lock();
        stop(shared, &mut state, DROPPED_RECV);
        release(shared, &mut state);
    }
}

/// 一个方向的句柄被丢弃。两个都丢了时决定关闭方式。
fn release(shared: &Arc<Shared>, state: &mut State) {
    state.halves -= 1;
    if state.halves > 0 {
        return;
    }
    shared.closed.notify_one();
    if state.raw_taken {
        // 原始方向的搬运方监听同一个吊销信号，并负责重置 socket。
        return;
    }
    if shared.is_revoked() {
        i3t_underlay::tcp_reset::close(Arc::clone(&shared.socket));
        return;
    }
    let send_clean = matches!(state.send, SendState::Finished);
    match (&state.recv, send_clean, state.aborted) {
        (RecvState::Eof, true, false) => socket::graceful_close(&shared.socket),
        (RecvState::Stopped, true, false) => linger_then_close(shared),
        _ if state.abort_sent => close_after_abort(shared),
        // 异常结束。发送方向可能正常结束过，macOS 上那时已经改回了正常关闭（`socket::finish_write`）：重新设成关闭时发 RST。
        _ => socket::rst_on_close(&shared.socket),
    }
}

/// 本端已经正常写完、但叫停了对端：先把对端还在路上的数据收下扔掉，再正常关闭（见 [`LINGER_AFTER_STOP`]）。
fn linger_then_close(shared: &Arc<Shared>) {
    let socket = Arc::clone(&shared.socket);
    let Ok(runtime) = tokio::runtime::Handle::try_current() else { return };
    socket::graceful_close(&socket);
    drop(runtime.spawn(async move {
        let _ = tokio::time::timeout(LINGER_AFTER_STOP, discard(&socket)).await;
    }));
}

/// 本端发了 ABORT 帧、两个方向都丢弃了：发 FIN，把对端还在路上的数据收下扔掉，等对端关掉（最多 [`LINGER_AFTER_STOP`]）再关。
/// socket 还设着"关闭时发 RST"，那时对端看到的是重置。见模块文档的异常结束一节。
///
/// 对端读到 ABORT 之后会关掉它那一头；两头都发了 ABORT 时，各自的 FIN 让对方的等待马上结束。
fn close_after_abort(shared: &Arc<Shared>) {
    let socket = Arc::clone(&shared.socket);
    let Ok(runtime) = tokio::runtime::Handle::try_current() else { return };
    if let Err(error) = socket::shutdown_write(&socket) {
        tracing::debug!(%error, "ABORT 之后发不了 FIN，对端读完 ABORT 之后要等关闭时的 RST");
    }
    drop(runtime.spawn(async move {
        let _ = tokio::time::timeout(LINGER_AFTER_STOP, discard(&socket)).await;
    }));
}

/// 把收到的都读出来扔掉，直到对端结束或者出错。
async fn discard(socket: &TcpStream) {
    let mut scratch = [0; 4096];
    loop {
        if socket.readable().await.is_err() {
            return;
        }
        match socket.try_read(&mut scratch) {
            Ok(0) => return,
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
            Err(_) => return,
        }
    }
}

#[cfg(test)]
mod tests;
