import { ArrowLeftRight, ChevronsUpDown, Plus, RefreshCw } from 'lucide-react'

import { type Node, type NodeDetail, type NodeId, type PathLeg, type PathView } from '@/api/types'
import { Combobox, type ComboOption } from '@/components/combobox'
import { Flag } from '@/components/flag'
import { InfoTip } from '@/components/info-tip'
import { OsIcon } from '@/components/os-icon'
import { OnlineDot } from '@/components/status'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Skeleton } from '@/components/ui/skeleton'
import { Tooltip } from '@/components/ui/tooltip'
import { t } from '@/i18n'
import { formatAge } from '@/lib/time'
import { cn } from '@/lib/utils'
import { type Pair } from '@/topology/pair'
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

/** 国旗（不知道国家时是系统的图标，再不知道是名字的头一个字），右下角在线的点。 */
export function NodeTile({ node, size = 'lg' }: { node: Node | NodeDetail; size?: 'lg' | 'md' }) {
  return (
    <span
      className={cn(
        'relative grid shrink-0 place-items-center',
        size === 'lg' ? 'size-12 rounded-xl' : 'size-9 rounded-lg',
        node.relay !== null ? 'bg-relay-soft text-relay' : 'bg-muted text-muted-foreground',
        !node.online && 'grayscale',
      )}
    >
      {node.country !== null && node.country !== undefined ? (
        <Flag code={node.country} className={size === 'lg' ? 'size-6' : 'size-5'} />
      ) : (
        <OsIcon
          platform={node.platform}
          className={size === 'lg' ? 'size-5' : 'size-4'}
          fallback={<span className="text-sm font-semibold uppercase">{node.name.slice(0, 1)}</span>}
        />
      )}
      <span
        aria-hidden
        className={cn(
          'absolute -right-1 -bottom-1 rounded-full ring-2 ring-card',
          size === 'lg' ? 'size-3' : 'size-2.5',
          node.online ? 'bg-ok' : 'bg-idle-dot',
        )}
      />
    </span>
  )
}

/** 圆的 A / B。 */
function Letter({ label, muted = false }: { label: string; muted?: boolean }) {
  return (
    <span
      className={cn(
        'grid size-5 shrink-0 place-items-center rounded-full text-[11px] font-semibold',
        muted ? 'bg-muted text-muted-foreground' : 'bg-primary text-primary-foreground',
      )}
    >
      {label}
    </span>
  )
}

/**
 * 一端：选了节点时是它的国旗、圆的 A / B、名字、中继的标签，下面虚拟 IP、在线、版本；没选时是一个虚线的方块
 * 和"选择节点 A"。整块是一个按钮，点了弹出可以搜索的节点列表，换一个。
 */
