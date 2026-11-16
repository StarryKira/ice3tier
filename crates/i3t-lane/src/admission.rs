//! 拨进来、还没握手完的连接的名额。
//!
//! 谁都可以连上来，所以握手没完成的连接要有上限，不能让人占着位子把真正的对端挡在外面：
//!
//! - 同一个来源（IPv4 地址，或者 IPv6 的 /64）同时最多 [`MAX_PER_SOURCE`] 条，全部来源合计最多 [`MAX_TOTAL`] 条；
//! - 满了的时候挤掉最老的一条（同一来源满了挤同一来源的），而不是拒绝新来的：真正的对端握手只要一个多往返，
//!   不会是最老的那一条；赖着不走的连接才会被挤掉。
//!
//! 一个拨号方在池被抽干时一次会并发拨上百条（见 `dialler` 里的并发上限），同一来源的名额要比它大。

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::net::{IpAddr, Ipv6Addr};
use std::sync::{Arc, Mutex};

use tokio::task::AbortHandle;

pub(crate) const MAX_PER_SOURCE: usize = 256;
pub(crate) const MAX_TOTAL: usize = 1024;

#[derive(Debug, Default)]
pub(crate) struct Admission {
    state: Mutex<State>,
    limits: Limits,
}

#[derive(Debug, Clone, Copy)]
struct Limits {
    per_source: usize,
    total: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self { per_source: MAX_PER_SOURCE, total: MAX_TOTAL }
    }
}

#[derive(Debug, Default)]
struct State {
    next: u64,
    by_source: HashMap<IpAddr, VecDeque<u64>>,
    /// 按到达的先后排：编号越小越老。
    all: BTreeMap<u64, Entry>,
}

#[derive(Debug)]
struct Entry {
    source: IpAddr,
    abort: Option<AbortHandle>,
}

/// 一条握手中的连接占着的名额。握手结束（成功或失败）时丢弃。
#[derive(Debug)]
pub(crate) struct Ticket {
    admission: Arc<Admission>,
    id: u64,
}

impl Admission {
    #[cfg(test)]
    fn with_limits(per_source: usize, total: usize) -> Self {
        Self { state: Mutex::default(), limits: Limits { per_source, total } }
    }

    /// 给来自 `from` 的一条新连接一个名额，必要时挤掉老的。返回名额和挤掉了几条。
    pub(crate) fn admit(self: &Arc<Self>, from: IpAddr) -> (Ticket, usize) {
        let source = source_key(from);
        let mut state = self.state.lock().expect("admission mutex poisoned");
        let mut evicted = 0;
        let same_source = state.by_source.get(&source).map_or(0, VecDeque::len);
        if same_source >= self.limits.per_source
            && let Some(oldest) = state.by_source.get(&source).and_then(|queue| queue.front().copied())
        {
            state.evict(oldest);
            evicted += 1;
        }
        if state.all.len() >= self.limits.total
            && let Some(oldest) = state.all.keys().next().copied()
        {
            state.evict(oldest);
            evicted += 1;
        }
        let id = state.next;
        state.next += 1;
        state.by_source.entry(source).or_default().push_back(id);
        state.all.insert(id, Entry { source, abort: None });
        (Ticket { admission: Arc::clone(self), id }, evicted)
    }

    /// 编号是 `id` 的名额对应的握手任务起来了：被挤掉时要能取消它。名额已经被挤掉的话现在就取消。
    pub(crate) fn attach(&self, id: u64, abort: AbortHandle) {
        let mut state = self.state.lock().expect("admission mutex poisoned");
        match state.all.get_mut(&id) {
            Some(entry) => entry.abort = Some(abort),
            None => abort.abort(),
        }
    }

    #[cfg(test)]
    fn pending(&self) -> usize {
        self.state.lock().expect("admission mutex poisoned").all.len()
    }
}

impl State {
    fn evict(&mut self, id: u64) {
        if let Some(entry) = self.remove(id)
            && let Some(abort) = entry.abort
        {
            abort.abort();
        }
    }

    fn remove(&mut self, id: u64) -> Option<Entry> {
        let entry = self.all.remove(&id)?;
        if let Some(queue) = self.by_source.get_mut(&entry.source) {
            queue.retain(|&other| other != id);
            if queue.is_empty() {
                self.by_source.remove(&entry.source);
            }
        }
        Some(entry)
    }
}

