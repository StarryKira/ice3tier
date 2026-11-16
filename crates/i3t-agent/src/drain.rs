//! 排空：更新之前不再接新的流，手上的流不受影响（白皮书 §10.5）。
//!
//! 管理员选"排空后更新"时，更新进行到"排空中"这一步，节点就把门关上，直到进程重启：
//!
//! - **出口一侧**（别的节点开过来的流和数据报上下文）：[`DrainGate`] 挡在出口模块前面，用一个说明原因的应答拒绝。
//!   状态用的是"出口自己出了问题"那一类：调用方据此换一个出口，而不是以为自己没有权限。
//! - **入口一侧**（本机的代理监听器和端口转发）：装配层把它们从交给入口模块的 netmap 里去掉（见 `node::localize`），
//!   监听随之关闭，新的连接连不上；已经建立的连接照常跑完。
//!
//! 门什么时候关由更新的状态决定（[`Drain`]），不另外记一个开关：更新被撤回或者失败了，状态离开这几步，门自己就开了。
//! "正在替换程序"和"马上重启"两步门也是关着的：从流数到零到进程退出之间不该再放新的流进来。

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use i3t_conn::contract::DatagramSender;
use i3t_conn::{RecvStream, SendStream};
use i3t_flow::{Handler, Request, datagram};
use i3t_model::update::{UpdateStatus, Version};
use i3t_proto::NodeId;
use i3t_proto::flow::{Reply, Status};
use tokio::sync::watch;

/// 节点现在是不是在排空。可以克隆，各处拿着的看到的是同一个状态。
#[derive(Debug, Clone)]
pub struct Drain {
    status: watch::Receiver<UpdateStatus>,
    /// [`changed`](Self::changed) 上一次告诉调用方的值。一开始当作没有在排空。
    told: bool,
}

impl Drain {
    /// `status` 是节点报给 controller 的那个更新状态。
    #[must_use]
    pub const fn new(status: watch::Receiver<UpdateStatus>) -> Self {
        Self { status, told: false }
    }

    /// 正在排空的话，是为了更新到哪个版本。
    #[must_use]
    pub fn version(&self) -> Option<Version> {
        draining_for(&self.status.borrow()).cloned()
    }

    #[must_use]
    pub fn is_draining(&self) -> bool {
        draining_for(&self.status.borrow()).is_some()
    }

    /// 等到"是不是在排空"和上一次告诉调用方的不一样了，返回新的值。第一次调用时，已经在排空的话马上返回。
    /// 状态不会再变（发送端没了）时一直等下去。
    pub async fn changed(&mut self) -> bool {
        loop {
            let now = draining_for(&self.status.borrow_and_update()).is_some();
            if now != self.told {
                self.told = now;
                return now;
            }
            if self.status.changed().await.is_err() {
                return std::future::pending().await;
            }
        }
    }
}

/// 这个更新状态下门是不是关着的；关着的话是为了哪个版本。
fn draining_for(status: &UpdateStatus) -> Option<&Version> {
    match status {
        UpdateStatus::Draining { version } | UpdateStatus::Installing { version } | UpdateStatus::Restarting { version } => Some(version),
        UpdateStatus::Idle
        | UpdateStatus::Downloading { .. }
        | UpdateStatus::Verifying { .. }
        | UpdateStatus::SelfCheck { .. }
        | UpdateStatus::WaitingForIdle { .. }
        | UpdateStatus::Confirmed { .. }
        | UpdateStatus::Failed { .. }
        | UpdateStatus::Unknown => None,
    }
}

/// 挡在出口模块前面的门：排空期间拒绝别的节点新开过来的流和数据报上下文，平时原样交给 `inner`。
#[derive(Debug)]
pub struct DrainGate<H> {
    inner: Arc<H>,
    drain: Drain,
    refused: AtomicU64,
}

impl<H> DrainGate<H> {
    #[must_use]
    pub const fn new(inner: Arc<H>, drain: Drain) -> Self {
        Self { inner, drain, refused: AtomicU64::new(0) }
    }

    /// 门现在是不是关着的。
    #[must_use]
    pub fn is_draining(&self) -> bool {
        self.drain.is_draining()
    }

    /// 启动以来因为排空而拒绝的请求数。
    #[must_use]
    pub fn refused(&self) -> u64 {
        self.refused.load(Ordering::Relaxed)
    }

    /// 排空期间给发起方的应答；不在排空时是 `None`。每拒绝一次记一条带原因的日志。
    fn refusal(&self, peer: NodeId, what: &str) -> Option<Reply> {
        let version = self.drain.version()?;
        self.refused.fetch_add(1, Ordering::Relaxed);
        tracing::info!(%peer, %version, what, "拒绝了一个请求：节点正在排空，更新到这个版本并重启之前不再接新的流");
        Some(Reply::new(
            Status::InternalError,
            format!("节点正在排空，准备更新到 {version} 后重启，不接新的流；请换一个出口，或者稍后再试"),
        ))
    }
}

