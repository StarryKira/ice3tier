import { Link } from 'react-router'

import { type MatrixEntry, type MatrixNode, type NodeId } from '@/api/types'
import { StatusText } from '@/components/status'
import { Button } from '@/components/ui/button'
import { t } from '@/i18n'
import { formatAge } from '@/lib/time'
import { pairPath } from '@/topology/pair'
import { CLASS_TONE, classify, hopsOf, routeText } from '@/topology/route'

const g = t.topology

function Leg({
  from,
  to,
  entry,
  name,
  testId,
}: {
  from: MatrixNode
  to: MatrixNode
  entry: MatrixEntry | null
  name: (id: NodeId) => string
  testId: string
}) {
  const hops = entry === null ? null : hopsOf(entry.from, entry.to, entry.route)
  return (
    <section className="space-y-1" data-testid={testId}>
      <h3 className="text-[13px] font-semibold">{g.forward(from.name, to.name)}</h3>
      {entry === null ? (
        <p className="text-[13px] text-muted-foreground">{g.legNoData}</p>
      ) : (
        <>
          <p className="text-sm">
            <StatusText tone={entry.stale ? 'idle' : CLASS_TONE[classify(entry.route)]}>
              {routeText(entry.route, name)}
            </StatusText>
            {entry.rtt_ms !== null ? (
              <span className="ml-2 text-muted-foreground tabular">{t.routes.rtt(entry.rtt_ms)}</span>
            ) : null}
          </p>
          {hops !== null && hops.length > 2 ? (
            <p className="text-[13px] text-muted-foreground">{hops.map(name).join(t.routes.hopSeparator)}</p>
          ) : null}
          {entry.route.via === 'direct' ? (
            <p className="font-mono text-xs text-muted-foreground">{t.routes.addr(entry.route.addr)}</p>
          ) : null}
          <p className={entry.stale ? 'text-[13px] text-warn' : 'text-[13px] text-muted-foreground'}>
            {entry.stale ? g.legStale(formatAge(entry.age_secs)) : g.legAge(formatAge(entry.age_secs))}
          </p>
        </>
      )}
    </section>
  )
}

interface PairPanelProps {
  network: number
  a: MatrixNode
  b: MatrixNode
  forward: MatrixEntry | null
  reverse: MatrixEntry | null
  name: (id: NodeId) => string
}

/** 选中的那一对：图上画的东西用文字再说一遍，两个方向各一段。后面是去路径诊断现问一次的入口。 */
export function PairPanel({ network, a, b, forward, reverse, name }: PairPanelProps) {
  return (
    <aside className="space-y-4" aria-label={g.panelTitle} data-testid="pair-panel">
      <Leg from={a} to={b} entry={forward} name={name} testId="pair-forward" />
      <Leg from={b} to={a} entry={reverse} name={name} testId="pair-reverse" />
      <div className="space-y-1.5 border-t pt-3">
        <Button asChild size="sm">
          <Link to={pairPath(network, 'diagnosis', a.id, b.id)}>{g.toPath}</Link>
        </Button>
        <p className="text-[13px] text-muted-foreground">{g.toPathHint}</p>
      </div>
    </aside>
  )
}
