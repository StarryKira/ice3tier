//! 表结构和它的演进。
//!
//! 数据库文件里的 `user_version` 记着已经执行到第几条迁移。打开时把还没执行的依次执行，每条在自己的事务里。
//! 所以改表结构的办法只有一个：在 [`MIGRATIONS`] 末尾加一条，已经发布过的不能改。
//!
//! 列的写法：枚举和结构体按它们的 JSON 形式存在 TEXT 列里，地址存文本，公钥和哈希存 BLOB，虚拟 IP 存成整数，
//! 时间是 Unix 秒。

pub(super) const MIGRATIONS: &[&str] = &[V1, V2, V3, V4, V5, V6, V7, V8, V9];

const V1: &str = "
CREATE TABLE meta (
    key   TEXT PRIMARY KEY,
    value BLOB NOT NULL
);

CREATE TABLE networks (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    name           TEXT NOT NULL UNIQUE,
    cidr           TEXT NOT NULL,
    routing        TEXT NOT NULL,
    max_relay_hops INTEGER NOT NULL,
    reach          TEXT NOT NULL,
    created_at     INTEGER NOT NULL
);

-- AUTOINCREMENT 保证编号不复用：删掉的节点的编号不会再分给别人。
CREATE TABLE nodes (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    network_id       INTEGER NOT NULL REFERENCES networks(id),
    public_key       BLOB NOT NULL UNIQUE,
    name             TEXT NOT NULL,
    virtual_ip       INTEGER NOT NULL UNIQUE,
    tags             TEXT NOT NULL,
    declared         TEXT NOT NULL,
    relay            TEXT,
    exit             TEXT,
    dial_out_only    INTEGER NOT NULL,
    routing          TEXT,
    reach            TEXT,
    pinned_relays    TEXT NOT NULL,
    endpoints        TEXT NOT NULL,
    observed         TEXT,
    attached         TEXT NOT NULL,
    platform         TEXT NOT NULL,
    version          TEXT NOT NULL,
    created_at       INTEGER NOT NULL,
    last_seen_at     INTEGER,
    netmap_version   INTEGER NOT NULL,
    netmap_digest    BLOB,
    sent_version     INTEGER NOT NULL,
    acked_version    INTEGER NOT NULL,
    enrolled_with    BLOB,
    UNIQUE (network_id, name)
);

CREATE TABLE enroll_tokens (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    network_id   INTEGER NOT NULL REFERENCES networks(id) ON DELETE CASCADE,
    secret_hash  BLOB NOT NULL UNIQUE,
    created_at   INTEGER NOT NULL,
    expires_at   INTEGER NOT NULL,
    tags         TEXT NOT NULL,
    virtual_ip   INTEGER,
    successor_of INTEGER REFERENCES nodes(id) ON DELETE CASCADE
);

CREATE TABLE rules (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    network_id INTEGER NOT NULL REFERENCES networks(id) ON DELETE CASCADE,
    name       TEXT NOT NULL,
    src        TEXT NOT NULL,
    dst        TEXT NOT NULL,
    direction  TEXT NOT NULL,
    enabled    INTEGER NOT NULL
);

CREATE TABLE credentials (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    network_id     INTEGER NOT NULL REFERENCES networks(id) ON DELETE CASCADE,
    name           TEXT NOT NULL,
    token_hash     BLOB NOT NULL,
    max_concurrent INTEGER,
    created_at     INTEGER NOT NULL,
    UNIQUE (network_id, name)
);

-- 默认动作拆成两列：走固定出口时节点编号单独放，删节点时才能用外键和普通的 UPDATE 处理。
CREATE TABLE listeners (
    id                 INTEGER PRIMARY KEY AUTOINCREMENT,
    node_id            INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    name               TEXT NOT NULL,
    bind               TEXT NOT NULL,
    protocol           TEXT NOT NULL,
    allow_from         TEXT NOT NULL,
    require_credential INTEGER NOT NULL,
    default_action     TEXT,
    default_exit       INTEGER REFERENCES nodes(id),
    fixed_exit         INTEGER REFERENCES nodes(id),
    UNIQUE (node_id, name)
);

