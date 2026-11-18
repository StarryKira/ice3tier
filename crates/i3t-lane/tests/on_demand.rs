//! 按需拨号（节点里 `udp_fallback` 的拨号方）：拨号方把对端的地址拿掉就是放手，两边什么都不连着，也不记成失败；
//! 地址再给回来，lane 重新建好。

// clippy.toml 里的 allow-unwrap-in-tests 只管 #[test] 函数，管不到集成测试里的辅助函数。这里 unwrap 失败就是测试失败。
#![allow(clippy::unwrap_used)]

mod common;

use std::time::Duration;

use common::{Pair, quick, read_to_end, wait_for};
use i3t_lane::Payload;
use tokio::io::AsyncWriteExt;

#[tokio::test]
async fn letting_go_closes_the_control_and_idle_lanes_on_both_sides_without_counting_failures_and_dialling_again_restores_them() {
    let pair = Pair::new(quick()).await;
    pair.ready(4).await;
    let (exit_id, entry_id) = (pair.exit.id, pair.entry.id);
    let exit_before = pair.exit.lanes.status().peer(entry_id).unwrap().counts;

    // 出口放手：还认入口（入口拨来的照收），只是不再拨它。
    pair.exit.lanes.set_peers([pair.entry.peer(Vec::new())]).unwrap();
    wait_for("入口那里出口的控制 lane 和停着的 lane 都关掉", Duration::from_secs(3), || {
        let status = pair.entry.lanes.status();
        let peer = status.peer(exit_id).unwrap();
        !peer.control && peer.idle == 0
    })
    .await;
    wait_for("出口那边守着停车 lane 的任务看到它们关掉", Duration::from_secs(3), || {
        let status = pair.exit.lanes.status();
        let peer = status.peer(entry_id).unwrap();
        !peer.dials && !peer.control && peer.parked_at_peer == 0 && peer.ready == 0 && peer.counts.dialling == 0
    })
    .await;
    let exit_after = pair.exit.lanes.status().peer(entry_id).unwrap().counts;
    assert_eq!(exit_after.dial_failures, exit_before.dial_failures, "放手不是拨号失败");
    let entry_side = pair.entry.lanes.status();
    let held = entry_side.peer(exit_id).unwrap().counts;
    assert_eq!((held.refused, held.claims_failed, held.fallbacks, held.in_use), (0, 0, 0, 0), "放手不是拒绝、失败或者改走：{held:?}");
    assert!(!pair.entry.lanes.can_open(exit_id), "lane 没建着：开流的一方直接走共用的连接");

    // 又要拨了：地址给回来，控制 lane 和停着的 lane 重新建好，流照常走。
    pair.introduce();
    pair.ready(4).await;
    let ((mut send, mut recv), (mut far_send, mut far_recv)) = pair.entry_opens(b"hello", Payload::Sealed).await;
    far_send.write_all(b"again").await.unwrap();
    far_send.shutdown().await.unwrap();
    send.shutdown().await.unwrap();
    assert_eq!(read_to_end(&mut recv).await.unwrap(), b"again");
    assert_eq!(read_to_end(&mut far_recv).await.unwrap(), b"");
}
