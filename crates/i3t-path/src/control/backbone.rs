//! 骨干矩阵的行：收别的 relay 的行，本节点是 relay 时发自己的。

use std::time::Instant;

use bytes::Bytes;
use i3t_crypto::Signature;
use i3t_proto::NodeId;
use i3t_proto::backbone::{Link, LinkQuality, Row};

use super::{Control, NEWCOMER_BATCH, RelayEntry, RowGate};
use crate::SessionLeg;
use crate::relays::Leg;
use crate::timing::ROW_INTERVAL;

impl Control {
    /// 骨干矩阵的一行，是从 `via` 这个 relay 的会话上收到的（它自己的，或者它转来的别的 relay 的）。
    pub(super) fn on_row(&mut self, via: NodeId, frame: &Bytes, now: Instant) {
        // 只认现在有会话的 relay 发来的：没有会话就没有核对过它的地址。
        if self.relays.get(&via).is_none_or(|entry| entry.link.leg().is_none()) {
            return;
        }
        let Some((origin, seq)) = Row::peek(frame) else { return };
        let Some(origin_key) = self.netmap.relay(origin).map(|relay| relay.key).filter(|_| origin != self.shared.me) else { return };
        // 不比手上的新就不用往下看了：验签名是这里唯一花钱的事。来源地址是可以伪造的，所以验的次数也有上限，
        // 按行的主人分开算：一个 relay 不停地发新的行，花掉的只是它自己的额度。正常情况下每个 relay 几秒才有一行新的。
        if !self.matrix.is_news(origin, seq, now) {
            return;
        }
        let gate = self.row_gates.entry(origin).or_insert_with(|| RowGate::new(now));
        if !gate.budget.take(now) {
            tracing::debug!(%via, %origin, "这个 relay 的矩阵行来得太多，这一行不验了");
            return;
        }
        let (Some((row, signature)), Some(signed)) = (Row::parse(frame), Row::signed_part(frame)) else { return };
        if i3t_crypto::verify(&origin_key, Row::PURPOSE, signed, &Signature::from_bytes(signature)).is_err() {
            tracing::debug!(%via, %origin, "骨干矩阵的一行签名不对，丢弃");
            return;
        }
        let state = row.state;
        if self.matrix.insert(row, frame.clone(), now) {
            tracing::info!(relay = %origin, ?state, %via, "relay 的状态变了");
            // 状态变了的行不等下一轮，立刻传给本节点 relay 上的会话：经它中转的节点也要尽快知道。
            // 同一个 relay 的行转得太勤就等下一轮一起发，状态来回翻的 relay 借不了本节点放大。
            if let Some(local) = self.shared.local_relay.get()
                && gate.forward(now)
            {
                local.broadcast(frame);
            }
        }
    }

    /// 本节点是 relay 时：把自己测到的骨干链路和手上别的 relay 的行发给会话。定期发；自己的状态变了立刻发。
    pub(super) fn publish_rows(&mut self, now: Instant) {
        let Some(local) = self.shared.local_relay.get() else { return };
        let Some(status) = local.status() else {
            self.own_state = None;
            return;
        };
        let state_changed = self.own_state != Some(status.state);
        let since_sent = self.rows_sent.map(|at| now.saturating_duration_since(at));
        let due = since_sent.is_none_or(|since| since >= ROW_INTERVAL);
        // 有新的会话：让它马上拿到矩阵。一批节点同时挂上来时合成一次发，不是每来一个发一轮。
        let newcomers = self.sessions_seen != status.sessions_opened && since_sent.is_none_or(|since| since >= NEWCOMER_BATCH);
        if !due && !state_changed && !newcomers {
            return;
        }
        self.own_state = Some(status.state);
        self.sessions_seen = status.sessions_opened;
        self.rows_sent = Some(now);
        let me = self.shared.me;
        let links: Vec<Link> = self
            .netmap
            .relays
            .iter()
            .filter(|relay| relay.id != me)
            .map(|relay| {
                // 一条骨干连接算通，要本节点在对方那里有会话（包从那里送过去），对方在本节点这里也有（它据此认本节点发的包），
                // 两个会话还要在同一处：都走 UDP，或者都在同一条 TCP 上（见 [`crate::backbone`]）。
                let theirs = local.session_leg(relay.id);
                let paired = |entry: &&RelayEntry| match (entry.link.leg(), theirs) {
                    (Some(Leg::Udp(_)), Some(SessionLeg::Udp)) => true,
                    (Some(Leg::Tcp), Some(theirs)) => self.legs.get(&relay.id).is_some_and(|leg| leg.carries(theirs)),
                    (Some(Leg::Udp(_) | Leg::Tcp) | None, _) => false,
                };
                let quality = self
                    .relays
                    .get(&relay.id)
                    .filter(paired)
                    .and_then(|entry| entry.link.quality(now))
                    .map(|quality| LinkQuality::from_measured(quality.rtt, quality.jitter, quality.lost, quality.probes));
                Link { relay: relay.id, quality }
            })
            .collect();
        local.publish_row(&links);
        if due || newcomers {
            for frame in self.matrix.frames(now) {
                local.broadcast(frame);
            }
        }
    }
}
