//! 更新的持久状态：哪个版本换上去了还没确认，哪些版本在这台机器上失败过。
//!
//! 存在状态目录的 `update/state.json` 里。写它的和读它的常常不是同一个版本的程序：记下"待确认"的是旧程序，
//! 启动时读它的是新程序；换回之后读失败记录的又是旧程序。所以字段只增不改，读的时候忽略不认识的。

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use i3t_model::update::{UpdateCause, Version};
use serde::{Deserialize, Serialize};

use crate::error::UpdateError;

const STATE_FILE: &str = "state.json";

/// 失败记录留多少条。一个版本只占一条，新版本装成之后旧的记录会被清掉，所以正常情况下只有零到一条；
/// 上限只是防止文件在意料之外的情况下一直变长。
const MAX_FAILURES: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub(crate) struct State {
    #[serde(default)]
    pub pending: Option<Pending>,
    #[serde(default)]
    pub failed: Vec<FailedVersion>,
}

/// 一个已经换上去、还没有被确认的版本。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Pending {
    pub version: Version,
    /// 被换下来、留作备份的那个版本。
    pub previous: Version,
    /// 新程序已经启动过几次。
    pub starts: u32,
    /// 什么时候换上去的，Unix 秒。
    pub since: u64,
}

impl Pending {
    /// 换上去之后过了多久。系统时钟被往回拨过的话算作刚换上去。
    pub fn age(&self, now: u64) -> Duration {
        Duration::from_secs(now.saturating_sub(self.since))
    }
}

/// 一个在这台机器上失败过的版本：自检没有通过，或者装上之后没能连上 controller、被换回了。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailedVersion {
    pub version: Version,
    pub reason: String,
    /// 什么时候失败的，Unix 秒。
    pub at: u64,
    /// `reason` 的代码。这个字段出现之前记下的失败没有。
    #[serde(default)]
    pub cause: Option<UpdateCause>,
}

impl State {
    pub fn failure_of(&self, version: &Version) -> Option<&FailedVersion> {
        self.failed.iter().find(|failure| &failure.version == version)
    }

    /// 记下一次失败。同一个版本只留最近的一次。
    pub fn record_failure(&mut self, version: Version, reason: String, cause: UpdateCause) {
        self.failed.retain(|failure| failure.version != version);
        self.failed.push(FailedVersion { version, reason, at: now(), cause: Some(cause) });
        let excess = self.failed.len().saturating_sub(MAX_FAILURES);
        self.failed.drain(..excess);
    }

    /// `version` 装成了：它和比它旧的版本的失败记录不再有意义。
    pub fn forget_failures_up_to(&mut self, version: &Version) {
        self.failed.retain(|failure| &failure.version > version);
    }
}

/// 现在的时间，Unix 秒。
pub(crate) fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since_epoch| since_epoch.as_secs())
}

#[derive(Debug, Clone)]
pub(crate) struct StateFile {
    path: PathBuf,
}

impl StateFile {
    pub fn new(update_dir: &Path) -> Self {
        Self { path: update_dir.join(STATE_FILE) }
    }

