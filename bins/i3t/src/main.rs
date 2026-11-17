//! `i3t`：ice3tier 的节点程序。
//!
//! 终端上的文字跟着系统的语言（见 `i3t_lang`），日志不翻译。

mod connect;
mod show;
mod texts;
#[cfg(target_os = "linux")]
mod tune;

use std::io::{IsTerminal, Read};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, bail};
use clap::{CommandFactory, Parser, Subcommand};
use i3t_agent::config::{Change, Config, Setting};
use i3t_agent::join::{JoinOptions, join};
use i3t_agent::node::{self, Outcome, Source};
use i3t_agent::program::Program;
use i3t_agent::state::StateDir;
use i3t_agent::{RESTART_EXIT_CODE, Version, local, selfcheck};
use i3t_lang::Lang;
use i3t_lang::table::Fields;
use i3t_update::SelfCheckFailure;

use crate::connect::Destination;
use crate::texts::texts;

/// 这个程序的版本号：报给 controller 的、`i3t --version` 显示的、更新时和发布清单比的都是它。
///
/// 它就是这个包在 `Cargo.toml` 里的版本号。唯一的例外是编译时设了环境变量 `I3T_TEST_VERSION`：
/// 端到端测试要有"同一份代码、版本号不同"的两个程序才能把更新跑一遍，用它编出第二个。
/// 发布不用它：发布流水线会核对标签和 `Cargo.toml`，清单上的版本号和程序自己报的对不上时，节点的自检也不会放行。
const VERSION: &str = match option_env!("I3T_TEST_VERSION") {
    Some(version) => version,
    None => env!("CARGO_PKG_VERSION"),
};

// 帮助的文字在文字目录里（`texts`），由 `command` 按语言装上去，所以这里不写文档注释：clap 会把它们当成说明。
#[derive(Parser)]
#[command(name = "i3t", version = VERSION)]
struct Cli {
    #[arg(long, global = true, env = "I3T_STATE_DIR")]
    state_dir: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Join {
        #[arg(long)]
        can_exit: bool,
        #[arg(long)]
        can_relay: bool,
        #[arg(long)]
        can_forward: bool,
    },
    Run {
        #[arg(long)]
        r#static: Option<PathBuf>,
    },
    Set {
        #[arg(value_parser = parse_setting)]
        setting: Setting,
        value: String,
    },
    Status,
    Routes,
    Key,
    Connect {
        host: String,
        port: u16,
        #[arg(long)]
        via: Option<String>,
    },
    Tune {
        #[arg(long)]
        apply: bool,
    },
    SelfCheck,
}

/// `i3t set` 的设置项。解析的错误由节点的文字目录按语言写。
fn parse_setting(text: &str) -> Result<Setting, String> {
    text.parse().map_err(|error: anyhow::Error| error.to_string())
}

/// 命令行的定义，帮助按 `lang` 写。
fn command(lang: Lang) -> clap::Command {
    let (h, v) = (&texts::texts_in(lang).help, &texts::texts_in(lang).values);
    Cli::command()
        .about(h.about)
        .mut_arg("state_dir", |arg| arg.help(h.state_dir).value_name(v.dir))
        .mut_subcommand("join", |sub| {
            sub.about(h.join)
                .mut_arg("can_exit", |arg| arg.help(h.can_exit))
                .mut_arg("can_relay", |arg| arg.help(h.can_relay))
                .mut_arg("can_forward", |arg| arg.help(h.can_forward))
        })
        .mut_subcommand("run", |sub| sub.about(h.run).mut_arg("static", |arg| arg.help(h.run_static).value_name(v.file)))
        .mut_subcommand("set", |sub| {
            sub.about(h.set)
                .mut_arg("setting", |arg| arg.value_name("can-exit|can-relay|can-forward|throughput-tests|profile"))
                .mut_arg("value", |arg| arg.value_name("on|off|low-footprint|balanced|high-throughput"))
        })
        .mut_subcommand("status", |sub| sub.about(h.status))
        .mut_subcommand("routes", |sub| sub.about(h.routes))
        .mut_subcommand("key", |sub| sub.about(h.key))
        .mut_subcommand("connect", |sub| {
            sub.about(h.connect)
                .mut_arg("host", |arg| arg.help(h.connect_host).value_name(v.host))
                .mut_arg("port", |arg| arg.value_name(v.port))
                .mut_arg("via", |arg| arg.help(h.connect_via).value_name(v.exit))
        })
        .mut_subcommand("tune", |sub| sub.about(h.tune).mut_arg("apply", |arg| arg.help(h.tune_apply)))
        .mut_subcommand("self-check", |sub| sub.about(h.self_check))
}

