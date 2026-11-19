import { RefreshCw } from 'lucide-react'
import { Link } from 'react-router'

import { type Node, type NodeDetail, type NodeId, type PathLeg, type PathView } from '@/api/types'
import { Flag } from '@/components/flag'
import { InfoTip } from '@/components/info-tip'
import { OsIcon } from '@/components/os-icon'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Skeleton } from '@/components/ui/skeleton'
import { Tooltip } from '@/components/ui/tooltip'
import { t } from '@/i18n'
import { nodePath } from '@/lib/nodes'
import { formatAge } from '@/lib/time'
import { cn } from '@/lib/utils'
import { describeLeg } from '@/topology/path'

const d = t.diagnosis
const p = t.path

const BADGE_TONE = { ok: 'ok', idle: 'neutral', relay: 'relay', warn: 'warn', danger: 'danger' } as const
const LINE = { ok: 'bg-ok', relay: 'bg-relay', idle: 'bg-border-strong', warn: 'bg-warn', danger: 'bg-danger' } as const
const HEAD = {
  ok: 'text-ok',
  relay: 'text-relay',
  idle: 'text-border-strong',
  warn: 'text-warn',
  danger: 'text-danger',
} as const

/** 一端：国旗（或系统图标）、圆的 A / B、名字、中继的标签，下面虚拟 IP、在线、版本。点名字去节点详情。 */
function End({
  network,
  label,
  node,
  align,
}: {
  network: number
  label: string
  node: Node | NodeDetail | undefined
  align: 'left' | 'right'
}) {
  if (node === undefined) {
    return (
      <div className="space-y-2">
        <Skeleton className="h-5 w-28" />
        <Skeleton className="h-3.5 w-20" />
      </div>
    )
  }
  return (
    <div
      className={cn('flex min-w-0 items-center gap-3', align === 'right' && '@3xl:flex-row-reverse @3xl:text-right')}
    >
      <span
        className={cn(
          'relative grid size-12 shrink-0 place-items-center rounded-xl',
          node.relay !== null ? 'bg-relay-soft text-relay' : 'bg-muted text-muted-foreground',
          !node.online && 'grayscale',
        )}
      >
        {node.country !== null && node.country !== undefined ? (
          <Flag code={node.country} className="size-6" />
        ) : (
          <OsIcon
            platform={node.platform}
            className="size-5"
            fallback={<span className="text-base font-semibold uppercase">{node.name.slice(0, 1)}</span>}
          />
        )}
        <span
          aria-hidden
          className={cn(
            'absolute -right-1 -bottom-1 size-3 rounded-full ring-2 ring-card',
            node.online ? 'bg-ok' : 'bg-idle-dot',
          )}
        />
      </span>
      <div className="min-w-0">
        <p className={cn('flex min-w-0 items-center gap-2', align === 'right' && '@3xl:flex-row-reverse')}>
          <span className="grid size-5 shrink-0 place-items-center rounded-full bg-primary text-[11px] font-semibold text-primary-foreground">
            {label}
          </span>
          <Link
            to={nodePath(network, node.id)}
            className="truncate rounded-sm text-base font-semibold hover:text-primary hover:underline"
          >
            {node.name}
          </Link>
          {node.relay !== null ? (
            <span className="shrink-0 rounded-full bg-relay-soft px-1.5 text-[11px] leading-4 font-medium text-relay">
              {d.relay}
            </span>
          ) : null}
        </p>
        <p className="mt-1 truncate font-mono text-xs text-muted-foreground">{node.virtual_ip}</p>
        <p className="mt-0.5 truncate text-xs text-muted-foreground">
          {[node.online ? d.online : d.offline, node.version === '' ? null : node.version, node.platform]
            .filter((part) => part !== null && part !== '')
            .join(' · ')}
        </p>
      </div>
    </div>
  )
}

/**
 * 一个方向：上面一行是方向、路（颜色跟路走）、直连的方式或者到中继的传输、延迟、数据的来源；
 * 下面一根带箭头的线，A → B 箭头朝右，B → A 朝左。不是现问到的数据线是虚的。
 */
