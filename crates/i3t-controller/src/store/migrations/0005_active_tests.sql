-- 主动测试的结果（白皮书 §7.6）：每个测试一行，整个结果按 JSON 存，节点删掉时它的测试跟着删。

CREATE TABLE tests (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    network_id INTEGER NOT NULL REFERENCES networks(id) ON DELETE CASCADE,
    node_a     INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    node_b     INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    started_at INTEGER NOT NULL,
    view       TEXT NOT NULL
);
CREATE INDEX tests_by_pair ON tests(network_id, node_a, node_b);
