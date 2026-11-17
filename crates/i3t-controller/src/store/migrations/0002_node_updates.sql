-- 节点更新（白皮书 §10.5）要记在节点上的东西，和节点上报的主机名。

ALTER TABLE nodes ADD COLUMN hostname TEXT NOT NULL DEFAULT '';
-- 节点自己报的能不能自己更新（JSON）。空值表示它没有报过：版本太旧，不懂更新。
ALTER TABLE nodes ADD COLUMN self_update TEXT;
-- 最近一次归因于版本的更新失败（JSON）。有它在，自动更新不再让这个节点装同一个版本。
ALTER TABLE nodes ADD COLUMN update_failure TEXT;
