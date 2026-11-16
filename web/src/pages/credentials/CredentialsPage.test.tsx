import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it } from 'vitest'

import { type CreatedCredential, type Credential, type Listener } from '@/api/types'
import { t } from '@/i18n'
import { apiFailure, fakeApi, type FakeApi, NETWORK, renderWithApp, type Routes } from '@/test/harness'

import { CredentialsPage } from './CredentialsPage'

const c = t.credentials
const SECRET = 'i3tc-c2VjcmV0LXNob3duLW9ubHktb25jZS1hYmNkZWZnaGlqa2xtbm8'

const PHONE: Credential = { id: 1, name: 'phone', max_concurrent: 8, created_at: 1_794_000_000 }

const LAN_LISTENER: Listener = {
  id: 3,
  node: 1,
  name: 'lan',
  bind: '192.168.1.5:1080',
  protocol: 'socks5',
  allow_from: ['192.168.1.0/24'],
  require_credential: true,
  default_action: null,
  fixed_exit: null,
}

/** 一个会记住签发过哪些凭据的 controller。列表接口和真的一样，从不返回令牌。 */
function controller(existing: Credential[] = [], extra: Routes = {}): FakeApi {
  const stored = [...existing]
  return fakeApi({
    'GET /networks/1/credentials': () => stored,
    'GET /networks/1/listeners': [],
    'GET /networks/1/nodes': [],
    'POST /networks/1/credentials': (call) => {
      const body = call.body as { name: string; max_concurrent?: number }
      const credential: Credential = {
        id: stored.length + 1,
        name: body.name,
        max_concurrent: body.max_concurrent ?? null,
        created_at: 1_794_000_100,
      }
      stored.push(credential)
      return { ...credential, token: SECRET } satisfies CreatedCredential
    },
    ...extra,
  })
}

async function openIssueDialog(user: ReturnType<typeof userEvent.setup>): Promise<HTMLElement> {
  await user.click(await screen.findByRole('button', { name: c.issue }))
  return screen.findByRole('dialog', { name: c.createTitle })
}

describe('签发凭据', () => {
  it('签发之后令牌显示这一次；关掉之后列表里只有名字，哪里都查不回来', async () => {
    const user = userEvent.setup()
    const api = controller([PHONE])
    renderWithApp(<CredentialsPage />, { network: NETWORK })

    const dialog = await openIssueDialog(user)
    await user.type(within(dialog).getByLabelText(c.nameLabel), 'guest')
    await user.click(within(dialog).getByRole('button', { name: c.submit }))

    const secret = await screen.findByTestId('secret-dialog')
    expect(within(secret).getByRole('heading', { name: c.secretTitle('guest') })).toBeVisible()
    expect(within(secret).getByTestId('secret-value')).toHaveTextContent(SECRET)
    expect(within(secret).getByText(c.secretWarning)).toBeVisible()
    expect(api.callsTo('POST /networks/1/credentials')[0]?.body).toEqual({ name: 'guest' })
    // 填名字的那个对话框已经让位了。
    expect(screen.queryByRole('dialog', { name: c.createTitle })).not.toBeInTheDocument()

    await user.keyboard('{Escape}')
    expect(screen.getByTestId('secret-dialog')).toBeVisible()

    await user.click(within(secret).getByRole('button', { name: c.savedClose }))
    await waitFor(() => expect(screen.queryByTestId('secret-dialog')).not.toBeInTheDocument())
    expect(document.body).not.toHaveTextContent(SECRET)
    expect(await screen.findByTestId('credential-row-guest')).toBeVisible()
    expect(screen.getByText(c.tokenHidden)).toBeVisible()
  })

  it('并发上限：填了就交给 controller，留空就不写', async () => {
    const user = userEvent.setup()
    const api = controller([PHONE])
    renderWithApp(<CredentialsPage />, { network: NETWORK })
    const dialog = await openIssueDialog(user)
    await user.type(within(dialog).getByLabelText(c.nameLabel), '  tablet ')
    await user.type(within(dialog).getByLabelText(new RegExp(c.maxLabel)), '4')
    await user.click(within(dialog).getByRole('button', { name: c.submit }))
    await screen.findByTestId('secret-dialog')
    expect(api.callsTo('POST /networks/1/credentials')[0]?.body).toEqual({ name: 'tablet', max_concurrent: 4 })
  })

  it('名字没填、并发上限不是正整数：写在各自的输入框下面，不发请求', async () => {
    const user = userEvent.setup()
    const api = controller([PHONE])
    renderWithApp(<CredentialsPage />, { network: NETWORK })
    const dialog = await openIssueDialog(user)
    await user.type(within(dialog).getByLabelText(new RegExp(c.maxLabel)), '0')
    await user.click(within(dialog).getByRole('button', { name: c.submit }))

    expect(within(dialog).getByLabelText(c.nameLabel)).toHaveAccessibleDescription(new RegExp(t.validate.titleRequired))
    expect(within(dialog).getByLabelText(new RegExp(c.maxLabel))).toHaveAccessibleDescription(
      new RegExp(t.validate.positiveInt),
    )
    expect(api.callsTo('POST /networks/1/credentials')).toHaveLength(0)
    expect(screen.queryByTestId('secret-dialog')).not.toBeInTheDocument()
  })

  it('名字已经被占用：controller 的话写在名字下面，对话框留着，填的内容还在', async () => {
    const user = userEvent.setup()
    controller([PHONE], {
      'POST /networks/1/credentials': () => apiFailure(409, 'name_taken', '名字 phone 已经被占用'),
    })
    renderWithApp(<CredentialsPage />, { network: NETWORK })
    const dialog = await openIssueDialog(user)
    const name = within(dialog).getByLabelText(c.nameLabel)
    await user.type(name, 'phone')
    await user.click(within(dialog).getByRole('button', { name: c.submit }))

    await waitFor(() => expect(name).toHaveAccessibleDescription(/名字 phone 已经被占用/))
    expect(name).toHaveValue('phone')
    expect(screen.queryByTestId('secret-dialog')).not.toBeInTheDocument()
    // 右下角不再重复弹同一句话。
    const toasts = screen.getByRole('list', { name: t.common.notifications })
    expect(within(toasts).queryByText(/已经被占用/)).not.toBeInTheDocument()
  })

  it('说不清属于哪个字段的错误写在表单底部', async () => {
    const user = userEvent.setup()
    controller([PHONE], { 'POST /networks/1/credentials': () => apiFailure(500, 'internal', 'controller 内部出错') })
    renderWithApp(<CredentialsPage />, { network: NETWORK })
    const dialog = await openIssueDialog(user)
    await user.type(within(dialog).getByLabelText(c.nameLabel), 'guest')
    await user.click(within(dialog).getByRole('button', { name: c.submit }))
    expect(await within(dialog).findByText('controller 内部出错')).toBeVisible()
    expect(within(dialog).getByLabelText(c.nameLabel)).not.toHaveAttribute('aria-invalid')
  })
})

