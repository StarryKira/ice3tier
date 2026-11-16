import { type ListenStatus, type ListenTraffic } from '@/api/types'
import { type Tone } from '@/components/status'
import { t } from '@/i18n'

import { parseSocketAddr } from './cidr'

const s = t.listen

export interface ListenContext {
  /** 监听节点的名字，写进原因里。 */
  node: string
  /** 配置里写的监听地址；端口可能是 0。 */
  bind: string
  online: boolean
}

export interface ListenView {
  tone: Tone
  /** 圆点旁边的几个字。 */
  label: string
  /** 原因和下一步；正常时是 `null`。 */
  detail: string | null
  /** 这种状态要用户去做点什么。 */
  needsAttention: boolean
  /** 实际在监听的地址；没有在监听时是 `null`。 */
  local: string | null
  /** 配置里端口写的是 0，这是节点挑定的端口。 */
  picked: number | null
  /** 节点报的连接数和流量；没有在监听（或者没有报过）时是 `null`：那时的 0 没有意思。 */
  traffic: ListenTraffic | null
}

function portOf(address: string): number | null {
  return parseSocketAddr(address)?.port ?? null
}

/**
 * 一个监听（代理监听器、端口转发的监听一侧）现在怎么说，按"状态：原因。下一步。"来写，原因里有节点名和端口。
 * 状态来自监听节点每 10 秒一次的上报；没有上报时分"节点离线"和"等它上报"两种，后者是刚下发、还没轮到上报。
 */
export function listenView(status: ListenStatus | null, context: ListenContext): ListenView {
  const { node, bind } = context
  const base = { local: null, picked: null, traffic: null, needsAttention: false }
  if (status === null) {
    return context.online
      ? { ...base, tone: 'idle', label: s.pending, detail: s.pendingDetail(node) }
      : { ...base, tone: 'idle', label: s.offline, detail: s.offlineDetail(node) }
  }
  const traffic: ListenTraffic = {
    connections: status.connections,
    bytes_up: status.bytes_up,
    bytes_down: status.bytes_down,
  }
  const configured = portOf(bind)
  const port = configured === null || configured === 0 ? bind : String(configured)
  switch (status.state) {
    case 'listening': {
      const actual = portOf(status.local)
      return {
        ...base,
        traffic,
        tone: 'ok',
        label: s.listening,
        detail: null,
        local: status.local,
        picked: configured === 0 ? actual : null,
      }
    }
    case 'virtual':
      return { ...base, traffic, tone: 'ok', label: s.virtual, detail: s.virtualDetail }
    case 'port_in_use':
      return {
        ...base,
        tone: 'danger',
        label: s.portInUse,
        detail: s.portInUseDetail(node, port, status.detail),
        needsAttention: true,
      }
    case 'failed':
      return {
        ...base,
        tone: 'danger',
        label: s.failed,
        detail: s.failedDetail(node, bind, status.detail),
        needsAttention: true,
      }
    case 'unsupported':
      return {
        ...base,
        tone: 'danger',
        label: s.unsupported,
        detail: s.unsupportedDetail(node, status.detail),
        needsAttention: true,
      }
    case 'unknown':
      return { ...base, tone: 'idle', label: s.unknown, detail: s.unknownDetail }
  }
}
