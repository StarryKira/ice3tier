import { type QuotaUsage, type Timestamp } from '@/api/types'
import { type Tone } from '@/components/status'
import { t } from '@/i18n'

import { formatBytes, formatPercent } from './format'
import { formatUtcDay } from './time'

/** 用到上限的这么多就算"快用完了"：进度条变成琥珀色，总览里列为需要处理。 */
export const NEARLY_USED = 0.8

export interface UsageView {
  /** 已用占上限的比例；不限时是 `null`。用完之后可能超过 1。 */
  ratio: number | null
  /** 取整的百分数，给读屏软件和"已用 92%"这样的话用。 */
  percent: number | null
  tone: Tone
  /** "1.2 GB / 10 GB"；不限时只有已用的那个数。 */
  text: string
  used: string
  limit: string | null
  /** 已经用到或超过上限。 */
  exhausted: boolean
  nearly: boolean
}

/**
 * 一个"已用多少、上限多少"的量怎么写，凭据的流量和出口的配额共用：
 * 不到八成是绿色，八成以上琥珀色，用完了红色；没有上限时不画进度条，只写已用的数。
 */
export function usageView(used: number, limit: number | null): UsageView {
  const usedText = formatBytes(used)
  if (limit === null || limit <= 0) {
    return {
      ratio: null,
      percent: null,
      tone: 'idle',
      text: usedText,
      used: usedText,
      limit: null,
      exhausted: false,
      nearly: false,
    }
  }
  const ratio = used / limit
  const exhausted = used >= limit
  const nearly = !exhausted && ratio >= NEARLY_USED
  const limitText = formatBytes(limit)
  return {
    ratio,
    percent: formatPercent(ratio),
    tone: exhausted ? 'danger' : nearly ? 'warn' : 'ok',
    text: t.common.usageOf(usedText, limitText),
    used: usedText,
    limit: limitText,
    exhausted,
    nearly,
  }
}

export interface QuotaView extends UsageView {
  /** "12 月 1 日重置"。 */
  resets: string
  /** 下一个账期开始的时刻（出口节点的钟），给悬停显示完整时间用。 */
  resetsAt: Timestamp
}

/** 出口本账期的配额：已用、配额、什么时候重新开始算。 */
export function quotaView(quota: QuotaUsage): QuotaView {
  return {
    ...usageView(quota.used_bytes, quota.limit_bytes),
    resets: t.exits.quotaResets(formatUtcDay(quota.period_end)),
    resetsAt: quota.period_end,
  }
}
