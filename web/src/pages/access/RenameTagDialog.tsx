import { useState } from 'react'

import { fieldError, formError, formFailure } from '@/api/formErrors'
import { api } from '@/api/endpoints'
import { useAction } from '@/api/queries'
import { type AccessGraph, type Network } from '@/api/types'
import { FormDialog } from '@/components/form-dialog'
import { Notice } from '@/components/notice'
import { Field } from '@/components/ui/field'
import { Input } from '@/components/ui/input'
import { t } from '@/i18n'
import { ALL_TAG, normalizeTag, validateTag } from '@/lib/validate'

const p = t.access.tagsPanel

interface RenameTagDialogProps {
  network: Network
  graph: AccessGraph
  tag: string
  onClose: () => void
  /** 改好之后：面板展开新名字的那一行。 */
  onRenamed: (to: string) => void
}

/**
 * 标签改名，整个网络里一起改：节点上的、规则里的、还没用掉的令牌预设的。
 * 新名字原来就有人用时两组合成一组，拆不回去，所以在保存之前把这一点说出来。
 */
export function RenameTagDialog({ network, graph, tag, onClose, onRenamed }: RenameTagDialogProps) {
  const [value, setValue] = useState(tag)
  const [touched, setTouched] = useState(false)
  const to = normalizeTag(value)
  const problem = validateTag(value) ?? (to === ALL_TAG ? p.renameAll : to === tag ? p.renameSame : null)

  const rename = useAction(network.id, {
    mutationFn: () => api.renameTag(network.id, { from: tag, to }),
    success: (renamed) => p.renamed(renamed.from, renamed.to, renamed.nodes, renamed.rules, renamed.tokens),
    toastError: false,
  })
  const failure = formFailure(rename.error, { invalid_request: 'to' } as const)

  const current = graph.tags.find((entry) => entry.tag === tag)
  const usedBy = graph.rules.filter((rule) => rule.src === tag || rule.dst === tag).length
  const existing = problem === null ? graph.tags.find((entry) => entry.tag === to) : undefined
  const existingRules = graph.rules.some((rule) => rule.src === to || rule.dst === to)

  return (
    <FormDialog
      open
      onClose={onClose}
      title={p.renameTitle(tag)}
      description={p.renameLead}
      size="sm"
      dirty={value.trim() !== tag}
      requireDirty
      submitLabel={p.renameSubmit}
      pending={rename.isPending}
      error={formError(failure)}
      onSubmit={() => {
        setTouched(true)
        if (problem !== null) return
        rename.mutate(undefined, {
          onSuccess: (renamed) => {
            onRenamed(renamed.to)
            onClose()
          },
        })
      }}
    >
      <p className="text-[13px] text-muted-foreground">{p.renameUses(current?.nodes.length ?? 0, usedBy)}</p>
      <Field
        label={p.renameLabel}
        error={(touched || value.trim() !== tag ? problem : null) ?? fieldError(failure, 'to')}
      >
        {(props) => (
          <Input
            {...props}
            name="tag"
            className="font-mono"
            value={value}
            autoFocus
            onChange={(event) => {
              setValue(event.target.value)
              rename.reset()
            }}
          />
        )}
      </Field>
      <div aria-live="polite">
        {problem !== null ? null : existing !== undefined && existing.nodes.length > 0 ? (
          <Notice tone="warn">{p.renameMerge(to, existing.nodes.length)}</Notice>
        ) : existing !== undefined || existingRules ? (
          <Notice tone="warn">{p.renameMergeRules(to)}</Notice>
        ) : (
          <Notice>{p.renameNoEffect}</Notice>
        )}
      </div>
    </FormDialog>
  )
}
