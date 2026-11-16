import { LoaderCircle } from 'lucide-react'

import { CodeBlock } from '@/components/copy'
import { t } from '@/i18n'
import { declareCommand, declareConfig } from '@/lib/commands'
import { type Capability } from '@/lib/product'
import { cn } from '@/lib/utils'

interface DeclareHowToProps {
  capability: Capability
  /** 要在哪台机器上执行。 */
  nodeName: string
  /** 页面是不是正在等这台机器上报（在轮询）。是的话显示一行"正在等"。 */
  waiting?: boolean
  className?: string
}

/**
 * "怎么声明"：一张照着做的操作说明。灰掉的开关旁边要写三样东西——为什么不能用、
 * 在哪台机器上执行什么之后就能用、做完之后界面会怎样。命令先给 `i3t set …`，再提一句也可以改配置文件。
 */
export function DeclareHowTo({ capability, nodeName, waiting = false, className }: DeclareHowToProps) {
  return (
    <div className={cn('rounded-md border border-warn-line bg-warn-soft/60 px-4 py-3.5 text-[13px]', className)}>
      <ol className="list-decimal space-y-2.5 pl-4.5 marker:font-medium marker:text-warn">
        <li>
          <p>{t.declare.step1(nodeName)}</p>
          <CodeBlock
            copy={declareCommand(capability)}
            copyLabel={t.declare.copyCommand}
            className="mt-1.5 bg-card"
            testId="declare-command"
          >
            {declareCommand(capability)}
          </CodeBlock>
        </li>
        <li>{t.declare.step2}</li>
        <li>{t.declare.step3}</li>
      </ol>
      <details className="mt-3 text-muted-foreground">
        <summary className="rounded-sm">{t.declare.orConfig}</summary>
        <CodeBlock copy={declareConfig(capability)} className="mt-1.5 bg-card">
          {declareConfig(capability)}
        </CodeBlock>
      </details>
      <p className="mt-3 text-muted-foreground">{t.declare.why}</p>
      {waiting ? (
        <p role="status" className="mt-3 flex items-center gap-1.5 font-medium text-warn">
          <LoaderCircle className="size-3.5 animate-spin" aria-hidden />
          {t.declare.waiting}
        </p>
      ) : null}
    </div>
  )
}
