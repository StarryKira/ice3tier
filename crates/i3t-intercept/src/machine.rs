//! 和系统之间的接缝：执行 `nft`、`ip`、`resolvectl`，看本进程有没有需要的能力。
//!
//! 装、拆、读回都只经过 [`Machine`]，所以单元测试换一个假的机器就能把整个过程走一遍，不需要 root。
//! 真的机器是 [`Shell`]：它执行系统里的程序。用程序而不是直接说 netlink，是因为 nft 的脚本是一个事务，
//! 程序自己保证要么全换上、要么一点不动；出错时它给的说明也正是使用者该看的。

use std::fmt;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// 要执行的程序。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Program {
    Nft,
    Ip,
    Resolvectl,
}

impl Program {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Nft => "nft",
            Self::Ip => "ip",
            Self::Resolvectl => "resolvectl",
        }
    }
}

impl fmt::Display for Program {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// 执行一个程序没有成功。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CommandError {
    /// 系统里没有这个程序。
    #[error("找不到程序 {0}")]
    Missing(Program),
    /// 程序执行了，但是报错退出。`detail` 是它在标准错误上说的话（去掉了首尾的空白）。
    #[error("{program} 失败（退出码 {status:?}）：{detail}")]
    Failed { program: Program, status: Option<i32>, detail: String },
    /// 程序没能启动起来，或者和它之间的管道出错。
    #[error("执行 {program} 出错：{detail}")]
    Io { program: Program, detail: String },
}

impl CommandError {
    /// 程序说的话里有 `needle`（不分大小写）。用来分辨"东西本来就不在"这类可以当作成功的失败。
    #[must_use]
    pub fn says(&self, needle: &str) -> bool {
        match self {
            Self::Failed { detail, .. } => detail.to_lowercase().contains(&needle.to_lowercase()),
            Self::Missing(_) | Self::Io { .. } => false,
        }
    }
}

/// 执行程序的地方。真的是 [`Shell`]，测试里是一个按命令应答的假机器。
pub trait Machine {
    /// 执行 `program args…`，`input` 交给它的标准输入。成功时返回它的标准输出。
    ///
    /// # Errors
    ///
    /// 程序不存在、启动不了，或者以非零的退出码结束。
    fn run(&mut self, program: Program, args: &[String], input: Option<&str>) -> Result<String, CommandError>;
}

/// 真的系统：在 `PATH` 和几个常见的系统目录里找程序，执行它。
///
/// 系统服务的 `PATH` 里通常有 `/usr/sbin`，用户的 shell 里不一定有，所以另外看那几个目录。
#[derive(Debug, Clone, Copy, Default)]
pub struct Shell;

/// `PATH` 之外再看的目录。
const SYSTEM_DIRS: [&str; 4] = ["/usr/sbin", "/sbin", "/usr/bin", "/bin"];

impl Shell {
    /// 程序在哪里；找不到时是 `None`。
    #[must_use]
    pub fn locate(program: Program) -> Option<PathBuf> {
        let from_path = std::env::var_os("PATH").map(|path| std::env::split_paths(&path).collect::<Vec<_>>()).unwrap_or_default();
        from_path
            .into_iter()
            .chain(SYSTEM_DIRS.iter().map(PathBuf::from))
            .map(|dir| dir.join(program.name()))
            .find(|path| is_executable(path))
    }
}

fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata().is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

impl Machine for Shell {
    fn run(&mut self, program: Program, args: &[String], input: Option<&str>) -> Result<String, CommandError> {
        let path = Self::locate(program).ok_or(CommandError::Missing(program))?;
        let io = |error: std::io::Error| CommandError::Io { program, detail: error.to_string() };
        let mut child = Command::new(path)
            .args(args)
            // 程序的输出要按固定的写法解析，不能跟着使用者的语言变。
            .env("LC_ALL", "C")
            .stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(io)?;
        if let (Some(text), Some(mut stdin)) = (input, child.stdin.take()) {
            stdin.write_all(text.as_bytes()).map_err(io)?;
        }
        let output = child.wait_with_output().map_err(io)?;
        if output.status.success() {
            return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
        }
        Err(CommandError::Failed {
            program,
            status: output.status.code(),
            detail: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        })
    }
}

