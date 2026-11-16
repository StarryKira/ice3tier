//! 什么时候换程序。
//!
//! 时机由 controller 定（[`UpdateTiming`]），流有多少由调用方报（一个 `watch`，值是活跃的流的数量）。
//! 这里不数流，只是把两者对起来：
//!
//! | 时机 | 有流的时候 | 没有流的时候 |
//! |---|---|---|
//! | `Now` | 换 | 换 |
//! | `Idle` | 状态变成"等待空闲"，等到没有流 | 换 |
//! | `AfterDrain` | 状态变成"排空中"，等到没有流 | 状态变成"排空中"，然后换 |
//!
//! 调用方看到"排空中"就不再接新的流，直到进程重启。没有流的时候也先宣布再换，是为了把从这里到重启之间的门也关上。

use i3t_model::update::{UpdateStatus, UpdateTiming, Version};
use tokio::sync::watch;

use crate::error::UpdateError;

/// 等到可以换程序的时刻。随时可以丢弃这个 future：这里只是在等，没有动任何东西。
///
/// # Errors
///
/// `flows` 的发送端没了：调用方不再通报流的数量，也就等不到了。
pub(crate) async fn wait_for_slot(
    timing: UpdateTiming,
    version: &Version,
    flows: &mut watch::Receiver<usize>,
    status: &watch::Sender<UpdateStatus>,
) -> Result<(), UpdateError> {
    let waiting = match timing {
        UpdateTiming::Now => return Ok(()),
        UpdateTiming::Idle if *flows.borrow() == 0 => return Ok(()),
        UpdateTiming::Idle => UpdateStatus::WaitingForIdle { version: version.clone() },
        UpdateTiming::AfterDrain => UpdateStatus::Draining { version: version.clone() },
    };
    tracing::info!(%version, ?timing, flows = *flows.borrow(), "新程序已经准备好，等没有流的时候再换");
    status.send_replace(waiting);
    flows.wait_for(|active| *active == 0).await.map_err(|_| UpdateError::Cancelled)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::pin::pin;
    use std::task::{Context, Poll, Waker};

    use super::*;

    static VERSION: Version = Version::new(0, 2, 0);

    /// 推进一步，看它是不是已经结束。
    fn poll_once<T>(future: std::pin::Pin<&mut impl Future<Output = T>>) -> Poll<T> {
        future.poll(&mut Context::from_waker(Waker::noop()))
    }

    #[test]
    fn now_does_not_wait_for_flows() {
        let (_flows_tx, mut flows) = watch::channel(7);
        let (status, _) = watch::channel(UpdateStatus::Idle);
        let wait = pin!(wait_for_slot(UpdateTiming::Now, &VERSION, &mut flows, &status));
        assert!(matches!(poll_once(wait), Poll::Ready(Ok(()))));
        assert_eq!(*status.borrow(), UpdateStatus::Idle);
    }

    #[test]
    fn idle_proceeds_at_once_when_there_are_no_flows() {
        let (_flows_tx, mut flows) = watch::channel(0);
        let (status, _) = watch::channel(UpdateStatus::Idle);
        let wait = pin!(wait_for_slot(UpdateTiming::Idle, &VERSION, &mut flows, &status));
        assert!(matches!(poll_once(wait), Poll::Ready(Ok(()))));
        assert_eq!(*status.borrow(), UpdateStatus::Idle, "不用等的时候不该报等待空闲");
    }

    #[test]
    fn idle_waits_until_the_last_flow_ends() {
        let (flows_tx, mut flows) = watch::channel(2);
        let (status, _) = watch::channel(UpdateStatus::Idle);
        let mut wait = pin!(wait_for_slot(UpdateTiming::Idle, &VERSION, &mut flows, &status));
        assert!(poll_once(wait.as_mut()).is_pending());
        assert_eq!(*status.borrow(), UpdateStatus::WaitingForIdle { version: VERSION.clone() });
        flows_tx.send_replace(1);
        assert!(poll_once(wait.as_mut()).is_pending());
        flows_tx.send_replace(3);
        assert!(poll_once(wait.as_mut()).is_pending());
        flows_tx.send_replace(0);
        assert!(matches!(poll_once(wait), Poll::Ready(Ok(()))));
    }

    #[test]
    fn after_drain_announces_draining_even_without_flows() {
        let (_flows_tx, mut flows) = watch::channel(0);
        let (status, _) = watch::channel(UpdateStatus::Idle);
        let wait = pin!(wait_for_slot(UpdateTiming::AfterDrain, &VERSION, &mut flows, &status));
        assert!(matches!(poll_once(wait), Poll::Ready(Ok(()))));
        assert_eq!(*status.borrow(), UpdateStatus::Draining { version: VERSION.clone() });
    }

    #[test]
    fn after_drain_waits_for_the_flows_in_hand() {
        let (flows_tx, mut flows) = watch::channel(1);
        let (status, _) = watch::channel(UpdateStatus::Idle);
        let mut wait = pin!(wait_for_slot(UpdateTiming::AfterDrain, &VERSION, &mut flows, &status));
        assert!(poll_once(wait.as_mut()).is_pending());
        assert_eq!(*status.borrow(), UpdateStatus::Draining { version: VERSION.clone() });
        flows_tx.send_replace(0);
        assert!(matches!(poll_once(wait), Poll::Ready(Ok(()))));
    }

    #[test]
    fn waiting_ends_with_an_error_when_the_caller_stops_reporting_flows() {
        let (flows_tx, mut flows) = watch::channel(1);
        let (status, _) = watch::channel(UpdateStatus::Idle);
        let mut wait = pin!(wait_for_slot(UpdateTiming::Idle, &VERSION, &mut flows, &status));
        assert!(poll_once(wait.as_mut()).is_pending());
        drop(flows_tx);
        assert!(matches!(poll_once(wait), Poll::Ready(Err(UpdateError::Cancelled))));
    }
}
