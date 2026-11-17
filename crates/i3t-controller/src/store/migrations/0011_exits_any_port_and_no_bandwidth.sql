-- 到 0.1.9 为止，启用出口时不写端口，存下来的是 [80, 443]；现在不写端口就是不限（null），用户以为原来的出口也跟着变了。
-- 端口正好是 80 和 443 这两个（哪个在前都算）的出口改成不限；别的列表照旧，那是人特意写的。
-- `json_extract` 取出的数组是紧凑的写法，所以按文本比就够了。
--
-- 出口不再有带宽上限：去掉存着的 `bandwidth_mbps`。relay 的带宽上限存在 `relay` 列里，不动。

UPDATE nodes SET exit = json_set(exit, '$.ports', json('null')) WHERE json_extract(exit, '$.ports') IN ('[80,443]', '[443,80]');
UPDATE nodes SET exit = json_remove(exit, '$.bandwidth_mbps') WHERE exit IS NOT NULL;
