import { t } from '@/i18n'

import { type Timestamp } from '@/api/types'

const MINUTE = 60
const HOUR = 60 * MINUTE
const DAY = 24 * HOUR
const WEEK = 7 * DAY

/** 带时区的完整时间，给悬停提示用。 */
export function formatFullTime(at: Timestamp): string {
  return new Intl.DateTimeFormat('zh-CN', {
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
    hour12: false,
    timeZoneName: 'short',
  }).format(new Date(at * 1000))
}

/** 日期加时分，给表格里一周以前的时间用。 */
export function formatDate(at: Timestamp, now: Timestamp): string {
  const date = new Date(at * 1000)
  const sameYear = date.getFullYear() === new Date(now * 1000).getFullYear()
  return new Intl.DateTimeFormat('zh-CN', {
    year: sameYear ? undefined : 'numeric',
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
    hour12: false,
  }).format(date)
}

/**
 * 时间戳比"现在"还晚多少以内仍然算刚刚。两个原因会让刚发生的事看起来在未来：
 * 界面上的"现在"每 20 秒才走一格；浏览器和 controller 的钟也可能差几秒。
 */
const FUTURE_TOLERANCE = 5 * MINUTE

/** 一周以内写相对时间（"3 分钟前"），更早的写日期。 */
export function formatRelative(at: Timestamp, now: Timestamp): string {
  const elapsed = now - at
  if (elapsed < -FUTURE_TOLERANCE) return formatDate(at, now)
  if (elapsed < 45) return t.time.justNow
  if (elapsed < HOUR) return t.time.minutesAgo(Math.max(1, Math.round(elapsed / MINUTE)))
  if (elapsed < DAY) return t.time.hoursAgo(Math.round(elapsed / HOUR))
  if (elapsed < WEEK) return t.time.daysAgo(Math.round(elapsed / DAY))
  return formatDate(at, now)
}

/** 还剩多久（注册令牌的有效期）。已经过了返回 `null`。 */
export function formatRemaining(until: Timestamp, now: Timestamp): string | null {
  const left = until - now
  if (left <= 0) return null
  if (left < MINUTE) return t.time.lessThanMinute
  if (left < HOUR) return t.time.minutes(Math.ceil(left / MINUTE))
  if (left < 2 * DAY) return t.time.hours(Math.round(left / HOUR))
  return t.time.days(Math.round(left / DAY))
}

/**
 * 一段已经过去的时间（秒），写成"6 秒前""3 分钟前"。给节点上报的数据标新旧用：
 * 这些数据按秒变，"刚刚"太粗；接口给的本来就是间隔，不是时刻（节点和浏览器的钟不一定对得上）。
 */
export function formatAge(secs: number): string {
  if (secs < 1) return t.time.now
  if (secs < MINUTE) return t.time.secondsAgo(Math.round(secs))
  if (secs < HOUR) return t.time.minutesAgo(Math.floor(secs / MINUTE))
  if (secs < DAY) return t.time.hoursAgo(Math.floor(secs / HOUR))
  return t.time.daysAgo(Math.floor(secs / DAY))
}

export function nowSeconds(): Timestamp {
  return Math.floor(Date.now() / 1000)
}
