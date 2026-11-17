-- 管理员给节点选的性能档位（JSON 写法的字符串）；没有选时是 NULL。

ALTER TABLE nodes ADD COLUMN profile TEXT;
