import { useQuery } from '@tanstack/react-query'
import {
  ChevronRight,
  CircleArrowUp,
  EllipsisVertical,
  History,
  Pencil,
  Replace,
  Server,
  ServerOff,
  Trash2,
} from 'lucide-react'
import { useState } from 'react'
import { Link, useNavigate, useParams, useSearchParams } from 'react-router'

import { ApiError } from '@/api/client'
import { q } from '@/api/queries'
import { CopyText } from '@/components/copy'
import { ForceUpdateDialog } from '@/components/force-update'
import { Page } from '@/components/page'
import { EmptyState, ErrorState, SkeletonBlock } from '@/components/states'
import { IssueIcon, StatusDot, StatusText } from '@/components/status'
import { OsIcon } from '@/components/os-icon'
import { ReasonButton } from '@/components/update'
import { Button } from '@/components/ui/button'
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
import { attachedCount } from '@/lib/nodes'
import { forceAbility, isUpdating } from '@/lib/nodeUpdate'
import { LIVE_INTERVAL, WAITING_INTERVAL } from '@/lib/polling'
import { useDocumentTitle } from '@/lib/useDocumentTitle'
import { networkPath, useNetwork } from '@/pages/networks/context'
import { logPathFor } from '@/pages/settings/logFilter'

import { RenameNodeDialog } from './actions'
import { useDeleteNode } from './hooks'
import { AccessTab } from './detail/AccessTab'
import { ExitTab, ForwardTab, RelayTab } from './detail/Capabilities'
import { EditIpDialog, SucceedDialog } from './detail/dialogs'
import { LanesTab } from './detail/LanesTab'
import { ListenTab } from './detail/ListenTab'
import { OverviewTab } from './detail/OverviewTab'
import { RelatedTab } from './detail/RelatedTab'
import { RoutesTab } from './detail/RoutesTab'
import { UpdateTab } from './detail/UpdateTab'
import { RoleBadges } from './NodesPage'

const d = t.nodes.detail

/** 名字下面那一行里的分隔：一道细竖线。 */
function MetaDivider() {
  return <span aria-hidden className="h-3 w-px bg-border-strong" />
}

