import { useQuery } from '@tanstack/react-query'
import { Waypoints } from 'lucide-react'
import { type ReactNode, useMemo, useState } from 'react'
import { Link, useSearchParams } from 'react-router'

import { api } from '@/api/endpoints'
import { q, useAction } from '@/api/queries'
import { type NodeId } from '@/api/types'
import { Notice } from '@/components/notice'
import { Page, PageHeader } from '@/components/page'
import { ErrorState } from '@/components/states'
import { Button } from '@/components/ui/button'
import { t } from '@/i18n'
import { isBoolean, readStored, writeStored } from '@/lib/storage'
import { useDocumentTitle } from '@/lib/useDocumentTitle'
import { useNow } from '@/lib/useNow'
import { networkPath, useNetwork } from '@/pages/networks/context'
import { checks, type ReportSection, verdict } from '@/topology/checks'
import { type DiagnosisInput, hostOf, measuredPaths } from '@/topology/diagnosis'
import { type Pair, pairPath, readPair, writePair } from '@/topology/pair'
import { reportJson, reportText } from '@/topology/report'

import {
  CostChart,
  CPU_LEGEND,
  CpuChart,
  LatencyChart,
  MEMORY_LEGEND,
  MemoryChart,
  PathChart,
  THROUGHPUT_LEGEND,
  ThroughputChart,
} from './charts'
import { Decisions } from './Decisions'
import { HostsTab } from './HostsTab'
import { NodeGrid } from './NodeGrid'
import { Output } from './OutputTab'
import { PairRow } from './PairHero'
import { QualityCard } from './parts'
import { ReportSectionBlock, ResultBlock } from './Report'
import { RunRow } from './RunBar'
import { Legend } from './viz'

const p = t.path
const d = t.diagnosis
const c = t.diagnosis.chart

interface Block {
  id: ReportSection | 'result' | 'raw'
  title: string
  hint?: ReactNode
  action?: ReactNode
  bare?: boolean
  body: ReactNode
}

/**
 * 路径诊断：两个节点的互诊断，结果是一份报告。
 *
 * - 顶上一张卡片：这一对（左 A 右 B，点一端换节点；中间两个方向现在怎么走）和诊断的状态、按钮（默认量吞吐）。
 *   没选好两个节点时下面是一格一格的节点。
 * - 报告按节编号：诊断结果（一条总评和逐项检查：结果、测得、参考、说明，量不到的写为什么），路径（像 mtr 一样一段一行，
 *   瀑布条看延迟在哪一段累起来），延迟与丢包（各条路的分布和明细），吞吐，选路（候选路径的代价、两种模式各会怎么选），
 *   两端主机（CPU、内存的图和对照表），原始数据（文本报告和 JSON）。没有数据的节不出现，结果表里写着为什么。
 * - 数据是一次诊断（文档 §19 的主动测试）量的，默认显示这一对最近的一次；路径、候选路径、节点的上报是现在的。
 *
 * 选的两个节点、看的那次诊断写在地址里（`?a=&b=&test=`）。
 */
