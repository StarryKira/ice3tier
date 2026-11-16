import { expect, type Locator, type Page, test } from '@playwright/test'

import { Controller, type NodeHandle } from './controller'
import { clearToasts, expectToast, goTo, goToAdmin } from './helpers'

/**
 * 从一个空的 controller 开始，把主要的流程照着用户的做法走一遍：
 * 登录 → 建网络 → 生成注册令牌、拿到加入命令 → 节点加入 → 打标签 → 加规则、关系图跟着变 →
 * 启用出口（节点没有声明时怎么说、声明之后怎么变）→ 签发凭据（只显示一次）→ 建、删端口转发 → 退出。
 *
 * 都是真的：真的 controller，界面给出的那一行加入命令原样交给 shell 执行，节点是真的 `i3t`。
 * 界面上做完一件事，再用管理 API 核对它真的落到了 controller 上。
 */

interface ApiNode {
  id: number
  name: string
  virtual_ip: string
  tags: string[]
  online: boolean
  declared: { exit: boolean; relay: boolean; forward: boolean }
  exit: { ports: number[] | null } | null
}

const NET = '/networks/1'

let controller: Controller
let page: Page

/** 两个节点：`server` 是带 servers 标签、声明了可以作为转发目标的那个；`laptop` 是后加入的。 */
let server: { handle: NodeHandle; id: number; name: string; ip: string }
let laptop: { handle: NodeHandle; id: number; name: string; ip: string }

async function apiNodes(): Promise<ApiNode[]> {
  return controller.api<ApiNode[]>('GET', `${NET}/nodes`)
}

function joinCommand(): Locator {
  return page.getByTestId('join-command')
}

/** 关系图上的一条规则线。画图的库把线画成带说明文字的一组 SVG。 */
function ruleEdge(label: string, state: '启用' | '停用'): Locator {
  return page.getByTestId('tag-graph').locator(`[aria-label="规则 ${label}（${state}）"]`)
}

/**
 * 下拉里的某个节点。按虚拟 IP 认，不按名字：节点的名字取自主机名，同一台机器上加入的几个节点会叫
 * `host`、`host-2`，一个是另一个的开头。
 */
function nodeOption(node: { ip: string }): Locator {
  return page.getByRole('option', { name: new RegExp(`${node.ip.replaceAll('.', '\\.')}(?!\\d)`) })
}

function tagPoint(tag: string, members: number): Locator {
  return page.getByTestId('tag-graph').locator(`[aria-label="标签 ${tag}，${members} 个节点"]`)
}

test.describe.configure({ mode: 'serial' })

test.beforeAll(async ({ browser }) => {
  test.setTimeout(120_000)
  controller = await Controller.start()
  console.log(`用的程序：${controller.bins}`)
  const context = await browser.newContext()
  await context.grantPermissions(['clipboard-read', 'clipboard-write'])
  page = await context.newPage()
})

test.afterAll(async () => {
  await page.context().close()
  controller.stop()
})

test('用管理令牌登录', async () => {
  // 没登录时任何地址都落到登录页。
  await page.goto(`${controller.baseUrl}/settings/about`)
  await expect(page).toHaveURL(/\/login\?next=/)
  await expect(page.getByRole('heading', { name: '登录 ice3tier' })).toBeVisible()

  const token = page.getByLabel('管理令牌', { exact: true })
  await token.fill('not-the-admin-token')
  await page.getByRole('button', { name: '登录', exact: true }).click()
  await expect(page.getByRole('alert')).toContainText('管理令牌不对')
  await expect(page).toHaveURL(/\/login/)

  await token.fill(controller.adminToken)
  await page.getByRole('button', { name: '登录', exact: true }).click()
  // controller 里还没有网络：先去建第一个。
  await expect(page.getByRole('heading', { name: '建第一个网络' })).toBeVisible()
})

