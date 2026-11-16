import { useQuery } from '@tanstack/react-query'
import { ChevronRight } from 'lucide-react'
import { type ReactNode, useState } from 'react'
import { Link } from 'react-router'

import { q } from '@/api/queries'
import { type Network, type Node, type NodePair } from '@/api/types'
import { NodeLink } from '@/components/node'
import { nodePath } from '@/lib/nodes'
import { Page } from '@/components/page'
import { ErrorState, SkeletonBlock } from '@/components/states'
import { RelativeTime, StatusDot } from '@/components/status'
import { Button } from '@/components/ui/button'
import { Card, CardHeader } from '@/components/ui/card'
import { Tooltip } from '@/components/ui/tooltip'
import { t } from '@/i18n'
import { formatBytes } from '@/lib/format'
import { listenView } from '@/lib/listenState'
import { nodeName } from '@/lib/nodes'
import { GUIDE_INTERVAL } from '@/lib/polling'
import { quotaView } from '@/lib/usage'
import { useDocumentTitle } from '@/lib/useDocumentTitle'
import { exitHealthView } from '@/pages/exits/exitView'
import { forwardLabel, forwardStatusView } from '@/pages/forwards/status'
import { networkPath, useNetwork } from '@/pages/networks/context'
import { AddNodeDialog } from '@/pages/nodes/AddNodeDialog'

import { GettingStarted } from './GettingStarted'

const o = t.overview

/** "最近的操作"：controller 按网络筛好，只取这么几条。 */
const RECENT_SHOWN = 5
const PAIRS_SHOWN = 10

interface AttentionItem {
  key: string
  text: string
  action: ReactNode
  detail?: ReactNode
}

interface StatProps {
  label: string
  value: ReactNode
  hint?: string
  /** 悬停说明这个数是怎么来的。 */
  tip?: string
  to: string
}

/** 总览最上面的一个数：灰色的名字、28 像素的数、下面一行灰色的补充。不画框，整块是去那一页的链接。 */
function Stat({ label, value, hint, tip, to }: StatProps) {
  const link = (
    <Link
      to={to}
      aria-label={o.stats.open(label)}
      className="group -mx-3 -my-2 block rounded-md px-3 py-2 transition-colors hover:bg-muted/70"
    >
      <span className="block text-[13px] text-muted-foreground">{label}</span>
      <span className="mt-1 block truncate text-[28px] leading-9 font-semibold tracking-tight">{value}</span>
      <span className="block h-4 truncate text-xs text-muted-foreground tabular">{hint ?? ''}</span>
    </Link>
  )
  return tip === undefined ? link : <Tooltip content={tip}>{link}</Tooltip>
}

/** 第二层的数：一行小字，名字灰、数黑，各自是去那一页的链接。 */
function MinorStat({ label, value, to }: { label: string; value: number; to: string }) {
  return (
    <Link
      to={to}
      aria-label={o.stats.open(label)}
      className="inline-flex items-baseline gap-1.5 rounded-sm text-[13px] text-muted-foreground hover:text-foreground"
    >
      {label}
      <span className="font-medium text-foreground tabular">{value}</span>
    </Link>
  )
}

/** 一条"需要处理"后面的那个动作：安静的文字按钮，带一个向右的小箭头。 */
function GoLink({ to, children }: { to: string; children: ReactNode }) {
  return (
    <Button asChild size="sm" variant="ghost" className="text-muted-foreground hover:text-foreground">
      <Link to={to}>
        {children}
        <ChevronRight aria-hidden />
      </Link>
    </Button>
  )
}

/** 没有 relay 时两端互不可达：这是唯一需要用户再做事的情况，展开一段补救的步骤。 */
function RelayFix({ network, onAddNode }: { network: Network; onAddNode: () => void }) {
  return (
    <div className="mt-3 space-y-2 border-l-2 pl-4 text-[13px]">
      <p>{o.fix.why}</p>
      <ul className="list-disc space-y-1 pl-4 text-muted-foreground">
        <li>{o.fix.optionA}</li>
        <li>{o.fix.optionB}</li>
      </ul>
      <div className="flex gap-2 pt-1">
        <Button asChild size="sm">
          <Link to={networkPath(network.id, 'nodes')}>{t.exits.goNodes}</Link>
        </Button>
        <Button size="sm" onClick={onAddNode}>
          {o.fix.addNode}
        </Button>
      </div>
    </div>
  )
}

