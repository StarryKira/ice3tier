import '@testing-library/jest-dom/vitest'

import { cleanup } from '@testing-library/react'
import { afterEach } from 'vitest'

import { toast } from '@/components/ui/toast'

afterEach(() => {
  cleanup()
  toast.clear()
  window.localStorage.clear()
  window.sessionStorage.clear()
})

// jsdom 不做布局，也就没有下面这几样；Radix 的部件要用到，给它们空的实现。
class ResizeObserverStub implements ResizeObserver {
  observe(): void {
    // 没有布局可观察。
  }
  unobserve(): void {
    // 同上。
  }
  disconnect(): void {
    // 同上。
  }
}
globalThis.ResizeObserver = ResizeObserverStub
Element.prototype.scrollIntoView = () => undefined
Element.prototype.hasPointerCapture = () => false
Element.prototype.releasePointerCapture = () => undefined
