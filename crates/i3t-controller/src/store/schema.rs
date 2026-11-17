//! 表结构和它的演进。
//!
//! 数据库文件里的 `user_version` 记着已经执行到第几条迁移。打开时把还没执行的依次执行，每条在自己的事务里。
//! 所以改表结构的办法只有一个：在 [`MIGRATIONS`] 末尾加一条，已经发布过的不能改。
//!
//! 每条迁移是 `migrations/` 下面的一个 SQL 文件，文件名是四位的序号加一段英文说明（`0007_groups.sql`），
//! 编译时用 `include_str!` 收进来。文件开头的 `--` 注释说明这条迁移为什么这样做。加一条迁移就是加一个序号接着往下排的文件，
//! 再把它加到 [`MIGRATIONS`] 的末尾；测试会核对两边对得上。
//!
//! 列的写法：枚举和结构体按它们的 JSON 形式存在 TEXT 列里，地址存文本，公钥和哈希存 BLOB，虚拟 IP 存成整数，
//! 时间是 Unix 秒。

pub(super) const MIGRATIONS: &[&str] = &[
    include_str!("migrations/0001_initial.sql"),
    include_str!("migrations/0002_node_updates.sql"),
    include_str!("migrations/0003_limits_and_usage.sql"),
    include_str!("migrations/0004_pair_lane.sql"),
    include_str!("migrations/0005_active_tests.sql"),
    include_str!("migrations/0006_oplog_params.sql"),
    include_str!("migrations/0007_groups.sql"),
    include_str!("migrations/0008_node_profile.sql"),
    include_str!("migrations/0009_forward_proxy_protocol.sql"),
    include_str!("migrations/0010_found_relay_addresses_and_stats.sql"),
    include_str!("migrations/0011_exits_any_port_and_no_bandwidth.sql"),
    include_str!("migrations/0012_relay_reach_pair_congestion_route_switches.sql"),
    include_str!("migrations/0013_forward_target_switch.sql"),
    include_str!("migrations/0014_node_auto_update.sql"),
];

#[cfg(test)]
mod tests {
    use rusqlite::Connection;
    use serde_json::{Value, json};

    use super::MIGRATIONS;
    use crate::netmap::NetworkView;
    use crate::store::{Store, StoreError};

    /// 第 7 版之前的表结构：标签是节点、令牌上的 JSON 列表和规则里的名字。
    const BEFORE_GROUPS: usize = 6;

