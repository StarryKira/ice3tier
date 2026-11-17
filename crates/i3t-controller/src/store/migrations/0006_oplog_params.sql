-- 操作记录的参数（`LogEntry::params`）：按名字给出，只用操作和它们就能把一条记录写成一句话。旧的记录是空的。

ALTER TABLE oplog ADD COLUMN params TEXT NOT NULL DEFAULT '{}';
