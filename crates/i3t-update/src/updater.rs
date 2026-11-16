//! 把验签、取包、自检、替换、确认与回退连成一次更新。

use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex as StdMutex, MutexGuard};
use std::time::Duration;

use i3t_model::update::{
    Artifact, FailureKind, Manifest, Platform, SelfUpdateBlocker, SignedManifest, UpdateStatus, UpdateTiming, Version,
};
use tokio::sync::{Mutex, watch};

use crate::digest::FileDigest;
use crate::download::Download;
use crate::error::{Refusal, UpdateError, describe};
use crate::program::{ProgramFiles, Swap};
use crate::self_check;
use crate::source::PackageSource;
use crate::state::{self, FailedVersion, Pending, State, StateFile};
use crate::timing;
use crate::trust::TrustedKeys;

/// 新程序换上去之后最多启动几次；到了这个数还没有连上 controller，再启动时就换回旧程序。
pub const MAX_UNCONFIRMED_STARTS: u32 = 3;

/// 新程序换上去之后最多等多久；到时候还没有连上 controller 就换回旧程序。
pub const CONFIRMATION_WINDOW: Duration = Duration::from_secs(5 * 60);

/// 程序文件换过之后，进程用这个退出码退出，由外面把它重新拉起来。
///
/// 它是 sysexits 里的 `EX_TEMPFAIL`（"暂时失败，请重试"）。随项目发的 systemd 单元是 `Restart=always`，
/// 什么退出码都会重启；单独定一个值，是为了让日志和别的看护程序分得清"请重启我"和"出错了"。
pub const RESTART_EXIT_CODE: u8 = 75;

/// 状态目录里归更新用的子目录。
const UPDATE_DIR: &str = "update";

/// 取下来的包在更新目录里的名字是 `staged-<SHA-256>.bin`。名字里带哈希，所以接着取的一定是同一个包。
const STAGED_PREFIX: &str = "staged-";

/// 建一个 [`Updater`] 要知道的东西。
#[derive(Debug, Clone)]
pub struct UpdaterConfig {
    /// 正在运行的程序文件，[`current_program`](crate::current_program) 的结果。
    pub program: PathBuf,
    /// 节点的状态目录。更新只用它下面的 `update/`。
    pub state_dir: PathBuf,
    /// 正在运行的版本。
    pub running: Version,
    /// 运行新程序做自检时给它的参数，一般是 `["--state-dir", <状态目录>, "self-check"]`。约定见 [`mod@self_check`]。
    pub self_check_args: Vec<OsString>,
    /// 这个节点不能自己更新的原因，[`self_update_blocker`](crate::self_update_blocker) 的结果。不是 `None` 时所有清单都被拒绝。
    pub blocker: Option<SelfUpdateBlocker>,
}

/// 一份通过了检查、可以去取包的清单里，和本节点有关的部分。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offer {
    pub version: Version,
    /// 本平台的节点程序。
    pub artifact: Artifact,
}

/// 一个取完、验过、自检也通过了的新程序，放在程序旁边等着换上去。
#[derive(Debug)]
#[must_use = "新程序只是准备好了，交给 Updater::install 才会换上去"]
pub struct Prepared {
    version: Version,
    artifact: Artifact,
}

impl Prepared {
    #[must_use]
    pub const fn version(&self) -> &Version {
        &self.version
    }
}

/// 程序文件已经换过了，调用方现在要重启进程：以 [`RESTART_EXIT_CODE`] 退出，或者自己重新执行程序。
#[derive(Debug, Clone, PartialEq, Eq)]
#[must_use = "程序文件已经换过了，不重启的话跑着的还是原来的程序"]
pub struct RestartRequired {
    /// 重启之后运行的版本。
    pub version: Version,
}

/// 一个没有被确认的版本被换回去了。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RolledBack {
    /// 被换下去的那个版本。它已经被记为在这台机器上失败过。
    pub failed: Version,
    pub reason: String,
    pub restart: RestartRequired,
}

/// 启动时的情况，[`Updater::startup`] 的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Startup {
    /// 没有待确认的更新，照常启动。
    Clean,
    /// 这个程序是刚换上来的，还没有被确认。照常启动；连上 controller 时调用 [`Updater::confirm`]，
    /// 同时跑着 [`Updater::enforce_confirmation_deadline`]。
    AwaitingConfirmation { version: Version },
    /// 这个程序是刚换上来的，但一直没能连上 controller，已经换回旧程序。不要继续启动，按 [`RolledBack::restart`] 重启。
    RolledBack(RolledBack),
}

