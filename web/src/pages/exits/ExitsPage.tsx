import { useQuery } from '@tanstack/react-query'
import { ChevronDown, Copy, DoorOpen, EllipsisVertical, Plus, Power, SlidersHorizontal } from 'lucide-react'
import { Fragment, type ReactNode, useId, useState } from 'react'
import { Link, useNavigate } from 'react-router'

import { api } from '@/api/endpoints'
import { q, useAction } from '@/api/queries'
import { type ExitView, type Network, type NodeId, type NodeRef } from '@/api/types'
import { useConfirm } from '@/components/confirm'
import { CodeBlock, CopyText } from '@/components/copy'
import { DeclareHowTo } from '@/components/declare'
import { FormDialog } from '@/components/form-dialog'
import { NodeLink } from '@/components/node'
import { nodePath } from '@/lib/nodes'
import { Page, PageHeader } from '@/components/page'
import { EmptyState, ErrorState, SkeletonRows } from '@/components/states'
import { IssueIcon, OnlineDot, StatusText } from '@/components/status'
import { Table, Td, Th, Tr } from '@/components/table'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { RadioCards } from '@/components/ui/radio-cards'
import { Skeleton } from '@/components/ui/skeleton'
import { toast } from '@/components/ui/toast'
import { Tooltip } from '@/components/ui/tooltip'
import { TrafficText, UsageBar } from '@/components/usage'
import { t } from '@/i18n'
import { copyText } from '@/lib/clipboard'
import { declareCommand, exitUsage } from '@/lib/commands'
import { formatBytes } from '@/lib/format'
import { WAITING_INTERVAL } from '@/lib/polling'
import { formatAge, formatFullTime } from '@/lib/time'
import { FOCUSED_ROW, useFocusRow } from '@/lib/useFocusRow'
import { useDocumentTitle } from '@/lib/useDocumentTitle'
import { quotaView } from '@/lib/usage'
import { cn } from '@/lib/utils'
import { networkPath, useNetwork } from '@/pages/networks/context'
import { exitSummaryParts, useSaveExit } from '@/pages/nodes/detail/exitActions'
import { ExitFormFields } from '@/pages/nodes/detail/exitForm'
import { exitFormFrom, type ExitFormState } from '@/pages/nodes/detail/exitFormState'

import { crossCheck, exitHealthView } from './exitView'

const e = t.exits
const d = t.nodes.detail

/** "谁能用"：由访问规则算出，能访问这个节点的就能把它当出口。 */
function Users({ network, node }: { network: Network; node: NodeRef }) {
  const access = useQuery(q.nodeAccess(network.id, node.id))
  if (access.isPending) return <Skeleton className="h-3.5 w-16" />
  if (access.isError) return <span className="text-muted-foreground">—</span>
  const users = access.data.reachable_by
  if (users.length === 0) return <span className="text-warn">{e.usersNone}</span>
  return (
    <Tooltip content={e.usersList(users.map((edge) => edge.node.name).join('、'))}>
      <Link
        to={`${networkPath(network.id, 'access')}?view=node&node=${node.id}`}
        className="rounded-sm underline decoration-border-strong underline-offset-4 tabular hover:decoration-foreground"
      >
        {e.users(users.length)}
      </Link>
    </Tooltip>
  )
}

interface EnableExitDialogProps {
  network: Network
  open: boolean
  onClose: () => void
}

/**
 * "启用出口"：选一个节点，就地显示端口、带宽这几项默认值，确认即启用。
 * 没有声明的节点也列出来，只是不能选，每个后面有"怎么声明"：不列出来，用户会以为节点丢了。
 */
