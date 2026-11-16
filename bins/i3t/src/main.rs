//! `i3t`：ice3tier 的节点程序。

mod show;

use std::io::{IsTerminal, Read};
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, bail};
use clap::{Parser, Subcommand, ValueEnum};
use i3t_agent::config::{Capability, Config};
use i3t_agent::join::{JoinOptions, join};
use i3t_agent::node::{self, Outcome, Source};
use i3t_agent::program::Program;
use i3t_agent::state::StateDir;
use i3t_agent::{RESTART_EXIT_CODE, Version, local, selfcheck};

/// 这个程序的版本号：报给 controller 的、`i3t --version` 显示的、更新时和发布清单比的都是它。
///
/// 它就是这个包在 `Cargo.toml` 里的版本号。唯一的例外是编译时设了环境变量 `I3T_TEST_VERSION`：
/// 端到端测试要有"同一份代码、版本号不同"的两个程序才能把更新跑一遍，用它编出第二个。
/// 发布不用它：发布流水线会核对标签和 `Cargo.toml`，清单上的版本号和程序自己报的对不上时，节点的自检也不会放行。
const VERSION: &str = match option_env!("I3T_TEST_VERSION") {
    Some(version) => version,
    None => env!("CARGO_PKG_VERSION"),
};

#[derive(Parser)]
#[command(name = "i3t", version = VERSION, about = "ice3tier 节点", propagate_version = true)]
struct Cli {
    /// 状态目录：密钥、配置文件和 netmap 的缓存都在这里
    #[arg(long, global = true, env = "I3T_STATE_DIR", value_name = "目录")]
    state_dir: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// 用注册令牌加入一个网络。令牌从标准输入读，例如：echo i3t-… | i3t join
    Join {
        /// 声明这台机器可以作为出口
        #[arg(long)]
        can_exit: bool,
        /// 声明这台机器可以作为 relay（需要公网可达）
        #[arg(long)]
        can_relay: bool,
        /// 声明这台机器可以作为端口转发的目标
        #[arg(long)]
        can_forward: bool,
    },
    /// 运行节点。通常由系统服务启动
    Run {
        /// 测试用：不连 controller，从这个 JSON 文件读 netmap
        #[arg(long, value_name = "文件")]
        r#static: Option<PathBuf>,
    },
    /// 修改本机的声明，例如：i3t set can-exit on
    Set {
        /// can-exit、can-relay 或 can-forward
        capability: Capability,
        state: Switch,
    },
    /// 查看本机的状态。节点正在运行时还会显示它现在的情况：和 controller 的连接、监听、路由、更新
    Status,
    /// 正在运行的节点到各个对端现在怎么走
    Routes,
    /// 显示本节点的公钥；还没有密钥时先生成一把
    Key,
    /// 自检：读一遍现有的配置和状态，和 controller 握一次手。更新时由正在运行的旧程序调用，也可以手动运行
    SelfCheck,
}

