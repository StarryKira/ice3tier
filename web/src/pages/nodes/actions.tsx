import { useQuery } from '@tanstack/react-query'
import { useState } from 'react'

import { fieldError, formError, formErrorDetail, formFailure } from '@/api/formErrors'
import { api } from '@/api/endpoints'
import { q, useAction } from '@/api/queries'
import { type GroupRef, type Network, type Node } from '@/api/types'
import { FormDialog } from '@/components/form-dialog'
import { GroupPicker, GroupsLink } from '@/components/groups'
import { Button } from '@/components/ui/button'
import { Dialog, DialogBody, DialogContent, DialogFooter } from '@/components/ui/dialog'
import { Field } from '@/components/ui/field'
import { Input } from '@/components/ui/input'
import { t } from '@/i18n'
import { validateName } from '@/lib/validate'

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

interface NodeGroupsEditorProps {
  network: Network
  node: Node
  id?: string
  'aria-label'?: string
  'aria-describedby'?: string
  autoFocus?: boolean
}

/** 节点所在的分组：只能从已有的分组里选。页面上的单个控件：勾一下就保存，弹一条提示；保存完之前列表开着，先不能再勾。 */
export function NodeGroupsEditor({ network, node, ...props }: NodeGroupsEditorProps) {
  const groups = useQuery(q.groups(network.id))
  const save = useAction(network.id, {
    mutationFn: (next: GroupRef[]) => api.updateNode(network.id, node.id, { groups: next.map((group) => group.id) }),
    success: t.nodes.groupsSaved,
  })
  // 请求还在路上时先显示刚选的，免得分组闪回去再出现。
  const value = save.isPending ? save.variables : node.groups
  return (
    <GroupPicker
      {...props}
      value={value}
      onChange={(next) => save.mutate(next)}
      groups={groups.data ?? []}
      busy={save.isPending}
    />
  )
}

export function NodeGroupsDialog({ network, node, open, onClose }: NodeDialogProps) {
  return (
    <Dialog open={open} onOpenChange={(next) => (next ? undefined : onClose())}>
      <DialogContent title={t.nodes.groupsTitle(node.name)} description={t.nodes.groupsHint} size="sm">
        <DialogBody>
          <NodeGroupsEditor network={network} node={node} aria-label={t.nodes.groupsTitle(node.name)} autoFocus />
        </DialogBody>
        <DialogFooter className="justify-between">
          <GroupsLink network={network.id} onClick={onClose} />
          <Button variant="primary" onClick={onClose}>
            {t.nodes.groupsDone}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
