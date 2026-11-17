//! 备份：把恢复一个 controller 要的东西写进一个新目录，那个目录本身就是一个数据目录。恢复就是在它上面启动 controller。
//!
//! 数据目录里的东西逐个看过：
//!
//! | 文件 | 备份里 | 为什么 |
//! |---|---|---|
//! | `controller.db` 和旁边的 `-wal`、`-shm` | 一份一致的快照，一个文件 | 全部的配置和记录，见 [`Store::snapshot`]。快照是完整的，不带 `-wal` |
//! | `controller.key` | 原样照抄，只有属主可读 | 节点认的是这把密钥：换了它，已经加入的节点都不再连这个 controller |
//! | `releases/` | 不要 | 更新包的缓存。恢复之后要用时重新取，取回来照样按清单核对 |
//!
//! 管理令牌只存了哈希，在数据库里：恢复之后能用的是备份那一刻的令牌。

use std::fs::{self, File};
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};

use i3t_crypto::NodeKey;

use super::{Controller, DATABASE_FILE, KEY_FILE, create_private_file, write_synced};
use crate::error::Error;
use crate::store::Store;

/// [`Controller::back_up`] 写下的东西。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Backup {
    /// 备份所在的目录，绝对路径。
    pub dir: PathBuf,
    /// 写进去的文件，按写的顺序。
    pub files: Vec<BackupFile>,
}

/// 备份里的一个文件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupFile {
    /// 文件名，在 [`Backup::dir`] 里。
    pub name: &'static str,
    pub bytes: u64,
}

impl Controller {
    /// 把数据目录 `data_dir` 备份到 `to`。controller 正开着这个数据目录时（在别的进程里或者就在这个进程里）也可以备份，
    /// 它照常工作。
    ///
    /// `to` 不存在时建出来；已经存在的话必须是空目录，否则什么都不动，返回 [`Error::BackupTargetNotEmpty`]。
    /// 写到一半失败时删掉这里写下的文件和建的目录。密钥最后写，所以进程中途被杀掉时留下的目录里没有密钥：
    /// 在它上面启动会生成一把新的密钥，节点不认它，不受影响。反过来，有原来的密钥、数据库却不全的 controller
    /// 会把节点当成不认识的拒掉，节点被拒之后不再重连。
    ///
    /// # Errors
    ///
    /// `data_dir` 里没有密钥或者数据库（[`Error::NotDataDir`]），密钥读不懂，`to` 不是空的，或者读写失败。
    pub fn back_up(data_dir: &Path, to: &Path) -> Result<Backup, Error> {
        let key = required(data_dir, KEY_FILE)?;
        let database = required(data_dir, DATABASE_FILE)?;
        let pem = fs::read_to_string(&key).map_err(io_error(&key))?;
        // 读不懂的密钥备份了也没用，现在就说。
        NodeKey::from_pem(&pem).map_err(|source| Error::Key { path: key.clone(), source })?;

        let dir = std::path::absolute(to).map_err(io_error(to))?;
        let mut written = Written::claim(&dir)?;
        match written.fill(&database, &pem) {
            Ok(files) => {
                tracing::info!(from = %data_dir.display(), to = %dir.display(), "备份写好了");
                Ok(Backup { dir, files })
            }
            Err(error) => {
                written.discard();
                Err(error)
            }
        }
    }
}

/// 这次备份建出来的东西。备份没有做完时由它删掉；别的东西它不碰。
struct Written {
    dir: PathBuf,
    /// 目录是这次建的。
    created_dir: bool,
    files: Vec<PathBuf>,
}

impl Written {
    /// 占下备份的目录：不存在就建出来，已经存在的必须是空目录。
    fn claim(dir: &Path) -> Result<Self, Error> {
        let created_dir = match fs::metadata(dir) {
            Ok(metadata) => {
                if !metadata.is_dir() || fs::read_dir(dir).map_err(io_error(dir))?.next().is_some() {
                    return Err(Error::BackupTargetNotEmpty(dir.to_owned()));
                }
                false
            }
            Err(err) if err.kind() == ErrorKind::NotFound => {
                fs::create_dir_all(dir).map_err(io_error(dir))?;
                true
            }
            Err(source) => return Err(Error::Io { path: dir.to_owned(), source }),
        };
        Ok(Self { dir: dir.to_owned(), created_dir, files: Vec::new() })
    }

    fn fill(&mut self, database: &Path, pem: &str) -> Result<Vec<BackupFile>, Error> {
        // 先自己建出这个空文件（`VACUUM INTO` 也肯往空文件里写），它才确定是这次建的，失败时可以放心删掉。
        let (snapshot, file) = self.create(DATABASE_FILE, create_private_file)?;
        Store::snapshot(database, &snapshot)?;
        file.sync_all().map_err(io_error(&snapshot))?;

        let (key, file) = self.create(KEY_FILE, create_private_file)?;
        write_synced(file, pem.as_bytes()).map_err(io_error(&key))?;

        // 目录项也要落盘，不然断电之后文件可能不在目录里。Windows 上不能这样打开目录，也不需要。
        #[cfg(unix)]
        File::open(&self.dir).and_then(|dir| dir.sync_all()).map_err(io_error(&self.dir))?;

        [(DATABASE_FILE, &snapshot), (KEY_FILE, &key)]
            .into_iter()
            .map(|(name, path)| Ok(BackupFile { name, bytes: fs::metadata(path).map_err(io_error(path))?.len() }))
            .collect()
    }

    /// 在备份的目录里新建一个文件，记下它是这次建的。
    fn create(&mut self, name: &str, open: impl FnOnce(&Path) -> io::Result<File>) -> Result<(PathBuf, File), Error> {
        let path = self.dir.join(name);
        let file = open(&path).map_err(io_error(&path))?;
        self.files.push(path.clone());
        Ok((path, file))
    }

    fn discard(self) {
        for path in self.files.iter().rev() {
            if let Err(error) = fs::remove_file(path) {
                tracing::warn!(path = %path.display(), "备份没有做完，写了一半的文件删不掉：{error}");
            }
        }
        if self.created_dir
            && let Err(error) = fs::remove_dir(&self.dir)
        {
            tracing::warn!(path = %self.dir.display(), "备份没有做完，建的目录删不掉：{error}");
        }
    }
}

/// 数据目录里一定要有的文件。不在时说这不是一个数据目录。
fn required(dir: &Path, name: &'static str) -> Result<PathBuf, Error> {
    let path = dir.join(name);
    match fs::metadata(&path) {
        Ok(metadata) if metadata.is_file() => Ok(path),
        Err(source) if source.kind() != ErrorKind::NotFound => Err(Error::Io { path, source }),
        Ok(_) | Err(_) => Err(Error::NotDataDir { dir: dir.to_owned(), missing: name }),
    }
}

fn io_error(path: &Path) -> impl FnOnce(io::Error) -> Error {
    let path = path.to_owned();
    move |source| Error::Io { path, source }
}
