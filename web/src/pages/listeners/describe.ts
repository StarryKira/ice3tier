import { type DefaultAction, type Listener, type Node } from '@/api/types'
import { nodeName } from '@/lib/nodes'
import { t } from '@/i18n'
import { isLoopbackBind } from '@/lib/cidr'

/** 一个监听器实际生效的默认动作：没有设置时，回环地址是就地出站，其他地址是拒绝。 */
export function effectiveDefaultAction(listener: Pick<Listener, 'bind' | 'default_action'>): DefaultAction {
  return listener.default_action ?? (isLoopbackBind(listener.bind) ? { action: 'local' } : { action: 'reject' })
}

/** "谁能用"这一格：一句话。 */
export function describeWho(listener: Listener): string {
  const loopback = isLoopbackBind(listener.bind)
  const who = loopback
    ? listener.require_credential
      ? t.listeners.whoLocalCredential
      : t.listeners.whoLocal
    : listener.require_credential
      ? t.listeners.whoCredential
      : t.listeners.whoAnyone
  if (loopback) return who
  const sources =
    listener.allow_from.length === 0 ? t.listeners.anySource : t.listeners.allowFrom(listener.allow_from.join('、'))
  return `${who} · ${sources}`
}

/** "不写用户名时"这一格。固定了出口的监听器不看用户名，写固定的那个。 */
export function describeNoUser(listener: Listener, nodes: readonly Pick<Node, 'id' | 'name'>[]): string {
  if (listener.fixed_exit !== null) return t.listeners.fixedExit(nodeName(nodes, listener.fixed_exit))
  const action = effectiveDefaultAction(listener)
  switch (action.action) {
    case 'local':
      return t.listeners.actionLocal
    case 'reject':
      return t.listeners.actionReject
    case 'exit':
      return t.listeners.actionExit(nodeName(nodes, action.node))
  }
}

export const PROTOCOL_LABEL = { socks5: t.listeners.socks5, http: t.listeners.http } as const