/// 节点上的更新。一个进程里建一个，从启动用到退出。
///
/// 一次更新分三步，可以分开调用，也可以用 [`Updater::update`] 一口气做完：
///
/// 1. [`prepare`](Self::prepare)：验签、取包、核对、自检。不动正在运行的程序，随时可以丢弃。
/// 2. [`wait_for_slot`](Self::wait_for_slot)：等 controller 指定的时机。
/// 3. [`install`](Self::install)：换上新程序，旧的留作备份，然后调用方重启。
///
/// 重启之后的新进程先调用 [`startup`](Self::startup)，连上 controller 时调用 [`confirm`](Self::confirm)。
///
/// 清单只用编译在程序里的公钥验（[`TrustedKeys::compiled_in`]）。这里没有、也不应该有换公钥的入口。
#[derive(Debug)]
pub struct Updater {
    files: ProgramFiles,
    update_dir: PathBuf,
    state: StateFile,
    running: Version,
    platform: Platform,
    keys: TrustedKeys,
    self_check_args: Vec<OsString>,
    blocker: Option<SelfUpdateBlocker>,
    status: watch::Sender<UpdateStatus>,
    /// 同一时刻只准备一个新程序：暂存文件和程序旁边的新程序都只有一份。
    preparing: Mutex<()>,
    /// "读状态、换文件、写状态"的每一段都要一口气做完。确认和到期换回可能在两个线程上同时发生，
    /// 交错起来会出现程序已经换回、记录却写着已确认的情况。
    transition: StdMutex<()>,
}

impl Updater {
    #[must_use]
    pub fn new(config: UpdaterConfig) -> Self {
        Self::assemble(config, TrustedKeys::compiled_in(), Swap::NATIVE)
    }

    /// 测试要用自己的公钥和两种替换方式，所以装配留了这个只在本 crate 里看得见的口子。
    fn assemble(config: UpdaterConfig, keys: TrustedKeys, swap: Swap) -> Self {
        let update_dir = config.state_dir.join(UPDATE_DIR);
        Self {
            files: ProgramFiles::new(config.program, swap),
            state: StateFile::new(&update_dir),
            update_dir,
            running: config.running,
            platform: Platform::current(),
            keys,
            self_check_args: config.self_check_args,
            blocker: config.blocker,
            status: watch::Sender::new(UpdateStatus::Idle),
            preparing: Mutex::new(()),
            transition: StdMutex::new(()),
        }
    }

