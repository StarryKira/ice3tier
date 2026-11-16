import { useSyncExternalStore } from 'react'

import { readStored, writeStored } from '@/lib/storage'

import { en } from './en'
import { type Messages } from './messages'
import { zhCN } from './zh-CN'

export type { Messages }

/**
 * 界面上的文字都在 `src/i18n/` 下面，组件里不直接写界面文字。每种语言一个目录，导出一个 {@link Messages}。
 *
 * 加一种语言：照着 `en/` 写一个目录，在下面的表里加一行，写上它用自己的文字写的名字。
 */
export const LOCALES = [
  { code: 'zh-CN', name: '简体中文', messages: zhCN },
  { code: 'en', name: 'English', messages: en },
] as const satisfies readonly { code: string; name: string; messages: Messages }[]

/** 语言的代码，同时也是 `<html lang>` 和 `Intl` 用的 BCP 47 标记。 */
export type Locale = (typeof LOCALES)[number]['code']

/** 浏览器想要的语言一个也没有时用英文。 */
const FALLBACK: Locale = 'en'

/** 记在浏览器里的选择（`localStorage` 的 `i3t.locale`）。 */
const STORAGE_KEY = 'locale'

const isLocale = (value: unknown): value is Locale => LOCALES.some((locale) => locale.code === value)

function messagesOf(code: Locale): Messages {
  return LOCALES.find((locale) => locale.code === code)?.messages ?? en
}

/**
 * 按浏览器的语言偏好（`navigator.languages`，先想要的在前）挑一种有的语言：先找完全一样的，
 * 再只比主语言（`zh-TW`、`zh-HK` 也用简体中文，`en-GB` 用英文）。都没有时用英文。
 */
export function matchLocale(preferred: readonly string[]): Locale {
  for (const wanted of preferred) {
    const lower = wanted.toLowerCase()
    const exact = LOCALES.find((locale) => locale.code.toLowerCase() === lower)
    if (exact !== undefined) return exact.code
    const language = lower.split('-')[0]
    const sameLanguage = LOCALES.find((locale) => locale.code.toLowerCase().split('-')[0] === language)
    if (sameLanguage !== undefined) return sameLanguage.code
  }
  return FALLBACK
}

/** 第一次打开时用哪种语言：记住的选择优先，其次是浏览器的偏好。 */
export function initialLocale(): Locale {
  const stored = readStored<Locale | null>(STORAGE_KEY, null, isLocale)
  if (stored !== null) return stored
  return matchLocale(typeof navigator === 'undefined' ? [] : navigator.languages)
}

type Tree = Record<string, unknown>

const isTree = (value: unknown): value is Tree => typeof value === 'object' && value !== null && !Array.isArray(value)

/**
 * 把一种语言的目录逐项写进 `target`，不换掉里面的对象。模块里常有 `const d = t.nodes.detail` 这样的简写，
 * 它们拿着的是 `t` 里面的对象；就地改写，这些简写换了语言之后读到的也是新的文字。
 */
function copyInto(target: Tree, source: Tree): void {
  for (const key of Object.keys(target)) {
    if (!(key in source)) Reflect.deleteProperty(target, key)
  }
  for (const [key, value] of Object.entries(source)) {
    const existing = target[key]
    if (isTree(value)) {
      if (isTree(existing)) copyInto(existing, value)
      else {
        const fresh: Tree = {}
        copyInto(fresh, value)
        target[key] = fresh
      }
    } else if (Array.isArray(value) && Array.isArray(existing)) {
      existing.splice(0, existing.length, ...(value as unknown[]))
    } else {
      target[key] = Array.isArray(value) ? [...(value as unknown[])] : value
    }
  }
}

let current: Locale = initialLocale()

/**
 * 现在这种语言的全部文字。换语言时就地改写（见 {@link copyInto}），所以在渲染时读它总是对的；
 * 不要在模块加载时就把里面的一句话取出来存着，那样换了语言也不会变（`live.test.ts` 检查这一点）。
 */
export const t: Messages = (() => {
  const live: Tree = {}
  copyInto(live, messagesOf(current))
  return live as unknown as Messages
})()

const listeners = new Set<() => void>()

function applyDocument(): void {
  if (typeof document !== 'undefined') document.documentElement.lang = current
}
applyDocument()

/** 现在的语言。 */
export function getLocale(): Locale {
  return current
}

/**
 * 换一种语言：就地改写 {@link t}、改 `<html lang>`，然后通知订阅的组件重新渲染，不刷新页面。
 * `remember` 为假时不记进浏览器（测试里固定语言用）。
 */
export function setLocale(code: Locale, { remember = true }: { remember?: boolean } = {}): void {
  if (remember) writeStored(STORAGE_KEY, code)
  if (code === current) return
  current = code
  copyInto(t, messagesOf(code))
  applyDocument()
  for (const listener of listeners) listener()
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener)
  return () => listeners.delete(listener)
}

/**
 * 订阅语言。应用的根组件用它：换语言时根组件重新渲染，路由下面的每个页面跟着重新渲染，
 * 状态（填了一半的表单、展开的行）都留着。自己缓存了文字的地方（`useMemo`、画图的库的节点）
 * 把它放进依赖里。
 */
export function useLocale(): Locale {
  return useSyncExternalStore(subscribe, getLocale)
}
