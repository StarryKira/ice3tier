import { useQueries, useQuery } from '@tanstack/react-query'
import { Copy, EllipsisVertical, Pencil, Plus, RadioTower, Trash2 } from 'lucide-react'
import { Fragment, type ReactNode, useState } from 'react'
import { Link } from 'react-router'

import { api } from '@/api/endpoints'
import { q, useAction } from '@/api/queries'
import { type Listener, type NodeId, type NodeListener } from '@/api/types'
import { useConfirm } from '@/components/confirm'
import { CopyText } from '@/components/copy'
import { ListenStateCell } from '@/components/listen'
import { NodeLink } from '@/components/node'
import { Notice } from '@/components/notice'
import { Page, PageHeader } from '@/components/page'
import { EmptyState, ErrorState, SkeletonRows } from '@/components/states'
import { Table, TableNote, Td, Th, Tr } from '@/components/table'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { toast } from '@/components/ui/toast'
import { Tooltip } from '@/components/ui/tooltip'
import { t } from '@/i18n'
import { copyText } from '@/lib/clipboard'
import { listenerUsage } from '@/lib/commands'
import { listenView, type ListenView } from '@/lib/listenState'
import { nodeName } from '@/lib/nodes'
import { formatAge } from '@/lib/time'
import { FOCUSED_ROW, useFocusRow } from '@/lib/useFocusRow'
import { useDocumentTitle } from '@/lib/useDocumentTitle'
import { cn } from '@/lib/utils'
import { networkPath, useNetwork } from '@/pages/networks/context'

import { describeNoUser, describeWho, PROTOCOL_LABEL } from './describe'
import { ListenerDialog } from './ListenerDialog'

const l = t.listeners

/** 表格的列数：名字、地址、谁能用、不写用户名时、状态、操作。 */
const COLUMNS = 6

type Editor = { listener: Listener | null } | null

interface Group {
  id: NodeId
  name: string
  online: boolean | undefined
  listeners: Listener[]
  builtin: NodeListener[]
  reportedSecsAgo: number | null
}

/** 状态有问题时，原因单独占一行；节点的原话在它下面，字小一号。 */
function DetailRow({ view, focused }: { view: ListenView; focused: boolean }) {
  if (!view.needsAttention || view.detail === null) return null
  return (
    <tr className={cn('border-b last:border-b-0', focused && FOCUSED_ROW)}>
      <td colSpan={COLUMNS} className="px-3 pt-0 pb-3 pl-7 text-[13px] leading-snug text-muted-foreground">
        {view.detail}
        {view.reported !== null ? <span className="mt-0.5 block text-xs break-words">{view.reported}</span> : null}
      </td>
    </tr>
  )
}

/**
 * 监听器，按节点分组。每个节点下面是 controller 下发给它的监听器，和它按本机配置文件自己开的回环入口（"自带"，只能看）。
 * 状态、连接数和流量来自节点的上报。
 */
