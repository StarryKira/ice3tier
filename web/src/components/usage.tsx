import { type ReactNode } from 'react'

import { type ServiceClass } from '@/api/types'
import { type Tone } from '@/components/status'
import { fieldBase, NativeSelect } from '@/components/ui/input'
import { Tooltip } from '@/components/ui/tooltip'
import { t } from '@/i18n'
import { BYTE_UNITS, type ByteUnit, formatBytes } from '@/lib/format'
import { SERVICE_CLASSES } from '@/lib/serviceClass'
import { type UsageView } from '@/lib/usage'
import { cn } from '@/lib/utils'

/**
 * 进度条的颜色：正常时是中性的深灰（量本身不是状态），到了八成换琥珀，用完换红。
 * 底槽是很淡的灰，条只有 4 像素高。
 */
const BAR: Record<Tone, string> = {
  ok: 'bg-foreground/70',
  idle: 'bg-idle-dot',
  relay: 'bg-relay-dot',
  warn: 'bg-warn-dot',
  danger: 'bg-danger-dot',
}

interface UsageBarProps {
  view: UsageView
  /** 给读屏软件的：这是谁的什么量。 */
  label: string
  className?: string
  /** 进度条下面的一行，例如"12 月 1 日重置"。 */
  children?: ReactNode
}

/**
 * "已用 / 上限"：一行数字加一根进度条。没有上限时只写已用的数，不画条。
 * 颜色不单独承担含义：数字和百分数都写着，读屏软件读的是 meter 的值。
 */
export function UsageBar({ view, label, className, children }: UsageBarProps) {
  return (
    <div className={cn('min-w-0', className)}>
      <div className="flex items-baseline justify-between gap-3 text-[13px] whitespace-nowrap tabular">
        <span>{view.text}</span>
        {view.percent !== null ? (
          <span
            className={cn(
              view.tone === 'danger'
                ? 'font-medium text-danger'
                : view.tone === 'warn'
                  ? 'font-medium text-warn'
                  : 'text-muted-foreground',
            )}
          >
            {t.common.percent(view.percent)}
          </span>
        ) : null}
      </div>
      {view.ratio !== null && view.percent !== null ? (
        <div
          role="meter"
          aria-label={label}
          aria-valuemin={0}
          aria-valuemax={100}
          aria-valuenow={Math.min(100, view.percent)}
          aria-valuetext={view.text}
          className="mt-1.5 h-1 w-full min-w-24 overflow-hidden rounded-full bg-border"
        >
          <div
            className={cn('h-full rounded-full', BAR[view.tone])}
            style={{ width: `${Math.min(100, view.ratio * 100)}%` }}
          />
        </div>
      ) : null}
      {children}
    </div>
  )
}

/** 两个方向的流量："↑ 1.0 MB · ↓ 20 MB"。悬停说明这两个数是怎么来的。 */
export function TrafficText({
  up,
  down,
  tip,
  className,
}: {
  up: number
  down: number
  tip: string
  className?: string
}) {
  const upText = formatBytes(up)
  const downText = formatBytes(down)
  return (
    <Tooltip content={tip}>
      <span
        tabIndex={0}
        aria-label={t.common.trafficUpDownLabel(upText, downText)}
        className={cn('rounded-sm whitespace-nowrap tabular', className)}
      >
        {t.common.trafficUpDown(upText, downText)}
      </span>
    </Tooltip>
  )
}

interface ClassSelectProps {
  id?: string
  value: ServiceClass | null
  onChange: (value: ServiceClass | null) => void
  /** "不设"那一项怎么说：不设时用的是谁的。`null` 时没有这一项（网络的默认值总要有一个）。 */
  inheritLabel: string | null
  disabled?: boolean
  className?: string
  'aria-describedby'?: string
  'aria-invalid'?: true
}

/** 服务等级的下拉。每一项后面一句话说后果。 */
export function ClassSelect({ value, onChange, inheritLabel, className, ...props }: ClassSelectProps) {
  return (
    <NativeSelect
      {...props}
      value={value ?? ''}
      onChange={(event) => {
        const next = event.target.value
        onChange(next === '' ? null : (next as ServiceClass))
      }}
      className={cn('w-full max-w-md', className)}
    >
      {inheritLabel !== null ? <option value="">{inheritLabel}</option> : null}
      {SERVICE_CLASSES.map((candidate) => (
        <option key={candidate} value={candidate}>
          {`${t.serviceClass[candidate]} · ${t.serviceClass[`${candidate}Desc`]}`}
        </option>
      ))}
    </NativeSelect>
  )
}

interface BytesInputProps {
  id?: string
  value: string
  unit: ByteUnit
  onChange: (value: string, unit: ByteUnit) => void
  unitLabel: string
  disabled?: boolean
  'aria-describedby'?: string
  'aria-invalid'?: true
}

/** 流量上限的输入：一个数加一个单位（MB、GB、TB，按 1024 进位，和界面上别处写的大小一致）。留空表示不限。 */
export function BytesInput({ value, unit, onChange, unitLabel, disabled, ...props }: BytesInputProps) {
  return (
    <div className="flex items-center gap-2">
      <input
        {...props}
        inputMode="decimal"
        autoComplete="off"
        disabled={disabled}
        value={value}
        placeholder={t.common.unlimited}
        onChange={(event) => onChange(event.target.value, unit)}
        className={cn(fieldBase, 'h-9 w-28 tabular')}
      />
      <NativeSelect
        aria-label={unitLabel}
        value={unit}
        disabled={disabled}
        onChange={(event) => onChange(value, event.target.value as ByteUnit)}
        className="w-22"
      >
        {BYTE_UNITS.map((candidate) => (
          <option key={candidate} value={candidate}>
            {candidate}
          </option>
        ))}
      </NativeSelect>
    </div>
  )
}
