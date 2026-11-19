import { Switch as SwitchPrimitive } from 'radix-ui'
import { type ComponentProps } from 'react'

import { cn } from '@/lib/utils'

/** `sm` 是放在工具条、画布角上的小开关，和 28 像素高的按钮配。 */
function Switch({
  className,
  size = 'md',
  ...props
}: ComponentProps<typeof SwitchPrimitive.Root> & { size?: 'sm' | 'md' }) {
  return (
    <SwitchPrimitive.Root
      className={cn(
        'inline-flex shrink-0 items-center rounded-full border border-transparent bg-faint transition-colors data-[state=checked]:bg-primary data-disabled:opacity-45',
        size === 'sm' ? 'h-4 w-7' : 'h-5 w-9',
        className,
      )}
      {...props}
    >
      <SwitchPrimitive.Thumb
        className={cn(
          'block translate-x-0.5 rounded-full bg-white shadow-[0_1px_2px_rgb(24_24_27/0.2)] transition-transform',
          size === 'sm'
            ? 'size-3 data-[state=checked]:translate-x-[0.8125rem]'
            : 'size-4 data-[state=checked]:translate-x-[1.125rem]',
        )}
      />
    </SwitchPrimitive.Root>
  )
}

export { Switch }
