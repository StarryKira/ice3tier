import { useCallback, useRef } from 'react'
import { useSearchParams } from 'react-router'

export interface FocusRow {
  /** 地址里 `focus=` 指的编号；没有时是 `null`。 */
  focus: number | null
  /** 给每一行的 `ref`：指的那一行第一次出现时滚到看得见的地方。 */
  rowRef: (id: number) => (element: HTMLElement | null) => void
  /** 列表已经取回来了，却没有这一行：东西已经被删掉了。 */
  missing: boolean
}

/**
 * 从操作记录点过来时（`?focus=<编号>`），列表里那一行亮出来并滚到看得见的地方。
 * 东西后来被删掉了的话，页面上说一句，而不是让人对着列表找。
 */
export function useFocusRow(loaded: readonly { id: number }[] | undefined): FocusRow {
  const [params] = useSearchParams()
  const raw = Number(params.get('focus'))
  const focus = Number.isInteger(raw) && raw > 0 ? raw : null
  const scrolled = useRef<number | null>(null)

  const rowRef = useCallback(
    (id: number) => (element: HTMLElement | null) => {
      if (element === null || id !== focus || scrolled.current === focus) return
      scrolled.current = focus
      element.scrollIntoView({ block: 'center' })
    },
    [focus],
  )

  const missing = focus !== null && loaded !== undefined && !loaded.some((item) => item.id === focus)
  return { focus, rowRef, missing }
}

/** 亮出来的那一行的样式。 */
export const FOCUSED_ROW = 'bg-accent/60 shadow-[inset_3px_0_0_var(--color-primary)]'
