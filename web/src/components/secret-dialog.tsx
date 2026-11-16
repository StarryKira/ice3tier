import { type ReactNode } from 'react'

import { CodeBlock } from '@/components/copy'
import { Button } from '@/components/ui/button'
import { Dialog, DialogBody, DialogContent, DialogFooter } from '@/components/ui/dialog'

interface SecretDialogProps {
  open: boolean
  title: string
  /** 一句话："只显示这一次"。 */
  warning: string
  secret: string
  secretLabel: string
  copyLabel: string
  /** 紧接着的下一步：一段可以复制的用法，里面已经填好了这个密钥。 */
  children?: ReactNode
  closeLabel: string
  onClose: () => void
}

/**
 * 只显示一次的密钥，全站用同一套呈现：单独的一个对话框，按 Esc 和点外面都关不掉，只能点按钮；
 * 一句"只显示这一次"；密钥完整显示在等宽的框里，右侧是复制按钮；紧接着是下一步。
 * 关掉之后任何地方都没有"再看一次"，丢了就吊销重发。
 */
export function SecretDialog({
  open,
  title,
  warning,
  secret,
  secretLabel,
  copyLabel,
  children,
  closeLabel,
  onClose,
}: SecretDialogProps) {
  return (
    <Dialog open={open}>
      <DialogContent title={title} dismissible={false} data-testid="secret-dialog">
        <DialogBody className="space-y-5">
          <p className="rounded-md bg-warn-soft px-3.5 py-2.5 text-[13px] font-medium text-warn">{warning}</p>
          <CodeBlock copy={secret} copyLabel={copyLabel} label={secretLabel} testId="secret-value">
            {secret}
          </CodeBlock>
          {children}
        </DialogBody>
        <DialogFooter>
          <Button variant="primary" onClick={onClose}>
            {closeLabel}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
