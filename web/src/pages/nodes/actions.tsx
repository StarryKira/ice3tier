import { useState } from 'react'

import { fieldError, formError, formErrorDetail, formFailure } from '@/api/formErrors'
import { api } from '@/api/endpoints'
import { useAction } from '@/api/queries'
import { type Network, type Node } from '@/api/types'
import { FormDialog } from '@/components/form-dialog'
import { TagInput } from '@/components/tags'
import { Button } from '@/components/ui/button'
import { Dialog, DialogBody, DialogContent, DialogFooter } from '@/components/ui/dialog'
import { Field } from '@/components/ui/field'
import { Input } from '@/components/ui/input'
import { t } from '@/i18n'
import { validateName } from '@/lib/validate'

import { useKnownTags } from './hooks'

interface NodeDialogProps {
  network: Network
  node: Node
  open: boolean
  onClose: () => void
}

export function RenameNodeDialog({ network, node, open, onClose }: NodeDialogProps) {
  const [name, setName] = useState(node.name)
  const [touched, setTouched] = useState(false)
  const rename = useAction(network.id, {
    mutationFn: () => api.updateNode(network.id, node.id, { name: name.trim().toLowerCase() }),
    success: (updated) => t.nodes.renamed(updated.name),
    toastError: false,
  })
  const failure = formFailure(rename.error, { name_taken: 'name', name_cooling_down: 'name' } as const)
  const problem = validateName(name)
  const dirty = name.trim().toLowerCase() !== node.name

  return (
    <FormDialog
      open={open}
      onClose={onClose}
      title={t.nodes.renameTitle(node.name)}
      size="sm"
      dirty={dirty}
      requireDirty
      submitLabel={t.nodes.renameSubmit}
      pending={rename.isPending}
      error={formError(failure)}
      errorDetail={formErrorDetail(failure)}
      onSubmit={() => {
        setTouched(true)
        if (problem === null) rename.mutate(undefined, { onSuccess: onClose })
      }}
    >
      <Field
        label={t.nodes.renameLabel}
        hint={t.nodes.renameHint}
        error={(touched ? problem : null) ?? fieldError(failure, 'name')}
      >
        {(props) => (
          <Input
            {...props}
            value={name}
            autoFocus
            onChange={(event) => {
              setName(event.target.value)
              rename.reset()
            }}
          />
        )}
      </Field>
    </FormDialog>
  )
}

interface NodeTagsEditorProps {
  network: Network
  node: Node
  id?: string
  'aria-label'?: string
  'aria-describedby'?: string
  autoFocus?: boolean
}

/** 节点的标签。页面上的单个控件：改了立即生效，弹一条提示。 */
export function NodeTagsEditor({ network, node, ...props }: NodeTagsEditorProps) {
  const known = useKnownTags(network)
  const save = useAction(network.id, {
    mutationFn: (tags: string[]) => api.updateNode(network.id, node.id, { tags }),
    success: t.nodes.tagsSaved,
  })
  // 请求还在路上时先显示刚选的，免得标签闪回去再出现。
  const value = save.isPending ? save.variables : node.tags
  return (
    <TagInput
      {...props}
      value={value}
      onChange={(tags) => save.mutate(tags)}
      suggestions={known}
      disabled={save.isPending}
    />
  )
}

export function NodeTagsDialog({ network, node, open, onClose }: NodeDialogProps) {
  return (
    <Dialog open={open} onOpenChange={(next) => (next ? undefined : onClose())}>
      <DialogContent title={t.nodes.tagsTitle(node.name)} description={t.nodes.tagsHint} size="sm">
        <DialogBody>
          <NodeTagsEditor network={network} node={node} aria-label={t.nodes.tagsTitle(node.name)} autoFocus />
        </DialogBody>
        <DialogFooter>
          <Button variant="primary" onClick={onClose}>
            {t.nodes.tagsDone}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