#[derive(Clone, Copy, ValueEnum)]
enum Switch {
    On,
    Off,
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("i3t: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> anyhow::Result<ExitCode> {
    let version: Version = VERSION.parse().context("这个程序的版本号不是合法的语义化版本")?;
    let state = StateDir::open(match cli.state_dir {
        Some(dir) => dir,
        None => default_state_dir()?,
    })?;
    match cli.command {
        Command::Join { can_exit, can_relay, can_forward } => {
            let mut token = String::new();
            std::io::stdin().read_to_string(&mut token).context("从标准输入读注册令牌")?;
            let options = JoinOptions { can_exit, can_relay, can_forward };
            let program = Program::detect(version)?;
            let netmap = current_thread()?.block_on(join(&state, &token, options, &program))?;
            println!("已加入网络 {}。", netmap.network.name);
            println!("  节点名    {}（全名 {}.{}）", netmap.me.name, netmap.me.name, netmap.network.domain);
            println!("  虚拟 IP   {}", netmap.me.virtual_ip);
            println!("  配置文件  {}", state.config_path().display());
            println!("执行 i3t run 启动节点。");
        }
        Command::Run { r#static } => {
            let config = Config::load(&state.config_path())?;
            init_logging(config.log.level.as_str());
            // 程序文件在哪里要趁早查：更新换过文件之后，系统报出来的就不是原来的位置了。
            let program = Program::detect(version)?;
            let source = r#static.map_or(Source::Controller, Source::Static);
            let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().context("启动运行时")?;
            return Ok(match runtime.block_on(node::run(state, source, program, shutdown_signal()))? {
                Outcome::Stopped => ExitCode::SUCCESS,
                // 程序文件换过了：请看护程序（systemd 之类）把进程重新拉起来。
                Outcome::Restart => ExitCode::from(RESTART_EXIT_CODE),
            });
        }
        Command::Set { capability, state: switch } => {
            let on = matches!(switch, Switch::On);
            Config::set_declared(&state.config_path(), capability, on)?;
            println!("{capability} 已设为 {}，重启 i3t 之后生效。", if on { "on" } else { "off" });
            if on {
                println!("声明本身不启用任何东西，还要管理员在 WebUI 上打开。");
            }
        }
        Command::Status => {
            print_status(&state)?;
            match current_thread()?.block_on(local::query(&state))? {
                Some(live) => show::status(&live),
                None => println!("\n节点没有在运行（上面是存在本机的配置）。"),
            }
        }
        Command::Routes => match current_thread()?.block_on(local::query(&state))? {
            Some(live) => show::routes(&live),
            None => bail!("节点没有在运行。路由表要问正在运行的节点：先用 i3t run 启动它"),
        },
        Command::Key => println!("{}", state.key_or_generate()?.public()),
        Command::SelfCheck => {
            // 不初始化日志：调用它的旧程序把标准错误的最后一段当作失败的原因报给 controller，那里只该有原因本身。
            if let Err(failure) = current_thread()?.block_on(selfcheck::self_check(&state, &version)) {
                eprintln!("{failure}");
                return Ok(ExitCode::FAILURE);
            }
            println!("自检通过：i3t {version} 读得了这台机器上的配置和状态，并且和 controller 完成了握手。");
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// 只做一两次网络往返的子命令用的运行时。
fn current_thread() -> anyhow::Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_current_thread().enable_all().build().context("启动运行时")
}

/// 日志写到标准错误。级别来自配置文件，可以用环境变量 `I3T_LOG` 临时覆盖（写法同 `RUST_LOG`，可以按模块设）。
fn init_logging(level: &str) {
    let filter = std::env::var("I3T_LOG").unwrap_or_else(|_| level.to_owned());
    let filter = tracing_subscriber::EnvFilter::try_new(&filter).unwrap_or_else(|err| {
        eprintln!("环境变量 I3T_LOG 的值 {filter:?} 看不懂（{err}），改用配置文件里的级别 {level}");
        tracing_subscriber::EnvFilter::new(level)
    });
    // 输出到文件或者日志服务时不带颜色的控制符。
    tracing_subscriber::fmt().with_env_filter(filter).with_ansi(std::io::stderr().is_terminal()).with_writer(std::io::stderr).init();
}

/// 收到 Ctrl-C，或者（Unix 上）收到 SIGTERM 时完成。
async fn shutdown_signal() {
    let interrupt = tokio::signal::ctrl_c();
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut terminate) => {
                tokio::select! {
                    _ = interrupt => {}
                    _ = terminate.recv() => {}
                }
            }
            Err(err) => {
                tracing::warn!("装不上 SIGTERM 的处理（{err}），只响应 Ctrl-C");
                let _ = interrupt.await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = interrupt.await;
    }
}

fn print_status(state: &StateDir) -> anyhow::Result<()> {
    let config = Config::load(&state.config_path())?;
    println!("状态目录  {}", state.path().display());
    println!("端口      {}", config.port);
    let declared: Vec<_> = [(config.declare.exit, "出口"), (config.declare.relay, "relay"), (config.declare.forward, "端口转发的目标")]
        .into_iter()
        .filter_map(|(on, name)| on.then_some(name))
        .collect();
    println!("本地声明  {}", if declared.is_empty() { "无".to_owned() } else { declared.join("、") });
    let (Some(controller), Some(netmap)) = (state.controller()?, state.netmap()?) else {
        println!("还没有加入网络。在 WebUI 的“添加节点”里生成注册令牌，然后执行：echo <令牌> | i3t join");
        return Ok(());
    };
    println!("controller {}", controller.address);
    println!("网络      {}（{}）", netmap.network.name, netmap.network.cidr);
    println!("节点名    {}", netmap.me.name);
    println!("虚拟 IP   {}", netmap.me.virtual_ip);
    println!("netmap    第 {} 版，{} 个对端，{} 个 relay", netmap.version, netmap.peers.len(), netmap.relays.len());
    Ok(())
}

/// 没有用 `--state-dir` 指定时状态目录放在哪里。
///
/// 装成系统服务时安装脚本会建好 `/var/lib/i3t`，存在就用它；否则放在当前用户自己的目录里。
fn default_state_dir() -> anyhow::Result<PathBuf> {
    let system = PathBuf::from("/var/lib/i3t");
    if cfg!(unix) && system.is_dir() {
        return Ok(system);
    }
    let from_env = |name: &str| std::env::var_os(name).filter(|value| !value.is_empty()).map(PathBuf::from);
    let dir = if cfg!(windows) {
        from_env("LOCALAPPDATA").map(|base| base.join("i3t"))
    } else if cfg!(target_os = "macos") {
        from_env("HOME").map(|home| home.join("Library/Application Support/i3t"))
    } else {
        from_env("XDG_STATE_HOME").map(|base| base.join("i3t")).or_else(|| from_env("HOME").map(|home| home.join(".local/state/i3t")))
    };
    match dir {
        Some(dir) => Ok(dir),
        None => bail!("找不到放状态目录的位置，请用 --state-dir 指定一个目录"),
    }
}
