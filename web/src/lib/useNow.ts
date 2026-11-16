import { useSyncExternalStore } from 'react'

import { type Timestamp } from '@/api/types'

import { nowSeconds } from './time'

/**
 * 当前时间（Unix 秒），每 20 秒走一格。相对时间（"3 分钟前"）靠它自己往前走，不需要重新取数。
 * 所有用到它的组件共用一个定时器，标签页不在前台时浏览器会自己把定时器放慢。
 */
const TICK_MS = 20_000

let current = nowSeconds()
const listeners = new Set<() => void>()
let timer: number | undefined

function subscribe(listener: () => void): () => void {
  listeners.add(listener)
  if (timer === undefined) {
    current = nowSeconds()
    timer = window.setInterval(() => {
      current = nowSeconds()
      for (const notify of listeners) notify()
    }, TICK_MS)
  }
  return () => {
    listeners.delete(listener)
    if (listeners.size === 0 && timer !== undefined) {
      window.clearInterval(timer)
      timer = undefined
    }
  }
}

export function useNow(): Timestamp {
  return useSyncExternalStore(subscribe, () => current)
}
