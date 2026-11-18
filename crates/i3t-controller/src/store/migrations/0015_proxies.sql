-- 代理取代监听器和接入凭据：一个代理是"在节点 A 上监听，经出口节点 B 出去，认一对用户名和密码"。
--
-- 监听器原样搬过来，编号不变（操作记录里提到的监听器因此对得上代理）：
-- - 固定了出口的，出口就是它；没有固定、默认动作是"走某个出口"的，出口是那一个；都没有的没有出口，停用，
--   选好出口之前启用不了。出口是监听节点自己的同样当作没有出口：现在不支持从监听的节点自己出去。
-- - 要求凭据的（不在回环地址上的一定要求）生成一对用户名和密码，不会因为凭据没了就变成谁都能用；
--   回环上不要求的用户名和密码是空的。用户名是 u- 加 8 个十六进制字符，密码是 32 个十六进制字符。
-- - 盐每个代理一份，给节点算校验值用，改密码时换。
-- 凭据、它们的用量记录跟着去掉。
CREATE TABLE proxies (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    node_id    INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    name       TEXT NOT NULL,
    bind       TEXT NOT NULL,
    protocol   TEXT NOT NULL,
    exit_id    INTEGER REFERENCES nodes(id),
    username   TEXT NOT NULL,
    password   TEXT NOT NULL,
    salt       BLOB NOT NULL,
    enabled    INTEGER NOT NULL,
    allow_from TEXT NOT NULL,
    class      TEXT,
    optimistic INTEGER,
    UNIQUE (node_id, name)
);

INSERT INTO proxies (id, node_id, name, bind, protocol, exit_id, username, password, salt, enabled, allow_from, class, optimistic)
SELECT id, node_id, name, bind, protocol, exit_id,
       CASE WHEN guarded THEN 'u-' || lower(hex(randomblob(4))) ELSE '' END,
       CASE WHEN guarded THEN lower(hex(randomblob(16))) ELSE '' END,
       randomblob(16),
       exit_id IS NOT NULL,
       allow_from, class, optimistic
FROM (
    SELECT listeners.*,
           CASE WHEN chosen = node_id THEN NULL ELSE chosen END AS exit_id,
           require_credential OR NOT (bind LIKE '127.%' OR bind LIKE '[::1]:%') AS guarded
    FROM (SELECT *, COALESCE(fixed_exit, CASE WHEN default_action = 'exit' THEN default_exit END) AS chosen FROM listeners) AS listeners
);

DROP TABLE listeners;
DROP TABLE credential_counters;
DROP TABLE credentials;

-- 操作记录里提到的监听器现在是同一个编号的代理。提到凭据的照旧留着，读出来是不认识的种类。
UPDATE oplog SET refs = replace(refs, '"kind":"listener"', '"kind":"proxy"') WHERE refs LIKE '%"kind":"listener"%';