    fn node(conn: &Connection, id: u32, network: u32, name: &str, tags: &str) {
        conn.execute(
            "INSERT INTO nodes (id, network_id, public_key, name, virtual_ip, tags, declared, dial_out_only, pinned_relays, endpoints,
                                attached, platform, version, created_at, netmap_version, sent_version, acked_version)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, '{}', 0, '[]', '[]', '[]', 'linux-x86_64', '0.1.0', 1, 0, 0, 0)",
            rusqlite::params![id, network, vec![u8::try_from(id).unwrap(); 32], name, 0x0a42_0000 + id, tags],
        )
        .unwrap();
    }

    /// 一个旧版本写的数据库：两个网络，节点、令牌、规则上都有标签，`all`，只有规则提到的标签，
    /// 一条已经删掉的规则（它的编号不能再分出去），和几条旧写法的操作记录。
    fn old_database(path: &std::path::Path) {
        let store = Store::prepare(Connection::open(path).unwrap(), &MIGRATIONS[..BEFORE_GROUPS]).unwrap();
        let conn = &store.conn;
        for (id, name) in [(1, "home"), (2, "lab")] {
            conn.execute(
                "INSERT INTO networks (id, name, cidr, routing, max_relay_hops, reach, created_at) VALUES (?1, ?2, '10.66.0.0/16', '\"direct_first\"', 2, '\"any\"', 1)",
                rusqlite::params![id, name],
            )
            .unwrap();
        }
        node(conn, 1, 1, "nas", r#"["home","servers"]"#);
        node(conn, 2, 1, "phone", "[]");
        node(conn, 3, 1, "laptop", r#"["laptops"]"#);
        node(conn, 4, 2, "box", r#"["servers"]"#);
        conn.execute_batch(
            r#"
            INSERT INTO enroll_tokens (id, network_id, secret_hash, created_at, expires_at, tags) VALUES (1, 1, x'01', 1, 9, '["laptops","printers"]');
            INSERT INTO enroll_tokens (id, network_id, secret_hash, created_at, expires_at, tags, used_at, used_by) VALUES (2, 1, x'02', 1, 9, '["servers"]', 5, 1);
            INSERT INTO rules (id, network_id, name, src, dst, direction, enabled) VALUES (1, 1, 'default', 'all', 'all', '"both"', 1);
            INSERT INTO rules (id, network_id, name, src, dst, direction, enabled) VALUES (2, 1, 'ssh', 'laptops', 'servers', '"one_way"', 1);
            INSERT INTO rules (id, network_id, name, src, dst, direction, enabled) VALUES (3, 1, 'later', 'ghost', 'all', '"both"', 0);
            INSERT INTO rules (id, network_id, name, src, dst, direction, enabled) VALUES (4, 2, 'default', 'all', 'all', '"both"', 1);
            INSERT INTO rules (id, network_id, name, src, dst, direction, enabled) VALUES (5, 2, 'gone', 'all', 'all', '"both"', 1);
            DELETE FROM rules WHERE id = 5;
            INSERT INTO oplog (at, network_id, action, detail, params) VALUES
                (1, 1, 'rule.create', 'x', '{"rule":2,"rule_name":"ssh","src":"laptops","dst":"servers","direction":"one_way","enabled":true}'),
                (2, 1, 'rule.delete', 'x', '{"rule":9,"rule_name":"old","src":"vanished","dst":"all","direction":"both","enabled":true}'),
                (3, 1, 'token.create', 'x', '{"token":1,"prefix":"i3t-","expires_at":9,"tags":["laptops","printers"],"virtual_ip":null,"successor_of":null}'),
                (4, 1, 'node.update', 'x', '{"node":1,"node_name":"nas","changes":{"tags":["home","servers"],"dial_out_only":true}}'),
                (5, 1, 'tag.rename', 'x', '{"from":"a","to":"b","nodes":1,"rules":0,"tokens":0}');
            "#,
        )
        .unwrap();
    }

    #[test]
    fn tags_on_nodes_tokens_and_rules_become_groups_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("controller.db");
        old_database(&path);
        let store = Store::open(&path).unwrap();
        assert_eq!(store.schema_version().unwrap(), u32::try_from(MIGRATIONS.len()).unwrap());

        let home = i3t_model::admin::NetworkId(1);
        let lab = i3t_model::admin::NetworkId(2);
        let names = |network| store.groups(network).unwrap().into_iter().map(|group| (group.name, group.builtin)).collect::<Vec<_>>();
        let plain = |name: &str| (name.to_owned(), false);
        assert_eq!(
            names(home),
            [("All".to_owned(), true), plain("ghost"), plain("home"), plain("laptops"), plain("printers"), plain("servers")],
            "节点、令牌、规则里出现过的每个标签，包括只有规则提到的，all 成了内置的 All"
        );
        assert_eq!(names(lab), [("All".to_owned(), true), plain("servers")]);
        let id = |network, name: &str| store.group_named(network, name).unwrap().unwrap().id;
        assert_ne!(id(home, "servers"), id(lab, "servers"), "同名的标签在两个网络里是两个分组");

        let groups_of = |node: u32| {
            let record = store.node(i3t_proto::NodeId::new(node).unwrap()).unwrap().unwrap();
            let mut names: Vec<String> =
                store.groups(record.network).unwrap().into_iter().filter(|g| record.groups.contains(&g.id)).map(|g| g.name).collect();
            names.sort();
            names
        };
        assert_eq!(groups_of(1), ["home", "servers"]);
        assert!(groups_of(2).is_empty());
        assert_eq!(groups_of(4), ["servers"]);

        let tokens = store.tokens(home).unwrap();
        let token_groups = |index: usize| tokens[index].groups.iter().map(|group| group.name.as_str()).collect::<Vec<_>>();
        assert_eq!((token_groups(0), token_groups(1)), (vec!["laptops", "printers"], vec!["servers"]));

        let rules = store.rules(home).unwrap();
        let ends = |index: usize| (rules[index].src.name.as_str(), rules[index].dst.name.as_str());
        assert_eq!((ends(0), ends(1), ends(2)), (("All", "All"), ("laptops", "servers"), ("ghost", "All")));
        assert_eq!(rules[1].src.id, id(home, "laptops"));
        assert_eq!(store.rules(lab).unwrap()[0].src.id, id(lab, "all"));

        // 迁移之后的访问关系和迁移之前一样：laptop 经 ssh 访问 nas。
        let view = NetworkView::load(&store, home).unwrap().unwrap();
        let access = view.access();
        let node = |raw| i3t_proto::NodeId::new(raw).unwrap();
        assert_eq!(access.rules_allowing(node(3), node(1)).map(|rule| rule.id).collect::<Vec<_>>(), [1, 2]);

        let rule = i3t_model::admin::Rule {
            id: 0,
            name: String::new(),
            src: rules[0].src.clone(),
            dst: rules[0].dst.clone(),
            direction: i3t_model::admin::Direction::Both,
            enabled: true,
        };
        assert!(store.insert_rule(home, &rule).unwrap() > 5, "删掉的规则的编号不能再分出去");
        assert!(store.conn.prepare("SELECT tags FROM nodes").is_err(), "旧的列去掉了");
        assert!(store.conn.prepare("SELECT tags FROM enroll_tokens").is_err());

        let params = |action: &str| {
            let entry = store.oplog(10).unwrap().into_iter().find(|entry| entry.action == action).unwrap();
            Value::Object(entry.params)
        };
        let created = params("rule.create");
        assert_eq!(
            (&created["src"], &created["src_name"], &created["dst"], &created["dst_name"]),
            (&json!(id(home, "laptops")), &json!("laptops"), &json!(id(home, "servers")), &json!("servers"))
        );
        let deleted = params("rule.delete");
        assert_eq!((&deleted["src"], &deleted["src_name"], &deleted["dst_name"]), (&Value::Null, &json!("vanished"), &json!("All")));
        assert_eq!(
            params("token.create")["groups"],
            json!([{ "group": id(home, "laptops"), "group_name": "laptops" }, { "group": id(home, "printers"), "group_name": "printers" }])
        );
        assert!(params("token.create").get("tags").is_none());
        let update = params("node.update");
        assert_eq!(
            update["changes"],
            json!({ "dial_out_only": true, "groups": [{ "group": id(home, "home"), "group_name": "home" }, { "group": id(home, "servers"), "group_name": "servers" }] })
        );
        assert_eq!(params("tag.rename")["from"], "a", "没有新写法的旧记录原样留着");
    }

    #[test]
    fn a_backup_of_an_older_data_directory_is_migrated_when_a_controller_starts_on_it() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        std::fs::create_dir(&data).unwrap();
        old_database(&data.join("controller.db"));
        std::fs::write(data.join("controller.key"), i3t_crypto::NodeKey::generate().unwrap().to_pem()).unwrap();

        let backup = crate::Controller::back_up(&data, &dir.path().join("backup")).unwrap();
        let version = |path: &std::path::Path| -> u32 {
            Connection::open(path).unwrap().pragma_query_value(None, "user_version", |row| row.get(0)).unwrap()
        };
        assert_eq!(version(&backup.dir.join("controller.db")), u32::try_from(BEFORE_GROUPS).unwrap(), "备份原样照抄，不迁移");

        let controller = crate::Controller::open(&backup.dir, "ctl.example.com:20331".to_owned()).unwrap().controller;
        assert_eq!(version(&backup.dir.join("controller.db")), u32::try_from(MIGRATIONS.len()).unwrap());
        let home = i3t_model::admin::NetworkId(1);
        let groups: Vec<String> = controller.groups(home).unwrap().into_iter().map(|group| group.name).collect();
        assert_eq!(groups, ["All", "ghost", "home", "laptops", "printers", "servers"], "标签照常迁成了分组");
        assert_eq!(controller.nodes(home).unwrap().len(), 3);
        assert_eq!(version(&data.join("controller.db")), u32::try_from(BEFORE_GROUPS).unwrap(), "原来的数据目录没有动");
    }

    #[test]
    fn a_migration_that_fails_on_broken_data_leaves_the_tags_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("controller.db");
        old_database(&path);
        {
            let store = Store::prepare(Connection::open(&path).unwrap(), &MIGRATIONS[..BEFORE_GROUPS]).unwrap();
            store.conn.execute("UPDATE enroll_tokens SET tags = 'not json' WHERE id = 1", []).unwrap();
        }
        assert!(matches!(Store::open(&path), Err(StoreError::Sqlite(_))));
        let store = Store::prepare(Connection::open(&path).unwrap(), &MIGRATIONS[..BEFORE_GROUPS]).unwrap();
        assert_eq!(store.schema_version().unwrap(), u32::try_from(BEFORE_GROUPS).unwrap());
        let tags: String = store.conn.query_row("SELECT tags FROM nodes WHERE id = 1", [], |row| row.get(0)).unwrap();
        assert_eq!(tags, r#"["home","servers"]"#);
        assert!(store.conn.prepare("SELECT * FROM groups").is_err(), "失败的迁移不留下半张表");
        let src: String = store.conn.query_row("SELECT src FROM rules WHERE id = 2", [], |row| row.get(0)).unwrap();
        assert_eq!(src, "laptops");
    }

    #[test]
    fn every_file_in_the_migrations_directory_is_applied_in_name_order_without_gaps() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/store/migrations");
        let mut names: Vec<String> =
            std::fs::read_dir(&dir).unwrap().map(|entry| entry.unwrap().file_name().into_string().unwrap()).collect();
        names.sort();
        assert!(names.iter().all(|name| name.ends_with(".sql")), "目录里只放迁移：{names:?}");
        assert_eq!(names.len(), MIGRATIONS.len(), "每个文件都要在 MIGRATIONS 里，反过来也一样：{names:?}");
        for (index, name) in names.iter().enumerate() {
            let (number, description) = name.split_once('_').unwrap_or_else(|| panic!("{name} 要写成 序号_说明.sql"));
            assert_eq!(number.len(), 4, "{name} 的序号是四位数");
            assert_eq!(number.parse::<usize>().unwrap(), index + 1, "{name}：序号从 1 起，一个接一个，不能有空缺");
            assert!(description.len() > ".sql".len(), "{name} 要有一段说明");
            let content = std::fs::read_to_string(dir.join(name)).unwrap();
            assert_eq!(MIGRATIONS[index], content, "MIGRATIONS[{index}] 应该是 {name}");
        }
    }

    /// 第 7 版以后的表结构里的一个网络 `home`（编号 1）。
    fn home_network(conn: &Connection) {
        conn.execute(
            "INSERT INTO networks (id, name, cidr, routing, max_relay_hops, reach, created_at) VALUES (1, 'home', '10.66.0.0/16', '\"direct_first\"', 2, '\"any\"', 1)",
            [],
        )
        .unwrap();
    }

    /// 第 7 版以后的表结构里 `home` 的一个节点，没有启用任何能力。
    fn home_node(conn: &Connection, id: u32, name: &str) {
        conn.execute(
            "INSERT INTO nodes (id, network_id, public_key, name, virtual_ip, declared, dial_out_only, pinned_relays, endpoints,
                                attached, platform, version, created_at, netmap_version, sent_version, acked_version)
             VALUES (?1, 1, ?2, ?3, ?4, '{}', 0, '[]', '[]', '[]', 'linux-x86_64', '0.1.0', 1, 0, 0, 0)",
            rusqlite::params![id, vec![u8::try_from(id).unwrap(); 32], name, 0x0a42_0000 + id],
        )
        .unwrap();
    }

    /// 第 10 版之前：relay 的地址和区域是管理员填的。
    const BEFORE_FOUND_ADDRESSES: usize = 9;

    #[test]
    fn typed_relay_addresses_and_the_region_are_dropped_and_the_bandwidth_kept() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("controller.db");
        {
            let store = Store::prepare(Connection::open(&path).unwrap(), &MIGRATIONS[..BEFORE_FOUND_ADDRESSES]).unwrap();
            home_network(&store.conn);
            for (id, name) in [(1, "hk"), (2, "nas")] {
                home_node(&store.conn, id, name);
            }
            let typed = r#"{"udp":["203.0.113.7:20330"],"tcp":["203.0.113.7:20330"],"region":"hk","bandwidth_mbps":200}"#;
            store.conn.execute("UPDATE nodes SET relay = ?1 WHERE id = 1", [typed]).unwrap();
        }
        let store = Store::open(&path).unwrap();
        let raw: String = store.conn.query_row("SELECT relay FROM nodes WHERE id = 1", [], |row| row.get(0)).unwrap();
        assert_eq!(serde_json::from_str::<Value>(&raw).unwrap(), json!({ "bandwidth_mbps": 200 }));
        let hk = store.node(i3t_proto::NodeId::new(1).unwrap()).unwrap().unwrap();
        assert_eq!(hk.relay, Some(i3t_model::admin::RelaySettings { bandwidth_mbps: Some(200) }));
        assert_eq!(hk.local_addresses, None, "迁移之后还没有节点报过网卡地址");
        let nas = store.node(i3t_proto::NodeId::new(2).unwrap()).unwrap().unwrap();
        assert_eq!(nas.relay, None, "不是 relay 的节点照旧不是");
        assert!(store.conn.prepare("SELECT network_id, series, node_id, step, at, value FROM stats").is_ok());
    }

    /// 第 11 版之前：不写端口启用的出口存着 `[80, 443]`，出口有带宽上限。
    const BEFORE_EXITS_ANY_PORT: usize = 10;

    #[test]
    fn exits_stored_with_the_old_default_ports_become_any_port_and_every_exit_loses_its_bandwidth() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("controller.db");
        let exits = [
            (1, "old-default", Some(r#"{"ports":[80,443],"lan":false,"bandwidth_mbps":50,"upstream_socks5":null,"family":"any"}"#)),
            (2, "reversed", Some(r#"{"ports":[443,80],"lan":true,"bandwidth_mbps":10,"quota_bytes":1000,"quota_reset_day":5}"#)),
            (3, "https-only", Some(r#"{"ports":[443],"lan":false,"bandwidth_mbps":20}"#)),
            (4, "three-ports", Some(r#"{"ports":[80,443,8080],"lan":false,"bandwidth_mbps":30}"#)),
            (5, "any-port", Some(r#"{"ports":null,"lan":false,"bandwidth_mbps":40}"#)),
            (6, "relay-only", None),
        ];
        {
            let store = Store::prepare(Connection::open(&path).unwrap(), &MIGRATIONS[..BEFORE_EXITS_ANY_PORT]).unwrap();
            home_network(&store.conn);
            for (id, name, exit) in exits {
                home_node(&store.conn, id, name);
                store.conn.execute("UPDATE nodes SET exit = ?2 WHERE id = ?1", rusqlite::params![id, exit]).unwrap();
            }
            store.conn.execute(r#"UPDATE nodes SET relay = '{"bandwidth_mbps":100}' WHERE id = 6"#, []).unwrap();
        }
        let store = Store::open(&path).unwrap();
        assert_eq!(store.schema_version().unwrap(), u32::try_from(MIGRATIONS.len()).unwrap());
        let stored = |id: u32| -> Option<Value> {
            let raw: Option<String> = store.conn.query_row("SELECT exit FROM nodes WHERE id = ?1", [id], |row| row.get(0)).unwrap();
            raw.map(|raw| serde_json::from_str(&raw).unwrap())
        };
        let ports = |id: u32| stored(id).unwrap()["ports"].clone();
        assert_eq!((ports(1), ports(2)), (Value::Null, Value::Null), "正好是 80 和 443 的，哪个在前都改成不限");
        assert_eq!((ports(3), ports(4)), (json!([443]), json!([80, 443, 8080])), "别的列表是人特意写的，照旧");
        assert_eq!(ports(5), Value::Null);
        for id in 1..=5 {
            assert!(stored(id).unwrap().get("bandwidth_mbps").is_none(), "出口 {id} 的带宽上限去掉了");
        }
        assert_eq!(stored(2).unwrap(), json!({ "ports": null, "lan": true, "quota_bytes": 1000, "quota_reset_day": 5 }), "别的设置不动");

        let node = |id: u32| store.node(i3t_proto::NodeId::new(id).unwrap()).unwrap().unwrap();
        let reversed = node(2).exit.unwrap();
        assert_eq!((reversed.ports, reversed.lan, reversed.quota_bytes, reversed.quota_reset_day), (None, true, Some(1000), 5));
        assert_eq!(node(4).exit.unwrap().ports, Some(vec![80, 443, 8080]));
        let relay_only = node(6);
        assert_eq!(relay_only.exit, None, "不是出口的节点照旧不是");
        assert_eq!(relay_only.relay, Some(i3t_model::admin::RelaySettings { bandwidth_mbps: Some(100) }), "relay 的带宽上限不动");
    }

    /// 第 13 版之前：端口转发的目标由节点自己声明（`declared.forward`）。
    const BEFORE_FORWARD_TARGET_SWITCH: usize = 12;

    #[test]
    fn a_node_that_declared_itself_a_forward_target_gets_the_switch_on_and_the_declaration_goes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("controller.db");
        let declared = [
            (1, "nas", r#"{"exit":false,"relay":false,"forward":true}"#),
            (2, "hk", r#"{"exit":true,"relay":true,"forward":false,"relay_address":"203.0.113.9:40330"}"#),
            (3, "phone", "{}"),
        ];
        {
            let store = Store::prepare(Connection::open(&path).unwrap(), &MIGRATIONS[..BEFORE_FORWARD_TARGET_SWITCH]).unwrap();
            home_network(&store.conn);
            for (id, name, declared) in declared {
                home_node(&store.conn, id, name);
                store.conn.execute("UPDATE nodes SET declared = ?2 WHERE id = ?1", rusqlite::params![id, declared]).unwrap();
            }
        }
        let store = Store::open(&path).unwrap();
        let node = |id: u32| store.node(i3t_proto::NodeId::new(id).unwrap()).unwrap().unwrap();
        assert_eq!((node(1).forward_target, node(2).forward_target, node(3).forward_target), (true, false, false));
        let hk = node(2).declared;
        assert_eq!((hk.exit, hk.relay, hk.relay_address), (true, true, Some("203.0.113.9:40330".parse().unwrap())), "别的声明不动");
        for id in 1..=3 {
            let raw: String = store.conn.query_row("SELECT declared FROM nodes WHERE id = ?1", [id], |row| row.get(0)).unwrap();
            assert!(serde_json::from_str::<Value>(&raw).unwrap().get("forward").is_none(), "节点 {id} 的声明里没有 forward 了");
        }
    }

    /// 第 14 版之前：自动更新只有全局的开关。
    const BEFORE_NODE_AUTO_UPDATE: usize = 13;

    #[test]
    fn every_existing_node_keeps_automatic_updates_on() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("controller.db");
        {
            let store = Store::prepare(Connection::open(&path).unwrap(), &MIGRATIONS[..BEFORE_NODE_AUTO_UPDATE]).unwrap();
            home_network(&store.conn);
            for (id, name) in [(1, "nas"), (2, "hk")] {
                home_node(&store.conn, id, name);
            }
            store.conn.execute("UPDATE nodes SET forward_target = 1 WHERE id = 2", []).unwrap();
        }
        let store = Store::open(&path).unwrap();
        assert_eq!(store.schema_version().unwrap(), u32::try_from(MIGRATIONS.len()).unwrap());
        let node = |id: u32| store.node(i3t_proto::NodeId::new(id).unwrap()).unwrap().unwrap();
        assert!(node(1).auto_update && node(2).auto_update, "以前只有全局的开关，已有的节点都按开着算");
        assert!(node(2).forward_target, "别的开关不动");
    }
}