function EnableExitDialog({ network, open, onClose }: EnableExitDialogProps) {
  const [howTo, setHowTo] = useState<NodeId | null>(null)
  // "怎么声明"开着时页面在等节点上报，取得勤一些；声明一到，它自己挪到上面那一组。
  const nodes = useQuery({ ...q.nodes(network.id), refetchInterval: howTo !== null ? WAITING_INTERVAL : undefined })
  const [picked, setPicked] = useState<NodeId | null>(null)
  const [form, setForm] = useState<ExitFormState>(() => exitFormFrom(null))
  const [submitted, setSubmitted] = useState(false)
  const declaredLabel = useId()

  const candidates = (nodes.data ?? []).filter((node) => node.exit === null)
  const declared = candidates.filter((node) => node.declared.exit)
  const undeclared = candidates.filter((node) => !node.declared.exit)
  const chosen = declared.find((node) => node.id === picked)
  const save = useSaveExit(network, chosen ?? { id: 0, name: '', exit: null })

  return (
    <FormDialog
      open={open}
      onClose={onClose}
      title={e.dialogTitle}
      description={e.dialogLead}
      dirty={picked !== null}
      submitLabel={e.submit}
      pending={save.pending}
      canSubmit={chosen !== undefined}
      error={save.error}
      onSubmit={() => {
        setSubmitted(true)
        if (chosen === undefined) return
        void save.submit(form).then((ok) => {
          if (ok) onClose()
        })
      }}
    >
      {declared.length > 0 ? (
        <div>
          <p id={declaredLabel} className="mb-2 text-[13px] font-medium">
            {t.capability.exit.full}
          </p>
          <RadioCards
            aria-labelledby={declaredLabel}
            value={picked === null ? '' : String(picked)}
            onChange={(value) => setPicked(Number(value))}
            options={declared.map((node) => ({
              value: String(node.id),
              title: (
                <span className="inline-flex items-center gap-1.5">
                  <OnlineDot online={node.online} />
                  {node.name}
                </span>
              ),
              description: <span className="font-mono text-xs">{node.virtual_ip}</span>,
            }))}
            columns={declared.length > 1 ? 2 : 1}
          />
        </div>
      ) : null}

      {chosen !== undefined ? (
        <div className="rounded-lg bg-muted/60 p-4">
          <ExitFormFields state={form} onChange={setForm} showProblems={submitted} />
        </div>
      ) : null}

      {undeclared.length > 0 ? (
        <div>
          <p className="mb-2 text-[13px] font-medium text-muted-foreground">{e.notDeclared}</p>
          <ul className="divide-y border-y">
            {undeclared.map((node) => (
              <li key={node.id} className="py-2">
                <div className="flex items-center justify-between gap-3">
                  <span className="inline-flex min-w-0 items-center gap-1.5 text-sm text-muted-foreground">
                    <OnlineDot online={node.online} />
                    <span className="truncate">{node.name}</span>
                    <span className="font-mono text-xs">{node.virtual_ip}</span>
                  </span>
                  <Button
                    variant="link"
                    size="sm"
                    aria-expanded={howTo === node.id}
                    onClick={() => setHowTo(howTo === node.id ? null : node.id)}
                  >
                    {howTo === node.id ? t.declare.hide : t.declare.howTo}
                  </Button>
                </div>
                {howTo === node.id ? (
                  <DeclareHowTo capability="exit" nodeName={node.name} waiting className="mt-2" />
                ) : null}
              </li>
            ))}
          </ul>
        </div>
      ) : null}
    </FormDialog>
  )
}

/** 健康：结论、为什么，和"只算出口自己的失败"这条规则。 */
function Health({ view }: { view: ExitView }) {
  const health = exitHealthView(view)
  return (
    <div data-testid={`exit-health-${view.node.name}`}>
      <Tooltip content={`${health.detail}${view.online ? ` ${e.health.rule}` : ''}`}>
        <span tabIndex={0} className="rounded-sm">
          <StatusText tone={health.tone} className="text-[13px] whitespace-nowrap">
            {health.label}
          </StatusText>
        </span>
      </Tooltip>
      {view.health === 'degraded' && view.online ? (
        <span className="mt-0.5 block text-xs text-muted-foreground">
          {e.health.degradedShort(view.recent_requests, view.recent_failures)}
        </span>
      ) : null}
    </div>
  )
}

