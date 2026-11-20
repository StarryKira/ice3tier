-- 端口转发的目标改由管理员打开：节点不再声明，controller 只看这个开关。
-- 声明过可以作为端口转发目标的节点开关打开，指向它们的转发照旧生效；声明里的 `forward` 不再有用，去掉。
ALTER TABLE nodes ADD COLUMN forward_target INTEGER NOT NULL DEFAULT 0;
UPDATE nodes SET forward_target = 1 WHERE json_extract(declared, '$.forward') = 1;
UPDATE nodes SET declared = json_remove(declared, '$.forward');
