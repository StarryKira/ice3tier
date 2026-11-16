//! 把 controller 的几个入口装在一起跑：节点协议（TCP）、地址查询（同一个端口号的 UDP）、管理 API，和后台任务。

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::net::{TcpListener, UdpSocket};
use tokio::sync::watch;

use crate::controller::Controller;
use crate::error::{Error, error_chain};
use crate::{addr_server, admin, node_server};

/// 后台工作出错之后多久再试。
const RETRY_DELAY: Duration = Duration::from_secs(1);

/// [`Server::bind`] 要的东西。
#[derive(Debug, Clone)]
pub struct ServeOptions {
    /// 节点协议的监听地址。TCP 和 UDP 用同一个地址和端口。
    pub node_listen: SocketAddr,
    /// 管理 API 的监听地址。默认应该只绑回环地址。
    pub admin_listen: SocketAddr,
    /// WebUI 的静态文件目录；有的话挂在 `/` 上。
    pub static_dir: Option<PathBuf>,
}

/// 已经绑好端口、还没开始服务的 controller。
pub struct Server {
    controller: Arc<Controller>,
    tls: Arc<rustls::ServerConfig>,
    node_tcp: TcpListener,
    node_udp: UdpSocket,
    admin: TcpListener,
    static_dir: Option<PathBuf>,
}

impl Server {
    /// 绑定全部端口。端口写 0 时由系统挑，挑到的地址用 [`Server::node_addr`] 和 [`Server::admin_addr`] 查。
    ///
    /// # Errors
    ///
    /// 有端口绑不上，或者准备 TLS 配置失败。
    pub async fn bind(controller: Arc<Controller>, options: ServeOptions) -> Result<Self, Error> {
        let tls = Arc::new(controller.tls_config()?);
        let bind_error = |address: SocketAddr| move |source| Error::Bind { address, source };
        let node_tcp = TcpListener::bind(options.node_listen).await.map_err(bind_error(options.node_listen))?;
        // 节点按"和节点协议同一个端口号"去找地址查询，所以 UDP 跟着 TCP 实际绑到的端口走。
        let node_addr = node_tcp.local_addr().map_err(bind_error(options.node_listen))?;
        let node_udp = UdpSocket::bind(node_addr).await.map_err(bind_error(node_addr))?;
        let admin = TcpListener::bind(options.admin_listen).await.map_err(bind_error(options.admin_listen))?;
        Ok(Self { controller, tls, node_tcp, node_udp, admin, static_dir: options.static_dir })
    }

    /// 节点协议实际监听的地址。
    ///
    /// # Errors
    ///
    /// 系统查不出这个 socket 的地址。
    pub fn node_addr(&self) -> Result<SocketAddr, Error> {
        self.node_tcp.local_addr().map_err(Error::Socket)
    }

    /// 管理 API 实际监听的地址。
    ///
    /// # Errors
    ///
    /// 系统查不出这个 socket 的地址。
    pub fn admin_addr(&self) -> Result<SocketAddr, Error> {
        self.admin.local_addr().map_err(Error::Socket)
    }

    /// 开始服务，直到 `shutdown` 完成。之后不再接受新的连接和请求，等手上的收尾再返回：
    /// 在线的节点被记为离线，它们会自己重连。
    ///
    /// # Errors
    ///
    /// 管理 API 的服务出错。
    pub async fn run(self, shutdown: impl Future<Output = ()> + Send + 'static) -> Result<(), Error> {
        let (stop, stopped) = watch::channel(false);
        tokio::spawn(async move {
            shutdown.await;
            // 没有人在等也无所谓：那说明各个任务已经自己结束了。
            let _ = stop.send(true);
        });

        let nodes = tokio::spawn(node_server::serve(Arc::clone(&self.controller), self.tls, self.node_tcp, stopped.clone()));
        let addresses = tokio::spawn(addr_server::serve(self.node_udp, stopped.clone()));
        let maintenance = tokio::spawn(maintain(Arc::clone(&self.controller), stopped.clone()));
        let router = admin::router(Arc::clone(&self.controller), self.static_dir.as_deref());
        let mut admin_stopped = stopped.clone();
        let admin = axum::serve(self.admin, router.into_make_service_with_connect_info::<SocketAddr>())
            .with_graceful_shutdown(async move {
                // 发送端没了也算收到了停止的信号。
                let _ = admin_stopped.changed().await;
            })
            .await;

        // 管理 API 停了：正常情况是收到了关闭的信号，这时等节点的连接收尾（把在线的节点记为离线）。
        // 它要是因为出错而停，别的任务还在等一个不会来的信号，直接结束它们。
        addresses.abort();
        maintenance.abort();
        if *stopped.borrow() {
            if let Err(err) = nodes.await {
                tracing::error!("节点协议的服务异常结束：{err}");
            }
        } else {
            nodes.abort();
        }
        admin.map_err(Error::Socket)
    }
}

/// 后台任务：到点做延后的下发，不再等迟迟不确认的节点，清理过期的东西。
async fn maintain(controller: Arc<Controller>, mut shutdown: watch::Receiver<bool>) {
    loop {
        let next = match blocking(&controller, Controller::tick).await {
            Ok(next) => next.map(tokio::time::Instant::from_std),
            Err(err) => {
                tracing::error!("后台工作出错，稍后再试：{}", error_chain(&err));
                Some(tokio::time::Instant::now() + RETRY_DELAY)
            }
        };
        let until_next = async {
            match next {
                Some(at) => tokio::time::sleep_until(at).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            () = until_next => {}
            () = controller.wait_for_work() => {}
            _ = shutdown.changed() => return,
        }
    }
}

/// 在阻塞线程池里调用 controller 的同步方法。
///
/// 这些方法里有数据库读写，重算一个大网络也要花时间，直接在异步任务里调用会卡住运行时的工作线程。
pub(crate) async fn blocking<T: Send + 'static>(controller: &Arc<Controller>, call: impl FnOnce(&Controller) -> T + Send + 'static) -> T {
    let controller = Arc::clone(controller);
    match tokio::task::spawn_blocking(move || call(&controller)).await {
        Ok(value) => value,
        Err(err) => match err.try_into_panic() {
            // controller 里的 panic 是 bug，照原样传出去，不把它变成一个普通的错误。
            Ok(panic) => std::panic::resume_unwind(panic),
            // 运行时正在关闭，这个任务自己也马上会被丢弃。
            Err(_) => std::future::pending().await,
        },
    }
}
