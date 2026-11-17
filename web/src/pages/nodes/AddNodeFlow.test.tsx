import { screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'

import { type CreatedEnrollToken, type EnrollTokenInfo, type Group, type Node } from '@/api/types'
import { setLocale, t } from '@/i18n'
import { makeNode, makeToken } from '@/test/fixtures'
import { apiFailure, fakeApi, type FakeApi, NETWORK, renderWithApp, type Routes } from '@/test/harness'

import { AddNodeDialog } from './AddNodeDialog'
import { AddNodeFlow } from './AddNodeFlow'

// 等节点加入时页面每隔几秒取一次；测试里取得勤一些，不用真的等。
vi.mock('@/lib/polling', () => ({ LIVE_INTERVAL: 10_000, WAITING_INTERVAL: 40, GUIDE_INTERVAL: 5_000 }))

const TOKEN = 'i3t-AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE_first'
const SECOND_TOKEN = 'i3t-AgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgI_second'

const GROUPS: Group[] = [
  { id: 1, name: 'All', builtin: true, nodes: [], rules: [] },
  { id: 3, name: 'home_lab', builtin: false, nodes: [], rules: [] },
  { id: 2, name: 'Servers', builtin: false, nodes: [], rules: [] },
]

function tokenInfo(id: number): EnrollTokenInfo {
  const now = Math.floor(Date.now() / 1000)
  return makeToken(id, { created_at: now, expires_at: now + 86_400 })
}

function created(id: number, token: string): CreatedEnrollToken {
  return { ...tokenInfo(id), token }
}

/** 刚加入、还没跑起来的节点。主机名是 `<名字>.lan`：名字取的是它的第一段。 */
function node(id: number, name: string): Node {
  return makeNode(id, name, {
    hostname: `${name}.lan`,
    online: false,
    last_seen_at: null,
    sent_version: 0,
    acked_version: 0,
  })
}

/** 一个空网络的 controller：还没有节点，也没有令牌；生成令牌之后令牌出现在"没用掉的"列表里。 */
function emptyNetwork(extra: Routes = {}): FakeApi {
  const api = fakeApi({
    'GET /networks/1/nodes': [],
    'GET /networks/1/tokens': [],
    'GET /networks/1/groups': GROUPS,
    'POST /networks/1/tokens': () => {
      api.use({ 'GET /networks/1/tokens': [tokenInfo(7)] })
      return created(7, TOKEN)
    },
    ...extra,
  })
  return api
}

function command(): HTMLElement {
  return screen.getByTestId('join-command')
}

/** 名字里含有这段文字（标签旁边还有"可选"或一句提示）。 */
function containing(text: string): RegExp {
  return new RegExp(text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'))
}

/** 令牌生成之前命令里的占位符。测试开始时语言才固定下来，所以现取。 */
const placeholder = () => t.addNode.tokenPlaceholder

async function generate(user: ReturnType<typeof userEvent.setup>): Promise<void> {
  await user.click(screen.getByRole('button', { name: t.addNode.generate }))
  await waitFor(() => expect(command()).toHaveTextContent(TOKEN))
}

describe('添加节点', () => {
  it('令牌生成之前：命令里是占位符，让人先看懂结构；没有东西可以复制', () => {
    emptyNetwork()
    renderWithApp(<AddNodeFlow network={NETWORK} />)
    expect(command()).toHaveTextContent(`echo ${placeholder()} | i3t join`)
    expect(screen.getByRole('button', { name: t.addNode.copyCommand })).toBeDisabled()
    expect(screen.queryByRole('button', { name: t.addNode.copyToken })).not.toBeInTheDocument()
    expect(screen.queryByText(t.addNode.waiting)).not.toBeInTheDocument()
  })

  it('打开界面不会自己生成令牌：要点了按钮才发请求', () => {
    const api = emptyNetwork()
    renderWithApp(<AddNodeFlow network={NETWORK} />)
    expect(api.callsTo('POST /networks/1/tokens')).toHaveLength(0)
  })

  it('勾选"可以承担"，命令跟着变', async () => {
    const user = userEvent.setup()
    emptyNetwork()
    renderWithApp(<AddNodeFlow network={NETWORK} />)
    await user.click(screen.getByRole('checkbox', { name: containing(t.addNode.declareRelay) }))
    expect(command()).toHaveTextContent(`echo ${placeholder()} | i3t join --can-relay`)
    await user.click(screen.getByRole('checkbox', { name: containing(t.addNode.declareExit) }))
    expect(command()).toHaveTextContent(`echo ${placeholder()} | i3t join --can-exit --can-relay`)
    await user.click(screen.getByRole('checkbox', { name: containing(t.addNode.declareRelay) }))
    expect(command()).toHaveTextContent(`echo ${placeholder()} | i3t join --can-exit`)
    // 端口转发的目标在节点详情里打开，不在这里勾。
    expect(screen.getAllByRole('checkbox')).toHaveLength(2)
  })

  it('换平台，命令跟着变', async () => {
    const user = userEvent.setup()
    emptyNetwork()
    renderWithApp(<AddNodeFlow network={NETWORK} />)
    await user.click(screen.getByRole('radio', { name: 'Windows' }))
    expect(command()).toHaveTextContent(`echo ${placeholder()} | .\\i3t.exe join`)
    expect(screen.getByText(/curl\.exe -fLo i3t\.exe/)).toBeVisible()
  })

  it('生成之后：真正的令牌出现在命令里，可以整行复制；说明只显示这一次；开始等节点', async () => {
    const user = userEvent.setup()
    const api = emptyNetwork()
    renderWithApp(<AddNodeFlow network={NETWORK} />)
    await user.click(screen.getByRole('checkbox', { name: containing(t.addNode.declareRelay) }))
    await generate(user)

    expect(command()).toHaveTextContent(`echo ${TOKEN} | i3t join --can-relay`)
    expect(screen.getByText(t.addNode.shownOnce)).toBeVisible()
    expect(screen.getByText(t.addNode.waiting)).toBeVisible()
    // 声明是命令的参数，不写在令牌上。
    expect(api.callsTo('POST /networks/1/tokens')[0]?.body).toEqual({ expires_in_secs: 86_400, groups: [] })

    await user.click(screen.getByRole('button', { name: t.addNode.copyCommand }))
    expect(await navigator.clipboard.readText()).toBe(`echo ${TOKEN} | i3t join --can-relay`)
    await user.click(screen.getByRole('button', { name: t.addNode.copyToken }))
    expect(await navigator.clipboard.readText()).toBe(TOKEN)
  })

  for (const locale of ['zh-CN', 'en'] as const) {
    it(`分组写在令牌上：只从已有的分组里选，生成时按编号交给 controller（${locale}）`, async () => {
      setLocale(locale, { remember: false })
      const user = userEvent.setup()
      const api = emptyNetwork()
      renderWithApp(<AddNodeFlow network={NETWORK} />)
      await user.click(screen.getByRole('combobox', { name: containing(t.addNode.groupsLabel) }))
      const search = screen.getByRole('textbox', { name: t.groups.picker.search })
      // 搜索只是筛选：没有同名的分组时什么也不会新建。
      await user.type(search, 'nope')
      expect(await screen.findByText(t.groups.picker.noneFound)).toBeVisible()
      await user.type(search, '{Enter}')
      await user.clear(search)
      await user.type(search, 'serv{Enter}')
      await user.clear(search)
      await user.type(search, 'home{Enter}')
      await user.keyboard('{Escape}')
      expect(screen.getByRole('link', { name: t.groups.picker.manage })).toHaveAttribute('href', '/n/1/groups')
      await generate(user)
      expect(api.callsTo('POST /networks/1/tokens')[0]?.body).toMatchObject({ groups: [3, 2] })
    })
  }

  it('生成之后还能改"可以承担"：命令变，令牌不变，不再发请求', async () => {
    const user = userEvent.setup()
    const api = emptyNetwork()
    renderWithApp(<AddNodeFlow network={NETWORK} />)
    await generate(user)
    await user.click(screen.getByRole('checkbox', { name: containing(t.addNode.declareExit) }))
    expect(command()).toHaveTextContent(`echo ${TOKEN} | i3t join --can-exit`)
    expect(api.callsTo('POST /networks/1/tokens')).toHaveLength(1)
  })

  it('写在令牌上的选项生成之后不能再改，并说明为什么', async () => {
    const user = userEvent.setup()
    emptyNetwork()
    renderWithApp(<AddNodeFlow network={NETWORK} />)
    expect(screen.queryByText(t.addNode.lockedNote)).not.toBeInTheDocument()
    await generate(user)
    expect(screen.getByRole('combobox', { name: containing(t.addNode.groupsLabel) })).toBeDisabled()
    expect(screen.getByLabelText(t.addNode.ttlLabel)).toBeDisabled()
    expect(screen.getByLabelText(containing(t.addNode.ipLabel))).toBeDisabled()
    expect(screen.getByText(t.addNode.lockedNote)).toBeVisible()
  })

  it('预设的虚拟 IP 不在地址段里：输入框下面就说，不让生成', async () => {
    const user = userEvent.setup()
    const api = emptyNetwork()
    renderWithApp(<AddNodeFlow network={NETWORK} />)
    const ip = screen.getByLabelText(containing(t.addNode.ipLabel))
    await user.type(ip, '10.99.0.5')
    expect(screen.getByText(t.validate.ipOutOfRange('10.66.0.0/16'))).toBeInTheDocument()
    expect(screen.getByRole('button', { name: t.addNode.generate })).toBeDisabled()

    await user.clear(ip)
    await user.type(ip, '10.66.0.50')
    await generate(user)
    expect(api.callsTo('POST /networks/1/tokens')[0]?.body).toMatchObject({ virtual_ip: '10.66.0.50' })
  })

  it('controller 拒绝生成：按钮恢复，原因按错误码写在按钮下面', async () => {
    const user = userEvent.setup()
    emptyNetwork({
      'POST /networks/1/tokens': () => apiFailure(409, 'address_taken', '虚拟 IP 10.66.0.50 已经分给了节点 nas-01'),
    })
    renderWithApp(<AddNodeFlow network={NETWORK} />)
    await user.click(screen.getByRole('button', { name: t.addNode.generate }))
    expect(await screen.findByText(t.errors.codes.address_taken)).toBeVisible()
    expect(screen.getByRole('button', { name: t.addNode.generate })).toBeEnabled()
    expect(command()).toHaveTextContent(`echo ${placeholder()} | i3t join`)
  })

  it('节点用令牌加入之后，等待区自己变成结果；令牌不再显示', async () => {
    const user = userEvent.setup()
    const api = emptyNetwork()
    renderWithApp(<AddNodeFlow network={NETWORK} />)
    await generate(user)

    // 节点执行了加入命令：令牌用掉了，网络里多出一个节点。
    api.use({ 'GET /networks/1/tokens': [], 'GET /networks/1/nodes': [node(5, 'nas-01')] })
    expect(await screen.findByTestId('node-joined')).toHaveTextContent(t.addNode.joined('nas-01', '10.66.0.5'))
    // 名字取自它报的主机名，写出来，用户认得出是哪台机器。
    expect(screen.getByTestId('node-joined-hostname')).toHaveTextContent(t.addNode.joinedHostname('nas-01.lan'))
    expect(screen.queryByText(t.addNode.waiting)).not.toBeInTheDocument()
    expect(screen.queryByText(t.addNode.shownOnce)).not.toBeInTheDocument()
    expect(screen.queryByRole('button', { name: t.addNode.copyToken })).not.toBeInTheDocument()
    expect(screen.getByRole('link', { name: t.addNode.viewNode })).toHaveAttribute('href', '/n/1/nodes/5')
  })

  it('结果在最下面：节点加入时把它带进视野，免得对话框比屏幕高时看不到', async () => {
    const user = userEvent.setup()
    const api = emptyNetwork()
    const scroll = vi.spyOn(Element.prototype, 'scrollIntoView')
    renderWithApp(<AddNodeFlow network={NETWORK} />)
    await generate(user)
    // 还在等的时候不动：用户这时要看的是上面的命令。
    const before = scroll.mock.contexts.length

    api.use({ 'GET /networks/1/tokens': [], 'GET /networks/1/nodes': [node(5, 'nas-01')] })
    const result = await screen.findByTestId('node-joined')
    await waitFor(() => {
      const scrolled = scroll.mock.contexts.slice(before) as Element[]
      expect(scrolled.some((element) => element.contains(result))).toBe(true)
    })
  })

  it('"再添加一台"回到生成之前：令牌清掉，勾选保留', async () => {
    const user = userEvent.setup()
    const api = emptyNetwork()
    renderWithApp(<AddNodeFlow network={NETWORK} />)
    await user.click(screen.getByRole('checkbox', { name: containing(t.addNode.declareRelay) }))
    await generate(user)
    api.use({ 'GET /networks/1/tokens': [], 'GET /networks/1/nodes': [node(5, 'nas-01')] })
    await screen.findByTestId('node-joined')

    await user.click(screen.getByRole('button', { name: t.addNode.addAnother }))
    expect(command()).toHaveTextContent(`echo ${placeholder()} | i3t join --can-relay`)
    expect(document.body).not.toHaveTextContent(TOKEN)
    expect(screen.getByRole('button', { name: t.addNode.generate })).toBeEnabled()
  })

  it('网络里本来就有的节点不会被当成新加入的', async () => {
    const user = userEvent.setup()
    emptyNetwork({ 'GET /networks/1/nodes': [node(2, 'laptop')] })
    renderWithApp(<AddNodeFlow network={NETWORK} />)
    await generate(user)
    // 多等几轮轮询。
    await new Promise((done) => setTimeout(done, 150))
    expect(screen.queryByTestId('node-joined')).not.toBeInTheDocument()
    expect(screen.getByText(t.addNode.waiting)).toBeVisible()
  })

  it('令牌在别处被收回：不再等，给"重新生成"', async () => {
    const user = userEvent.setup()
    const api = emptyNetwork()
    renderWithApp(<AddNodeFlow network={NETWORK} />)
    await generate(user)
    api.use({ 'GET /networks/1/tokens': [] })
    expect(await screen.findByText(t.addNode.gone)).toBeVisible()
    expect(screen.queryByText(t.addNode.waiting)).not.toBeInTheDocument()
  })

  it('重新生成会先收回刚才那个令牌', async () => {
    const user = userEvent.setup()
    const api = emptyNetwork()
    renderWithApp(<AddNodeFlow network={NETWORK} />)
    await generate(user)

    api.use({
      'DELETE /networks/1/tokens/7': undefined,
      'POST /networks/1/tokens': () => {
        api.use({ 'GET /networks/1/tokens': [tokenInfo(8)] })
        return created(8, SECOND_TOKEN)
      },
    })
    await user.click(screen.getByRole('button', { name: t.addNode.regenerate }))
    await waitFor(() => expect(command()).toHaveTextContent(SECOND_TOKEN))
    expect(document.body).not.toHaveTextContent(TOKEN)

    const order = api.calls.filter((call) => call.method !== 'GET').map((call) => `${call.method} ${call.path}`)
    expect(order).toEqual(['POST /networks/1/tokens', 'DELETE /networks/1/tokens/7', 'POST /networks/1/tokens'])
  })
})

describe('"添加节点"对话框', () => {
  it('令牌还没用掉时点"完成"要先问一句；回答"继续等"则令牌还在', async () => {
    const user = userEvent.setup()
    emptyNetwork()
    const onClose = vi.fn()
    renderWithApp(<AddNodeDialog network={NETWORK} open onClose={onClose} />)
    await generate(user)

    await user.click(screen.getByRole('button', { name: t.addNode.done }))
    const question = await screen.findByRole('alertdialog', { name: t.addNode.closeWhileWaitingTitle })
    expect(question).toBeVisible()
    await user.click(screen.getByRole('button', { name: t.addNode.closeWhileWaitingCancel }))
    expect(onClose).not.toHaveBeenCalled()
    expect(command()).toHaveTextContent(TOKEN)

    await user.click(screen.getByRole('button', { name: t.addNode.done }))
    await user.click(await screen.findByRole('button', { name: t.addNode.closeWhileWaitingConfirm }))
    expect(onClose).toHaveBeenCalledOnce()
  })

  it('没生成令牌时点"完成"直接关', async () => {
    const user = userEvent.setup()
    emptyNetwork()
    const onClose = vi.fn()
    renderWithApp(<AddNodeDialog network={NETWORK} open onClose={onClose} />)
    await user.click(screen.getByRole('button', { name: t.addNode.done }))
    expect(onClose).toHaveBeenCalledOnce()
  })

  it('关闭时不收回令牌：命令可能已经复制走了', async () => {
    const user = userEvent.setup()
    const api = emptyNetwork()
    renderWithApp(<AddNodeDialog network={NETWORK} open onClose={() => undefined} />)
    await generate(user)
    await user.click(screen.getByRole('button', { name: t.addNode.done }))
    await user.click(await screen.findByRole('button', { name: t.addNode.closeWhileWaitingConfirm }))
    expect(api.calls.some((call) => call.method === 'DELETE')).toBe(false)
  })
})
