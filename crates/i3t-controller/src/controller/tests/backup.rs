//! 备份和恢复：controller 开着时备份，备份里有已经提交的全部东西，在备份上启动的是同一个 controller。

use std::fs;
use std::path::Path;

use i3t_model::admin::{CreateEnrollToken, CreateGroup, CreateNetwork, Direction};
use i3t_model::control::Declared;
use rusqlite::types::Value;
use rusqlite::{Connection, OpenFlags};

use super::super::testing::Lab;
use super::add_rule;
use crate::controller::{Controller, DATABASE_FILE, KEY_FILE};
use crate::error::{Error, Rejection};

const ADDR: &str = "ctl.example.com:20331";

/// 一个数据库文件里的全部内容：表结构的版本号，每个表和索引的定义，每个表的每一行（按 rowid）。
/// 另开一个只读的连接来读，所以看得到别的连接已经提交、还在 `-wal` 里的东西。
fn content(database: &Path) -> Vec<String> {
    let conn = Connection::open_with_flags(database, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let version: u32 = conn.pragma_query_value(None, "user_version", |row| row.get(0)).unwrap();
    let mut out = vec![format!("user_version {version}")];
    let mut statement = conn.prepare("SELECT type, name, sql FROM sqlite_master ORDER BY type, name").unwrap();
    let objects: Vec<(String, String, Option<String>)> =
        statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))).unwrap().map(Result::unwrap).collect();
    for (kind, name, sql) in objects {
        out.push(format!("{kind} {name}: {sql:?}"));
        if kind != "table" {
            continue;
        }
        let mut rows = conn.prepare(&format!("SELECT * FROM \"{name}\" ORDER BY rowid")).unwrap();
        let columns = rows.column_count();
        let rows = rows.query_map([], |row| (0..columns).map(|index| row.get::<_, Value>(index)).collect::<Result<Vec<_>, _>>()).unwrap();
        out.extend(rows.map(|row| format!("  {:?}", row.unwrap())));
    }
    out
}

fn listing(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir).unwrap().map(|entry| entry.unwrap().file_name().into_string().unwrap()).collect();
    names.sort();
    names
}

#[test]
fn a_backup_of_a_running_controller_has_every_commit_and_starts_as_the_same_controller() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let opened = Controller::open(&data, ADDR.to_owned()).unwrap();
    let admin_token = opened.admin_token.unwrap();
    let controller_key = opened.controller.public_key();
    let mut lab = Lab::around(opened.controller);
    let home = lab.network("home").id;
    let office = lab.network("office").id;
    let mut a = lab.join(home, &["laptops"]);
    let mut b = lab.join(home, &["servers"]);
    lab.join_offline(office, &["printers"], Declared::default());
    add_rule(&lab, home, "laptops", "servers", Direction::OneWay);
    let unused = lab.controller.create_token(home, &CreateEnrollToken::default()).unwrap();
    lab.settle(&mut [&mut a, &mut b]);
    let netmap_at_backup = a.netmap().clone();

    let live = content(&data.join(DATABASE_FILE));
    let plain = dir.path().join("plain");
    fs::create_dir(&plain).unwrap();
    fs::copy(data.join(DATABASE_FILE), plain.join(DATABASE_FILE)).unwrap();
    assert_ne!(content(&plain.join(DATABASE_FILE)), live, "前提：controller 开着，最近的提交还在 -wal 里，只拷主文件会漏掉");

    let backup = Controller::back_up(&data, &dir.path().join("backup")).unwrap();
    assert_eq!(backup.dir, dir.path().join("backup"));
    assert_eq!(backup.files.iter().map(|file| file.name).collect::<Vec<_>>(), [DATABASE_FILE, KEY_FILE]);
    for file in &backup.files {
        assert_eq!(file.bytes, fs::metadata(backup.dir.join(file.name)).unwrap().len());
    }
    assert_eq!(listing(&backup.dir), [DATABASE_FILE, KEY_FILE], "缓存的更新包和 -wal、-shm 都不要");
    assert_eq!(content(&backup.dir.join(DATABASE_FILE)), live);
    assert_eq!(fs::read(backup.dir.join(KEY_FILE)).unwrap(), fs::read(data.join(KEY_FILE)).unwrap());

    // 备份之后的改动：一个新加入的节点，一个新的分组。
    let later = lab.join(home, &[]);
    lab.controller.create_group(home, &CreateGroup { name: "later".to_owned() }).unwrap();
    drop(lab);

    let restored = Controller::open(&backup.dir, ADDR.to_owned()).unwrap();
    assert!(restored.admin_token.is_none(), "备份里有管理令牌，恢复时不再生成");
    let mut lab = Lab::around(restored.controller);
    assert_eq!(content(&backup.dir.join(DATABASE_FILE)), live, "在备份上启动不改它的内容");
    assert_eq!(lab.controller.public_key(), controller_key, "节点认的密钥没有变");
    assert!(lab.controller.is_admin_token(&admin_token));

    let a = lab.connect(a.key, Declared::default()).expect("备份时就有的节点照常报到");
    assert_eq!(a.netmap(), &netmap_at_backup);
    assert!(matches!(lab.connect(later.key, Declared::default()), Err(Rejection::UnknownKey)), "备份之后加入的节点不认识");
    let groups: Vec<String> = lab.controller.groups(home).unwrap().into_iter().map(|group| group.name).collect();
    assert_eq!(groups, ["All", "laptops", "servers"]);
    let tokens = lab.controller.tokens(home, true).unwrap();
    assert!(tokens.iter().any(|token| token.id == unused.info.id), "没用过的注册令牌还在");
}

