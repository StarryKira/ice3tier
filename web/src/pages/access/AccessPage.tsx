import { useQuery } from '@tanstack/react-query'
import { Plus, ScanSearch, ShieldOff } from 'lucide-react'
import { useState } from 'react'
import { useSearchParams } from 'react-router'

import { api } from '@/api/endpoints'
import { q, useAction } from '@/api/queries'
import { type AccessGraph, type Network, type NodeId, type Rule } from '@/api/types'
import { NodeLink, NodeSelect } from '@/components/node'
import { Notice } from '@/components/notice'
import { Page, PageHeader } from '@/components/page'
import { EmptyState, ErrorState, SkeletonBlock } from '@/components/states'
import { Button } from '@/components/ui/button'
import { Card, CardHeader } from '@/components/ui/card'
import { Segmented } from '@/components/ui/segmented'
import { t } from '@/i18n'
import { useDocumentTitle } from '@/lib/useDocumentTitle'
import { cn } from '@/lib/utils'
import { useNetwork } from '@/pages/networks/context'

import { AccessCheck } from './AccessCheck'
import { NodeGraphView } from './NodeGraphView'
import { RuleDialog, type RuleDraft } from './RuleDialog'
import { RuleList } from './RuleList'
import { TagGraphView } from './TagGraphView'
import { TagPanel } from './TagPanel'

const g = t.access.graph

type View = 'tags' | 'node'
type Editor = { rule: Rule | null; draft?: RuleDraft } | null

function numberParam(params: URLSearchParams, key: string): NodeId | null {
  const raw = params.get(key)
  if (raw === null) return null
  const value = Number(raw)
  return Number.isInteger(value) && value > 0 ? value : null
}

function Legend() {
  const sample = 'h-0 w-7 border-t-[1.5px]'
  return (
    <ul className="flex flex-wrap items-center gap-x-5 gap-y-1 text-xs text-muted-foreground" aria-hidden>
      <li className="flex items-center gap-1.5">
        <span>◀</span>
        <span className={cn(sample, 'border-foreground/70')} />
        <span>▶</span>
        {g.legendBoth}
      </li>
      <li className="flex items-center gap-1.5">
        <span className={cn(sample, 'border-foreground/70')} />
        <span>▶</span>
        {g.legendOneWay}
      </li>
      <li className="flex items-center gap-1.5">
        <span className={cn(sample, 'border-dashed border-idle-dot')} />
        {g.legendDisabled}
      </li>
    </ul>
  )
}

interface NodeViewProps {
  network: Network
  graph: AccessGraph
  focus: NodeId | null
  onFocus: (node: NodeId) => void
}

function NodeView({ network, graph, focus, onFocus }: NodeViewProps) {
  const nodes = useQuery(q.nodes(network.id))
  const access = useQuery({ ...q.nodeAccess(network.id, focus ?? 0), enabled: focus !== null })
  const [expanded, setExpanded] = useState({ left: false, right: false })

  return (
    <>
      <div className="flex flex-wrap items-center gap-3 border-b px-5 py-3">
        <div className="w-72">
          <NodeSelect nodes={nodes.data ?? []} value={focus} onChange={onFocus} aria-label={g.nodePickLabel} />
        </div>
        {focus === null ? <p className="text-[13px] text-muted-foreground">{g.nodePick}</p> : null}
      </div>
      {focus !== null && access.data !== undefined && !access.data.acknowledged ? (
        <div className="border-b px-5 py-3">
          <Notice tone="warn">{g.nodePending}</Notice>
        </div>
      ) : null}
      <div className="h-[460px]" data-testid="node-graph">
        {focus === null ? (
          <EmptyState icon={ScanSearch} title={g.nodePick} className="h-full justify-center" />
        ) : access.isError ? (
          <ErrorState title={g.loadFailed} error={access.error} onRetry={() => void access.refetch()} />
        ) : access.data === undefined ? (
          <SkeletonBlock lines={4} />
        ) : (
          <NodeGraphView
            access={access.data}
            rules={graph.rules}
            expanded={expanded}
            onExpand={(side) => setExpanded((current) => ({ ...current, [side]: true }))}
            onFocus={(node) => {
              setExpanded({ left: false, right: false })
              onFocus(node.id)
            }}
          />
        )}
      </div>
      {/* 图的文字版：两侧各是谁。 */}
      {focus !== null && access.data !== undefined ? (
        <dl className="grid grid-cols-2 gap-x-6 border-t px-5 py-3 text-[13px]">
          <div>
            <dt className="font-medium">
              {g.reachableBy}（{access.data.reachable_by.length}）
            </dt>
            <dd className="mt-0.5 text-muted-foreground">
              {access.data.reachable_by.length === 0
                ? g.nobody
                : access.data.reachable_by.map((edge) => edge.node.name).join('、')}
            </dd>
          </div>
          <div>
            <dt className="font-medium">
              {g.canReach}（{access.data.can_reach.length}）
            </dt>
            <dd className="mt-0.5 text-muted-foreground">
              {access.data.can_reach.length === 0
                ? g.nobody
                : access.data.can_reach.map((edge) => edge.node.name).join('、')}
            </dd>
          </div>
        </dl>
      ) : null}
    </>
  )
}

