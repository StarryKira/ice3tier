import { useSyncExternalStore } from 'react'

import { readStored, writeStored } from '@/lib/storage'

/**
 * 亮色和暗色。选择有三种：跟随系统（默认）、亮、暗；记在浏览器里（`localStorage` 的 `i3t.theme`）。
 * 暗色就是 `<html>` 上的 `dark` 类，颜色全在 `index.css` 里换。
 * `index.html` 里有一小段脚本在页面画出来之前做同样的事，打开时不会先闪一下白。
 */
export const THEMES = ['system', 'light', 'dark'] as const
export type Theme = (typeof THEMES)[number]
export type ResolvedTheme = 'light' | 'dark'

const STORAGE_KEY = 'theme'

const isTheme = (value: unknown): value is Theme => THEMES.some((theme) => theme === value)

const media: MediaQueryList | null =
  typeof window !== 'undefined' && typeof window.matchMedia === 'function'
    ? window.matchMedia('(prefers-color-scheme: dark)')
    : null

let chosen: Theme = readStored(STORAGE_KEY, 'system', isTheme)
let resolved: ResolvedTheme = resolve(chosen)
const listeners = new Set<() => void>()

function resolve(theme: Theme): ResolvedTheme {
  if (theme !== 'system') return theme
  return media?.matches === true ? 'dark' : 'light'
}

function apply(): void {
  resolved = resolve(chosen)
  if (typeof document === 'undefined') return
  const root = document.documentElement
  root.classList.toggle('dark', resolved === 'dark')
  root.style.colorScheme = resolved
}
apply()

function notify(): void {
  for (const listener of listeners) listener()
}

// 跟随系统时，系统换了跟着换。
media?.addEventListener('change', () => {
  if (chosen !== 'system') return
  apply()
  notify()
})

export function setTheme(theme: Theme): void {
  writeStored(STORAGE_KEY, theme)
  if (theme === chosen) return
  chosen = theme
  apply()
  notify()
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener)
  return () => listeners.delete(listener)
}

/** 选的是哪一种（可能是"跟随系统"）。 */
export function useTheme(): Theme {
  return useSyncExternalStore(subscribe, () => chosen)
}

/** 实际用的是亮还是暗。画图的库要具体的颜色值，靠它挑。 */
export function useResolvedTheme(): ResolvedTheme {
  return useSyncExternalStore(subscribe, () => resolved)
}
