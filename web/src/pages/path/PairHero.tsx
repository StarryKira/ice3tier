import { ArrowLeftRight, ChevronDown, Plus, RefreshCw } from 'lucide-react'

import { type Node, type NodeDetail, type NodeId, type PathLeg, type PathView } from '@/api/types'
import { Combobox, type ComboOption } from '@/components/combobox'
import { Flag } from '@/components/flag'
import { OsIcon } from '@/components/os-icon'
import { OnlineDot } from '@/components/status'
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

const DOT = { ok: 'bg-ok', relay: 'bg-relay', idle: 'bg-idle-dot', warn: 'bg-warn', danger: 'bg-danger' } as const

/** 国旗（不知道国家时是系统的图标，再不知道是名字的头一个字），右下角在线的点。底色一律是灰的，不按角色上色。 */
export function NodeTile({ node, size = 'lg' }: { node: Node | NodeDetail; size?: 'lg' | 'md' }) {
  return (
    <span
      className={cn(
        'relative grid shrink-0 place-items-center bg-muted text-muted-foreground',
        size === 'lg' ? 'size-11 rounded-xl' : 'size-9 rounded-lg',
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
          'absolute -right-0.5 -bottom-0.5 size-2.5 rounded-full ring-2 ring-card',
          node.online ? 'bg-ok' : 'bg-idle-dot',
        )}
      />
    </span>
  )
}

/**
 * 一端：选了节点时是国旗、"A"和名字，下面虚拟 IP，再下面在线、中继、版本，都是灰字；没选时是一个虚线的方块
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
        <span className="grid size-11 shrink-0 place-items-center rounded-xl border border-dashed border-border-strong text-muted-foreground transition-colors group-hover:border-primary group-hover:text-primary">
          <Plus className="size-4" aria-hidden />
        </span>
        <span className="min-w-0">
          <span className="block text-xs font-medium text-muted-foreground">{label}</span>
          <span className="block text-sm font-medium text-muted-foreground group-hover:text-foreground">{pick}</span>
        </span>
      </span>
    ) : (
      <span className={cn('flex min-w-0 items-center gap-3', right && '@3xl:flex-row-reverse @3xl:text-right')}>
        <NodeTile node={node} />
        <span className="min-w-0">
          <span className={cn('flex min-w-0 items-baseline gap-1.5', right && '@3xl:flex-row-reverse')}>
            <span className="text-xs font-semibold text-muted-foreground">{label}</span>
            <span className="truncate text-[15px] font-semibold">{node.name}</span>
            <ChevronDown
              className="size-3.5 shrink-0 self-center text-faint opacity-0 transition-opacity group-hover:opacity-100 group-focus-visible:opacity-100"
              aria-hidden
            />
          </span>
          <span className="mt-0.5 block truncate font-mono text-xs text-muted-foreground">{node.virtual_ip}</span>
          <span className="block truncate text-xs text-muted-foreground">
            {[
              node.online ? d.online : d.offline,
              node.relay !== null ? d.relay : null,
              node.version === '' ? null : node.version,
            ]
              .filter((part) => part !== null)
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
      className="group -m-2 min-w-0 rounded-lg p-2 transition-colors hover:bg-muted/50 data-[state=open]:bg-muted/50"
    />
  )
}

/**
 * 一个方向：一行字（方向、一个颜色点和路、直连的方式或者到中继的传输、延迟），下面一根细的带箭头的线，
 * A → B 朝右，B → A 朝左。线一律是灰的，路的种类只看那个点；到中继走 TCP 时那几个字是琥珀色。
 * 不是刚问到的数据线是虚的，后面写一句是什么时候的（悬停看为什么）；刚问到的"实时"只给读屏。
 */