test('建第一个网络', async () => {
  const name = page.getByLabel('名字', { exact: true })
  const cidr = page.getByLabel('虚拟地址段', { exact: true })
  // 两项都有默认值。
  await expect(name).toHaveValue('home')
  await expect(cidr).toHaveValue('10.66.0.0/16')
  await expect(page.getByText('有节点加入之后，地址段只能扩大，不能更换。')).toBeVisible()

  // 写法不对的，在输入框下面就说，不发请求。
  await name.fill('My Network')
  await expect(page.getByText('名字只能用字母、数字和连字符（-）。')).toBeVisible()
  await cidr.fill('10.66.0.0/31')
  await expect(page.getByText('地址段太小了，前缀最长是 /30。')).toBeVisible()
  await page.getByRole('button', { name: '创建网络' }).click()
  expect(await controller.api<unknown[]>('GET', '/networks')).toHaveLength(0)

  await name.fill('home')
  await cidr.fill('10.66.0.0/16')
  await page.getByRole('button', { name: '创建网络' }).click()

  // 落在总览页；节点不足两个时它就是引导。
  await expect(page.getByRole('heading', { name: '把前两个节点连起来' })).toBeVisible()
  await expect(page).toHaveURL(/\/n\/1\/overview$/)
  const networks = await controller.api<{ name: string; cidr: string }[]>('GET', '/networks')
  expect(networks).toMatchObject([{ name: 'home', cidr: '10.66.0.0/16' }])
  // 新网络自带默认规则。
  const rules = await controller.api<{ src: string; dst: string; enabled: boolean }[]>('GET', `${NET}/rules`)
  expect(rules).toMatchObject([{ src: 'all', dst: 'all', enabled: true }])
  await clearToasts(page)
})

test('生成注册令牌，拿到加入命令，节点加入后出现在界面上', async () => {
  await goTo(page, '节点')
  await expect(page.getByText('这个网络里还没有节点')).toBeVisible()
  await page.getByRole('button', { name: '添加节点' }).click()
  const dialog = page.getByRole('dialog', { name: '添加节点' })
  await expect(dialog).toBeVisible()

  // 生成之前：命令里是占位符，没有东西可以复制，controller 上也没有令牌。
  await expect(joinCommand()).toHaveText('echo <注册令牌> | i3t join')
  await expect(dialog.getByRole('button', { name: '复制命令' })).toBeDisabled()
  expect(await controller.api<unknown[]>('GET', `${NET}/tokens`)).toHaveLength(0)

  // 标签写在令牌上；"可以承担"是命令的参数。
  await dialog.getByRole('combobox', { name: '标签' }).fill('servers')
  await page.keyboard.press('Enter')
  await dialog.getByRole('checkbox', { name: /可以作为端口转发的目标/ }).check()
  await expect(joinCommand()).toHaveText('echo <注册令牌> | i3t join --can-forward')

  await dialog.getByRole('button', { name: '生成注册令牌' }).click()
  await expect(joinCommand()).toHaveText(/^echo i3t-[A-Za-z0-9_-]+ \| i3t join --can-forward$/)
  await expect(dialog.getByText('关闭这个对话框后不再显示。')).toBeVisible()
  await expect(dialog.getByText('等待节点加入…')).toBeVisible()
  const tokens = await controller.api<{ tags: string[] }[]>('GET', `${NET}/tokens`)
  expect(tokens).toMatchObject([{ tags: ['servers'] }])

  // 复制出去的就是屏幕上的那一行。
  await dialog.getByRole('button', { name: '复制命令' }).click()
  const command = (await joinCommand().textContent()) ?? ''
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(command)

  // 在"那台机器"上执行它。
  const handle = controller.join(command)

  // 不用刷新：等待区自己变成结果。
  await expect(page.getByTestId('node-joined')).toContainText('已加入 · 10.66.0.1')
  const [joined] = await apiNodes()
  if (joined === undefined) throw new Error('controller 上没有节点')
  expect(joined).toMatchObject({
    virtual_ip: '10.66.0.1',
    tags: ['servers'],
    declared: { exit: false, relay: false, forward: true },
  })
  server = { handle, id: joined.id, name: joined.name, ip: joined.virtual_ip }
  await expect(page.getByTestId('node-joined')).toContainText(server.name)
  // 令牌只能用一次。
  expect(await controller.api<unknown[]>('GET', `${NET}/tokens`)).toHaveLength(0)

  await dialog.getByRole('button', { name: '完成' }).click()
  await expect(dialog).toBeHidden()
  const row = page.getByTestId(`node-row-${server.name}`)
  await expect(row).toBeVisible()
  await expect(row).toContainText('10.66.0.1')
  await expect(row).toContainText('servers')
})

