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
  pending: 'text-foreground',
  done: 'text-muted-foreground',
  skipped: 'text-muted-foreground',
  failed: 'text-danger',
}

interface RunRowProps {
  test: TestView | undefined
  throughput: boolean
  onThroughput: (on: boolean) => void
  onStart: () => void
  onStop: () => void
  starting: boolean
  stopping: boolean
}

/**
 * 诊断头的最后一行：左边是这次诊断的状态（什么时候、用了多久），右边测吞吐的勾和开始 / 停止。
 * 进行中时状态后面一排每一项到了哪一步（在测的转圈），顶边一根进度条；做完了只在有项没做成时才列出来。
 */
export function RunRow({ test, throughput, onThroughput, onStart, onStop, starting, stopping }: RunRowProps) {
  const throughputId = useId()
  const running = test?.state === 'running'
  const list = test === undefined ? [] : steps(test)
  const finished = list.filter((step) => step.state !== 'pending').length
  const showSteps = running || list.some((step) => step.state === 'failed' || step.state === 'skipped')
  return (
    <div className="relative flex flex-wrap items-center gap-x-6 gap-y-2 border-t px-6 py-3" data-testid="run-bar">
      {running ? (
        <div className="absolute inset-x-0 -top-px h-0.5 bg-muted" aria-hidden>
          <div
            className="h-full bg-primary transition-[width] duration-500"
            style={{ width: `${list.length === 0 ? 0 : (100 * finished) / list.length}%` }}
          />
        </div>
      ) : null}
      <div className="flex min-w-0 flex-wrap items-center gap-x-5 gap-y-1.5 text-[13px]">
        {test === undefined ? (
          <span className="inline-flex items-center gap-1.5 text-muted-foreground">
            {d.never}
            <InfoTip>{d.neverHint}</InfoTip>
          </span>
        ) : (
          <span className="flex flex-wrap items-center gap-x-2 text-muted-foreground" data-testid="test-meta">
            <span data-testid="test-state" className="font-medium text-foreground">
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
          </span>
        )}
        {showSteps ? (
          <ol
            className="flex flex-wrap items-center gap-x-3 gap-y-1 text-xs"
            aria-label={d.progress(finished, list.length)}
          >
            {list.map((step) => {
              const Icon = STEP_ICON[step.state]
              return (
                <li
                  key={step.key}
                  className={cn('inline-flex items-center gap-1', STEP_TONE[step.state])}
                  data-testid={`step-${step.key}`}
                  data-state={step.state}
                >
                  <Icon className={cn('size-3', step.state === 'pending' && 'motion-safe:animate-spin')} aria-hidden />
                  {d.steps[step.key]}
                  <span className="sr-only">{d.stepState[step.state]}</span>
                </li>
              )
            })}
          </ol>
        ) : null}
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
  )
}
