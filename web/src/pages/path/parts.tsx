import { type ReactNode } from 'react'

import { type Node, type TestItemState, type TestProblem } from '@/api/types'
import { Skeleton } from '@/components/ui/skeleton'
import { t } from '@/i18n'
import { problemText } from '@/lib/tests'

const s = t.tests

export type Named = readonly Pick<Node, 'id' | 'name'>[]

/** 一项还没有结果、没有做、没做成时就是这一句（带原因）。 */
export function ItemState({
  state,
  problem,
  nodes,
}: {
  state: TestItemState
  problem: TestProblem | null
  nodes: Named
}): ReactNode {
  if (state === 'pending') {
    return (
      <div aria-busy="true" aria-label={s.pending} className="space-y-2 py-1">
        <Skeleton className="h-3.5 w-24" />
        <Skeleton className="h-3.5 w-32" />
      </div>
    )
  }
  return (
    <p className="text-[13px] text-muted-foreground">
      {state === 'skipped' ? s.skipped : s.failed}
      {problem !== null ? `：${problemText(problem, nodes)}` : null}
    </p>
  )
}
