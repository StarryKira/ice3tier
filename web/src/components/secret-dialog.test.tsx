import { act, fireEvent, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { useState } from 'react'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { t } from '@/i18n'
import { renderWithApp } from '@/test/harness'

import { CodeBlock } from './copy'
import { SecretDialog } from './secret-dialog'

const SECRET = 'i3tc-Zm9vYmFyYmF6cXV4LXNlY3JldC12YWx1ZS0xMjM0NTY3ODkw'

/** 和页面上的用法一样：点了"我已保存，关闭"之后，密钥连同对话框一起从界面上拿掉。 */
function Harness({ onClose }: { onClose?: () => void }) {
  const [secret, setSecret] = useState<string | null>(SECRET)
  if (secret === null) return <p>列表</p>
  return (
    <SecretDialog
      open
      title="凭据 phone 已签发"
      warning="它不会再显示。"
      warningRest="现在复制并保存好。"
      secret={secret}
      secretLabel="凭据"
      copyLabel="复制凭据"
      closeLabel="我已保存，关闭"
      onClose={() => {
        onClose?.()
        setSecret(null)
      }}
    >
      <p>下一步：把它填在代理的密码位置。</p>
    </SecretDialog>
  )
}

function stubClipboard(writeText: (text: string) => Promise<void>): void {
  Object.defineProperty(navigator, 'clipboard', { value: { writeText }, configurable: true })
}

afterEach(() => {
  vi.useRealTimers()
  Reflect.deleteProperty(navigator, 'clipboard')
})

describe('只显示一次的密钥', () => {
  it('完整显示，不遮挡；说明它不会再显示；紧接着是下一步', () => {
    renderWithApp(<Harness />)
    const dialog = screen.getByRole('dialog', { name: '凭据 phone 已签发' })
    expect(dialog).toBeVisible()
    expect(screen.getByTestId('secret-value')).toHaveTextContent(SECRET)
    expect(screen.getByText('它不会再显示。')).toBeVisible()
    expect(screen.getByText(/现在复制并保存好。/)).toBeVisible()
    expect(screen.getByText('下一步：把它填在代理的密码位置。')).toBeVisible()
  })

  it('按 Esc 关不掉', async () => {
    const user = userEvent.setup()
    const onClose = vi.fn()
    renderWithApp(<Harness onClose={onClose} />)
    await user.keyboard('{Escape}')
    expect(screen.getByRole('dialog')).toBeVisible()
    expect(screen.getByTestId('secret-value')).toHaveTextContent(SECRET)
    expect(onClose).not.toHaveBeenCalled()
  })

  it('没有右上角的叉，点对话框外面也关不掉', () => {
    const onClose = vi.fn()
    renderWithApp(<Harness onClose={onClose} />)
    expect(screen.queryByRole('button', { name: t.common.close })).not.toBeInTheDocument()
    // 对话框打开时页面的其余部分不接受点击；直接对着页面发指针事件，等于点在遮罩上。
    fireEvent.pointerDown(document.body)
    fireEvent.mouseDown(document.body)
    fireEvent.click(document.body)
    expect(screen.getByRole('dialog')).toBeVisible()
    expect(onClose).not.toHaveBeenCalled()
  })

  it('只能点"我已保存，关闭"；关掉之后界面上不再有这个密钥，也没有"再看一次"', async () => {
    const user = userEvent.setup()
    const onClose = vi.fn()
    renderWithApp(<Harness onClose={onClose} />)
    await user.click(screen.getByRole('button', { name: '我已保存，关闭' }))
    expect(onClose).toHaveBeenCalledOnce()
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument()
    expect(document.body).not.toHaveTextContent(SECRET)
    expect(screen.getByText('列表')).toBeVisible()
  })

  it('复制出去的是完整的密钥', async () => {
    const writeText = vi.fn<(text: string) => Promise<void>>().mockResolvedValue(undefined)
    stubClipboard(writeText)
    renderWithApp(<Harness />)
    fireEvent.click(screen.getByRole('button', { name: '复制凭据' }))
    expect(await screen.findByRole('button', { name: t.common.copied })).toBeVisible()
    expect(writeText).toHaveBeenCalledExactlyOnceWith(SECRET)
  })

  it('点过之后按钮变成"已复制"，保持两秒再变回来', async () => {
    vi.useFakeTimers()
    stubClipboard(() => Promise.resolve())
    renderWithApp(<Harness />)
    fireEvent.click(screen.getByRole('button', { name: '复制凭据' }))
    // 让写剪贴板的 Promise 落定。
    await act(async () => {
      await Promise.resolve()
    })
    expect(screen.getByRole('button', { name: t.common.copied })).toBeVisible()

    act(() => {
      vi.advanceTimersByTime(1900)
    })
    expect(screen.getByRole('button', { name: t.common.copied })).toBeVisible()

    act(() => {
      vi.advanceTimersByTime(200)
    })
    expect(screen.getByRole('button', { name: '复制凭据' })).toBeVisible()
    expect(screen.queryByRole('button', { name: t.common.copied })).not.toBeInTheDocument()
  })

  it('剪贴板用不了时不谎称"已复制"，说一句请手动复制', async () => {
    // jsdom 里既没有 navigator.clipboard，也没有 document.execCommand：两条路都走不通。
    renderWithApp(<Harness />)
    fireEvent.click(screen.getByRole('button', { name: '复制凭据' }))
    expect(await screen.findByText(t.common.copyFailed)).toBeVisible()
    expect(screen.queryByRole('button', { name: t.common.copied })).not.toBeInTheDocument()
    // 密钥还在屏幕上，可以手动选中。
    expect(screen.getByTestId('secret-value')).toHaveTextContent(SECRET)
  })

  it('剪贴板接口拒绝时退回到老办法', async () => {
    stubClipboard(() => Promise.reject(new Error('NotAllowedError')))
    const execCommand = vi.fn<(command: string) => boolean>().mockReturnValue(true)
    Object.defineProperty(document, 'execCommand', { value: execCommand, configurable: true })
    try {
      renderWithApp(<Harness />)
      fireEvent.click(screen.getByRole('button', { name: '复制凭据' }))
      expect(await screen.findByRole('button', { name: t.common.copied })).toBeVisible()
      expect(execCommand).toHaveBeenCalledWith('copy')
    } finally {
      Reflect.deleteProperty(document, 'execCommand')
    }
  })
})

describe('命令和密钥的代码块', () => {
  it('还没有可以复制的东西时按钮是灰的', () => {
    renderWithApp(
      <CodeBlock copy={null} copyLabel="复制命令" disabledReason="先生成注册令牌">
        {'echo <注册令牌> | i3t join'}
      </CodeBlock>,
    )
    expect(screen.getByRole('button', { name: '复制命令' })).toBeDisabled()
  })

  it('显示可以折行，复制出去的是另外给的那一整行', async () => {
    const writeText = vi.fn<(text: string) => Promise<void>>().mockResolvedValue(undefined)
    stubClipboard(writeText)
    renderWithApp(
      <CodeBlock
        copy="curl -fLo i3t https://example.com/i3t && sudo install -m 755 i3t /usr/local/bin/i3t"
        copyLabel="复制"
      >
        {'curl -fLo i3t https://example.com/i3t\nsudo install -m 755 i3t /usr/local/bin/i3t'}
      </CodeBlock>,
    )
    fireEvent.click(screen.getByRole('button', { name: '复制' }))
    await screen.findByRole('button', { name: t.common.copied })
    expect(writeText).toHaveBeenCalledWith(
      'curl -fLo i3t https://example.com/i3t && sudo install -m 755 i3t /usr/local/bin/i3t',
    )
  })
})
