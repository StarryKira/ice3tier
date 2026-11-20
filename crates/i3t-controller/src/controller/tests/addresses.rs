//! relay 的地址是找出来的，网卡地址落进虚拟地址段时有提醒。

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use i3t_model::admin::{AddressSource, NodeWarning, RelaySettings};
use i3t_model::control::{Declared, Hello, Report, ToController};
use tokio::sync::mpsc;

use super::super::testing::{CAN_DO_ALL, Lab, REMOTE, hello};

fn report_local(addresses: &[&str]) -> ToController {
    let local = addresses.iter().map(|text| text.parse().unwrap()).collect();
    ToController::Report(Box::new(Report { local_addresses: Some(local), ..Report::default() }))
}

fn addr(text: &str) -> SocketAddr {
    text.parse().unwrap()
}

#[test]
fn every_node_gets_the_relays_addresses_as_they_are_found_and_they_stay_while_it_is_offline() {
    let mut lab = Lab::new();
    let network = lab.network("home");
    let mut relay = lab.join_with(network.id, &[], CAN_DO_ALL);
    let mut a = lab.join(network.id, &[]);
    lab.controller.enable_relay(network.id, relay.id, RelaySettings::default()).unwrap();
    lab.settle(&mut [&mut relay, &mut a]);
    let observed = SocketAddr::new(REMOTE, 20330);
    assert_eq!(a.netmap().relays[0].udp, [observed]);

    relay.send(&lab.controller, report_local(&["192.168.1.5", "198.51.100.7", "fe80::1", "10.66.9.9"]));
    lab.settle(&mut [&mut relay, &mut a]);
    let expected = [observed, addr("192.168.1.5:20330")];
    assert_eq!(a.netmap().relays[0].udp, expected, "网卡上的地址跟着下发，和 controller 看到的重复的只算一次");
    assert_eq!(a.netmap().relays[0].tcp, expected, "UDP 和 TCP 是同一组地址");
    let view = lab.controller.node(network.id, relay.id).unwrap().node.relay.unwrap();
    let sources: Vec<AddressSource> = view.addresses.iter().map(|entry| entry.source).collect();
    assert_eq!(sources, [AddressSource::Observed, AddressSource::Local]);

    // 在节点上声明一个地址（重启 i3t 之后它带着新的声明报到）：排在最前面。
    lab.controller.disconnect(relay.session);
    let declared = Declared { relay_address: Some(addr("203.0.113.9:40330")), ..CAN_DO_ALL };
    let mut relay = lab.connect(relay.key, declared).unwrap();
    lab.settle(&mut [&mut relay, &mut a]);
    assert_eq!(a.netmap().relays[0].udp, [addr("203.0.113.9:40330"), observed, addr("192.168.1.5:20330")]);

    // 下线之后还是这些地址：存储里留着它最后报的。
    let version = a.netmap().version;
    lab.controller.disconnect(relay.session);
    lab.settle(&mut [&mut a]);
    assert_eq!(a.netmap().version, version, "relay 下线不改别人的 netmap");
    assert_eq!(lab.netmap(a.id).relays[0].udp.len(), 3);
}

#[test]
fn a_declared_address_others_cannot_connect_to_is_not_used() {
    let mut lab = Lab::new();
    let network = lab.network("home");
    for bad in ["127.0.0.1:20330", "0.0.0.0:20330", "10.66.0.200:20330", "224.0.0.1:20330"] {
        let declared = Declared { relay_address: Some(addr(bad)), ..CAN_DO_ALL };
        let relay = lab.join_offline(network.id, &[], declared);
        let node = lab.controller.enable_relay(network.id, relay, RelaySettings::default()).unwrap();
        let sources: Vec<AddressSource> = node.relay.unwrap().addresses.iter().map(|entry| entry.source).collect();
        assert_eq!(sources, [AddressSource::Observed], "{bad}");
        assert_eq!(node.declared.relay_address, None, "{bad} 不当作声明记下");
    }
}

#[test]
fn a_node_on_the_controllers_machine_is_seen_at_the_controllers_public_address() {
    let mut lab = Lab::new();
    let network = lab.network("home");
    let secret = lab.secret(network.id, &i3t_model::admin::CreateEnrollToken::default());
    let key = lab.fresh_key();
    let (outbox, _inbox) = mpsc::channel(64);
    let loopback = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let session = lab.controller.enroll(key, &secret, &hello(CAN_DO_ALL), loopback, outbox).unwrap();
    let observed = || lab.controller.node(network.id, session.node).unwrap().observed_endpoint;
    assert_eq!(observed(), Some(addr("127.0.0.1:20330")), "对外地址的主机是域名、还没解析出来：照旧是回环地址");

    let public: IpAddr = "203.0.113.1".parse().unwrap();
    lab.controller.set_public_host(Some(public)).unwrap();
    assert_eq!(observed(), Some(SocketAddr::new(public, 20330)), "解析出来之后，走回环连着的节点换成对外的地址");
    lab.controller.enable_relay(network.id, session.node, RelaySettings::default()).unwrap();
    let relay = lab.controller.node(network.id, session.node).unwrap().node.relay.unwrap();
    assert_eq!(relay.addresses[0].address, SocketAddr::new(public, 20330));

    let (outbox, _inbox) = mpsc::channel(64);
    let other = lab.fresh_key();
    let secret = lab.secret(network.id, &i3t_model::admin::CreateEnrollToken::default());
    let elsewhere = lab.controller.enroll(other, &secret, &Hello { port: 20444, ..hello(Declared::default()) }, REMOTE, outbox).unwrap();
    assert_eq!(lab.controller.node(network.id, elsewhere.node).unwrap().observed_endpoint, Some(SocketAddr::new(REMOTE, 20444)));
}

#[test]
fn an_interface_address_inside_the_networks_range_is_a_warning_on_the_node_and_in_the_overview() {
    let mut lab = Lab::new();
    let network = lab.network("home");
    let mut a = lab.join(network.id, &[]);
    let b = lab.join(network.id, &[]);
    a.send(&lab.controller, report_local(&["192.168.1.5", "10.66.200.7"]));
    b.send(&lab.controller, report_local(&["192.168.1.6"]));
    lab.settle(&mut [&mut a]);

    let warning = NodeWarning::RangeOverlap { address: "10.66.200.7".parse().unwrap() };
    assert_eq!(lab.controller.node(network.id, a.id).unwrap().node.warnings, [warning]);
    assert!(lab.controller.node(network.id, b.id).unwrap().node.warnings.is_empty());
    let overview = lab.controller.overview(network.id).unwrap();
    assert_eq!(overview.warnings.len(), 1);
    assert_eq!((overview.warnings[0].node.id, overview.warnings[0].warning), (a.id, warning));
    assert!(lab.controller.nodes(network.id).unwrap().iter().any(|node| node.warnings == [warning]), "列表里也有");

    a.send(&lab.controller, report_local(&["192.168.1.5"]));
    assert!(lab.controller.overview(network.id).unwrap().warnings.is_empty(), "地址换掉之后提醒就没了");
}
