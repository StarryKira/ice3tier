import { type BackboneRelay, type NodeId, type RelayState } from '@/api/types'
import { type Tone } from '@/components/status'
import { t } from '@/i18n'
import { formatAge } from '@/lib/time'

import { formatPermille, textFor } from './route'

const b = t.topology.backbone

export interface RelayRowView {
  /** 它自己说的状态；问到的节点手上都没有这一行时是"没有这一行"。 */
  state: { label: string; tone: Tone; hint: string }
  load: string | null
  /** "2 秒前的数据"。 */
  age: string | null
  /** "从 laptop 问来"。 */
  reportedBy: string | null
}

const STATE_TONE: Record<RelayState, Tone> = { serving: 'ok', overloaded: 'warn', draining: 'warn', unknown: 'idle' }

/**
 * 骨干矩阵一行的表头：relay 自己报的状态、负载、这一行多新、从谁那里问来的。
 * 满载和即将下线是琥珀色：它还在，只是节点会改走别的 relay。
 */
export function relayRowView(relay: BackboneRelay, name: (id: NodeId) => string): RelayRowView {
  if (relay.state === null) {
    return { state: { label: b.noRow, tone: 'idle', hint: b.noRowHint }, load: null, age: null, reportedBy: null }
  }
  // 比界面新的 controller 可能报这里不认识的状态，按 unknown 写。
  const known = Object.hasOwn(STATE_TONE, relay.state)
  return {
    state: {
      label: textFor(b.state, relay.state) ?? b.state.unknown,
      tone: known ? STATE_TONE[relay.state] : 'idle',
      hint: textFor(b.stateHint, relay.state) ?? b.stateHint.unknown,
    },
    load: relay.load_percent === null ? null : b.load(relay.load_percent),
    age: relay.age_ms === null ? null : b.rowAge(formatAge(relay.age_ms / 1000)),
    reportedBy: relay.reported_by === null ? null : b.reportedBy(name(relay.reported_by)),
  }
}

export interface LinkView {
  /** 格子里写的：延迟的毫秒数，或者"✕"。 */
  short: string
  tone: Tone
  /** 悬停和读屏的完整说法。 */
  text: string
}

/** 一个 relay 到另一个 relay 的链路怎么写。这一行里没有到它的链路时返回 `null`（没有数据）。 */
export function linkView(relay: BackboneRelay, to: NodeId): LinkView | null {
  const link = relay.links.find((candidate) => candidate.to === to)
  if (link === undefined) return null
  if (link.rtt_ms === null) return { short: t.routes.symbolNone, tone: 'danger', text: b.unreachable }
  return {
    short: String(link.rtt_ms),
    // 丢包到了百分之一就值得看一眼。
    tone: link.loss_permille >= 10 ? 'warn' : 'ok',
    text: b.cell(link.rtt_ms, link.jitter_ms, formatPermille(link.loss_permille)),
  }
}
