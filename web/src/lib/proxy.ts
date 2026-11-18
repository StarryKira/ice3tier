import { type Listener, type NodeId } from '@/api/types'

/**
 * 一个代理（监听器）从哪个出口出去：固定了出口的就是它；没固定、默认动作是走某个出口的，是那个出口。
 * 别的（就地出站、拒绝、按用户名选）不算哪个出口的代理。
 */
export function exitOf(listener: Pick<Listener, 'fixed_exit' | 'default_action'>): NodeId | null {
  if (listener.fixed_exit !== null) return listener.fixed_exit
  return listener.default_action?.action === 'exit' ? listener.default_action.node : null
}
