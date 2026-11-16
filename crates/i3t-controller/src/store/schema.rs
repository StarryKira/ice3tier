//! 表结构和它的演进。
//!
//! 数据库文件里的 `user_version` 记着已经执行到第几条迁移。打开时把还没执行的依次执行，每条在自己的事务里。
//! 所以改表结构的办法只有一个：在 [`MIGRATIONS`] 末尾加一条，已经发布过的不能改。
//!
//! 列的写法：枚举和结构体按它们的 JSON 形式存在 TEXT 列里，地址存文本，公钥和哈希存 BLOB，虚拟 IP 存成整数，
//! 时间是 Unix 秒。

pub(super) const MIGRATIONS: &[&str] = &[V1, V2, V3];

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
