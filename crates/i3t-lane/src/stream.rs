//! 一条 lane 上的一条流：[`LaneSend`] 和 [`LaneRecv`]，实现连接层契约里的 [`SendStream`] 和 [`RecvStream`]。
//!
//! # 两种负载方式
//!
//! 每个方向开始时都在 lane 的 TLS 里面（帧见 crate 文档）。发起方在头写完时（[`LaneSend::end_header`]）选定负载方式：
//!
//! - sealed：两个方向一直在 TLS 里，数据是 DATA 帧，正常结束是 close_notify 加 FIN。没有 close_notify 的 FIN 是截断，按异常结束算。
//! - passthrough：发起方的方向在 OPENING 所在的记录之后离开 TLS，接受方的方向在它的 SWITCH 所在的记录之后离开 TLS
//!   （接受方写完 REPLY 后调 [`LaneSend::end_header`] 时发出；不调就一直留在 TLS 里，对端照样读得懂）。
//!   离开之后是用户的原始字节，FIN 就是结束，RST 或者出错就是异常结束。
//!
//! # 异常结束
//!
//! lane 是一整条只属于这条流的 TCP，所以任何一个方向的异常结束都让整条 lane 异常结束：
//!
//! | 本端做了什么 | 对端看到什么 |
//! |---|---|
//! | [`SendStream::reset`]，这个方向还在 TLS 里 | ABORT 帧（带原因码），随后 RST |
//! | [`SendStream::reset`]，这个方向已经是原始字节 | RST（Linux 上立刻发，别处在关闭时） |
//! | [`RecvStream::stop`]，本端的发送方向还在 TLS 里 | STOP 帧（带原因码），对端之后的写出错；lane 不断，本端的发送照常 |
//! | [`RecvStream::stop`]，本端的发送方向已经不在 TLS 里 | RST |
//! | 没有 `shutdown` 就丢弃发送方向 | 同 `reset` |
//!
//! lane 从开流起就设成"关闭时发 RST"，只有两个方向都正常结束才改回去，所以任务被取消、进程退出也一样。
//! 收到的 RST 在读写时是一个不带原因码的连接错误（分不清是对端重置了流，还是路上断了），ABORT 帧是带原因码的
//! [`StreamAborted::Reset`]，STOP 帧让本端之后的写返回 [`StreamAborted::Stopped`]。
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
use i3t_conn::{Payload, RawRead, RawWrite, RecvStream, SendStream, StreamAborted, StreamCode};
use i3t_proto::NodeId;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio::sync::Notify;

use crate::frame::{Frame, MAX_DATA_LEN, MAX_HEADER_LEN, data_header};
use crate::lane::Lane;
use crate::sealed::{Next, Sealed};
use crate::socket;
use crate::status::{Book, Gauge};

/// 发送方向没有正常结束就被丢弃时，对端看到的原因码。和 QUIC 的实现取同一个数。
const DROPPED_SEND: StreamCode = StreamCode(u32::MAX);
/// 接收方向没读完就被丢弃时，对端看到的原因码。同上。
const DROPPED_RECV: StreamCode = StreamCode(0);

/// 一次从 socket 读多少（sealed 的方向）。
const READ_CHUNK: usize = 64 * 1024;

/// 本端叫停了对端、自己这一侧已经正常写完时，关闭之前最多再收多久（收到就扔）：
/// 关闭时接收缓冲里还有没读的数据，内核会发 RST 并丢掉还没发出去的数据，刚写完的 REPLY 可能就此丢了。
const LINGER_AFTER_STOP: Duration = Duration::from_secs(5);

/// 这一端在流里是哪一方。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    Initiator,
    Responder,
}

/// 一条流的发送方向。实现 [`SendStream`]。
pub struct LaneSend {
    shared: Arc<Shared>,
}

/// 一条流的接收方向。实现 [`RecvStream`]。
pub struct LaneRecv {
    shared: Arc<Shared>,
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
    /// 两个方向都被丢弃了：后台的发送任务不用再等。
    closed: Notify,
    book: Book,
    _in_use: [Gauge; 2],
}