const TABS = [
  'overview',
  'relay',
  'exit',
  'forward',
  'routes',
  'lanes',
  'access',
  'listen',
  'related',
  'update',
] as const
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

  // 有一行能力正在等节点上报声明、或者它正在更新时，取得勤一些：用户这时正看着它变。
  const [waiting, setWaiting] = useState(false)
  const detail = useQuery({
    ...q.node(network.id, nodeId),
    refetchInterval: (query) =>
      waiting || (query.state.data !== undefined && isUpdating(query.state.data.update))
        ? WAITING_INTERVAL
        : LIVE_INTERVAL,
    enabled: Number.isInteger(nodeId),
  })
  const nodes = useQuery(q.nodes(network.id))
  const deleteNode = useDeleteNode(network)
  const [dialog, setDialog] = useState<'rename' | 'ip' | 'succeed' | 'update' | null>(null)

  const node = detail.data
  useDocumentTitle(node?.name ?? t.nodes.title)

  const listPath = networkPath(network.id, 'nodes')

  if (
    !Number.isInteger(nodeId) ||
    (detail.isError && detail.error instanceof ApiError && detail.error.is('not_found'))
  ) {
    return (
      <Page>
        <EmptyState
          icon={ServerOff}
          title={d.notFoundTitle}
          action={
            <Button asChild variant="primary">
              <Link to={listPath}>{d.backToList}</Link>
            </Button>
          }
        />
      </Page>
    )
  }
  if (detail.isError && node === undefined) {
    return (
      <Page>
        <ErrorState title={d.loadFailed} error={detail.error} onRetry={() => void detail.refetch()} />
      </Page>
    )
  }
  if (node === undefined) {
    return (
      <Page>
        <SkeletonBlock lines={8} />
      </Page>
    )
  }

  const issue = nodeIssues(node, nodes.data ?? [])[0]
  const ability = forceAbility(node)
  // 「更新」标签上的小圆点：有新版本，或者上次更新失败了。
  const updateMarked = node.update.available !== null || issue?.kind === 'update_failed'
  // 独占 TCP 默认不用：只有和哪个对端之间打开了，才有这个标签页。
  const showLanes = (node.lanes?.peers.length ?? 0) > 0 || tab === 'lanes'

  return (
    <Page className="@container">
      <nav aria-label={t.nav.breadcrumb} className="mb-5 flex items-center gap-1.5 text-base text-muted-foreground">
        <Link to={listPath} className="rounded-sm hover:text-foreground hover:underline">
          {t.nodes.title}
        </Link>
        <ChevronRight className="size-4 text-faint" aria-hidden />
        <span className="font-medium text-foreground" aria-current="page">
          {node.name}
        </span>
      </nav>

      <header className="mb-8 flex items-center justify-between gap-6">
        <div className="flex min-w-0 items-center gap-4">
          {/* 左边一块是它的系统，和下面两行一样高，名字和状态都从它右边对齐开始。 */}
          <span
            aria-hidden
            className="flex size-[52px] shrink-0 items-center justify-center rounded-xl border bg-muted/60 text-foreground/80"
          >
            <OsIcon platform={node.platform} className="size-6" fallback={<Server className="size-6" />} />
          </span>
          <div className="min-w-0">
            <div className="flex min-w-0 items-center gap-1.5">
              <h1 className="truncate text-[22px] leading-[30px] font-bold tracking-tight">{node.name}</h1>
              <Tooltip content={t.nodes.rename}>
                <Button
                  variant="ghost"
                  size="icon-sm"
                  className="size-7 shrink-0 text-faint hover:text-foreground [&_svg]:size-3.5"
                  onClick={() => setDialog('rename')}
                  aria-label={t.nodes.rename}
                >
                  <Pencil aria-hidden />
                </Button>
              </Tooltip>
              <span className="ml-1 flex shrink-0 items-center gap-1.5">
                <RoleBadges node={node} attached={attachedCount(nodes.data ?? [], node.id)} size="md" />
                {issue !== undefined ? <IssueIcon message={issue.message} tone={issue.tone} /> : null}
              </span>
            </div>
            {/* 名字下面一行：状态、虚拟 IP、域名，同一种字、同一个颜色，细竖线隔开。点值就复制（悬停有说明），不另画复制图标，间距才匀。 */}
            <p className="mt-1 flex flex-wrap items-center gap-x-3 gap-y-1 text-[13px] leading-5 text-muted-foreground">
              <StatusText tone={node.online ? 'ok' : 'idle'} className="gap-1.5 text-muted-foreground">
                {node.online ? t.common.online : t.common.offline}
              </StatusText>
              <MetaDivider />
              <CopyText value={node.virtual_ip} mono={false} className="tabular [&>svg]:hidden" />
              <MetaDivider />
              <CopyText value={node.fqdn} mono={false} className="[&>svg]:hidden" />
            </p>
          </div>
        </div>
        <div className="flex shrink-0 items-center gap-2">
          {/* 有新版本时才出现；不能点时置灰，悬停写原因和做法。 */}
          {node.update.available !== null ? (
            <ReasonButton reason={ability.ok ? null : ability.reason} onClick={() => setDialog('update')}>
              <CircleArrowUp aria-hidden />
              {t.updates.force.button}
            </ReasonButton>
          ) : null}
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button variant="ghost" aria-label={d.more}>
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
              <DropdownMenuItem onSelect={() => void navigate(logPathFor({ kind: 'node', id: node.id }, network.id))}>
                <History aria-hidden />
                {d.nodeLog}
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
        </div>
      </header>

      <Tabs
        value={tab}
        onValueChange={(next) => {
          setSearch(next === 'overview' ? {} : { tab: next }, { replace: true })
        }}
      >
        <TabsList>
          <TabsTrigger value="overview">{d.tabOverview}</TabsTrigger>
          <TabsTrigger value="relay">{d.tabRelay}</TabsTrigger>
          <TabsTrigger value="exit">{d.tabExit}</TabsTrigger>
          <TabsTrigger value="forward">{d.tabForward}</TabsTrigger>
          <TabsTrigger value="routes">{d.tabRoutes}</TabsTrigger>
          {showLanes ? <TabsTrigger value="lanes">{d.tabLanes}</TabsTrigger> : null}
          <TabsTrigger value="access">{d.tabAccess}</TabsTrigger>
          <TabsTrigger value="listen">{d.tabListen}</TabsTrigger>
          <TabsTrigger value="related">{d.tabRelated}</TabsTrigger>
          <TabsTrigger value="update">
            {t.updates.detail.tab}
            {updateMarked ? <StatusDot tone={issue?.kind === 'update_failed' ? 'danger' : 'warn'} /> : null}
          </TabsTrigger>
        </TabsList>
        <TabsContent value="overview">
          <OverviewTab network={network} node={node} onEditIp={() => setDialog('ip')} />
        </TabsContent>
        {/* 表单不铺满整个宽度：开关和"修改"离名字太远就对不上了。 */}
        <TabsContent value="relay">
          <RelayTab network={network} node={node} onWaiting={setWaiting} />
        </TabsContent>
        <TabsContent value="exit" className="max-w-5xl">
          <ExitTab network={network} node={node} onWaiting={setWaiting} />
        </TabsContent>
        <TabsContent value="forward" className="max-w-5xl">
          <ForwardTab network={network} node={node} onWaiting={setWaiting} />
        </TabsContent>
        <TabsContent value="routes">
          <RoutesTab network={network} node={node} />
        </TabsContent>
        <TabsContent value="lanes">
          <LanesTab network={network} node={node} />
        </TabsContent>
        <TabsContent value="access">
          <AccessTab network={network} node={node} />
        </TabsContent>
        <TabsContent value="listen">
          <ListenTab network={network} node={node} />
        </TabsContent>
        <TabsContent value="related">
          <RelatedTab network={network} node={node} />
        </TabsContent>
        <TabsContent value="update">
          <UpdateTab node={node} />
        </TabsContent>
      </Tabs>

      {dialog === 'rename' ? (
        <RenameNodeDialog network={network} node={node} open onClose={() => setDialog(null)} />
      ) : null}
      {dialog === 'ip' ? <EditIpDialog network={network} node={node} open onClose={() => setDialog(null)} /> : null}
      {dialog === 'succeed' ? (
        <SucceedDialog network={network} node={node} open onClose={() => setDialog(null)} />
      ) : null}
      {dialog === 'update' ? <ForceUpdateDialog node={node} onClose={() => setDialog(null)} /> : null}
    </Page>
  )
}
