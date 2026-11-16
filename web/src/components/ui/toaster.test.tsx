import { act, fireEvent, screen } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { t } from '@/i18n'
import { renderWithApp } from '@/test/harness'

import { Dialog, DialogBody, DialogContent } from './dialog'
import { toast } from './toast'

afterEach(() => {
  vi.useRealTimers()
})

describe('操作的反馈', () => {
  it('成功的通知过几秒自己消失', () => {
    vi.useFakeTimers()
    renderWithApp(<p>页面</p>)
    act(() => toast.success('标签已更新'))
    expect(screen.getByRole('status')).toHaveTextContent('标签已更新')
    act(() => {
      vi.advanceTimersByTime(4000)
    })
    expect(screen.getByRole('status')).toBeInTheDocument()
    act(() => {
      vi.advanceTimersByTime(1000)
    })
    expect(screen.queryByRole('status')).not.toBeInTheDocument()
  })

  it('出错的不会自己消失，要用户看过之后关掉', () => {
    vi.useFakeTimers()
    renderWithApp(<p>页面</p>)
    act(() => toast.error('节点 laptop 没有声明可以作为出口'))
    act(() => {
      vi.advanceTimersByTime(60_000)
    })
    expect(screen.getByRole('alert')).toHaveTextContent('节点 laptop 没有声明可以作为出口')
    fireEvent.click(screen.getByRole('button', { name: t.common.dismiss }))
    expect(screen.queryByRole('alert')).not.toBeInTheDocument()
  })

  it('"正在……"的那一条就地变成结果，不另弹一条', () => {
    renderWithApp(<p>页面</p>)
    let id = 0
    act(() => {
      id = toast.loading('正在打标签…')
    })
    expect(screen.getByRole('status')).toHaveTextContent('正在打标签…')
    act(() => toast.success('已给 2 个节点打上标签', id))
    expect(screen.getAllByRole('status')).toHaveLength(1)
    expect(screen.getByRole('status')).toHaveTextContent('已给 2 个节点打上标签')
  })

  it('对话框开着时弹出的通知，读屏软件也读得到', () => {
    renderWithApp(
      <Dialog open>
        <DialogContent title="nas-01 的标签">
          <DialogBody>改了立即生效。</DialogBody>
        </DialogContent>
      </Dialog>,
    )
    act(() => toast.success('标签已更新'))
    // 对话框把它外面的内容都对读屏软件隐藏了；通知的容器不在其中。
    // getByRole 默认只找读屏软件读得到的元素，所以找得到就说明没有被隐藏。
    expect(screen.getByRole('list', { name: t.common.notifications })).toBeInTheDocument()
    expect(screen.getByRole('status')).toHaveTextContent('标签已更新')
  })
})
