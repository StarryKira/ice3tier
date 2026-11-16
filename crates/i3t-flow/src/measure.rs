//! 测量流：主动测试（白皮书 §7.6）量吞吐用的流。格式见 [`i3t_proto::flow::OpenMeasure`]。
//!
//! - 发起方用 [`measure`] 开一条测量流，量一个方向：[`MeasureMode::Sink`] 是自己发、对端收（上行），
//!   [`MeasureMode::Source`] 是对端发、自己收（下行）。要用共用的连接开（不经独占 TCP）：量的是那条连接现在走的路。
//! - 接受方从 [`crate::serve`] 拿到 [`MeasureRequest`]（[`crate::Handler::handle_measure`]），先自己决定接不接（测试编号、时间窗、
//!   本机的开关），接的话调用 [`MeasureRequest::serve`]。接受方的上限（[`MeasureLimits`]）由它自己定，不管发起方要多少：
//!   字节数和时长到了就停，另有一个总的时限，发起方不发数据、不读数据都占不住它。
//! - 数据是全零的块。连接本身是加密的，量到的就是真实负载的吞吐。

use std::time::{Duration, Instant};

use i3t_conn::{Connector, Priority, RecvStream, SendStream, StreamCode};
use i3t_proto::NodeId;
use i3t_proto::flow::{MeasureEnd, MeasureMode, MeasureSummary, OpenMeasure, Reason, Reply};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time;

use crate::{FlowError, header};

/// 一次读写的块有多大。
const CHUNK: usize = 64 * 1024;
/// 量完之后等对方的收尾（"收"的方式下接受方写回的那一段）最多等多久。
const SUMMARY_WAIT: Duration = Duration::from_secs(5);
/// 除了时长上限，接受方另外给流留的余量：建流、慢启动之前的空档、收尾。过了这个总时限一律重置。
const GRACE: Duration = Duration::from_secs(3);

/// 接受方自己的上限。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MeasureLimits {
    pub bytes: u64,
    pub duration: Duration,
}

impl MeasureLimits {
    /// 发起方要的和接受方自己的上限里小的那个。
    #[must_use]
    pub fn within(self, open: &OpenMeasure) -> Self {
        Self { bytes: self.bytes.min(open.bytes), duration: self.duration.min(Duration::from_millis(u64::from(open.millis))) }
    }
}

/// 一个方向搬了多少。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Measured {
    pub bytes: u64,
    /// 从第一个字节到最后一个字节。
    pub elapsed: Duration,
    pub end: MeasureEnd,
}

impl Measured {
    /// 每秒多少比特。用时太短（不到 1 毫秒）时算不出来，是 `None`。
    #[must_use]
    pub fn bits_per_second(&self) -> Option<f64> {
        let seconds = self.elapsed.as_secs_f64();
        // 一次测量最多几十 MB，放得进 u32；真放不下时停在最大值，算出来的只会偏低。
        let bits = f64::from(u32::try_from(self.bytes).unwrap_or(u32::MAX)) * 8.0;
        (seconds >= 0.001).then(|| bits / seconds)
    }
}

/// 对端开过来的一条测量流，OPEN 已经读出来了。
pub struct MeasureRequest<S, R> {
    pub(crate) peer: NodeId,
    pub(crate) open: OpenMeasure,
    pub(crate) send: S,
    pub(crate) recv: R,
}

impl<S, R> std::fmt::Debug for MeasureRequest<S, R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MeasureRequest").field("peer", &self.peer).field("open", &self.open).finish_non_exhaustive()
    }
}

impl<S: SendStream, R: RecvStream> MeasureRequest<S, R> {
    /// 开这条流的节点，来自连接的握手。
    #[must_use]
    pub const fn peer(&self) -> NodeId {
        self.peer
    }

    #[must_use]
    pub const fn open(&self) -> &OpenMeasure {
        &self.open
    }

    /// 不接受：回这个拒绝，结束这条流。
    pub async fn refuse(mut self, reply: &Reply) {
        crate::acceptor::refuse(self.peer, &mut self.send, &mut self.recv, reply).await;
    }