function Direction({ leg, name, reverse }: { leg: PathLeg; name: (id: NodeId) => string; reverse: boolean }) {
  const view = describeLeg(leg, name)
  const live = view.state === 'query'
  return (
    <div data-testid={reverse ? 'path-reverse' : 'path-forward'} data-source={view.state}>
      <div className="flex min-w-0 items-baseline gap-2 text-[13px]">
        <span className="shrink-0 text-xs text-muted-foreground">{p.forward(leg.from.name, leg.to.name)}</span>
        <span className="inline-flex min-w-0 items-center gap-1.5 font-medium">
          <span aria-hidden className={cn('size-1.5 shrink-0 rounded-full', DOT[view.route?.tone ?? 'idle'])} />
          <span className={cn('truncate', view.route === null && 'text-muted-foreground')}>
            {view.route?.text ?? t.routes.noData}
          </span>
        </span>
        {view.route?.detail !== null && view.route?.detail !== undefined ? (
          <Tooltip content={view.route.detailHint ?? undefined}>
            <span
              tabIndex={view.route.detailHint === null ? undefined : 0}
              className={cn('shrink-0 rounded-sm', leg.relay_leg === 'tcp' ? 'text-warn' : 'text-muted-foreground')}
            >
              {view.route.detail}
            </span>
          </Tooltip>
        ) : null}
        {live ? (
          <span className="sr-only">{view.sourceLabel}</span>
        ) : (
          <Tooltip content={view.sourceHint}>
            <span tabIndex={0} className="shrink-0 rounded-sm text-xs text-warn">
              {view.sourceLabel}
              {view.age !== null ? ` · ${view.age}` : null}
            </span>
          </Tooltip>
        )}
        <span className="ml-auto shrink-0 font-semibold tabular">{view.rtt ?? '—'}</span>
      </div>
      <div className={cn('mt-1.5 flex items-center text-border-strong', reverse && 'flex-row-reverse')}>
        <span className={cn('flex-1', live ? 'h-px bg-current' : 'border-t border-dashed border-current')} />
        <svg aria-hidden viewBox="0 0 10 10" className={cn('size-2 shrink-0', reverse && 'rotate-180')}>
          <path d="M0 0 L10 5 L0 10 Z" fill="currentColor" />
        </svg>
      </div>
    </div>
  )
}

interface PairRowProps {
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
 * 诊断头的第一行，也是选节点的地方：左边 A、右边 B，点一端换一个；中间是两个方向现在怎么走（刚问两端得到的），
 * 上面一行小字是什么时候问的，右边两个小图标：重新查询、交换 A 和 B。没选好时中间是一句该怎么选。窄的时候三块上下排。
 */
export function PairRow({
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
}: PairRowProps) {
  const same = pair.a !== null && pair.a === pair.b
  return (
    <div
      className="grid grid-cols-1 items-center gap-x-8 gap-y-5 px-6 py-5 @3xl:grid-cols-[minmax(0,14rem)_minmax(0,1fr)_minmax(0,14rem)]"
      data-testid="pair-hero"
    >
      <End
        label={d.nodeA}
        pick={d.pickA}
        node={a}
        nodes={nodes}
        align="left"
        onPick={(id) => onChange({ ...pair, a: id })}
      />
      <div className="min-w-0">
        {!ready ? (
          <p className={cn('text-center text-[13px]', same ? 'text-warn' : 'text-muted-foreground')}>
            {same ? p.same : d.pickHint}
          </p>
        ) : (
          <div className="space-y-3">
            <div className="flex items-center justify-between gap-2 text-xs text-muted-foreground">
              <Tooltip content={d.liveHint}>
                <span tabIndex={0} className="rounded-sm">
                  {fetching
                    ? p.querying
                    : queriedAgo === null
                      ? d.live
                      : `${d.live} · ${p.queried(formatAge(queriedAgo))}`}
                </span>
              </Tooltip>
              <span className="flex items-center">
                <Tooltip content={p.query}>
                  <Button size="icon-sm" variant="ghost" pending={fetching} onClick={onRequery} aria-label={p.query}>
                    {fetching ? null : <RefreshCw aria-hidden />}
                  </Button>
                </Tooltip>
                <Tooltip content={p.swap}>
                  <Button
                    size="icon-sm"
                    variant="ghost"
                    onClick={() => onChange({ a: pair.b, b: pair.a })}
                    aria-label={p.swap}
                  >
                    <ArrowLeftRight aria-hidden />
                  </Button>
                </Tooltip>
              </span>
            </div>
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
  )
}