test('在「注册」页新建令牌：关掉对话框后令牌留在列表里，用掉之后消失', async () => {
  await goToAdmin(page, '注册')
  await expect(page.getByText('没有未使用的令牌')).toBeVisible()
  await page.getByRole('button', { name: '添加节点' }).click()
  const dialog = page.getByRole('dialog', { name: '添加节点' })
  await dialog.getByRole('button', { name: '生成注册令牌' }).click()
  await expect(joinCommand()).toHaveText(/^echo i3t-[A-Za-z0-9_-]+ \| i3t join$/)
  const command = (await joinCommand().textContent()) ?? ''
  const secret = command.split(' ')[1] ?? ''

  // 令牌还没用掉：按 Esc 关不掉，点"完成"要先问一句。
  await page.keyboard.press('Escape')
  await expect(dialog).toBeVisible()
  await dialog.getByRole('button', { name: '完成' }).click()
  const question = page.getByRole('alertdialog', { name: '令牌还没用掉，现在关闭？' })
  await expect(question).toBeVisible()
  await question.getByRole('button', { name: '关闭', exact: true }).click()
  await expect(dialog).toBeHidden()

  // 列表里有这个令牌，但它的文本哪里都查不回来。
  const [pending] = await controller.api<{ id: number }[]>('GET', `${NET}/tokens`)
  if (pending === undefined) throw new Error('controller 上没有令牌')
  const row = page.getByTestId(`token-row-${pending.id}`)
  await expect(row).toContainText('未使用')
  await expect(page.getByText(secret)).toHaveCount(0)

  // 复制走的命令在关掉对话框之后仍然有效。
  const handle = controller.join(command)
  const joined = (await apiNodes()).find((node) => node.id !== server.id)
  if (joined === undefined) throw new Error('第二个节点没有加入')
  laptop = { handle, id: joined.id, name: joined.name, ip: joined.virtual_ip }

  // 这个节点是在界面之外加入的（对话框已经关了）：节点列表靠轮询发现它，周期是 10 秒。
  await goTo(page, '节点')
  await expect(page.getByTestId(`node-row-${laptop.name}`)).toBeVisible({ timeout: 20_000 })
  await goToAdmin(page, '注册')
  await expect(page.getByText('没有未使用的令牌')).toBeVisible()
})

test('给节点打标签', async () => {
  await goTo(page, '节点')
  const row = page.getByTestId(`node-row-${laptop.name}`)
  await expect(row.getByText('添加标签')).toBeVisible()
  await row.getByRole('button', { name: `编辑 ${laptop.name} 的标签` }).click()

  const dialog = page.getByRole('dialog', { name: `${laptop.name} 的标签` })
  const input = dialog.getByRole('combobox')
  // 内置的 all 不能打在节点上。
  await input.fill('all')
  await page.keyboard.press('Enter')
  await expect(dialog.getByText('all 是内置标签，每个节点都有，不用打。')).toBeVisible()
  // 写法不对的标签在这里就拦下。
  await input.fill('my.tag')
  await page.keyboard.press('Enter')
  await expect(dialog.getByText('标签只能用字母、数字、连字符（-）和下划线（_）。')).toBeVisible()

  // 改了立即生效，不用点保存。大写转成小写。
  await input.fill('Laptops')
  await page.keyboard.press('Enter')
  await expectToast(page, '标签已更新')
  await expect(dialog.getByRole('button', { name: '移除标签 laptops' })).toBeVisible()
  await dialog.getByRole('button', { name: '完成' }).click()

  await expect(row).toContainText('laptops')
  const node = (await apiNodes()).find((candidate) => candidate.id === laptop.id)
  expect(node?.tags).toEqual(['laptops'])
  await clearToasts(page)

  // 按标签筛选：只剩它。筛选写在地址里。
  await page.getByRole('button', { name: '筛选' }).click()
  await page.getByRole('checkbox', { name: 'laptops' }).check()
  await page.keyboard.press('Escape')
  await expect(page.getByTestId(`node-row-${server.name}`)).toBeHidden()
  await expect(row).toBeVisible()
  await expect(page).toHaveURL(/[?&]tag=laptops/)
  await page.getByRole('button', { name: '清除全部' }).click()
  await expect(page.getByTestId(`node-row-${server.name}`)).toBeVisible()
})

