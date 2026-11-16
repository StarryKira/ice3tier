import { Check, Minus } from 'lucide-react'
import { Checkbox as CheckboxPrimitive } from 'radix-ui'
import { type ComponentProps } from 'react'

import { cn } from '@/lib/utils'

function Checkbox({ className, ...props }: ComponentProps<typeof CheckboxPrimitive.Root>) {
  return (
    <CheckboxPrimitive.Root
      className={cn(
        'group flex size-4 shrink-0 items-center justify-center rounded-[4px] border border-faint bg-card transition-colors data-[state=checked]:border-primary data-[state=checked]:bg-primary data-[state=indeterminate]:border-primary data-[state=indeterminate]:bg-primary data-disabled:opacity-50',
        className,
      )}
      {...props}
    >
      <CheckboxPrimitive.Indicator className="text-primary-foreground">
        <Check className="hidden size-3 group-data-[state=checked]:block" aria-hidden style={{ strokeWidth: 3 }} />
        <Minus
          className="hidden size-3 group-data-[state=indeterminate]:block"
          aria-hidden
          style={{ strokeWidth: 3 }}
        />
      </CheckboxPrimitive.Indicator>
    </CheckboxPrimitive.Root>
  )
}

export { Checkbox }
