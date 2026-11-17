import {
  type LaneBlock,
  type LaneCounts,
  type LaneHandshakeFailures,
  type LaneMode,
  type Node,
  type NodeId,
  type PairLaneSide,
  type PairView,
  type PeerLanes,
} from '@/api/types'
import { t } from '@/i18n'

import { nodeName } from './nodes'

/**
 * ice3proxy（lane）在界面上怎么说。纯函数：节点对页和节点详情用同一套。
 *
 * 起不起作用、为什么，按 controller 给的原因码（`lane_effect.blocked`）用界面自己的话写，带着修好它的那一步；
 * controller 的 `summary` 只在原因码不认识时（比界面新的 controller）原样显示。
 */

const l = t.pairs.lane

/** 下拉里的顺序。 */
export const LANE_MODES = ['off', 'udp_fallback', 'always'] as const satisfies readonly LaneMode[]

/** 比界面旧的 controller 不写 `lane`，按 `off` 算。 */
export function laneMode(pair: Pick<PairView, 'lane'>): LaneMode {
  return pair.lane ?? 'off'
}

export function laneModeLabel(mode: LaneMode): string {
  return l.modes[mode]
}

/** 有节点报了这一对之间的 ice3proxy：设置起作用之后，或者刚关掉、还有流在跑的时候。没有报的就没有数可看。 */
export function lanesReported(pair: PairView): boolean {
  return (pair.lanes?.length ?? 0) > 0
}

type NodeInfo = Pick<Node, 'id' | 'name' | 'online' | 'declared' | 'relay'>

/** 修好一个原因的那一步，指向哪里由页面决定。 */
export type LaneFix =
  | { kind: 'enable_relay'; node: NodeId }
  | { kind: 'tcp_address'; node: NodeId }
  | { kind: 'update'; node: NodeId }
  | { kind: 'access'; from: NodeId; to: NodeId }

export interface LaneReason {
  text: string
  fix: LaneFix | null
  /** 修的那一步的名字；没有能做的事时是 `null`。 */
  fixLabel: string | null
}

export type LaneEffectView =
  | { kind: 'off' }
  | {
      kind: 'effective'
      label: string
      /** 谁拨号；controller 没说时是 `null`。 */
      dialers: string | null
    }
  | { kind: 'blocked'; label: string; reasons: LaneReason[] }

/**
 * 两个节点都没有启用 relay 时，在哪一个上启用：已经声明了可以当 relay 的优先（启用就行，不用先去那台机器上声明），
 * 其次在线的；都一样时是节点 A。
 */
export function relayCandidate(a: NodeId, b: NodeId, nodes: readonly NodeInfo[]): NodeId {
  const score = (id: NodeId) => {
    const node = nodes.find((candidate) => candidate.id === id)
    return node === undefined ? 0 : (node.declared.relay ? 2 : 0) + (node.online ? 1 : 0)
  }
  return score(b) > score(a) ? b : a
}

function reasonOf(block: LaneBlock, pair: PairView, nodes: readonly NodeInfo[]): LaneReason | null {
  const name = (id: NodeId) => nodeName(nodes, id)
  // 比界面新的 controller 可能报出这里不认识的原因：交给调用方用 controller 的原话。
  const reason: string = block.reason
  switch (reason) {
    case 'no_relay': {
      const node = relayCandidate(pair.a, pair.b, nodes)
      return {
        text: l.reason.noRelay(name(pair.a), name(pair.b)),
        fix: { kind: 'enable_relay', node },
        fixLabel: l.fix.enableRelay(name(node)),
      }
    }
    case 'no_tcp_address': {
      const { node } = block as Extract<LaneBlock, { reason: 'no_tcp_address' }>
      return {
        text: l.reason.noTcpAddress(name(node)),
        fix: { kind: 'tcp_address', node },
        fixLabel: l.fix.tcpAddress(name(node)),
      }
    }
    case 'node_too_old': {
      const { node } = block as Extract<LaneBlock, { reason: 'node_too_old' }>
      return {
        text: l.reason.nodeTooOld(name(node)),
        fix: { kind: 'update', node },
        fixLabel: l.fix.update(name(node)),
      }
    }
    case 'not_connected':
      return {
        text: l.reason.notConnected(name(pair.a), name(pair.b)),
        fix: { kind: 'access', from: pair.a, to: pair.b },
        fixLabel: l.fix.access,
      }
    default:
      return null
  }
}

