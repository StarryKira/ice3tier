import { type Choice, type DecisionView, type NodeId, type TestView } from '@/api/types'
import { StatusText } from '@/components/status'
import { Table, Td, Th, Tr } from '@/components/table'
import { Badge } from '@/components/ui/badge'
import { t } from '@/i18n'
import { msText, problemText } from '@/lib/tests'
import { routeName } from '@/topology/diagnosis'

import { ItemState, type Named } from './parts'

const p = t.path
const d = t.diagnosis

const routeTone = (choice: Choice) =>
  choice.route?.via === 'direct' ? 'ok' : choice.route?.via === 'relay' ? 'relay' : ('idle' as const)

function reasonText(reason: string): string {
  return Object.hasOwn(d.reason, reason) ? d.reason[reason as keyof typeof d.reason] : reason
}

/** 一个方向两种模式各会走哪条：现在用的那种标出来；延迟优先留在原地时写出那条更便宜的。 */
function DecisionRows({ decision, nodes }: { decision: DecisionView; nodes: Named }) {
  const name = (id: NodeId) => nodes.find((node) => node.id === id)?.name ?? t.common.nodeMissing(id)
  const rows = [
    { mode: 'direct_first' as const, choice: decision.direct_first },
    { mode: 'lowest_cost' as const, choice: decision.lowest_cost },
  ]
  return (
    <>
      <tr className="bg-muted/40">
        <th colSpan={4} scope="colgroup" className="px-3 py-1 text-left text-xs font-medium">
          {p.forward(name(decision.from), name(decision.to))}
        </th>
      </tr>
      {rows.map(({ mode, choice }) => (
        <Tr key={mode} data-testid={`decision-${decision.from}-${mode}`}>
          <Td className="py-1.5 whitespace-nowrap">
            <span className="text-[13px]">{p.mode[mode]}</span>
            {decision.mode === mode ? (
              <Badge tone="accent" className="ml-2">
                {p.current}
              </Badge>
            ) : null}
          </Td>
          <Td className="py-1.5 whitespace-nowrap">
            <StatusText tone={routeTone(choice)} className="text-[13px] font-medium">
              {routeName(choice.route, nodes)}
            </StatusText>
          </Td>
          <Td className="py-1.5 text-[13px] text-muted-foreground">
            {reasonText(choice.reason)}
            {choice.cheaper !== null && choice.cheaper !== undefined ? (
              <span className="block text-xs">
                {d.cheaperLine(routeName(choice.cheaper.route, nodes), msText(choice.cheaper.cost_us))}
              </span>
            ) : null}
          </Td>
          <Td numeric className="py-1.5 whitespace-nowrap text-[13px] tabular">
            {choice.cost_us === null ? '—' : msText(choice.cost_us)}
          </Td>
        </Tr>
      ))}
    </>
  )
}

/** 诊断里两端各自算的：两种选路模式各会走哪条、为什么、代价。一张表，按方向分组。放在面板里，自己不带框。 */
export function Decisions({ test, nodes }: { test: TestView | undefined; nodes: Named }) {
  const route = test?.route ?? null
  const decisions = [route?.forward, route?.reverse].filter(
    (decision): decision is DecisionView => decision !== null && decision !== undefined,
  )
  if (route === null) return <p className="px-4 py-3 text-[13px] text-muted-foreground">{d.decisionsNone}</p>
  if (route.state !== 'done') {
    return (
      <div className="px-4 py-3">
        <ItemState state={route.state} problem={route.problem} nodes={nodes} />
      </div>
    )
  }
  return (
    <div className="px-3" data-testid="decisions">
      <Table aria-label={d.decisionsTitle}>
        <thead>
          <tr>
            <Th>{d.colMode}</Th>
            <Th>{d.colChoice}</Th>
            <Th>{d.colReason}</Th>
            <Th numeric>{d.colCost}</Th>
          </tr>
        </thead>
        <tbody>
          {decisions.map((decision) => (
            <DecisionRows key={decision.from} decision={decision} nodes={nodes} />
          ))}
        </tbody>
      </Table>
      {route.problem !== null ? (
        <p className="pb-3 text-[13px] text-muted-foreground">{problemText(route.problem, nodes)}</p>
      ) : null}
    </div>
  )
}
