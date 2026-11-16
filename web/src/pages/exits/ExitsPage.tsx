import { useQuery } from '@tanstack/react-query'
import { Copy, DoorOpen, EllipsisVertical, Plus, Power, SlidersHorizontal } from 'lucide-react'
import { useId, useState } from 'react'
import { Link, useNavigate } from 'react-router'

import { api } from '@/api/endpoints'
import { q, useAction } from '@/api/queries'
import { type Network, type Node, type NodeId } from '@/api/types'
import { useConfirm } from '@/components/confirm'
import { CodeBlock, CopyText } from '@/components/copy'
import { DeclareHowTo } from '@/components/declare'
import { FormDialog } from '@/components/form-dialog'
import { NodeLink } from '@/components/node'
import { nodePath } from '@/lib/nodes'
import { Page, PageHeader } from '@/components/page'
import { EmptyState, ErrorState, SkeletonRows } from '@/components/states'
import { OnlineDot } from '@/components/status'
import { Table, Td, Th, Tr } from '@/components/table'
import { Button } from '@/components/ui/button'
import { Card } from '@/components/ui/card'
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
import { t } from '@/i18n'
import { copyText } from '@/lib/clipboard'
import { declareCommand, exitUsage } from '@/lib/commands'
import { WAITING_INTERVAL } from '@/lib/polling'
import { useDocumentTitle } from '@/lib/useDocumentTitle'
import { networkPath, useNetwork } from '@/pages/networks/context'
import { exitSummary, useSaveExit } from '@/pages/nodes/detail/exitActions'
import { ExitFormFields } from '@/pages/nodes/detail/exitForm'
import { exitFormFrom, type ExitFormState } from '@/pages/nodes/detail/exitFormState'

const e = t.exits
const d = t.nodes.detail

/** "谁能用"：由访问规则算出，能访问这个节点的就能把它当出口。 */
function Users({ network, node }: { network: Network; node: Node }) {
  const access = useQuery(q.nodeAccess(network.id, node.id))
  if (access.isPending) return <Skeleton className="h-3.5 w-16" />
  if (access.isError) return <span className="text-muted-foreground">—</span>
  const users = access.data.reachable_by
  if (users.length === 0) return <span className="text-warn">{e.usersNone}</span>
  return (
    <Tooltip content={e.usersList(users.map((edge) => edge.node.name).join('、'))}>
      <Link
        to={`${networkPath(network.id, 'access')}?view=node&node=${node.id}`}
        className="rounded-sm text-primary underline-offset-4 tabular hover:underline"
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
        <div className="rounded-md border bg-muted/40 p-4">
          <ExitFormFields state={form} onChange={setForm} showProblems={submitted} />
        </div>
      ) : null}

      {undeclared.length > 0 ? (
        <div>
          <p className="mb-2 text-[13px] font-medium text-muted-foreground">{e.notDeclared}</p>
          <ul className="divide-y rounded-md border">
            {undeclared.map((node) => (
              <li key={node.id} className="px-3 py-2">
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

/**
 * 出口页是节点的一个筛选视图加上出口专有的列，不是另一套对象：
 * 同一种对象不因为用途不同就拆出两处编辑入口。修改限制去节点详情页。
 */
export function ExitsPage() {
  useDocumentTitle(e.title)
  const network = useNetwork()
  const navigate = useNavigate()
  const confirm = useConfirm()
  const nodes = useQuery(q.nodes(network.id))
  const [enabling, setEnabling] = useState(false)

  const disable = useAction(network.id, {
    mutationFn: (node: Node) => api.disableExit(network.id, node.id),
    success: (_data, node) => d.exitDisabledToast(node.name),
  })

  const onDisable = async (node: Node) => {
    const confirmed = await confirm({
      title: d.exitOffTitle(node.name),
      body: d.exitOffBody,
      confirmLabel: d.exitOffConfirm,
    })
    if (confirmed) disable.mutate(node)
  }

  const all = nodes.data ?? []
  const exits = all.filter((node) => node.exit !== null)
  const declared = all.filter((node) => node.exit === null && node.declared.exit)
  const empty = nodes.isSuccess && exits.length === 0

  const enableButton = (
    <Button variant="primary" onClick={() => setEnabling(true)}>
      <Plus aria-hidden />
      {e.enable}
    </Button>
  )

  return (
    <Page>
      <PageHeader title={e.title} description={e.description} actions={empty ? undefined : enableButton} />
      <Card>
        {nodes.isError ? (
          <ErrorState title={e.loadFailed} error={nodes.error} onRetry={() => void nodes.refetch()} />
        ) : empty && all.length === 0 ? (
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
              {nodes.isPending ? <SkeletonRows columns={4} rows={2} /> : null}
              {exits.map((node) => (
                <Tr key={node.id} data-testid={`exit-row-${node.name}`}>
                  <Td>
                    <NodeLink network={network.id} node={node} online={node.online} />
                    <div className="mt-0.5">
                      <CopyText value={node.virtual_ip} className="text-xs text-muted-foreground" />
                    </div>
                  </Td>
                  <Td className="text-[13px]">{node.exit === null ? null : exitSummary(node.exit)}</Td>
                  <Td className="text-[13px]">
                    <Users network={network} node={node} />
                  </Td>
                  <Td className="pl-0 text-right">
                    <DropdownMenu>
                      <DropdownMenuTrigger asChild>
                        <Button variant="ghost" size="icon-sm" aria-label={t.common.actionsFor(node.name)}>
                          <EllipsisVertical aria-hidden />
                        </Button>
                      </DropdownMenuTrigger>
                      <DropdownMenuContent>
                        <DropdownMenuItem
                          onSelect={() =>
                            void copyText(exitUsage(node.virtual_ip)).then((ok) =>
                              ok ? toast.success(e.usageCopied) : toast.error(t.common.copyFailed),
                            )
                          }
                        >
                          <Copy aria-hidden />
                          {e.menuCopy}
                        </DropdownMenuItem>
                        <DropdownMenuItem onSelect={() => void navigate(nodePath(network.id, node.id))}>
                          <SlidersHorizontal aria-hidden />
                          {e.menuEdit}
                        </DropdownMenuItem>
                        <DropdownMenuSeparator />
                        <DropdownMenuItem danger onSelect={() => void onDisable(node)}>
                          <Power aria-hidden />
                          {e.menuDisable}
                        </DropdownMenuItem>
                      </DropdownMenuContent>
                    </DropdownMenu>
                  </Td>
                </Tr>
              ))}
            </tbody>
          </Table>
        )}
      </Card>

      {enabling ? <EnableExitDialog network={network} open onClose={() => setEnabling(false)} /> : null}
    </Page>
  )
}
