//! 表结构和它的演进。
//!
//! 数据库文件里的 `user_version` 记着已经执行到第几条迁移。打开时把还没执行的依次执行，每条在自己的事务里。
//! 所以改表结构的办法只有一个：在 [`MIGRATIONS`] 末尾加一条，已经发布过的不能改。
//!
//! 列的写法：枚举和结构体按它们的 JSON 形式存在 TEXT 列里，地址存文本，公钥和哈希存 BLOB，虚拟 IP 存成整数，
//! 时间是 Unix 秒。

pub(super) const MIGRATIONS: &[&str] = &[V1];

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