fn main() -> ExitCode {
    let lang = i3t_lang::set(Lang::detect());
    match run(i3t_lang::cli::parse(command(lang), lang)) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("i3t: {}", lang.chain(error.chain()));
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> anyhow::Result<ExitCode> {
    let t = &texts().ui;
    let version: Version = VERSION.parse().context(t.bad_version)?;
    let state = StateDir::open(match cli.state_dir {
        Some(dir) => dir,
        None => default_state_dir()?,
    })?;
    match cli.command {
        Command::Join { can_exit, can_relay, can_forward } => {
            let mut token = String::new();
            std::io::stdin().read_to_string(&mut token).context(t.read_token)?;
            let options = JoinOptions { can_exit, can_relay, can_forward };
            let program = Program::detect(version)?;
            let netmap = current_thread()?.block_on(join(&state, &token, options, &program))?;
            println!("{}", (t.joined)(&netmap.network.name));
            let mut fields = Fields::new(2);
            fields.add(t.node, netmap.me.name.clone());
            fields.add(t.fqdn, format!("{}.{}", netmap.me.name, netmap.network.domain));
            fields.add(t.virtual_ip, netmap.me.virtual_ip.to_string());
            fields.add(t.config_file, state.config_path().display().to_string());
            fields.print();
            println!("{}", t.start_with_run);
        }
        Command::Run { r#static } => {
            let config = Config::load(&state.config_path())?;
            init_logging(config.log.level.as_str());
            #[cfg(unix)]
            raise_open_file_limit();
            // 程序文件在哪里要趁早查：更新换过文件之后，系统报出来的就不是原来的位置了。
            let program = Program::detect(version)?;
            let source = r#static.map_or(Source::Controller, Source::Static);
            let threads = node::starting_performance(&state, &source)?.threads;
            let runtime = runtime(threads).context(t.start_runtime)?;
            return Ok(match runtime.block_on(node::run(state, source, program, threads, shutdown_signal()))? {
                Outcome::Stopped => ExitCode::SUCCESS,
                // 程序文件换过了：请看护程序（systemd 之类）把进程重新拉起来。
                Outcome::Restart => ExitCode::from(RESTART_EXIT_CODE),
            });
        }
        Command::Set { setting, value } => {
            let change = setting.change(&value)?;
            Config::set(&state.config_path(), change)?;
            println!("{}", (t.setting_set)(&setting.to_string(), &change.to_string()));
            match change {
                Change::Declare(_, true) => println!("{}", t.declare_hint),
                Change::Profile(profile) => {
                    // 读不出上次的 netmap 只是少一句提示。
                    let chosen = state.netmap().ok().flatten().and_then(|netmap| netmap.me.profile);
                    if let Some(chosen) = chosen.filter(|chosen| *chosen != profile) {
                        println!("{}", (t.profile_chosen)(chosen.spelling()));
                    }
                }
                Change::Declare(_, false) | Change::ThroughputTests(_) => {}
            }
        }
        Command::Status => {
            show::local(&state)?;
            match current_thread()?.block_on(local::query(&state))? {
                Some(live) => show::status(&live),
                None => println!("\n{}", t.not_running),
            }
        }
        Command::Routes => match current_thread()?.block_on(local::query(&state))? {
            Some(live) => show::routes(&live),
            None => bail!(t.routes_not_running),
        },
        Command::Key => println!("{}", state.key_or_generate()?.public()),
        Command::Connect { host, port, via } => {
            let config = Config::load(&state.config_path())?;
            let Some(listen) = config.proxy.socks5.addr() else { bail!(t.socks5_off) };
            let domain = state.netmap()?.map(|netmap| netmap.network.domain);
            let destination = Destination::new(host, port, via, domain.as_deref());
            let runtime = current_thread()?;
            let result = runtime.block_on(connect::connect(reachable_locally(listen), &destination));
            // 读标准输入的那个线程可能还卡在读上，它是取消不了的：不等它，直接退出。
            runtime.shutdown_background();
            result?;
        }
        Command::Tune { apply } => {
            let config = Config::load(&state.config_path())?;
            tune(config.port, apply)?;
        }
        Command::SelfCheck => {
            // 不初始化日志：调用它的旧程序把标准错误的最后一段当作失败的原因报给 controller，那里只该有原因本身。
            if let Err(failure) = current_thread()?.block_on(selfcheck::self_check(&state, &version)) {
                eprintln!("{}", self_check_failure(&failure));
                return Ok(ExitCode::FAILURE);
            }
            println!("{}", (t.self_check_passed)(&version.to_string()));
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// 自检没有通过的原因，按这个程序的语言写。它会被旧程序收下、报给 controller。
fn self_check_failure(failure: &SelfCheckFailure) -> String {
    let t = &texts().ui;
    match failure {
        SelfCheckFailure::WrongVersion { expected, running } => (t.wrong_version)(&running.to_string(), expected),
        SelfCheckFailure::State(reason) => (t.state_unreadable)(reason),
        SelfCheckFailure::Handshake(reason) => (t.handshake_failed)(reason),
        SelfCheckFailure::HandshakeTimeout => (t.handshake_timeout)(i3t_update::self_check::HANDSHAKE_TIMEOUT.as_secs()),
    }
}

#[cfg(target_os = "linux")]
fn tune(port: u16, apply: bool) -> anyhow::Result<()> {
    tune::run(port, apply)
}

#[cfg(not(target_os = "linux"))]
fn tune(_port: u16, _apply: bool) -> anyhow::Result<()> {
    bail!(texts().ui.tune_linux_only)
}

/// 从本机连一个监听地址该用的地址：绑在"所有地址"上的监听，连它的回环地址。
fn reachable_locally(listen: SocketAddr) -> SocketAddr {
    let ip = match listen.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(ip) if ip.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
        ip => ip,
    };
    SocketAddr::new(ip, listen.port())
}

/// `i3t run` 的运行时：一个工作线程时是单线程的运行时，多个时是多线程的（线程数由性能档位定，见 `i3t_agent::performance`）。
///
/// 同时让 glibc 的分配器尽快把空出来的内存还给系统。本机实验环境里空闲的节点因此少占约 1.4MB，满速搬运的 CPU 量不出差别。
fn runtime(threads: usize) -> std::io::Result<tokio::runtime::Runtime> {
    return_memory_promptly();
    if threads <= 1 {
        return tokio::runtime::Builder::new_current_thread().enable_all().build();
    }
    tokio::runtime::Builder::new_multi_thread().worker_threads(threads).enable_all().build()
}

/// glibc 默认会把释放的内存留在进程里，大块的阈值还会随用量往上调，一次大流量之后常驻内存就降不下来。
/// 把两个阈值固定在 128KB：大于它的分配直接向系统要、用完就还，堆顶空出这么多就还。
#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn return_memory_promptly() {
    const THRESHOLD: libc::c_int = 128 * 1024;
    for (option, name) in [(libc::M_TRIM_THRESHOLD, "M_TRIM_THRESHOLD"), (libc::M_MMAP_THRESHOLD, "M_MMAP_THRESHOLD")] {
        // SAFETY: `mallopt` 只改分配器的参数，不碰任何调用方的内存；两个参数都是 glibc 认识的选项，值在它允许的范围里。
        // 这时还没有别的线程。
        if unsafe { libc::mallopt(option, THRESHOLD) } == 0 {
            tracing::debug!(option = name, "glibc 不接受这个分配器参数");
        }
    }
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
fn return_memory_promptly() {}

/// 只做一两次网络往返的子命令用的运行时。
fn current_thread() -> anyhow::Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_current_thread().enable_all().build().context(texts().ui.start_runtime)
}

/// 日志写到标准错误。级别来自配置文件，可以用环境变量 `I3T_LOG` 临时覆盖（写法同 `RUST_LOG`，可以按模块设）。
fn init_logging(level: &str) {
    let filter = std::env::var("I3T_LOG").unwrap_or_else(|_| level.to_owned());
    let filter = tracing_subscriber::EnvFilter::try_new(&filter).unwrap_or_else(|err| {
        eprintln!("{}", (texts().ui.bad_log_env)(&filter, &err.to_string(), level));
        tracing_subscriber::EnvFilter::new(level)
    });
    // 输出到文件或者日志服务时不带颜色的控制符。
    tracing_subscriber::fmt().with_env_filter(filter).with_ansi(std::io::stderr().is_terminal()).with_writer(std::io::stderr).init();
}

/// 把能同时打开的文件数的软上限抬到硬上限。
///
/// 经过本节点的每条流在入口或者出口上占一个文件描述符（客户端的连接、到目标的连接），每个数据报上下文占一个 UDP 端口。
/// 直接在终端里执行 `i3t run` 时软上限常常只有 1024，一千来条流就会让整个节点打不开新的连接。
/// 硬上限是系统允许这个用户自己抬到的数，抬上去不需要特权。装成系统服务时单元文件里另外设了 `LimitNOFILE`，这里只是兜底。
/// macOS 的硬上限常常写着"不限"，实际能抬到的是 `kern.maxfilesperproc`，`rlimit` 会按它取。
#[cfg(unix)]
fn raise_open_file_limit() {
    match rlimit::increase_nofile_limit(u64::MAX) {
        Ok(limit) => tracing::info!(limit, "能同时打开的文件数的上限"),
        Err(error) => tracing::warn!(%error, "抬不高能同时打开的文件数的上限，流多的时候可能打不开新的连接"),
    }
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
        None => bail!(texts().ui.no_state_dir),
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use i3t_lang::cli::{CLAP_ENGLISH, all_help, has_chinese, try_parse_from};

    use super::*;

    #[test]
    fn the_help_of_every_command_is_written_in_each_language_without_the_other() {
        for lang in Lang::ALL {
            i3t_lang::cli::localize(command(lang), lang).debug_assert();
        }
        for (path, help) in all_help(command(Lang::English), Lang::English) {
            assert!(!has_chinese(&help), "{path} 的英文帮助里有中文：\n{help}");
        }
        for (path, help) in all_help(command(Lang::ZhCn), Lang::ZhCn) {
            for english in CLAP_ENGLISH {
                assert!(!help.contains(english), "{path} 的中文帮助里有 {english}：\n{help}");
            }
            assert!(has_chinese(&help), "{path}：\n{help}");
        }
    }

    #[test]
    fn commands_parse_and_a_wrong_setting_is_refused() {
        let parse = |args: &[&str]| try_parse_from::<Cli>(command(Lang::English), Lang::English, args.iter().map(OsString::from));
        assert!(matches!(parse(&["i3t", "set", "can-exit", "on"]).unwrap().command, Command::Set { setting: Setting::Declare(_), .. }));
        assert!(matches!(
            parse(&["i3t", "set", "profile", "low-footprint"]).unwrap().command,
            Command::Set { setting: Setting::Profile, .. }
        ));
        assert!(parse(&["i3t", "set", "can-fly", "on"]).is_err());
        assert!(matches!(parse(&["i3t", "connect", "nas", "22", "--via", "home"]).unwrap().command, Command::Connect { .. }));
    }

    #[cfg(unix)]
    #[test]
    fn the_soft_open_file_limit_is_raised_as_far_as_the_hard_limit_allows() {
        use rlimit::Resource;

        let (_, hard) = Resource::NOFILE.get().unwrap();
        let low = hard.min(256);
        Resource::NOFILE.set(low, hard).unwrap();

        super::raise_open_file_limit();

        let (soft, still_hard) = Resource::NOFILE.get().unwrap();
        assert_eq!(still_hard, hard, "硬上限不动");
        if cfg!(target_os = "linux") {
            assert_eq!(soft, hard);
        } else {
            assert!(soft > low || low == hard, "软上限应该抬上去了：{low} -> {soft}（硬上限 {hard}）");
        }
    }
}
