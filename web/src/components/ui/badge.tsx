import { cva, type VariantProps } from 'class-variance-authority'
import { type ComponentProps } from 'react'

import { cn } from '@/lib/utils'

const badgeVariants = cva(
  'inline-flex h-5 items-center gap-1 rounded-sm px-1.5 text-xs leading-none font-medium whitespace-nowrap [&_svg]:size-3',
  {
    variants: {
      tone: {
        neutral: 'bg-secondary text-muted-foreground',
        accent: 'bg-accent text-accent-foreground',
        ok: 'bg-ok-soft text-ok',
        relay: 'bg-relay-soft text-relay',
        warn: 'bg-warn-soft text-warn',
        danger: 'bg-danger-soft text-danger',
        outline: 'border border-border-strong font-normal text-muted-foreground',
      },
    },
    defaultVariants: { tone: 'neutral' },
  },
)

function Badge({ className, tone, ...props }: ComponentProps<'span'> & VariantProps<typeof badgeVariants>) {
  return <span className={cn(badgeVariants({ tone }), className)} {...props} />
}

export { Badge }
