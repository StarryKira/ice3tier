import { useQuery } from '@tanstack/react-query'
import { WifiOff } from 'lucide-react'
import { useState } from 'react'
import { Navigate, Outlet, useNavigate, useParams } from 'react-router'

import { useReachable } from '@/api/connection'
import { q } from '@/api/queries'
import { session } from '@/auth/session'
import { ErrorState, SkeletonBlock } from '@/components/states'
import { t } from '@/i18n'
import { isBoolean, readStored, writeStored } from '@/lib/storage'
import { CreateNetworkDialog } from '@/pages/networks/CreateNetworkDialog'
import { pickCurrentNetwork } from '@/pages/networks/current'

import { Sidebar } from './Sidebar'

const SIDEBAR_COLLAPSED_KEY = 'sidebarCollapsed'

/**
 * 登录之后的外壳：侧栏和正文，没有顶栏。页面自己的标题就是"现在在哪里"，
 * 网络的名字在侧栏顶上；账号菜单和收起侧栏的按钮在侧栏底部。
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
        <ErrorState title={t.errors.loadFailed} error={networks.error} onRetry={() => void networks.refetch()} />
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
        onToggle={toggleSidebar}
        onSignOut={signOut}
        onCreateNetwork={() => setCreating(true)}
      />
      <div className="flex min-w-0 flex-1 flex-col">
        {reachable ? null : (
          <div
            role="alert"
            className="sticky top-0 z-30 flex items-center gap-2 bg-warn-soft px-10 py-2 text-[13px] font-medium text-warn"
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
