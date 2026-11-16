//! 程序文件的替换与换回。
//!
//! 程序旁边会出现这几个文件（以 `i3t` 为例，Windows 上标记插在 `.exe` 前面）：
//!
//! | 文件 | 是什么 | 什么时候有 |
//! |---|---|---|
//! | `i3t.new` | 验过、等着换上去的新程序 | 自检开始到换上去之前 |
//! | `i3t.old` | 被换下来的旧程序 | 换上去之后，到新程序被确认或者被换回之前 |
//! | `i3t.failed` | 被换回时挪开的新程序 | 只在 Windows 上，到下一次启动为止 |
//!
//! 它们和程序在同一个目录里，所以改名不会跨文件系统，是原子的。

use std::ffi::OsString;
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};

/// 正在运行的程序文件能怎么换。两种做法都只用到改名，哪个系统上都能跑，所以测试可以把两种都测到。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Swap {
    /// 把新文件改名盖上去。Unix 上正在运行的程序文件可以被盖掉，进程继续用着旧的那一份。
    /// 这样做任何时刻程序的路径上都有一个完整的文件。
    Overwrite,
    /// 先把正在运行的文件改名挪开，再把新文件改名过来。Windows 不许覆盖或者删除正在运行的程序文件，但许改名。
    /// 两次改名之间有一瞬间程序的路径上没有文件，这一瞬间断电的话要手动把备份改回来。
    MoveAside,
}

impl Swap {
    pub const NATIVE: Self = if cfg!(windows) { Self::MoveAside } else { Self::Overwrite };
}

#[derive(Debug, Clone)]
pub(crate) struct ProgramFiles {
    program: PathBuf,
    swap: Swap,
}

impl ProgramFiles {
    pub fn new(program: PathBuf, swap: Swap) -> Self {
        Self { program, swap }
    }

    pub fn program(&self) -> &Path {
        &self.program
    }

    /// 等着换上去的新程序。
    pub fn candidate(&self) -> PathBuf {
        self.sibling("new")
    }

    /// 被换下来的旧程序。
    pub fn backup(&self) -> PathBuf {
        self.sibling("old")
    }

    fn discarded(&self) -> PathBuf {
        self.sibling("failed")
    }

    /// 程序旁边带标记的文件。`.exe` 留在最后：Windows 上要运行的文件得是这个扩展名。
    fn sibling(&self, tag: &str) -> PathBuf {
        let name = self.program.file_name().unwrap_or_default();
        let is_exe = self.program.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("exe"));
        let mut tagged = OsString::from(if is_exe { self.program.file_stem().unwrap_or(name) } else { name });
        tagged.push(".");
        tagged.push(tag);
        if is_exe {
            tagged.push(".exe");
        }
        self.program.with_file_name(tagged)
    }

    /// 把验过的包拷到程序旁边，权限照着现在的程序。这是阻塞的调用。
    pub fn place_candidate(&self, package: &Path) -> io::Result<()> {
        let candidate = self.candidate();
        remove_if_exists(&candidate)?;
        let mut file = File::create_new(&candidate)?;
        io::copy(&mut File::open(package)?, &mut file)?;
        file.set_permissions(fs::metadata(&self.program)?.permissions())?;
        file.sync_all()
    }

    /// 把新程序换上去，原来的留作备份。失败时程序还是原来的那个。
    pub fn install_candidate(&self) -> io::Result<()> {
        let (candidate, backup) = (self.candidate(), self.backup());
        remove_if_exists(&backup)?;
        match self.swap {
            Swap::Overwrite => {
                // 硬链接不用拷贝，也不占地方；文件系统不支持时才拷一份。
                if fs::hard_link(&self.program, &backup).is_err() {
                    fs::copy(&self.program, &backup)?;
                }
                fs::rename(&candidate, &self.program).inspect_err(|_| discard(&backup))?;
            }
            Swap::MoveAside => {
                fs::rename(&self.program, &backup)?;
                fs::rename(&candidate, &self.program).inspect_err(|_| put_back(&backup, &self.program))?;
            }
        }
        self.sync_dir()
    }

    pub fn has_backup(&self) -> bool {
        self.backup().exists()
    }

    pub fn remove_backup(&self) -> io::Result<()> {
        remove_if_exists(&self.backup())
    }

    /// 把备份换回来。调用的是正在运行的新程序，它自己的文件被盖掉或者挪开。失败时程序还是新的那个。
    pub fn restore_backup(&self) -> io::Result<()> {
        let backup = self.backup();
        match self.swap {
            Swap::Overwrite => fs::rename(&backup, &self.program)?,
            Swap::MoveAside => {
                let discarded = self.discarded();
                remove_if_exists(&discarded)?;
                fs::rename(&self.program, &discarded)?;
                fs::rename(&backup, &self.program).inspect_err(|_| put_back(&discarded, &self.program))?;
            }
        }
        self.sync_dir()
    }

    /// 清掉不该再留着的文件：没换上去的新程序，和上次换回时挪开的那个（那时它正在运行，删不掉）。
    /// 清不掉不要紧，下次再清，所以只记日志。
    pub fn remove_leftovers(&self) {
        for leftover in [self.candidate(), self.discarded()] {
            if let Err(error) = remove_if_exists(&leftover) {
                tracing::warn!(path = %leftover.display(), %error, "清不掉上次更新留下的文件");
            }
        }
    }

    /// 让改名落盘。Windows 上目录不能这样打开，那里改名的落盘由文件系统的日志保证。
    fn sync_dir(&self) -> io::Result<()> {
        if cfg!(unix) {
            let dir = self.program.parent().ok_or_else(|| io::Error::other("程序的路径没有上一级目录"))?;
            File::open(dir)?.sync_all()?;
        }
        Ok(())
    }
}

