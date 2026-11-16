import { useQuery } from '@tanstack/react-query'
import { CircleCheck, CircleMinus, TriangleAlert } from 'lucide-react'
import { type ReactNode, useState } from 'react'
import { Link } from 'react-router'

import { q } from '@/api/queries'
import { type Network, type Node, type NodePair } from '@/api/types'
import { NodeLink } from '@/components/node'
import { nodePath } from '@/lib/nodes'
import { Page } from '@/components/page'
import { ErrorState, SkeletonBlock } from '@/components/states'
import { RelativeTime } from '@/components/status'
import { Button } from '@/components/ui/button'
import { Card, CardHeader } from '@/components/ui/card'
import { t } from '@/i18n'
import { GUIDE_INTERVAL } from '@/lib/polling'
import { useDocumentTitle } from '@/lib/useDocumentTitle'
import { cn } from '@/lib/utils'
import { forwardLabel, forwardStatusView } from '@/pages/forwards/status'
import { networkPath, useNetwork } from '@/pages/networks/context'
import { AddNodeDialog } from '@/pages/nodes/AddNodeDialog'

import { GettingStarted } from './GettingStarted'

const o = t.overview

/** "最近的操作"取多少条来挑：操作记录不分网络，要多取一些才凑得出这个网络的最近几条。 */
const RECENT_FETCH = 100
const RECENT_SHOWN = 5
const PAIRS_SHOWN = 10

interface AttentionItem {
  key: string
  text: string
  action: ReactNode
  detail?: ReactNode
}

function Stat({ label, value, hint, to }: { label: string; value: ReactNode; hint?: string; to: string }) {
  return (
    <Link
      to={to}
      aria-label={o.stats.open(label)}
      className="rounded-lg border bg-card px-4 py-3.5 shadow-card transition-colors hover:border-primary/50"
    >
      <span className="block text-[13px] text-muted-foreground">{label}</span>
      <span className="mt-1 block text-2xl leading-tight font-semibold tabular">{value}</span>
      {hint !== undefined ? <span className="block text-xs text-muted-foreground">{hint}</span> : null}
    </Link>
  )
}

