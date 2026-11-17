-- 自动更新可以按节点关掉：全局的开关和节点自己的都开着，controller 才自动让它更新。
-- 以前只有全局的开关，所以每个已有的节点都是开着的；新节点也是。
ALTER TABLE nodes ADD COLUMN auto_update INTEGER NOT NULL DEFAULT 1;
