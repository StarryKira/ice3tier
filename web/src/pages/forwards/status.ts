import { type Forward, type ForwardStatus, type Node } from '@/api/types'
import { nodeName } from '@/lib/nodes'
import { type Tone } from '@/components/status'
import { t } from '@/i18n'

export interface ForwardStatusView {
  tone: Tone
  /** 短的：圆点旁边的那几个字。 */
  label: string
  /** 原因和下一步。 */
  detail: string
  /** 这种状态要用户去做点什么。 */
  needsAttention: boolean
  /** 去访问规则页能解决。 */
  linkToRules: boolean
}

const s = t.forwards.status

/**
 * 一条转发的状态怎么写：状态、原因、下一步。原因里带具体的节点名。
 * 状态按 controller 知道的算；"端口被占用""目标连不上"节点还不上报，所以这里没有。
 */
export function forwardStatusView(forward: Forward, nodes: readonly Pick<Node, 'id' | 'name'>[]): ForwardStatusView {
  const listen = nodeName(nodes, forward.listen_node)
  const target = nodeName(nodes, forward.target_node)
  const status: ForwardStatus = forward.status
  switch (status) {
    case 'active':
      return { tone: 'ok', label: s.active, detail: s.activeDetail, needsAttention: false, linkToRules: false }
    case 'disabled':
      return { tone: 'idle', label: s.disabled, detail: s.disabledDetail, needsAttention: false, linkToRules: false }
    case 'target_not_declared':
      return {
        tone: 'warn',
        label: s.targetNotDeclared,
        detail: s.targetNotDeclaredDetail(target),
        needsAttention: true,
        linkToRules: false,
      }
    case 'access_denied':
      return {
        tone: 'danger',
        label: s.accessDenied,
        detail: s.accessDeniedDetail(listen, target),
        needsAttention: true,
        linkToRules: true,
      }
    case 'listener_offline':
      return {
        tone: 'idle',
        label: s.listenerOffline,
        detail: s.listenerOfflineDetail(listen),
        needsAttention: false,
        linkToRules: false,
      }
    case 'target_offline':
      return {
        tone: 'idle',
        label: s.targetOffline,
        detail: s.targetOfflineDetail(target),
        needsAttention: false,
        linkToRules: false,
      }
  }
}

/** 转发在文字里的叫法：有名字用名字，没有就写监听地址。 */
export function forwardLabel(forward: Pick<Forward, 'name' | 'bind'>): string {
  return forward.name.trim() === '' ? forward.bind : forward.name
}
