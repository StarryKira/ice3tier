import { type NodeId, type Route } from '@/api/types'

/**
 * 0.1.16 起，"经本节点自己的 relay 到对端"那一段还没量到时，按探测等回答的上限（1.5 秒）定价，
 * 整条路的代价是 1.5 秒多一点。这个数的意思是"还没量"，不是"这么慢"，界面上写成"未测"。
 * 后端在定这个价的地方（`crates/i3t-path/src/control/route.rs`）写明了界面读它。
 * 线路极差时真量出来的这一段也可能到 1.5 秒，那时也会写成"未测"：少见、无害；真要分清，请后端在候选上加一个明确的标记。
 */
export const UNMEASURED_US = 1_500_000

/** 这条路是经 `from` 自己的 relay、代价到了"还没量"的那个价。`costUs` 是代价或延迟，微秒。 */
export function unmeasuredOwnRelay(route: Route | null, from: NodeId, costUs: number | null): boolean {
  return (
    route !== null && route.via === 'relay' && route.relays[0] === from && costUs !== null && costUs >= UNMEASURED_US
  )
}
