import { useQuery } from '@tanstack/react-query'
import { ArrowLeftRight, Link2, Plus, Trash2 } from 'lucide-react'
import { useState } from 'react'

import { api } from '@/api/endpoints'
import { formError, formFailure } from '@/api/formErrors'
import { q, useAction } from '@/api/queries'
import { type Network, type NodeId, type PairSetting } from '@/api/types'
import { FormDialog } from '@/components/form-dialog'
import { NodeLink, NodeSelect } from '@/components/node'
import { nodeName } from '@/lib/nodes'
import { Notice } from '@/components/notice'
import { Page, PageHeader } from '@/components/page'
import { EmptyState, ErrorState, SkeletonRows } from '@/components/states'
import { Table, Td, Th, Tr } from '@/components/table'
import { Button } from '@/components/ui/button'
import { Card } from '@/components/ui/card'
import { Field } from '@/components/ui/field'
import { Switch } from '@/components/ui/switch'
import { Tooltip } from '@/components/ui/tooltip'
import { t } from '@/i18n'
import { useDocumentTitle } from '@/lib/useDocumentTitle'
import { useNetwork } from '@/pages/networks/context'

const p = t.pairs

interface AddPairDialogProps {
  network: Network
  existing: readonly PairSetting[]
  open: boolean
  onClose: () => void
}

function AddPairDialog({ network, existing, open, onClose }: AddPairDialogProps) {
  const nodes = useQuery(q.nodes(network.id))
  const [a, setA] = useState<NodeId | null>(null)
  const [b, setB] = useState<NodeId | null>(null)
  const [touched, setTouched] = useState(false)
  const all = nodes.data ?? []

  const add = useAction(network.id, {
    mutationFn: () => api.setPair(network.id, a ?? 0, b ?? 0, { keep_alive: true }),
    success: () => p.on(nodeName(all, a ?? 0), nodeName(all, b ?? 0)),
    toastError: false,
  })
  const failure = formFailure(add.error, {})

  const same = a !== null && a === b
  const exists =
    a !== null && b !== null && existing.some((pair) => pair.a === Math.min(a, b) && pair.b === Math.max(a, b))
  const problem = a === null || b === null ? t.validate.pickNode : same ? p.same : exists ? p.exists : null

  return (
    <FormDialog
      open={open}
      onClose={onClose}
      title={p.dialogTitle}
      description={p.keepAliveHint}
      dirty={a !== null || b !== null}
      submitLabel={p.submit}
      pending={add.isPending}
      error={formError(failure)}
      onSubmit={() => {
        setTouched(true)
        if (problem === null) add.mutate(undefined, { onSuccess: onClose })
      }}
    >
      <div className="grid grid-cols-[minmax(0,1fr)_auto_minmax(0,1fr)] items-end gap-3">
        <Field label={p.nodeA}>{(props) => <NodeSelect {...props} nodes={all} value={a} onChange={setA} />}</Field>
        <ArrowLeftRight className="mb-2.5 size-4 text-muted-foreground" aria-hidden />
        <Field label={p.nodeB}>{(props) => <NodeSelect {...props} nodes={all} value={b} onChange={setB} />}</Field>
      </div>
      {(touched || same || exists) && problem !== null ? <Notice tone="warn">{problem}</Notice> : null}
    </FormDialog>
  )
}

/** 节点对设置。管理 API 现在只有"连接常驻"这一项。 */
export function PairsPage() {
  useDocumentTitle(p.title)
  const network = useNetwork()
  const pairs = useQuery(q.pairs(network.id))
  const nodes = useQuery(q.nodes(network.id))
  const [adding, setAdding] = useState(false)
  const all = nodes.data ?? []

  const names = (pair: PairSetting) => [nodeName(all, pair.a), nodeName(all, pair.b)] as const

  // 开关拨一下立即生效：可以马上改回来，不打断任何人。
  const setKeepAlive = useAction(network.id, {
    mutationFn: ({ pair, keepAlive }: { pair: PairSetting; keepAlive: boolean }) =>
      api.setPair(network.id, pair.a, pair.b, { keep_alive: keepAlive }),
    success: (_data, { pair, keepAlive }) => (keepAlive ? p.on(...names(pair)) : p.off(...names(pair))),
  })
  const remove = useAction(network.id, {
    mutationFn: (pair: PairSetting) => api.deletePair(network.id, pair.a, pair.b),
    success: (_data, pair) => p.removed(...names(pair)),
  })

  const empty = pairs.isSuccess && pairs.data.length === 0
  const tooFew = nodes.isSuccess && all.length < 2
  const addButton = (
    <Button variant="primary" onClick={() => setAdding(true)} disabled={tooFew}>
      <Plus aria-hidden />
      {p.add}
    </Button>
  )

  return (
    <Page>
      <PageHeader title={p.title} description={p.description} actions={empty ? undefined : addButton} />
      <Card>
        {pairs.isError ? (
          <ErrorState title={p.loadFailed} error={pairs.error} onRetry={() => void pairs.refetch()} />
        ) : empty ? (
          <EmptyState
            icon={Link2}
            title={p.emptyTitle}
            description={tooFew ? `${p.emptyBody} ${p.emptyNeedTwo}` : p.emptyBody}
            action={addButton}
          />
        ) : (
          <>
            <Table>
              <thead>
                <tr>
                  <Th>{p.colPair}</Th>
                  <Th>{p.colKeepAlive}</Th>
                  <Th className="w-12">
                    <span className="sr-only">{t.common.moreActions}</span>
                  </Th>
                </tr>
              </thead>
              <tbody>
                {pairs.isPending ? <SkeletonRows columns={3} rows={2} /> : null}
                {(pairs.data ?? []).map((pair) => {
                  const [a, b] = names(pair)
                  return (
                    <Tr key={`${pair.a}-${pair.b}`} data-testid={`pair-row-${a}-${b}`}>
                      <Td>
                        <span className="inline-flex flex-wrap items-center gap-2.5">
                          <NodeLink
                            network={network.id}
                            node={{ id: pair.a, name: a }}
                            online={all.find((node) => node.id === pair.a)?.online}
                          />
                          <ArrowLeftRight className="size-3.5 text-muted-foreground" aria-hidden />
                          <NodeLink
                            network={network.id}
                            node={{ id: pair.b, name: b }}
                            online={all.find((node) => node.id === pair.b)?.online}
                          />
                        </span>
                      </Td>
                      <Td>
                        <Switch
                          aria-label={p.toggle(a, b)}
                          checked={pair.keep_alive}
                          disabled={setKeepAlive.isPending}
                          onCheckedChange={(keepAlive) => setKeepAlive.mutate({ pair, keepAlive })}
                        />
                      </Td>
                      <Td className="pl-0 text-right">
                        <Tooltip content={p.remove}>
                          <Button
                            variant="ghost"
                            size="icon-sm"
                            aria-label={p.removeFor(a, b)}
                            disabled={remove.isPending}
                            onClick={() => remove.mutate(pair)}
                          >
                            <Trash2 aria-hidden />
                          </Button>
                        </Tooltip>
                      </Td>
                    </Tr>
                  )
                })}
              </tbody>
            </Table>
            <p className="border-t px-4 py-2.5 text-[13px] text-muted-foreground">{p.keepAliveHint}</p>
          </>
        )}
      </Card>
      {adding ? (
        <AddPairDialog network={network} existing={pairs.data ?? []} open onClose={() => setAdding(false)} />
      ) : null}
    </Page>
  )
}