test('加一条规则，关系图跟着变；停用默认规则后按标签收紧', async () => {
  await goTo(page, '访问控制')
  await expect(page.getByTestId('tag-graph')).toBeVisible()
  // 现在：三个标签，一条默认规则画成 all 上的一个环。
  await expect(tagPoint('all', 2)).toBeVisible()
  await expect(tagPoint('servers', 1)).toBeVisible()
  await expect(tagPoint('laptops', 1)).toBeVisible()
  await expect(ruleEdge('default', '启用')).toHaveCount(1)
  await expect(page.getByTestId('tag-graph').locator('.react-flow__edge')).toHaveCount(1)

  await page.getByRole('button', { name: '新建规则' }).click()
  const dialog = page.getByRole('dialog', { name: '新建规则' })
  await dialog.getByRole('combobox', { name: '标签 a' }).click()
  await page.getByRole('option', { name: /laptops/ }).click()
  await dialog.getByRole('combobox', { name: '标签 b' }).click()
  await page.getByRole('option', { name: /servers/ }).click()
  // 一句白话，随输入变化。
  await expect(page.getByTestId('rule-sentence')).toContainText(
    '带 laptops 标签的 1 个节点，和带 servers 标签的 1 个节点，可以互相访问。',
  )
  await dialog.getByRole('radio', { name: '只能 a 访问 b' }).click()
  await expect(page.getByTestId('rule-sentence')).toContainText(
    '带 laptops 标签的 1 个节点可以访问带 servers 标签的 1 个节点。反过来不行。',
  )
  // 默认规则还开着，所以这条规则现在不改变谁能访问谁。
  await expect(page.getByTestId('rule-impact')).toContainText('保存后谁能访问谁不变。')
  await dialog.getByLabel(/^名字/).fill('ssh')
  await dialog.getByRole('button', { name: '保存规则' }).click()
  await expectToast(page, '规则 ssh 已建好')
  await expect(dialog).toBeHidden()

  // 列表里多了一行，图上多了一条线。
  const rules = await controller.api<{ id: number; name: string; src: string; dst: string; direction: string }[]>(
    'GET',
    `${NET}/rules`,
  )
  const ssh = rules.find((rule) => rule.name === 'ssh')
  expect(ssh).toMatchObject({ src: 'laptops', dst: 'servers', direction: 'one_way' })
  await expect(page.getByTestId(`rule-row-${ssh?.id ?? 0}`)).toBeVisible()
  await expect(ruleEdge('ssh', '启用')).toHaveCount(1)
  await expect(page.getByTestId('tag-graph').locator('.react-flow__edge')).toHaveCount(2)
  // 界面上说出来：默认规则开着时别的规则不起限制作用。
  await expect(
    page.getByText('all ⇄ all 开着时所有节点都互通，其他规则不起限制作用。要按标签收紧，先停用它。'),
  ).toBeVisible()
  await clearToasts(page)

  // 停用默认规则。还有 ssh 连着这两个节点，但少了一个方向，所以要确认。
  await page.getByRole('switch', { name: '启用规则 default' }).click()
  const question = page.getByRole('alertdialog', { name: '停用规则 default？' })
  await expect(question).toContainText('之后有 1 对节点不能再访问')
  await question.getByRole('button', { name: '停用规则' }).click()
  await expectToast(page, '规则 default 已停用')
  await expect(ruleEdge('default', '停用')).toHaveCount(1)
  await expect(ruleEdge('default', '启用')).toHaveCount(0)
  await expect(page.getByText('all ⇄ all 开着时所有节点都互通')).toHaveCount(0)
  await clearToasts(page)

  // 访问检查用文字说出同一件事：单向。
  await page.getByRole('button', { name: '访问检查' }).click()
  const check = page.getByTestId('access-check')
  await check.getByRole('combobox', { name: '从' }).click()
  await nodeOption(laptop).click()
  await check.getByRole('combobox', { name: '到' }).click()
  await nodeOption(server).click()
  await expect(page.getByTestId(`check-${laptop.name}-${server.name}`)).toContainText('可以。')
  await expect(page.getByTestId(`check-${laptop.name}-${server.name}`)).toContainText('由规则 ssh 允许。')
  await expect(page.getByTestId(`check-${server.name}-${laptop.name}`)).toContainText('不可以。')
  await page.getByRole('button', { name: '收起访问检查' }).click()

  // 按节点看：laptop 能访问 server，没有谁能访问 laptop。
  await page.getByRole('radio', { name: '按节点' }).click()
  await page.getByRole('combobox', { name: '焦点节点' }).click()
  await nodeOption(laptop).click()
  const nodeGraph = page.getByTestId('node-graph')
  await expect(nodeGraph.getByText(laptop.name, { exact: true })).toBeVisible()
  await expect(nodeGraph.getByText(server.name, { exact: true })).toHaveCount(1)
  await page.getByRole('radio', { name: '按标签' }).click()
})

