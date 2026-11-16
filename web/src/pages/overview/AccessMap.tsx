import { useQuery } from '@tanstack/react-query'
import {
  Background,
  BackgroundVariant,
  Controls,
  type Edge,
  type EdgeProps,
  getBezierPath,
  Handle,
  type Node as FlowNode,
  type NodeProps,
  Position,
  ReactFlow,
} from '@xyflow/react'
import { ChevronsUpDown, Server, Tag } from 'lucide-react'
import { useMemo } from 'react'
import { Link, useSearchParams } from 'react-router'

import {
  buildAccessMap,
  layoutAccessMap,
  MAP_SIZE,
  type MapFlow,
  type MapRule,
  type MapTarget,
  nodeTags,
} from '@/access/map'
import { q } from '@/api/queries'
import { type AccessGraph, type Network, type Node } from '@/api/types'
import { ErrorState } from '@/components/states'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import { Segmented } from '@/components/ui/segmented'
import { t, useLocale } from '@/i18n'
import { useResolvedTheme } from '@/lib/theme'
import { cn } from '@/lib/utils'
import { networkPath } from '@/pages/networks/context'

import { AttentionChip } from './Attention'

const m = t.overview.map

type View = 'node' | 'tag'

/** 左边那一块：选中的节点或标签，点开换一个。 */
interface SubjectData extends Record<string, unknown> {
  view: View
  title: string
  subtitle: string
  online: boolean | null
  options: { key: string; title: string; subtitle: string; online: boolean | null; selected: boolean }[]
  onPick: (key: string) => void
}

interface PillData extends Record<string, unknown> {
  entry: MapRule
  width: number
  label: string
  href: string
}

interface TargetData extends Record<string, unknown> {
  target: MapTarget
  onOpen: (tag: string) => void
}

interface NoteData extends Record<string, unknown> {
  text: string
}

type SubjectNode = FlowNode<SubjectData, 'subject'>
type PillNode = FlowNode<PillData, 'pill'>
type TargetNode = FlowNode<TargetData, 'target'>
type NoteNode = FlowNode<NoteData, 'note'>
type MapNode = SubjectNode | PillNode | TargetNode | NoteNode

interface FlowEdgeData extends Record<string, unknown> {
  flow: MapFlow
  enabled: boolean
}
type MapEdge = Edge<FlowEdgeData, 'flow'>

const HIDDEN_HANDLE = '!size-1 !min-h-0 !min-w-0 !border-0 !bg-transparent'

/** 图标放在一个浅灰的圆角方块里；节点带一个在线与否的小圆点。 */
function IconBox({ kind, online }: { kind: View; online: boolean | null }) {
  const Icon = kind === 'node' ? Server : Tag
  return (
    <span className="relative flex size-9 shrink-0 items-center justify-center rounded-lg bg-muted text-muted-foreground">
      <Icon className="size-[18px]" strokeWidth={1.75} aria-hidden />
      {online !== null ? (
        <span
          aria-hidden
          className={cn(
            'absolute -right-0.5 -bottom-0.5 size-2.5 rounded-full ring-2 ring-card',
            online ? 'bg-ok-dot' : 'bg-idle-dot',
          )}
        />
      ) : null}
    </span>
  )
}

function SubjectView({ data }: NodeProps<SubjectNode>) {
  return (
    <div style={{ width: MAP_SIZE.subject.width, height: MAP_SIZE.subject.height }}>
      <DropdownMenu>
        <DropdownMenuTrigger
          aria-label={`${data.view === 'node' ? m.pickNode : m.pickTag}（${data.title}）`}
          className="nodrag nopan flex size-full items-center gap-3 rounded-xl border bg-card px-3 text-left shadow-card transition-colors outline-none hover:border-border-strong focus-visible:ring-2 focus-visible:ring-ring data-[state=open]:border-border-strong"
        >
          <IconBox kind={data.view} online={data.online} />
          <span className="min-w-0 flex-1">
            <span className="block truncate text-sm leading-5 font-medium">{data.title}</span>
            <span className="block truncate text-[13px] leading-[18px] text-muted-foreground tabular">
              {data.subtitle}
            </span>
          </span>
          <ChevronsUpDown className="size-4 shrink-0 text-faint" aria-hidden />
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start" className="max-h-80 w-64 overflow-y-auto">
          {data.options.map((option) => (
            <DropdownMenuItem
              key={option.key}
              onSelect={() => data.onPick(option.key)}
              className={cn(option.selected && 'bg-muted')}
            >
              {option.online !== null ? (
                <span
                  aria-hidden
                  className={cn('size-2 shrink-0 rounded-full', option.online ? 'bg-ok-dot' : 'bg-idle-dot')}
                />
              ) : null}
              <span className="min-w-0 flex-1 truncate">{option.title}</span>
              <span className="shrink-0 text-xs text-muted-foreground tabular">{option.subtitle}</span>
            </DropdownMenuItem>
          ))}
        </DropdownMenuContent>
      </DropdownMenu>
      <Handle type="source" position={Position.Right} className={HIDDEN_HANDLE} isConnectable={false} />
    </div>
  )
}

