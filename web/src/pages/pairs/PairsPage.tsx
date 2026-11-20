import { useQuery } from '@tanstack/react-query'
import { ArrowLeftRight, Link2, Plus, Trash2 } from 'lucide-react'
import { useCallback, useId, useRef, useState } from 'react'
import { useSearchParams } from 'react-router'

import { api } from '@/api/endpoints'
import { formError, formErrorDetail, formFailure } from '@/api/formErrors'
import { q, useAction } from '@/api/queries'
import { type Network, type Node, type NodeId, type PairCongestion, type PairView } from '@/api/types'
import { FormDialog } from '@/components/form-dialog'
import { NodeLink, NodeSelect } from '@/components/node'
import { Notice } from '@/components/notice'
import { Page, PageHeader } from '@/components/page'
import { EmptyState, ErrorState, SkeletonRows } from '@/components/states'
import { Table, TableNote, Td, Th, Tr } from '@/components/table'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Field } from '@/components/ui/field'
import { NativeSelect } from '@/components/ui/input'
import { Switch } from '@/components/ui/switch'
import { Tooltip } from '@/components/ui/tooltip'
import { t } from '@/i18n'
import { pairStored } from '@/lib/lanes'
import { nodeName } from '@/lib/nodes'
import { useDocumentTitle } from '@/lib/useDocumentTitle'
import { FOCUSED_ROW } from '@/lib/useFocusRow'
import { cn } from '@/lib/utils'
import { useNetwork } from '@/pages/networks/context'
import { readPair } from '@/topology/pair'

const p = t.pairs

/** 拥塞控制的下拉：节点的默认（`null`）、BBR、CUBIC。`<select>` 的值不能是 `null`，默认写成空串。 */
const CONGESTION: readonly (PairCongestion | null)[] = [null, 'bbr', 'cubic']
const congestionLabel = (value: PairCongestion | null | undefined) => p.congestion[value ?? 'default']
const congestionValue = (raw: string): PairCongestion | null => (raw === '' ? null : (raw as PairCongestion))

/** 拥塞控制的下拉。 */
function CongestionSelect({
  value,
  onChange,
  className,
  ...props
}: {
  value: PairCongestion | null | undefined
  onChange: (value: PairCongestion | null) => void
  className?: string
  id?: string
  disabled?: boolean
  'aria-label'?: string
}) {
  return (
    <NativeSelect
      {...props}
      className={className}
      value={value ?? ''}
      onChange={(event) => onChange(congestionValue(event.target.value))}
    >
      {CONGESTION.map((option) => (
        <option key={option ?? 'default'} value={option ?? ''}>
          {congestionLabel(option)}
        </option>
      ))}
    </NativeSelect>
  )
}

const pairKey = (a: NodeId, b: NodeId) => `${Math.min(a, b)}-${Math.max(a, b)}`

interface AddPairDialogProps {
  network: Network
  existing: readonly PairView[]
  open: boolean
  onClose: () => void
}

/**
 * 添加节点对：两个节点和两项设置。连接常驻默认开着，和以前只有这一项时一样；拥塞控制默认用节点的默认。
 * 两项都和默认一样时不用添加。
 */