test('启用出口：节点没有声明时说明原因和要执行的命令，声明一到开关自己变为可用', async () => {
  // 出口页：没有出口，也没有节点声明过。给出要执行的命令。
  await goTo(page, '出口')
  await expect(page.getByText('还没有出口，也没有节点声明可以作为出口')).toBeVisible()
  await expect(page.getByTestId('declare-command')).toHaveText('i3t set can-exit on')

  // "启用出口"里没有声明的节点照样列出来，只是不能选。
  await page.getByRole('button', { name: '启用出口' }).click()
  const dialog = page.getByRole('dialog', { name: '启用出口' })
  await expect(dialog.getByText('还没有声明，现在启用不了')).toBeVisible()
  await expect(dialog.getByText(server.name, { exact: true })).toBeVisible()
  await expect(dialog.getByRole('button', { name: '启用出口' })).toBeDisabled()
  await dialog.getByRole('button', { name: '取消', exact: true }).click()

  // controller 自己也不答应（界面上的置灰不是唯一的一道关）。
  const refused = await fetch(`${controller.baseUrl}/api/v1${NET}/nodes/${server.id}/exit`, {
    method: 'PUT',
    headers: { Authorization: `Bearer ${controller.adminToken}`, 'Content-Type': 'application/json' },
    body: '{}',
  })
  expect(refused.status).toBe(409)
  expect(((await refused.json()) as { error: { code: string } }).error.code).toBe('not_declared')

  // 节点详情页：开关是灰的，旁边写着原因；"怎么声明"点开是要在那台机器上执行的命令。
  await goTo(page, '节点')
  await page.getByRole('link', { name: server.name, exact: true }).click()
  const exitRow = page.getByTestId('capability-exit')
  const exitSwitch = exitRow.getByRole('switch', { name: `把 ${server.name} 启用为出口` })
  await expect(exitSwitch).toBeDisabled()
  await expect(page.getByTestId('not-declared-exit')).toContainText(
    `${server.name} 没有声明可以作为出口，所以这里启用不了。`,
  )
  await exitRow.getByRole('button', { name: '怎么声明' }).click()
  await expect(exitRow.getByText(`在 ${server.name} 上执行：`)).toBeVisible()
  await expect(exitRow.getByTestId('declare-command')).toHaveText('i3t set can-exit on')
  await expect(exitRow.getByText('正在等这台机器上报声明…')).toBeVisible()

  // 在那台机器上照着做：声明，然后把 i3t 跑起来。
  controller.set(server.handle, 'can-exit', true)
  await controller.run(server.handle)

  // 不用刷新：声明一到，说明自己收起，开关变为可用。
  await expect(exitSwitch).toBeEnabled({ timeout: 30_000 })
  await expect(page.getByTestId('not-declared-exit')).toHaveCount(0)
  await expect(exitRow.getByTestId('declare-command')).toHaveCount(0)

  // 打开：就地出现几项限制，都有默认值。
  await exitSwitch.click()
  await expect(exitRow.getByRole('textbox', { name: '允许的目标端口' })).toHaveValue('80, 443')
  await exitRow.getByRole('button', { name: '启用出口' }).click()
  await expectToast(page, `${server.name} 已启用为出口`)
  await expect(exitSwitch).toBeChecked()
  // 启用之后给出一个马上能用的例子，地址是这个节点真实的虚拟 IP。
  await expect(exitRow.getByText(`curl -x socks5h://${server.ip}@127.0.0.1:1080 https://example.com`)).toBeVisible()
  const node = (await apiNodes()).find((candidate) => candidate.id === server.id)
  expect(node?.exit).toMatchObject({ ports: [80, 443] })
  await clearToasts(page)

  // 出口页现在列出它。
  await goTo(page, '出口')
  await expect(page.getByTestId(`exit-row-${server.name}`)).toBeVisible()
})