export function PathPage() {
  useDocumentTitle(p.title)
  const network = useNetwork()
  const now = useNow()
  const [params, setParams] = useSearchParams()
  const pair = readPair(params)
  const rawTest = Number(params.get('test'))
  const testParam = Number.isInteger(rawTest) && rawTest > 0 ? rawTest : null
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

  // 完整的诊断默认连吞吐一起量；取消了记在浏览器里。
  const [throughput, setThroughput] = useState(() => readStored('diagnosis.throughput', true, isBoolean))
  const start = useAction(network.id, {
    mutationFn: (body: { a: NodeId; b: NodeId; throughput: boolean }) => api.createTest(network.id, body),
    success: (created) => d.started(created.a.name, created.b.name),
  })
  const stop = useAction(network.id, {
    mutationFn: (id: number) => api.cancelTest(network.id, id),
    success: d.stopped,
  })

  // 换了节点，原来看的那次诊断就不是这一对的了。
  const setPair = (next: Pair) => {
    const updated = writePair(params, next)
    updated.delete('test')
    setParams(updated, { replace: true })
  }
  const openTest = (id: number) => {
    const next = new URLSearchParams(params)
    next.set('test', String(id))
    setParams(next, { replace: true })
  }
  const goTo = (section: ReportSection) =>
    document.getElementById(`report-${section}`)?.scrollIntoView({ behavior: 'smooth', block: 'start' })

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

  const results = checks(input)
  const paths = measuredPaths(test)
  const hosts = (test?.hosts ?? []).filter((host) => host.state === 'done')

  // 报告的各节：没有数据的节不出现（结果表里写着为什么），编号跟着排。
  const blocks: Block[] = [
    {
      id: 'result',
      title: d.sections.result,
      bare: true,
      body: <ResultBlock checks={results} verdict={verdict(results, test, named)} onOpen={goTo} />,
    },
    {
      id: 'path',
      title: d.sections.path,
      hint: c.pathHint,
      body: <PathChart path={path.data} test={test} nodes={named} />,
    },
  ]
  if (paths.length > 0) {
    blocks.push({
      id: 'latency',
      title: d.sections.latency,
      hint: c.latencyHint,
      body: (
        <>
          <LatencyChart test={test} nodes={named} />
          <div className="-mx-4 mt-3 border-t">
            <QualityCard paths={paths} nodes={named} />
          </div>
        </>
      ),
    })
  }
  if (paths.some(({ result }) => result.throughput !== null)) {
    blocks.push({
      id: 'throughput',
      title: d.sections.throughput,
      hint: t.tests.throughputCaveat,
      action: <Legend items={THROUGHPUT_LEGEND()} />,
      body: <ThroughputChart test={test} nodes={named} />,
    })
  }
  blocks.push({
    id: 'routing',
    title: d.sections.routing,
    hint: p.costHint,
    body: (
      <>
        <CostChart path={path.data} nodes={named} />
        {test?.route !== null && test?.route !== undefined ? (
          <div className="-mx-4 mt-3 border-t pt-2">
            <p className="px-4 pb-1 text-[13px] font-semibold">{d.decisionsTitle}</p>
            <Decisions test={test} nodes={named} />
          </div>
        ) : null}
      </>
    ),
  })
  blocks.push({
    id: 'hosts',
    title: d.sections.hosts,
    bare: true,
    body: (
      <div className="overflow-hidden rounded-lg border bg-card">
        {hosts.length > 0 ? (
          <div className="grid grid-cols-1 gap-x-8 gap-y-4 border-b px-4 py-3 @4xl:grid-cols-2">
            <div>
              <p className="mb-2 flex items-center justify-between gap-3 text-[13px] font-semibold">
                {c.cpuTitle}
                <Legend items={CPU_LEGEND()} />
              </p>
              <CpuChart hosts={hosts} nodes={named} empty="" />
            </div>
            <div>
              <p className="mb-2 flex items-center justify-between gap-3 text-[13px] font-semibold">
                {c.memoryTitle}
                <Legend items={MEMORY_LEGEND()} />
              </p>
              <MemoryChart hosts={hosts} nodes={named} empty="" />
            </div>
          </div>
        ) : null}
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
            direct: test?.direct?.sides.find((side) => side.node === a),
          }}
          b={{
            id: b,
            name: name(b),
            detail: detailB.data,
            host: hostOf(test, b),
            toPeer: routesB.data?.routes.find((route) => route.peer.id === a),
            direct: test?.direct?.sides.find((side) => side.node === b),
          }}
        />
      </div>
    ),
  })
  blocks.push({
    id: 'raw',
    title: d.sections.raw,
    bare: true,
    body: (
      <Output fileBase={fileBase} report={() => ({ text: reportText(input, now), json: reportJson(input, now) })} />
    ),
  })

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
      <div className="space-y-6">
        {/* 这一对和诊断的按钮。 */}
        <section className="rounded-lg border bg-card">
          <PairRow
            nodes={list}
            pair={pair}
            onChange={setPair}
            a={detailA.data ?? byId.get(a)}
            b={detailB.data ?? byId.get(b)}
            path={ready ? path.data : undefined}
            ready={ready}
            name={name}
            fetching={path.isFetching}
            queriedAgo={queriedAgo}
            onRequery={() => void path.refetch()}
          />
          {ready ? (
            <RunRow
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
          ) : null}
        </section>

        {!ready ? (
          <NodeGrid nodes={list} loading={nodes.isPending} pair={pair} onChange={setPair} />
        ) : (
          <>
            {path.isError && path.data === undefined ? (
              <ErrorState title={p.loadFailed} error={path.error} onRetry={() => void path.refetch()} />
            ) : null}
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
            {current.isError && shown !== null ? <Notice tone="warn">{t.tests.testGone(shown)}</Notice> : null}

            {blocks.map((block, index) => (
              <ReportSectionBlock
                key={block.id}
                id={`report-${block.id}`}
                index={index + 1}
                title={block.title}
                hint={block.hint}
                action={block.action}
                bare={block.bare}
              >
                {block.body}
              </ReportSectionBlock>
            ))}
          </>
        )}
      </div>
    </Page>
  )
}
