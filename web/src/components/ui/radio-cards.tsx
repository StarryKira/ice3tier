import { RadioGroup } from 'radix-ui'
import { type ReactNode } from 'react'

import { cn } from '@/lib/utils'

interface RadioCardOption<V extends string> {
  value: V
  title: ReactNode
  /** 一句话，说选了它会怎样。 */
  description?: ReactNode
  disabled?: boolean
  /** 不能选的原因，写在卡片里。 */
  disabledReason?: ReactNode
}

interface RadioCardsProps<V extends string> {
  value: V
  onChange: (value: V) => void
  options: readonly RadioCardOption<V>[]
  'aria-label'?: string
  'aria-labelledby'?: string
  columns?: 1 | 2 | 3
  className?: string
}

const COLUMNS = { 1: 'grid-cols-1', 2: 'grid-cols-2', 3: 'grid-cols-3' } as const

/** 单选卡片：每个选项带一句说明后果的话。 */
function RadioCards<V extends string>({
  value,
  onChange,
  options,
  columns = 1,
  className,
  ...aria
}: RadioCardsProps<V>) {
  return (
    <RadioGroup.Root
      value={value}
      onValueChange={(next) => onChange(next as V)}
      className={cn('grid gap-2', COLUMNS[columns], className)}
      {...aria}
    >
      {options.map((option) => (
        <RadioGroup.Item
          key={option.value}
          value={option.value}
          disabled={option.disabled}
          className="group flex items-start gap-2.5 rounded-md border border-input bg-card px-3 py-2.5 text-left transition-colors hover:bg-muted/60 data-disabled:bg-muted data-disabled:opacity-70 data-[state=checked]:border-primary data-[state=checked]:bg-accent/50"
        >
          <span
            aria-hidden
            className="mt-0.5 flex size-4 shrink-0 items-center justify-center rounded-full border border-input bg-card group-data-[state=checked]:border-primary"
          >
            <span className="size-2 rounded-full bg-primary opacity-0 group-data-[state=checked]:opacity-100" />
          </span>
          <span className="min-w-0">
            <span className="block text-sm font-medium">{option.title}</span>
            {option.description !== undefined ? (
              <span className="mt-0.5 block text-[13px] leading-snug text-muted-foreground">{option.description}</span>
            ) : null}
            {option.disabled === true && option.disabledReason !== undefined ? (
              <span className="mt-1 block text-[13px] leading-snug text-warn">{option.disabledReason}</span>
            ) : null}
          </span>
        </RadioGroup.Item>
      ))}
    </RadioGroup.Root>
  )
}

export { RadioCards, type RadioCardOption }
