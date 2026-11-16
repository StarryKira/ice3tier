import { useQuery } from '@tanstack/react-query'
import { ChevronRight, LogOut, PanelLeft, Settings, UserRound, WifiOff } from 'lucide-react'
import { useState } from 'react'
import { Link, Navigate, Outlet, useLocation, useNavigate, useParams } from 'react-router'

import { useReachable } from '@/api/connection'
import { q } from '@/api/queries'
import { session } from '@/auth/session'
import { ErrorState, SkeletonBlock } from '@/components/states'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { Tooltip } from '@/components/ui/tooltip'
import { t } from '@/i18n'
import { isBoolean, readStored, writeStored } from '@/lib/storage'
import { networkPath } from '@/pages/networks/context'
import { CreateNetworkDialog } from '@/pages/networks/CreateNetworkDialog'
import { pickCurrentNetwork } from '@/pages/networks/current'

import { currentPage, pageLabel } from './nav'
import { Sidebar } from './Sidebar'

const SIDEBAR_COLLAPSED_KEY = 'sidebarCollapsed'

/** 顶栏上的位置：网络名 / 页面名。详情页自己在标题上方再写一行更细的。 */
function Breadcrumb({ networkName, networkId }: { networkName: string; networkId: number }) {
  const location = useLocation()
  const page = currentPage(location.pathname)
  const label = page === null ? (location.pathname.startsWith('/settings') ? t.nav.settings : null) : pageLabel(page)
  return (
    <nav aria-label={t.nav.breadcrumb} className="flex min-w-0 items-center gap-1.5 text-sm">
      {page !== null ? (
        <>
          <Link to={networkPath(networkId)} className="truncate rounded-sm text-muted-foreground hover:text-foreground">
            {networkName}
          </Link>
          {label !== null ? <ChevronRight className="size-3.5 shrink-0 text-muted-foreground/70" aria-hidden /> : null}
        </>
      ) : null}
      {label !== null ? (
        <span className="truncate font-medium" aria-current="page">
          {label}
        </span>
      ) : null}
    </nav>
  )
}

/**
 * 登录之后的外壳：侧栏、顶栏、正文。
 *
 * 侧栏里的链接都属于"当前网络"：地址里有网络编号就是它；在全局的页面上（设置）用上次看的那个。
 */
export function AppLayout() {
  const params = useParams()
  const navigate = useNavigate()
  const networks = useQuery(q.networks())
  const reachable = useReachable()
  const [collapsed, setCollapsed] = useState(() => readStored(SIDEBAR_COLLAPSED_KEY, false, isBoolean))
  const [creating, setCreating] = useState(false)

  if (networks.isPending) {
    return (
      <div className="mx-auto max-w-3xl pt-24">
        <SkeletonBlock lines={6} />
      </div>
    )
  }
  if (networks.isError) {
    return (
      <div className="mx-auto max-w-3xl pt-24">
        <ErrorState
          title={t.errors.loadFailed(t.networks.title)}
          error={networks.error}
          onRetry={() => void networks.refetch()}
        />
      </div>
    )
  }
  const current = pickCurrentNetwork(networks.data, params.network === undefined ? undefined : Number(params.network))
  // 一个网络都没有：先去建第一个。
  if (current === undefined) return <Navigate to="/welcome" replace />

  const toggleSidebar = () => {
    setCollapsed((value) => {
      writeStored(SIDEBAR_COLLAPSED_KEY, !value)
      return !value
    })
  }

  const signOut = () => {
    session.signOut('manual')
    void navigate('/login', { replace: true })
  }

  return (
    <div className="flex min-h-screen">
      <a
        href="#main"
        className="sr-only focus:not-sr-only focus:fixed focus:top-2 focus:left-2 focus:z-[80] focus:rounded-md focus:bg-card focus:px-3 focus:py-2 focus:shadow-pop"
      >
        {t.nav.skipToContent}
      </a>
      <Sidebar
        networks={networks.data}
        current={current}
        collapsed={collapsed}
        onCreateNetwork={() => setCreating(true)}
      />
      <div className="flex min-w-0 flex-1 flex-col">
        <header className="sticky top-0 z-30 flex h-12 shrink-0 items-center gap-3 border-b bg-background/90 px-4 backdrop-blur">
          <Tooltip content={collapsed ? t.nav.expand : t.nav.collapse} side="bottom">
            <Button
              variant="ghost"
              size="icon"
              onClick={toggleSidebar}
              aria-label={collapsed ? t.nav.expand : t.nav.collapse}
              aria-pressed={collapsed}
            >
              <PanelLeft aria-hidden />
            </Button>
          </Tooltip>
          <Breadcrumb networkName={current.name} networkId={current.id} />
          <div className="flex-1" />
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button variant="ghost" size="sm" aria-label={t.nav.accountMenu}>
                <UserRound aria-hidden />
                {t.nav.account}
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent>
              <DropdownMenuLabel>{t.nav.account}</DropdownMenuLabel>
              <DropdownMenuItem onSelect={() => void navigate('/settings')}>
                <Settings aria-hidden />
                {t.nav.settings}
              </DropdownMenuItem>
              <DropdownMenuSeparator />
              <DropdownMenuItem onSelect={signOut}>
                <LogOut aria-hidden />
                {t.nav.signOut}
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        </header>
        {reachable ? null : (
          <div
            role="alert"
            className="flex items-center gap-2 border-b border-warn-line bg-warn-soft px-6 py-2 text-[13px] text-warn"
          >
            <WifiOff className="size-4 shrink-0" aria-hidden />
            {t.errors.connectionLost}
          </div>
        )}
        <main id="main" tabIndex={-1} className="min-w-0 flex-1 outline-none">
          <Outlet />
        </main>
      </div>
      <CreateNetworkDialog open={creating} onClose={() => setCreating(false)} />
    </div>
  )
}
