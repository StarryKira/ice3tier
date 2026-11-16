import { RadioGroup } from 'radix-ui'
import { type ReactNode } from 'react'

import { cn } from '@/lib/utils'

interface SegmentedProps<V extends string> {
  value: V
  onChange: (value: V) => void
  options: readonly { value: V; label: ReactNode }[]
  'aria-label': string
  size?: 'sm' | 'md'
  className?: string
}

/** 带文字的分段切换：几个互斥的选项并排，选中的那个有底色。键盘上用方向键换。 */
function Segmented<V extends string>({
  value,
  onChange,
  options,
  size = 'md',
  className,
  ...props
}: SegmentedProps<V>) {
  return (
    <RadioGroup.Root
      value={value}
      onValueChange={(next) => onChange(next as V)}
      orientation="horizontal"
      aria-label={props['aria-label']}
      className={cn('inline-flex rounded-md border border-input bg-muted p-0.5', className)}
    >
      {options.map((option) => (
        <RadioGroup.Item
          key={option.value}
          value={option.value}
          className={cn(
            'inline-flex items-center justify-center gap-1.5 rounded-[5px] px-3 font-medium whitespace-nowrap text-muted-foreground transition-colors hover:text-foreground data-[state=checked]:bg-card data-[state=checked]:text-foreground data-[state=checked]:shadow-card',
            size === 'sm' ? 'h-6.5 text-[13px]' : 'h-7.5 text-sm',
          )}
        >
          {option.label}
        </RadioGroup.Item>
      ))}
    </RadioGroup.Root>
  )
}

export { Segmented }