function PillView({ data }: NodeProps<PillNode>) {
  const { entry } = data
  return (
    <div style={{ width: data.width, height: MAP_SIZE.pill.height }}>
      <Handle type="target" position={Position.Left} className={HIDDEN_HANDLE} isConnectable={false} />
      <Link
        to={data.href}
        aria-label={data.label}
        className={cn(
          'nodrag nopan flex size-full items-center gap-2 rounded-full border bg-card pr-3 pl-3 text-[13px] shadow-card transition-colors hover:border-border-strong',
          !entry.rule.enabled && 'border-dashed text-muted-foreground',
        )}
      >
        <span
          aria-hidden
          className={cn('size-2 shrink-0 rounded-full', entry.rule.enabled ? 'bg-ok-dot' : 'bg-idle-dot')}
        />
        <span className="min-w-0 flex-1 truncate font-medium">{entry.rule.name}</span>
        <span aria-hidden className="h-4 w-px shrink-0 bg-border-strong" />
        <span className="shrink-0 text-xs text-muted-foreground">
          {entry.rule.enabled ? (entry.rule.direction === 'both' ? m.both : m.oneWay) : m.disabled}
        </span>
      </Link>
      <Handle type="source" position={Position.Right} className={HIDDEN_HANDLE} isConnectable={false} />
    </div>
  )
}

function TargetView({ data }: NodeProps<TargetNode>) {
  const { target } = data
  return (
    <div style={{ width: MAP_SIZE.target.width, height: MAP_SIZE.target.height }}>
      <Handle type="target" position={Position.Left} className={HIDDEN_HANDLE} isConnectable={false} />
      <button
        type="button"
        onClick={() => data.onOpen(target.tag)}
        aria-label={m.tagLabel(target.tag, target.members)}
        className="nodrag nopan flex size-full items-center gap-3 rounded-xl border bg-card px-3 text-left shadow-card transition-colors hover:border-border-strong"
      >
        <IconBox kind="tag" online={null} />
        <span className="min-w-0 flex-1">
          <span className="block truncate text-sm leading-5 font-medium">{target.tag}</span>
          <span className="block truncate text-[13px] leading-[18px] text-muted-foreground tabular">
            {m.members(target.members)}
          </span>
        </span>
      </button>
    </div>
  )
}

function NoteView({ data }: NodeProps<NoteNode>) {
  return (
    <div className="flex h-9 items-center rounded-full border border-dashed px-4 text-[13px] text-muted-foreground">
      <Handle type="target" position={Position.Left} className={HIDDEN_HANDLE} isConnectable={false} />
      {data.text}
    </div>
  )
}

/**
 * 一条连线：虚线，顺着访问的方向流动；双向的画两条，一来一回。启用的是绿的，停用的灰而且不动。
 */
function FlowEdgeView({
  sourceX,
  sourceY,
  targetX,
  targetY,
  sourcePosition,
  targetPosition,
  data,
}: EdgeProps<MapEdge>) {
  const [path] = getBezierPath({ sourceX, sourceY, sourcePosition, targetX, targetY, targetPosition, curvature: 0.4 })
  const flow = data?.flow ?? 'out'
  const enabled = data?.enabled ?? true
  const lines: { offset: number; reverse: boolean }[] =
    flow === 'both'
      ? [
          { offset: -1.75, reverse: false },
          { offset: 1.75, reverse: true },
        ]
      : [{ offset: 0, reverse: flow === 'in' }]
  return (
    <g>
      {lines.map((line) => (
        <path
          key={line.offset}
          d={path}
          transform={`translate(0 ${line.offset})`}
          fill="none"
          className={cn(enabled && 'access-map-dash', line.reverse && 'access-map-dash-reverse')}
          style={{
            stroke: enabled ? 'var(--ok-dot)' : 'var(--idle-dot)',
            strokeWidth: 1.4,
            strokeDasharray: enabled ? '6 5' : '3 4',
          }}
        />
      ))}
    </g>
  )
}