/** 用量：配了配额时是进度条和重置的日子，没配时是这次运行以来搬运的量。出口报的和发起端看到的差得多时带个记号。 */
function Usage({ view }: { view: ExitView }) {
  const usage = view.usage
  if (usage === null) {
    return (
      <Tooltip content={e.usageNoneHint}>
        <span tabIndex={0} className="rounded-sm text-[13px] text-muted-foreground">
          {e.usageNone}
        </span>
      </Tooltip>
    )
  }
  const check = crossCheck(view)
  const mismatch =
    check?.mismatch === true ? (
      <IssueIcon message={e.mismatchTip(formatBytes(check.exitBytes), formatBytes(check.observedBytes))} />
    ) : null
  if (usage.quota !== null) {
    const quota = quotaView(usage.quota)
    return (
      <div className="flex items-start gap-1.5" data-testid={`exit-usage-${view.node.name}`}>
        <UsageBar view={quota} label={e.quotaLabel(view.node.name)} className="w-40">
          <Tooltip content={e.quotaResetsFull(formatFullTime(quota.resetsAt))}>
            <span tabIndex={0} className="mt-0.5 block w-fit rounded-sm text-xs text-muted-foreground">
              {quota.exhausted ? e.quotaOver : quota.resets}
            </span>
          </Tooltip>
        </UsageBar>
        {mismatch}
      </div>
    )
  }
  return (
    <div className="flex items-start gap-1.5 text-[13px]" data-testid={`exit-usage-${view.node.name}`}>
      <div>
        <TrafficText up={usage.bytes_to_target} down={usage.bytes_from_target} tip={e.usageTip} />
        <span className="block text-xs text-muted-foreground">{e.usageTraffic}</span>
      </div>
      {mismatch}
    </div>
  )
}

/**
 * 对照：出口自己报的计数，和用它的各个节点自己看到的，并排放着。
 * 每一列是一个来源，行是同一种数；对不上的地方一眼看得出来。
 */
