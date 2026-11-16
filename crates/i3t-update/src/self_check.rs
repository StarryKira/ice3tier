//! 自检：换程序之前，先让新程序在这台机器上证明自己能用。
//!
//! 约定有两头。
//!
//! **正在运行的旧程序**把验过的新程序放到自己旁边，然后运行它（这个 crate 内部做）：
//!
//! - 命令行是 `<新程序> <参数…>`，参数由节点程序在 [`UpdaterConfig::self_check_args`](crate::UpdaterConfig::self_check_args)
//!   里给，一般是 `--state-dir <状态目录> self-check`；
//! - 环境变量 [`EXPECTED_VERSION_ENV`] 是清单上的版本号，其余的环境变量原样继承；
//! - 标准输入是关着的，标准输出被丢弃，标准错误被收下来；
//! - 退出码 0 是通过。别的退出码、被信号杀掉、[`TIMEOUT`] 之内没有结束（会被杀掉），都是不通过，
//!   原因取标准错误的最后一段，报给 controller。
//!
//! **新程序**的自检子命令调用 [`self_check`]，成功就以 0 退出，失败就把原因写到标准错误、以非 0 退出。它要确认三件事：
//!
//! 1. 能启动，而且就是清单上说的那个版本（走到这个函数就说明能启动）；
//! 2. 能读现有的配置和状态；
//! 3. 能和 controller 完成一次握手。
//!
//! 自检的时候旧程序还在运行，所以新程序**不能**绑定节点的端口，不能改状态目录里的东西，
//! 也不能用节点的身份向 controller 报到：握手做到双方互相认证完就断开，否则 controller 会以为节点重连了，把旧程序的连接挤掉。

use std::ffi::OsString;
use std::fmt;
use std::io;
use std::path::Path;
use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use i3t_model::update::Version;
use tokio::process::{Child, Command};

/// 旧程序用这个环境变量告诉新程序：清单上说你是哪个版本。
pub const EXPECTED_VERSION_ENV: &str = "I3T_SELF_CHECK_VERSION";

/// 整个自检最多跑多久。握手自己有更短的时限（[`HANDSHAKE_TIMEOUT`]），正常情况下轮不到这个。
pub const TIMEOUT: Duration = Duration::from_secs(60);

/// 自检里和 controller 握手最多等多久。
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);

/// 报给 controller 的原因最多带多少个字符的标准错误。
const REASON_CHARS: usize = 400;

/// 新程序的自检没有通过的原因。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SelfCheckFailure {
    /// 发布时把版本号弄错了。装上去的话节点会一直以为自己是旧版本，反复更新。
    #[error("这个程序是 {running} 版，清单上说的是 {expected} 版")]
    WrongVersion { expected: String, running: Version },
    #[error("读不了现有的配置和状态：{0}")]
    State(String),
    #[error("没能和 controller 完成握手：{0}")]
    Handshake(String),
    #[error("{} 秒之内没能和 controller 完成握手", HANDSHAKE_TIMEOUT.as_secs())]
    HandshakeTimeout,
}

/// 新程序的自检。节点程序的自检子命令调用它，按结果决定退出码。
///
/// - `running` 是这个程序自己的版本；
/// - `load` 读现有的配置和状态，读出来的东西交给下一步；
/// - `handshake` 用它们和 controller 完成一次握手然后断开，不要报到（原因见模块文档）。
///
/// 手动运行自检子命令时没有 [`EXPECTED_VERSION_ENV`]，这时不核对版本。
///
/// # Errors
///
/// 三件事里的哪一件没有做成。
pub async fn self_check<T, E, L, H>(running: &Version, load: L, handshake: H) -> Result<(), SelfCheckFailure>
where
    L: FnOnce() -> Result<T, E>,
    H: AsyncFnOnce(T) -> Result<(), E>,
    E: fmt::Display,
{
    check(std::env::var(EXPECTED_VERSION_ENV).ok(), running, load, handshake).await
}

async fn check<T, E, L, H>(expected: Option<String>, running: &Version, load: L, handshake: H) -> Result<(), SelfCheckFailure>
where
    L: FnOnce() -> Result<T, E>,
    H: AsyncFnOnce(T) -> Result<(), E>,
    E: fmt::Display,
{
    if let Some(expected) = expected
        && expected != running.to_string()
    {
        return Err(SelfCheckFailure::WrongVersion { expected, running: running.clone() });
    }
    // `{:#}` 让 anyhow 的错误把来由一并写出来，别的错误类型不受影响。
    let loaded = load().map_err(|error| SelfCheckFailure::State(format!("{error:#}")))?;
    match tokio::time::timeout(HANDSHAKE_TIMEOUT, handshake(loaded)).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => Err(SelfCheckFailure::Handshake(format!("{error:#}"))),
        Err(_) => Err(SelfCheckFailure::HandshakeTimeout),
    }
}