const nodeTypes = { subject: SubjectView, pill: PillView, target: TargetView, note: NoteView }
const edgeTypes = { flow: FlowEdgeView }

const byName = <T extends { name: string }>(a: T, b: T) => a.name.localeCompare(b.name, undefined, { numeric: true })

/** 默认看哪个节点：名字排第一的在线节点；都不在线时排第一的那个。 */
function defaultNode(nodes: readonly Node[]): Node | undefined {
  const sorted = [...nodes].sort(byName)
  return sorted.find((node) => node.online) ?? sorted[0]
}

function buildFlow(
  graph: AccessGraph,
  nodes: readonly Node[],
  network: Network,
  view: View,
  node: Node | undefined,
  tag: string,
  pick: (key: string) => void,
  openTag: (tag: string) => void,
): { nodes: MapNode[]; edges: MapEdge[] } {
  const members = new Map(graph.tags.map((entry) => [entry.tag, entry.nodes.length]))
  const subjectName = view === 'node' ? (node?.name ?? '') : tag
  const map = buildAccessMap(graph, view === 'node' ? nodeTags(node?.tags ?? []) : new Set([tag]))
  const layout = layoutAccessMap(map, (direction) => (direction === 'both' ? m.both : m.oneWay))

  const options: SubjectData['options'] =
    view === 'node'
      ? [...nodes].sort(byName).map((entry) => ({
          key: String(entry.id),
          title: entry.name,
          subtitle: entry.virtual_ip,
          online: entry.online,
          selected: entry.id === node?.id,
        }))
      : graph.tags.map((entry) => ({
          key: entry.tag,
          title: entry.tag,
          subtitle: m.members(entry.nodes.length),
          online: null,
          selected: entry.tag === tag,
        }))

  const subject: SubjectNode = {
    id: 'subject',
    type: 'subject',
    position: { x: layout.subject.x, y: layout.subject.y },
    data: {
      view,
      title: subjectName,
      subtitle: view === 'node' ? (node?.virtual_ip ?? '') : m.members(members.get(tag) ?? 0),
      online: view === 'node' ? (node?.online ?? false) : null,
      options,
      onPick: pick,
    },
    draggable: false,
    selectable: false,
  }

  const flowText = (entry: MapRule) =>
    entry.flow === 'both'
      ? m.flowBoth(subjectName, entry.other)
      : entry.flow === 'out'
        ? m.flowOut(subjectName, entry.other)
        : m.flowIn(subjectName, entry.other)

  const pills: PillNode[] = map.rules.map((entry) => {
    const place = layout.pills.get(entry.id) ?? { x: 0, y: 0, width: MAP_SIZE.pill.min }
    return {
      id: entry.id,
      type: 'pill',
      position: { x: place.x, y: place.y },
      data: {
        entry,
        width: place.width,
        label: m.ruleLabel(entry.rule.name, entry.rule.enabled ? flowText(entry) : m.disabled),
        href: networkPath(network.id, 'access'),
      },
      draggable: false,
      selectable: false,
    }
  })

  const targets: TargetNode[] = map.targets.map((entry) => {
    const place = layout.targets.get(entry.id) ?? { x: 0, y: 0, width: MAP_SIZE.target.width }
    return {
      id: entry.id,
      type: 'target',
      position: { x: place.x, y: place.y },
      data: { target: entry, onOpen: openTag },
      draggable: false,
      selectable: false,
    }
  })

  const edges: MapEdge[] = map.rules.flatMap((entry) => {
    const data = { flow: entry.flow, enabled: entry.rule.enabled }
    return [
      { id: `in:${entry.id}`, type: 'flow', source: 'subject', target: entry.id, data, focusable: false },
      {
        id: `out:${entry.id}`,
        type: 'flow',
        source: entry.id,
        target: `target:${entry.other}`,
        data,
        focusable: false,
      },
    ]
  })

  if (map.rules.length === 0) {
    const note: NoteNode = {
      id: 'note',
      type: 'note',
      position: { x: MAP_SIZE.subject.width + MAP_SIZE.gapX, y: -MAP_SIZE.pill.height / 2 },
      data: { text: m.noRules },
      draggable: false,
      selectable: false,
    }
    return { nodes: [subject, note], edges: [] }
  }
  return { nodes: [subject, ...pills, ...targets], edges }
}

