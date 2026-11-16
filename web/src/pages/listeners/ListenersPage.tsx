import { useQuery } from '@tanstack/react-query'
import { Copy, EllipsisVertical, Pencil, Plus, RadioTower, Trash2 } from 'lucide-react'
import { Fragment, useState } from 'react'
import { Link } from 'react-router'

import { api } from '@/api/endpoints'
import { q, useAction } from '@/api/queries'
import { type Listener } from '@/api/types'
import { useConfirm } from '@/components/confirm'
import { CopyText } from '@/components/copy'
import { NodeLink } from '@/components/node'
import { nodeName } from '@/lib/nodes'
import { Notice } from '@/components/notice'
import { Page, PageHeader } from '@/components/page'
import { EmptyState, ErrorState, SkeletonRows } from '@/components/states'
import { Table, Td, Th, Tr } from '@/components/table'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card } from '@/components/ui/card'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { toast } from '@/components/ui/toast'
import { t } from '@/i18n'
import { copyText } from '@/lib/clipboard'
import { listenerUsage } from '@/lib/commands'
import { useDocumentTitle } from '@/lib/useDocumentTitle'
import { networkPath, useNetwork } from '@/pages/networks/context'

import { describeNoUser, describeWho, PROTOCOL_LABEL } from './describe'
import { ListenerDialog } from './ListenerDialog'

const l = t.listeners

type Editor = { listener: Listener | null } | null

export function ListenersPage() {
  useDocumentTitle(l.title)
  const network = useNetwork()
  const confirm = useConfirm()
  const listeners = useQuery(q.listeners(network.id))
  const nodes = useQuery(q.nodes(network.id))
  const [editor, setEditor] = useState<Editor>(null)

  const remove = useAction(network.id, {
    mutationFn: (listener: Listener) => api.deleteListener(network.id, listener.id),
    success: (_data, listener) => l.deleted(listener.name),
  })

  const all = nodes.data ?? []
  const list = listeners.data ?? []
  const empty = listeners.isSuccess && list.length === 0
  const noNodes = nodes.isSuccess && all.length === 0

  const onDelete = async (listener: Listener) => {
    const confirmed = await confirm({
      title: l.deleteTitle(listener.name, nodeName(all, listener.node)),
      body: l.deleteBody,
      confirmLabel: l.deleteConfirm,
    })
    if (confirmed) remove.mutate(listener)
  }

  const copyUsage = (listener: Listener) => {
    const exit = all.find((node) => node.exit !== null)
    const usage = listenerUsage(listener, {
      exit: listener.fixed_exit !== null ? undefined : (exit?.virtual_ip ?? t.credentials.usageExitPlaceholder),
      credential: listener.require_credential ? '<凭据>' : undefined,
      hostPlaceholder: nodeName(all, listener.node),
    })
    void copyText(usage).then((ok) => (ok ? toast.success(l.usageCopied) : toast.error(t.common.copyFailed)))
  }

  // 按节点分组：每个节点下面是它的监听器。节点按名字排，组内按编号。
  const groups = [...new Set(list.map((listener) => listener.node))]
    .map((id) => ({ id, name: nodeName(all, id), items: list.filter((listener) => listener.node === id) }))
    .sort((a, b) => a.name.localeCompare(b.name, undefined, { numeric: true }))

  return (
    <Page>
      <PageHeader
        title={l.title}
        description={l.description}
        actions={
          empty ? undefined : (
            <Button variant="primary" onClick={() => setEditor({ listener: null })}>
              <Plus aria-hidden />
              {l.create}
            </Button>
          )
        }
      />
      <Notice className="mb-4">{l.builtinNote}</Notice>
      <Card>
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
          <EmptyState
            icon={RadioTower}
            title={l.emptyTitle}
            description={l.emptyBody}
            action={
              <Button variant="primary" onClick={() => setEditor({ listener: null })}>
                <Plus aria-hidden />
                {l.create}
              </Button>
            }
          />
        ) : (
          <Table>
            <thead>
              <tr>
                <Th>{l.colName}</Th>
                <Th>{l.colType}</Th>
                <Th>{l.colBind}</Th>
                <Th>{l.colWho}</Th>
                <Th>{l.colNoUser}</Th>
                <Th className="w-12">
                  <span className="sr-only">{t.common.moreActions}</span>
                </Th>
              </tr>
            </thead>
            <tbody>
              {listeners.isPending ? <SkeletonRows columns={6} rows={3} /> : null}
              {groups.map((group) => {
                const node = all.find((candidate) => candidate.id === group.id)
                return (
                  <Fragment key={group.id}>
                    <tr className="border-b bg-muted/30">
                      <th scope="rowgroup" colSpan={6} className="px-4 py-2 text-left text-[13px] font-medium">
                        <span className="inline-flex items-center gap-2">
                          <NodeLink
                            network={network.id}
                            node={{ id: group.id, name: group.name }}
                            online={node?.online}
                          />
                          <span className="font-normal text-muted-foreground">{l.count(group.items.length)}</span>
                          {node !== undefined && !node.online ? <Badge>{l.nodeOffline}</Badge> : null}
                        </span>
                      </th>
                    </tr>
                    {group.items.map((listener) => (
                      <Tr key={listener.id} data-testid={`listener-row-${listener.name}`}>
                        <Td className="pl-8 font-medium">{listener.name}</Td>
                        <Td>
                          <Badge>{PROTOCOL_LABEL[listener.protocol]}</Badge>
                        </Td>
                        <Td>
                          <CopyText value={listener.bind} />
                        </Td>
                        <Td className="text-[13px]">{describeWho(listener)}</Td>
                        <Td className="text-[13px]">{describeNoUser(listener, all)}</Td>
                        <Td className="pl-0 text-right">
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
                        </Td>
                      </Tr>
                    ))}
                  </Fragment>
                )
              })}
            </tbody>
          </Table>
        )}
      </Card>

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
