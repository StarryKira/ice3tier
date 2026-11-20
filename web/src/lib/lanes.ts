import {
  type LaneBlock,
  type LaneCounts,
  type LaneEffect,
  type LaneHandshakeFailures,
  type LaneSide,
  type Node,
  type NodeId,
  type PairView,
  type PeerLanes,
  type Proxy,
} from '@/api/types'
import { t } from '@/i18n'

import { nodeName } from './nodes'

/**
 * ice3proxy（lane）在界面上怎么说。纯函数：代理页和节点详情用同一套。
 *
 * 0.1.26 起 ice3proxy 是代理的一个选项：打开的代理的流一直走它（监听的节点和出口之间一条连接一条 TCP），
 * 开不了时退回共用连接；没打开的永远不走。节点对上没有它了。
 * 起不起作用、为什么，按 controller 给的原因码（`lane_effect.blocked`）用界面自己的话写，带着修好它的那一步；
 * controller 的 `summary` 只在原因码不认识时（比界面新的 controller）原样显示。
 */

const l = t.proxies.lane

/** 比 0.1.19 旧的 controller 不写 `stored`：那时只有存下的才答得出来。 */
export function pairStored(pair: Pick<PairView, 'stored'>): boolean {
  return pair.stored ?? true
}

/** 一个代理的 ice3proxy：比界面旧的 controller 不写这三个字段，按关着、`null`、`[]` 算。 */
export function proxyLane(proxy: Pick<Proxy, 'lane' | 'lane_effect' | 'lanes'>): {
  on: boolean
  effect: LaneEffect | null
  sides: LaneSide[]
} {
  return { on: proxy.lane === true, effect: proxy.lane_effect ?? null, sides: proxy.lanes ?? [] }
}

/** 这个代理两端报了 ice3proxy 的计数：有就能展开看。 */
export function lanesReported(proxy: Pick<Proxy, 'lane' | 'lane_effect' | 'lanes'>): boolean {
  return proxyLane(proxy).sides.length > 0
}

/**
 * 节点详情里，一个对端之间现在有没有 ice3proxy：有在用的连接，或者和对端之间的控制连接在（只有它们之间有选了
 * ice3proxy 的代理时才有）。
 */
export function peerUsesLanes(peer: PeerLanes): boolean {
  return peer.in_use > 0 || peer.control
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
      /** 下面一行：谁拨号（controller 没说时是 `null`）。 */
      detail: string | null
    }
  | { kind: 'blocked'; label: string; reasons: LaneReason[] }

/** 代理的两端：监听的节点和出口。 */
export interface LaneEnds {
  listen: NodeId
  exit: NodeId
}

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

function reasonOf(block: LaneBlock, ends: LaneEnds, nodes: readonly NodeInfo[]): LaneReason | null {
  const name = (id: NodeId) => nodeName(nodes, id)
  // 比界面新的 controller 可能报出这里不认识的原因：交给调用方用 controller 的原话。
  const reason: string = block.reason
  switch (reason) {
    case 'no_relay': {
      const node = relayCandidate(ends.listen, ends.exit, nodes)
      return {
        text: l.reason.noRelay(name(ends.listen), name(ends.exit)),
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
      // 访问规则不让监听的节点访问出口。
      return {
        text: l.reason.notConnected(name(ends.listen), name(ends.exit)),
        fix: { kind: 'access', from: ends.listen, to: ends.exit },
        fixLabel: l.fix.access,
      }
    default:
      return null
  }
}

/**
 * 一个代理的 ice3proxy 起不起作用。没打开、没有出口、或者 controller 太旧没有说时是 `off`。
 * 起作用时有在用的连接写"在用"，不然写"生效"；下面一行是谁拨号。
 */
export function laneEffectView(
  effect: LaneEffect | null,
  ends: LaneEnds,
  sides: readonly LaneSide[],
  nodes: readonly NodeInfo[],
): LaneEffectView {
  if (effect === null) return { kind: 'off' }
  if (effect.effective) {
    const dialers =
      effect.dialers.length > 1
        ? l.bothDial
        : effect.dialers.length === 1
          ? l.dials(nodeName(nodes, effect.dialers[0] ?? 0))
          : null
    const inUse = (pairGlance(sides)?.inUse ?? 0) > 0
    return { kind: 'effective', label: inUse ? l.inUse : l.effective, detail: dialers }
  }
  const known = effect.blocked.map((block) => reasonOf(block, ends, nodes))
  const reasons = known.filter((reason): reason is LaneReason => reason !== null)
  // 有不认识的原因（或者 controller 没给原因）时，补上它的原话。
  if (known.length === 0 || reasons.length < known.length)
    reasons.push({ text: effect.summary, fix: null, fixLabel: null })
  return { kind: 'blocked', label: l.blocked, reasons }
}

/**
 * 代理列表里 ice3proxy 那一格的样子。没打开是 `null`；打开了但没有出口（controller 不说）是 `off`；
 * 其余按 `lane_effect` 说起不起作用。
 */
export function proxyLaneView(
  proxy: Pick<Proxy, 'node' | 'exit' | 'lane' | 'lane_effect' | 'lanes'>,
  nodes: readonly NodeInfo[],
): LaneEffectView | null {
  const lane = proxyLane(proxy)
  if (!lane.on) return null
  if (proxy.exit === null) return { kind: 'off' }
  return laneEffectView(lane.effect, { listen: proxy.node, exit: proxy.exit }, lane.sides, nodes)
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
 * 代理两端（一对节点）的数，由两边各自报的合起来：
 * - 一条在用的连接两头都算它，所以取两边里大的那个，不相加。
 * - 空闲的连接按停在哪一边算：两边各自的"对端拨来停在这里的"和"自己拨好留着的"相加。只有一边报时，
 *   用它报的"停在对端的"补上另一边。
 * - 回退和拨号失败只记在开流、拨号的那一边，相加。
 *
 * 两边都没报时是 `null`。
 */
export function pairGlance(sides: readonly LaneSide[]): LaneGlance | null {
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
