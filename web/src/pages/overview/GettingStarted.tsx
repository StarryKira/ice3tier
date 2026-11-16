import { Check } from 'lucide-react'
import { type ReactNode } from 'react'
import { Link } from 'react-router'

import { type Network, type Node } from '@/api/types'
import { nodePath } from '@/lib/nodes'
import { Notice } from '@/components/notice'
import { Page, PageHeader } from '@/components/page'
import { OnlineDot } from '@/components/status'
import { Card } from '@/components/ui/card'
import { t } from '@/i18n'
import { cn } from '@/lib/utils'
import { AddNodeFlow } from '@/pages/nodes/AddNodeFlow'

const g = t.overview.guide

type StepState = 'done' | 'current' | 'upcoming'

function Step({
  index,
  title,
  state,
  children,
}: {
  index: number
  title: string
  state: StepState
  children?: ReactNode
}) {
  return (
    <li aria-current={state === 'current' ? 'step' : undefined} className="relative pb-8 last:pb-0">
      <div className="flex items-center gap-3">
        <span
          aria-hidden
          className={cn(
            'flex size-7 shrink-0 items-center justify-center rounded-full text-[13px] font-semibold tabular',
            state === 'done' && 'bg-muted text-foreground',
            state === 'current' && 'bg-primary text-primary-foreground',
            state === 'upcoming' && 'border border-border-strong text-muted-foreground',
          )}
        >
          {state === 'done' ? <Check className="size-4" /> : index}
        </span>
        <h2 className={cn('text-base font-semibold', state === 'upcoming' && 'font-medium text-muted-foreground')}>
          {title}
        </h2>
        {state === 'done' ? <span className="text-[13px] text-muted-foreground">{g.done}</span> : null}
        {state === 'current' ? <span className="sr-only">{g.current}</span> : null}
      </div>
      {children !== undefined && state !== 'done' ? <div className="mt-4 pl-10">{children}</div> : null}
    </li>
  )
}

/** 右侧的节点卡片：节点没来时是虚线框，来了之后变成实卡。 */
function Slot({
  network,
  label,
  node,
  waiting,
}: {
  network: Network
  label: string
  node: Node | undefined
  waiting: string
}) {
  if (node === undefined) {
    return (
      <div className="flex h-24 flex-col items-center justify-center rounded-lg border border-dashed border-border-strong px-4 text-center">
        <span className="text-xs text-muted-foreground">{label}</span>
        <span className="mt-1 text-[13px] text-muted-foreground">{waiting}</span>
      </div>
    )
  }
  return (
    <Link
      to={nodePath(network.id, node.id)}
      className="flex h-24 animate-rise flex-col justify-center rounded-lg border border-border-strong bg-card px-4 transition-colors hover:border-foreground/30"
      data-testid={`guide-node-${node.name}`}
    >
      <span className="flex items-center gap-2 text-sm font-semibold">
        <OnlineDot online={node.online} />
        {node.name}
      </span>
      <span className="mt-0.5 font-mono text-xs text-muted-foreground">
        {node.virtual_ip} · {node.platform}
      </span>
      <span className="mt-1 text-xs text-muted-foreground">{node.online ? g.joinedOnline : g.joinedOffline}</span>
    </Link>
  )
}

/**
 * 节点不足两个时，总览页就是引导：不另做一个全屏向导。
 * 左边是编号的步骤，右边是实况。用户走开再回来它还在原地，进度由数据决定，不需要记"做到第几步"；
 * 用命令行加的节点这里一样会发现。
 */
export function GettingStarted({ network, nodes }: { network: Network; nodes: readonly Node[] }) {
  const ordered = [...nodes].sort((a, b) => a.created_at - b.created_at || a.id - b.id)
  const [first, second] = ordered
  const current = first === undefined ? 1 : 2

  return (
    <Page>
      <PageHeader title={g.title} />
      <div className="grid grid-cols-[minmax(0,1fr)_16rem] items-start gap-12">
        <ol>
          <Step index={1} title={g.step1} state={current === 1 ? 'current' : 'done'}>
            <AddNodeFlow key="first" network={network} />
          </Step>
          <Step index={2} title={g.step2} state={current === 2 ? 'current' : 'upcoming'}>
            {current === 2 ? (
              <AddNodeFlow
                key="second"
                network={network}
                intro={<Notice>{g.step2Tip(t.capability.relay.full)}</Notice>}
              />
            ) : undefined}
          </Step>
          <Step index={3} title={g.step3} state="upcoming">
            <p className="text-sm text-muted-foreground">{g.step3Body}</p>
          </Step>
        </ol>

        <Card className="sticky top-8" aria-label={g.live}>
          <h2 className="mb-3 text-xs font-medium text-muted-foreground">{g.live}</h2>
          <div aria-live="polite">
            <Slot network={network} label={g.slotFirst} node={first} waiting={g.waiting} />
            <div aria-hidden className="mx-auto h-6 w-px border-l border-dashed border-border-strong" />
            <Slot
              network={network}
              label={g.slotSecond}
              node={second}
              waiting={first === undefined ? g.notYet : g.waiting}
            />
          </div>
        </Card>
      </div>
    </Page>
  )
}
