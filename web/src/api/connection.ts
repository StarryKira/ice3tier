import { useSyncExternalStore } from 'react'

/**
 * controller 现在连不连得上。每个请求结束时由 API 客户端报告；连不上时页面顶部出现一条横幅，
 * 之后任何一个请求成功（轮询会一直重试）横幅就撤掉。
 */
let reachable = true
const listeners = new Set<() => void>()

export function reportReachability(next: boolean): void {
  if (next === reachable) return
  reachable = next
  for (const listener of listeners) listener()
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener)
  return () => listeners.delete(listener)
}

export function useReachable(): boolean {
  return useSyncExternalStore(subscribe, () => reachable)
}
