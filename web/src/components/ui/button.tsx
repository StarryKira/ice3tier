import { cva, type VariantProps } from 'class-variance-authority'
import { LoaderCircle } from 'lucide-react'
import { Slot } from 'radix-ui'
import { type ComponentProps } from 'react'

import { cn } from '@/lib/utils'

const buttonVariants = cva(
  'inline-flex shrink-0 items-center justify-center gap-1.5 rounded-md text-sm font-medium whitespace-nowrap transition-colors select-none disabled:opacity-45 aria-disabled:opacity-45 [&_svg]:size-4 [&_svg]:shrink-0',
  {
    variants: {
      variant: {
        // 一页最多一个：这一页最主要的那件事。
        primary: 'bg-primary text-primary-foreground hover:bg-primary-hover',
        // 默认：次要的动作。白底、细边。
        outline:
          'border border-input bg-card text-foreground hover:border-border-strong hover:bg-muted aria-pressed:border-border-strong aria-pressed:bg-muted',
        // 最安静的：工具栏、行内、菜单的触发。
        ghost: 'text-foreground hover:bg-muted [&_svg]:text-muted-foreground',
        danger: 'bg-destructive text-destructive-foreground hover:bg-destructive-hover',
        'danger-outline': 'border border-input bg-card text-danger hover:border-danger-line hover:bg-danger-soft',
        link: 'h-auto rounded-sm px-0 text-primary underline-offset-4 hover:underline',
      },
      size: {
        sm: 'h-7 px-2.5 text-[13px] [&_svg]:size-3.5',
        md: 'h-9 px-4',
        icon: 'size-8',
        'icon-sm': 'size-7',
      },
    },
    compoundVariants: [{ variant: 'link', class: 'h-auto px-0' }],
    defaultVariants: { variant: 'outline', size: 'md' },
  },
)

interface ButtonProps extends ComponentProps<'button'>, VariantProps<typeof buttonVariants> {
  asChild?: boolean
  /** 请求进行中：显示转圈，并且不能重复点。 */
  pending?: boolean
}

function Button({
  className,
  variant,
  size,
  asChild = false,
  pending = false,
  disabled,
  children,
  type,
  ...props
}: ButtonProps) {
  const classes = cn(buttonVariants({ variant, size }), className)
  if (asChild) {
    return (
      <Slot.Root className={classes} {...props}>
        {children}
      </Slot.Root>
    )
  }
  return (
    <button
      type={type ?? 'button'}
      className={classes}
      disabled={disabled === true || pending}
      aria-busy={pending || undefined}
      {...props}
    >
      {pending ? <LoaderCircle className="animate-spin" aria-hidden /> : null}
      {children}
    </button>
  )
}

export { Button, type ButtonProps }