/// 新程序的自检没有通过：给人看的一句话，和写成别的语言要用到的值。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Rejected {
    pub(crate) reason: String,
    /// 新程序的退出码；被信号杀掉、没有运行起来或者超时时没有。
    pub(crate) exit_code: Option<i32>,
    /// 新程序在标准错误上最后说的话；没有运行到那一步时是出错的原话。
    pub(crate) output: String,
}

impl Rejected {
    fn without_output(reason: String) -> Self {
        Self { output: reason.clone(), reason, exit_code: None }
    }
}

/// 旧程序这一头：运行放在程序旁边的新程序，看它的自检过不过。不过的话返回原因。
pub(crate) async fn run_candidate(candidate: &Path, args: &[OsString], version: &Version, limit: Duration) -> Result<(), Rejected> {
    let mut command = Command::new(candidate);
    command
        .args(args)
        .env(EXPECTED_VERSION_ENV, version.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        // 超时或者调用方不等了，这个 future 被丢弃，子进程跟着被杀掉。
        .kill_on_drop(true);
    let child = spawn(&mut command).await.map_err(|error| Rejected::without_output(format!("新程序运行不起来：{error}")))?;
    match tokio::time::timeout(limit, child.wait_with_output()).await {
        Ok(Ok(output)) if output.status.success() => Ok(()),
        Ok(Ok(output)) => {
            let said = tail(&output.stderr);
            Err(Rejected { reason: format!("{}：{said}", describe_exit(output.status)), exit_code: output.status.code(), output: said })
        }
        Ok(Err(error)) => Err(Rejected::without_output(format!("等新程序结束的时候出错：{error}"))),
        Err(_) => Err(Rejected::without_output(format!("{} 秒之内没有结束", limit.as_secs()))),
    }
}

/// 启动子进程。
///
/// 文件刚写完就运行，在 Unix 上偶尔会得到"文件正忙"：别的线程恰好在这时启动了别的子进程，
/// 那个子进程在换成自己的程序之前，还短暂地拿着我们写文件用的描述符。等一下就好，所以重试几次。
async fn spawn(command: &mut Command) -> io::Result<Child> {
    const ATTEMPTS: u32 = 20;
    const PAUSE: Duration = Duration::from_millis(50);
    for _ in 1..ATTEMPTS {
        match command.spawn() {
            Err(error) if error.kind() == io::ErrorKind::ExecutableFileBusy => tokio::time::sleep(PAUSE).await,
            result => return result,
        }
    }
    command.spawn()
}

fn describe_exit(status: ExitStatus) -> String {
    match status.code() {
        Some(code) => format!("退出码 {code}"),
        None => "被信号终止".to_owned(),
    }
}

/// 标准错误的最后一段。原因一般就在最后几行，前面的可能是很长的日志。
fn tail(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let text = text.trim();
    if text.is_empty() {
        return "没有输出原因".to_owned();
    }
    let start = text.char_indices().rev().nth(REASON_CHARS - 1).map_or(0, |(index, _)| index);
    text[start..].to_owned()
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    fn version() -> Version {
        Version::new(0, 2, 0)
    }

    async fn passing_handshake(_loaded: u8) -> Result<(), String> {
        Ok(())
    }

    #[tokio::test]
    async fn check_passes_when_all_three_things_work() {
        let loaded = Cell::new(None);
        let result = check(
            Some("0.2.0".to_owned()),
            &version(),
            || Ok::<_, String>(7u8),
            async |state| {
                loaded.set(Some(state));
                Ok(())
            },
        )
        .await;
        assert_eq!(result, Ok(()));
        assert_eq!(loaded.get(), Some(7), "读出来的配置和状态应该交给握手那一步");
    }

    #[tokio::test]
    async fn check_refuses_a_program_that_is_not_the_version_the_manifest_names() {
        let result = check(Some("0.3.0".to_owned()), &version(), || Ok::<_, String>(0u8), passing_handshake).await;
        assert_eq!(result, Err(SelfCheckFailure::WrongVersion { expected: "0.3.0".to_owned(), running: version() }));
    }

    #[tokio::test]
    async fn check_run_by_hand_does_not_compare_versions() {
        assert_eq!(check(None, &version(), || Ok::<_, String>(0u8), passing_handshake).await, Ok(()));
    }

    #[tokio::test]
    async fn unreadable_state_fails_before_any_handshake() {
        let shook_hands = Cell::new(false);
        let result = check(
            None,
            &version(),
            || Err::<u8, _>("config.toml 第 3 行写错了"),
            async |_| {
                shook_hands.set(true);
                Ok(())
            },
        )
        .await;
        assert_eq!(result, Err(SelfCheckFailure::State("config.toml 第 3 行写错了".to_owned())));
        assert!(!shook_hands.get());
    }

    #[tokio::test]
    async fn a_failed_handshake_fails_the_check() {
        let result = check(None, &version(), || Ok(0u8), async |_| Err("controller 不认这把公钥".to_owned())).await;
        assert_eq!(result, Err(SelfCheckFailure::Handshake("controller 不认这把公钥".to_owned())));
    }

    #[tokio::test(start_paused = true)]
    async fn a_handshake_that_never_finishes_times_out() {
        let started = tokio::time::Instant::now();
        let result = check(None, &version(), || Ok::<_, String>(0u8), async |_| std::future::pending().await).await;
        assert_eq!(result, Err(SelfCheckFailure::HandshakeTimeout));
        assert_eq!(started.elapsed(), HANDSHAKE_TIMEOUT);
    }

    #[test]
    fn reason_keeps_only_the_end_of_a_long_stderr() {
        assert_eq!(tail(b"  \n"), "没有输出原因");
        assert_eq!(tail("第一行\n原因在这里\n".as_bytes()), "第一行\n原因在这里");
        let long = format!("{}结尾", "很长的日志。".repeat(1000));
        let kept = tail(long.as_bytes());
        assert_eq!(kept.chars().count(), REASON_CHARS);
        assert!(kept.ends_with("结尾"));
        // 不是合法 UTF-8 的输出也不能让它出错。
        assert!(tail(&[0xff, 0xfe, b'x']).ends_with('x'));
    }

    #[cfg(unix)]
    mod candidate {
        use std::os::unix::fs::PermissionsExt;
        use std::path::PathBuf;

        use super::*;

        fn script(dir: &Path, body: &str) -> PathBuf {
            let path = dir.join("i3t.new");
            std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            path
        }

        async fn run(body: &str, limit: Duration) -> Result<(), String> {
            let dir = tempfile::tempdir().unwrap();
            let args = [OsString::from("--state-dir"), OsString::from("/var/lib/i3t"), OsString::from("self-check")];
            run_candidate(&script(dir.path(), body), &args, &version(), limit).await.map_err(|rejected| rejected.reason)
        }

        #[tokio::test]
        async fn exit_status_zero_passes() {
            assert_eq!(run("exit 0", TIMEOUT).await, Ok(()));
        }

        #[tokio::test]
        async fn candidate_receives_the_arguments_and_the_expected_version() {
            let body = r#"[ "$1 $2 $3" = "--state-dir /var/lib/i3t self-check" ] || { echo "参数不对：$*" >&2; exit 2; }
[ "$I3T_SELF_CHECK_VERSION" = 0.2.0 ] || { echo "版本不对：$I3T_SELF_CHECK_VERSION" >&2; exit 3; }"#;
            assert_eq!(run(body, TIMEOUT).await, Ok(()));
        }

        #[tokio::test]
        async fn a_non_zero_exit_fails_with_the_reason_from_stderr() {
            let result = run("echo 这一行在标准输出上，不算; echo 读不了 config.toml >&2; exit 3", TIMEOUT).await;
            assert_eq!(result, Err("退出码 3：读不了 config.toml".to_owned()));
        }

        #[tokio::test]
        async fn a_failure_keeps_the_exit_code_and_what_the_program_said() {
            let dir = tempfile::tempdir().unwrap();
            let rejected = run_candidate(&script(dir.path(), "echo 读不了 config.toml >&2; exit 3"), &[], &version(), TIMEOUT).await;
            let rejected = rejected.unwrap_err();
            assert_eq!((rejected.exit_code, rejected.output.as_str()), (Some(3), "读不了 config.toml"));
        }

        #[tokio::test]
        async fn a_candidate_killed_by_a_signal_fails() {
            assert_eq!(run("kill -9 $$", TIMEOUT).await, Err("被信号终止：没有输出原因".to_owned()));
        }

        #[tokio::test]
        async fn a_candidate_that_hangs_is_stopped_at_the_time_limit() {
            let started = std::time::Instant::now();
            let result = run("exec sleep 30", Duration::from_millis(300)).await;
            assert_eq!(result, Err("0 秒之内没有结束".to_owned()));
            assert!(started.elapsed() < Duration::from_secs(10));
        }

        #[tokio::test]
        async fn a_file_that_cannot_be_run_fails() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("i3t.new");
            std::fs::write(&path, "不是程序").unwrap();
            let result = run_candidate(&path, &[], &version(), TIMEOUT).await;
            assert!(result.unwrap_err().reason.starts_with("新程序运行不起来："));
        }
    }
}
