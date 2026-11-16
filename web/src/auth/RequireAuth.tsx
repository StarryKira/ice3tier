import { Navigate, Outlet, useLocation } from 'react-router'

import { useToken } from './session'

/** 没登录就去登录页，并记下原来要去的地址，登录后回到那里。 */
export function RequireAuth() {
  const token = useToken()
  const location = useLocation()
  if (token === null) {
    const next = location.pathname + location.search
    const target = next === '/' ? '/login' : `/login?next=${encodeURIComponent(next)}`
    return <Navigate to={target} replace />
  }
  return <Outlet />
}
