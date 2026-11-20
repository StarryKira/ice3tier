-- relay 的地址不再由人填：去掉存着的 UDP、TCP 地址和区域，只留带宽上限。节点报的网卡地址另存一列
-- （JSON 数组；NULL 是节点没有报过），relay 的地址和地址段重叠的提醒都从它算。
--
-- 给界面画图的时间序列：每个点一行。`series` 是序列的编号（`store::stats`），`node_id` 是 0 时是网络的序列。
-- 不对 `nodes` 设外键：删掉的节点的点留到过了保留期自己清掉，节点编号不复用，不会被认成别的节点。

UPDATE nodes SET relay = json_object('bandwidth_mbps', json_extract(relay, '$.bandwidth_mbps')) WHERE relay IS NOT NULL;
ALTER TABLE nodes ADD COLUMN local_addresses TEXT;

CREATE TABLE stats (
    network_id INTEGER NOT NULL REFERENCES networks(id) ON DELETE CASCADE,
    series     INTEGER NOT NULL,
    node_id    INTEGER NOT NULL,
    step       INTEGER NOT NULL,
    at         INTEGER NOT NULL,
    value      REAL NOT NULL,
    PRIMARY KEY (network_id, series, node_id, step, at)
) WITHOUT ROWID;
