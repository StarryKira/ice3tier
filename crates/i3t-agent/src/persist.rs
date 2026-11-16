//! 运行中要跨重启留下来的两样东西：端口写 0 的端口转发挑定的端口，和出口本账期的流量用量。
//!
//! 都存在状态目录里（见 [`crate::state`]）。存不下来不影响这次运行，只记一条日志：端口下次可能换一个，
//! 用量下次从上一次存下的数接着记。

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use i3t_access::PortBook;
use i3t_exit::QuotaState;

use crate::runtime::NodeExit;
use crate::state::{QuotaRecord, StateDir};

/// 出口的用量多久存一次。节点被杀掉时最多丢这么久的计数。
const QUOTA_SAVE_INTERVAL: Duration = Duration::from_secs(10);

/// 端口写 0 的端口转发挑定的端口，记在状态目录的 `ports.json` 里。
#[derive(Debug)]
pub(crate) struct StatePorts {
    state: StateDir,
    ports: Mutex<BTreeMap<u32, u16>>,
}

impl StatePorts {
    pub(crate) fn open(state: StateDir) -> Self {
        let ports = state.forward_ports().unwrap_or_else(|error| {
            tracing::warn!("端口转发挑定的端口读不出来，这次重新挑：{error:#}");
            BTreeMap::new()
        });
        Self { state, ports: Mutex::new(ports) }
    }
}

impl PortBook for StatePorts {
    fn port(&self, forward: u32) -> Option<u16> {
        self.ports.lock().expect("forward ports mutex poisoned").get(&forward).copied()
    }

    fn remember(&self, forward: u32, port: u16) {
        let mut ports = self.ports.lock().expect("forward ports mutex poisoned");
        ports.insert(forward, port);
        if let Err(error) = self.state.save_forward_ports(&ports) {
            tracing::warn!(forward, port, "端口转发挑定的端口没有存下来，重启之后可能换一个：{error:#}");
        }
    }
}

/// 每隔一会儿把出口的用量存下来（有变化才写），直到任务被取消。
pub(crate) async fn save_quota(state: StateDir, exit: Arc<NodeExit>) {
    let mut saved = None;
    let mut tick = tokio::time::interval(QUOTA_SAVE_INTERVAL);
    loop {
        tick.tick().await;
        let current = exit.quota_state();
        if saved != Some(current) && current.used_bytes > 0 {
            save(&state, current);
            saved = Some(current);
        }
    }
}

/// 节点停下之前最后存一次。
pub(crate) fn save_quota_now(state: &StateDir, exit: &NodeExit) {
    let current = exit.quota_state();
    if current.used_bytes > 0 {
        save(state, current);
    }
}

fn save(state: &StateDir, quota: QuotaState) {
    let record = QuotaRecord { period_start: quota.period_start, used_bytes: quota.used_bytes };
    if let Err(error) = state.save_exit_quota(&record) {
        tracing::warn!("出口的流量用量没有存下来，重启之后会少算一些：{error:#}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_remembered_port_is_found_again_after_reopening() {
        let dir = tempfile::tempdir().unwrap();
        let state = StateDir::open(dir.path()).unwrap();
        let ports = StatePorts::open(state.clone());
        assert_eq!(ports.port(7), None);
        ports.remember(7, 41_234);
        assert_eq!(ports.port(7), Some(41_234));
        let reopened = StatePorts::open(StateDir::open(dir.path()).unwrap());
        assert_eq!(reopened.port(7), Some(41_234), "重启之后还是这个端口");
    }
}
