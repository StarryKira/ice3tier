-- 节点对的 ice3proxy（lane）设置：`off`、`udp_fallback`、`always`。

ALTER TABLE pair_settings ADD COLUMN lane TEXT NOT NULL DEFAULT 'off';
