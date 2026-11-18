//! 代理：出口和规则在什么时候查、名字和用户名密码怎么补、出口没了之后怎样。

use i3t_model::admin::{Direction, ErrorCode, ProxyStatus, UpdateProxy};
use i3t_model::control::Declared;
use i3t_model::netmap::ExitRole;

use super::super::testing::{CAN_DO_ALL, Lab, proxy};
use super::{add_rule, drop_default_rule, rename};
use crate::error::Error;

fn described(error: &Error) -> (ErrorCode, Option<String>) {
    (error.code(), error.field().map(str::to_owned))
}

#[test]
fn a_proxy_needs_another_node_that_is_an_exit_and_that_the_rules_let_it_reach() {
    let mut lab = Lab::new();
    let network = lab.network("home");
    drop_default_rule(&lab, network.id);
    let laptop = lab.join_offline(network.id, &["laptops"], Declared::default());
    let gw = lab.join_offline(network.id, &["exits"], CAN_DO_ALL);
    let create = |bind: &str, exit| lab.controller.create_proxy(network.id, proxy(laptop, bind, exit));

    let refused = create("127.0.0.1:1080", laptop).unwrap_err();
    assert_eq!(described(&refused), (ErrorCode::ExitIsNode, Some("exit".to_owned())));
    let refused = create("127.0.0.1:1080", gw).unwrap_err();
    assert_eq!(described(&refused), (ErrorCode::ExitNotEnabled, Some("exit".to_owned())), "{refused}");
    lab.controller.enable_exit(network.id, gw, ExitRole::default()).unwrap();
    let refused = create("127.0.0.1:1080", gw).unwrap_err();
    assert_eq!(described(&refused), (ErrorCode::AccessDenied, Some("exit".to_owned())), "{refused}");
    // 反方向的规则不算：要监听的节点能访问出口。
    add_rule(&lab, network.id, "exits", "laptops", Direction::OneWay);
    assert_eq!(create("127.0.0.1:1080", gw).unwrap_err().code(), ErrorCode::AccessDenied);
    add_rule(&lab, network.id, "laptops", "exits", Direction::OneWay);
    let created = create("127.0.0.1:1080", gw).unwrap();
    assert_eq!(created.status, ProxyStatus::NodeOffline);

    // 停用的可以先建着，不查出口。
    let mut later = proxy(laptop, "127.0.0.1:1081", gw);
    later.enabled = false;
    lab.controller.disable_exit(network.id, gw).unwrap();
    let later = lab.controller.create_proxy(network.id, later).unwrap();
    assert_eq!(later.status, ProxyStatus::Disabled);
    // 已经启用着、出口没换的只改名字不查；要启用的查。
    let renamed = UpdateProxy { name: Some("renamed".into()), ..UpdateProxy::default() };
    assert_eq!(lab.controller.update_proxy(network.id, created.id, renamed).unwrap().name, "renamed");
    let enable = UpdateProxy { enabled: Some(true), ..UpdateProxy::default() };
    assert_eq!(lab.controller.update_proxy(network.id, later.id, enable).unwrap_err().code(), ErrorCode::ExitNotEnabled);
}

#[test]
fn names_and_credentials_are_filled_in_when_left_out() {
    let mut lab = Lab::new();
    let network = lab.network("home");
    let laptop = lab.join_offline(network.id, &[], Declared::default());
    rename(&lab, network.id, laptop, "laptop");
    let gw = lab.join_offline(network.id, &[], CAN_DO_ALL);
    lab.controller.enable_exit(network.id, gw, ExitRole::default()).unwrap();

    let first = lab.controller.create_proxy(network.id, proxy(laptop, "127.0.0.1:1080", gw)).unwrap();
    let second = lab.controller.create_proxy(network.id, proxy(laptop, "[::1]:1080", gw)).unwrap();
    assert_eq!((first.name.as_str(), second.name.as_str()), ("laptop-1080", "laptop-1080-2"));
    assert_eq!((first.username.as_str(), first.password.as_str()), ("", ""), "回环上不写就不认证");

    let mut given = proxy(laptop, "0.0.0.0:3128", gw);
    (given.username, given.password) = ("alice".into(), "s3cret".into());
    let given = lab.controller.create_proxy(network.id, given).unwrap();
    assert_eq!((given.username.as_str(), given.password.as_str()), ("alice", "s3cret"), "写了的原样存着");
    let mut colon = proxy(laptop, "0.0.0.0:3129", gw);
    (colon.username, colon.password) = ("a:b".into(), "p".into());
    let refused = lab.controller.create_proxy(network.id, colon).unwrap_err();
    assert_eq!(described(&refused), (ErrorCode::InvalidRequest, Some("username".to_owned())));

    // 两个都清掉：不在回环上的重新生成一对；搬到别的地址上的也一样。
    let cleared = UpdateProxy { username: Some(String::new()), password: Some(String::new()), ..UpdateProxy::default() };
    let regenerated = lab.controller.update_proxy(network.id, given.id, cleared).unwrap();
    assert!(regenerated.username.starts_with("u-") && regenerated.password.len() == 32);
    let moved = UpdateProxy { bind: Some("0.0.0.0:1080".parse().unwrap()), ..UpdateProxy::default() };
    let moved = lab.controller.update_proxy(network.id, first.id, moved).unwrap();
    assert!(!moved.username.is_empty() && !moved.password.is_empty(), "离开回环就要认证");
    let half = UpdateProxy { password: Some(String::new()), ..UpdateProxy::default() };
    let refused = lab.controller.update_proxy(network.id, moved.id, half).unwrap_err();
    assert_eq!(described(&refused), (ErrorCode::CredentialsIncomplete, Some("password".to_owned())));
}

#[test]
fn a_proxy_whose_exit_node_is_deleted_waits_without_an_exit_until_one_is_chosen() {
    let mut lab = Lab::new();
    let network = lab.network("home");
    let laptop = lab.join_offline(network.id, &[], Declared::default());
    let old = lab.join_offline(network.id, &[], CAN_DO_ALL);
    let new = lab.join_offline(network.id, &[], CAN_DO_ALL);
    for exit in [old, new] {
        lab.controller.enable_exit(network.id, exit, ExitRole::default()).unwrap();
    }
    let created = lab.controller.create_proxy(network.id, proxy(laptop, "127.0.0.1:1080", old)).unwrap();
    assert_eq!(lab.netmap(laptop).me.proxies.len(), 1);
    lab.controller.delete_node(network.id, old).unwrap();

    let left = lab.controller.proxy(network.id, created.id).unwrap();
    assert_eq!((left.exit, left.enabled, left.status), (None, false, ProxyStatus::ExitMissing));
    assert!(lab.netmap(laptop).me.proxies.is_empty());
    let enable = UpdateProxy { enabled: Some(true), ..UpdateProxy::default() };
    let refused = lab.controller.update_proxy(network.id, created.id, enable.clone()).unwrap_err();
    assert_eq!((refused.code(), refused.what().as_deref(), refused.field()), (ErrorCode::InvalidRequest, Some("required"), Some("exit")));
    let chosen = UpdateProxy { exit: Some(new), ..enable };
    let chosen = lab.controller.update_proxy(network.id, created.id, chosen).unwrap();
    assert_eq!((chosen.exit, chosen.enabled, chosen.status), (Some(new), true, ProxyStatus::NodeOffline));
    assert_eq!(lab.netmap(laptop).me.proxies[0].exit, new);
}
