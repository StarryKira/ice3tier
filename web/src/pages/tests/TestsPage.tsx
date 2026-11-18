import { useQuery } from '@tanstack/react-query'
import { ArrowLeftRight, Play, RotateCw, X } from 'lucide-react'
import { useId } from 'react'
import { useSearchParams } from 'react-router'

import { api } from '@/api/endpoints'
import { q, useAction } from '@/api/queries'
import { type Network, type Node, type NodeId, type TestView } from '@/api/types'
import { InfoTip } from '@/components/info-tip'
import { NodeSelect } from '@/components/node'
import { Notice } from '@/components/notice'
import { Page, PageHeader } from '@/components/page'
import { EmptyState, ErrorState, SkeletonRows } from '@/components/states'
import { RelativeTime, StatusText, type Tone } from '@/components/status'
import { RowsTotal, Table, TableFooter, Td, Th, Toolbar, Tr } from '@/components/table'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { t } from '@/i18n'
import { msText, rateText } from '@/lib/tests'
import { useDocumentTitle } from '@/lib/useDocumentTitle'
import { cn } from '@/lib/utils'
import { useNetwork } from '@/pages/networks/context'

import { MatrixView } from './MatrixView'
import { TestResult } from './TestResult'

const s = t.tests

const STATE_TONE: Record<TestView['state'], Tone> = {
  running: 'relay',
  done: 'ok',
  failed: 'danger',
  cancelled: 'idle',
}

function numberParam(params: URLSearchParams, key: string): number | null {
  const value = Number(params.get(key))
  return Number.isInteger(value) && value > 0 ? value : null
}

/** 历史里一行的"直连"：能不能、怎么通的。 */
function directCell(test: TestView): string {
  const direct = test.direct
  if (direct?.state !== 'done') return '—'
  if (direct.ok !== true) return s.directNo
  return direct.kind === null ? s.directOk : t.routes.directKind[direct.kind]
}

/** 历史里一行的"延迟"：直连的和经中继的平均延迟。 */
function latencyCell(test: TestView): string {
  const parts: string[] = []
  const direct = test.direct_quality?.latency?.avg_us
  if (direct !== undefined && direct !== null) parts.push(`${t.nodes.detail.routesP2P} ${msText(direct)}`)
  const relay = test.relay_quality.find((result) => result.latency?.avg_us != null)?.latency?.avg_us
  if (relay !== undefined && relay !== null) parts.push(`${t.nodes.detail.routesRelay} ${msText(relay)}`)
  return parts.length === 0 ? '—' : parts.join(' · ')
}

/** 历史里一行的"吞吐"：量了的那条路径的上下行。 */
function throughputCell(test: TestView): string {
  const result = [test.direct_quality, ...test.relay_quality].find((path) => path?.throughput?.state === 'done')
  const throughput = result?.throughput
  if (throughput === null || throughput === undefined) return '—'
  return `↑ ${rateText(throughput.up?.bits_per_second)} ↓ ${rateText(throughput.down?.bits_per_second)}`
}

interface PairFormProps {
  nodes: readonly Node[]
  a: NodeId | null
  b: NodeId | null
  throughput: boolean
  onChange: (changes: { a?: NodeId | null; b?: NodeId | null; throughput?: boolean }) => void
  onStart: () => void
  pending: boolean
}

/** 发起测试的一行：两个节点（中间能对调）、要不要测吞吐、开始。 */
function PairForm({ nodes, a, b, throughput, onChange, onStart, pending }: PairFormProps) {
  const throughputId = useId()
  const ready = a !== null && b !== null && a !== b
  return (
    <Toolbar className="gap-3">
      <div className="w-60">
        <NodeSelect
          nodes={nodes}
          value={a}
          onChange={(id) => onChange({ a: id })}
          aria-label={s.nodeA}
          placeholder={s.nodeA}
        />
      </div>
      <Button
        variant="ghost"
        size="icon-sm"
        aria-label={s.swap}
        disabled={a === null && b === null}
        onClick={() => onChange({ a: b, b: a })}
      >
        <ArrowLeftRight aria-hidden />
      </Button>
      <div className="w-60">
        <NodeSelect
          nodes={nodes}
          value={b}
          onChange={(id) => onChange({ b: id })}
          aria-label={s.nodeB}
          placeholder={s.nodeB}
        />
      </div>
      <label htmlFor={throughputId} className="ml-2 inline-flex items-center gap-2 text-sm">
        <Checkbox
          id={throughputId}
          checked={throughput}
          onCheckedChange={(value) => onChange({ throughput: value === true })}
        />
        {s.throughput}
        <InfoTip>{s.throughputHint}</InfoTip>
      </label>
      <Button variant="primary" className="ml-auto" pending={pending} disabled={!ready} onClick={onStart}>
        <Play aria-hidden />
        {s.start}
      </Button>
      {a !== null && a === b ? <p className="w-full text-[13px] text-danger">{s.pickTwo}</p> : null}
    </Toolbar>
  )
}

