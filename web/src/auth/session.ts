import { useSyncExternalStore } from 'react'

/**
 * 登录状态就是一枚管理令牌（controller 第一次启动时打印的那个）。
 *
 * 令牌存在浏览器里：勾了"保持登录"放在 localStorage，关掉浏览器之后还在；没勾放在 sessionStorage，关掉标签页就没了。
 * 两处都读不到（存储被禁用）时只留在内存里，刷新页面要重新登录。
 */
const KEY = 'i3t.adminToken'

function read(): string | null {
  try {
    return window.sessionStorage.getItem(KEY) ?? window.localStorage.getItem(KEY)
  } catch {
    return null
  }
}

/** 上一次退出的原因：登录页据此说一句"登录已失效"或者"已退出"。 */
export type SignOutReason = 'manual' | 'expired'

let token: string | null = read()
let lastSignOut: SignOutReason | null = null
const listeners = new Set<() => void>()

function set(next: string | null): void {
  token = next
  for (const listener of listeners) listener()
}

function forget(): void {
  try {
    window.sessionStorage.removeItem(KEY)
    window.localStorage.removeItem(KEY)
  } catch {
    // 存储不可用时本来就没存。
  }
}

export const session = {
  token: () => token,

  signIn: (next: string, remember: boolean): void => {
    forget()
    try {
      ;(remember ? window.localStorage : window.sessionStorage).setItem(KEY, next)
    } catch {
      // 存不下就只留在内存里。
    }
    lastSignOut = null
    set(next)
  },

  /** 登录时勾了"保持登录"：令牌在 localStorage 里，关掉浏览器之后还在。 */
  remembered: (): boolean => {
    try {
      return window.localStorage.getItem(KEY) !== null
    } catch {
      return false
    }
  },

  /** `expired`：带着令牌的请求被 controller 拒绝了（令牌被重新生成过）。 */
  signOut: (reason: SignOutReason): void => {
    forget()
    lastSignOut = reason
    set(null)
  },

  /** 取走上一次退出的原因；只说一次。 */
  takeSignOutReason: (): SignOutReason | null => {
    const reason = lastSignOut
    lastSignOut = null
    return reason
  },

  subscribe: (listener: () => void): (() => void) => {
    listeners.add(listener)
    return () => listeners.delete(listener)
  },
}

export function useToken(): string | null {
  return useSyncExternalStore(session.subscribe, session.token)
}