/**
 * 总览的访问地图：整个面板是一块点阵的画布。左上角换视角（节点、标签），右上角是整个网络的状态。
 * 看哪个写在地址里（`view`、`node`、`tag`），刷新、分享都还是这一个。
 */
export function AccessMap({ network, nodes }: { network: Network; nodes: readonly Node[] }) {
  const locale = useLocale()
  const theme = useResolvedTheme()
  const [params, setParams] = useSearchParams()
  const graph = useQuery(q.graph(network.id))

  const view: View = params.get('view') === 'tag' ? 'tag' : 'node'
  const nodeParam = Number(params.get('node'))
  const node = nodes.find((entry) => entry.id === nodeParam) ?? defaultNode(nodes)
  const tags = graph.data?.tags.map((entry) => entry.tag) ?? []
  const tagParam = params.get('tag') ?? ''
  const tag = tags.includes(tagParam) ? tagParam : tags.includes('all') ? 'all' : (tags[0] ?? 'all')

  const update = (next: Record<string, string>) => {
    setParams(
      (current) => {
        const merged = new URLSearchParams(current)
        for (const [key, value] of Object.entries(next)) merged.set(key, value)
        return merged
      },
      { replace: true },
    )
  }
  const pick = (key: string) => update(view === 'node' ? { node: key } : { tag: key })
  const openTag = (name: string) => update({ view: 'tag', tag: name })

  const flow = useMemo(
    () => (graph.data === undefined ? null : buildFlow(graph.data, nodes, network, view, node, tag, pick, openTag)),
    // pick、openTag 只读地址里的参数，参数变了这里会跟着重算；语言变了文字要重算。
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [graph.data, nodes, network, view, node, tag, locale],
  )
  const subjectKey = view === 'node' ? `node:${node?.id ?? ''}` : `tag:${tag}`

  return (
    <div className="relative min-h-0 flex-1">
      {graph.isError && graph.data === undefined ? (
        <div className="p-8">
          <ErrorState title={m.loadFailed} error={graph.error} onRetry={() => void graph.refetch()} />
        </div>
      ) : flow !== null ? (
        <div className="absolute inset-0">
          <ReactFlow<MapNode, MapEdge>
            // 换了看的对象就整个重画，重新缩放到刚好看全。
            key={subjectKey}
            aria-label={m.canvas}
            colorMode={theme}
            nodes={flow.nodes}
            edges={flow.edges}
            nodeTypes={nodeTypes}
            edgeTypes={edgeTypes}
            fitView
            fitViewOptions={{ padding: 0.25, maxZoom: 1 }}
            minZoom={0.3}
            maxZoom={1.5}
            nodesConnectable={false}
            nodesDraggable={false}
            elementsSelectable={false}
            zoomOnDoubleClick={false}
            deleteKeyCode={null}
          >
            <Background variant={BackgroundVariant.Dots} gap={18} size={1.2} color="var(--border-strong)" />
            <Controls showInteractive={false} position="bottom-left" />
          </ReactFlow>
        </div>
      ) : null}

      <div className="pointer-events-none absolute inset-x-0 top-0 flex items-start justify-between gap-4 p-5">
        <Segmented
          className="pointer-events-auto shadow-nav"
          aria-label={m.view}
          value={view}
          onChange={(next) => update({ view: next })}
          options={[
            {
              value: 'node',
              label: (
                <>
                  <Server className="size-3.5" strokeWidth={1.75} aria-hidden />
                  {m.byNode}
                </>
              ),
            },
            {
              value: 'tag',
              label: (
                <>
                  <Tag className="size-3.5" strokeWidth={1.75} aria-hidden />
                  {m.byTag}
                </>
              ),
            },
          ]}
        />
        <div className="pointer-events-auto">
          <AttentionChip network={network} nodes={nodes} />
        </div>
      </div>
    </div>
  )
}
