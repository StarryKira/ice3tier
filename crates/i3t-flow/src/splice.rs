//! 把两条流接起来：一条是别的节点开过来的，一条是本节点替它开到第三个节点的。
//!
//! 监听在本节点虚拟 IP 上、目标是别的节点的端口转发走这里：发起节点以为自己在访问本节点的一个端口，
//! 本节点按转发的编号到目标节点开一条流，再把两条流对接。数据在本节点上解密再加密，两段各自端到端。
//!
//! 结束方式如实传过去：一条流的一个方向正常结束，另一条流对应的方向也正常结束；一个方向被对端重置或者叫停，
//! 两条流的四个方向全部带着同一个原因码异常结束，被截断的数据不会看起来像正常结束。
//!
//! 开过来的那条流在 ice3proxy 上、发起方选了不再加密时，往回的方向和 [`Flow::relay`] 一样，看到目标回的 TLS 1.3
//! ServerHello 之后才不再加密（[`ServerHelloWatch`]），发起方的方向再跟着它。

use std::future::Future;
use std::io;

use i3t_conn::{Payload, RecvStream, SendStream, ServerHelloVerdict, ServerHelloWatch, StreamAborted, StreamCode};
use i3t_proto::flow::Reason;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::relay::{End, Flow, Parts, buffer_len, cancel_on_revocation};

/// 一次对接的结果。
#[derive(Debug)]
pub struct Spliced {
    /// 从 `self`（调用 [`Flow::splice`] 的那条流）读出来、写进另一条流的字节数。
    pub near_to_far: u64,
    /// 反过来的字节数。
    pub far_to_near: u64,
    /// [`End::Clean`]：四个方向都正常结束；[`End::Aborted`]：有一个方向被对端异常结束，原因码已经传给了别的方向；
    /// [`End::Lost`]：有一条连接断了；[`End::Cancelled`]：调用者取消。
    pub end: End,
}

/// 一个方向停下来的原因。
enum Broken {
    Io(io::Error),
    Cancelled(Reason),
}

impl<S: SendStream, R: RecvStream> Flow<S, R> {
    /// 把这条流和 `other` 接起来，直到两边都结束，或者有一个方向异常结束。
    ///
    /// `cancel` 完成时立刻停止，四个方向都带着它给的原因码异常结束。不需要取消时传 [`std::future::pending`]。
    pub async fn splice<S2: SendStream, R2: RecvStream>(self, other: Flow<S2, R2>, cancel: impl Future<Output = Reason>) -> Spliced {
        let Parts { send: mut near_send, recv: mut near_recv, .. } = self.parts;
        let Parts { send: mut far_send, recv: mut far_recv, .. } = other.parts;
        let near_revocation = near_recv.revocation();
        let far_revocation = far_recv.revocation();
        let cancel = cancel_on_revocation(cancel, near_revocation.clone());
        let cancel = cancel_on_revocation(cancel, far_revocation.clone());
        let (mut near_to_far, mut far_to_near) = (0, 0);
        let outcome = {
            let up = pump(&mut near_recv, &mut far_send, &mut near_to_far, None);
            let down = pump(&mut far_recv, &mut near_send, &mut far_to_near, Some(ServerHelloWatch::new()));
            both(up, down, cancel).await
        };
        let outcome = if [near_revocation.as_ref(), far_revocation.as_ref()].into_iter().flatten().any(|state| *state.borrow()) {
            Err(Broken::Cancelled(Reason::AccessRevoked))
        } else {
            outcome
        };
        let end = match outcome {
            Ok(()) => End::Clean,
            Err(broken) => {
                let (code, end) = match broken {
                    Broken::Cancelled(reason) => (StreamCode(reason.to_wire()), End::Cancelled(reason)),
                    Broken::Io(error) => match StreamAborted::of(&error) {
                        Some(aborted) => (aborted.code(), End::Aborted(aborted)),
                        None => (StreamCode(Reason::Cancelled.to_wire()), End::Lost(error)),
                    },
                };
                near_send.reset(code);
                near_recv.stop(code);
                far_send.reset(code);
                far_recv.stop(code);
                end
            }
        };
        Spliced { near_to_far, far_to_near, end }
    }
}

/// 两个方向同时跑。一个方向正常结束时另一个继续；一个方向出错时整个结束。
async fn both(
    up: impl Future<Output = Result<(), Broken>>,
    down: impl Future<Output = Result<(), Broken>>,
    cancel: impl Future<Output = Reason>,
) -> Result<(), Broken> {
    tokio::pin!(up, down, cancel);
    let (mut up_done, mut down_done) = (false, false);
    while !(up_done && down_done) {
        tokio::select! {
            biased;
            reason = &mut cancel => return Err(Broken::Cancelled(reason)),
            end = &mut up, if !up_done => {
                end?;
                up_done = true;
            }
            end = &mut down, if !down_done => {
                end?;
                down_done = true;
            }
        }
    }
    Ok(())
}

/// 从 `recv` 搬到 `send`。给了 `watch` 时，目标的 TLS 1.3 ServerHello 写完就告诉 `send` 可以不再加密。
async fn pump(
    recv: &mut impl RecvStream,
    send: &mut impl SendStream,
    count: &mut u64,
    mut watch: Option<ServerHelloWatch>,
) -> Result<(), Broken> {
    let mut buf = vec![0; buffer_len()];
    loop {
        let len = recv.read(&mut buf).await.map_err(Broken::Io)?;
        if len == 0 {
            return send.shutdown().await.map_err(Broken::Io);
        }
        match watch.as_mut().map(|watch| watch.feed(&buf[..len])) {
            None | Some(ServerHelloVerdict::Undecided) => send.write_all(&buf[..len]).await.map_err(Broken::Io)?,
            Some(ServerHelloVerdict::Sealed) => {
                watch = None;
                send.write_all(&buf[..len]).await.map_err(Broken::Io)?;
            }
            Some(ServerHelloVerdict::PassthroughAfter(at)) => {
                watch = None;
                send.write_all(&buf[..at]).await.map_err(Broken::Io)?;
                send.end_header(Payload::Passthrough);
                send.write_all(&buf[at..len]).await.map_err(Broken::Io)?;
            }
        }
        // usize 不会比 u64 宽，这里不会截断。
        *count += len as u64;
    }
}
