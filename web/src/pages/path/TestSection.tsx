import { useQuery } from '@tanstack/react-query'
import { Play, X } from 'lucide-react'
import { useId, useState } from 'react'

import { api } from '@/api/endpoints'
import { q, useAction } from '@/api/queries'
import { type Network, type Node, type NodeId, type TestView } from '@/api/types'
import { InfoTip } from '@/components/info-tip'
import { Notice } from '@/components/notice'
import { ErrorState, SkeletonBlock } from '@/components/states'
import { RelativeTime, StatusText, type Tone } from '@/components/status'
import { Table, Td, Th, Tr } from '@/components/table'
import { Button } from '@/components/ui/button'
import { Card, CardHeader } from '@/components/ui/card'
import { Checkbox } from '@/components/ui/checkbox'
import { t } from '@/i18n'
import { msText, rateText } from '@/lib/tests'
import { cn } from '@/lib/utils'

import { TestResult } from './TestResult'

const s = t.tests

const STATE_TONE: Record<TestView['state'], Tone> = {
  running: 'relay',
  done: 'ok',
  failed: 'danger',
  cancelled: 'idle',
}

/** 列表里一行的"直连"：能不能、怎么通的。 */
function directCell(test: TestView): string {
  const direct = test.direct
  if (direct?.state !== 'done') return '—'
  if (direct.ok !== true) return s.directNo
  return direct.kind === null ? s.directOk : t.routes.directKind[direct.kind]
}

/** 列表里一行的"延迟"：直连的和经中继的平均延迟。 */
function latencyCell(test: TestView): string {
  const parts: string[] = []
  const direct = test.direct_quality?.latency?.avg_us
  if (direct !== undefined && direct !== null) parts.push(`${s.qualityDirect} ${msText(direct)}`)
  const relay = test.relay_quality.find((result) => result.latency?.avg_us != null)?.latency?.avg_us
  if (relay !== undefined && relay !== null) parts.push(`${s.qualityRelay} ${msText(relay)}`)
  return parts.length === 0 ? '—' : parts.join(' · ')
}

/** 列表里一行的"吞吐"：量了的那条路径的上下行。 */
function throughputCell(test: TestView): string {
  const result = [test.direct_quality, ...test.relay_quality].find((path) => path?.throughput?.state === 'done')
  const throughput = result?.throughput
  if (throughput === null || throughput === undefined) return '—'
  return `↑ ${rateText(throughput.up?.bits_per_second)} ↓ ${rateText(throughput.down?.bits_per_second)}`
}

/**
 * 最近的测试，新的在前；点一行看它。`pairs` 时多一列是哪一对（没选节点时列整个网络的）。
 */
