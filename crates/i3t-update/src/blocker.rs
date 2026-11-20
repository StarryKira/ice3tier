//! 这个节点能不能自己更新。
//!
//! 判断故意做得简单，宁可漏判也不去猜：漏判的后果只是 WebUI 上"立即更新"点了之后报一个明确的错，
//! 误判的后果是一个本来能更新的节点被标成不能。三条依次检查：
//!
//! 1. **在容器里**（只看 Linux）：有 `/.dockerenv`（Docker）或 `/run/.containerenv`（Podman），
//!    或者环境变量里有 `KUBERNETES_SERVICE_HOST`（Kubernetes）或非空的 `container`（Podman、systemd-nspawn 等会设）。
//!    像虚拟机一样用的系统容器（LXC）里，服务进程看不到这些标记，按普通机器对待：它的文件系统是持久的，自己更新没有问题。
//! 2. **包管理器装的**：只看程序文件的路径。在 `/usr` 下面（`/usr/local` 除外）、`/bin`、`/sbin`、`/nix/store`、`/snap`、
//!    `/opt/homebrew`、`/home/linuxbrew` 里，或者路径中有一级目录叫 `Cellar`、`WindowsApps`、`WinGet`、`scoop`、`chocolatey`。
//!    不去问 dpkg 或 rpm。
//! 3. **目录不可写**：在程序文件所在的目录里实际建一个文件试试。只读挂载、权限不够、服务单元的保护选项，都是这样才试得出来。

use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

use i3t_model::update::SelfUpdateBlocker;

/// 放着这些文件就说明在容器里。
const CONTAINER_MARKER_FILES: [&str; 2] = ["/.dockerenv", "/run/.containerenv"];

/// 设了这些环境变量（并且不是空的）就说明在容器里。
const CONTAINER_MARKER_VARS: [&str; 2] = ["KUBERNETES_SERVICE_HOST", "container"];

/// 这些目录归包管理器管。
const MANAGED_ROOTS: [&str; 7] = ["/usr", "/bin", "/sbin", "/nix/store", "/snap", "/opt/homebrew", "/home/linuxbrew"];

/// `/usr` 下面留给手动安装的那一块。
const UNMANAGED_ROOT: &str = "/usr/local";

/// 路径里有一级目录叫这些名字，说明是包管理器放的：Homebrew、Windows 应用商店、winget、Scoop、Chocolatey。
const MANAGED_DIR_NAMES: [&str; 5] = ["Cellar", "WindowsApps", "WinGet", "scoop", "chocolatey"];

/// 正在运行的程序文件的真实路径（符号链接已经解开）。
///
/// 进程一启动就要调用并把结果留着：程序文件被换过之后，系统报出来的"自己的路径"指向的就不是原来的位置了。
///
/// # Errors
///
/// 系统给不出当前程序的路径。
pub fn current_program() -> io::Result<PathBuf> {
    std::env::current_exe()?.canonicalize()
}

/// 这个节点不能自己更新的原因；能的话是 `None`。`program` 是 [`current_program`] 的结果。
///
/// 会在程序所在的目录里建一个文件再删掉，所以只在启动时调用一次，结果报给 controller 并交给
/// [`UpdaterConfig::blocker`](crate::UpdaterConfig::blocker)。
#[must_use]
pub fn self_update_blocker(program: &Path) -> Option<SelfUpdateBlocker> {
    let blocker = if cfg!(target_os = "linux") && in_container(Path::exists, |name| std::env::var_os(name)) {
        Some(SelfUpdateBlocker::Container)
    } else if is_package_managed(program) {
        Some(SelfUpdateBlocker::PackageManager)
    } else if !dir_is_writable(program) {
        Some(SelfUpdateBlocker::ReadOnlyDir)
    } else {
        None
    };
    if let Some(blocker) = blocker {
        tracing::info!(program = %program.display(), reason = %blocker, "这个节点不能自己更新");
    }
    blocker
}

fn in_container(exists: impl Fn(&Path) -> bool, var: impl Fn(&str) -> Option<OsString>) -> bool {
    CONTAINER_MARKER_FILES.iter().any(|file| exists(Path::new(file)))
        || CONTAINER_MARKER_VARS.iter().any(|name| var(name).is_some_and(|value| !value.is_empty()))
}