function AddPairDialog({ network, existing, open, onClose }: AddPairDialogProps) {
  const nodes = useQuery(q.nodes(network.id))
  const [a, setA] = useState<NodeId | null>(null)
  const [b, setB] = useState<NodeId | null>(null)
  const [keepAlive, setKeepAlive] = useState(true)
  const [congestion, setCongestion] = useState<PairCongestion | null>(null)
  const [touched, setTouched] = useState(false)
  const keepAliveId = useId()
  const congestionId = useId()
  const all = nodes.data ?? []

  const add = useAction(network.id, {
    mutationFn: () => api.setPair(network.id, a ?? 0, b ?? 0, { keep_alive: keepAlive, congestion }),
    success: () => p.added(nodeName(all, a ?? 0), nodeName(all, b ?? 0)),
    toastError: false,
  })
  const failure = formFailure(add.error, {})

  const same = a !== null && a === b
  const exists = a !== null && b !== null && existing.some((pair) => pairKey(pair.a, pair.b) === pairKey(a, b))
  const problem =
    a === null || b === null
      ? t.validate.pickNode
      : same
        ? p.same
        : exists
          ? p.exists
          : !keepAlive && congestion === null
            ? p.nothingSet
            : null

  return (
    <FormDialog
      open={open}
      onClose={onClose}
      title={p.dialogTitle}
      dirty={a !== null || b !== null}
      submitLabel={p.submit}
      pending={add.isPending}
      error={formError(failure)}
      errorDetail={formErrorDetail(failure)}
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
      <div className="flex items-start justify-between gap-4">
        <div>
          <label htmlFor={keepAliveId} className="text-[13px] font-medium">
            {p.colKeepAlive}
          </label>
          <p className="mt-0.5 text-[13px] text-muted-foreground">{p.keepAliveHint}</p>
        </div>
        <Switch id={keepAliveId} checked={keepAlive} onCheckedChange={setKeepAlive} />
      </div>
      <div className="flex items-start justify-between gap-4">
        <div>
          <label htmlFor={congestionId} className="text-[13px] font-medium">
            {p.colCongestion}
          </label>
          <p className="mt-0.5 text-[13px] text-muted-foreground">{p.congestionTip}</p>
        </div>
        <CongestionSelect id={congestionId} className="w-40 shrink-0" value={congestion} onChange={setCongestion} />
      </div>
      {(touched || same || exists) && problem !== null ? <Notice tone="warn">{problem}</Notice> : null}
    </FormDialog>
  )
}

/** 表头上带解释的名字：虚线下划线，悬停或聚焦时显示。 */
function TipHeader({ label, tip }: { label: string; tip: string }) {
  return (
    <Tooltip content={tip}>
      <span tabIndex={0} className="cursor-help rounded-sm underline decoration-dotted underline-offset-4">
        {label}
      </span>
    </Tooltip>
  )
}

/**
 * 节点对设置：连接常驻和拥塞控制，改了立即生效（0.1.26 起 ice3proxy 在代理上，这里没有了）。
 * 从节点详情过来时地址里带着 `a`、`b`：那一对亮出来。它没有设置时单独取来，标着"默认"排在最前面，
 * 改一项就存下来，成为列表里的一行。
 */