impl Shared {
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
    payload: Option<Payload>,
    /// TLS 状态。两个方向都离开 TLS 之后就扔掉，静默的 passthrough 流不再占它的内存。
    sealed: Option<Sealed>,
    /// 待发的密文。
    out: BytesMut,
    send: SendState,
    recv: RecvState,
    /// 对端叫停了本端的发送（STOP），或者异常结束了整条流（ABORT）：本端之后的写都返回它。
    send_refused: Option<StreamAborted>,
    /// 写在等的任务。对端叫停时要叫醒它，不然它会一直等一个不会再变得可写的 socket。
    send_waker: Option<Waker>,
    flushing: bool,
    halves: u8,
    raw_taken: bool,
    /// 本端或对端异常结束了这条流：关闭时发 RST。
    aborted: bool,
    heard_from_peer: bool,
}

impl LaneSend {
    /// 对端节点。
    #[must_use]
    pub fn peer(&self) -> NodeId {
        self.shared.peer
    }
}

impl LaneRecv {
    /// 这个方向已经离开 TLS 时，把它交出去，让调用方直接读 socket（见 [`RawRead`]）。还在 TLS 里、已经结束或者已经交出去时返回 `None`。
    /// 不等待；要等对端的切换用 [`RecvStream::wait_raw`]。
    ///
    /// 交出去之后这个 [`LaneRecv`] 不再能读，丢弃它也不会叫停流。
    ///
    /// 已经收到、还没处理的记录先处理掉（不等待）：接受方的 REPLY 和 SWITCH 往往一起到，读完 REPLY 时 SWITCH 已经在缓冲里了。
    pub fn take_raw(&mut self) -> Option<RawRead> {
        let shared = &self.shared;
        let mut state = shared.lock();
        while matches!(&state.recv, RecvState::Sealed(data) if data.is_empty()) {
            if !matches!(state.poll_sealed_step(&shared.socket, Read::Now), Poll::Ready(Ok(()))) {
                break;
            }
        }
        if !state.out.is_empty() {
            push(shared, &mut state);
        }
        let RecvState::Raw(pending) = &mut state.recv else { return None };
        let received = std::mem::take(pending);
        state.recv = RecvState::Handed;
        state.raw_taken = true;
        state.drop_tls_if_unused();
        Some(RawRead::new(Arc::clone(&shared.socket), received, shared.book.spliced()))
    }

    /// 这条流的负载方式。发起方在头写完之前是 `None`。
    #[must_use]
    pub fn payload(&self) -> Option<Payload> {
        self.shared.lock().payload
    }
}

/// 把一条刚取到的 lane 变成本端发起的一条流。
pub(crate) fn initiate(lane: Lane, book: Book) -> (LaneSend, LaneRecv) {
    let Lane { socket, mut sealed, out, peer, .. } = lane;
    sealed.shrink();
    socket::rst_on_close(&socket);
    socket::set_user_timeout(&socket, socket::OPENING_USER_TIMEOUT);
    let state = State {
        role: Role::Initiator,
        payload: None,
        sealed: Some(sealed),
        out,
        send: SendState::Header(Vec::new()),
        recv: RecvState::Sealed(Bytes::new()),
        send_refused: None,
        send_waker: None,
        flushing: false,
        halves: 2,
        raw_taken: false,
        aborted: false,
        heard_from_peer: false,
    };
    split(socket, peer, state, book)
}

/// 对端在一条 lane 上开过来的流：它的 OPENING 已经读到了，`header` 是里面流的头。
///
/// passthrough 时 OPENING 所在的记录之后就是原始字节，调用方已经确认这条记录在这个帧的末尾结束。
pub(crate) fn respond(lane: Lane, payload: Payload, header: &[u8], book: Book) -> (LaneSend, LaneRecv) {
    let Lane { socket, mut sealed, out, peer, .. } = lane;
    socket::rst_on_close(&socket);
    socket::set_user_timeout(&socket, socket::STREAM_USER_TIMEOUT);
    let recv = match payload {
        Payload::Sealed => RecvState::Sealed(compact(header)),
        Payload::Passthrough => RecvState::Raw(compact(&[header, &sealed.take_inbound()].concat())),
    };
    let state = State {
        role: Role::Responder,
        payload: Some(payload),
        sealed: Some(sealed),
        out,
        send: SendState::Sealed,
        recv,
        send_refused: None,
        send_waker: None,
        flushing: false,
        halves: 2,
        raw_taken: false,
        aborted: false,
        heard_from_peer: true,
    };
    split(socket, peer, state, book)
}