fn remove_if_exists(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

/// 替换失败之后收拾刚做的备份。收拾不掉也不影响程序，下次启动会再清。
fn discard(backup: &Path) {
    if let Err(error) = remove_if_exists(backup) {
        tracing::warn!(path = %backup.display(), %error, "替换程序失败之后删不掉刚做的备份");
    }
}

/// 第二次改名失败之后把刚挪开的文件放回去。放不回去的话程序的路径上就没有文件了，这只能交给人处理，所以要把话说清楚。
fn put_back(moved: &Path, program: &Path) {
    if let Err(error) = fs::rename(moved, program) {
        tracing::error!(
            from = %moved.display(), to = %program.display(), %error,
            "替换程序中途失败，而且没能把原来的程序放回去：请手动把它改回原来的名字，否则节点下次启动不了"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        dir: tempfile::TempDir,
        files: ProgramFiles,
    }

    impl Fixture {
        fn new(swap: Swap) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let program = dir.path().join("i3t");
            fs::write(&program, "旧程序").unwrap();
            Self { files: ProgramFiles::new(program, swap), dir }
        }

        fn place(&self, content: &str) {
            let package = self.dir.path().join("package");
            fs::write(&package, content).unwrap();
            self.files.place_candidate(&package).unwrap();
            fs::remove_file(package).unwrap();
        }

        fn names(&self) -> Vec<String> {
            let mut names: Vec<String> =
                fs::read_dir(self.dir.path()).unwrap().map(|entry| entry.unwrap().file_name().into_string().unwrap()).collect();
            names.sort();
            names
        }

        fn read(&self, name: &str) -> String {
            fs::read_to_string(self.dir.path().join(name)).unwrap()
        }
    }

    #[test]
    fn tagged_files_sit_next_to_the_program_and_keep_the_exe_extension() {
        let unix = ProgramFiles::new(PathBuf::from("/opt/i3t/bin/i3t"), Swap::Overwrite);
        assert_eq!(unix.candidate(), Path::new("/opt/i3t/bin/i3t.new"));
        assert_eq!(unix.backup(), Path::new("/opt/i3t/bin/i3t.old"));
        let windows = ProgramFiles::new(PathBuf::from("bin/i3t.EXE"), Swap::MoveAside);
        assert_eq!(windows.candidate(), Path::new("bin/i3t.new.exe"));
        assert_eq!(windows.discarded(), Path::new("bin/i3t.failed.exe"));
        // 名字里别的点不算扩展名。
        let dotted = ProgramFiles::new(PathBuf::from("i3t-0.2.0"), Swap::Overwrite);
        assert_eq!(dotted.backup(), Path::new("i3t-0.2.0.old"));
    }

    #[test]
    fn install_swaps_the_program_and_keeps_the_old_one_with_either_strategy() {
        for swap in [Swap::Overwrite, Swap::MoveAside] {
            let fixture = Fixture::new(swap);
            fixture.place("新程序");
            assert_eq!(fixture.names(), ["i3t", "i3t.new"]);
            assert!(!fixture.files.has_backup());

            fixture.files.install_candidate().unwrap();
            assert_eq!(fixture.names(), ["i3t", "i3t.old"], "{swap:?}");
            assert_eq!(fixture.read("i3t"), "新程序");
            assert_eq!(fixture.read("i3t.old"), "旧程序");
            assert!(fixture.files.has_backup());

            fixture.files.remove_backup().unwrap();
            assert_eq!(fixture.names(), ["i3t"]);
        }
    }

    #[test]
    fn restore_brings_the_old_program_back() {
        let fixture = Fixture::new(Swap::Overwrite);
        fixture.place("新程序");
        fixture.files.install_candidate().unwrap();
        fixture.files.restore_backup().unwrap();
        assert_eq!(fixture.names(), ["i3t"]);
        assert_eq!(fixture.read("i3t"), "旧程序");
    }

    #[test]
    fn restore_by_moving_aside_leaves_the_new_program_to_be_cleaned_up_later() {
        let fixture = Fixture::new(Swap::MoveAside);
        fixture.place("新程序");
        fixture.files.install_candidate().unwrap();
        fixture.files.restore_backup().unwrap();
        assert_eq!(fixture.names(), ["i3t", "i3t.failed"]);
        assert_eq!(fixture.read("i3t"), "旧程序");
        assert_eq!(fixture.read("i3t.failed"), "新程序");
        fixture.files.remove_leftovers();
        assert_eq!(fixture.names(), ["i3t"]);
    }

    #[test]
    fn a_failed_install_leaves_the_program_as_it_was() {
        for swap in [Swap::Overwrite, Swap::MoveAside] {
            let fixture = Fixture::new(swap);
            // 没有放新程序就去换：第二步改名会失败。
            assert!(fixture.files.install_candidate().is_err());
            assert_eq!(fixture.names(), ["i3t"], "{swap:?}");
            assert_eq!(fixture.read("i3t"), "旧程序");
        }
    }

    #[test]
    fn a_failed_restore_leaves_the_program_as_it_was() {
        for swap in [Swap::Overwrite, Swap::MoveAside] {
            let fixture = Fixture::new(swap);
            assert!(fixture.files.restore_backup().is_err());
            assert_eq!(fixture.names(), ["i3t"], "{swap:?}");
            assert_eq!(fixture.read("i3t"), "旧程序");
        }
    }

    #[test]
    fn a_stale_backup_or_candidate_does_not_get_in_the_way() {
        let fixture = Fixture::new(Swap::NATIVE);
        fs::write(fixture.files.backup(), "更早的备份").unwrap();
        fs::write(fixture.files.candidate(), "上次没换上去的").unwrap();
        fixture.place("新程序");
        fixture.files.install_candidate().unwrap();
        assert_eq!(fixture.read("i3t"), "新程序");
        assert_eq!(fixture.read("i3t.old"), "旧程序");
    }

    #[cfg(unix)]
    #[test]
    fn candidate_gets_the_permissions_of_the_running_program() {
        use std::os::unix::fs::PermissionsExt;
        let fixture = Fixture::new(Swap::Overwrite);
        fs::set_permissions(fixture.files.program(), fs::Permissions::from_mode(0o750)).unwrap();
        fixture.place("新程序");
        let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o7777;
        assert_eq!(mode(&fixture.files.candidate()), 0o750);
        fixture.files.install_candidate().unwrap();
        assert_eq!(mode(fixture.files.program()), 0o750);
    }
}