export function TestHistory({
  tests,
  shown,
  onOpen,
  pairs = false,
  title,
}: {
  tests: readonly TestView[]
  shown: number | null
  onOpen: (test: TestView) => void
  pairs?: boolean
  title: string
}) {
  return (
    <Card>
      <CardHeader title={title} />
      <Table aria-label={title}>
        <thead>
          <tr>
            <Th>{s.colTime}</Th>
            {pairs ? <Th>{s.colPair}</Th> : null}
            <Th>{s.colDirect}</Th>
            <Th>{s.colLatency}</Th>
            <Th>{s.colThroughput}</Th>
            <Th>{s.colState}</Th>
          </tr>
        </thead>
        <tbody>
          {tests.map((test) => (
            <Tr
              key={test.id}
              onClick={() => onOpen(test)}
              className={cn('h-11 cursor-pointer hover:bg-muted/50', test.id === shown && 'bg-accent/60 hover:bg-accent')}
              aria-current={test.id === shown ? 'true' : undefined}
              data-testid={`test-row-${test.id}`}
              aria-label={s.viewTest(test.id)}
            >
              <Td className="text-[13px] whitespace-nowrap text-muted-foreground">
                <RelativeTime at={test.started_at} />
              </Td>
              {pairs ? (
                <Td className="whitespace-nowrap">
                  {test.a.name} <span className="text-muted-foreground">⇄</span> {test.b.name}
                </Td>
              ) : null}
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
    </Card>
  )
}

interface TestSectionProps {
  network: Network
  nodes: readonly Node[]
  a: NodeId
  b: NodeId
  /** 地址里点名要看的测试；没有时看这一对最近的一次。 */
  test: number | null
  onOpen: (test: number) => void
}

/**
 * 路径诊断里的"实测"（文档 §19）：发包测这一对能不能直连、各条路的延迟和丢包，要的话加上吞吐。
 * 默认显示这一对最近的一次；测过不止一次时下面列出来，点一行换着看。
 */
export function TestSection({ network, nodes, a, b, test, onOpen }: TestSectionProps) {
  const throughputId = useId()
  const [throughput, setThroughput] = useState(false)
  const history = useQuery(q.tests(network.id, { a, b }))
  const rows = history.data ?? []
  const shown = test ?? rows[0]?.id ?? null
  const current = useQuery({ ...q.test(network.id, shown ?? 0), enabled: shown !== null })

  const start = useAction(network.id, {
    mutationFn: (body: { a: NodeId; b: NodeId; throughput: boolean }) => api.createTest(network.id, body),
    success: (created) => s.started(created.a.name, created.b.name),
  })
  const cancel = useAction(network.id, {
    mutationFn: (id: number) => api.cancelTest(network.id, id),
    success: s.cancelled,
  })

  const running = current.data?.state === 'running'
  const action = running ? (
    <Button size="sm" pending={cancel.isPending} onClick={() => current.data && cancel.mutate(current.data.id)}>
      <X aria-hidden />
      {s.cancel}
    </Button>
  ) : (
    <>
      <label htmlFor={throughputId} className="inline-flex items-center gap-2 text-[13px]">
        <Checkbox id={throughputId} checked={throughput} onCheckedChange={(value) => setThroughput(value === true)} />
        {s.throughput}
        <InfoTip>{s.throughputHint}</InfoTip>
      </label>
      <Button
        size="sm"
        variant="primary"
        className="ml-2"
        pending={start.isPending}
        onClick={() => start.mutate({ a, b, throughput }, { onSuccess: (created) => onOpen(created.id) })}
      >
        <Play aria-hidden />
        {s.start}
      </Button>
    </>
  )

  return (
    <div className="space-y-10" data-testid="test-section">
      <Card>
        <CardHeader
          title={s.title}
          description={
            current.data === undefined ? (
              s.subtitle
            ) : (
              <span className="inline-flex flex-wrap items-center gap-x-2" data-testid="test-meta">
                <span data-testid="test-state">
                  <StatusText tone={STATE_TONE[current.data.state]}>{s.state[current.data.state]}</StatusText>
                </span>
                <span aria-hidden>·</span>
                <RelativeTime at={current.data.started_at} />
                {current.data.finished_at !== null ? (
                  <>
                    <span aria-hidden>·</span>
                    {s.tookSecs(current.data.finished_at - current.data.started_at)}
                  </>
                ) : null}
              </span>
            )
          }
          action={action}
        />
        {history.isError && history.data === undefined ? (
          <ErrorState title={s.title} error={history.error} onRetry={() => void history.refetch()} />
        ) : shown === null ? (
          history.isPending ? (
            <SkeletonBlock lines={3} />
          ) : (
            <p className="rounded-xl border border-dashed px-6 py-10 text-center text-[13px] text-muted-foreground">
              {s.none}
            </p>
          )
        ) : current.isError ? (
          <Notice tone="warn">{s.testGone(shown)}</Notice>
        ) : current.data === undefined ? (
          <SkeletonBlock lines={4} />
        ) : (
          <div data-testid="test-current">
            <TestResult test={current.data} nodes={nodes} />
          </div>
        )}
      </Card>
      {rows.length > 1 ? (
        <TestHistory
          title={s.historyTitle}
          tests={rows}
          shown={shown}
          onOpen={(picked) => onOpen(picked.id)}
        />
      ) : null}
    </div>
  )
}
