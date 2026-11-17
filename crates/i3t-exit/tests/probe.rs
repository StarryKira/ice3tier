//! 出口测试（`Outbound::probe`）：按替别人连时的同一套规矩试着连一个目标，说出每一步和拒绝的原因。

// clippy.toml 里的 allow-unwrap-in-tests 只管 #[test] 函数，管不到集成测试里的辅助函数。这里 unwrap 失败就是测试失败。
#![allow(clippy::unwrap_used)]

mod common;

use std::sync::Arc;

use common::{FakeSystem, LAPTOP, PUBLIC_IP, Target, netmap};
use i3t_exit::Outbound;
use i3t_model::control::exit_test::{ExitTestCause, ExitVia};
use i3t_model::netmap::{ExitRole, IpFamily};
use i3t_proto::flow::Host;
use tokio::sync::watch;

fn outbound(exit: Option<ExitRole>) -> (Outbound<FakeSystem>, FakeSystem) {
    let mut netmap = netmap(LAPTOP);
    netmap.me.exit = exit;
    let (sender, receiver) = watch::channel(Arc::new(netmap));
    // 拨号器只借用接收端，发送端丢了也没有关系：netmap 不会再变。
    drop(sender);
    let system = FakeSystem::default();
    (Outbound::new(receiver, system.clone()), system)
}

fn host(text: &str) -> Host {
    text.parse().unwrap()
}

#[tokio::test]
async fn a_reachable_target_reports_resolution_connection_and_the_address_used() {
    let (outbound, system) = outbound(Some(ExitRole { ports: None, ..ExitRole::default() }));
    let target = Target::bind("127.0.0.1").await;
    system.name("example.com", &[PUBLIC_IP, "2606:2800:220:1::248"]);
    system.redirect(&format!("{PUBLIC_IP}:8443"), target.addr());

    let probe = outbound.probe(&host("example.com"), 8443).await;
    let result = probe.result;
    assert_eq!((result.via, result.refused), (ExitVia::Direct, None));
    let resolve = result.resolve.unwrap();
    assert_eq!(resolve.addresses, [PUBLIC_IP.parse::<std::net::IpAddr>().unwrap(), "2606:2800:220:1::248".parse().unwrap()]);
    assert_eq!((resolve.family, resolve.cause), (IpFamily::Any, None));
    let connect = result.connect.unwrap();
    assert_eq!((connect.address, connect.cause), (Some(format!("{PUBLIC_IP}:8443").parse().unwrap()), None));
    assert!(probe.stream.is_some(), "连接交给调用方做 TLS 握手");
    let _accepted = target.accept().await;
    assert_eq!(system.resolved(), ["example.com"], "解析不用缓存：量的是这一次");
}

#[tokio::test]
async fn the_exits_own_policy_refuses_with_the_same_cause_as_for_a_user() {
    let (outbound, system) = outbound(None);
    assert_eq!(outbound.probe(&host("example.com"), 443).await.result.refused, Some(ExitTestCause::NotEnabled));

    let (outbound, system2) = outbound_with(ExitRole { ports: Some(vec![443]), ..ExitRole::default() });
    let probe = outbound.probe(&host("example.com"), 80).await.result;
    assert_eq!(probe.refused, Some(ExitTestCause::PortNotAllowed { port: 80 }));
    assert!(probe.resolve.is_none() && probe.connect.is_none(), "端口不允许时什么都不做");

    system2.name("intranet.example", &["10.1.2.3"]);
    let probe = outbound.probe(&host("intranet.example"), 443).await.result;
    assert_eq!(probe.refused, Some(ExitTestCause::AddressNotAllowed { address: "10.1.2.3".parse().unwrap() }));
    assert_eq!(probe.resolve.unwrap().addresses.len(), 1, "解析这一步做了，结果在里面");
    assert!(probe.connect.is_none() && system2.connected().is_empty(), "不该连的地址一次也没有连");

    let probe = outbound.probe(&host("192.168.77.20"), 22).await.result;
    assert_eq!(probe.refused, Some(ExitTestCause::LanNotEnabled), "局域网的地址按局域网目标查");
    drop(system);
}

fn outbound_with(exit: ExitRole) -> (Outbound<FakeSystem>, FakeSystem) {
    outbound(Some(exit))
}

#[tokio::test]
async fn resolution_and_connection_failures_say_which_step_and_why() {
    let (outbound, system) = outbound_with(ExitRole { ports: None, family: IpFamily::Ipv6, ..ExitRole::default() });
    let probe = outbound.probe(&host("nowhere.example"), 443).await.result;
    assert_eq!(probe.resolve.unwrap().cause, Some(ExitTestCause::NameNotFound));
    assert_eq!((probe.refused, probe.connect), (None, None));

    system.name("v4only.example", &[PUBLIC_IP]);
    let probe = outbound.probe(&host("v4only.example"), 443).await.result;
    assert_eq!(probe.refused, Some(ExitTestCause::FamilyUnavailable { family: IpFamily::Ipv6 }));

    let (outbound, system) = outbound_with(ExitRole { ports: None, ..ExitRole::default() });
    let closed = common::closed_port().await;
    system.redirect(&format!("{PUBLIC_IP}:443"), closed);
    let probe = outbound.probe(&host(PUBLIC_IP), 443).await.result;
    assert!(probe.resolve.is_none(), "写成地址的目标不用解析");
    let connect = probe.connect.unwrap();
    assert_eq!((connect.address, connect.cause), (None, Some(ExitTestCause::Refused)));

    let (outbound, _) = outbound_with(ExitRole { ports: None, upstream_socks5: Some(closed), ..ExitRole::default() });
    let probe = outbound.probe(&host(PUBLIC_IP), 443).await.result;
    assert_eq!(probe.via, ExitVia::Upstream);
    assert_eq!(probe.connect.unwrap().cause, Some(ExitTestCause::UpstreamFailed), "连不上上游代理");
}
