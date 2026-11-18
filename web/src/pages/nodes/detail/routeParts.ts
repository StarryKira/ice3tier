import { type NodeDetail, type NodeId, type Route } from '@/api/types'
import { t } from '@/i18n'

/** 一条路底下第一段的传输：P2P 是 UDP；经中继时看连第一个中继用的什么。不知道时是 `null`。 */
export type Leg = 'udp' | 'tcp' | 'unknown' | null

/**
 * 两端之间经过的别的节点：经中继时的中继，去掉本节点自己和对端自己（本节点是中继、对端挂在它上面，或者对端是中继、
 * 本节点挂在它上面，中间都没有别人）。P2P 是空的；走不通是 `null`。
 */
export function middleOf(route: Route, self: NodeId, peer: NodeId): NodeId[] | null {
  if (route.via === 'none') return null
  return route.via === 'direct' ? [] : route.relays.filter((id) => id !== self && id !== peer)
}

/** 路径长度：中间经过的节点数加一。 */
export const lengthOf = (middle: readonly NodeId[] | null): number | null =>
  middle === null ? null : middle.length + 1

/**
 * 一条路底下的传输：P2P 直连只用 UDP；经中继时是到第一个中继那一段的传输（`reported`）。本节点自己就是那个中继、
 * 对端挂在它上面时，这一段是对端连过来的：看对端连本节点用的是什么（`peerLeg`）。
 */
export function legOf(route: Route, self: NodeId, reported: Leg, peerLeg: Leg): Leg {
  if (route.via === 'direct') return 'udp'
  if (route.via !== 'relay') return null
  if (route.relays[0] === self) return route.relays.length === 1 ? peerLeg : null
  return reported
}

/** 对端连本节点用的传输，取自本节点的 `reached_by`。 */
export function peerLegOf(node: NodeDetail, peer: NodeId): Leg {
  const outcome = node.reached_by?.find((reach) => reach.node.id === peer)?.outcome
  return outcome === 'udp' || outcome === 'tcp' ? outcome : null
}

/** 协议写成一句话：平常 QUIC/UDP；QUIC 包在 TCP 里送到中继时只写 TCP；不知道底下是什么时只写 QUIC。走不通是 `null`。 */
export function protocolText(route: Route, leg: Leg): string | null {
  if (route.via === 'none') return null
  const p = t.routes.protocol
  if (leg === 'tcp') return t.routes.legTransport.tcp
  return leg === 'udp' ? p.over(p.quic, t.routes.legTransport.udp) : p.quic
}
