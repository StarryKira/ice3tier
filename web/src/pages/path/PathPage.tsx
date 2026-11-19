import { useQuery } from '@tanstack/react-query'
import { Route as RouteIcon, Waypoints } from 'lucide-react'
import { useMemo, useState } from 'react'
import { Link, useSearchParams } from 'react-router'

import { api } from '@/api/endpoints'
import { q, useAction } from '@/api/queries'
import { type NodeId } from '@/api/types'
import { Notice } from '@/components/notice'
import { Page, PageHeader } from '@/components/page'
import { PairPicker } from '@/components/pair-picker'
import { EmptyState, ErrorState } from '@/components/states'
import { Button } from '@/components/ui/button'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { t } from '@/i18n'
import { isBoolean, readStored, writeStored } from '@/lib/storage'
import { useDocumentTitle } from '@/lib/useDocumentTitle'
import { useNow } from '@/lib/useNow'
import { networkPath, useNetwork } from '@/pages/networks/context'
import { type DiagnosisInput, findings, hostOf, kpis, type Section, SECTIONS } from '@/topology/diagnosis'
import { type Pair, pairPath, readPair, writePair } from '@/topology/pair'
import { reportJson, reportText } from '@/topology/report'

import { BandwidthTab } from './BandwidthTab'
import { HostsTab } from './HostsTab'
import { LatencyTab } from './LatencyTab'
import { OutputTab } from './OutputTab'
import { PairHero } from './PairHero'
import { DirectCard } from './parts'
import { RouteTab } from './RouteTab'
import { RunBar } from './RunBar'
import { Findings, KpiStrip } from './Summary'

const p = t.path
const d = t.diagnosis

const isSection = (value: string | null): value is Section => SECTIONS.includes(value as Section)

/**
 * 路径诊断：两个节点的互诊断。
 *
 * - 顶上是这一对：左 A 右 B，中间两个方向现在怎么走（打开时问两端，有"重新查询"）。
 * - 下面是诊断：一次把直连、各条路的延迟和丢包、（勾了的话）吞吐、两种选路模式的选择、两端主机都量一遍
 *   （文档 §19 的主动测试），进行中每一项到了哪一步看得见。默认显示这一对最近的一次。
 * - 一排指标（直连、最低延迟、丢包、吞吐、CPU 峰值），点了去对应的标签页。
 * - 标签页：概览（结论、直连）、路由、延迟与丢包、带宽、两端主机、完整输出（文本报告和 JSON，能复制、下载）。
 *
 * 选的两个节点、看的那次诊断、在哪个标签页都写在地址里（`?a=&b=&test=&tab=`）。
 */