CREATE TABLE forwards (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    network_id  INTEGER NOT NULL REFERENCES networks(id) ON DELETE CASCADE,
    name        TEXT NOT NULL,
    protocol    TEXT NOT NULL,
    listen_node INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    bind        TEXT NOT NULL,
    target_node INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    target_addr TEXT NOT NULL,
    allow_from  TEXT NOT NULL,
    enabled     INTEGER NOT NULL
);

CREATE TABLE pair_settings (
    node_a     INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    node_b     INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    keep_alive INTEGER NOT NULL,
    PRIMARY KEY (node_a, node_b),
    CHECK (node_a < node_b)
);

-- 删掉的节点留下的虚拟 IP 和名字，到期之前不分给别的节点。
CREATE TABLE cooldowns (
    network_id INTEGER NOT NULL REFERENCES networks(id) ON DELETE CASCADE,
    kind       TEXT NOT NULL,
    value      TEXT NOT NULL,
    until      INTEGER NOT NULL,
    PRIMARY KEY (network_id, kind, value)
);

CREATE TABLE oplog (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    at         INTEGER NOT NULL,
    network_id INTEGER,
    action     TEXT NOT NULL,
    detail     TEXT NOT NULL
);

CREATE INDEX nodes_by_network ON nodes(network_id);
CREATE INDEX listeners_by_node ON listeners(node_id);
";

/// 节点更新（白皮书 §10.5）要记在节点上的东西，和节点上报的主机名。
const V2: &str = "
ALTER TABLE nodes ADD COLUMN hostname TEXT NOT NULL DEFAULT '';
-- 节点自己报的能不能自己更新（JSON）。空值表示它没有报过：版本太旧，不懂更新。
ALTER TABLE nodes ADD COLUMN self_update TEXT;
-- 最近一次归因于版本的更新失败（JSON）。有它在，自动更新不再让这个节点装同一个版本。
ALTER TABLE nodes ADD COLUMN update_failure TEXT;
";

/// 限额、用量和状态：服务等级的默认值、凭据的流量上限和用量、令牌的使用记录、操作记录里提到的东西。
const V3: &str = "
ALTER TABLE networks ADD COLUMN class TEXT;
ALTER TABLE listeners ADD COLUMN class TEXT;
ALTER TABLE listeners ADD COLUMN optimistic INTEGER;
ALTER TABLE forwards ADD COLUMN class TEXT;

-- 令牌开头的一小段（不是秘密），给人认凭据用。这一列出现之前签发的凭据是空的。
ALTER TABLE credentials ADD COLUMN prefix TEXT NOT NULL DEFAULT '';
ALTER TABLE credentials ADD COLUMN max_bytes INTEGER;
-- 签发或者上次清零以来各个入口报上来的字节数之和。
ALTER TABLE credentials ADD COLUMN used_bytes INTEGER NOT NULL DEFAULT 0;
ALTER TABLE credentials ADD COLUMN last_used_at INTEGER;
-- 吊销之后行留着，看得到它用过多少；入口不再认它。
ALTER TABLE credentials ADD COLUMN revoked_at INTEGER;

-- 每个入口上每个凭据最近一次报的累计字节数，和那是它哪一次运行报的。controller 靠它把累计的数换成增量。
CREATE TABLE credential_counters (
    credential_id INTEGER NOT NULL REFERENCES credentials(id) ON DELETE CASCADE,
    node_id       INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    run           INTEGER NOT NULL,
    bytes         INTEGER NOT NULL,
    PRIMARY KEY (credential_id, node_id)
);