#[test]
fn a_backup_taken_during_writes_holds_whole_transactions_only() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let controller = Controller::open(&data, ADDR.to_owned()).unwrap().controller;
    let network = controller.create_network(&CreateNetwork { name: "home".to_owned(), ..CreateNetwork::default() }).unwrap().id;
    let backups: Vec<_> = std::thread::scope(|scope| {
        let writer = scope.spawn(|| {
            for index in 0..300 {
                controller.create_group(network, &CreateGroup { name: format!("g{index:03}") }).unwrap();
            }
        });
        let backups = (0..5)
            .map(|round| {
                std::thread::sleep(std::time::Duration::from_millis(15));
                Controller::back_up(&data, &dir.path().join(format!("backup-{round}"))).unwrap()
            })
            .collect();
        writer.join().unwrap();
        backups
    });

    let mut seen = Vec::new();
    for backup in backups {
        let conn = Connection::open_with_flags(backup.dir.join(DATABASE_FILE), OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        let check: String = conn.query_row("PRAGMA integrity_check", [], |row| row.get(0)).unwrap();
        assert_eq!(check, "ok");
        let mut statement = conn.prepare("SELECT name FROM groups WHERE builtin = 0 ORDER BY id").unwrap();
        let names: Vec<String> = statement.query_map([], |row| row.get(0)).unwrap().map(Result::unwrap).collect();
        let expected: Vec<String> = (0..names.len()).map(|index| format!("g{index:03}")).collect();
        assert_eq!(names, expected, "建分组是一个一个来的，快照里是从头开始的连续一段");
        let logged: i64 = conn.query_row("SELECT count(*) FROM oplog WHERE action = 'group.create'", [], |row| row.get(0)).unwrap();
        assert_eq!(usize::try_from(logged).unwrap(), names.len(), "分组和它的操作记录在同一个事务里，快照里不会只有一半");
        seen.push(names.len());
    }
    assert!(seen.windows(2).all(|pair| pair[0] <= pair[1]), "后做的备份不比先做的少：{seen:?}");
}

#[test]
fn a_backup_never_writes_into_a_place_that_holds_something() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let opened = Controller::open(&data, ADDR.to_owned()).unwrap();
    opened.controller.create_network(&CreateNetwork { name: "home".to_owned(), ..CreateNetwork::default() }).unwrap();
    drop(opened);
    let before = content(&data.join(DATABASE_FILE));

    let target = dir.path().join("target");
    fs::create_dir(&target).unwrap();
    fs::write(target.join("notes.txt"), "mine").unwrap();
    let refused = Controller::back_up(&data, &target).unwrap_err();
    assert!(matches!(&refused, Error::BackupTargetNotEmpty(path) if *path == target), "{refused:?}");
    assert_eq!(listing(&target), ["notes.txt"]);
    assert_eq!(fs::read_to_string(target.join("notes.txt")).unwrap(), "mine");

    assert!(matches!(Controller::back_up(&data, &data), Err(Error::BackupTargetNotEmpty(_))), "不写进数据目录自己");
    let file = dir.path().join("file");
    fs::write(&file, "x").unwrap();
    assert!(matches!(Controller::back_up(&data, &file), Err(Error::BackupTargetNotEmpty(_))));
    assert_eq!(fs::read_to_string(&file).unwrap(), "x");

    let missing = Controller::back_up(&dir.path().join("nothing"), &dir.path().join("never")).unwrap_err();
    assert!(matches!(missing, Error::NotDataDir { missing: KEY_FILE, .. }), "{missing:?}");
    assert!(!dir.path().join("never").exists(), "来源不对时什么都不建");

    // 已经停下的 controller 的数据目录，写进一个空目录。
    let empty = dir.path().join("empty");
    fs::create_dir(&empty).unwrap();
    Controller::back_up(&data, &empty).unwrap();
    assert_eq!(content(&empty.join(DATABASE_FILE)), before);
    assert_eq!(content(&data.join(DATABASE_FILE)), before, "备份不改原来的数据库");
}

#[test]
fn a_backup_that_fails_halfway_leaves_nothing_behind() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    drop(Controller::open(&data, ADDR.to_owned()).unwrap());
    fs::write(data.join(DATABASE_FILE), "这不是数据库").unwrap();

    let created = dir.path().join("created");
    assert!(matches!(Controller::back_up(&data, &created), Err(Error::Store(_))));
    assert!(!created.exists(), "这次建的目录删掉了");
    let existing = dir.path().join("existing");
    fs::create_dir(&existing).unwrap();
    assert!(Controller::back_up(&data, &existing).is_err());
    assert!(listing(&existing).is_empty(), "原来就有的空目录留着，里面写了一半的文件删掉了");
}

#[cfg(unix)]
#[test]
fn the_key_in_a_backup_is_readable_only_by_its_owner() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    drop(Controller::open(&data, ADDR.to_owned()).unwrap());
    let backup = Controller::back_up(&data, &dir.path().join("backup")).unwrap();
    let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&data.join(KEY_FILE)), 0o600);
    assert_eq!(mode(&backup.dir.join(KEY_FILE)), 0o600);
}