function Direction({ leg, name, reverse }: { leg: PathLeg; name: (id: NodeId) => string; reverse: boolean }) {
  const view = describeLeg(leg, name)
  const tone = view.route?.tone ?? 'idle'
  const live = view.state === 'query'
  return (
    <div data-testid={reverse ? 'path-reverse' : 'path-forward'} data-source={view.state}>
      <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
        <span className="text-xs text-muted-foreground">{p.forward(leg.from.name, leg.to.name)}</span>
        <span className={cn('text-sm font-semibold', view.route === null ? 'text-muted-foreground' : HEAD[tone])}>
          {view.route?.text ?? t.routes.noData}
        </span>
        {view.route?.detail !== null && view.route?.detail !== undefined ? (
          <Tooltip content={view.route.detailHint ?? undefined}>
            <Badge
              tone={leg.relay_leg === 'tcp' ? 'warn' : 'neutral'}
              tabIndex={view.route.detailHint === null ? undefined : 0}
            >
              {view.route.detail}
            </Badge>
          </Tooltip>
        ) : null}
        <span className="ml-auto flex items-center gap-2">
          {view.rtt !== null ? <span className="text-sm font-semibold tabular">{view.rtt}</span> : null}
          <Tooltip content={view.sourceHint}>
            <Badge tone={BADGE_TONE[view.sourceTone]} tabIndex={0}>
              {view.sourceLabel}
            </Badge>
          </Tooltip>
        </span>
      </div>
      <div className={cn('relative mt-2 flex items-center', reverse && 'flex-row-reverse')}>
        <span
          className={cn(
            'h-0.5 flex-1 rounded-full',
            live ? LINE[tone] : 'border-t-2 border-dashed border-border-strong',
          )}
        />
        <svg
          aria-hidden
          viewBox="0 0 10 10"
          className={cn('size-2.5 shrink-0', live ? HEAD[tone] : 'text-border-strong', reverse && 'rotate-180')}
        >
          <path d="M0 0 L10 5 L0 10 Z" fill="currentColor" />
        </svg>
      </div>
    </div>
  )
}

interface PairHeroProps {
  network: number
  a: Node | NodeDetail | undefined
  b: Node | NodeDetail | undefined
  path: PathView | undefined
  name: (id: NodeId) => string
  fetching: boolean
  /** 刚问到距现在多少秒；还没问到是 `null`。 */
  queriedAgo: number | null
  onRequery: () => void
}

/**
 * 顶上的一对：左边 A、右边 B，中间两个方向现在怎么走（刚问两端得到的）。下面一行是什么时候问的和重新查询。
 * 窄的时候三块上下排。
 */
export function PairHero({ network, a, b, path, name, fetching, queriedAgo, onRequery }: PairHeroProps) {
  return (
    <section className="rounded-xl border bg-card shadow-card" data-testid="pair-hero">
      <div className="grid grid-cols-1 items-center gap-6 px-6 py-5 @3xl:grid-cols-[minmax(0,15rem)_minmax(0,1fr)_minmax(0,15rem)]">
        <End network={network} label={d.nodeA} node={a} align="left" />
        <div className="space-y-4">
          {path === undefined ? (
            <>
              <Skeleton className="h-4 w-3/4" />
              <Skeleton className="h-4 w-2/3" />
            </>
          ) : (
            <>
              <Direction leg={path.forward} name={name} reverse={false} />
              <Direction leg={path.reverse} name={name} reverse />
            </>
          )}
        </div>
        <End network={network} label={d.nodeB} node={b} align="right" />
      </div>
      <div className="flex items-center justify-between gap-3 border-t px-6 py-2 text-xs text-muted-foreground">
        <span className="inline-flex items-center gap-1.5">
          {d.live}
          <InfoTip>{d.liveHint}</InfoTip>
          {queriedAgo !== null && !fetching ? <span>· {p.queried(formatAge(queriedAgo))}</span> : null}
          {fetching ? <span>· {p.querying}</span> : null}
        </span>
        <Button size="sm" variant="ghost" pending={fetching} onClick={onRequery}>
          {fetching ? null : <RefreshCw aria-hidden />}
          {p.query}
        </Button>
      </div>
    </section>
  )
}