export function PairsPage() {
  useDocumentTitle(p.title)
  const network = useNetwork()
  const [params] = useSearchParams()
  const pairs = useQuery(q.pairs(network.id))
  const nodes = useQuery(q.nodes(network.id))
  const [adding, setAdding] = useState(false)
  const focus = readPair(params)
  const focusKey = focus.a !== null && focus.b !== null ? pairKey(focus.a, focus.b) : null
  const scrolled = useRef(false)
  const scrollTo = useCallback((element: HTMLElement | null) => {
    if (element === null || scrolled.current) return
    scrolled.current = true
    element.scrollIntoView({ block: 'center' })
  }, [])
  const all: readonly Node[] = nodes.data ?? []

  const names = (pair: PairView) => [nodeName(all, pair.a), nodeName(all, pair.b)] as const

  // 开关和下拉改了立即生效：可以马上改回来，不打断任何正在跑的流。请求只带改的那一项。
  const setKeepAlive = useAction(network.id, {
    mutationFn: ({ pair, keepAlive }: { pair: PairView; keepAlive: boolean }) =>
      api.setPair(network.id, pair.a, pair.b, { keep_alive: keepAlive }),
    success: (_data, { pair, keepAlive }) => (keepAlive ? p.on(...names(pair)) : p.off(...names(pair))),
  })
  const setCongestion = useAction(network.id, {
    mutationFn: ({ pair, congestion }: { pair: PairView; congestion: PairCongestion | null }) =>
      api.setPair(network.id, pair.a, pair.b, { congestion }),
    success: (_data, { pair, congestion }) => p.congestionChanged(...names(pair), congestionLabel(congestion)),
  })
  const remove = useAction(network.id, {
    mutationFn: (pair: PairView) => api.deletePair(network.id, pair.a, pair.b),
    success: (_data, pair) => p.removed(...names(pair)),
  })

  const list = pairs.data ?? []
  const focusListed = list.some((pair) => pairKey(pair.a, pair.b) === focusKey)
  const focusDefault = useQuery({
    ...q.pair(network.id, focus.a ?? 0, focus.b ?? 0),
    enabled: focusKey !== null && focus.a !== focus.b && pairs.isSuccess && !focusListed,
  })
  const rows: readonly PairView[] =
    focusDefault.data !== undefined && !pairStored(focusDefault.data) && !focusListed
      ? [focusDefault.data, ...list]
      : list
  const empty = pairs.isSuccess && rows.length === 0
  const tooFew = nodes.isSuccess && all.length < 2
  const addButton = (
    <Button variant="primary" onClick={() => setAdding(true)} disabled={tooFew}>
      <Plus aria-hidden />
      {p.add}
    </Button>
  )

  return (
    <Page>
      <PageHeader title={p.title} description={p.subtitle} actions={empty ? undefined : addButton} />
      <div>
        {pairs.isError ? (
          <ErrorState title={p.loadFailed} error={pairs.error} onRetry={() => void pairs.refetch()} />
        ) : empty ? (
          <EmptyState
            icon={Link2}
            title={p.emptyTitle}
            description={tooFew ? p.emptyNeedTwo : p.defaultNote}
            action={addButton}
          />
        ) : (
          <>
            <Table>
              <thead>
                <tr>
                  <Th>{p.colPair}</Th>
                  <Th>
                    <TipHeader label={p.colKeepAlive} tip={p.keepAliveHint} />
                  </Th>
                  <Th>
                    <TipHeader label={p.colCongestion} tip={p.congestionTip} />
                  </Th>
                  <Th className="w-12">
                    <span className="sr-only">{t.common.moreActions}</span>
                  </Th>
                </tr>
              </thead>
              <tbody>
                {pairs.isPending ? <SkeletonRows columns={4} rows={2} /> : null}
                {rows.map((pair) => {
                  const [a, b] = names(pair)
                  const stored = pairStored(pair)
                  const key = pairKey(pair.a, pair.b)
                  const focused = focusKey === key
                  return (
                    <Tr
                      key={key}
                      ref={focused ? scrollTo : undefined}
                      className={cn(focused && FOCUSED_ROW)}
                      data-testid={`pair-row-${a}-${b}`}
                    >
                      <Td className="whitespace-nowrap">
                        <span className="inline-flex items-center gap-2.5">
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
                          {stored ? null : (
                            <Tooltip content={p.defaultBadgeTip}>
                              <Badge tone="outline" tabIndex={0}>
                                {p.defaultBadge}
                              </Badge>
                            </Tooltip>
                          )}
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
                      <Td>
                        <CongestionSelect
                          aria-label={p.congestionSelect(a, b)}
                          className="h-8 w-36 text-[13px]"
                          value={pair.congestion}
                          disabled={setCongestion.isPending}
                          onChange={(congestion) => setCongestion.mutate({ pair, congestion })}
                        />
                      </Td>
                      <Td className="pl-0 text-right whitespace-nowrap">
                        {stored ? (
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
                        ) : null}
                      </Td>
                    </Tr>
                  )
                })}
              </tbody>
            </Table>
            <TableNote>{p.defaultNote}</TableNote>
          </>
        )}
      </div>
      {adding ? <AddPairDialog network={network} existing={list} open onClose={() => setAdding(false)} /> : null}
    </Page>
  )
}
