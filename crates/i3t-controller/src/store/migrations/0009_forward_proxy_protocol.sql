-- 端口转发的目标节点连上目标之后先发的 PROXY protocol 头（`"v1"`、`"v2"`）；NULL 是不发。

ALTER TABLE forwards ADD COLUMN proxy_protocol TEXT;
