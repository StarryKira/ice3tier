-- 分组取代标签：分组是单独的东西，有自己的编号；节点和注册令牌的成员关系各是一张表，规则记两头分组的编号。
--
-- 已有的数据原地迁过来：节点、注册令牌、规则里出现过的每个标签成为同名的分组（标签本来就是小写的 ASCII），
-- 每个网络的内置分组 `All` 在最前面建，原来的 `all` 就是它。旧的操作记录的参数改成新的写法，原来的标签名成了 `…_name`。
--
-- 规则表要重建（两列换了类型和外键）。它的编号照旧不复用：新表接着旧表用到的序号数。

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
