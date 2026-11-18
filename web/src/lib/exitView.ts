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
