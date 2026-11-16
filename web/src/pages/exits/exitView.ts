import { type ExitView } from '@/api/types'
import { type Tone } from '@/components/status'
import { t } from '@/i18n'

const h = t.exits.health

export interface HealthView {
  tone: Tone
  label: string
  /** 为什么是这个结论：带着最近 5 分钟的数。 */
  detail: string
}

/**
 * 出口的健康怎么说。排空和离线比 controller 算的健康更要紧，先说它们。
 * "有问题"用琥珀色而不是红色：出口还能用，只是有一部分请求因为它自己的原因失败了。
 */
export function exitHealthView(view: ExitView): HealthView {
  if (!view.online) return { tone: 'idle', label: h.offline, detail: h.offlineDetail }
  if (view.usage?.draining === true)
    return { tone: 'warn', label: h.draining, detail: h.drainingDetail(view.usage.active) }
  switch (view.health) {
    case 'ok':
      return { tone: 'ok', label: h.ok, detail: h.okDetail(view.recent_requests, view.recent_failures) }
    case 'degraded':
      return {
        tone: 'warn',
        label: h.degraded,
        detail: h.degradedDetail(view.recent_requests, view.recent_failures),
      }
    case 'unknown':
      return { tone: 'idle', label: h.unknown, detail: h.unknownDetail }
  }
}

/** 两边差多少才算"对不上"：各自每 10 秒报一次，差一点是时间差；差出四分之一以上、而且超过这么多字节才提醒。 */
export const MISMATCH_RATIO = 0.25
export const MISMATCH_FLOOR = 10 * 1024 * 1024

export interface CrossCheck {
  /** 出口报的搬运量（发往目标的加收回的）。 */
  exitBytes: number
  /** 用它的节点合计看到的（发出的加收到的）。 */
  observedBytes: number
  /** 差得多：任何一边重启过都会这样，所以只是提醒，不是告警。 */
  mismatch: boolean
}

/**
 * 出口报的和用它的节点看到的对照（白皮书的交叉核对）。两边都要有数才对照：
 * 出口还没报过、或者还没有节点报过经它的流时返回 `null`。
 */
export function crossCheck(view: ExitView): CrossCheck | null {
  if (view.usage === null || view.observed.length === 0) return null
  const exitBytes = view.usage.bytes_to_target + view.usage.bytes_from_target
  const observedBytes = view.observed.reduce((sum, seen) => sum + seen.bytes_up + seen.bytes_down, 0)
  const gap = Math.abs(exitBytes - observedBytes)
  const mismatch = gap > Math.max(MISMATCH_RATIO * Math.max(exitBytes, observedBytes), MISMATCH_FLOOR)
  return { exitBytes, observedBytes, mismatch }
}
