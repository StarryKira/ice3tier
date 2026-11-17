import { type ReactNode } from 'react'

import { InfoTip } from '@/components/info-tip'

interface SettingRowProps {
  label: string
  /** 悬停在 ⓘ 上看的说明。 */
  hint?: string
  /** 直接写在名字下面的说明：不看就不懂这一项时用它，不用 `hint`。控件用 `aria-describedby` 指向 `descriptionId`。 */
  description?: string
  descriptionId?: string
  /** 名字是哪个控件的 `<label>`；不给时名字是一段带 `labelId` 的文字（控件用 `aria-labelledby` 指向它）。 */
  htmlFor?: string
  labelId?: string
  /** 控件放在名字下面（可以选好几个的那种），而不是右边。 */
  stacked?: boolean
  /** 这一项的值不对：写在这一行的最下面。 */
  error?: string | null
  children: ReactNode
}

/** 一项设置：左边是名字和悬停的说明，右边是控件。行和信息卡一样高、一样的字。 */
export function SettingRow({
  label,
  hint,
  description,
  descriptionId,
  htmlFor,
  labelId,
  stacked = false,
  error,
  children,
}: SettingRowProps) {
  const problem =
    error !== undefined && error !== null && error !== '' ? (
      <p role="alert" className="mt-1.5 text-[13px] text-danger">
        {error}
      </p>
    ) : null
  const name = (
    <div className="min-w-0">
      <div className="flex items-center gap-1.5 text-sm">
        {htmlFor !== undefined ? <label htmlFor={htmlFor}>{label}</label> : <span id={labelId}>{label}</span>}
        {hint !== undefined ? <InfoTip>{hint}</InfoTip> : null}
      </div>
      {description !== undefined ? (
        <p id={descriptionId} className="mt-0.5 text-[13px] text-muted-foreground">
          {description}
        </p>
      ) : null}
    </div>
  )
  if (stacked) {
    return (
      <div className="px-5 py-3">
        <div className="flex min-h-7 items-center">{name}</div>
        <div className="mt-2">{children}</div>
        {problem}
      </div>
    )
  }
  return (
    <div className="px-5 py-2.5">
      <div className="flex min-h-9 items-center justify-between gap-4">
        {name}
        <div className="shrink-0">{children}</div>
      </div>
      {problem}
    </div>
  )
}