export function ListenersPage() {
  useDocumentTitle(l.title)
  const network = useNetwork()
  const confirm = useConfirm()
  const listeners = useQuery(q.listeners(network.id))
  const nodes = useQuery(q.nodes(network.id))
  const [editor, setEditor] = useState<Editor>(null)
  const focus = useFocusRow(listeners.data)

  const all = nodes.data ?? []
  // 自带的回环入口只有节点自己知道：在线的节点各问一次。离线的节点报不了，问了也是空的。
  const online = all.filter((node) => node.online)
  const reports = useQueries({ queries: online.map((node) => q.nodeListeners(network.id, node.id)) })

  const remove = useAction(network.id, {
    mutationFn: (listener: Listener) => api.deleteListener(network.id, listener.id),
    success: (_data, listener) => l.deleted(listener.name),
  })

  const list = listeners.data ?? []
  const noNodes = nodes.isSuccess && all.length === 0

  const onDelete = async (listener: Listener) => {
    const connections = listener.status?.connections ?? 0
    const confirmed = await confirm({
      title: l.deleteTitle(listener.name, nodeName(all, listener.node)),
      body: (
        <>
          {connections > 0 ? <p>{l.deleteActive(connections)}</p> : null}
          <p>{l.deleteBody}</p>
        </>
      ),
      confirmLabel: l.deleteConfirm,
    })
    if (confirmed) remove.mutate(listener)
  }

  const copyUsage = (listener: Pick<Listener, 'bind' | 'protocol' | 'fixed_exit' | 'require_credential' | 'node'>) => {
    const exit = all.find((node) => node.exit !== null)
    const usage = listenerUsage(listener, {
      exit: listener.fixed_exit !== null ? undefined : (exit?.virtual_ip ?? t.credentials.usageExitPlaceholder),
      credential: listener.require_credential ? t.credentials.credentialPlaceholder : undefined,
      hostPlaceholder: nodeName(all, listener.node),
    })
    void copyText(usage).then((ok) => (ok ? toast.success(l.usageCopied) : toast.error(t.common.copyFailed)))
  }

  // 按节点分组：节点按名字排，组内按编号，自带的排在最后。
  const groupIds = new Set<NodeId>(list.map((listener) => listener.node))
  const builtinByNode = new Map<NodeId, { items: NodeListener[]; age: number | null }>()
  for (const report of reports) {
    const data = report.data
    if (data === undefined) continue
    const items = data.listeners.filter((item) => item.builtin)
    builtinByNode.set(data.node.id, { items, age: data.reported_secs_ago })
    if (items.length > 0) groupIds.add(data.node.id)
  }
  const groups: Group[] = [...groupIds]
    .map((id) => ({
      id,
      name: nodeName(all, id),
      online: all.find((node) => node.id === id)?.online,
      listeners: list.filter((listener) => listener.node === id),
      builtin: builtinByNode.get(id)?.items ?? [],
      reportedSecsAgo: builtinByNode.get(id)?.age ?? null,
    }))
    .sort((a, b) => a.name.localeCompare(b.name, undefined, { numeric: true }))

  const configuredNone = listeners.isSuccess && list.length === 0
  const empty = configuredNone && groups.length === 0
  const createButton = (
    <Button variant="primary" onClick={() => setEditor({ listener: null })}>
      <Plus aria-hidden />
      {l.create}
    </Button>
  )

  const actions = (listener: Listener): ReactNode => (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button variant="ghost" size="icon-sm" aria-label={t.common.actionsFor(listener.name)}>
          <EllipsisVertical aria-hidden />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent>
        <DropdownMenuItem onSelect={() => setEditor({ listener })}>
          <Pencil aria-hidden />
          {l.menuEdit}
        </DropdownMenuItem>
        <DropdownMenuItem onSelect={() => copyUsage(listener)}>
          <Copy aria-hidden />
          {l.menuCopy}
        </DropdownMenuItem>
        <DropdownMenuSeparator />
        <DropdownMenuItem danger onSelect={() => void onDelete(listener)}>
          <Trash2 aria-hidden />
          {l.menuDelete}
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  )

  return (
    <Page>
      <PageHeader title={l.title} actions={empty || noNodes ? undefined : createButton} />
      {focus.missing ? (
        <Notice className="mb-4" tone="warn">
          {l.focusMissing(focus.focus ?? 0)}
        </Notice>
      ) : null}
      <div>
        {listeners.isError ? (
          <ErrorState title={l.loadFailed} error={listeners.error} onRetry={() => void listeners.refetch()} />
        ) : empty && noNodes ? (
          <EmptyState
            icon={RadioTower}
            title={l.emptyTitle}
            description={l.emptyNoNodes}
            action={
              <Button asChild variant="primary">
                <Link to={networkPath(network.id, 'nodes')}>{t.exits.goNodes}</Link>
              </Button>
            }
          />
        ) : empty ? (
          <EmptyState icon={RadioTower} title={l.emptyTitle} action={createButton} />
        ) : (
          <>
            <Table>
              <thead>
                <tr>
                  <Th>{l.colName}</Th>
                  <Th>{l.colBind}</Th>
                  <Th>{l.colWho}</Th>
                  <Th>{l.colNoUser}</Th>
                  <Th>{l.colState}</Th>
                  <Th className="w-12">
                    <span className="sr-only">{t.common.moreActions}</span>
                  </Th>
                </tr>
              </thead>
              <tbody>
                {listeners.isPending ? <SkeletonRows columns={COLUMNS} rows={3} /> : null}
                {groups.map((group) => (
                  <Fragment key={group.id}>
                    <tr className="border-b">
                      <th
                        scope="rowgroup"
                        colSpan={COLUMNS}
                        className="px-3 pt-6 pb-2 text-left text-[13px] font-medium"
                      >
                        <span className="flex flex-wrap items-center gap-x-2 gap-y-1">
                          <NodeLink
                            network={network.id}
                            node={{ id: group.id, name: group.name }}
                            online={group.online}
                          />
                          <span className="font-normal text-muted-foreground">
                            {l.count(group.listeners.length + group.builtin.length)}
                          </span>
                          {group.online === false ? <Badge>{l.nodeOffline}</Badge> : null}
                          {group.reportedSecsAgo !== null ? (
                            <span className="ml-auto text-xs font-normal text-muted-foreground">
                              {t.common.reportedAgo(formatAge(group.reportedSecsAgo))}
                            </span>
                          ) : null}
                        </span>
                      </th>
                    </tr>
                    {group.listeners.map((listener) => {
                      const view = listenView(listener.status, {
                        node: group.name,
                        bind: listener.bind,
                        online: group.online ?? false,
                      })
                      const focused = focus.focus === listener.id
                      return (
                        <Fragment key={listener.id}>
                          <Tr
                            ref={focus.rowRef(listener.id)}
                            className={cn(view.needsAttention && 'border-b-0', focused && FOCUSED_ROW)}
                            data-testid={`listener-row-${listener.name}`}
                          >
                            <Td className="pl-7">
                              <span className="block font-medium">{listener.name}</span>
                              <span className="mt-0.5 flex gap-1.5">
                                <Badge>{PROTOCOL_LABEL[listener.protocol]}</Badge>
                              </span>
                            </Td>
                            <Td>
                              <CopyText value={listener.bind} />
                            </Td>
                            <Td className="text-[13px]">{describeWho(listener)}</Td>
                            <Td className="text-[13px]">{describeNoUser(listener, all)}</Td>
                            <Td data-testid={`listener-state-${listener.name}`}>
                              <ListenStateCell view={view} />
                            </Td>
                            <Td className="pl-0 text-right">{actions(listener)}</Td>
                          </Tr>
                          <DetailRow view={view} focused={focused} />
                        </Fragment>
                      )
                    })}
                    {group.builtin.map((item) => {
                      const view = listenView(item, { node: group.name, bind: item.bind, online: true })
                      return (
                        <Fragment key={`builtin-${item.name}`}>
                          <Tr
                            className={cn(view.needsAttention && 'border-b-0')}
                            data-testid={`builtin-row-${group.name}-${item.name}`}
                          >
                            <Td className="pl-7">
                              <span className="block font-medium">{item.name}</span>
                              <span className="mt-0.5 flex gap-1.5">
                                <Badge>{PROTOCOL_LABEL[item.protocol]}</Badge>
                                <Tooltip content={t.listen.builtinTip}>
                                  <Badge tone="outline" tabIndex={0}>
                                    {t.listen.builtin}
                                  </Badge>
                                </Tooltip>
                              </span>
                            </Td>
                            <Td>
                              <CopyText value={item.bind} />
                            </Td>
                            <Td className="text-[13px]">{l.whoLocal}</Td>
                            <Td className="text-[13px]">{l.actionLocal}</Td>
                            <Td>
                              <ListenStateCell view={view} />
                            </Td>
                            <Td className="pl-0 text-right">
                              <Button
                                variant="ghost"
                                size="icon-sm"
                                aria-label={l.copyUsageOf(item.name)}
                                onClick={() =>
                                  copyUsage({
                                    bind: item.bind,
                                    protocol: item.protocol,
                                    fixed_exit: null,
                                    require_credential: false,
                                    node: group.id,
                                  })
                                }
                              >
                                <Copy aria-hidden />
                              </Button>
                            </Td>
                          </Tr>
                          <DetailRow view={view} focused={false} />
                        </Fragment>
                      )
                    })}
                  </Fragment>
                ))}
              </tbody>
            </Table>
            <TableNote>{t.common.updatedEvery(10)}</TableNote>
          </>
        )}
      </div>

      {editor !== null ? (
        <ListenerDialog
          key={editor.listener?.id ?? 'new'}
          network={network}
          listener={editor.listener}
          open
          onClose={() => setEditor(null)}
        />
      ) : null}
    </Page>
  )
}
