import { type ReactNode } from 'react'
import { Link } from 'react-router'

import { type Node, type NodeId, type NodeRef } from '@/api/types'
import { Chip, ChipList } from '@/components/chip'
import { Combobox, type ComboOption } from '@/components/combobox'
import { Flag } from '@/components/flag'
import { OsIcon } from '@/components/os-icon'
import { OnlineDot, StatusDot } from '@/components/status'
import { t } from '@/i18n'
import { nodePath } from '@/lib/nodes'
import { cn } from '@/lib/utils'

interface NodeLinkProps {
  network: number
  node: Pick<NodeRef, 'id' | 'name'>
  /** 知道在线状态时给，名字前面就有那个圆点。 */
  online?: boolean
  className?: string
}

/** 节点的名字，点了去它的详情页。全站指代节点的地方都用它。 */
export function NodeLink({ network, node, online, className }: NodeLinkProps) {
  return (
    <Link
      to={nodePath(network, node.id)}
      className={cn(
        'inline-flex max-w-full items-center gap-1.5 rounded-sm font-medium hover:text-primary hover:underline',
        className,
      )}
    >
      {online !== undefined ? <OnlineDot online={online} /> : null}
      <span className="truncate">{node.name}</span>
    </Link>
  )
}

/**
 * 节点在列表里的记号：在线与否的圆点，后面是国旗；不知道国家时是系统的图标。离线的国旗褪成灰的。
 * 旗子的位置总是留着，名字因此在一列上对齐。
 */
export function NodeMark({ node }: { node: Pick<Node, 'online' | 'country' | 'platform'> }) {
  return (
    <span className="flex shrink-0 items-center gap-1.5">
      <OnlineDot online={node.online} />
      <span className={cn('grid size-4 place-items-center', !node.online && 'opacity-60 grayscale')}>
        {node.country !== null && node.country !== undefined ? (
          <Flag code={node.country} />
        ) : (
          <OsIcon platform={node.platform} className="size-3.5 text-muted-foreground" />
        )}
      </span>
    </span>
  )
}

/**
 * 节点选择器里的一项一项：名字，右边虚拟 IP（离线的加一句离线），按分组和虚拟 IP 也搜得到。
 * 不能选的照样列出来、写出原因，排在能选的后面。
 */
function nodeOptions(nodes: readonly Node[], disabledReason?: (node: Node) => ReactNode | null): ComboOption<NodeId>[] {
  const options = nodes.map((node): ComboOption<NodeId> => {
    const reason = disabledReason?.(node) ?? null
    return {
      value: node.id,
      label: node.name,
      hint: node.online ? node.virtual_ip : `${node.virtual_ip} · ${t.common.offline}`,
      keywords: node.groups.map((group) => group.name).join(' '),
      leading: <NodeMark node={node} />,
      disabled: reason !== null,
      disabledReason: reason ?? undefined,
    }
  })
  return [
    ...options.filter((option) => option.disabled !== true),
    ...options.filter((option) => option.disabled === true),
  ]
}

interface NodeSelectProps {
  id?: string
  nodes: readonly Node[]
  value: NodeId | null
  onChange: (id: NodeId) => void
  placeholder?: string
  /** 某个节点不能选时返回原因；能选返回 `null`。不能选的照样列出来，只是置灰。 */
  disabledReason?: (node: Node) => ReactNode | null
  disabled?: boolean
  /** 给了就用它做按钮里的内容，按钮也不画成输入框（路径诊断里点一端的卡片换节点）。 */
  trigger?: ReactNode
  className?: string
  'aria-label'?: string
  'aria-describedby'?: string
  'aria-invalid'?: true
}

/** 可以搜索的节点选择器，全站选节点的地方都用它。离线的节点照样能选，只是标出来。 */
export function NodeSelect({ nodes, value, onChange, placeholder, disabledReason, ...props }: NodeSelectProps) {
  return (
    <Combobox
      {...props}
      value={value}
      onChange={onChange}
      options={nodeOptions(nodes, disabledReason)}
      placeholder={placeholder ?? t.select.nodePlaceholder}
      searchPlaceholder={t.select.nodeSearch}
      emptyText={t.select.nodeEmpty}
    />
  )
}

/** 节点的小标签：前面是在线与否的圆点，和分组、策略的小标签一个样子。 */
export function NodeChip({ name, online }: { name: string; online?: boolean }) {
  return (
    <Chip
      lead={
        <StatusDot
          tone={online === false ? 'idle' : 'ok'}
          label={online === false ? t.common.offline : t.common.online}
        />
      }
    >
      {name}
    </Chip>
  )
}

/** 一串节点：显示前几个，其余折成"+N"。 */
export function NodeList({
  nodes,
  online,
  max = 3,
  empty,
}: {
  nodes: readonly NodeRef[]
  online: (id: NodeId) => boolean | undefined
  max?: number
  empty?: ReactNode
}) {
  return (
    <ChipList
      items={nodes}
      max={max}
      empty={empty}
      name={(node) => node.name}
      render={(node) => <NodeChip key={node.id} name={node.name} online={online(node.id)} />}
    />
  )
}
