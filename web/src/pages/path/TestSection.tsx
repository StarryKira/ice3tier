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
import { Button } from '@/components/ui/button'
import { Card, CardHeader } from '@/components/ui/card'
import { Checkbox } from '@/components/ui/checkbox'
import { t } from '@/i18n'

import { TestResult } from './TestResult'

const s = t.tests

const STATE_TONE: Record<TestView['state'], Tone> = {
  running: 'relay',
  done: 'ok',
  failed: 'danger',
  cancelled: 'idle',
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
 * 默认显示这一对最近的一次（地址里点名了就看那一次），不列以前的。
 */
export function TestSection({ network, nodes, a, b, test, onOpen }: TestSectionProps) {
  const throughputId = useId()
  const [throughput, setThroughput] = useState(false)
  const history = useQuery(q.tests(network.id, { a, b }))
  const shown = test ?? history.data?.[0]?.id ?? null
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
    <div data-testid="test-section">
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
    </div>
  )
}