function PairRow({ network, pair, nodes }: { network: Network; pair: NodePair; nodes: readonly Node[] }) {
  const online = (id: number) => nodes.find((node) => node.id === id)?.online
  return (
    <li className="flex flex-wrap items-center justify-between gap-3 py-2.5 text-sm">
      <span className="inline-flex flex-wrap items-center gap-2">
        <NodeLink network={network.id} node={pair.a} online={online(pair.a.id)} />
        <span className="text-faint">⇹</span>
        <NodeLink network={network.id} node={pair.b} online={online(pair.b.id)} />
      </span>
      <Link
        to={`${nodePath(network.id, pair.a.id)}?tab=routes`}
        className="rounded-sm text-[13px] text-primary underline-offset-4 hover:underline"
      >
        {o.viewRoutes}
      </Link>
    </li>
  )
}

/**
 * 总览：打开界面的第一眼——一切正常，还是有事要处理。多数时候只是看一眼然后去别处。
 * 节点不足两个时，整页是引导。
 */
export function OverviewPage() {
  useDocumentTitle(o.title)
  const network = useNetwork()
  const [adding, setAdding] = useState(false)
  const [fixOpen, setFixOpen] = useState<string | null>(null)

  const nodes = useQuery(q.nodes(network.id))
  const few = nodes.data !== undefined && nodes.data.length < 2
  // 引导期间一直在等新节点出现，取得勤一些。
  const guideNodes = useQuery({ ...q.nodes(network.id), refetchInterval: GUIDE_INTERVAL, enabled: few })
  const overview = useQuery({ ...q.overview(network.id), enabled: !few })
  const forwards = useQuery({ ...q.forwards(network.id), enabled: !few })
  const exits = useQuery({ ...q.exits(network.id), enabled: !few })
  const credentials = useQuery({ ...q.credentials(network.id), enabled: !few })
  const listeners = useQuery({ ...q.listeners(network.id), enabled: !few })
  const log = useQuery({ ...q.log({ network: network.id, limit: RECENT_SHOWN }), enabled: !few })

  if (nodes.isError && nodes.data === undefined) {
    return (
      <Page>
        <ErrorState title={o.loadFailed} error={nodes.error} onRetry={() => void nodes.refetch()} />
      </Page>
    )
  }
  if (nodes.data === undefined) {
    return (
      <Page>
        <SkeletonBlock lines={6} />
      </Page>
    )
  }
  if (few) return <GettingStarted network={network} nodes={guideNodes.data ?? nodes.data} />

  if (overview.isError && overview.data === undefined) {
    return (
      <Page>
        <ErrorState title={o.loadFailed} error={overview.error} onRetry={() => void overview.refetch()} />
      </Page>
    )
  }
  const data = overview.data
  if (data === undefined) {
    return (
      <Page>
        <SkeletonBlock lines={6} />
      </Page>
    )
  }

  const all = nodes.data
  const attention: AttentionItem[] = []

  for (const pair of data.unreachable_pairs) {
    const key = `pair:${pair.a.id}:${pair.b.id}`
    if (data.relays === 0) {
      attention.push({
        key,
        text: o.unreachableNoRelay(pair.a.name, pair.b.name),
        action: (
          <Button
            size="sm"
            variant="ghost"
            className="text-muted-foreground hover:text-foreground"
            aria-expanded={fixOpen === key}
            onClick={() => setFixOpen(fixOpen === key ? null : key)}
          >
            {fixOpen === key ? o.hideFix : o.howToFix}
          </Button>
        ),
        detail: fixOpen === key ? <RelayFix network={network} onAddNode={() => setAdding(true)} /> : undefined,
      })
    } else {
      attention.push({
        key,
        text: o.unreachableWithRelay(pair.a.name, pair.b.name),
        action: <GoLink to={`${nodePath(network.id, pair.a.id)}?tab=routes`}>{o.viewRoutes}</GoLink>,
      })
    }
  }
  for (const node of all) {
    if (node.online) continue
    if (node.relay !== null || node.exit !== null) {
      attention.push({
        key: `offline:${node.id}`,
        text: node.relay !== null ? o.relayOffline(node.name) : o.exitOffline(node.name),
        action: <GoLink to={nodePath(network.id, node.id)}>{o.viewNode}</GoLink>,
      })
    }
  }
  for (const exit of exits.data ?? []) {
    const quota = exit.usage?.quota
    const view = quota === undefined || quota === null ? null : quotaView(quota)
    const health = exitHealthView(exit)
    const text =
      view?.exhausted === true
        ? o.quotaExhausted(exit.node.name, view.resets)
        : view?.nearly === true
          ? o.quotaNearly(exit.node.name, view.percent ?? 0)
          : exit.health === 'degraded'
            ? o.exitDegraded(exit.node.name, health.detail)
            : null
    if (text === null) continue
    attention.push({
      key: `exit:${exit.node.id}`,
      text,
      action: <GoLink to={`${networkPath(network.id, 'exits')}?focus=${exit.node.id}`}>{o.view}</GoLink>,
    })
  }
  for (const credential of credentials.data ?? []) {
    if (credential.state !== 'exhausted') continue
    attention.push({
      key: `credential:${credential.id}`,
      text: o.credentialExhausted(credential.name),
      action: <GoLink to={`${networkPath(network.id, 'credentials')}?focus=${credential.id}`}>{o.view}</GoLink>,
    })
  }
  for (const listener of listeners.data ?? []) {
    const host = all.find((node) => node.id === listener.node)
    const view = listenView(listener.status, {
      node: nodeName(all, listener.node),
      bind: listener.bind,
      online: host?.online ?? false,
    })
    if (!view.needsAttention) continue
    attention.push({
      key: `listener:${listener.id}`,
      text: o.listenerBroken(listener.name, view.detail ?? view.label),
      action: <GoLink to={`${networkPath(network.id, 'listeners')}?focus=${listener.id}`}>{o.view}</GoLink>,
    })
  }
  for (const forward of forwards.data ?? []) {
    const view = forwardStatusView(forward, all)
    if (!view.needsAttention) continue
    attention.push({
      key: `forward:${forward.id}`,
      text: o.forwardBlocked(forwardLabel(forward), view.detail),
      action: (
        <GoLink
          to={
            view.linkToRules
              ? networkPath(network.id, 'access')
              : `${networkPath(network.id, 'forwards')}?focus=${forward.id}`
          }
        >
          {view.linkToRules ? o.viewRules : o.view}
        </GoLink>
      ),
    })
  }

  const recent = log.data ?? []
  const healthy = attention.length === 0
  const allOffline = data.online === 0

  return (
    <Page>
      {/* 第一行：一句话的状态。这是这个页面一眼要看到的东西；有事时紧接着就是那几件事。 */}
      <header className="mb-8">
        <h1 className="flex items-center gap-3 text-xl leading-7 font-semibold tracking-tight">
          <StatusDot tone={!healthy ? 'warn' : allOffline ? 'idle' : 'ok'} className="[&>span]:size-2.5" />
          <span data-testid="overview-status">
            {!healthy
              ? o.statusIssues(attention.length)
              : allOffline
                ? o.statusAllOffline(data.nodes)
                : o.statusOk(data.online, data.nodes)}
          </span>
        </h1>
        {!healthy ? (
          <p className="mt-1 pl-5.5 text-[13px] text-muted-foreground">{o.statusSub(data.online, data.nodes)}</p>
        ) : null}
      </header>

      {attention.length > 0 ? (
        <section aria-label={o.attention} className="mb-12">
          <h2 className="sr-only">{o.attention}</h2>
          <ul className="border-y">
            {attention.map((item) => (
              <li key={item.key} className="border-b py-3 last:border-b-0">
                <div className="flex items-center justify-between gap-6">
                  <p className="flex min-w-0 items-baseline gap-3 text-sm">
                    <StatusDot tone="warn" className="relative -top-px" />
                    <span className="min-w-0">{item.text}</span>
                  </p>
                  <div className="shrink-0">{item.action}</div>
                </div>
                {item.detail !== undefined ? <div className="pl-5">{item.detail}</div> : null}
              </li>
            ))}
          </ul>
        </section>
      ) : null}

      <section aria-label={o.title} className="mb-12">
        <div className="grid grid-cols-4 gap-x-10">
          <Stat
            label={o.stats.online}
            value={
              <>
                {data.online}
                <span className="text-xl font-normal text-muted-foreground"> / {data.nodes}</span>
              </>
            }
            to={networkPath(network.id, 'nodes')}
          />
          <Stat
            label={o.stats.flows}
            value={data.flows}
            hint={o.stats.flowsHint}
            to={networkPath(network.id, 'topology')}
          />
          <Stat
            label={o.stats.traffic}
            value={formatBytes(data.tx_bytes + data.rx_bytes)}
            hint={o.stats.trafficHint(formatBytes(data.tx_bytes), formatBytes(data.rx_bytes))}
            tip={o.stats.trafficTip}
            to={networkPath(network.id, 'nodes')}
          />
          <Stat
            label={o.stats.relays}
            value={data.relays}
            hint={o.stats.relaysHint}
            to={`${networkPath(network.id, 'nodes')}?role=relay`}
          />
        </div>
        <div className="mt-6 flex flex-wrap items-center gap-x-8 gap-y-2 border-t pt-4">
          <MinorStat label={o.stats.exits} value={data.exits} to={networkPath(network.id, 'exits')} />
          <MinorStat label={o.stats.rules} value={data.rules} to={networkPath(network.id, 'access')} />
          <MinorStat label={o.stats.forwards} value={data.forwards} to={networkPath(network.id, 'forwards')} />
          <MinorStat label={o.stats.tokens} value={data.pending_tokens} to={networkPath(network.id, 'tokens')} />
        </div>
      </section>

      <div className="grid grid-cols-1 items-start gap-x-12 gap-y-10 lg:grid-cols-2">
        <Card>
          <CardHeader title={o.unreachableTitle} description={o.unreachableLead} />
          {data.unreachable_pairs.length === 0 ? (
            <div className="border-t pt-3">
              <p className="text-sm">{o.unreachableNone}</p>
              <p className="mt-1 text-[13px] text-muted-foreground">{o.unreachableNoneHint}</p>
            </div>
          ) : (
            <ul className="divide-y border-t">
              {data.unreachable_pairs.slice(0, PAIRS_SHOWN).map((pair) => (
                <PairRow key={`${pair.a.id}:${pair.b.id}`} network={network} pair={pair} nodes={all} />
              ))}
              {data.unreachable_pairs.length > PAIRS_SHOWN ? (
                <li className="py-2.5 text-[13px] text-muted-foreground">
                  {t.access.graph.more(data.unreachable_pairs.length - PAIRS_SHOWN)}
                </li>
              ) : null}
            </ul>
          )}
        </Card>

        <Card>
          <CardHeader
            title={o.recentTitle}
            action={
              <Button asChild size="sm" variant="ghost" className="-my-1 text-muted-foreground hover:text-foreground">
                <Link to={`/settings/log?network=${network.id}`}>
                  {o.recentAll}
                  <ChevronRight aria-hidden />
                </Link>
              </Button>
            }
          />
          {log.isPending ? (
            <SkeletonBlock lines={3} />
          ) : recent.length === 0 ? (
            <p className="border-t pt-3 text-sm text-muted-foreground">{o.recentEmpty}</p>
          ) : (
            <ul className="divide-y border-t">
              {recent.map((entry) => (
                <li key={entry.id} className="grid grid-cols-[minmax(0,1fr)_auto] items-baseline gap-4 py-2.5">
                  <span className="min-w-0">
                    <span className="block text-sm">{t.settings.log.actions[entry.action] ?? entry.action}</span>
                    <span className="block truncate text-[13px] text-muted-foreground" title={entry.detail}>
                      {entry.detail}
                    </span>
                  </span>
                  <RelativeTime at={entry.at} className="text-xs text-muted-foreground" />
                </li>
              ))}
            </ul>
          )}
        </Card>
      </div>

      <AddNodeDialog network={network} open={adding} onClose={() => setAdding(false)} />
    </Page>
  )
}
