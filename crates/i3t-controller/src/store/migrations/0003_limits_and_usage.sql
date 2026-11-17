-- 限额、用量和状态：服务等级的默认值、凭据的流量上限和用量、令牌的使用记录、操作记录里提到的东西。

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

-- 这条记录说的是哪些东西：[{"kind":"node","id":3}]。
ALTER TABLE oplog ADD COLUMN refs TEXT NOT NULL DEFAULT '[]';