/// 透明访问要的进程能力。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Capability {
    /// 改包过滤规则、路由、网卡；`IP_TRANSPARENT`、`SO_MARK`。
    NetAdmin,
    /// 绑 1024 以下的端口：入口的 [`PORT`](crate::PORT) 和名字服务的 53。
    NetBindService,
}

impl Capability {
    pub const ALL: [Self; 2] = [Self::NetAdmin, Self::NetBindService];

    /// 内核里的编号（`linux/capability.h`）。
    const fn bit(self) -> u32 {
        match self {
            Self::NetAdmin => 12,
            Self::NetBindService => 10,
        }
    }

    /// 写法和 systemd 的 `AmbientCapabilities=` 一样。
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::NetAdmin => "CAP_NET_ADMIN",
            Self::NetBindService => "CAP_NET_BIND_SERVICE",
        }
    }

    /// 本进程缺哪些能力。读不出来（不是 Linux）时一律算缺。
    #[must_use]
    pub fn missing() -> Vec<Self> {
        let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
        Self::missing_in(&status)
    }

    /// `/proc/<pid>/status` 的内容里，`CapEff` 缺哪些能力。
    fn missing_in(status: &str) -> Vec<Self> {
        let effective = status
            .lines()
            .find_map(|line| line.strip_prefix("CapEff:"))
            .and_then(|hex| u64::from_str_radix(hex.trim(), 16).ok())
            .unwrap_or(0);
        Self::ALL.into_iter().filter(|capability| effective & (1 << capability.bit()) == 0).collect()
    }
}

impl fmt::Display for Capability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effective_capabilities_are_read_from_the_status_file() {
        let root = "Name:\ti3t\nCapInh:\t0000000000000000\nCapPrm:\t000001ffffffffff\nCapEff:\t000001ffffffffff\n";
        assert_eq!(Capability::missing_in(root), []);
        let plain = "CapEff:\t0000000000000000\n";
        assert_eq!(Capability::missing_in(plain), [Capability::NetAdmin, Capability::NetBindService]);
        // systemd 的 AmbientCapabilities=CAP_NET_ADMIN：只有第 12 位。
        let admin_only = "CapEff:\t0000000000001000\n";
        assert_eq!(Capability::missing_in(admin_only), [Capability::NetBindService]);
        assert_eq!(Capability::missing_in(""), Capability::ALL, "读不出来时一律算缺");
    }

    #[test]
    fn a_failure_can_be_matched_by_what_the_program_said() {
        let error = CommandError::Failed { program: Program::Ip, status: Some(2), detail: "Error: ipv4: FIB table does not exist.".into() };
        assert!(error.says("fib table does not exist"));
        assert!(!CommandError::Missing(Program::Ip).says("does not exist"));
    }

    #[cfg(unix)]
    #[test]
    fn programs_are_found_on_the_path_or_in_the_system_directories() {
        let mut shell = Shell;
        let version = shell.run(Program::Nft, &["--version".into()], None);
        match Shell::locate(Program::Nft) {
            Some(_) => assert!(version.unwrap().contains("nftables")),
            None => assert_eq!(version, Err(CommandError::Missing(Program::Nft)), "没有这个程序要说出来"),
        }
        let wrong = shell.run(Program::Ip, &["no-such-object".into()], None);
        match Shell::locate(Program::Ip) {
            Some(path) => {
                assert!(path.ends_with("ip"));
                assert!(matches!(wrong, Err(CommandError::Failed { program: Program::Ip, .. })), "报错退出要带着它说的话：{wrong:?}");
            }
            None => assert_eq!(wrong, Err(CommandError::Missing(Program::Ip))),
        }
    }
}