function End({
  label,
  pick,
  node,
  nodes,
  align,
  onPick,
}: {
  label: string
  /** "选择节点 A"：没选时写在上面，也是读屏的名字。 */
  pick: string
  node: Node | NodeDetail | undefined
  nodes: readonly Node[]
  align: 'left' | 'right'
  onPick: (id: NodeId) => void
}) {
  const options: ComboOption<NodeId>[] = nodes.map((option) => ({
    value: option.id,
    label: option.name,
    hint: option.online ? option.virtual_ip : `${option.virtual_ip} · ${t.common.offline}`,
    keywords: option.groups.map((group) => group.name).join(' '),
    leading: (
      <span className="flex items-center gap-1.5">
        <OnlineDot online={option.online} />
        <Flag code={option.country} />
      </span>
    ),
  }))
  const right = align === 'right'
  const content =
    node === undefined ? (
      <span className={cn('flex min-w-0 items-center gap-3', right && '@3xl:flex-row-reverse @3xl:text-right')}>
        <span className="grid size-12 shrink-0 place-items-center rounded-xl border-2 border-dashed text-faint transition-colors group-hover:border-primary/50 group-hover:text-primary">
          <Plus className="size-5" aria-hidden />
        </span>
        <span className={cn('flex min-w-0 items-center gap-2', right && '@3xl:flex-row-reverse')}>
          <Letter label={label} muted />
          <span className="text-[15px] font-medium text-muted-foreground group-hover:text-foreground">{pick}</span>
        </span>
      </span>
    ) : (
      <span className={cn('flex min-w-0 items-center gap-3', right && '@3xl:flex-row-reverse @3xl:text-right')}>
        <NodeTile node={node} />
        <span className="min-w-0">
          <span className={cn('flex min-w-0 items-center gap-2', right && '@3xl:flex-row-reverse')}>
            <Letter label={label} />
            <span className="truncate text-base font-semibold">{node.name}</span>
            {node.relay !== null ? (
              <span className="shrink-0 rounded-full bg-relay-soft px-1.5 text-[11px] leading-4 font-medium text-relay">
                {d.relay}
              </span>
            ) : null}
            <ChevronsUpDown
              className="size-3.5 shrink-0 text-faint opacity-0 transition-opacity group-hover:opacity-100 group-focus-visible:opacity-100"
              aria-hidden
            />
          </span>
          <span className="mt-1 block truncate font-mono text-xs text-muted-foreground">{node.virtual_ip}</span>
          <span className="mt-0.5 block truncate text-xs text-muted-foreground">
            {[node.online ? d.online : d.offline, node.version === '' ? null : node.version, node.platform]
              .filter((part) => part !== null && part !== '')
              .join(' · ')}
          </span>
        </span>
      </span>
    )
  return (
    <Combobox<NodeId>
      value={node?.id ?? null}
      onChange={onPick}
      options={options}
      placeholder={pick}
      searchPlaceholder={t.select.nodeSearch}
      emptyText={t.select.nodeEmpty}
      aria-label={pick}
      trigger={content}
      className="group -m-2 min-w-0 rounded-xl p-2 transition-colors hover:bg-muted/60 data-[state=open]:bg-muted/60"
    />
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
  /** 节点列表：两端的下拉里选的就是它们。 */
  nodes: readonly Node[]
  pair: Pair
  onChange: (pair: Pair) => void
  a: Node | NodeDetail | undefined
  b: Node | NodeDetail | undefined
  /** 两端都选好了、不是同一个时才有。 */
  path: PathView | undefined
  ready: boolean
  name: (id: NodeId) => string
  fetching: boolean
  /** 刚问到距现在多少秒；还没问到是 `null`。 */
  queriedAgo: number | null
  onRequery: () => void
}

/**
 * 顶上的一对，也是选节点的地方：左边 A、右边 B，点一端弹出节点列表换一个；中间是两个方向现在怎么走（刚问两端得到的），
 * 没选好时是一句该怎么选。选好了下面一行左边是什么时候问的，右边交换 A 和 B、重新查询。窄的时候三块上下排。
 */
export function PairHero({
  nodes,
  pair,
  onChange,
  a,
  b,
  path,
  ready,
  name,
  fetching,
  queriedAgo,
  onRequery,
}: PairHeroProps) {
  const same = pair.a !== null && pair.a === pair.b
  return (
    <section className="rounded-xl border bg-card shadow-card" data-testid="pair-hero">
      <div className="grid grid-cols-1 items-center gap-6 px-6 py-5 @3xl:grid-cols-[minmax(0,15rem)_minmax(0,1fr)_minmax(0,15rem)]">
        <End
          label={d.nodeA}
          pick={d.pickA}
          node={a}
          nodes={nodes}
          align="left"
          onPick={(id) => onChange({ ...pair, a: id })}
        />
        <div className="space-y-4">
          {!ready ? (
            <div className="flex flex-col items-center gap-2 text-center">
              <div className="flex w-full items-center gap-2" aria-hidden>
                <span className="h-0 flex-1 border-t-2 border-dashed" />
                <ArrowLeftRight className="size-4 text-faint" />
                <span className="h-0 flex-1 border-t-2 border-dashed" />
              </div>
              <p className={cn('text-[13px]', same ? 'text-warn' : 'text-muted-foreground')}>
                {same ? p.same : d.pickHint}
              </p>
            </div>
          ) : path === undefined ? (
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
        <End
          label={d.nodeB}
          pick={d.pickB}
          node={b}
          nodes={nodes}
          align="right"
          onPick={(id) => onChange({ ...pair, b: id })}
        />
      </div>
      {ready ? (
        <div className="flex min-h-11 items-center justify-between gap-3 border-t px-6 py-1.5 text-xs text-muted-foreground">
          <span className="inline-flex items-center gap-1.5">
            {d.live}
            <InfoTip>{d.liveHint}</InfoTip>
            {queriedAgo !== null && !fetching ? <span>· {p.queried(formatAge(queriedAgo))}</span> : null}
            {fetching ? <span>· {p.querying}</span> : null}
          </span>
          <span className="flex items-center gap-1">
            <Button size="sm" variant="ghost" onClick={() => onChange({ a: pair.b, b: pair.a })}>
              <ArrowLeftRight aria-hidden />
              {p.swap}
            </Button>
            <Button size="sm" variant="ghost" pending={fetching} onClick={onRequery}>
              {fetching ? null : <RefreshCw aria-hidden />}
              {p.query}
            </Button>
          </span>
        </div>
      ) : null}
    </section>
  )
}
