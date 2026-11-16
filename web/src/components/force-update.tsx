import { useQuery } from '@tanstack/react-query'
import { useState } from 'react'

import { errorMessage } from '@/api/client'
import { api } from '@/api/endpoints'
import { q, useAction } from '@/api/queries'
import { type ForcedUpdates, type ForceTiming, type Node } from '@/api/types'
import { Button } from '@/components/ui/button'
import { Dialog, DialogBody, DialogContent, DialogFooter } from '@/components/ui/dialog'
import { FormError } from '@/components/ui/field'
import { RadioCards } from '@/components/ui/radio-cards'
import { Skeleton } from '@/components/ui/skeleton'
import { t } from '@/i18n'
import { relayedPairs } from '@/topology/route'

const f = t.updates.force

function TimingChoice({
  value,
  onChange,
  flows,
}: {
  value: ForceTiming
  onChange: (v: ForceTiming) => void
  flows: number | null
}) {
  return (
    <RadioCards<ForceTiming>
      aria-label={f.timingLabel}
      value={value}
      onChange={onChange}
      options={[
        { value: 'now', title: f.now, description: flows === null ? undefined : f.nowFlows(flows) },
        { value: 'after_drain', title: f.afterDrain, description: f.afterDrainHint },
      ]}
    />
  )
}

interface ForceUpdateDialogProps {
  node: Node
  onClose: () => void
}

/**
 * "立即更新"的确认框（普通确认：会打断正在进行的连接）。
 *
 * 写出具体的后果：它现在有几条流（从节点详情取）；有流时让人在"马上"和"排空后"之间选，没有流时只是一个普通的确认。
 * 它是 relay 时说明重启对中转的影响。算不出来的不写。
 */
export function ForceUpdateDialog({ node, onClose }: ForceUpdateDialogProps) {
  const detail = useQuery(q.node(node.network, node.id))
  const nodes = useQuery({ ...q.nodes(node.network), enabled: node.relay !== null })
  const matrix = useQuery({ ...q.matrix(node.network), enabled: node.relay !== null })
  const [timing, setTiming] = useState<ForceTiming>('now')
  const [error, setError] = useState<string | null>(null)
  const version = node.update.available ?? ''

  const force = useAction(node.network, {
    mutationFn: (chosen: ForceTiming) => api.forceNodeUpdate(node.network, node.id, { timing: chosen }),
    toastError: false,
    success: (result) =>
      result.ordered !== null
        ? f.ordered(node.name, result.ordered.version)
        : result.waiting === 'package'
          ? f.orderedWaitingPackage(node.name)
          : f.ordered(node.name, version),
  })

  const flows = detail.data?.flows ?? null
  const otherRelays = (nodes.data ?? []).filter((other) => other.relay !== null && other.id !== node.id).length
  // 没有流时没有可选的：马上更新就是了。
  const chosen: ForceTiming = flows === 0 ? 'now' : timing

  const submit = () => {
    setError(null)
    force.mutate(chosen, { onSuccess: onClose, onError: (cause) => setError(errorMessage(cause)) })
  }

  return (
    <Dialog open onOpenChange={(open) => (open || force.isPending ? undefined : onClose())}>
      <DialogContent title={f.title(node.name, version)} size="sm">
        <DialogBody className="space-y-4 text-sm">
          {detail.isPending ? (
            <div aria-busy="true" aria-label={f.loadingDetail} className="space-y-2">
              <Skeleton className="h-3.5 w-3/5" />
              <Skeleton className="h-14 w-full" />
            </div>
          ) : flows === 0 ? (
            <p>{f.idleBody}</p>
          ) : (
            <TimingChoice value={timing} onChange={setTiming} flows={flows} />
          )}
          {node.relay !== null ? (
            <p className="text-muted-foreground" data-testid="force-relay-note">
              {otherRelays > 0 ? f.relayOthers(otherRelays) : f.relayOnly(relayedPairs(matrix.data, node.id))}
            </p>
          ) : null}
          <p className="text-[13px] text-muted-foreground">{f.controllerNote}</p>
          <FormError message={error} />
        </DialogBody>
        <DialogFooter>
          <Button onClick={onClose} disabled={force.isPending}>
            {t.common.cancel}
          </Button>
          <Button variant="primary" pending={force.isPending} disabled={detail.isPending} onClick={submit}>
            {chosen === 'after_drain' ? f.confirmDrain : f.confirmNow}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

interface ForceAllDialogProps {
  version: string
  /** 有新版本、能自己更新的节点数（其中不在线的这次会跳过）。 */
  candidates: number
  onClose: () => void
  onDone: (result: ForcedUpdates) => void
}

/** "全部更新"的确认框。各个节点现在有多少条流这里算不出来，就照实说，不假装。 */
export function ForceAllDialog({ version, candidates, onClose, onDone }: ForceAllDialogProps) {
  const [timing, setTiming] = useState<ForceTiming>('now')
  const [error, setError] = useState<string | null>(null)
  const force = useAction('all', {
    mutationFn: (chosen: ForceTiming) => api.forceUpdateAll({ timing: chosen }),
    toastError: false,
    success: (result) => t.updates.forceAll.result(result.ordered.length, result.queued.length, result.skipped.length),
  })
  const a = t.updates.forceAll

  const submit = () => {
    setError(null)
    force.mutate(timing, {
      onSuccess: (result) => {
        onDone(result)
        onClose()
      },
      onError: (cause) => setError(errorMessage(cause)),
    })
  }

  return (
    <Dialog open onOpenChange={(open) => (open || force.isPending ? undefined : onClose())}>
      <DialogContent title={a.title(version)} size="sm">
        <DialogBody className="space-y-4 text-sm">
          <p>{a.body(candidates)}</p>
          <TimingChoice value={timing} onChange={setTiming} flows={null} />
          <p className="text-[13px] text-muted-foreground">{a.flowsUnknown}</p>
          <p className="text-[13px] text-muted-foreground">{f.controllerNote}</p>
          <FormError message={error} />
        </DialogBody>
        <DialogFooter>
          <Button onClick={onClose} disabled={force.isPending}>
            {t.common.cancel}
          </Button>
          <Button variant="primary" pending={force.isPending} onClick={submit}>
            {timing === 'after_drain' ? a.confirmDrain : a.confirmNow}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