fn is_package_managed(program: &Path) -> bool {
    let under_managed_root = MANAGED_ROOTS.iter().any(|root| program.starts_with(root)) && !program.starts_with(UNMANAGED_ROOT);
    let in_managed_dir = program.parent().is_some_and(|dir| {
        dir.components().any(|component| MANAGED_DIR_NAMES.iter().any(|name| component.as_os_str().eq_ignore_ascii_case(name)))
    });
    under_managed_root || in_managed_dir
}

fn dir_is_writable(program: &Path) -> bool {
    let Some(dir) = program.parent() else { return false };
    let probe = dir.join(format!(".i3t-update-probe-{}", std::process::id()));
    let created = OpenOptions::new().write(true).create(true).truncate(true).open(&probe).is_ok();
    if created && let Err(error) = fs::remove_file(&probe) {
        tracing::warn!(path = %probe.display(), %error, "删不掉试写用的文件");
    }
    created
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn container_is_recognised_by_marker_files_and_variables() {
        let nothing = |_: &Path| false;
        let no_vars = |_: &str| None;
        assert!(!in_container(nothing, no_vars));
        assert!(in_container(|path| path == Path::new("/.dockerenv"), no_vars));
        assert!(in_container(|path| path == Path::new("/run/.containerenv"), no_vars));
        assert!(in_container(nothing, |name| (name == "KUBERNETES_SERVICE_HOST").then(|| OsString::from("10.0.0.1"))));
        assert!(in_container(nothing, |name| (name == "container").then(|| OsString::from("podman"))));
        assert!(!in_container(nothing, |name| (name == "container").then(OsString::new)), "空的变量不算");
        assert!(!in_container(nothing, |name| (name == "HOME").then(|| OsString::from("/root"))));
    }

    #[test]
    fn package_manager_installs_are_recognised_by_path() {
        let managed = [
            "/usr/bin/i3t",
            "/usr/sbin/i3t",
            "/usr/lib/i3t/i3t",
            "/bin/i3t",
            "/nix/store/abc123-i3t-0.2.0/bin/i3t",
            "/snap/i3t/12/bin/i3t",
            "/opt/homebrew/bin/i3t",
            "/usr/local/Cellar/i3t/0.2.0/bin/i3t",
            "/home/linuxbrew/.linuxbrew/bin/i3t",
            "/Users/me/scoop/apps/i3t/current/i3t.exe",
            "/ProgramData/Chocolatey/lib/i3t/tools/i3t.exe",
            "/Program Files/WindowsApps/i3t_1.0/i3t.exe",
        ];
        for path in managed {
            assert!(is_package_managed(Path::new(path)), "{path} 应该算包管理器装的");
        }
        let manual = [
            "/var/lib/i3t/bin/i3t",
            "/usr/local/bin/i3t",
            "/opt/i3t/i3t",
            "/home/me/bin/i3t",
            "/home/me/.local/bin/i3t",
            "/usrlocal/i3t",
            // 程序自己叫这个名字不算，要的是它上面的某一级目录。
            "/home/me/bin/scoop",
        ];
        for path in manual {
            assert!(!is_package_managed(Path::new(path)), "{path} 不应该算包管理器装的");
        }
    }

    #[test]
    fn writability_is_found_out_by_actually_writing() {
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("i3t");
        assert!(dir_is_writable(&program));
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0, "试写的文件不应该留下");
        assert!(!dir_is_writable(&dir.path().join("missing-dir").join("i3t")));
    }

    #[cfg(unix)]
    #[test]
    fn a_read_only_directory_blocks_self_update() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("i3t");
        fs::write(&program, "").unwrap();
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o555)).unwrap();
        // root 不受权限位的限制，那样的环境里这一条测不出来。
        let restricted = fs::File::create(dir.path().join("probe")).is_err();
        let writable = dir_is_writable(&program);
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(writable, !restricted);
    }

    #[test]
    fn the_running_test_program_has_a_real_path() {
        let program = current_program().unwrap();
        assert!(program.is_absolute() && program.is_file(), "{}", program.display());
    }
}
