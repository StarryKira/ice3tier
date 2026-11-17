-- 节点上报的"连得上哪些 relay"（白皮书 §7.2）：每个节点一列，JSON 数组，每个 relay 最近一次试的结果、时刻和它看到的节点 IP，
-- 最多 32 项（`Report::relays` 的上限）。节点的出口地址和"谁连不上这个 relay"都从它和 `observed` 算，不另存。
ALTER TABLE nodes ADD COLUMN relay_reach TEXT NOT NULL DEFAULT '[]';

-- 节点对的拥塞控制（白皮书 §6.3）：`bbr` 或 `cubic`，NULL 是用节点的默认。
ALTER TABLE pair_settings ADD COLUMN congestion TEXT;

-- 换路的历史（白皮书 §7.7、§13）：节点上报的每一次换路一行（`from_node` 发往 `to_node` 这个方向），路和原因的代码按 JSON 存。
-- 一对节点（两个方向一共）只留最近的 20 条，一个网络一共最多留一万条；节点删掉时跟着删。
CREATE TABLE route_switches (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    network_id INTEGER NOT NULL REFERENCES networks(id) ON DELETE CASCADE,
    from_node  INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    to_node    INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    at         INTEGER NOT NULL,
    previous   TEXT NOT NULL,
    route      TEXT NOT NULL,
    cause      TEXT NOT NULL
);
CREATE INDEX route_switches_by_pair ON route_switches(network_id, from_node, to_node);