    /// 文件不存在就是"什么都没有发生过"。文件坏了是错误，不能当成什么都没有：那样会丢掉待确认的记录，备份就再也不会被换回了。
    pub fn load(&self) -> Result<State, UpdateError> {
        match fs::read(&self.path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|source| UpdateError::State { path: self.path.clone(), source }),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(State::default()),
            Err(error) => Err(UpdateError::io("读", &self.path)(error)),
        }
    }

    /// 先写临时文件再改名，并且落盘之后才返回：调用方接下来要换程序文件，这条记录得先稳稳地在盘上。
    pub fn save(&self, state: &State) -> Result<(), UpdateError> {
        let bytes = serde_json::to_vec_pretty(state).expect("状态里只有字符串和整数，序列化不会失败");
        let temporary = self.path.with_extension("json.tmp");
        let write = || -> io::Result<()> {
            let mut file = File::create(&temporary)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            fs::rename(&temporary, &self.path)?;
            // 目录在 Windows 上不能这样打开；那里改名的落盘由文件系统的日志保证。
            if cfg!(unix)
                && let Some(dir) = self.path.parent()
            {
                File::open(dir)?.sync_all()?;
            }
            Ok(())
        };
        write().map_err(UpdateError::io("写", &self.path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(text: &str) -> Version {
        text.parse().unwrap()
    }

    #[test]
    fn state_survives_a_round_trip_and_leaves_no_temporary_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = StateFile::new(dir.path());
        assert_eq!(file.load().unwrap(), State::default());

        let mut state = State {
            pending: Some(Pending { version: version("0.2.0"), previous: version("0.1.0"), starts: 2, since: 1_763_200_000 }),
            failed: Vec::new(),
        };
        state.record_failure(version("0.1.5"), "自检没有通过".to_owned(), UpdateCause::Cancelled);
        file.save(&state).unwrap();
        assert_eq!(file.load().unwrap(), state);
        let names: Vec<_> = fs::read_dir(dir.path()).unwrap().map(|entry| entry.unwrap().file_name()).collect();
        assert_eq!(names, [STATE_FILE]);
    }

    #[test]
    fn state_written_by_another_version_of_the_program_is_readable() {
        let dir = tempfile::tempdir().unwrap();
        let file = StateFile::new(dir.path());
        // 没有的字段取默认值，多出来的字段忽略。
        fs::write(dir.path().join(STATE_FILE), r#"{ "pending": null, "added_later": [1, 2] }"#).unwrap();
        assert_eq!(file.load().unwrap(), State::default());
        fs::write(dir.path().join(STATE_FILE), r#"{ "failed": [{ "version": "0.2.0", "reason": "x", "at": 5, "added_later": true }] }"#)
            .unwrap();
        assert_eq!(file.load().unwrap().failure_of(&version("0.2.0")).unwrap().reason, "x");
    }

    #[test]
    fn a_corrupt_state_file_is_an_error_not_a_clean_slate() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(STATE_FILE), "{ 写了一半").unwrap();
        assert!(matches!(StateFile::new(dir.path()).load(), Err(UpdateError::State { .. })));
    }

    #[test]
    fn each_version_has_one_failure_record_and_old_ones_are_dropped_on_success() {
        let mut state = State::default();
        state.record_failure(version("0.2.0"), "第一次".to_owned(), UpdateCause::Cancelled);
        state.record_failure(version("0.3.0"), "另一个版本".to_owned(), UpdateCause::Cancelled);
        state.record_failure(version("0.2.0"), "第二次".to_owned(), UpdateCause::Cancelled);
        assert_eq!(state.failed.len(), 2);
        assert_eq!(state.failure_of(&version("0.2.0")).unwrap().reason, "第二次");
        assert_eq!(state.failure_of(&version("0.2.1")), None);

        state.forget_failures_up_to(&version("0.2.5"));
        assert_eq!(state.failure_of(&version("0.2.0")), None);
        assert!(state.failure_of(&version("0.3.0")).is_some());
    }

    #[test]
    fn failure_records_are_bounded() {
        let mut state = State::default();
        for patch in 0..40 {
            state.record_failure(Version::new(0, 1, patch), String::new(), UpdateCause::Cancelled);
        }
        assert_eq!(state.failed.len(), MAX_FAILURES);
        assert_eq!(state.failed.last().unwrap().version, Version::new(0, 1, 39));
    }

    #[test]
    fn age_never_goes_negative_when_the_clock_steps_back() {
        let pending = Pending { version: version("0.2.0"), previous: version("0.1.0"), starts: 0, since: 1000 };
        assert_eq!(pending.age(1090), Duration::from_secs(90));
        assert_eq!(pending.age(900), Duration::ZERO);
    }
}
