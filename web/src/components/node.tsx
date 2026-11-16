import { type ReactNode } from 'react'
import { Link } from 'react-router'

import { type Node, type NodeId, type NodeRef } from '@/api/types'
import { Combobox, type ComboOption } from '@/components/combobox'
import { OnlineDot } from '@/components/status'
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

interface NodeSelectProps {
  id?: string
  nodes: readonly Node[]
  value: NodeId | null
  onChange: (id: NodeId) => void
  placeholder?: string
  /** 某个节点不能选时返回原因；能选返回 `null`。不能选的照样列出来，只是置灰。 */
  disabledReason?: (node: Node) => ReactNode | null
  disabled?: boolean
  'aria-label'?: string
  'aria-describedby'?: string
  'aria-invalid'?: true
}

/** 可以搜索的节点选择器。离线的节点照样能选，只是标出来。 */
export function NodeSelect({ nodes, value, onChange, placeholder, disabledReason, ...props }: NodeSelectProps) {
  const options: ComboOption<NodeId>[] = nodes.map((node) => {
    const reason = disabledReason?.(node) ?? null
    return {
      value: node.id,
      label: node.name,
      hint: node.online ? node.virtual_ip : `${node.virtual_ip} · ${t.common.offline}`,
      keywords: node.groups.map((group) => group.name).join(' '),
      leading: <OnlineDot online={node.online} />,
      disabled: reason !== null,
      disabledReason: reason ?? undefined,
    }
  })
  return (
    <Combobox
      {...props}
      value={value}
      onChange={onChange}
      options={options}
      placeholder={placeholder ?? t.select.nodePlaceholder}
      searchPlaceholder={t.select.nodeSearch}
      emptyText={t.select.nodeEmpty}
    />
  )
}