impl Ticket {
    pub(crate) fn id(&self) -> u64 {
        self.id
    }
}

impl Drop for Ticket {
    fn drop(&mut self) {
        let mut state = self.admission.state.lock().expect("admission mutex poisoned");
        state.remove(self.id);
    }
}

/// 按来源计数时用的键：IPv4 地址本身，IPv6 取前 64 位（一个用户通常拿到整个 /64）。
pub(crate) fn source_key(ip: IpAddr) -> IpAddr {
    match ip.to_canonical() {
        IpAddr::V4(ip) => IpAddr::V4(ip),
        IpAddr::V6(ip) => IpAddr::V6(Ipv6Addr::from(ip.to_bits() & !u128::from(u64::MAX))),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn ip(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    #[test]
    fn the_oldest_from_a_full_source_makes_room() {
        let admission = Arc::new(Admission::with_limits(2, 100));
        let (first, evicted) = admission.admit(ip("10.0.0.1"));
        assert_eq!(evicted, 0);
        let (_second, _) = admission.admit(ip("10.0.0.1"));
        let (_third, evicted) = admission.admit(ip("10.0.0.1"));
        assert_eq!(evicted, 1, "同一来源的第三条挤掉最老的");
        assert_eq!(admission.pending(), 2);
        drop(first);
        assert_eq!(admission.pending(), 2, "被挤掉的名额丢弃时不能把别人的名额也删了");
        let (_other, evicted) = admission.admit(ip("10.0.0.2"));
        assert_eq!(evicted, 0, "别的来源不受影响");
    }

    #[test]
    fn ipv6_sources_are_counted_per_slash_64() {
        let admission = Arc::new(Admission::with_limits(1, 100));
        let (_a, _) = admission.admit(ip("2001:db8::1"));
        let (_b, evicted) = admission.admit(ip("2001:db8::ffff:1"));
        assert_eq!(evicted, 1, "同一个 /64 算一个来源");
        let (_c, evicted) = admission.admit(ip("2001:db8:0:1::1"));
        assert_eq!(evicted, 0);
        assert_eq!(source_key(ip("::ffff:10.0.0.1")), ip("10.0.0.1"), "映射的 IPv4 地址按 IPv4 算");
    }

    #[test]
    fn the_total_is_bounded_by_evicting_the_oldest_anywhere() {
        let admission = Arc::new(Admission::with_limits(10, 3));
        let tickets: Vec<_> = ["10.0.0.1", "10.0.0.2", "10.0.0.3", "10.0.0.4"].into_iter().map(|from| admission.admit(ip(from))).collect();
        assert_eq!(tickets.iter().map(|(_, evicted)| evicted).sum::<usize>(), 1);
        assert_eq!(admission.pending(), 3);
    }

    #[tokio::test]
    async fn an_evicted_handshake_task_is_cancelled() {
        let admission = Arc::new(Admission::with_limits(1, 10));
        let (ticket, _) = admission.admit(ip("10.0.0.1"));
        let id = ticket.id();
        let handle = tokio::spawn(async move {
            let _ticket = ticket;
            tokio::time::sleep(Duration::from_secs(60)).await;
        });
        admission.attach(id, handle.abort_handle());
        let (_newer, evicted) = admission.admit(ip("10.0.0.1"));
        assert_eq!(evicted, 1);
        let outcome = tokio::time::timeout(Duration::from_secs(5), handle).await.unwrap();
        assert!(outcome.unwrap_err().is_cancelled(), "被挤掉的握手任务要被取消");
    }

    #[tokio::test]
    async fn a_task_attached_after_its_ticket_was_evicted_is_cancelled_at_once() {
        let admission = Arc::new(Admission::with_limits(1, 10));
        let (ticket, _) = admission.admit(ip("10.0.0.1"));
        let id = ticket.id();
        let (_newer, _) = admission.admit(ip("10.0.0.1"));
        let handle = tokio::spawn(std::future::pending::<()>());
        admission.attach(id, handle.abort_handle());
        assert!(handle.await.unwrap_err().is_cancelled());
        drop(ticket);
    }
}
