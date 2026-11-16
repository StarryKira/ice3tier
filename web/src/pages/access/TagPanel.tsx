import { useQuery } from '@tanstack/react-query'
import { ChevronRight, Pencil, X } from 'lucide-react'
import { useEffect, useRef, useState } from 'react'

import { api } from '@/api/endpoints'
import { q, useAction } from '@/api/queries'
import { type AccessGraph, type Network, type Node, type NodeId } from '@/api/types'
import { useConfirm } from '@/components/confirm'
import { NodeLink, NodeSelect } from '@/components/node'
import { ruleLabel } from '@/lib/rules'
import { TagChip } from '@/components/tags'
import { Button } from '@/components/ui/button'
import { t } from '@/i18n'
import { cn } from '@/lib/utils'
import { ALL_TAG } from '@/lib/validate'

import { RenameTagDialog } from './RenameTagDialog'

const p = t.access.tagsPanel

interface TagPanelProps {
  network: Network
  graph: AccessGraph
  /** 展开着的那个标签。图上点一个标签，这里对应的一行展开。 */
  open: string | null
  onOpenChange: (tag: string | null) => void
}

/**
 * 标签面板：从节点和规则派生出来的。每个标签一行，显示成员数、被几条规则用到；展开可以增减成员。
 * 标签没有"新建"和"删除"：给节点打上就有了，没有节点带着、也没有规则用到时自然消失。
 */
export function TagPanel({ network, graph, open, onOpenChange }: TagPanelProps) {
  const confirm = useConfirm()
  const nodes = useQuery(q.nodes(network.id))
  const all = nodes.data ?? []
  const rows = useRef(new Map<string, HTMLLIElement>())
  const [renaming, setRenaming] = useState<string | null>(null)

  useEffect(() => {
    if (open !== null) rows.current.get(open)?.scrollIntoView({ block: 'nearest', behavior: 'smooth' })
  }, [open])

  const setTags = useAction(network.id, {
    mutationFn: ({ node, tags }: { node: Node; tags: string[]; message: string }) =>
      api.updateNode(network.id, node.id, { tags }),
    success: (_data, vars) => vars.message,
  })

  const add = (tag: string, id: NodeId) => {
    const node = all.find((candidate) => candidate.id === id)
    if (node === undefined) return
    setTags.mutate({ node, tags: [...node.tags, tag].sort(), message: p.added(node.name, tag) })
  }

  const remove = async (tag: string, id: NodeId, memberCount: number) => {
    const node = all.find((candidate) => candidate.id === id)
    if (node === undefined) return
    const users = graph.rules.filter((rule) => rule.enabled && (rule.src === tag || rule.dst === tag))
    // 从最后一个节点上摘掉、而规则还在用它：这些规则会落空，先说一声。
    if (memberCount === 1 && users.length > 0) {
      const confirmed = await confirm({
        title: p.lastTitle(tag),
        body: p.lastBody(tag, users.map(ruleLabel).join('、')),
        confirmLabel: p.lastConfirm,
      })
      if (!confirmed) return
    }
    setTags.mutate({ node, tags: node.tags.filter((other) => other !== tag), message: p.removed(node.name, tag) })
  }

  return (
    <div>
      <p className="mb-3 text-[13px] text-muted-foreground">{p.hint}</p>
      <ul className="-mx-3 divide-y border-t">
        {graph.tags.map((entry) => {
          const isOpen = open === entry.tag
          const builtin = entry.tag === ALL_TAG
          const usedBy = graph.rules.filter((rule) => rule.src === entry.tag || rule.dst === entry.tag).length
          const candidates = all.filter((node) => !node.tags.includes(entry.tag))
          return (
            <li
              key={entry.tag}
              ref={(element) => {
                if (element === null) rows.current.delete(entry.tag)
                else rows.current.set(entry.tag, element)
              }}
            >
              <button
                type="button"
                onClick={() => onOpenChange(isOpen ? null : entry.tag)}
                aria-expanded={isOpen}
                aria-label={p.toggle(entry.tag, isOpen)}
                className="flex w-full items-center gap-2.5 rounded-md px-3 py-2.5 text-left hover:bg-muted/60"
              >
                <ChevronRight
                  className={cn('size-3.5 shrink-0 text-faint transition-transform', isOpen && 'rotate-90')}
                  aria-hidden
                />
                <TagChip tag={entry.tag} />
                <span className="text-[13px] text-muted-foreground tabular">{p.members(entry.nodes.length)}</span>
                <span className="ml-auto text-[13px] text-muted-foreground tabular">{p.usedBy(usedBy)}</span>
              </button>
              {isOpen ? (
                <div className="space-y-2.5 pt-1 pr-3 pb-4 pl-9">
                  {builtin ? <p className="text-[13px] text-muted-foreground">{p.allMembers}</p> : null}
                  {entry.nodes.length === 0 ? (
                    <p className="text-[13px] text-muted-foreground">{p.noMembers}</p>
                  ) : (
                    <ul className="flex flex-wrap gap-1.5">
                      {entry.nodes.map((member) => (
                        <li
                          key={member.id}
                          className="inline-flex h-7 items-center gap-1 rounded-md bg-muted pr-1 pl-2.5 text-[13px]"
                        >
                          <NodeLink
                            network={network.id}
                            node={member}
                            online={all.find((node) => node.id === member.id)?.online}
                            className="font-normal"
                          />
                          {builtin ? (
                            <span className="w-1.5" />
                          ) : (
                            <button
                              type="button"
                              disabled={setTags.isPending}
                              onClick={() => void remove(entry.tag, member.id, entry.nodes.length)}
                              aria-label={p.removeMember(member.name, entry.tag)}
                              className="flex size-5 items-center justify-center rounded text-muted-foreground hover:bg-foreground/10 hover:text-foreground"
                            >
                              <X className="size-3.5" aria-hidden />
                            </button>
                          )}
                        </li>
                      ))}
                    </ul>
                  )}
                  {builtin ? null : (
                    <div className="flex flex-wrap items-center gap-2">
                      <div className="w-64">
                        <NodeSelect
                          nodes={candidates}
                          value={null}
                          onChange={(id) => add(entry.tag, id)}
                          placeholder={candidates.length === 0 ? p.noCandidates : p.addMember}
                          disabled={candidates.length === 0 || setTags.isPending}
                          aria-label={p.addMemberTo(entry.tag)}
                        />
                      </div>
                      <Button
                        size="sm"
                        variant="ghost"
                        onClick={() => setRenaming(entry.tag)}
                        aria-label={p.renameFor(entry.tag)}
                      >
                        <Pencil aria-hidden />
                        {p.rename}
                      </Button>
                    </div>
                  )}
                </div>
              ) : null}
            </li>
          )
        })}
      </ul>
      {renaming !== null ? (
        <RenameTagDialog
          network={network}
          graph={graph}
          tag={renaming}
          onClose={() => setRenaming(null)}
          onRenamed={(to) => onOpenChange(to)}
        />
      ) : null}
    </div>
  )
}