test('签发凭据：令牌只显示一次', async () => {
  await goTo(page, '接入凭据')
  await expect(page.getByText('还没有凭据')).toBeVisible()
  await page.getByRole('button', { name: '签发凭据' }).click()
  const form = page.getByRole('dialog', { name: '签发凭据' })

  // 名字不能是空的。
  await form.getByRole('button', { name: '签发', exact: true }).click()
  await expect(form.getByText('请填一个名字。')).toBeVisible()
  expect(await controller.api<unknown[]>('GET', `${NET}/credentials`)).toHaveLength(0)

  await form.getByLabel('名字', { exact: true }).fill('phone')
  await form.getByLabel(/^并发上限/).fill('4')
  await form.getByRole('button', { name: '签发', exact: true }).click()

  const secretDialog = page.getByTestId('secret-dialog')
  await expect(secretDialog).toBeVisible()
  await expect(secretDialog.getByRole('heading', { name: '凭据 phone 已签发' })).toBeVisible()
  await expect(secretDialog.getByText('它不会再显示。')).toBeVisible()
  const secret = ((await secretDialog.getByTestId('secret-value').textContent()) ?? '').trim()
  expect(secret.length).toBeGreaterThan(20)

  // 复制出去的是完整的令牌。
  await secretDialog.getByRole('button', { name: '复制凭据' }).click()
  await expect(secretDialog.getByRole('button', { name: '已复制' })).toBeVisible()
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(secret)

  // 按 Esc 关不掉，只能点按钮。
  await page.keyboard.press('Escape')
  await expect(secretDialog).toBeVisible()
  await secretDialog.getByRole('button', { name: '我已保存，关闭' }).click()
  await expect(secretDialog).toBeHidden()

  // 之后哪里都看不到它：列表里只有名字，刷新也一样，接口也不返回。
  const row = page.getByTestId('credential-row-phone')
  await expect(row).toBeVisible()
  await expect(row).toContainText('4')
  await expect(page.getByText(secret)).toHaveCount(0)
  await page.reload()
  await expect(page.getByTestId('credential-row-phone')).toBeVisible()
  await expect(page.getByText(secret)).toHaveCount(0)
  const listed = await controller.api<Record<string, unknown>[]>('GET', `${NET}/credentials`)
  expect(listed).toHaveLength(1)
  expect(listed[0]).toMatchObject({ name: 'phone', max_concurrent: 4 })
  expect(JSON.stringify(listed)).not.toContain(secret)

  // 重名的，controller 的话写在名字下面。
  await page.getByRole('button', { name: '签发凭据' }).click()
  await form.getByLabel('名字', { exact: true }).fill('phone')
  await form.getByRole('button', { name: '签发', exact: true }).click()
  await expect(form.getByRole('alert')).toBeVisible()
  await expect(page.getByTestId('secret-dialog')).toHaveCount(0)
  await form.getByRole('button', { name: '取消', exact: true }).click()
  await page.getByRole('button', { name: '放弃改动' }).click()
  await expect(form).toBeHidden()
})