-- 用掉的令牌不再删掉：留着谁用的、什么时候用的。
ALTER TABLE enroll_tokens ADD COLUMN prefix TEXT NOT NULL DEFAULT '';
ALTER TABLE enroll_tokens ADD COLUMN used_at INTEGER;
ALTER TABLE enroll_tokens ADD COLUMN used_by INTEGER REFERENCES nodes(id) ON DELETE SET NULL;

-- 这条记录说的是哪些东西：[{\"kind\":\"node\",\"id\":3}]。
ALTER TABLE oplog ADD COLUMN refs TEXT NOT NULL DEFAULT '[]';
";

/// 节点对的独占 TCP（lane）设置：`off`、`udp_fallback`、`always`。
const V4: &str = "
ALTER TABLE pair_settings ADD COLUMN lane TEXT NOT NULL DEFAULT 'off';
";

/// 主动测试的结果（白皮书 §7.6）：每个测试一行，整个结果按 JSON 存，节点删掉时它的测试跟着删。
const V5: &str = "
CREATE TABLE tests (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    network_id INTEGER NOT NULL REFERENCES networks(id) ON DELETE CASCADE,
    node_a     INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    node_b     INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    started_at INTEGER NOT NULL,
    view       TEXT NOT NULL
);
CREATE INDEX tests_by_pair ON tests(network_id, node_a, node_b);
";

/// 操作记录的参数（`LogEntry::params`）：按名字给出，只用操作和它们就能把一条记录写成一句话。旧的记录是空的。
const V6: &str = "
ALTER TABLE oplog ADD COLUMN params TEXT NOT NULL DEFAULT '{}';
";

/// 分组取代标签：分组是单独的东西，有自己的编号；节点和注册令牌的成员关系各是一张表，规则记两头分组的编号。
///
/// 已有的数据原地迁过来：节点、注册令牌、规则里出现过的每个标签成为同名的分组（标签本来就是小写的 ASCII），
/// 每个网络的内置分组 `All` 在最前面建，原来的 `all` 就是它。旧的操作记录的参数改成新的写法，原来的标签名成了 `…_name`。
///
/// 规则表要重建（两列换了类型和外键）。它的编号照旧不复用：新表接着旧表用到的序号数。
const V7: &str = "
CREATE TABLE groups (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    network_id INTEGER NOT NULL REFERENCES networks(id) ON DELETE CASCADE,
    name       TEXT NOT NULL,
    -- 比较用的写法（小写）：名字在一个网络里不分大小写唯一。
    folded     TEXT NOT NULL,
    builtin    INTEGER NOT NULL DEFAULT 0,
    UNIQUE (network_id, folded)
);

-- 内置分组 All 的成员是全部节点，不在这里。
CREATE TABLE group_members (
    group_id INTEGER NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
    node_id  INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    PRIMARY KEY (group_id, node_id)
);
CREATE INDEX group_members_by_node ON group_members(node_id);

CREATE TABLE token_groups (
    token_id INTEGER NOT NULL REFERENCES enroll_tokens(id) ON DELETE CASCADE,
    group_id INTEGER NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
    PRIMARY KEY (token_id, group_id)
);

INSERT INTO groups (network_id, name, folded, builtin) SELECT id, 'All', 'all', 1 FROM networks ORDER BY id;

INSERT INTO groups (network_id, name, folded)
SELECT network_id, min(tag), lower(tag) FROM (
    SELECT nodes.network_id, tag.value AS tag FROM nodes, json_each(nodes.tags) AS tag
    UNION SELECT enroll_tokens.network_id, tag.value FROM enroll_tokens, json_each(enroll_tokens.tags) AS tag
    UNION SELECT network_id, src FROM rules
    UNION SELECT network_id, dst FROM rules
)
WHERE lower(tag) <> 'all'
GROUP BY network_id, lower(tag)
ORDER BY network_id, lower(tag);

INSERT OR IGNORE INTO group_members (group_id, node_id)
SELECT groups.id, nodes.id
FROM nodes, json_each(nodes.tags) AS tag
JOIN groups ON groups.network_id = nodes.network_id AND groups.folded = lower(tag.value) AND groups.builtin = 0;

INSERT OR IGNORE INTO token_groups (token_id, group_id)
SELECT enroll_tokens.id, groups.id
FROM enroll_tokens, json_each(enroll_tokens.tags) AS tag
JOIN groups ON groups.network_id = enroll_tokens.network_id AND groups.folded = lower(tag.value) AND groups.builtin = 0;

CREATE TABLE rules_v7 (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    network_id INTEGER NOT NULL REFERENCES networks(id) ON DELETE CASCADE,
    name       TEXT NOT NULL,
    src        INTEGER NOT NULL REFERENCES groups(id),
    dst        INTEGER NOT NULL REFERENCES groups(id),
    direction  TEXT NOT NULL,
    enabled    INTEGER NOT NULL
);
INSERT INTO sqlite_sequence (name, seq) SELECT 'rules_v7', seq FROM sqlite_sequence WHERE name = 'rules';
INSERT INTO rules_v7 (id, network_id, name, src, dst, direction, enabled)
SELECT rules.id, rules.network_id, rules.name, src.id, dst.id, rules.direction, rules.enabled
FROM rules
JOIN groups AS src ON src.network_id = rules.network_id AND src.folded = lower(rules.src)
JOIN groups AS dst ON dst.network_id = rules.network_id AND dst.folded = lower(rules.dst);
DROP TABLE rules;
ALTER TABLE rules_v7 RENAME TO rules;

UPDATE oplog SET params = json_set(params,
    '$.src', (SELECT groups.id FROM groups
              WHERE groups.network_id = oplog.network_id AND groups.folded = lower(json_extract(oplog.params, '$.src'))),
    '$.src_name', CASE lower(json_extract(params, '$.src')) WHEN 'all' THEN 'All' ELSE json_extract(params, '$.src') END,
    '$.dst', (SELECT groups.id FROM groups
              WHERE groups.network_id = oplog.network_id AND groups.folded = lower(json_extract(oplog.params, '$.dst'))),
    '$.dst_name', CASE lower(json_extract(params, '$.dst')) WHEN 'all' THEN 'All' ELSE json_extract(params, '$.dst') END)
WHERE action IN ('rule.create', 'rule.update', 'rule.delete') AND json_type(params, '$.src') = 'text';

UPDATE oplog SET params = json_set(json_remove(params, '$.tags'), '$.groups', json((
    SELECT json_group_array(json_object('group', groups.id, 'group_name', tag.value))
    FROM json_each(oplog.params, '$.tags') AS tag
    LEFT JOIN groups ON groups.network_id = oplog.network_id AND groups.folded = lower(tag.value))))
WHERE action = 'token.create' AND json_type(params, '$.tags') = 'array';

UPDATE oplog SET params = json_set(json_remove(params, '$.changes.tags'), '$.changes.groups', json((
    SELECT json_group_array(json_object('group', groups.id, 'group_name', tag.value))
    FROM json_each(oplog.params, '$.changes.tags') AS tag
    LEFT JOIN groups ON groups.network_id = oplog.network_id AND groups.folded = lower(tag.value))))
WHERE action = 'node.update' AND json_type(params, '$.changes.tags') = 'array';

ALTER TABLE nodes DROP COLUMN tags;
ALTER TABLE enroll_tokens DROP COLUMN tags;
";

/// 管理员给节点选的性能档位（JSON 写法的字符串）；没有选时是 NULL。
const V8: &str = "
ALTER TABLE nodes ADD COLUMN profile TEXT;
";

// 端口转发的目标节点连上目标之后先发的 PROXY protocol 头（`"v1"`、`"v2"`）；NULL 是不发。
const V9: &str = "
ALTER TABLE forwards ADD COLUMN proxy_protocol TEXT;
";

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
}
