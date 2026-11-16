import { useQuery } from '@tanstack/react-query'
import { ArrowLeftRight, ChevronDown, Link2, Plus, Trash2 } from 'lucide-react'
import { Fragment, type ReactNode, useCallback, useId, useRef, useState } from 'react'
import { Link, useSearchParams } from 'react-router'

import { api } from '@/api/endpoints'
import { formError, formErrorDetail, formFailure } from '@/api/formErrors'
import { q, useAction } from '@/api/queries'
import { type LaneMode, type Network, type Node, type NodeId, type PairLaneSide, type PairView } from '@/api/types'
import { FormDialog } from '@/components/form-dialog'
import { NodeLink, NodeSelect } from '@/components/node'
import { Notice } from '@/components/notice'
import { Page, PageHeader } from '@/components/page'
import { EmptyState, ErrorState, SkeletonRows } from '@/components/states'
import { StatusText } from '@/components/status'
import { Table, TableNote, Td, Th, Tr } from '@/components/table'
import { Button } from '@/components/ui/button'
import { Field } from '@/components/ui/field'
import { NativeSelect } from '@/components/ui/input'
import { RadioCards } from '@/components/ui/radio-cards'
import { Switch } from '@/components/ui/switch'
import { Tooltip } from '@/components/ui/tooltip'
import { t } from '@/i18n'
import { formatBytes } from '@/lib/format'
import {
  handshakeParts,
  handshakeTotal,
  LANE_MODES,
  type LaneFix,
  laneEffectView,
  laneMode,
  laneModeLabel,
  lanesReported,
  pairGlance,
} from '@/lib/lanes'
import { nodeName, nodePath } from '@/lib/nodes'
import { useDocumentTitle } from '@/lib/useDocumentTitle'
import { FOCUSED_ROW } from '@/lib/useFocusRow'
import { cn } from '@/lib/utils'
import { networkPath, useNetwork } from '@/pages/networks/context'
import { readPair } from '@/topology/pair'

const p = t.pairs
const l = t.pairs.lane

const pairKey = (a: NodeId, b: NodeId) => `${Math.min(a, b)}-${Math.max(a, b)}`

interface AddPairDialogProps {
  network: Network
  existing: readonly PairView[]
  open: boolean
  onClose: () => void
}

