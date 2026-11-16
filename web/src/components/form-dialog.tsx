import { type SyntheticEvent, type ReactNode } from 'react'

import { useConfirm } from '@/components/confirm'
import { Button } from '@/components/ui/button'
import { Dialog, DialogBody, DialogContent, DialogFooter } from '@/components/ui/dialog'
import { FormError } from '@/components/ui/field'
import { t } from '@/i18n'

interface FormDialogProps {
  open: boolean
  onClose: () => void
  title: ReactNode
  description?: ReactNode
  size?: 'sm' | 'md' | 'lg'
  /** 表单有没有改动。有改动时关闭要问一句；没有改动时"保存"是灰的（`requireDirty` 为真时）。 */
  dirty: boolean
  requireDirty?: boolean
  submitLabel: string
  pending: boolean
  /** 提交之前的校验没过时为 false："保存"不可用。 */
  canSubmit?: boolean
  /** 整个表单的错误（服务器的拒绝理由说不清属于哪个字段时）。 */
  error?: string | null
  /** 它的补充：controller 的原话（`formErrorDetail`）。 */
  errorDetail?: string | null
  onSubmit: () => void
  children: ReactNode
  /** 页脚左侧的一句常驻说明。 */
  footerNote?: ReactNode
}

/**
 * 对话框里的表单：点"保存"才生效；有改动时关闭要问一句；请求期间按钮不能重复点。
 * 回车提交。
 */
export function FormDialog({
  open,
  onClose,
  title,
  description,
  size,
  dirty,
  requireDirty = false,
  submitLabel,
  pending,
  canSubmit = true,
  error,
  errorDetail,
  onSubmit,
  children,
  footerNote,
}: FormDialogProps) {
  const confirm = useConfirm()

  const requestClose = async () => {
    if (pending) return
    if (dirty) {
      const discard = await confirm({
        title: t.common.discardTitle,
        confirmLabel: t.common.discardConfirm,
        cancelLabel: t.common.discardCancel,
      })
      if (!discard) return
    }
    onClose()
  }

  const submit = (event: SyntheticEvent) => {
    event.preventDefault()
    if (!pending && canSubmit) onSubmit()
  }

  return (
    <Dialog open={open} onOpenChange={(next) => (next ? undefined : void requestClose())}>
      <DialogContent title={title} description={description} size={size}>
        <form onSubmit={submit} noValidate className="flex min-h-0 flex-1 flex-col">
          <DialogBody className="space-y-5">
            {children}
            <FormError message={error} detail={errorDetail} />
          </DialogBody>
          <DialogFooter className={footerNote !== undefined ? 'justify-between' : undefined}>
            {footerNote !== undefined ? (
              <p className="min-w-0 text-[13px] text-muted-foreground">{footerNote}</p>
            ) : null}
            <div className="flex shrink-0 items-center gap-2">
              <Button onClick={() => void requestClose()} disabled={pending}>
                {t.common.cancel}
              </Button>
              <Button
                type="submit"
                variant="primary"
                pending={pending}
                disabled={!canSubmit || (requireDirty && !dirty)}
              >
                {submitLabel}
              </Button>
            </div>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  )
}