/**
 * 访问控制：左边规则，右边关系图，放在同一个页面上。
 * 一条规则只有一行，改一下、右边的图跟着变，这种即时的因果是这个页面的全部价值。窗口窄时改成两个标签页。
 */
export function AccessPage() {
  useDocumentTitle(t.access.title)
  const network = useNetwork()
  const graph = useQuery(q.graph(network.id))
  const nodes = useQuery(q.nodes(network.id))
  const [params, setParams] = useSearchParams()

  const view: View = params.get('view') === 'node' ? 'node' : 'tags'
  const focus = numberParam(params, 'node')
  const checking = params.get('check') === '1'
  const checkFrom = numberParam(params, 'from')
  const checkTo = numberParam(params, 'to')

  const [panel, setPanel] = useState<'rules' | 'graph'>('rules')
  const [editor, setEditor] = useState<Editor>(null)
  const [hovered, setHovered] = useState<number | null>(null)
  const [selected, setSelected] = useState<number | null>(null)
  const [openTag, setOpenTag] = useState<string | null>(null)

  const update = (changes: Record<string, string | null>) => {
    const next = new URLSearchParams(params)
    for (const [key, value] of Object.entries(changes)) {
      if (value === null) next.delete(key)
      else next.set(key, value)
    }
    setParams(next, { replace: true })
  }

  const restore = useAction(network.id, {
    mutationFn: () =>
      api.createRule(network.id, { name: 'default', src: 'all', dst: 'all', direction: 'both', enabled: true }),
    success: t.access.restored,
  })

  const data = graph.data
  const enabledCount = data?.rules.filter((rule) => rule.enabled).length ?? 0

  return (
    <Page wide className="@container">
      <PageHeader
        title={t.access.title}
        description={t.access.description}
        actions={
          <>
            <Button onClick={() => update({ check: checking ? null : '1' })} aria-pressed={checking}>
              <ScanSearch aria-hidden />
              {t.access.check}
            </Button>
            <Button variant="primary" onClick={() => setEditor({ rule: null })} disabled={data === undefined}>
              <Plus aria-hidden />
              {t.access.newRule}
            </Button>
          </>
        }
      />

      {checking ? (
        <AccessCheck
          network={network}
          nodes={nodes.data ?? []}
          from={checkFrom}
          to={checkTo}
          onChange={(from, to) =>
            update({ from: from === null ? null : String(from), to: to === null ? null : String(to) })
          }
          onClose={() => update({ check: null, from: null, to: null })}
          onCreateRule={(draft) => setEditor({ rule: null, draft })}
        />
      ) : null}

      {graph.isError && data === undefined ? (
        <Card>
          <ErrorState title={t.access.loadFailed} error={graph.error} onRetry={() => void graph.refetch()} />
        </Card>
      ) : data === undefined ? (
        <Card>
          <SkeletonBlock lines={6} />
        </Card>
      ) : (
        <>
          <Segmented
            aria-label={t.access.layoutLabel}
            value={panel}
            onChange={setPanel}
            options={[
              { value: 'rules', label: t.access.tabRules },
              { value: 'graph', label: t.access.tabGraph },
            ]}
            className="mb-4 @4xl:hidden"
          />
          <div className="grid grid-cols-1 items-start gap-5 @4xl:grid-cols-[minmax(0,27rem)_minmax(0,1fr)]">
            <div className={cn('space-y-5', panel === 'graph' && 'hidden @4xl:block')}>
              <Card>
                <CardHeader
                  title={t.access.rulesTitle}
                  description={data.rules.length > 0 ? t.access.rulesCount(enabledCount, data.rules.length) : undefined}
                />
                {data.rules.length === 0 ? (
                  <EmptyState
                    icon={ShieldOff}
                    title={t.access.emptyTitle}
                    description={t.access.emptyBody}
                    action={
                      <>
                        <Button pending={restore.isPending} onClick={() => restore.mutate()}>
                          {t.access.restoreDefault}
                        </Button>
                        <Button variant="primary" onClick={() => setEditor({ rule: null })}>
                          <Plus aria-hidden />
                          {t.access.newRule}
                        </Button>
                      </>
                    }
                  />
                ) : (
                  <RuleList
                    network={network}
                    graph={data}
                    highlighted={hovered ?? selected}
                    focused={selected}
                    onHighlight={setHovered}
                    onEdit={(rule) => setEditor({ rule })}
                  />
                )}
              </Card>

              <Card>
                <CardHeader title={t.access.tagsPanel.title} />
                <TagPanel network={network} graph={data} open={openTag} onOpenChange={setOpenTag} />
              </Card>
            </div>

            <Card className={cn(panel === 'rules' && 'hidden @4xl:block')}>
              <CardHeader
                title={g.title}
                action={
                  <Segmented
                    aria-label={g.viewLabel}
                    size="sm"
                    value={view}
                    onChange={(next) => update({ view: next === 'tags' ? null : next })}
                    options={[
                      { value: 'tags', label: g.byTag },
                      { value: 'node', label: g.byNode },
                    ]}
                  />
                }
              />
              {data.unacknowledged.length > 0 ? (
                <div className="border-b px-5 py-3">
                  <Notice tone="warn">{g.unacked(data.unacknowledged.map((node) => node.name).join('、'))}</Notice>
                </div>
              ) : null}

              {view === 'tags' ? (
                <>
                  <div className="h-[520px]" data-testid="tag-graph">
                    <TagGraphView
                      graph={data}
                      highlightedRule={hovered ?? selected}
                      activeTag={openTag}
                      onRuleClick={setSelected}
                      onRuleOpen={(id) => {
                        const rule = data.rules.find((candidate) => candidate.id === id)
                        if (rule !== undefined) setEditor({ rule })
                      }}
                      onRuleHover={setHovered}
                      onTagClick={(tag) => {
                        setOpenTag(tag)
                        setPanel('rules')
                      }}
                    />
                  </div>
                  <div className="flex flex-wrap items-center justify-between gap-x-6 gap-y-1.5 border-t px-5 py-3">
                    <Legend />
                    <p className="text-xs text-muted-foreground">{g.tagHint}</p>
                  </div>
                </>
              ) : (
                <NodeView network={network} graph={data} focus={focus} onFocus={(id) => update({ node: String(id) })} />
              )}

              <div className="border-t px-5 py-3 text-[13px]" data-testid="unconnected">
                {nodes.data?.length === 0 ? (
                  <p className="text-muted-foreground">{g.noNodes}</p>
                ) : data.unconnected.length === 0 ? (
                  <p className="text-muted-foreground">{g.unconnectedNone}</p>
                ) : (
                  <>
                    <p className="font-medium text-warn">{g.unconnected(data.unconnected.length)}</p>
                    <ul className="mt-1.5 flex flex-wrap gap-x-4 gap-y-1">
                      {data.unconnected.map((node) => (
                        <li key={node.id}>
                          <NodeLink network={network.id} node={node} className="font-normal" />
                        </li>
                      ))}
                    </ul>
                  </>
                )}
              </div>
              <p className="border-t px-5 py-2.5 text-xs text-muted-foreground">{g.textEquivalent}</p>
            </Card>
          </div>
        </>
      )}

      {editor !== null && data !== undefined ? (
        <RuleDialog
          key={editor.rule?.id ?? 'new'}
          network={network}
          graph={data}
          rule={editor.rule}
          draft={editor.draft}
          open
          onClose={() => setEditor(null)}
        />
      ) : null}
    </Page>
  )
}