/** 没有 relay 时两端互不可达：这是唯一需要用户再做事的情况，展开一段补救的步骤。 */
function RelayFix({ network, onAddNode }: { network: Network; onAddNode: () => void }) {
  return (
    <div className="mt-2 space-y-2 rounded-md bg-muted/70 px-4 py-3 text-[13px]">
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
    <li className="flex flex-wrap items-center justify-between gap-3 px-5 py-2.5 text-sm">
      <span className="inline-flex flex-wrap items-center gap-2">
        <NodeLink network={network.id} node={pair.a} online={online(pair.a.id)} />
        <span className="text-muted-foreground">⇹</span>
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
  const log = useQuery({ ...q.log(RECENT_FETCH), enabled: !few })

  if (nodes.isError && nodes.data === undefined) {
    return (
      <Page>
        <Card>
          <ErrorState title={o.loadFailed} error={nodes.error} onRetry={() => void nodes.refetch()} />
        </Card>
      </Page>
    )
  }
  if (nodes.data === undefined) {
    return (
      <Page>
        <Card>
          <SkeletonBlock lines={6} />
        </Card>
      </Page>
    )
  }
  if (few) return <GettingStarted network={network} nodes={guideNodes.data ?? nodes.data} />

  if (overview.isError && overview.data === undefined) {
    return (
      <Page>
        <Card>
          <ErrorState title={o.loadFailed} error={overview.error} onRetry={() => void overview.refetch()} />
        </Card>
      </Page>
    )
  }
  const data = overview.data
  if (data === undefined) {
    return (
      <Page>
        <Card>
          <SkeletonBlock lines={6} />
        </Card>
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
          <Button size="sm" aria-expanded={fixOpen === key} onClick={() => setFixOpen(fixOpen === key ? null : key)}>
            {fixOpen === key ? o.hideFix : o.howToFix}
          </Button>
        ),
        detail: fixOpen === key ? <RelayFix network={network} onAddNode={() => setAdding(true)} /> : undefined,
      })
    } else {
      attention.push({
        key,
        text: o.unreachableWithRelay(pair.a.name, pair.b.name),
        action: (
          <Button asChild size="sm">
            <Link to={`${nodePath(network.id, pair.a.id)}?tab=routes`}>{o.viewRoutes}</Link>
          </Button>
        ),
      })
    }
  }
  for (const node of all) {
    if (node.online) continue
    if (node.relay !== null || node.exit !== null) {
      attention.push({
        key: `offline:${node.id}`,
        text: node.relay !== null ? o.relayOffline(node.name) : o.exitOffline(node.name),
        action: (
          <Button asChild size="sm">
            <Link to={nodePath(network.id, node.id)}>{o.viewNode}</Link>
          </Button>
        ),
      })
    }
  }
  for (const forward of forwards.data ?? []) {
    const view = forwardStatusView(forward, all)
    if (!view.needsAttention) continue
    attention.push({
      key: `forward:${forward.id}`,
      text: o.forwardBlocked(forwardLabel(forward), view.detail),
      action: (
        <Button asChild size="sm">
          <Link to={networkPath(network.id, view.linkToRules ? 'access' : 'forwards')}>
            {view.linkToRules ? o.viewRules : o.view}
          </Link>
        </Button>
      ),
    })
  }

  const recent = (log.data ?? []).filter((entry) => entry.network === network.id).slice(0, RECENT_SHOWN)
  const healthy = attention.length === 0
  const allOffline = data.online === 0
  const StatusIcon = !healthy ? TriangleAlert : allOffline ? CircleMinus : CircleCheck

  return (
    <Page>
      {/* 第一行：一句话的状态。这是这个页面一眼要看到的东西。 */}
      <header className="mb-6 flex items-center gap-3.5">
        <span
          className={cn(
            'flex size-10 shrink-0 items-center justify-center rounded-full',
            !healthy ? 'bg-warn-soft text-warn' : allOffline ? 'bg-idle-soft text-idle' : 'bg-ok-soft text-ok',
          )}
        >
          <StatusIcon className="size-5" aria-hidden />
        </span>
        <div>
          <h1 className="text-xl font-semibold tracking-tight" data-testid="overview-status">
            {!healthy
              ? o.statusIssues(attention.length)
              : allOffline
                ? o.statusAllOffline(data.nodes)
                : o.statusOk(data.online, data.nodes)}
          </h1>
          {!healthy ? (
            <p className="mt-0.5 text-sm text-muted-foreground">{o.statusSub(data.online, data.nodes)}</p>
          ) : null}
        </div>
      </header>

      {attention.length > 0 ? (
        <Card className="mb-6 border-warn-line">
          <CardHeader title={o.attention} className="border-warn-line/60" />
          <ul className="divide-y">
            {attention.map((item) => (
              <li key={item.key} className="px-5 py-3">
                <div className="flex items-center justify-between gap-4">
                  <p className="text-sm">{item.text}</p>
                  <div className="shrink-0">{item.action}</div>
                </div>
                {item.detail}
              </li>
            ))}
          </ul>
        </Card>
      ) : null}

      <section aria-label={o.title} className="mb-6 grid grid-cols-3 gap-3 lg:grid-cols-6">
        <Stat label={o.stats.online} value={`${data.online} / ${data.nodes}`} to={networkPath(network.id, 'nodes')} />
        <Stat
          label={o.stats.relays}
          value={data.relays}
          hint={o.stats.relaysHint}
          to={`${networkPath(network.id, 'nodes')}?role=relay`}
        />
        <Stat label={o.stats.exits} value={data.exits} hint={o.stats.exitsHint} to={networkPath(network.id, 'exits')} />
        <Stat label={o.stats.rules} value={data.rules} to={networkPath(network.id, 'access')} />
        <Stat label={o.stats.forwards} value={data.forwards} to={networkPath(network.id, 'forwards')} />
        <Stat label={o.stats.tokens} value={data.pending_tokens} to={networkPath(network.id, 'tokens')} />
      </section>

      <div className="grid grid-cols-1 items-start gap-5 lg:grid-cols-2">
        <Card>
          <CardHeader title={o.unreachableTitle} description={o.unreachableLead} />
          {data.unreachable_pairs.length === 0 ? (
            <div className="px-5 py-4">
              <p className="text-sm">{o.unreachableNone}</p>
              <p className="mt-1 text-[13px] text-muted-foreground">{o.unreachableNoneHint}</p>
            </div>
          ) : (
            <ul className="divide-y">
              {data.unreachable_pairs.slice(0, PAIRS_SHOWN).map((pair) => (
                <PairRow key={`${pair.a.id}:${pair.b.id}`} network={network} pair={pair} nodes={all} />
              ))}
              {data.unreachable_pairs.length > PAIRS_SHOWN ? (
                <li className="px-5 py-2.5 text-[13px] text-muted-foreground">
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
              <Button asChild size="sm" variant="ghost">
                <Link to="/settings/log">{o.recentAll}</Link>
              </Button>
            }
          />
          {log.isPending ? (
            <SkeletonBlock lines={3} />
          ) : recent.length === 0 ? (
            <p className="px-5 py-4 text-sm text-muted-foreground">{o.recentEmpty}</p>
          ) : (
            <ul className="divide-y">
              {recent.map((entry) => (
                <li key={entry.id} className="grid grid-cols-[5.5rem_minmax(0,1fr)] items-baseline gap-3 px-5 py-2.5">
                  <RelativeTime at={entry.at} className="text-xs text-muted-foreground" />
                  <span className="min-w-0">
                    <span className="block text-[13px] font-medium">
                      {t.settings.log.actions[entry.action] ?? entry.action}
                    </span>
                    <span className="block truncate text-[13px] text-muted-foreground" title={entry.detail}>
                      {entry.detail}
                    </span>
                  </span>
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
