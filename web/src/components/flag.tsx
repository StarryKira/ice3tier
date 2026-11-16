import { useState } from 'react'

import { useLocale } from '@/i18n'
import { countryName } from '@/lib/region'
import { cn } from '@/lib/utils'

/**
 * 一面圆形的小国旗（16 像素），悬停写出国家或地区的名字。代码不是两个大写字母（没有、不知道）时什么都不画。
 *
 * 旗子是 `public/flags/` 里的 SVG（circle-flags，MIT，许可在同一个目录里），随界面一起发布，按需取、不联网。
 * 不用国旗表情：Windows 上的浏览器不画国旗表情。
 */
export function Flag({ code, className }: { code: string | null | undefined; className?: string }) {
  const locale = useLocale()
  // 库里给的代码偶尔没有对应的旗子（新的或者罕见的地区）：取不到就不画，不留一个破图。
  const [missing, setMissing] = useState(false)
  if (missing || code === null || code === undefined || !/^[A-Z]{2}$/.test(code)) return null
  const name = countryName(code, locale)
  return (
    <img
      src={`/flags/${code.toLowerCase()}.svg`}
      alt={name}
      title={name}
      width={16}
      height={16}
      loading="lazy"
      draggable={false}
      onError={() => setMissing(true)}
      className={cn('size-4 shrink-0 rounded-full', className)}
    />
  )
}
