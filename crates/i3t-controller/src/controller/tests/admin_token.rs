//! 重新生成管理令牌（`i3t-controller --reset-admin-token`）：只要数据目录，不要对外地址，也不在写错的目录里建出新的 controller。

use std::fs;
use std::path::Path;

use i3t_model::admin::{CreateNetwork, action};

use crate::controller::{Controller, DATABASE_FILE, KEY_FILE};
use crate::error::Error;

const ADDR: &str = "ctl.example.com:20331";

fn listing(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir).unwrap().map(|entry| entry.unwrap().file_name().into_string().unwrap()).collect();
    names.sort();
    names
}

#[test]
fn a_new_admin_token_needs_only_the_data_dir_and_leaves_everything_else_as_it_was() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let opened = Controller::open(&data, ADDR.to_owned()).unwrap();
    let old = opened.admin_token.unwrap();
    let controller_key = opened.controller.public_key();
    let home = opened.controller.create_network(&CreateNetwork { name: "home".to_owned(), ..CreateNetwork::default() }).unwrap();
    drop(opened.controller);

    let fresh = Controller::reset_admin_token(&data).unwrap();
    assert_ne!(fresh, old);

    let reopened = Controller::open(&data, "other.example.com:1".to_owned()).unwrap();
    assert!(reopened.admin_token.is_none(), "令牌已经有了，打开时不再生成");
    let controller = reopened.controller;
    assert!(controller.is_admin_token(&fresh), "新的令牌能用");
    assert!(!controller.is_admin_token(&old), "原来的作废");
    assert_eq!(controller.public_key(), controller_key, "密钥没动");
    assert_eq!(controller.networks().unwrap().iter().map(|network| network.id).collect::<Vec<_>>(), [home.id]);
    let latest = &controller.oplog(1).unwrap()[0];
    assert_eq!(latest.action, action::ADMIN_TOKEN, "记了一条操作");
    assert!(!latest.detail.contains(&fresh) && !latest.params.values().any(|value| value.to_string().contains(&fresh)), "记录里没有令牌");
}

#[test]
fn a_place_that_is_not_a_data_dir_gets_no_admin_token_and_nothing_is_created_there() {
    let dir = tempfile::tempdir().unwrap();

    let missing = dir.path().join("typo");
    let refused = Controller::reset_admin_token(&missing).unwrap_err();
    assert!(matches!(&refused, Error::NotDataDir { dir, missing: KEY_FILE } if *dir == missing), "{refused:?}");
    assert!(!missing.exists(), "写错的目录不建出来");

    let empty = dir.path().join("empty");
    fs::create_dir(&empty).unwrap();
    let refused = Controller::reset_admin_token(&empty).unwrap_err();
    assert!(matches!(refused, Error::NotDataDir { missing: KEY_FILE, .. }), "{refused:?}");
    assert!(listing(&empty).is_empty(), "不生成密钥，也不建数据库");

    // 只有密钥、没有数据库：也不在这里建一个空数据库。
    let data = dir.path().join("data");
    drop(Controller::open(&data, ADDR.to_owned()).unwrap());
    let half = dir.path().join("half");
    fs::create_dir(&half).unwrap();
    fs::copy(data.join(KEY_FILE), half.join(KEY_FILE)).unwrap();
    let refused = Controller::reset_admin_token(&half).unwrap_err();
    assert!(matches!(refused, Error::NotDataDir { missing: DATABASE_FILE, .. }), "{refused:?}");
    assert_eq!(listing(&half), [KEY_FILE]);
}

#[test]
fn an_unreadable_key_or_database_is_refused_and_the_token_stays() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let opened = Controller::open(&data, ADDR.to_owned()).unwrap();
    let old = opened.admin_token.unwrap();
    drop(opened.controller);

    // 密钥读不懂的 controller 启动不了：不换令牌，现在就说。
    let key = fs::read(data.join(KEY_FILE)).unwrap();
    fs::write(data.join(KEY_FILE), "这不是密钥").unwrap();
    let refused = Controller::reset_admin_token(&data).unwrap_err();
    assert!(matches!(&refused, Error::Key { path, .. } if *path == data.join(KEY_FILE)), "{refused:?}");
    fs::write(data.join(KEY_FILE), key).unwrap();
    let reopened = Controller::open(&data, ADDR.to_owned()).unwrap();
    assert!(reopened.controller.is_admin_token(&old), "原来的令牌还能用");
    drop(reopened);

    fs::write(data.join(DATABASE_FILE), "这不是数据库").unwrap();
    assert!(matches!(Controller::reset_admin_token(&data), Err(Error::Store(_))));
    assert_eq!(fs::read_to_string(data.join(DATABASE_FILE)).unwrap(), "这不是数据库");
}

#[cfg(unix)]
#[test]
fn a_new_admin_token_keeps_the_database_readable_only_by_its_owner() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    drop(Controller::open(&data, ADDR.to_owned()).unwrap());
    // 早先的版本建的数据库谁都可读，和打开时一样收紧。
    fs::set_permissions(data.join(DATABASE_FILE), fs::Permissions::from_mode(0o644)).unwrap();

    Controller::reset_admin_token(&data).unwrap();
    assert_eq!(fs::metadata(data.join(DATABASE_FILE)).unwrap().permissions().mode() & 0o777, 0o600);
}
