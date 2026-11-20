-- ice3proxy 从节点对的设置变成代理的选项（白皮书 §9.7 第 9 条）：选了它的代理的 TCP 流不管路上有没有 UDP 都走 ice3proxy，
-- 没选的代理和端口转发不走。
--
-- 已有的代理：它监听的节点和它的出口这一对存着 `always` 的算"要"；别的都算"不要"：存着 `off`、`udp_fallback`（或者别的值）的，
-- 和没有存下设置的（以前的默认是 `udp_fallback`）。所以以前在没有 UDP 的路上自己会走 ice3proxy 的代理，升级之后要在代理上打开。
--
-- 节点对的 `lane` 去掉。只为它存过的节点对（连接不常驻、拥塞控制用默认）没有别的设置了，一起去掉：留着只是列表里的一行默认值。
ALTER TABLE proxies ADD COLUMN lane INTEGER NOT NULL DEFAULT 0;

UPDATE proxies SET lane = 1
WHERE exit_id IS NOT NULL
  AND EXISTS (
      SELECT 1 FROM pair_settings
      WHERE pair_settings.lane = 'always'
        AND pair_settings.node_a = MIN(proxies.node_id, proxies.exit_id)
        AND pair_settings.node_b = MAX(proxies.node_id, proxies.exit_id)
  );

ALTER TABLE pair_settings DROP COLUMN lane;
DELETE FROM pair_settings WHERE keep_alive = 0 AND congestion IS NULL;