    /// 接受，按 `limits`（和发起方要的里小的那个）搬运，直到结束。返回这个方向搬了多少。
    ///
    /// 不管发起方怎么做，这个函数最多占用"时长上限加几秒"：到时候流被重置。
    pub async fn serve(mut self, limits: MeasureLimits) -> Measured {
        let limits = limits.within(&self.open);
        let deadline = limits.duration + GRACE;
        let mut reply = Vec::with_capacity(Reply::MAX_LEN);
        Reply::ok().encode(&mut reply);
        // 测量流让着这条连接上别的流：吞吐测的是"别的流用剩下的"，测试期间不该把别人的交互挤慢。
        self.send.set_priority(Priority::Low);
        let mode = self.open.mode;
        let work = async {
            self.send.write_all(&reply).await?;
            match mode {
                MeasureMode::Sink => sink(&mut self.send, &mut self.recv, limits).await,
                MeasureMode::Source => {
                    self.recv.stop(StreamCode(Reason::Cancelled.to_wire()));
                    source(&mut self.send, limits).await
                }
            }
        };
        let outcome = time::timeout(deadline, work).await;
        match outcome {
            Ok(Ok(measured)) => measured,
            failed => {
                let code = StreamCode(Reason::Cancelled.to_wire());
                self.send.reset(code);
                self.recv.stop(code);
                if let Ok(Err(error)) = failed {
                    tracing::debug!(peer = %self.peer, %error, "测量流中途断了");
                }
                Measured { bytes: 0, elapsed: Duration::ZERO, end: MeasureEnd::Time }
            }
        }
    }
}

/// "收"：读到发起方结束、字节数或者时长到了为止，然后写回收了多少。
async fn sink(send: &mut impl SendStream, recv: &mut impl RecvStream, limits: MeasureLimits) -> std::io::Result<Measured> {
    let mut buf = vec![0; CHUNK];
    let mut bytes = 0u64;
    let started = Instant::now();
    let (mut first, mut last) = (None, started);
    let end = loop {
        // 发起方迟迟不发第一个字节时也按时长算：从接受这条流算起。
        let since = first.unwrap_or(started);
        let wait = limits.duration.saturating_sub(since.elapsed());
        // 数据一直读得到时等待不会超时，所以每一轮都先看一眼时间。
        if wait.is_zero() {
            break MeasureEnd::Time;
        }
        match time::timeout(wait, recv.read(&mut buf)).await {
            Err(_) => break MeasureEnd::Time,
            Ok(Ok(0)) => break MeasureEnd::Finished,
            Ok(Ok(read)) => {
                let now = Instant::now();
                first.get_or_insert(now);
                last = now;
                bytes = bytes.saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
                if bytes >= limits.bytes {
                    break MeasureEnd::Bytes;
                }
            }
            Ok(Err(error)) => return Err(error),
        }
    };
    if end != MeasureEnd::Finished {
        // 到了上限：叫发起方别再发了。
        recv.stop(StreamCode(Reason::Cancelled.to_wire()));
    }
    let elapsed = first.map_or(Duration::ZERO, |first| last.saturating_duration_since(first));
    let millis = u32::try_from(elapsed.as_millis()).unwrap_or(u32::MAX);
    send.write_all(&MeasureSummary { bytes, millis, end }.to_bytes()).await?;
    send.shutdown().await?;
    Ok(Measured { bytes, elapsed, end })
}

/// "发"：写到字节数或者时长到了为止，然后正常结束。
async fn source(send: &mut impl SendStream, limits: MeasureLimits) -> std::io::Result<Measured> {
    let chunk = vec![0; CHUNK];
    let started = Instant::now();
    let mut bytes = 0u64;
    let end = loop {
        if bytes >= limits.bytes {
            break MeasureEnd::Bytes;
        }
        let left = limits.duration.saturating_sub(started.elapsed());
        // 写一直写得进去时等待不会超时，所以每一轮都先看一眼时间。
        if left.is_zero() {
            break MeasureEnd::Time;
        }
        let size = usize::try_from(limits.bytes - bytes).unwrap_or(CHUNK).min(CHUNK);
        match time::timeout(left, send.write_all(&chunk[..size])).await {
            Err(_) => break MeasureEnd::Time,
            Ok(Ok(())) => bytes += u64::try_from(size).unwrap_or(u64::MAX),
            Ok(Err(error)) => return Err(error),
        }
    };
    send.shutdown().await?;
    Ok(Measured { bytes, elapsed: started.elapsed(), end })
}