function CrossCheckTable({ view }: { view: ExitView }) {
  const usage = view.usage
  const dash = <span className="text-muted-foreground">{e.notApplicable}</span>
  const rows: { label: ReactNode; exit: ReactNode; seen: (index: number) => ReactNode }[] = [
    {
      label: e.rowTraffic,
      exit:
        usage === null
          ? dash
          : t.common.trafficUpDown(formatBytes(usage.bytes_to_target), formatBytes(usage.bytes_from_target)),
      seen: (index) => {
        const seen = view.observed[index]
        return seen === undefined
          ? dash
          : t.common.trafficUpDown(formatBytes(seen.bytes_up), formatBytes(seen.bytes_down))
      },
    },
    { label: e.rowRequests, exit: usage?.total ?? dash, seen: (index) => view.observed[index]?.flows ?? dash },
    { label: e.rowActive, exit: usage?.active ?? dash, seen: () => dash },
    {
      label: e.rowFailedTarget,
      exit: usage?.failed_by_target ?? dash,
      seen: (index) => view.observed[index]?.failed_by_target ?? dash,
    },
    {
      label: (
        <Tooltip content={e.rowRefusedHint}>
          <span tabIndex={0} className="cursor-help rounded-sm underline decoration-dotted underline-offset-4">
            {e.rowRefused}
          </span>
        </Tooltip>
      ),
      exit: usage?.refused ?? dash,
      seen: (index) => view.observed[index]?.refused ?? dash,
    },
    {
      label: e.rowFailedExit,
      exit: usage?.failed_by_exit ?? dash,
      seen: (index) => view.observed[index]?.failed_by_exit ?? dash,
    },
    { label: e.rowAborted, exit: usage?.aborted ?? dash, seen: () => dash },
    {
      label: (
        <Tooltip content={e.rowUnreachableHint}>
          <span tabIndex={0} className="cursor-help rounded-sm underline decoration-dotted underline-offset-4">
            {e.rowUnreachable}
          </span>
        </Tooltip>
      ),
      exit: dash,
      seen: (index) => view.observed[index]?.unreachable ?? dash,
    },
    {
      label: e.rowAge,
      exit: view.reported_secs_ago === null ? dash : formatAge(view.reported_secs_ago),
      seen: (index) => {
        const seen = view.observed[index]
        return seen === undefined ? dash : formatAge(seen.reported_secs_ago)
      },
    },
  ]
  return (
    <div className="space-y-2" data-testid={`exit-crosscheck-${view.node.name}`}>
      <p className="text-[13px] text-muted-foreground">{e.detailsLead}</p>
      {view.observed.length === 0 ? <p className="text-[13px] text-muted-foreground">{e.observedNone}</p> : null}
      <div className="overflow-x-auto rounded-lg bg-muted/60 px-3 py-1">
        <table className="w-full border-collapse text-[13px]" aria-label={e.detailsFor(view.node.name)}>
          <thead>
            <tr className="border-b border-border-strong/60 text-xs text-muted-foreground">
              <th scope="col" className="px-3 py-2 text-left font-medium">
                <span className="sr-only">{e.details}</span>
              </th>
              <th scope="col" className="px-3 py-2 text-right font-medium">
                {e.exitReported}
              </th>
              {view.observed.map((seen) => (
                <th key={seen.by.id} scope="col" className="px-3 py-2 text-right font-medium">
                  {e.reportedBy(seen.by.name)}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {rows.map((row, rowIndex) => (
              <tr key={rowIndex} className="border-b border-border-strong/60 last:border-b-0">
                <th scope="row" className="px-3 py-1.5 text-left font-normal text-muted-foreground">
                  {row.label}
                </th>
                <td className="px-3 py-1.5 text-right whitespace-nowrap tabular">{row.exit}</td>
                {view.observed.map((seen, index) => (
                  <td key={seen.by.id} className="px-3 py-1.5 text-right whitespace-nowrap tabular">
                    {row.seen(index)}
                  </td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  )
}

/**
 * 出口页是节点的一个筛选视图加上出口专有的列，不是另一套对象：
 * 同一种对象不因为用途不同就拆出两处编辑入口。修改限制去节点详情页。
 * 健康、用量和对照来自 `GET …/exits`：出口自己报的，和用它的节点看到的。
 */
export function ExitsPage() {
  useDocumentTitle(e.title)
  const network = useNetwork()
  const navigate = useNavigate()
  const confirm = useConfirm()
  const exits = useQuery(q.exits(network.id))
  const nodes = useQuery(q.nodes(network.id))
  const [enabling, setEnabling] = useState(false)
  const [open, setOpen] = useState<NodeId | null>(null)
  const focus = useFocusRow(exits.data?.map((view) => ({ id: view.node.id })))

  const disable = useAction(network.id, {
    mutationFn: (node: NodeRef) => api.disableExit(network.id, node.id),
    success: (_data, node) => d.exitDisabledToast(node.name),
  })

  const onDisable = async (view: ExitView) => {
    const active = view.usage?.active ?? 0
    const confirmed = await confirm({
      title: d.exitOffTitle(view.node.name),
      body: (
        <>
          {active > 0 ? <p>{d.exitOffActive(active)}</p> : null}
          <p>{d.exitOffBody}</p>
        </>
      ),
      confirmLabel: d.exitOffConfirm,
    })
    if (confirmed) disable.mutate(view.node)
  }

  const all = nodes.data ?? []
  const list = exits.data ?? []
  const declared = all.filter((node) => node.exit === null && node.declared.exit)
  // 没有出口时要看节点才知道该说哪一种空：节点还没取回来时先不下结论。
  const empty = exits.isSuccess && list.length === 0 && !nodes.isPending

  const enableButton = (
    <Button variant="primary" onClick={() => setEnabling(true)}>
      <Plus aria-hidden />
      {e.enable}
    </Button>
  )

  return (
    <Page>
      <PageHeader title={e.title} description={e.description} actions={empty ? undefined : enableButton} />
      <div>
        {exits.isError ? (
          <ErrorState title={e.loadFailed} error={exits.error} onRetry={() => void exits.refetch()} />
        ) : empty && nodes.isSuccess && all.length === 0 ? (
          <EmptyState
            icon={DoorOpen}
            title={e.emptyDeclaredTitle}
            description={e.emptyNoNodes}
            action={
              <Button asChild variant="primary">
                <Link to={networkPath(network.id, 'nodes')}>{e.goNodes}</Link>
              </Button>
            }
          />
        ) : empty && declared.length > 0 ? (
          <EmptyState
            icon={DoorOpen}
            title={e.emptyDeclaredTitle}
            description={e.emptyDeclaredBody(declared.length)}
            action={enableButton}
          />
        ) : empty ? (
          <EmptyState icon={DoorOpen} title={e.emptyNoneTitle} description={e.emptyNoneBody}>
            <CodeBlock
              copy={declareCommand('exit')}
              className="mt-4 w-full max-w-md text-left"
              testId="declare-command"
            >
              {declareCommand('exit')}
            </CodeBlock>
            <p className="mt-3 text-[13px] text-muted-foreground">{e.emptyNoneAfter}</p>
            <Button className="mt-4" onClick={() => setEnabling(true)}>
              {e.enable}
            </Button>
          </EmptyState>
        ) : (
          <Table>
            <thead>
              <tr>
                <Th>{e.colNode}</Th>
                <Th>{e.colLimits}</Th>
                <Th>{e.colHealth}</Th>
                <Th>{e.colUsage}</Th>
                <Th>
                  <Tooltip content={e.usersTip}>
                    <span
                      tabIndex={0}
                      className="cursor-help rounded-sm underline decoration-dotted underline-offset-4"
                    >
                      {e.colUsers}
                    </span>
                  </Tooltip>
                </Th>
                <Th className="w-12">
                  <span className="sr-only">{t.common.moreActions}</span>
                </Th>
              </tr>
            </thead>
            <tbody>
              {exits.isPending || list.length === 0 ? <SkeletonRows columns={6} rows={2} /> : null}
              {list.map((view) => {
                const expanded = open === view.node.id
                const focused = focus.focus === view.node.id
                return (
                  <Fragment key={view.node.id}>
                    <Tr
                      ref={focus.rowRef(view.node.id)}
                      className={cn(expanded && 'border-b-0', focused && FOCUSED_ROW)}
                      data-testid={`exit-row-${view.node.name}`}
                    >
                      <Td>
                        <NodeLink network={network.id} node={view.node} online={view.online} />
                        <div className="mt-0.5">
                          <CopyText value={view.node.virtual_ip} className="text-xs text-muted-foreground" />
                        </div>
                      </Td>
                      <Td className="max-w-56 text-[13px]">
                        {/* 分隔符放在不折行的那一段外面：折行只发生在两项之间，而不是把整串挤出格子。 */}
                        {exitSummaryParts(view.settings).map((part, index) => (
                          <Fragment key={part}>
                            {index > 0 ? ' · ' : null}
                            <span className="whitespace-nowrap">{part}</span>
                          </Fragment>
                        ))}
                      </Td>
                      <Td>
                        <Health view={view} />
                      </Td>
                      <Td>
                        <Usage view={view} />
                        <Button
                          variant="link"
                          size="sm"
                          className="mt-1 h-auto px-0 text-muted-foreground hover:text-foreground"
                          aria-expanded={expanded}
                          aria-label={expanded ? e.detailsHide : e.detailsFor(view.node.name)}
                          onClick={() => setOpen(expanded ? null : view.node.id)}
                        >
                          {expanded ? e.detailsHide : e.details}
                          <ChevronDown className={cn('transition-transform', expanded && 'rotate-180')} aria-hidden />
                        </Button>
                      </Td>
                      <Td className="text-[13px]">
                        <Users network={network} node={view.node} />
                      </Td>
                      <Td className="pl-0 text-right">
                        <DropdownMenu>
                          <DropdownMenuTrigger asChild>
                            <Button variant="ghost" size="icon-sm" aria-label={t.common.actionsFor(view.node.name)}>
                              <EllipsisVertical aria-hidden />
                            </Button>
                          </DropdownMenuTrigger>
                          <DropdownMenuContent>
                            <DropdownMenuItem
                              onSelect={() =>
                                void copyText(exitUsage(view.node.virtual_ip)).then((ok) =>
                                  ok ? toast.success(e.usageCopied) : toast.error(t.common.copyFailed),
                                )
                              }
                            >
                              <Copy aria-hidden />
                              {e.menuCopy}
                            </DropdownMenuItem>
                            <DropdownMenuItem onSelect={() => void navigate(nodePath(network.id, view.node.id))}>
                              <SlidersHorizontal aria-hidden />
                              {e.menuEdit}
                            </DropdownMenuItem>
                            <DropdownMenuSeparator />
                            <DropdownMenuItem danger onSelect={() => void onDisable(view)}>
                              <Power aria-hidden />
                              {e.menuDisable}
                            </DropdownMenuItem>
                          </DropdownMenuContent>
                        </DropdownMenu>
                      </Td>
                    </Tr>
                    {expanded ? (
                      <tr className={cn('border-b last:border-b-0', focused && FOCUSED_ROW)}>
                        <td colSpan={6} className="px-3 pt-0 pb-4">
                          <CrossCheckTable view={view} />
                        </td>
                      </tr>
                    ) : null}
                  </Fragment>
                )
              })}
            </tbody>
          </Table>
        )}
      </div>

      {enabling ? <EnableExitDialog network={network} open onClose={() => setEnabling(false)} /> : null}
    </Page>
  )
}
