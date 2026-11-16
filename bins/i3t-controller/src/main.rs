//! ice3tier 的 controller：节点协议、管理 API 和内嵌的 WebUI，一个进程。
//!
//! 这里只做装配：读参数，打开数据目录，绑端口，等退出信号。逻辑都在 `i3t-controller` 这个库里。

use std::io::IsTerminal;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, bail};
use clap::Parser;
use i3t_controller::webui::{Asset, WebUi};
use i3t_controller::{Controller, ServeOptions, Server};
use tracing_subscriber::EnvFilter;

/// 构建时打进来的 WebUI：`web/dist` 里的全部文件。构建的时候没有 `web/dist` 的话是空的（见 `build.rs`）。
static WEBUI: &[Asset] = include!(concat!(env!("OUT_DIR"), "/webui_assets.rs"));

/// 日志级别用这个环境变量调，写法和 `RUST_LOG` 相同，例如 `I3T_LOG=debug`。
const LOG_ENV: &str = "I3T_LOG";

#[derive(Parser)]
#[command(name = "i3t-controller", version, about = "ice3tier 的 controller：节点协议、管理 API 和内嵌的 WebUI")]
struct Args {
    /// 数据目录：数据库和 controller 的密钥。第一次启动时创建
    #[arg(long, env = "I3T_DATA_DIR")]
    data_dir: PathBuf,

    /// 节点连 controller 用的"主机:端口"，会写进每个注册令牌。主机可以是域名
    #[arg(long, env = "I3T_PUBLIC_ADDR")]
    public_addr: String,

    /// 节点协议的监听地址。TCP 之外，同一个端口号的 UDP 用来回答节点的地址查询
    #[arg(long, default_value = "0.0.0.0:20331")]
    listen: SocketAddr,

    /// 管理 API（和 WebUI）的监听地址。默认只在本机可达；要远程管理请用 SSH 端口转发
    #[arg(long, default_value = "127.0.0.1:20332")]
    admin_listen: SocketAddr,

    /// 从这个目录取 WebUI 的静态文件，而不用程序自带的那一份
    #[arg(long)]
    static_dir: Option<PathBuf>,

    /// 重新生成管理令牌，打印出来然后退出。原来的令牌作废。要在 controller 停着的时候执行
    #[arg(long)]
    reset_admin_token: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let filter = EnvFilter::try_from_env(LOG_ENV).unwrap_or_else(|_| EnvFilter::new("info"));
    // 输出到文件或者日志服务时不带颜色的控制符。
    tracing_subscriber::fmt().with_env_filter(filter).with_ansi(std::io::stderr().is_terminal()).with_writer(std::io::stderr).init();
    abort_on_panic();
    check_public_addr(&args.public_addr)?;
    if let Some(dir) = &args.static_dir {
        // 目录写错了的话 WebUI 只会全是 404，不如启动时就说。
        if !dir.join("index.html").is_file() {
            bail!("静态文件目录 {} 里没有 index.html", dir.display());
        }
    }

    let opened =
        Controller::open(&args.data_dir, args.public_addr.clone()).with_context(|| format!("打开数据目录 {}", args.data_dir.display()))?;
    // 发布清单里的"需要的 controller 最低版本"是和可执行文件的版本比的。
    let version = env!("CARGO_PKG_VERSION").parse().context("这个程序的版本号不是合法的语义化版本")?;
    let controller = opened.controller.with_version(version);
    let fresh_token = match opened.admin_token {
        Some(token) => Some(token),
        None if args.reset_admin_token => Some(controller.issue_admin_token().context("重新生成管理令牌")?),
        None => None,
    };
    if let Some(token) = fresh_token {
        // 令牌只在这里出现一次，直接写到标准输出，不进日志。
        println!("管理令牌（只显示这一次，请保存好）：\n\n    {token}\n");
        println!("i3tctl 和 WebUI 用它登录。丢了的话停掉 controller，用 --reset-admin-token 重新生成。");
    }
    if args.reset_admin_token {
        return Ok(());
    }

    let webui = match args.static_dir {
        Some(dir) => WebUi::Dir(dir),
        None if WEBUI.is_empty() => WebUi::NotIncluded,
        None => WebUi::Embedded(WEBUI),
    };
    let webui_from = match &webui {
        WebUi::Dir(_) => "指定的目录",
        WebUi::Embedded(_) => "程序自带",
        WebUi::NotIncluded => "这次构建没有带",
    };
    let options = ServeOptions { node_listen: args.listen, admin_listen: args.admin_listen, webui, follow_releases: true };
    let server = Server::bind(Arc::new(controller), options).await.context("绑定端口")?;
    tracing::info!(
        nodes = %server.node_addr().context("查询节点协议的监听地址")?,
        admin = %server.admin_addr().context("查询管理 API 的监听地址")?,
        public = %args.public_addr,
        webui = webui_from,
        "controller 已启动"
    );
    if !args.admin_listen.ip().is_loopback() {
        tracing::warn!(admin = %args.admin_listen, "管理 API 没有绑在回环地址上：它是明文 HTTP，管理令牌会在网络上明文传输");
    }
    server.run(shutdown_signal()).await.context("controller 运行出错")?;
    tracing::info!("controller 已停止");
    Ok(())
}

/// 等 Ctrl-C；在 Unix 上也等 SIGTERM（systemd 和容器停服务时发的是它）。
async fn shutdown_signal() {
    let interrupt = async {
        if let Err(err) = tokio::signal::ctrl_c().await {
            tracing::error!("监听 Ctrl-C 失败：{err}");
            std::future::pending::<()>().await;
        }
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(err) => {
                tracing::error!("监听 SIGTERM 失败：{err}");
                std::future::pending::<()>().await;
            }
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        () = interrupt => {}
        () = terminate => {}
    }
    tracing::info!("收到退出信号，正在停止");
}

/// 注册令牌里的地址写错了，节点拿着令牌也连不上，而且要到那时才发现。所以启动时先看一眼它的样子对不对。
fn check_public_addr(address: &str) -> anyhow::Result<()> {
    let well_formed =
        address.rsplit_once(':').is_some_and(|(host, port)| !host.is_empty() && port.parse::<u16>().is_ok_and(|port| port != 0));
    if !well_formed {
        bail!("--public-addr 要写成「主机:端口」，例如 ctl.example.com:20331；现在是 {address:?}");
    }
    Ok(())
}

/// 任何线程 panic 都让整个进程退出。
///
/// controller 的状态在一把锁后面，持锁的线程 panic 之后这把锁就中毒了，之后每个请求都会失败，
/// 进程却还活着、端口还开着。让它直接退出，由 systemd 之类的拉起来，比留着一个什么都做不了的进程好。
fn abort_on_panic() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        default_hook(info);
        std::process::abort();
    }));
}