/** 一个测试的标题行：哪一对、状态、什么时候开始、用了多久；进行中能取消，结束了能再测一次。 */
function TestHeading({
  test,
  onCancel,
  onAgain,
  busy,
}: {
  test: TestView
  onCancel: () => void
  onAgain: () => void
  busy: boolean
}) {
  return (
    <div className="mb-4 flex flex-wrap items-center gap-x-4 gap-y-2">
      <h2 className="text-[15px] font-semibold">
        {test.a.name} <span className="font-normal text-muted-foreground">⇄</span> {test.b.name}
      </h2>
      <span data-testid="test-state">
        <StatusText tone={STATE_TONE[test.state]} className="text-[13px]">
          {s.state[test.state]}
        </StatusText>
      </span>
      <span className="text-[13px] text-muted-foreground">
        <RelativeTime at={test.started_at} />
        {test.finished_at !== null ? ` · ${s.tookSecs(test.finished_at - test.started_at)}` : null}
      </span>
      <span className="ml-auto">
        {test.state === 'running' ? (
          <Button size="sm" onClick={onCancel} pending={busy}>
            <X aria-hidden />
            {s.cancel}
          </Button>
        ) : (
          <Button size="sm" onClick={onAgain} pending={busy}>
            <RotateCw aria-hidden />
            {s.again}
          </Button>
        )}
      </span>
    </div>
  )
}

/** 单对测试：上面发起，中间是当前（或者从历史里点开的）测试，下面是历史。 */
function PairTab({ network, nodes }: { network: Network; nodes: readonly Node[] }) {
  const [params, setParams] = useSearchParams()
  const a = numberParam(params, 'a')
  const b = numberParam(params, 'b')
  const throughput = params.get('throughput') === '1'
  const testId = numberParam(params, 'test')

  const update = (changes: Record<string, string | null>) => {
    const next = new URLSearchParams(params)
    for (const [key, value] of Object.entries(changes)) {
      if (value === null) next.delete(key)
      else next.set(key, value)
    }
    setParams(next, { replace: true })
  }

  const start = useAction(network.id, {
    mutationFn: (body: { a: NodeId; b: NodeId; throughput: boolean }) => api.createTest(network.id, body),
    success: (created) => s.started(created.a.name, created.b.name),
  })
  const cancel = useAction(network.id, {
    mutationFn: (id: number) => api.cancelTest(network.id, id),
    success: s.cancelled,
  })

  const current = useQuery({ ...q.test(network.id, testId ?? 0), enabled: testId !== null })
  const pair = a !== null && b !== null && a !== b ? { a, b } : undefined
  const history = useQuery(q.tests(network.id, pair))
  const rows = history.data ?? []

  const startWith = (body: { a: NodeId; b: NodeId; throughput: boolean }) =>
    start.mutate(body, {
      onSuccess: (created) => update({ test: String(created.id), a: String(created.a.id), b: String(created.b.id) }),
    })

  return (
    <>
      <PairForm
        nodes={nodes}
        a={a}
        b={b}
        throughput={throughput}
        pending={start.isPending}
        onChange={(changes) =>
          update({
            ...(changes.a !== undefined ? { a: changes.a === null ? null : String(changes.a) } : {}),
            ...(changes.b !== undefined ? { b: changes.b === null ? null : String(changes.b) } : {}),
            ...(changes.throughput !== undefined ? { throughput: changes.throughput ? '1' : null } : {}),
          })
        }
        onStart={() => a !== null && b !== null && startWith({ a, b, throughput })}
      />

      {testId !== null ? (
        <section className="mt-2 mb-10" data-testid="test-current">
          {current.isError ? (
            <Notice tone="warn">{s.testGone(testId)}</Notice>
          ) : current.data === undefined ? (
            <SkeletonRows columns={2} rows={3} />
          ) : (
            <>
              <TestHeading
                test={current.data}
                busy={cancel.isPending || start.isPending}
                onCancel={() => cancel.mutate(current.data.id)}
                onAgain={() =>
                  startWith({ a: current.data.a.id, b: current.data.b.id, throughput: current.data.request.throughput })
                }
              />
              <TestResult test={current.data} nodes={nodes} />
            </>
          )}
        </section>
      ) : null}

      <h2 className="mt-4 mb-2 text-[15px] font-semibold">
        {pair !== undefined
          ? s.historyPair(
              nodes.find((node) => node.id === pair.a)?.name ?? '',
              nodes.find((node) => node.id === pair.b)?.name ?? '',
            )
          : s.historyTitle}
      </h2>
      {history.isError ? (
        <ErrorState title={s.historyTitle} error={history.error} onRetry={() => void history.refetch()} />
      ) : history.isSuccess && rows.length === 0 ? (
        <EmptyState title={s.historyEmpty} />
      ) : (
        <>
          <Table>
            <thead>
              <tr>
                <Th>{s.colTime}</Th>
                <Th>{s.colPair}</Th>
                <Th>{s.colDirect}</Th>
                <Th>{s.colLatency}</Th>
                <Th>{s.colThroughput}</Th>
                <Th>{s.colState}</Th>
              </tr>
            </thead>
            <tbody>
              {history.isPending ? <SkeletonRows columns={6} rows={3} /> : null}
              {rows.map((test) => (
                <Tr
                  key={test.id}
                  onClick={() => update({ test: String(test.id) })}
                  className={cn(
                    'h-12 cursor-pointer hover:bg-muted/50',
                    test.id === testId && 'bg-accent/60 hover:bg-accent',
                  )}
                  data-testid={`test-row-${test.id}`}
                  aria-label={s.viewTest(test.id)}
                >
                  <Td className="text-[13px] whitespace-nowrap text-muted-foreground">
                    <RelativeTime at={test.started_at} />
                  </Td>
                  <Td className="whitespace-nowrap">
                    {test.a.name} <span className="text-muted-foreground">⇄</span> {test.b.name}
                  </Td>
                  <Td className="text-[13px]">{directCell(test)}</Td>
                  <Td className="text-[13px] whitespace-nowrap tabular">{latencyCell(test)}</Td>
                  <Td className="text-[13px] whitespace-nowrap tabular">{throughputCell(test)}</Td>
                  <Td>
                    <StatusText tone={STATE_TONE[test.state]} className="text-[13px]">
                      {s.state[test.state]}
                    </StatusText>
                  </Td>
                </Tr>
              ))}
            </tbody>
          </Table>
          {rows.length > 0 ? (
            <TableFooter>
              <RowsTotal shown={rows.length} />
            </TableFooter>
          ) : null}
        </>
      )}
    </>
  )
}

