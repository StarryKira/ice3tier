-- 节点对的独占 TCP（lane）设置：`off`、`udp_fallback`、`always`。

ALTER TABLE pair_settings ADD COLUMN lane TEXT NOT NULL DEFAULT 'off';