/// 到 `peer` 开一条测量流，按 `open` 量一个方向，返回量到的结果。
///
/// 上行（[`MeasureMode::Sink`]）的结果是对端说它收到的；下行（[`MeasureMode::Source`]）的结果是本端自己收到的。
/// 本端这边也守着 `open` 里的上限，外面再套一个总的时限，对端不配合也不会一直等下去。
///
/// # Errors
///
/// 开不了流、对端拒绝（[`FlowError::Refused`]），或者中途出错、超时。
pub async fn measure<C: Connector>(connector: &C, peer: NodeId, open: &OpenMeasure) -> Result<Measured, FlowError> {
    let (mut send, mut recv) = connector.open(peer).await?;
    send.set_priority(Priority::Low);
    let mut head = Vec::with_capacity(OpenMeasure::MAX_LEN);
    open.encode(&mut head);
    send.write_all(&head).await.map_err(FlowError::Write)?;
    let limit = Duration::from_millis(u64::from(open.millis)) + GRACE + SUMMARY_WAIT;
    let run = async {
        let reply = header::read_reply(&mut recv).await?;
        if !reply.status.is_ok() {
            return Err(FlowError::Refused(reply));
        }
        match open.mode {
            MeasureMode::Sink => upload(&mut send, &mut recv, open).await,
            MeasureMode::Source => {
                // 下行时本端不发数据。对端接受时叫停了这个方向，这里的结束可能失败，那也没有关系。
                let _ended = send.shutdown().await;
                download(&mut recv, open).await
            }
        }
    };
    match time::timeout(limit, run).await {
        Ok(Ok(measured)) => Ok(measured),
        Ok(Err(error)) => {
            abandon(&mut send, &mut recv);
            Err(error)
        }
        Err(_) => {
            abandon(&mut send, &mut recv);
            Err(FlowError::Deadline(limit))
        }
    }
}

fn abandon(send: &mut impl SendStream, recv: &mut impl RecvStream) {
    let code = StreamCode(Reason::Cancelled.to_wire());
    send.reset(code);
    recv.stop(code);
}

/// 上行：写到自己的上限或者对端叫停，然后读对端写回的那一段。
async fn upload(send: &mut impl SendStream, recv: &mut impl RecvStream, open: &OpenMeasure) -> Result<Measured, FlowError> {
    let chunk = vec![0; CHUNK];
    let started = Instant::now();
    let duration = Duration::from_millis(u64::from(open.millis));
    let mut written = 0u64;
    while written < open.bytes && started.elapsed() < duration {
        let size = usize::try_from(open.bytes - written).unwrap_or(CHUNK).min(CHUNK);
        let left = duration.saturating_sub(started.elapsed());
        match time::timeout(left, send.write_all(&chunk[..size])).await {
            Ok(Ok(())) => written += u64::try_from(size).unwrap_or(u64::MAX),
            // 对端到了它的上限，叫停了这个方向；或者时间到了。都去读它写回的那一段。
            Ok(Err(_)) | Err(_) => break,
        }
    }
    // 对端叫停之后这里的结束可能失败，那也没有关系：它已经不读了。
    let _ended = send.shutdown().await;
    let mut summary = [0; MeasureSummary::LEN];
    time::timeout(SUMMARY_WAIT, recv.read_exact(&mut summary)).await.map_err(|_| FlowError::Deadline(SUMMARY_WAIT))?.map_err(|error| {
        match error.kind() {
            std::io::ErrorKind::UnexpectedEof => FlowError::Truncated,
            _ => FlowError::Read(error),
        }
    })?;
    let summary = MeasureSummary::from_bytes(&summary).ok_or(FlowError::Truncated)?;
    Ok(Measured { bytes: summary.bytes, elapsed: Duration::from_millis(u64::from(summary.millis)), end: summary.end })
}

/// 下行：读到对端结束、字节数够了，或者从第一个字节算起时长到了为止，自己数。
///
/// 时长按收到的一侧算：对端写进发送缓冲就算写完了，它停笔的时候缓冲里还有一截在路上，按它的时钟停会把那一截也算进来。
async fn download(recv: &mut impl RecvStream, open: &OpenMeasure) -> Result<Measured, FlowError> {
    let mut buf = vec![0; CHUNK];
    let mut bytes = 0u64;
    let limit = Duration::from_millis(u64::from(open.millis));
    let (mut first, mut last) = (None, Instant::now());
    let end = loop {
        let left = first.map_or(limit + GRACE, |first: Instant| limit.saturating_sub(first.elapsed()));
        if left.is_zero() {
            break MeasureEnd::Time;
        }
        match time::timeout(left, recv.read(&mut buf)).await {
            Err(_) => break MeasureEnd::Time,
            // 对端结束了：字节数没够的话，是它那边的时长上限先到了。
            Ok(Ok(0)) => break if bytes >= open.bytes { MeasureEnd::Bytes } else { MeasureEnd::Time },
            Ok(Ok(read)) => {
                let now = Instant::now();
                first.get_or_insert(now);
                last = now;
                bytes = bytes.saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
                if bytes >= open.bytes {
                    break MeasureEnd::Bytes;
                }
            }
            Ok(Err(error)) => return Err(FlowError::Read(error)),
        }
    };
    if end == MeasureEnd::Time {
        // 时长到了，后面还在路上的不要了。
        recv.stop(StreamCode(Reason::Cancelled.to_wire()));
    }
    let elapsed = first.map_or(Duration::ZERO, |first| last.saturating_duration_since(first));
    Ok(Measured { bytes, elapsed, end })
}
