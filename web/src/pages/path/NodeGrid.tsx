import { useState } from 'react'

import { type Node, type NodeId } from '@/api/types'
import { SearchBox } from '@/components/search-box'
import { Skeleton } from '@/components/ui/skeleton'
import { t } from '@/i18n'
import { cn } from '@/lib/utils'
import { type Pair } from '@/topology/pair'

import { NodeTile } from './PairHero'

const d = t.diagnosis

/**
 * 还没选好两个节点时，下面把全部节点铺成一格一格的卡片：点一个是 A，再点一个是 B（点已经选了的那个是取消）。
 * 在线的在前；节点多的时候上面一个搜索框，按名字、虚拟 IP、分组找。
 */
export function NodeGrid({
  nodes,
  loading,
  pair,
  onChange,
}: {
  nodes: readonly Node[]
  /** 节点列表还没取到：先画几个空的格子。 */
  loading: boolean
  pair: Pair
  onChange: (pair: Pair) => void
}) {
  const [query, setQuery] = useState('')
  const needle = query.trim().toLowerCase()
  const shown = [...nodes]
    .filter(
      (node) =>
        needle === '' ||
        `${node.name} ${node.virtual_ip} ${node.groups.map((group) => group.name).join(' ')}`
          .toLowerCase()
          .includes(needle),
    )
    .sort((x, y) => Number(y.online) - Number(x.online) || x.name.localeCompare(y.name))
  const pick = (id: NodeId) => {
    if (pair.a === id) onChange({ ...pair, a: null })
    else if (pair.b === id) onChange({ ...pair, b: null })
    else if (pair.a === null) onChange({ ...pair, a: id })
    else onChange({ ...pair, b: id })
  }
  return (
    <section data-testid="node-grid">
      <header className="mb-3 flex flex-wrap items-center justify-between gap-3">
        <div>
          <h2 className="text-[15px] font-semibold">{d.nodesTitle}</h2>
          <p className="mt-0.5 text-xs text-muted-foreground">{d.nodesHint}</p>
        </div>
        {nodes.length > 8 ? (
          <SearchBox value={query} onChange={setQuery} placeholder={d.nodesSearch} className="w-64" />
        ) : null}
      </header>
      {loading ? (
        <ul className="grid grid-cols-1 gap-3 @xl:grid-cols-2 @4xl:grid-cols-3 @6xl:grid-cols-4" aria-busy="true">
          {[0, 1, 2].map((key) => (
            <li key={key}>
              <Skeleton className="h-15 rounded-xl" />
            </li>
          ))}
        </ul>
      ) : shown.length === 0 ? (
        <p className="rounded-xl border border-dashed px-6 py-8 text-center text-[13px] text-muted-foreground">
          {d.nodesEmpty}
        </p>
      ) : (
        <ul className="grid grid-cols-1 gap-3 @xl:grid-cols-2 @4xl:grid-cols-3 @6xl:grid-cols-4">
          {shown.map((node) => {
            const label = pair.a === node.id ? d.nodeA : pair.b === node.id ? d.nodeB : null
            return (
              <li key={node.id}>
                <button
                  type="button"
                  onClick={() => pick(node.id)}
                  aria-pressed={label !== null}
                  aria-label={label === null ? node.name : `${node.name}，${d.picked(label)}`}
                  className={cn(
                    'relative flex w-full items-center gap-3 rounded-xl border bg-card px-3 py-2.5 text-left transition-[box-shadow,border-color] hover:border-border-strong hover:shadow-card',
                    label !== null && 'border-primary ring-2 ring-primary/20 hover:border-primary',
                    !node.online && 'border-dashed',
                  )}
                >
                  <NodeTile node={node} size="md" />
                  <span className="min-w-0 flex-1">
                    <span className="flex min-w-0 items-center gap-1.5">
                      <span
                        className={cn('truncate text-[13px] font-semibold', !node.online && 'text-muted-foreground')}
                      >
                        {node.name}
                      </span>
                      {node.relay !== null ? (
                        <span className="shrink-0 rounded-full bg-relay-soft px-1.5 text-[10px] leading-4 font-medium text-relay">
                          {d.relay}
                        </span>
                      ) : null}
                    </span>
                    <span className="block truncate font-mono text-[11px] text-muted-foreground">
                      {node.virtual_ip}
                      {node.online ? null : ` · ${d.offline}`}
                    </span>
                  </span>
                  {label !== null ? (
                    <span className="grid size-6 shrink-0 place-items-center rounded-full bg-primary text-xs font-semibold text-primary-foreground">
                      {label}
                    </span>
                  ) : null}
                </button>
              </li>
            )
          })}
        </ul>
      )}
    </section>
  )
}