/** 设置起不起作用。设置是 `off`、或者 controller 太旧没有说时是 `off`。 */
export function laneEffectView(pair: PairView, nodes: readonly NodeInfo[]): LaneEffectView {
  const effect = pair.lane_effect
  if (laneMode(pair) === 'off' || effect == null) return { kind: 'off' }
  if (effect.effective) {
    const dialers =
      effect.dialers.length > 1
        ? l.bothDial
        : effect.dialers.length === 1
          ? l.dials(nodeName(nodes, effect.dialers[0] ?? 0))
          : null
    return { kind: 'effective', label: l.effective, dialers }
  }
  const known = effect.blocked.map((block) => reasonOf(block, pair, nodes))
  const reasons = known.filter((reason): reason is LaneReason => reason !== null)
  // 有不认识的原因（或者 controller 没给原因）时，补上它的原话。
  if (known.length === 0 || reasons.length < known.length)
    reasons.push({ text: effect.summary, fix: null, fixLabel: null })
  return { kind: 'blocked', label: l.blocked, reasons }
}

/** 一眼要看的几个数。 */
export interface LaneGlance {
  inUse: number
  idle: number
  fallbacks: number
  dialFailures: number
}

/**
 * 一个节点看得到的、和一个对端之间的空闲连接：对端拨来停在这里的、自己拨好留着的、自己拨出去停在对端的。
 * 对端拨好留给它自己用的那些只有对端知道。
 */
function idleSeenBy(peer: PeerLanes): number {
  return peer.idle + peer.ready + peer.parked_at_peer
}

/** 一个节点上和一个对端之间的数（节点详情的一行）。 */
export function peerGlance(peer: PeerLanes): LaneGlance {
  return { inUse: peer.in_use, idle: idleSeenBy(peer), fallbacks: peer.fallbacks, dialFailures: peer.dial_failures }
}

/**
 * 一对节点的数，由两边各自报的合起来：
 * - 一条在用的连接两头都算它，所以取两边里大的那个，不相加。
 * - 空闲的连接按停在哪一边算：两边各自的"对端拨来停在这里的"和"自己拨好留着的"相加。只有一边报时，
 *   用它报的"停在对端的"补上另一边。
 * - 回退和拨号失败只记在开流、拨号的那一边，相加。
 *
 * 两边都没报时是 `null`。
 */
export function pairGlance(sides: readonly PairLaneSide[]): LaneGlance | null {
  const [first, second] = sides
  if (first === undefined) return null
  if (second === undefined) return peerGlance(first.lanes)
  const both = [first.lanes, second.lanes]
  const sum = (value: (peer: PeerLanes) => number) => both.reduce((total, peer) => total + value(peer), 0)
  return {
    inUse: Math.max(first.lanes.in_use, second.lanes.in_use),
    idle: sum((peer) => peer.idle + peer.ready),
    fallbacks: sum((peer) => peer.fallbacks),
    dialFailures: sum((peer) => peer.dial_failures),
  }
}

/** 搬运的字节数，三种方式相加。 */
export function laneBytes(counts: LaneCounts): number {
  return counts.sealed_bytes + counts.passthrough_bytes + counts.spliced_bytes
}

const CAUSES = [
  'deadline',
  'unknown_key',
  'tls',
  'protocol',
  'busy',
] as const satisfies readonly (keyof LaneHandshakeFailures)[]

export function handshakeTotal(failures: LaneHandshakeFailures): number {
  return CAUSES.reduce((total, cause) => total + failures[cause], 0)
}

/** 握手失败按原因，只列不是 0 的："公钥不认识 2"。 */
export function handshakeParts(failures: LaneHandshakeFailures): string[] {
  return CAUSES.filter((cause) => failures[cause] > 0).map((cause) =>
    l.causeCount(l.handshakeCause[cause], failures[cause]),
  )
}
