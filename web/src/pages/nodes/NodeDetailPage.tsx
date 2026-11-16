import { useQuery } from '@tanstack/react-query'
import { ChevronRight, EllipsisVertical, Pencil, Replace, ServerOff, Trash2 } from 'lucide-react'
import { useState } from 'react'
import { Link, useNavigate, useParams, useSearchParams } from 'react-router'

import { ApiError } from '@/api/client'
import { q } from '@/api/queries'
import { CopyText } from '@/components/copy'
import { Page } from '@/components/page'
import { EmptyState, ErrorState, SkeletonBlock } from '@/components/states'
import { IssueIcon, OnlineDot } from '@/components/status'
import { Button } from '@/components/ui/button'
import { Card } from '@/components/ui/card'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { Tooltip } from '@/components/ui/tooltip'
import { t } from '@/i18n'
import { nodeIssues } from '@/lib/nodeIssues'
import { WAITING_INTERVAL } from '@/lib/polling'
import { useDocumentTitle } from '@/lib/useDocumentTitle'
import { networkPath, useNetwork } from '@/pages/networks/context'

import { RenameNodeDialog } from './actions'
import { useDeleteNode } from './hooks'
import { AccessTab } from './detail/AccessTab'
import { EditIpDialog, SucceedDialog } from './detail/dialogs'
import { OverviewTab } from './detail/OverviewTab'
import { RelatedTab } from './detail/RelatedTab'
import { RoutesTab } from './detail/RoutesTab'
import { RoleBadges } from './NodesPage'

const d = t.nodes.detail
const TABS = ['overview', 'routes', 'access', 'related'] as const
type Tab = (typeof TABS)[number]

/**
 * 节点详情：独立的页面，有自己的地址。字段多、关联多的对象不用抽屉。
 * 侧栏里没有它自己的入口，打开时"节点"那一项保持高亮。
 */
export function NodeDetailPage() {
  const network = useNetwork()
  const navigate = useNavigate()
  const params = useParams()
  const nodeId = Number(params.node)
  const [search, setSearch] = useSearchParams()
  const requested = search.get('tab') as Tab | null
  const tab: Tab = requested !== null && TABS.includes(requested) ? requested : 'overview'

  // 有一行能力正在等节点上报声明时，取得勤一些。
  const [waiting, setWaiting] = useState(false)
  const detail = useQuery({
    ...q.node(network.id, nodeId),
    ...(waiting ? { refetchInterval: WAITING_INTERVAL } : {}),
    enabled: Number.isInteger(nodeId),
  })
  const nodes = useQuery(q.nodes(network.id))
  const deleteNode = useDeleteNode(network)
  const [dialog, setDialog] = useState<'rename' | 'ip' | 'succeed' | null>(null)

  const node = detail.data
  useDocumentTitle(node?.name ?? t.nodes.title)

  const listPath = networkPath(network.id, 'nodes')

  if (
    !Number.isInteger(nodeId) ||
    (detail.isError && detail.error instanceof ApiError && detail.error.is('not_found'))
  ) {
    return (
      <Page>
        <Card>
          <EmptyState
            icon={ServerOff}
            title={d.notFoundTitle}
            description={d.notFoundBody}
            action={
              <Button asChild variant="primary">
                <Link to={listPath}>{d.backToList}</Link>
              </Button>
            }
          />
        </Card>
      </Page>
    )
  }
  if (detail.isError && node === undefined) {
    return (
      <Page>
        <Card>
          <ErrorState title={d.loadFailed} error={detail.error} onRetry={() => void detail.refetch()} />
        </Card>
      </Page>
    )
  }
  if (node === undefined) {
    return (
      <Page>
        <Card>
          <SkeletonBlock lines={8} />
        </Card>
      </Page>
    )
  }

  const issue = nodeIssues(node, nodes.data ?? [])[0]

  return (
    <Page className="@container">
      <nav aria-label={t.nav.breadcrumb} className="mb-3 flex items-center gap-1.5 text-[13px] text-muted-foreground">
        <Link to={listPath} className="rounded-sm hover:text-foreground hover:underline">
          {t.nodes.title}
        </Link>
        <ChevronRight className="size-3.5" aria-hidden />
        <span className="text-foreground" aria-current="page">
          {node.name}
        </span>
      </nav>

      <header className="mb-5 flex items-start justify-between gap-6">
        <div className="min-w-0">
          <h1 className="flex flex-wrap items-center gap-2.5 text-xl font-semibold tracking-tight">
            <OnlineDot online={node.online} className="[&>span:first-child]:size-2.5" />
            <span className="truncate">{node.name}</span>
            <Tooltip content={t.nodes.rename}>
              <Button variant="ghost" size="icon-sm" onClick={() => setDialog('rename')} aria-label={t.nodes.rename}>
                <Pencil aria-hidden />
              </Button>
            </Tooltip>
            <RoleBadges node={node} />
            {issue !== undefined ? <IssueIcon message={issue.message} /> : null}
          </h1>
          <p className="mt-1.5 flex flex-wrap items-center gap-x-4 gap-y-1 text-sm text-muted-foreground">
            <span className={node.online ? 'font-medium text-ok' : undefined}>
              {node.online ? t.common.online : t.common.offline}
            </span>
            <CopyText value={node.virtual_ip} className="text-foreground" />
            <CopyText value={node.fqdn} />
          </p>
        </div>
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button aria-label={d.more}>
              <EllipsisVertical aria-hidden />
              {d.more}
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent>
            <DropdownMenuItem onSelect={() => setDialog('rename')}>
              <Pencil aria-hidden />
              {t.nodes.menuRename}
            </DropdownMenuItem>
            <DropdownMenuItem onSelect={() => setDialog('succeed')}>
              <Replace aria-hidden />
              {d.succeed}
            </DropdownMenuItem>
            <DropdownMenuSeparator />
            <DropdownMenuItem
              danger
              onSelect={() =>
                void deleteNode(node).then((deleted) => {
                  if (deleted) void navigate(listPath, { replace: true })
                })
              }
            >
              <Trash2 aria-hidden />
              {t.nodes.menuDelete}
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      </header>

      <Tabs
        value={tab}
        onValueChange={(next) => {
          setSearch(next === 'overview' ? {} : { tab: next }, { replace: true })
        }}
      >
        <TabsList>
          <TabsTrigger value="overview">{d.tabOverview}</TabsTrigger>
          <TabsTrigger value="routes">{d.tabRoutes}</TabsTrigger>
          <TabsTrigger value="access">{d.tabAccess}</TabsTrigger>
          <TabsTrigger value="related">{d.tabRelated}</TabsTrigger>
        </TabsList>
        <TabsContent value="overview">
          <OverviewTab network={network} node={node} onEditIp={() => setDialog('ip')} onWaiting={setWaiting} />
        </TabsContent>
        <TabsContent value="routes">
          <RoutesTab network={network} node={node} />
        </TabsContent>
        <TabsContent value="access">
          <AccessTab network={network} node={node} />
        </TabsContent>
        <TabsContent value="related">
          <RelatedTab network={network} node={node} />
        </TabsContent>
      </Tabs>

      {dialog === 'rename' ? (
        <RenameNodeDialog network={network} node={node} open onClose={() => setDialog(null)} />
      ) : null}
      {dialog === 'ip' ? <EditIpDialog network={network} node={node} open onClose={() => setDialog(null)} /> : null}
      {dialog === 'succeed' ? (
        <SucceedDialog network={network} node={node} open onClose={() => setDialog(null)} />
      ) : null}
    </Page>
  )
}
