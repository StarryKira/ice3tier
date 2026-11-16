import { Check, ChevronRight } from 'lucide-react'
import { DropdownMenu as MenuPrimitive } from 'radix-ui'
import { type ComponentProps } from 'react'

import { cn } from '@/lib/utils'

const DropdownMenu = MenuPrimitive.Root
const DropdownMenuTrigger = MenuPrimitive.Trigger

function DropdownMenuContent({
  className,
  align = 'end',
  sideOffset = 4,
  ...props
}: ComponentProps<typeof MenuPrimitive.Content>) {
  return (
    <MenuPrimitive.Portal>
      <MenuPrimitive.Content
        align={align}
        sideOffset={sideOffset}
        collisionPadding={8}
        className={cn(
          'z-50 min-w-44 animate-fade rounded-lg bg-popover p-1 text-popover-foreground shadow-pop',
          className,
        )}
        {...props}
      />
    </MenuPrimitive.Portal>
  )
}

interface ItemProps extends ComponentProps<typeof MenuPrimitive.Item> {
  /** 破坏性的操作：红色，放在菜单最后、分隔线之后。 */
  danger?: boolean
}

function DropdownMenuItem({ className, danger = false, ...props }: ItemProps) {
  return (
    <MenuPrimitive.Item
      className={cn(
        'flex cursor-pointer items-center gap-2.5 rounded-md px-2.5 py-1.5 text-sm outline-none select-none data-disabled:cursor-not-allowed data-disabled:opacity-50 data-highlighted:bg-muted [&_svg]:size-4 [&_svg]:shrink-0',
        danger ? 'text-danger data-highlighted:bg-danger-soft' : '[&_svg]:text-faint',
        className,
      )}
      {...props}
    />
  )
}

function DropdownMenuSeparator({ className, ...props }: ComponentProps<typeof MenuPrimitive.Separator>) {
  return <MenuPrimitive.Separator className={cn('-mx-1 my-1 h-px bg-border', className)} {...props} />
}

function DropdownMenuLabel({ className, ...props }: ComponentProps<typeof MenuPrimitive.Label>) {
  return <MenuPrimitive.Label className={cn('px-2.5 py-1.5 text-xs text-muted-foreground', className)} {...props} />
}

const DropdownMenuRadioGroup = MenuPrimitive.RadioGroup

/** 几个里选一个（例如语言）：选中的那一项右端一个勾。 */
function DropdownMenuRadioItem({ className, children, ...props }: ComponentProps<typeof MenuPrimitive.RadioItem>) {
  return (
    <MenuPrimitive.RadioItem
      className={cn(
        'flex cursor-pointer items-center gap-2.5 rounded-md px-2.5 py-1.5 text-sm outline-none select-none data-highlighted:bg-muted',
        className,
      )}
      {...props}
    >
      <span className="min-w-0 flex-1">{children}</span>
      <MenuPrimitive.ItemIndicator>
        <Check className="size-4 text-foreground" aria-hidden />
      </MenuPrimitive.ItemIndicator>
    </MenuPrimitive.RadioItem>
  )
}

const DropdownMenuSub = MenuPrimitive.Sub

/** 打开下一级菜单的一项：右端一个向右的箭头。 */
function DropdownMenuSubTrigger({ className, children, ...props }: ComponentProps<typeof MenuPrimitive.SubTrigger>) {
  return (
    <MenuPrimitive.SubTrigger
      className={cn(
        'flex cursor-pointer items-center gap-2.5 rounded-md px-2.5 py-1.5 text-sm outline-none select-none data-highlighted:bg-muted data-[state=open]:bg-muted [&_svg]:size-4 [&_svg]:shrink-0 [&_svg]:text-faint',
        className,
      )}
      {...props}
    >
      {children}
      <ChevronRight className="ml-auto" aria-hidden />
    </MenuPrimitive.SubTrigger>
  )
}

function DropdownMenuSubContent({ className, ...props }: ComponentProps<typeof MenuPrimitive.SubContent>) {
  return (
    <MenuPrimitive.Portal>
      <MenuPrimitive.SubContent
        sideOffset={6}
        collisionPadding={8}
        className={cn(
          'z-50 min-w-40 animate-fade rounded-lg bg-popover p-1 text-popover-foreground shadow-pop',
          className,
        )}
        {...props}
      />
    </MenuPrimitive.Portal>
  )
}

export {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuTrigger,
}
