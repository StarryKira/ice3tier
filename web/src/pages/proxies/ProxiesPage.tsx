import { useQuery } from '@tanstack/react-query'
import { ChevronRight, Copy, DoorOpen, Globe, Pencil, Plus, Server, Smartphone, Trash2 } from 'lucide-react'
import { type ReactNode, useState } from 'react'

import { api } from '@/api/endpoints'
import { q, useAction } from '@/api/queries'
import { type Node, type Proxy } from '@/api/types'
import { useConfirm } from '@/components/confirm'
import { copyText } from '@/lib/clipboard'
import { NodeLink } from '@/components/node'
import { Page, PageHeader } from '@/components/page'
import { RowActions } from '@/components/row-actions'
import { ErrorState, SkeletonRows } from '@/components/states'
import { StatusText } from '@/components/status'
import { RowsTotal, Table, TableFooter, Td, Th, Tr } from '@/components/table'
import { Button } from '@/components/ui/button'
import { DropdownMenuItem, DropdownMenuSeparator } from '@/components/ui/dropdown-menu'
import { Switch } from '@/components/ui/switch'
import { toast } from '@/components/ui/toast'
import { t } from '@/i18n'
import { parseSocketAddr } from '@/lib/cidr'
import { formatBytes } from '@/lib/format'
import { nodeName } from '@/lib/nodes'
import { proxyStatus, proxyUrl } from '@/lib/proxy'
import { useDocumentTitle } from '@/lib/useDocumentTitle'
import { cn } from '@/lib/utils'
import { useNetwork } from '@/pages/networks/context'

import { ProxyDialog } from './ProxyDialog'

const s = t.proxies

/** 设备里要填的地址：监听所有地址的写成"局域网 IP"让人自己换。 */
function addressOf(proxy: Proxy): string {
  const bind = parseSocketAddr(proxy.status?.local ?? proxy.bind)
  if (bind === null) return proxy.bind
  return proxyUrl({
    protocol: proxy.protocol,
    host: bind.host === '0.0.0.0' ? `<${s.lanHost}>` : bind.host,
    port: bind.port,
    username: proxy.username,
    password: proxy.password,
  })
}

/** 没有代理时的样子：虚线框铺满标题下面剩下的地方，正中一行图说明代理是怎么走的，下面一个新建的按钮。 */
function Empty({ onCreate }: { onCreate: () => void }) {
  const stop = (icon: ReactNode, label: string) => (
    <span className="inline-flex items-center gap-2 rounded-lg border bg-card px-3 py-2 text-[13px] whitespace-nowrap [&>svg]:size-4 [&>svg]:text-muted-foreground">
      {icon}
      {label}
    </span>
  )
  const arrow = <ChevronRight aria-hidden className="size-4 shrink-0 text-faint" />
  return (
    <div className="flex flex-1 flex-col items-center justify-center gap-5 rounded-xl border border-dashed px-6 py-12 text-center">
      <div aria-hidden className="flex flex-wrap items-center justify-center gap-2">
        {stop(<Smartphone />, s.flow.device)}
        {arrow}
        {stop(<Server />, s.flow.entry)}
        {arrow}
        {stop(<DoorOpen />, s.flow.exit)}
        {arrow}
        {stop(<Globe />, s.flow.internet)}
      </div>
      <div>
        <p className="text-[15px] font-semibold">{s.emptyTitle}</p>
        <p className="mt-1 text-[13px] text-muted-foreground">{s.emptyBody}</p>
      </div>
      <Button variant="primary" onClick={onCreate}>
        <Plus aria-hidden />
        {s.create}
      </Button>
    </div>
  )
}

/**
 * 代理：在一台节点（入口）上开 SOCKS5 / HTTP 代理端口，流量经组网从另一台节点（出口）出去，用户名和密码认证。
 * 一行一个代理：入口（节点、协议、地址）、出口、认证、状态、用量；开关直接在行里拨。
 */