impl<H: Handler> Handler for DrainGate<H> {
    async fn handle<S: SendStream, R: RecvStream>(&self, request: Request<S, R>) {
        match self.refusal(request.peer(), "流") {
            Some(reply) => request.refuse(&reply).await,
            None => self.inner.handle(request).await,
        }
    }

    async fn handle_datagrams<S: SendStream, R: RecvStream, T: DatagramSender>(&self, request: datagram::Request<S, R, T>) {
        match self.refusal(request.peer(), "数据报上下文") {
            Some(reply) => request.refuse(&reply).await,
            None => self.inner.handle_datagrams(request).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use i3t_flow::FlowError;
    use i3t_flow::memory::MemoryMesh;
    use i3t_proto::flow::{Class, Open, Origin, Target};
    use tokio::time::timeout;

    use super::*;

    fn version() -> Version {
        Version::new(0, 2, 0)
    }

    /// 一个什么都答应的出口：接受每条流，然后不再管它。
    #[derive(Default)]
    struct AcceptAll {
        handled: AtomicU64,
    }

    impl Handler for AcceptAll {
        async fn handle<S: SendStream, R: RecvStream>(&self, request: Request<S, R>) {
            self.handled.fetch_add(1, Ordering::Relaxed);
            // 接受之后把流一直拿着：丢掉它等于重置，发起方可能连成功的应答都读不到。
            let _flow = request.accept().await;
            std::future::pending::<()>().await;
        }
    }

    #[test]
    fn the_door_is_shut_from_draining_until_the_restart() {
        let shut = [
            UpdateStatus::Draining { version: version() },
            UpdateStatus::Installing { version: version() },
            UpdateStatus::Restarting { version: version() },
        ];
        for status in shut {
            assert_eq!(draining_for(&status), Some(&version()), "{status:?}");
        }
        let open = [
            UpdateStatus::Idle,
            UpdateStatus::Downloading { version: version(), received: 1, total: 2 },
            UpdateStatus::SelfCheck { version: version() },
            UpdateStatus::WaitingForIdle { version: version() },
            UpdateStatus::Confirmed { version: version() },
            UpdateStatus::Unknown,
        ];
        for status in open {
            assert_eq!(draining_for(&status), None, "{status:?}");
        }
    }

    #[tokio::test]
    async fn drain_changes_are_reported_once_per_flip() {
        let (status, receiver) = watch::channel(UpdateStatus::Idle);
        let mut drain = Drain::new(receiver);
        assert!(!drain.is_draining());
        status.send_replace(UpdateStatus::WaitingForIdle { version: version() });
        status.send_replace(UpdateStatus::Draining { version: version() });
        assert!(timeout(Duration::from_secs(5), drain.changed()).await.unwrap());
        assert_eq!(drain.version(), Some(version()));
        // 从排空走到替换程序：门还是关着的，不算变化。
        status.send_replace(UpdateStatus::Installing { version: version() });
        assert!(timeout(Duration::from_millis(100), drain.changed()).await.is_err());
        // 更新被撤回：门开了。
        status.send_replace(UpdateStatus::Idle);
        assert!(!timeout(Duration::from_secs(5), drain.changed()).await.unwrap());
    }

    #[tokio::test]
    async fn new_flows_are_refused_with_a_reason_while_draining_and_served_otherwise() {
        let mesh = MemoryMesh::new();
        let (client, exit) = (NodeId::new(1).unwrap(), NodeId::new(2).unwrap());
        let (connector, _unused) = mesh.join(client);
        let (_unused_connector, acceptor) = mesh.join(exit);
        let (status, receiver) = watch::channel(UpdateStatus::Idle);
        let inner = Arc::new(AcceptAll::default());
        let gate = Arc::new(DrainGate::new(Arc::clone(&inner), Drain::new(receiver)));
        let serving = tokio::spawn({
            let gate = Arc::clone(&gate);
            async move { i3t_flow::serve(&acceptor, gate).await }
        });
        let request =
            Open { target: Target::Node { port: 80 }, class: Class::default(), origin: Origin { credential: None, client: None } };
        let open = || async { i3t_flow::open(&connector, exit, &request, b"").await.unwrap().established().await };

        assert!(open().await.is_ok(), "平时原样交给出口");
        assert_eq!((inner.handled.load(Ordering::Relaxed), gate.refused()), (1, 0));

        status.send_replace(UpdateStatus::Draining { version: version() });
        let Err(FlowError::Refused(reply)) = open().await else { panic!("排空期间应该被拒绝") };
        assert_eq!(reply.status, Status::InternalError, "算在出口自己头上，调用方据此换一个出口");
        assert!(reply.detail.contains("排空") && reply.detail.contains("0.2.0"), "{}", reply.detail);
        assert_eq!((inner.handled.load(Ordering::Relaxed), gate.refused()), (1, 1), "出口模块根本没有看到这条流");

        // 撤回之后门重新打开。
        status.send_replace(UpdateStatus::Idle);
        assert!(open().await.is_ok());
        assert_eq!(inner.handled.load(Ordering::Relaxed), 2);
        serving.abort();
    }
}