fn split(socket: TcpStream, peer: NodeId, state: State, book: Book) -> (LaneSend, LaneRecv) {
    let shared = Arc::new(Shared {
        socket: Arc::new(socket),
        peer,
        state: Mutex::new(state),
        closed: Notify::new(),
        _in_use: book.enter(|tally| &tally.in_use),
        book,
    });
    (LaneSend { shared: Arc::clone(&shared) }, LaneRecv { shared })
}

impl State {
    fn end_header(&mut self, payload: Payload) {
        match (&mut self.send, self.role) {
            (SendState::Header(header), Role::Initiator) => {
                let header = Bytes::from(std::mem::take(header));
                self.payload = Some(payload);
                let sealed = self.sealed.as_mut().expect("头还没发出去时 TLS 状态一定还在");
                sealed.seal_frame(&Frame::Opening { payload, header });
                sealed.take_output(&mut self.out);
                self.send = match payload {
                    Payload::Sealed => SendState::Sealed,
                    Payload::Passthrough => SendState::Raw,
                };
            }
            (SendState::Sealed, Role::Responder) if payload == Payload::Passthrough && self.payload == Some(Payload::Passthrough) => {
                let sealed = self.sealed.as_mut().expect("sealed 的方向一定还有 TLS 状态");
                sealed.seal_frame(&Frame::Switch);
                sealed.take_output(&mut self.out);
                self.send = SendState::Raw;
                self.drop_tls_if_unused();
            }
            _ => {}
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
    fn drain_now(&mut self, socket: &TcpStream) -> io::Result<bool> {
        while !self.out.is_empty() {
            match socket.try_write(&self.out) {
                Ok(written) => self.out.advance(written),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(false),
                Err(error) => return Err(error),
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
                if self.role != Role::Initiator || self.payload != Some(Payload::Passthrough) || !at_boundary {
                    return Poll::Ready(Err(self.fail_recv(Failure::Protocol("不该出现的 SWITCH".to_owned()))));
                }
                self.heard(socket);
                let leftover = self.sealed.as_mut().map(Sealed::take_inbound).unwrap_or_default();
                self.recv = RecvState::Raw(compact(&leftover));
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
    loop {
        {
            let mut state = shared.lock();
            let done = state.halves == 0 || !matches!(state.drain_now(&shared.socket), Ok(false));
            if done {
                state.flushing = false;
                state.wake_sender();
                return;
            }
        }
        tokio::select! {
            ready = shared.socket.writable() => if ready.is_err() {
                shared.lock().flushing = false;
                return;
            },
            () = shared.closed.notified() => {
                shared.lock().flushing = false;
                return;
            }
        }
    }
}

impl AsyncRead for LaneRecv {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        let shared = &self.shared;
        let mut state = shared.lock();
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
                RecvState::Raw(pending) if !pending.is_empty() => {
                    let len = take_into(pending, buf);
                    shared.book.add(|tally| &tally.raw_bytes, len);
                    return Poll::Ready(Ok(()));
                }
                RecvState::Raw(_) => match shared.socket.try_read(buf.initialize_unfilled()) {
                    Ok(0) => {
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
        let shared = &self.shared;
        let mut state = shared.lock();
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
                        Err(error) => return Poll::Ready(Err(error)),
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
        let shared = &self.shared;
        let mut state = shared.lock();
        if matches!(state.send, SendState::Header(_)) {
            state.end_header(Payload::Sealed);
        }
        state.poll_drain(&shared.socket, cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let shared = &self.shared;
        let mut state = shared.lock();
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
                    socket::shutdown_write(&shared.socket)?;
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

    /// 头写完了：发起方是 OPEN 连同首批数据，接受方是 REPLY。
    ///
    /// - 发起方：`payload` 选定这条流的负载方式，攒下的头作为一个 OPENING 帧送出；
    ///   选了 [`Payload::Passthrough`] 时，之后这个方向是原始字节。
    /// - 接受方：发起方选了 passthrough、并且这里也传 [`Payload::Passthrough`] 时，发一个 SWITCH，之后这个方向是原始字节；
    ///   别的情况什么都不做，这个方向留在 TLS 里。
    ///
    /// 不用等：密文立刻交给内核，交不完的由后台送出。头已经结束之后再调用没有效果。
    ///
    /// 发起方不调它也能用，只是一直是 sealed：开始读（等回复）、`flush`、`shutdown`，或者头超过上限时，
    /// 攒下的头按 sealed 送出。
    fn end_header(&mut self, payload: Payload) {
        let mut state = self.shared.lock();
        state.end_header(payload);
        push(&self.shared, &mut state);
    }

    /// 这个方向已经离开 TLS 时，把它交出去，让调用方直接写 socket（见 [`RawWrite`]）。还在 TLS 里、已经结束或者已经交出去时返回 `None`。
    ///
    /// 交出去之后这个 [`LaneSend`] 不再能写，丢弃它也不会重置流。
    fn take_raw(&mut self) -> Option<RawWrite> {
        let shared = &self.shared;
        let mut state = shared.lock();
        if !matches!(state.send, SendState::Raw) || state.aborted {
            return None;
        }
        state.send = SendState::Handed;
        state.raw_taken = true;
        let unsent = std::mem::take(&mut state.out).freeze();
        state.drop_tls_if_unused();
        Some(RawWrite::new(Arc::clone(&shared.socket), unsent, shared.book.spliced()))
    }
}

fn reset(shared: &Shared, state: &mut State, code: StreamCode) {
    let raw = match state.send {
        SendState::Finished | SendState::Aborted | SendState::Handed => return,
        // 头还没发出去：对端还不知道有这条流，重置连接就够了。
        SendState::Header(_) | SendState::Closing => false,
        SendState::Sealed => {
            let sealed = state.sealed.as_mut().expect("sealed 的方向一定还有 TLS 状态");
            sealed.seal_frame(&Frame::Abort(code.0));
            sealed.take_output(&mut state.out);
            // 尽力而为：ABORT 进得了内核就送得到，进不去对端也会看到 RST。
            let _ = state.drain_now(&shared.socket);
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

    /// 等到知道这个方向接下来是什么：离开了 TLS（对端发了 SWITCH，或者发起方选了 passthrough）就交出来，和
    /// [`take_raw`](Self::take_raw) 一样；接下来是 TLS 里的数据、或者这个方向结束了、出错了，返回 `None`，调用方照常读。
    ///
    /// 发起方读完 REPLY 之后用它：接受方紧接着发 SWITCH，但它可能还在路上。
    async fn wait_raw(&mut self) -> Option<RawRead> {
        std::future::poll_fn(|cx| {
            let shared = &self.shared;
            let mut state = shared.lock();
            loop {
                match &state.recv {
                    RecvState::Raw(_) => break,
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
    if matches!(state.recv, RecvState::Eof | RecvState::Stopped | RecvState::Failed(_) | RecvState::Handed) {
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
        // 交出去的方向怎么结束由调用方决定，见 `RawWrite`。
        return;
    }
    let send_clean = matches!(state.send, SendState::Finished);
    match (&state.recv, send_clean, state.aborted) {
        (RecvState::Eof, true, false) => socket::graceful_close(&shared.socket),
        (RecvState::Stopped, true, false) => linger_then_close(shared),
        _ => {}
    }
}

/// 本端已经正常写完、但叫停了对端：先把对端还在路上的数据收下扔掉，再正常关闭（见 [`LINGER_AFTER_STOP`]）。
fn linger_then_close(shared: &Arc<Shared>) {
    let socket = Arc::clone(&shared.socket);
    let Ok(runtime) = tokio::runtime::Handle::try_current() else { return };
    socket::graceful_close(&socket);
    drop(runtime.spawn(async move {
        let discard = async {
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
        };
        let _ = tokio::time::timeout(LINGER_AFTER_STOP, discard).await;
    }));
}