export function ProxiesPage() {
  useDocumentTitle(s.title)
  const network = useNetwork()
  const confirm = useConfirm()
  const proxies = useQuery(q.proxies(network.id))
  const nodes = useQuery(q.nodes(network.id))
  const all: readonly Node[] = nodes.data ?? []
  const [editing, setEditing] = useState<{ proxy: Proxy | null } | null>(null)
  const list = proxies.data ?? []

  const toggle = useAction(network.id, {
    mutationFn: ({ proxy, enabled }: { proxy: Proxy; enabled: boolean }) =>
      api.updateProxy(network.id, proxy.id, { enabled }),
    success: (saved) => (saved.enabled ? s.enabledToast(saved.name) : s.disabledToast(saved.name)),
  })
  const remove = useAction(network.id, {
    mutationFn: (proxy: Proxy) => api.deleteProxy(network.id, proxy.id),
    success: (_data, proxy) => s.deleted(proxy.name),
  })
  const onDelete = async (proxy: Proxy) => {
    const confirmed = await confirm({
      tone: 'danger',
      title: s.deleteTitle(proxy.name),
      body: s.deleteBody,
      confirmLabel: s.deleteConfirm,
    })
    if (confirmed) remove.mutate(proxy)
  }

  return (
    <Page fill>
      <PageHeader
        title={s.title}
        description={s.subtitle}
        actions={
          list.length > 0 ? (
            <Button variant="primary" onClick={() => setEditing({ proxy: null })}>
              <Plus aria-hidden />
              {s.create}
            </Button>
          ) : undefined
        }
      />
      {proxies.isError ? (
        <ErrorState title={s.loadFailed} error={proxies.error} onRetry={() => void proxies.refetch()} />
      ) : proxies.isSuccess && list.length === 0 ? (
        <Empty onCreate={() => setEditing({ proxy: null })} />
      ) : (
        <>
          <Table>
            <thead>
              <tr>
                <Th>{s.colName}</Th>
                <Th>{s.colEntry}</Th>
                <Th>{s.colExit}</Th>
                <Th>{s.colAuth}</Th>
                <Th>{s.colStatus}</Th>
                <Th>{s.colUsage}</Th>
                <Th className="w-0">
                  <span className="sr-only">{t.common.moreActions}</span>
                </Th>
              </tr>
            </thead>
            <tbody>
              {proxies.isPending ? <SkeletonRows columns={7} rows={3} /> : null}
              {list.map((proxy) => {
                const status = proxyStatus(proxy)
                const entry = all.find((node) => node.id === proxy.node)
                const exit = proxy.exit === null ? undefined : all.find((node) => node.id === proxy.exit)
                return (
                  <Tr
                    key={proxy.id}
                    dimmed={!proxy.enabled}
                    className="h-16 hover:bg-muted/50"
                    data-testid={`proxy-row-${proxy.name}`}
                  >
                    <Td>
                      <span className="inline-flex items-center gap-3">
                        <Switch
                          aria-label={s.switchLabel(proxy.name)}
                          checked={proxy.enabled}
                          disabled={toggle.isPending && toggle.variables.proxy.id === proxy.id}
                          onCheckedChange={(enabled) => toggle.mutate({ proxy, enabled })}
                        />
                        <span className="font-medium">{proxy.name}</span>
                      </span>
                    </Td>
                    <Td>
                      <NodeLink
                        network={network.id}
                        node={{ id: proxy.node, name: nodeName(all, proxy.node) }}
                        online={entry?.online}
                      />
                      <div className="mt-0.5 flex items-center gap-1.5 text-xs text-muted-foreground">
                        <span className="rounded-sm bg-muted px-1.5 leading-5">{s.protocol[proxy.protocol]}</span>
                        <span className="font-mono">{proxy.status?.local ?? proxy.bind}</span>
                      </div>
                    </Td>
                    <Td>
                      {proxy.exit === null ? (
                        <span className="text-muted-foreground">—</span>
                      ) : (
                        <NodeLink
                          network={network.id}
                          node={{ id: proxy.exit, name: nodeName(all, proxy.exit) }}
                          online={exit?.online}
                        />
                      )}
                    </Td>
                    <Td className="text-[13px]">
                      {proxy.username === '' ? (
                        <span className="text-muted-foreground">{s.noAuth}</span>
                      ) : (
                        <span className="font-mono">{proxy.username}</span>
                      )}
                    </Td>
                    <Td>
                      <StatusText tone={status.tone} className="text-[13px] whitespace-nowrap">
                        {s.status[status.code]}
                      </StatusText>
                      {proxy.status?.detail != null && status.tone === 'danger' ? (
                        <div
                          className="mt-0.5 max-w-60 truncate text-xs text-muted-foreground"
                          title={proxy.status.detail}
                        >
                          {proxy.status.detail}
                        </div>
                      ) : null}
                    </Td>
                    <Td className={cn('text-xs whitespace-nowrap text-muted-foreground tabular')}>
                      {s.connections(proxy.connections)}
                      <div>
                        ↑ {formatBytes(proxy.bytes_up)} ↓ {formatBytes(proxy.bytes_down)}
                      </div>
                    </Td>
                    <Td className="pl-0 text-right">
                      <RowActions label={t.common.actionsFor(proxy.name)}>
                        <DropdownMenuItem onSelect={() => setEditing({ proxy })}>
                          <Pencil aria-hidden />
                          {s.menuEdit}
                        </DropdownMenuItem>
                        <DropdownMenuItem
                          onSelect={() =>
                            void copyText(addressOf(proxy)).then((ok) => (ok ? toast.success(s.copied) : undefined))
                          }
                        >
                          <Copy aria-hidden />
                          {s.menuCopy}
                        </DropdownMenuItem>
                        <DropdownMenuSeparator />
                        <DropdownMenuItem danger onSelect={() => void onDelete(proxy)}>
                          <Trash2 aria-hidden />
                          {s.menuDelete}
                        </DropdownMenuItem>
                      </RowActions>
                    </Td>
                  </Tr>
                )
              })}
            </tbody>
          </Table>
          {list.length > 0 ? (
            <TableFooter>
              <RowsTotal shown={list.length} />
            </TableFooter>
          ) : null}
        </>
      )}
      {editing !== null ? (
        <ProxyDialog network={network} proxy={editing.proxy} onClose={() => setEditing(null)} />
      ) : null}
    </Page>
  )
}
