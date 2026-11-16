import { describe, expect, it } from 'vitest'

import { setTheme } from './theme'

describe('亮色和暗色', () => {
  it('选暗色：<html> 上有 dark 类，配色方案是暗的，选择记在浏览器里', () => {
    setTheme('dark')
    expect(document.documentElement).toHaveClass('dark')
    expect(document.documentElement.style.colorScheme).toBe('dark')
    expect(window.localStorage.getItem('i3t.theme')).toBe('"dark"')
  })

  it('选亮色：去掉 dark 类', () => {
    setTheme('dark')
    setTheme('light')
    expect(document.documentElement).not.toHaveClass('dark')
    expect(document.documentElement.style.colorScheme).toBe('light')
  })

  it('跟随系统：系统没说要暗色时是亮的', () => {
    setTheme('dark')
    setTheme('system')
    expect(document.documentElement).not.toHaveClass('dark')
    expect(window.localStorage.getItem('i3t.theme')).toBe('"system"')
  })
})