export function PathPage() {
  useDocumentTitle(p.title)
  const network = useNetwork()
  const now = useNow()
  const [params, setParams] = useSearchParams()
  const pair = readPair(params)
  const rawTest = Number(params.get('test'))
  const testParam = Number.isInteger(rawTest) && rawTest > 0 ? rawTest : null
  const rawTab = params.get('tab')
  const tab: Section = isSection(rawTab) ? rawTab : 'overview'
  const same = pair.a !== null && pair.a === pair.b
  const ready = pair.a !== null && pair.b !== null && !same
  const a = pair.a ?? 0
  const b = pair.b ?? 0

  const nodes = useQuery(q.nodes(network.id))
  const detailA = useQuery({ ...q.node(network.id, a), enabled: ready })
  const detailB = useQuery({ ...q.node(network.id, b), enabled: ready })
  const routesA = useQuery({ ...q.routes(network.id, a), enabled: ready })
  const routesB = useQuery({ ...q.routes(network.id, b), enabled: ready })
  const path = useQuery({ ...q.path(network.id, a, b), enabled: ready })
  const history = useQuery({ ...q.tests(network.id, { a, b }), enabled: ready })
  const shown = testParam ?? history.data?.[0]?.id ?? null
  const current = useQuery({ ...q.test(network.id, shown ?? 0), enabled: ready && shown !== null })
  const test = current.data

  const [throughput, setThroughput] = useState(() => readStored('diagnosis.throughput', false, isBoolean))
  const start = useAction(network.id, {
    mutationFn: (body: { a: NodeId; b: NodeId; throughput: boolean }) => api.createTest(network.id, body),
    success: (created) => d.started(created.a.name, created.b.name),
  })
  const stop = useAction(network.id, {
    mutationFn: (id: number) => api.cancelTest(network.id, id),
    success: d.stopped,
  })

  const update = (change: (next: URLSearchParams) => void) => {
    const next = new URLSearchParams(params)
    change(next)
    setParams(next, { replace: true })
  }
  // 换了节点，原来看的那次诊断就不是这一对的了。
  const setPair = (next: Pair) => {
    const updated = writePair(params, next)
    updated.delete('test')
    setParams(updated, { replace: true })
  }
  const setTab = (next: Section) =>
    update((query) => (next === 'overview' ? query.delete('tab') : query.set('tab', next)))
  const openTest = (id: number) => update((query) => query.set('test', String(id)))

  const list = useMemo(() => nodes.data ?? [], [nodes.data])
  const byId = new Map(list.map((node) => [node.id, node]))
  const named = useMemo(() => [...list, ...(test === undefined ? [] : [test.a, test.b])], [list, test])
  const name = (id: NodeId) => byId.get(id)?.name ?? t.common.nodeMissing(id)
  const input: DiagnosisInput = { a: detailA.data, b: detailB.data, path: path.data, test, nodes: named }
  const notPeers =
    path.data !== undefined && [path.data.forward, path.data.reverse].some((leg) => leg.source === 'not_peers')
  const queriedAgo = path.data === undefined ? null : Math.max(0, now - Math.floor(path.dataUpdatedAt / 1000))
  const stamp = new Date(now * 1000).toISOString().slice(0, 19).replace(/[-:T]/g, '')
  const fileBase = `ice3tier-${name(a)}-${name(b)}-${stamp}`

  return (
    <Page className="@container">
      <PageHeader
        title={p.title}
        description={p.subtitle}
        actions={
          <Button asChild variant="ghost">
            <Link to={pairPath(network.id, 'topology', pair.a, pair.b)}>
              <Waypoints aria-hidden />
              {p.toTopology}
            </Link>
          </Button>
        }
      />
      <div className="mb-8">
        <PairPicker nodes={list} pair={pair} onChange={setPair} labels={{ a: p.pickFrom, b: p.pickTo, swap: p.swap }} />
        {same ? <p className="mt-2 text-[13px] text-warn">{p.same}</p> : null}
      </div>

      {!ready ? (
        same ? null : (
          <EmptyState icon={RouteIcon} title={p.pickHint} className="py-12" />
        )
      ) : (
        <div className="space-y-6">
          {path.isError && path.data === undefined ? (
            <ErrorState title={p.loadFailed} error={path.error} onRetry={() => void path.refetch()} />
          ) : (
            <PairHero
              network={network.id}
              a={detailA.data ?? byId.get(a)}
              b={detailB.data ?? byId.get(b)}
              path={path.data}
              name={name}
              fetching={path.isFetching}
              queriedAgo={queriedAgo}
              onRequery={() => void path.refetch()}
            />
          )}
          {notPeers ? (
            <Notice
              tone="warn"
              title={p.notPeersTitle}
              action={
                <Button asChild size="sm">
                  <Link to={`${networkPath(network.id, 'access')}?check=1&from=${a}&to=${b}`}>{p.toAccess}</Link>
                </Button>
              }
            />
          ) : null}
          {[a, b].map((id) => {
            const node = byId.get(id)
            return node === undefined || node.online ? null : <Notice key={id}>{p.offline(node.name)}</Notice>
          })}

          <RunBar
            test={test}
            throughput={throughput}
            onThroughput={(on) => {
              setThroughput(on)
              writeStored('diagnosis.throughput', on)
            }}
            starting={start.isPending}
            stopping={stop.isPending}
            onStart={() => start.mutate({ a, b, throughput }, { onSuccess: (created) => openTest(created.id) })}
            onStop={() => {
              if (test !== undefined) stop.mutate(test.id)
            }}
          />
          {current.isError && shown !== null ? <Notice tone="warn">{t.tests.testGone(shown)}</Notice> : null}

          <KpiStrip kpis={kpis(input)} onOpen={setTab} />

          <Tabs value={tab} onValueChange={(next) => setTab(isSection(next) ? next : 'overview')}>
            <TabsList className="overflow-x-auto">
              {SECTIONS.map((section) => (
                <TabsTrigger key={section} value={section} className="shrink-0">
                  {d.tabs[section]}
                </TabsTrigger>
              ))}
            </TabsList>
            <TabsContent value="overview" className="pt-6">
              <div
                className={
                  test?.direct !== null && test?.direct !== undefined
                    ? 'grid grid-cols-1 items-start gap-5 @4xl:grid-cols-[minmax(0,1fr)_20rem]'
                    : undefined
                }
              >
                <Findings findings={findings(input)} test={test} onOpen={setTab} />
                {test?.direct !== null && test?.direct !== undefined ? (
                  <DirectCard result={test.direct} nodes={named} />
                ) : null}
              </div>
            </TabsContent>
            <TabsContent value="route" className="pt-6">
              <RouteTab path={path.data} test={test} nodes={named} name={name} />
            </TabsContent>
            <TabsContent value="latency" className="pt-6">
              <LatencyTab test={test} nodes={named} />
            </TabsContent>
            <TabsContent value="bandwidth" className="pt-6">
              <BandwidthTab test={test} nodes={named} />
            </TabsContent>
            <TabsContent value="hosts" className="pt-6">
              <HostsTab
                network={network.id}
                tested={test !== undefined}
                nodes={named}
                a={{
                  id: a,
                  name: name(a),
                  detail: detailA.data,
                  host: hostOf(test, a),
                  toPeer: routesA.data?.routes.find((route) => route.peer.id === b),
                }}
                b={{
                  id: b,
                  name: name(b),
                  detail: detailB.data,
                  host: hostOf(test, b),
                  toPeer: routesB.data?.routes.find((route) => route.peer.id === a),
                }}
              />
            </TabsContent>
            <TabsContent value="output" className="pt-6">
              {tab === 'output' ? (
                <OutputTab text={reportText(input, now)} json={reportJson(input, now)} fileBase={fileBase} />
              ) : null}
            </TabsContent>
          </Tabs>
        </div>
      )}
    </Page>
  )
}