test('新建端口转发，再删除', async () => {
  await goTo(page, '端口转发')
  await expect(page.getByText('还没有端口转发')).toBeVisible()
  await page.getByRole('button', { name: '新建转发' }).first().click()
  const dialog = page.getByRole('dialog', { name: '新建转发' })
  const listenSide = dialog.getByRole('group', { name: '在哪里监听' })
  const targetSide = dialog.getByRole('group', { name: '转发到哪里' })

  await listenSide.getByRole('combobox', { name: '节点' }).click()
  await nodeOption(laptop).click()
  // 等第一个下拉收起来再开第二个：收起的动画期间两个列表同时在页面上，选项会认重。
  await expect(page.getByRole('listbox')).toHaveCount(0)
  await targetSide.getByRole('combobox', { name: '节点' }).click()
  await nodeOption(server).click()
  // 即时检查：转发不绕过访问规则。laptop → server 由刚才那条规则允许。
  await expect(page.getByTestId('forward-check')).toContainText(`${laptop.name} 可以访问 ${server.name}（规则 ssh）。`)

  // 端口写错了，在输入框下面说。
  await listenSide.getByLabel('端口', { exact: true }).fill('70000')
  await targetSide.getByLabel('端口', { exact: true }).fill('80')
  await dialog.getByLabel(/^名字/).fill('web')
  await dialog.getByRole('button', { name: '新建转发' }).click()
  await expect(listenSide.getByText('端口是 1 到 65535 之间的数字。')).toBeVisible()
  expect(await controller.api<unknown[]>('GET', `${NET}/forwards`)).toHaveLength(0)

  await listenSide.getByLabel('端口', { exact: true }).fill('8080')
  await dialog.getByRole('button', { name: '新建转发' }).click()
  await expect(dialog).toBeHidden()
  await expectToast(page, /转发已建好/)

  const row = page.getByTestId('forward-row-web')
  await expect(row).toBeVisible()
  await expect(row).toContainText(laptop.name)
  await expect(row).toContainText('127.0.0.1:8080')
  await expect(row).toContainText(server.name)
  await expect(row).toContainText('127.0.0.1:80')
  await expect(row).toContainText('TCP')
  const forwards = await controller.api<Record<string, unknown>[]>('GET', `${NET}/forwards`)
  expect(forwards).toMatchObject([
    {
      name: 'web',
      protocol: 'tcp',
      listen_node: laptop.id,
      bind: '127.0.0.1:8080',
      target_node: server.id,
      target_addr: '127.0.0.1:80',
      enabled: true,
    },
  ])
  await clearToasts(page)

  // 节点详情的"关联配置"里也看得到它。
  await row.getByRole('link', { name: server.name, exact: true }).click()
  await page.getByRole('tab', { name: '关联配置' }).click()
  await expect(page.getByText('127.0.0.1:8080')).toBeVisible()

  // 删除要确认，确认框里写明后果。
  await goTo(page, '端口转发')
  await page.getByRole('button', { name: 'web 的操作' }).click()
  await page.getByRole('menuitem', { name: '删除…' }).click()
  const question = page.getByRole('alertdialog', { name: '删除转发 web？' })
  await expect(question).toContainText(`${laptop.name} 上的 127.0.0.1:8080 不再监听`)
  // 先取消：什么都没变。
  await question.getByRole('button', { name: '取消', exact: true }).click()
  await expect(page.getByTestId('forward-row-web')).toBeVisible()
  expect(await controller.api<unknown[]>('GET', `${NET}/forwards`)).toHaveLength(1)

  await page.getByRole('button', { name: 'web 的操作' }).click()
  await page.getByRole('menuitem', { name: '删除…' }).click()
  await question.getByRole('button', { name: '删除转发' }).click()
  await expectToast(page, '转发 web 已删除')
  await expect(page.getByTestId('forward-row-web')).toHaveCount(0)
  await expect(page.getByText('还没有端口转发')).toBeVisible()
  expect(await controller.api<unknown[]>('GET', `${NET}/forwards`)).toHaveLength(0)
  await clearToasts(page)
})

test('做过的事都记在操作记录里；总览反映现在的状态', async () => {
  await goTo(page, '总览')
  // 两个节点了，总览不再是引导。
  await expect(page.getByTestId('overview-status')).toBeVisible()
  await expect(page.getByRole('heading', { name: '把前两个节点连起来' })).toHaveCount(0)

  await goToAdmin(page, '设置')
  await page.getByRole('link', { name: '操作记录' }).click()
  await expect(page.getByRole('columnheader', { name: '说明' })).toBeVisible()
  const log = await controller.api<{ action: string }[]>('GET', '/log')
  const actions = new Set(log.map((entry) => entry.action))
  // 界面上做的每一类修改，controller 都留了记录。
  expect(log.length).toBeGreaterThanOrEqual(8)
  expect([...actions].some((action) => action.startsWith('network.'))).toBe(true)
  expect([...actions].some((action) => action.startsWith('rule.'))).toBe(true)
  expect([...actions].some((action) => action.startsWith('forward.'))).toBe(true)
  await expect(page.getByRole('row')).not.toHaveCount(1)
})

test('刷新后还在登录状态；退出后回到登录页，要重新登录', async () => {
  await page.reload()
  await expect(page.getByRole('columnheader', { name: '说明' })).toBeVisible()

  await page.getByRole('button', { name: '账号菜单' }).click()
  await page.getByRole('menuitem', { name: '退出登录' }).click()
  await expect(page.getByRole('heading', { name: '登录 ice3tier' })).toBeVisible()
  await expect(page.getByText('已退出登录。')).toBeVisible()

  // 退出之后直接打开里面的页面，还是落到登录页。
  await page.goto(`${controller.baseUrl}/n/1/nodes`)
  await expect(page).toHaveURL(/\/login\?next=%2Fn%2F1%2Fnodes/)
  await page.getByLabel('管理令牌', { exact: true }).fill(controller.adminToken)
  await page.getByRole('button', { name: '登录', exact: true }).click()
  // 登录后回到原来要去的地址。
  await expect(page).toHaveURL(/\/n\/1\/nodes$/)
  await expect(page.getByTestId(`node-row-${server.name}`)).toBeVisible()
})
