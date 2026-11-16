import { Tooltip as TooltipPrimitive } from 'radix-ui'
import { type ReactNode } from 'react'

const TooltipProvider = TooltipPrimitive.Provider

interface TooltipProps {
  content: ReactNode
  children: ReactNode
  side?: 'top' | 'right' | 'bottom' | 'left'
}

/** 悬停或者键盘聚焦时出现的一小段说明。`children` 必须是能获得焦点的元素。 */
function Tooltip({ content, children, side = 'top' }: TooltipProps) {
  if (content === null || content === undefined || content === '') return <>{children}</>
  return (
    <TooltipPrimitive.Root>
      <TooltipPrimitive.Trigger asChild>{children}</TooltipPrimitive.Trigger>
      <TooltipPrimitive.Portal>
        <TooltipPrimitive.Content
          side={side}
          sideOffset={6}
          collisionPadding={8}
          className="z-50 max-w-80 animate-fade rounded-md bg-tooltip px-2.5 py-1.5 text-xs leading-snug text-tooltip-foreground"
        >
          {content}
        </TooltipPrimitive.Content>
      </TooltipPrimitive.Portal>
    </TooltipPrimitive.Root>
  )
}

export { Tooltip, TooltipProvider }
