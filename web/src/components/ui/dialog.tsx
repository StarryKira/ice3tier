import { X } from 'lucide-react'
import { Dialog as DialogPrimitive } from 'radix-ui'
import { type ComponentProps, type ReactNode } from 'react'

import { t } from '@/i18n'
import { cn } from '@/lib/utils'

const Dialog = DialogPrimitive.Root
const DialogTrigger = DialogPrimitive.Trigger
const DialogClose = DialogPrimitive.Close

interface DialogContentProps extends Omit<ComponentProps<typeof DialogPrimitive.Content>, 'title'> {
  title: ReactNode
  description?: ReactNode
  /** 宽度。表单用默认的；左右两栏的对话框用 `lg`。 */
  size?: 'sm' | 'md' | 'lg'
  /**
   * 能不能用 Esc 和右上角的叉关掉。显示一次性密钥的对话框是 false：只能点里面的按钮。
   * 点对话框外面在任何情况下都不关闭，免得误触丢掉填了一半的表单。
   */
  dismissible?: boolean
}

const SIZES = { sm: 'max-w-md', md: 'max-w-xl', lg: 'max-w-4xl' } as const

function DialogContent({
  title,
  description,
  size = 'md',
  dismissible = true,
  className,
  children,
  ...props
}: DialogContentProps) {
  return (
    <DialogPrimitive.Portal>
      <DialogPrimitive.Overlay className="fixed inset-0 z-40 animate-fade bg-foreground/35" />
      <DialogPrimitive.Content
        onPointerDownOutside={(event) => event.preventDefault()}
        onInteractOutside={(event) => event.preventDefault()}
        onEscapeKeyDown={dismissible ? undefined : (event) => event.preventDefault()}
        aria-describedby={description === undefined ? undefined : props['aria-describedby']}
        className={cn(
          'fixed top-1/2 left-1/2 z-50 flex max-h-[calc(100vh-3rem)] w-[calc(100vw-3rem)] -translate-x-1/2 -translate-y-1/2 animate-fade flex-col rounded-xl border bg-card shadow-pop outline-none',
          SIZES[size],
          className,
        )}
        {...props}
      >
        <header className="flex items-start justify-between gap-4 border-b px-6 py-4">
          <div className="min-w-0">
            <DialogPrimitive.Title className="text-base font-semibold">{title}</DialogPrimitive.Title>
            {description !== undefined ? (
              <DialogPrimitive.Description className="mt-1 text-[13px] text-muted-foreground">
                {description}
              </DialogPrimitive.Description>
            ) : null}
          </div>
          {dismissible ? (
            <DialogPrimitive.Close
              aria-label={t.common.close}
              className="-mt-0.5 -mr-2 flex size-8 shrink-0 items-center justify-center rounded-md text-muted-foreground hover:bg-muted hover:text-foreground"
            >
              <X className="size-4" aria-hidden />
            </DialogPrimitive.Close>
          ) : null}
        </header>
        {children}
      </DialogPrimitive.Content>
    </DialogPrimitive.Portal>
  )
}

function DialogBody({ className, ...props }: ComponentProps<'div'>) {
  return <div className={cn('flex-1 overflow-y-auto px-6 py-5', className)} {...props} />
}

function DialogFooter({ className, ...props }: ComponentProps<'footer'>) {
  // 页脚有自己的底色，下面两个角要跟着对话框一起圆，不然方角会从圆角外面露出来。
  return (
    <footer
      className={cn('flex items-center justify-end gap-2 rounded-b-xl border-t bg-muted/40 px-6 py-3.5', className)}
      {...props}
    />
  )
}

export { Dialog, DialogBody, DialogClose, DialogContent, DialogFooter, DialogTrigger }