/** 添加节点对：两个节点和两项设置。连接常驻默认开着，和以前只有这一项时一样。 */
function AddPairDialog({ network, existing, open, onClose }: AddPairDialogProps) {
  const nodes = useQuery(q.nodes(network.id))
  const [a, setA] = useState<NodeId | null>(null)
  const [b, setB] = useState<NodeId | null>(null)
  const [keepAlive, setKeepAlive] = useState(true)
  const [lane, setLane] = useState<LaneMode>('off')
  const [touched, setTouched] = useState(false)
  const keepAliveId = useId()
  const laneLabel = useId()
  const all = nodes.data ?? []

  const add = useAction(network.id, {
    mutationFn: () => api.setPair(network.id, a ?? 0, b ?? 0, { keep_alive: keepAlive, lane }),
    success: () => p.added(nodeName(all, a ?? 0), nodeName(all, b ?? 0)),
    toastError: false,
  })
  const failure = formFailure(add.error, {})

  // 选中的节点里已经有能被拨的 relay 时，"要一个节点启用 relay"就不用说了。
  const relayChosen = all.some((node) => (node.id === a || node.id === b) && (node.relay?.tcp.length ?? 0) > 0)
  const same = a !== null && a === b
  const exists = a !== null && b !== null && existing.some((pair) => pairKey(pair.a, pair.b) === pairKey(a, b))
  const problem =
    a === null || b === null
      ? t.validate.pickNode
      : same
        ? p.same
        : exists
          ? p.exists
          : !keepAlive && lane === 'off'
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
      <div>
        <p id={laneLabel} className="mb-1.5 text-[13px] font-medium">
          {l.title}
        </p>
        <RadioCards<LaneMode>
          aria-labelledby={laneLabel}
          columns={3}
          value={lane}
          onChange={setLane}
          options={LANE_MODES.map((mode) => ({ value: mode, title: laneModeLabel(mode) }))}
        />
        {lane !== 'off' && !relayChosen ? <p className="mt-1 text-[13px] text-muted-foreground">{l.hint}</p> : null}
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

/** 修好一个原因的那一步，去它该去的页面。 */
function fixPath(network: number, fix: LaneFix): string {
  switch (fix.kind) {
    case 'enable_relay':
    case 'tcp_address':
      return nodePath(network, fix.node)
    case 'update':
      return `${nodePath(network, fix.node)}?tab=update`
    case 'access':
      return `${networkPath(network, 'access')}?check=1&from=${fix.from}&to=${fix.to}`
  }
}

/**
 * 两个节点各自报的、和对方之间的独占 TCP，并排放着：一眼看的那几个数之外的其余部分。
 * 握手失败分不到对端名下，是两个节点各自全节点的数，所以要取节点详情。
 */
function LaneDetails({ network, pair, names }: { network: Network; pair: PairView; names: readonly [string, string] }) {
  const a = useQuery(q.node(network.id, pair.a))
  const b = useQuery(q.node(network.id, pair.b))
  const sides: { id: NodeId; name: string; report: PairLaneSide | undefined; detail: typeof a }[] = [
    { id: pair.a, name: names[0], report: pair.lanes?.find((side) => side.node === pair.a), detail: a },
    { id: pair.b, name: names[1], report: pair.lanes?.find((side) => side.node === pair.b), detail: b },
  ]
  const dash = <span className="text-muted-foreground">{l.notReported}</span>
  const rows: { label: ReactNode; value: (side: (typeof sides)[number]) => ReactNode }[] = [
    {
      label: <TipHeader label={l.row.control} tip={l.row.controlTip} />,
      value: ({ report }) => (report === undefined ? dash : report.lanes.control ? l.row.controlUp : l.row.controlDown),
    },
    { label: l.row.dialling, value: ({ report }) => report?.lanes.dialling ?? dash },
    {
      label: <TipHeader label={l.row.claims} tip={l.row.claimsTip} />,
      value: ({ report }) => report?.lanes.claims ?? dash,
    },
    {
      label: l.row.waited,
      value: ({ report }) =>
        report === undefined
          ? dash
          : report.lanes.claims_waited === 0
            ? 0
            : l.row.waitedValue(report.lanes.claims_waited, t.common.ms(report.lanes.claim_wait_ms)),
    },
    { label: l.row.claimsFailed, value: ({ report }) => report?.lanes.claims_failed ?? dash },
    {
      label: <TipHeader label={l.row.refused} tip={l.row.refusedTip} />,
      value: ({ report }) => report?.lanes.refused ?? dash,
    },
    {
      label: <TipHeader label={l.row.sealed} tip={l.row.sealedTip} />,
      value: ({ report }) => (report === undefined ? dash : formatBytes(report.lanes.sealed_bytes)),
    },
    {
      label: <TipHeader label={l.row.passthrough} tip={l.row.passthroughTip} />,
      value: ({ report }) => (report === undefined ? dash : formatBytes(report.lanes.passthrough_bytes)),
    },
    {
      label: <TipHeader label={l.row.spliced} tip={l.row.splicedTip} />,
      value: ({ report }) => (report === undefined ? dash : formatBytes(report.lanes.spliced_bytes)),
    },
    {
      label: <TipHeader label={l.row.handshake} tip={l.row.handshakeTip} />,
      value: ({ detail }) => {
        const failures = detail.data?.lanes?.handshake_failures
        if (failures === undefined) return dash
        const parts = handshakeParts(failures)
        return (
          <>
            {handshakeTotal(failures)}
            {parts.length > 0 ? <span className="ml-2 text-muted-foreground">{parts.join(' · ')}</span> : null}
          </>
        )
      },
    },
  ]
  return (
    <div
      className="overflow-x-auto rounded-lg bg-muted/60 px-3 py-1"
      data-testid={`lane-details-${names[0]}-${names[1]}`}
    >
      <table className="w-full border-collapse text-[13px]" aria-label={l.detailsFor(names[0], names[1])}>
        <thead>
          <tr className="border-b border-border-strong/60 text-xs text-muted-foreground">
            <th scope="col" className="px-3 py-2 text-left font-medium">
              <span className="sr-only">{l.details}</span>
            </th>
            {sides.map((side) => (
              <th key={side.id} scope="col" className="px-3 py-2 text-right font-medium">
                {side.name}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {rows.map((row, index) => (
            <tr key={index} className="border-b border-border-strong/60 last:border-b-0">
              <th scope="row" className="px-3 py-1.5 text-left font-normal text-muted-foreground">
                {row.label}
              </th>
              {sides.map((side) => (
                <td key={side.id} className="px-3 py-1.5 text-right whitespace-nowrap tabular">
                  {row.value(side)}
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  )
}

/**
 * 节点对设置：连接常驻和独占 TCP（lane）。设置的控件改了立即生效；独占 TCP 不是"不用"时，
 * 写着它起不起作用、谁拨号，不起作用时下面一行是原因和修好它的那一步；两个节点报的计数一眼看几个，其余的点开看。
 * 从节点详情过来时地址里带着 `a`、`b`：那一对亮出来，详情展开。
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
  const [open, setOpen] = useState<string | null>(focusKey)
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
  const setLane = useAction(network.id, {
    mutationFn: ({ pair, lane }: { pair: PairView; lane: LaneMode }) =>
      api.setPair(network.id, pair.a, pair.b, { lane }),
    success: (_data, { pair, lane }) => l.changed(...names(pair), laneModeLabel(lane)),
  })
  const remove = useAction(network.id, {
    mutationFn: (pair: PairView) => api.deletePair(network.id, pair.a, pair.b),
    success: (_data, pair) => p.removed(...names(pair)),
  })

  const list = pairs.data ?? []
  const empty = pairs.isSuccess && list.length === 0
  const tooFew = nodes.isSuccess && all.length < 2
  // 状态那一列只在有哪一对开着独占 TCP 时画；计数的几列只在有数可看（或者马上就会有）时画。不然它们整列都是空的。
  const effectShown = list.some((pair) => laneMode(pair) !== 'off')
  const lanesShown = list.some((pair) => lanesReported(pair) || pair.lane_effect?.effective === true)
  const columns = 4 + (effectShown ? 1 : 0) + (lanesShown ? 4 : 0)
  const addButton = (
    <Button variant="primary" onClick={() => setAdding(true)} disabled={tooFew}>
      <Plus aria-hidden />
      {p.add}
    </Button>
  )

  return (
    <Page>
      <PageHeader title={p.title} actions={empty ? undefined : addButton} />
      <div>
        {pairs.isError ? (
          <ErrorState title={p.loadFailed} error={pairs.error} onRetry={() => void pairs.refetch()} />
        ) : empty ? (
          <EmptyState
            icon={Link2}
            title={p.emptyTitle}
            description={tooFew ? p.emptyNeedTwo : undefined}
            action={addButton}
          />
        ) : (
          <>
            <Table>
              <thead>
                <tr>
                  <Th>{p.colPair}</Th>
                  <Th>{p.colKeepAlive}</Th>
                  <Th>
                    <TipHeader label={l.title} tip={l.tip} />
                  </Th>
                  {effectShown ? <Th>{l.colState}</Th> : null}
                  {lanesShown ? (
                    <>
                      <Th numeric>{l.colInUse}</Th>
                      <Th numeric>{l.colIdle}</Th>
                      <Th numeric>
                        <TipHeader label={l.colFallbacks} tip={t.common.sentences(l.fallbacksTip, l.countsTip)} />
                      </Th>
                      <Th numeric>
                        <TipHeader label={l.colDialFailures} tip={l.countsTip} />
                      </Th>
                    </>
                  ) : null}
                  <Th className="w-12">
                    <span className="sr-only">{t.common.moreActions}</span>
                  </Th>
                </tr>
              </thead>
              <tbody>
                {pairs.isPending ? <SkeletonRows columns={columns} rows={2} /> : null}
                {list.map((pair) => {
                  const [a, b] = names(pair)
                  const key = pairKey(pair.a, pair.b)
                  const mode = laneMode(pair)
                  const effect = laneEffectView(pair, all)
                  const glance = pairGlance(pair.lanes ?? [])
                  // 两边都没报时没有数可看，也就没有详情。
                  const showLanes = glance !== null
                  const expanded = open === key && showLanes
                  const focused = focusKey === key
                  const reasons = effect.kind === 'blocked' ? effect.reasons : []
                  const followed = reasons.length > 0 || expanded
                  return (
                    <Fragment key={key}>
                      <Tr
                        ref={focused ? scrollTo : undefined}
                        className={cn(followed && 'border-b-0', focused && FOCUSED_ROW)}
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
                          <NativeSelect
                            aria-label={l.select(a, b)}
                            className="h-8 w-36 text-[13px]"
                            value={mode}
                            disabled={setLane.isPending}
                            onChange={(event) => setLane.mutate({ pair, lane: event.target.value as LaneMode })}
                          >
                            {LANE_MODES.map((value) => (
                              <option key={value} value={value}>
                                {laneModeLabel(value)}
                              </option>
                            ))}
                          </NativeSelect>
                        </Td>
                        {effectShown ? (
                          <Td className="whitespace-nowrap" data-testid={`lane-effect-${a}-${b}`}>
                            {effect.kind === 'off' ? null : (
                              <>
                                <StatusText tone={effect.kind === 'effective' ? 'ok' : 'warn'}>
                                  {effect.label}
                                </StatusText>
                                {effect.kind === 'effective' && effect.dialers !== null ? (
                                  <div className="mt-0.5 text-xs text-muted-foreground">{effect.dialers}</div>
                                ) : null}
                              </>
                            )}
                          </Td>
                        ) : null}
                        {lanesShown ? (
                          <>
                            <Td numeric data-testid={`lane-in-use-${a}-${b}`}>
                              {glance?.inUse}
                            </Td>
                            <Td numeric>{glance?.idle}</Td>
                            <Td numeric>{glance?.fallbacks}</Td>
                            <Td numeric>{glance?.dialFailures}</Td>
                          </>
                        ) : null}
                        <Td className="pl-0 text-right whitespace-nowrap">
                          {showLanes ? (
                            <Button
                              variant="ghost"
                              size="sm"
                              className="text-muted-foreground"
                              aria-expanded={expanded}
                              aria-label={expanded ? l.detailsHide : l.detailsFor(a, b)}
                              onClick={() => setOpen(expanded ? null : key)}
                            >
                              {expanded ? l.detailsHide : l.details}
                              <ChevronDown
                                className={cn('transition-transform', expanded && 'rotate-180')}
                                aria-hidden
                              />
                            </Button>
                          ) : null}
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
                      {reasons.length > 0 ? (
                        <tr className={cn(!expanded && 'border-b last:border-b-0', focused && FOCUSED_ROW)}>
                          <td
                            colSpan={columns}
                            className="px-3 pt-0 pb-3 text-[13px] leading-snug"
                            data-testid={`lane-blocked-${a}-${b}`}
                          >
                            <ul className="space-y-0.5">
                              {reasons.map((reason) => (
                                <li key={reason.text} className="flex flex-wrap gap-x-3">
                                  <span className="text-muted-foreground">{reason.text}</span>
                                  {reason.fix !== null && reason.fixLabel !== null ? (
                                    <Link
                                      to={fixPath(network.id, reason.fix)}
                                      className="rounded-sm text-primary hover:underline"
                                    >
                                      {reason.fixLabel}
                                    </Link>
                                  ) : null}
                                </li>
                              ))}
                            </ul>
                          </td>
                        </tr>
                      ) : null}
                      {expanded ? (
                        <tr className={cn('border-b last:border-b-0', focused && FOCUSED_ROW)}>
                          <td colSpan={columns} className="px-3 pt-0 pb-4">
                            <LaneDetails network={network} pair={pair} names={[a, b]} />
                          </td>
                        </tr>
                      ) : null}
                    </Fragment>
                  )
                })}
              </tbody>
            </Table>
            <TableNote>
              <span className="block">{p.keepAliveHint}</span>
              {list.some((pair) => laneMode(pair) !== 'off') ? <span className="block">{l.routeNote}</span> : null}
            </TableNote>
          </>
        )}
      </div>
      {adding ? <AddPairDialog network={network} existing={list} open onClose={() => setAdding(false)} /> : null}
    </Page>
  )
}
