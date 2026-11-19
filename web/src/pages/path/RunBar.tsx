import { Check, Loader2, Minus, Play, Square, X } from 'lucide-react'
import { useId } from 'react'

import { type TestItemState, type TestView } from '@/api/types'
import { InfoTip } from '@/components/info-tip'
import { RelativeTime, StatusText, type Tone } from '@/components/status'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { t } from '@/i18n'
import { cn } from '@/lib/utils'
import { steps } from '@/topology/diagnosis'

const d = t.diagnosis
const s = t.tests

const STATE_TONE: Record<TestView['state'], Tone> = {
  running: 'relay',
  done: 'ok',
  failed: 'danger',
  cancelled: 'idle',
}

const STEP_ICON = { pending: Loader2, done: Check, skipped: Minus, failed: X } as const
const STEP_TONE: Record<TestItemState, string> = {
  pending: 'border-relay/40 bg-relay-soft text-relay',
  done: 'border-ok/30 bg-ok-soft text-ok',
  skipped: 'bg-muted text-muted-foreground',
  failed: 'border-danger/30 bg-danger-soft text-danger',
}

interface RunBarProps {
  test: TestView | undefined
  throughput: boolean
  onThroughput: (on: boolean) => void
  onStart: () => void
  onStop: () => void
  starting: boolean
  stopping: boolean
}

/**
 * 诊断的控制条：左边是这次诊断的状态（什么时候、用了多久），右边测吞吐的勾和开始 / 停止。
 * 下面一排是每一项到了哪一步（进行中的转圈），进行中时最底下一根进度条。
 */
export function RunBar({ test, throughput, onThroughput, onStart, onStop, starting, stopping }: RunBarProps) {
  const throughputId = useId()
  const running = test?.state === 'running'
  const list = test === undefined ? [] : steps(test)
  const finished = list.filter((step) => step.state !== 'pending').length
  return (
    <section className="relative overflow-hidden rounded-xl border bg-card" data-testid="run-bar">
      <div className="flex flex-wrap items-center gap-x-6 gap-y-3 px-5 py-3.5">
        <div className="min-w-0">
          <h2 className="text-[15px] font-semibold">{d.runTitle}</h2>
          {test === undefined ? (
            <p className="mt-0.5 text-xs text-muted-foreground">
              {d.never}
              <InfoTip className="ml-1 align-middle">{d.neverHint}</InfoTip>
            </p>
          ) : (
            <p
              className="mt-0.5 flex flex-wrap items-center gap-x-2 text-xs text-muted-foreground"
              data-testid="test-meta"
            >
              <span data-testid="test-state">
                <StatusText tone={STATE_TONE[test.state]}>{s.state[test.state]}</StatusText>
              </span>
              <span aria-hidden>·</span>
              <RelativeTime at={test.started_at} />
              {test.finished_at !== null ? (
                <>
                  <span aria-hidden>·</span>
                  {s.tookSecs(test.finished_at - test.started_at)}
                </>
              ) : null}
              <span aria-hidden>·</span>
              <span className="tabular">{d.number(test.id)}</span>
            </p>
          )}
        </div>
        <div className="ml-auto flex items-center gap-4">
          {running ? (
            <Button size="sm" pending={stopping} onClick={onStop}>
              <Square aria-hidden />
              {d.stop}
            </Button>
          ) : (
            <>
              <label htmlFor={throughputId} className="inline-flex items-center gap-2 text-[13px]">
                <Checkbox
                  id={throughputId}
                  checked={throughput}
                  onCheckedChange={(value) => onThroughput(value === true)}
                />
                {s.throughput}
                <InfoTip>{s.throughputHint}</InfoTip>
              </label>
              <Button size="sm" variant="primary" pending={starting} onClick={onStart}>
                <Play aria-hidden />
                {test === undefined ? d.run : d.rerun}
              </Button>
            </>
          )}
        </div>
      </div>
      {list.length > 0 ? (
        <ol
          className="flex flex-wrap items-center gap-2 border-t px-5 py-3"
          aria-label={d.progress(finished, list.length)}
        >
          {list.map((step) => {
            const Icon = STEP_ICON[step.state]
            return (
              <li
                key={step.key}
                className={cn(
                  'inline-flex h-7 items-center gap-1.5 rounded-full border border-transparent px-2.5 text-xs font-medium',
                  STEP_TONE[step.state],
                )}
                data-testid={`step-${step.key}`}
                data-state={step.state}
              >
                <Icon className={cn('size-3.5', step.state === 'pending' && 'motion-safe:animate-spin')} aria-hidden />
                {d.steps[step.key]}
                <span className="sr-only">{d.stepState[step.state]}</span>
              </li>
            )
          })}
          <li className="ml-auto text-xs text-muted-foreground tabular">{d.progress(finished, list.length)}</li>
        </ol>
      ) : null}
      {running ? (
        <div className="absolute inset-x-0 bottom-0 h-0.5 bg-muted" aria-hidden>
          <div
            className="h-full bg-relay transition-[width] duration-500"
            style={{ width: `${list.length === 0 ? 0 : (100 * finished) / list.length}%` }}
          />
        </div>
      ) : null}
    </section>
  )
}
