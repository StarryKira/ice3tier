import { useQuery } from '@tanstack/react-query'
import { useState } from 'react'
import { useNavigate } from 'react-router'

import { fieldError, formError, formFailure } from '@/api/formErrors'
import { api } from '@/api/endpoints'
import { q, useAction } from '@/api/queries'
import { type Network, type Node, type NodeId } from '@/api/types'
import { useConfirm } from '@/components/confirm'
import { FormDialog } from '@/components/form-dialog'
import { NodeSelect } from '@/components/node'
import { nodePath } from '@/lib/nodes'
import { Notice } from '@/components/notice'
import { Field } from '@/components/ui/field'
import { Input } from '@/components/ui/input'
import { t } from '@/i18n'
import { validateVirtualIp } from '@/lib/validate'

const d = t.nodes.detail

interface NodeDialogProps {
  network: Network
  node: Node
  open: boolean
  onClose: () => void
}

/** 改虚拟 IP。会让写着旧地址的配置失效，所以对话框里固定有一段提醒。 */
export function EditIpDialog({ network, node, open, onClose }: NodeDialogProps) {
  const [value, setValue] = useState(node.virtual_ip)
  const [touched, setTouched] = useState(false)
  const save = useAction(network.id, {
    mutationFn: () => api.updateNode(network.id, node.id, { virtual_ip: value.trim() }),
    success: (updated) => d.ipChanged(updated.virtual_ip),
    toastError: false,
  })
  const failure = formFailure(save.error, {
    address_taken: 'ip',
    address_cooling_down: 'ip',
    address_out_of_range: 'ip',
  } as const)
  const dirty = value.trim() !== node.virtual_ip
  const problem = validateVirtualIp(value, network.cidr)

  return (
    <FormDialog
      open={open}
      onClose={onClose}
      title={d.ipTitle(node.name)}
      size="sm"
      dirty={dirty}
      requireDirty
      submitLabel={d.ipSubmit}
      pending={save.isPending}
      error={formError(failure)}
      onSubmit={() => {
        setTouched(true)
        if (problem === null) save.mutate(undefined, { onSuccess: onClose })
      }}
    >
      <Field
        label={d.ipLabel}
        hint={d.ipHint(network.cidr)}
        error={(touched || dirty ? problem : null) ?? fieldError(failure, 'ip')}
      >
        {(props) => (
          <Input
            {...props}
            className="font-mono"
            value={value}
            autoFocus
            onChange={(event) => {
              setValue(event.target.value)
              save.reset()
            }}
          />
        )}
      </Field>
      <Notice tone="warn">{d.ipWarning}</Notice>
    </FormDialog>
  )
}

/**
 * 继任：让这个节点接手一个已下线节点的身份。旧节点随即被删除，不能撤回，所以提交前是危险级别的确认。
 */
export function SucceedDialog({ network, node, open, onClose }: NodeDialogProps) {
  const confirm = useConfirm()
  const navigate = useNavigate()
  const nodes = useQuery(q.nodes(network.id))
  const [predecessor, setPredecessor] = useState<NodeId | null>(null)
  const [touched, setTouched] = useState(false)
  const candidates = (nodes.data ?? []).filter((other) => other.id !== node.id)
  const chosen = candidates.find((other) => other.id === predecessor)

  const succeed = useAction(network.id, {
    mutationFn: (id: NodeId) => api.succeed(network.id, node.id, { predecessor: id }),
    success: (updated) => d.succeeded(updated.name),
    toastError: false,
  })
  const failure = formFailure(succeed.error, { predecessor_online: 'predecessor' } as const)

  const submit = async () => {
    setTouched(true)
    if (chosen === undefined) return
    const confirmed = await confirm({
      tone: 'danger',
      title: d.succeedConfirmTitle(node.name, chosen.name),
      body: (
        <>
          <p>{d.succeedInherits(node.name, chosen.name)}</p>
          <p>{d.succeedDeletes(chosen.name)}</p>
        </>
      ),
      confirmLabel: d.succeedConfirm,
    })
    if (!confirmed) return
    succeed.mutate(chosen.id, {
      onSuccess: () => {
        onClose()
        void navigate(nodePath(network.id, node.id), { replace: true })
      },
    })
  }

  return (
    <FormDialog
      open={open}
      onClose={onClose}
      title={d.succeedTitle(node.name)}
      description={d.succeedLead}
      dirty={predecessor !== null}
      submitLabel={d.succeedSubmit}
      pending={succeed.isPending}
      error={formError(failure)}
      onSubmit={() => void submit()}
    >
      <Field
        label={d.succeedLabel}
        hint={d.succeedOnlyOffline}
        error={(touched && chosen === undefined ? t.validate.pickNode : null) ?? fieldError(failure, 'predecessor')}
      >
        {(props) => (
          <NodeSelect
            {...props}
            nodes={candidates}
            value={predecessor}
            onChange={(id) => {
              setPredecessor(id)
              succeed.reset()
            }}
            placeholder={d.succeedPlaceholder}
            disabledReason={(other) => (other.online ? d.succeedOnline : null)}
          />
        )}
      </Field>
      {chosen !== undefined ? (
        <div className="space-y-1.5 text-[13px] text-muted-foreground">
          <p>{d.succeedInherits(node.name, chosen.name)}</p>
          <p>{d.succeedNotInherits}</p>
          <p className="font-medium text-danger">{d.succeedDeletes(chosen.name)}</p>
        </div>
      ) : null}
    </FormDialog>
  )
}
