import { useSyncExternalStore } from 'react'

export type ToastKind = 'loading' | 'success' | 'error' | 'info'

export interface ToastItem {
  id: number
  kind: ToastKind
  message: string
}

/**
 * 操作的反馈：右下角的通知。这里是它的数据和调用入口，显示在 `toaster.tsx`。
 */
let items: readonly ToastItem[] = []
let nextId = 1
const listeners = new Set<() => void>()

function emit(next: readonly ToastItem[]): void {
  items = next
  for (const listener of listeners) listener()
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener)
  return () => listeners.delete(listener)
}

function show(kind: ToastKind, message: string): number {
  const id = nextId
  nextId += 1
  emit([...items, { id, kind, message }])
  return id
}

function update(id: number, kind: ToastKind, message: string): void {
  if (!items.some((item) => item.id === id)) {
    show(kind, message)
    return
  }
  emit(items.map((item) => (item.id === id ? { id, kind, message } : item)))
}

function dismiss(id: number): void {
  emit(items.filter((item) => item.id !== id))
}

/**
 * `loading` 返回的编号可以交给 `success` / `error`，让同一条通知从"正在……"变成结果。
 * 成功和提示类的过几秒自己消失；出错的不消失，要用户看过之后关掉。
 */
export const toast = {
  success: (message: string, id?: number): void => {
    if (id === undefined) show('success', message)
    else update(id, 'success', message)
  },
  error: (message: string, id?: number): void => {
    if (id === undefined) show('error', message)
    else update(id, 'error', message)
  },
  info: (message: string): void => {
    show('info', message)
  },
  loading: (message: string): number => show('loading', message),
  dismiss,
  /** 测试用：清空。 */
  clear: (): void => emit([]),
}

export function useToasts(): readonly ToastItem[] {
  return useSyncExternalStore(subscribe, () => items)
}