/**
 * 连通性测试（文档 §19）：一对节点能不能直连、直连和经中继的延迟和吞吐、两种选路模式各会选哪条；
 * 或者对全网每一对测直连和延迟。地址里记着选的节点（`?a=&b=`）、看的测试（`?test=`）和标签页（`?tab=matrix`），
 * 路由表上的"测一下"就是带着这一对打开这里。
 */
export function TestsPage() {
  useDocumentTitle(s.title)
  const network = useNetwork()
  const nodes = useQuery(q.nodes(network.id))
  const [params, setParams] = useSearchParams()
  const tab = params.get('tab') === 'matrix' ? 'matrix' : 'pair'

  return (
    <Page className="@container">
      <PageHeader title={s.title} description={s.subtitle} />
      {nodes.isError ? (
        <ErrorState title={s.title} error={nodes.error} onRetry={() => void nodes.refetch()} />
      ) : (
        <Tabs
          value={tab}
          onValueChange={(next) => {
            const changed = new URLSearchParams(params)
            if (next === 'matrix') changed.set('tab', 'matrix')
            else changed.delete('tab')
            setParams(changed, { replace: true })
          }}
        >
          <TabsList>
            <TabsTrigger value="pair">{s.tabPair}</TabsTrigger>
            <TabsTrigger value="matrix">{s.tabMatrix}</TabsTrigger>
          </TabsList>
          <TabsContent value="pair">
            <PairTab network={network} nodes={nodes.data ?? []} />
          </TabsContent>
          <TabsContent value="matrix">
            <MatrixView
              network={network}
              nodes={nodes.data ?? []}
              onOpen={(test, pair) => {
                setParams(new URLSearchParams({ test: String(test), a: String(pair.a), b: String(pair.b) }), {
                  replace: false,
                })
              }}
            />
          </TabsContent>
        </Tabs>
      )}
    </Page>
  )
}