    fn transition(&self) -> MutexGuard<'_, ()> {
        self.transition.lock().expect("transition mutex poisoned")
    }

    /// 更新进行到哪一步了。调用方把每次变化转给 controller。
    ///
    /// 看到 [`UpdateStatus::Draining`] 时调用方要停止接新的流，直到进程重启。
    #[must_use]
    pub fn status(&self) -> watch::Receiver<UpdateStatus> {
        self.status.subscribe()
    }

    /// 在这台机器上失败过、因而不会自动重试的版本。
    ///
    /// # Errors
    ///
    /// 状态文件读不了。
    pub fn failures(&self) -> Result<Vec<FailedVersion>, UpdateError> {
        Ok(self.state.load()?.failed)
    }

    /// 进程启动时调用一次，要在连 controller 之前。
    ///
    /// 有待确认的更新时给启动次数加一；已经启动过 [`MAX_UNCONFIRMED_STARTS`] 次，或者换上去已经超过
    /// [`CONFIRMATION_WINDOW`]，就换回旧程序，把这个版本记为失败。
    ///
    /// 做不到的事：新程序要是在走到这里之前就崩了，没有人替它换回去。这一种情况靠换之前的自检挡住。
    ///
    /// # Errors
    ///
    /// 状态文件读不了或写不了，或者要换回的时候换不了。
    pub fn startup(&self) -> Result<Startup, UpdateError> {
        let _transition = self.transition();
        let mut state = self.state.load()?;
        let Some(pending) = state.pending.clone() else {
            self.tidy();
            self.announce_standing_failure(&state);
            return Ok(Startup::Clean);
        };
        if pending.version != self.running {
            // 记下了要换成它，可跑着的不是它：换到一半断了电，或者有人手动换了程序。
            tracing::warn!(recorded = %pending.version, running = %self.running, "待确认的记录和正在运行的版本对不上，作废这条记录");
            state.pending = None;
            self.state.save(&state)?;
            self.tidy();
            return Ok(Startup::Clean);
        }

        let give_up = if pending.starts >= MAX_UNCONFIRMED_STARTS {
            Some(format!("新程序启动了 {MAX_UNCONFIRMED_STARTS} 次都没有连上 controller"))
        } else if pending.age(state::now()) >= CONFIRMATION_WINDOW {
            Some(overdue_reason())
        } else {
            None
        };
        if let Some(reason) = give_up {
            return Ok(self.roll_back(state, &pending, reason)?.map_or(Startup::Clean, Startup::RolledBack));
        }

        let starts = pending.starts + 1;
        state.pending = Some(Pending { starts, ..pending });
        self.state.save(&state)?;
        tracing::info!(version = %self.running, starts, "这是刚换上来的新程序，连上 controller 才算更新成功");
        Ok(Startup::AwaitingConfirmation { version: self.running.clone() })
    }

    /// 新程序连上了 controller：更新成了，删掉备份。返回被确认的版本；没有待确认的更新时什么都不做，返回 `None`。
    ///
    /// 每次连上 controller 都可以调用。
    ///
    /// # Errors
    ///
    /// 状态文件读不了或写不了。
    pub fn confirm(&self) -> Result<Option<Version>, UpdateError> {
        let _transition = self.transition();
        let mut state = self.state.load()?;
        if state.pending.as_ref().is_none_or(|pending| pending.version != self.running) {
            return Ok(None);
        }
        state.pending = None;
        state.forget_failures_up_to(&self.running);
        self.state.save(&state)?;
        // 记录已经落盘，从这里起备份只是占地方：删不掉的话下次启动再清。
        self.tidy();
        tracing::info!(version = %self.running, "新程序连上了 controller，更新完成");
        self.status.send_replace(UpdateStatus::Confirmed { version: self.running.clone() });
        Ok(Some(self.running.clone()))
    }

    /// 守着确认的期限：换上去满 [`CONFIRMATION_WINDOW`] 还没有 [`confirm`](Self::confirm)，就换回旧程序。
    ///
    /// [`startup`](Self::startup) 返回 [`Startup::AwaitingConfirmation`] 之后，调用方让它在后台跑着：
    /// 返回 `Some` 就按里面的 [`RolledBack::restart`] 重启；返回 `None` 是已经确认了，或者本来就没有待确认的更新。
    ///
    /// 期限按系统时钟从换上去的那一刻算起，所以跨重启也有效。
    ///
    /// # Errors
    ///
    /// 状态文件读不了或写不了，或者换不回去。
    pub async fn enforce_confirmation_deadline(&self) -> Result<Option<RolledBack>, UpdateError> {
        let Some(pending) = self.unconfirmed(&self.state.load()?) else { return Ok(None) };
        tokio::time::sleep(CONFIRMATION_WINDOW.saturating_sub(pending.age(state::now()))).await;
        // 等的这段时间里可能已经确认了。醒来之后不再看系统时钟：等够了就是等够了。
        let _transition = self.transition();
        let state = self.state.load()?;
        match self.unconfirmed(&state) {
            Some(pending) => self.roll_back(state, &pending, overdue_reason()),
            None => Ok(None),
        }
    }

    /// 验签，再看这份清单该不该装。不取包，不动磁盘上的任何东西。被拒绝的原因会出现在状态里
    /// （"不比现在的新"除外：那不是出了什么错）。
    ///
    /// 规矩（白皮书 §10.5、§11.2）：
    ///
    /// - 签名必须是编译在程序里的某一把公钥签的；
    /// - 只装比正在运行的版本新的，一样的和更旧的都不装；
    /// - 预发布版只装在正在运行预发布版的节点上。跑正式版的节点不会因为有人把一份测试用的清单发给它，就换成预发布版；
    /// - 在这台机器上失败过的版本，自动更新（[`UpdateTiming::Idle`]）不再试，管理员强制的更新可以再试；
    /// - 清单里要有本平台的包。
    ///
    /// # Errors
    ///
    /// [`UpdateError::Refused`]，里面是不装的原因；或者状态文件读不了。
    pub fn check(&self, signed: &SignedManifest, timing: UpdateTiming) -> Result<Offer, UpdateError> {
        let manifest = self.keys.verify(signed).map_err(|error| self.refuse(None, error.into()))?;
        let state = self.state.load()?;
        self.judge(&manifest, &state, timing).map_err(|refusal| self.refuse(Some(&manifest.version), refusal))
    }

    fn judge(&self, manifest: &Manifest, state: &State, timing: UpdateTiming) -> Result<Offer, Refusal> {
        let offered = &manifest.version;
        if let Some(installed) = self.awaiting_restart(state) {
            return Err(Refusal::RestartPending { installed });
        }
        if *offered <= self.running {
            return Err(Refusal::NotNewer { offered: offered.clone(), running: self.running.clone() });
        }
        if let Some(blocker) = self.blocker {
            return Err(Refusal::CannotSelfUpdate(blocker));
        }
        if manifest.is_prerelease() && !self.running.is_prerelease() {
            return Err(Refusal::Prerelease { offered: offered.clone(), running: self.running.clone() });
        }
        if let Some(failure) = state.failure_of(offered).filter(|_| !timing.is_forced()) {
            return Err(Refusal::FailedBefore { version: offered.clone(), reason: failure.reason.clone() });
        }
        let artifact = manifest
            .artifact(&self.platform)
            .ok_or_else(|| Refusal::NoArtifact { version: offered.clone(), platform: self.platform.clone() })?;
        Ok(Offer { version: offered.clone(), artifact: artifact.clone() })
    }

    /// 第一步：验签、取包、核对大小和 SHA-256、把新程序放到程序旁边并运行它的自检。
    ///
    /// 正在运行的程序不受影响。链路中断会一直重试，不算失败；这个 future 可以随时丢弃，取了一半的包留在状态目录里，
    /// 下次（包括进程重启之后）接着取。同一时刻只有一个在做，后来的等前一个结束或者被丢弃。
    ///
    /// 自检不通过时不替换，把这个版本记为在这台机器上失败过。
    ///
    /// # Errors
    ///
    /// 清单被拒绝、包取不到或者对不上、自检不通过、磁盘出错。除了"不比现在的新"之外，原因都会出现在状态里。
    pub async fn prepare<S: PackageSource>(
        &self,
        signed: &SignedManifest,
        timing: UpdateTiming,
        source: &S,
    ) -> Result<Prepared, UpdateError> {
        let _preparing = self.preparing.lock().await;
        let offer = self.check(signed, timing)?;
        tracing::info!(version = %offer.version, running = %self.running, ?timing, size = offer.artifact.size, "开始准备新版本");
        self.stage(&offer, source).await.inspect_err(|error| self.fail(Some(&offer.version), error))?;
        Ok(Prepared { version: offer.version, artifact: offer.artifact })
    }

    async fn stage<S: PackageSource>(&self, offer: &Offer, source: &S) -> Result<(), UpdateError> {
        let Offer { version, artifact } = offer;
        fs::create_dir_all(&self.update_dir).map_err(UpdateError::io("创建", &self.update_dir))?;
        let package = self.update_dir.join(format!("{STAGED_PREFIX}{}.bin", artifact.sha256));
        self.remove_staged_packages(Some(&package));
        let download = Download { source, version, platform: &self.platform, artifact, path: &package };
        download
            .run(|status| {
                self.status.send_replace(status);
            })
            .await?;

        self.status.send_replace(UpdateStatus::SelfCheck { version: version.clone() });
        let candidate = self.files.candidate();
        let (files, from) = (self.files.clone(), package.clone());
        tokio::task::spawn_blocking(move || files.place_candidate(&from))
            .await
            .expect("拷贝文件的线程不会 panic：里面只有文件操作")
            .map_err(UpdateError::io("把新程序放到", &candidate))?;
        if let Err(reason) = self_check::run_candidate(&candidate, &self.self_check_args, version, self_check::TIMEOUT).await {
            discard(&candidate);
            discard(&package);
            let _transition = self.transition();
            let mut state = self.state.load()?;
            state.record_failure(version.clone(), format!("自检没有通过：{reason}"));
            self.state.save(&state)?;
            return Err(UpdateError::SelfCheck { version: version.clone(), reason });
        }
        tracing::info!(%version, "新程序的自检通过了");
        Ok(())
    }

    /// 第二步：等到可以换程序的时刻。`flows` 是调用方通报的活跃的流的数量；这里不数流。
    ///
    /// - [`UpdateTiming::Now`]：不等；
    /// - [`UpdateTiming::Idle`]：等到数量是 0，等的时候状态是 [`UpdateStatus::WaitingForIdle`]；
    /// - [`UpdateTiming::AfterDrain`]：状态变成 [`UpdateStatus::Draining`]（调用方看到就不再接新的流），等到数量是 0。
    ///
    /// 可以随时丢弃再用别的时机重新调用，比如等空闲的时候管理员点了"立即更新"。
    /// 返回之后要马上调用 [`install`](Self::install)：这中间新来的流会被重启打断。
    ///
    /// # Errors
    ///
    /// `flows` 的发送端没了。
    pub async fn wait_for_slot(
        &self,
        prepared: &Prepared,
        timing: UpdateTiming,
        flows: &mut watch::Receiver<usize>,
    ) -> Result<(), UpdateError> {
        timing::wait_for_slot(timing, &prepared.version, flows, &self.status)
            .await
            .inspect_err(|error| self.fail(Some(&prepared.version), error))
    }

    /// 第三步：把新程序换上去，旧的留作备份，并记下"这个版本待确认"。成功之后调用方要重启进程。
    ///
    /// 换之前再核对一次新程序的内容：等时机可能等了很久。
    ///
    /// # Errors
    ///
    /// 新程序的文件不见了或者被改过、状态写不了、文件换不了。这时正在运行的程序没有被动过。
    pub async fn install(&self, prepared: Prepared) -> Result<RestartRequired, UpdateError> {
        let Prepared { version, artifact } = prepared;
        self.status.send_replace(UpdateStatus::Installing { version: version.clone() });
        let restart = self.swap_in(&version, &artifact).await.inspect_err(|error| self.fail(Some(&version), error))?;
        tracing::info!(%version, previous = %self.running, "新程序已经换上去，需要重启");
        self.status.send_replace(UpdateStatus::Restarting { version });
        Ok(restart)
    }

    async fn swap_in(&self, version: &Version, artifact: &Artifact) -> Result<RestartRequired, UpdateError> {
        let candidate = self.files.candidate();
        let path = candidate.clone();
        let digest = tokio::task::spawn_blocking(move || FileDigest::of_file(&path))
            .await
            .expect("算哈希的线程不会 panic：里面只有读文件和喂数据")
            .map_err(UpdateError::io("读", &candidate))?;
        if !digest.matches(artifact) {
            discard(&candidate);
            return Err(UpdateError::Corrupt {
                expected_size: artifact.size,
                expected: artifact.sha256,
                actual_size: digest.size,
                actual: digest.sha256,
            });
        }

        let _transition = self.transition();
        let mut state = self.state.load()?;
        if let Some(installed) = self.awaiting_restart(&state) {
            return Err(Refusal::RestartPending { installed }.into());
        }
        // 先记账再换文件。反过来的话，换完还没记上就断电，新程序起来之后没有人知道它需要确认，坏了也不会被换回。
        // 记了账却没换成（下面失败，或者这中间断电），启动时会发现版本对不上，把这条记录作废。
        state.pending = Some(Pending { version: version.clone(), previous: self.running.clone(), starts: 0, since: state::now() });
        self.state.save(&state)?;
        if let Err(error) = self.files.install_candidate() {
            state.pending = None;
            if let Err(error) = self.state.save(&state) {
                tracing::warn!(error = %describe(&error), "替换程序失败之后没能撤掉待确认的记录，下次启动时会作废它");
            }
            return Err(UpdateError::io("替换程序文件", self.files.program())(error));
        }
        self.remove_staged_packages(None);
        Ok(RestartRequired { version: version.clone() })
    }

    /// 三步连起来做：准备、等时机、换上去。成功之后调用方要重启进程。
    ///
    /// controller 中途改了主意（换了时机，或者有了更新的版本）时，丢弃这个 future 重新调用：取过的包不会重新取。
    ///
    /// # Errors
    ///
    /// 见 [`prepare`](Self::prepare)、[`wait_for_slot`](Self::wait_for_slot)、[`install`](Self::install)。
    pub async fn update<S: PackageSource>(
        &self,
        signed: &SignedManifest,
        timing: UpdateTiming,
        source: &S,
        flows: &mut watch::Receiver<usize>,
    ) -> Result<RestartRequired, UpdateError> {
        let prepared = self.prepare(signed, timing, source).await?;
        self.wait_for_slot(&prepared, timing, flows).await?;
        self.install(prepared).await
    }

    /// 正在运行的这个程序是不是刚换上来、还没有被确认的。
    fn unconfirmed(&self, state: &State) -> Option<Pending> {
        state.pending.clone().filter(|pending| pending.version == self.running)
    }

    /// 这个进程已经把别的版本换上去了、还没有重启：这时程序文件和备份都不是它以为的样子，不能再换一次。
    fn awaiting_restart(&self, state: &State) -> Option<Version> {
        state.pending.as_ref().filter(|pending| pending.version != self.running).map(|pending| pending.version.clone())
    }

    /// 换回旧程序并把新版本记为失败。备份不见了就没有东西可换，返回 `None`，继续跑现在的程序。
    fn roll_back(&self, mut state: State, pending: &Pending, reason: String) -> Result<Option<RolledBack>, UpdateError> {
        state.pending = None;
        if !self.files.has_backup() {
            tracing::error!(version = %pending.version, %reason, "新版本没有被确认，但备份的旧程序不见了，只能继续运行新版本");
            self.state.save(&state)?;
            return Ok(None);
        }
        self.files.restore_backup().map_err(UpdateError::io("换回旧程序", self.files.program()))?;
        state.record_failure(pending.version.clone(), reason.clone());
        self.state.save(&state)?;
        tracing::warn!(version = %pending.version, previous = %pending.previous, %reason, "新版本没有被确认，已经换回旧程序，需要重启");
        self.status.send_replace(UpdateStatus::Failed {
            version: Some(pending.version.clone()),
            kind: FailureKind::VersionRejected,
            reason: reason.clone(),
        });
        Ok(Some(RolledBack { failed: pending.version.clone(), reason, restart: RestartRequired { version: pending.previous.clone() } }))
    }

    /// 旧程序在换回之后重新启动时，把"上次想更新到某个版本，失败了"摆到状态上，好让 controller 一连上就知道。
    fn announce_standing_failure(&self, state: &State) {
        if let Some(failure) = state.failed.iter().filter(|failure| failure.version > self.running).max_by(|a, b| a.version.cmp(&b.version))
        {
            self.status.send_replace(UpdateStatus::Failed {
                version: Some(failure.version.clone()),
                kind: FailureKind::VersionRejected,
                reason: failure.reason.clone(),
            });
        }
    }

    fn refuse(&self, version: Option<&Version>, refusal: Refusal) -> UpdateError {
        let not_newer = matches!(refusal, Refusal::NotNewer { .. });
        let error = UpdateError::from(refusal);
        if not_newer {
            // 节点已经是这个版本或者更新的了，没有什么出了错，所以不去动状态。
            tracing::info!(reason = %describe(&error), "不装这份清单");
        } else {
            self.fail(version, &error);
        }
        error
    }

    fn fail(&self, version: Option<&Version>, error: &UpdateError) {
        let reason = describe(error);
        tracing::warn!(version = version.map(tracing::field::display), %reason, "更新没有成功");
        self.status.send_replace(UpdateStatus::Failed { version: version.cloned(), kind: error.kind(), reason });
    }

    /// 没有待确认的更新时，把程序旁边不该再留着的文件清掉：备份，和没换上去的新程序。
    /// 取了一半的包不在这里清：它留在状态目录里，下次接着取。
    fn tidy(&self) {
        if let Err(error) = self.files.remove_backup() {
            tracing::warn!(path = %self.files.backup().display(), %error, "删不掉备份的旧程序");
        }
        self.files.remove_leftovers();
    }

    /// 删掉取下来的包，`keep` 除外。
    fn remove_staged_packages(&self, keep: Option<&Path>) {
        let Ok(entries) = fs::read_dir(&self.update_dir) else { return };
        entries
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().starts_with(STAGED_PREFIX))
            .map(|entry| entry.path())
            .filter(|path| Some(path.as_path()) != keep)
            .for_each(|path| discard(&path));
    }
}

/// 删掉一个更新留下的文件。删不掉只是占地方，下次还会再清，所以只记一条日志。
fn discard(path: &Path) {
    match fs::remove_file(path) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => tracing::warn!(path = %path.display(), %error, "删不掉更新留下的文件"),
        _ => {}
    }
}

fn overdue_reason() -> String {
    format!("新程序换上去 {} 分钟还没有连上 controller", CONFIRMATION_WINDOW.as_secs() / 60)
}

#[cfg(test)]
mod tests;