describe('签发之后的"下一步"', () => {
  it('还没有要求凭据的监听器：说明暂时用不上，给出去建监听器的入口', async () => {
    const user = userEvent.setup()
    controller([PHONE])
    renderWithApp(<CredentialsPage />, { network: NETWORK })
    const dialog = await openIssueDialog(user)
    await user.type(within(dialog).getByLabelText(c.nameLabel), 'guest')
    await user.click(within(dialog).getByRole('button', { name: c.submit }))
    const secret = await screen.findByTestId('secret-dialog')
    expect(within(secret).getByText(c.usageNoListener)).toBeVisible()
    expect(within(secret).getByRole('link', { name: c.newListener })).toHaveAttribute('href', '/n/1/listeners')
  })

  it('有的话，例子里是真实的地址和这个凭据', async () => {
    const user = userEvent.setup()
    controller([PHONE], { 'GET /networks/1/listeners': [LAN_LISTENER] })
    renderWithApp(<CredentialsPage />, { network: NETWORK })
    const dialog = await openIssueDialog(user)
    await user.type(within(dialog).getByLabelText(c.nameLabel), 'guest')
    await user.click(within(dialog).getByRole('button', { name: c.submit }))
    const secret = await screen.findByTestId('secret-dialog')
    expect(await within(secret).findByText(new RegExp(`:${SECRET}@192\\.168\\.1\\.5:1080`))).toBeVisible()
  })
})

describe('凭据列表', () => {
  it('没有凭据也没有对外的监听器：说明用不到，先指给监听器', async () => {
    controller([])
    renderWithApp(<CredentialsPage />, { network: NETWORK })
    expect(await screen.findByText(c.emptyTitle)).toBeVisible()
    expect(screen.getByText(c.emptyNoListener)).toBeVisible()
    expect(screen.getByRole('link', { name: c.goListeners })).toHaveAttribute('href', '/n/1/listeners')
  })

  it('取不到列表：说是哪一步、什么原因，可以重试', async () => {
    const user = userEvent.setup()
    const api = controller([PHONE], {
      'GET /networks/1/credentials': () => apiFailure(500, 'internal', 'controller 内部出错'),
    })
    renderWithApp(<CredentialsPage />, { network: NETWORK })
    expect(await screen.findByText(c.loadFailed)).toBeVisible()
    expect(screen.getByText(/controller 内部出错/)).toBeVisible()

    api.use({ 'GET /networks/1/credentials': [PHONE] })
    await user.click(screen.getByRole('button', { name: t.common.retry }))
    expect(await screen.findByTestId('credential-row-phone')).toBeVisible()
  })

  it('吊销要确认，写明后果；确认之后才发请求', async () => {
    const user = userEvent.setup()
    const api = controller([PHONE], { 'DELETE /networks/1/credentials/1': undefined })
    renderWithApp(<CredentialsPage />, { network: NETWORK })
    await user.click(await screen.findByRole('button', { name: t.common.actionsFor('phone') }))
    await user.click(await screen.findByRole('menuitem', { name: c.menuRevoke }))

    const question = await screen.findByRole('alertdialog', { name: c.revokeTitle('phone') })
    expect(within(question).getByText(c.revokeBody)).toBeVisible()
    expect(api.callsTo('DELETE /networks/1/credentials/1')).toHaveLength(0)

    await user.click(within(question).getByRole('button', { name: c.revokeConfirm }))
    await waitFor(() => expect(api.callsTo('DELETE /networks/1/credentials/1')).toHaveLength(1))
    expect(await screen.findByText(c.revoked('phone'))).toBeVisible()
  })
})
