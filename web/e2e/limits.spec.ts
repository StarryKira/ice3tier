import { execFile } from 'node:child_process'
import { createServer as createHttpServer, type Server as HttpServer } from 'node:http'
import { createServer as createTcpServer, type Server as TcpServer } from 'node:net'
import { networkInterfaces } from 'node:os'

import { expect, type Page, test } from '@playwright/test'

import { Controller, type NodeHandle } from './controller'
import { clearToasts, expectToast, goTo, goToAdmin, signIn } from './helpers'

/**
 * 限额、用量和状态：两个真的跑着的节点（gw 是出口和转发的目标，laptop 是入口），流量真的经它们走。
 *
 * - 凭据经一个要求凭据的监听器用过之后有用量；改上限让它用完，吊销，再彻底删掉。
 * - 出口配了配额，经它下载一个文件之后，出口页有本账期的用量。
 * - 端口写 0 的转发显示节点挑定的端口，并且真的能连；监听在被占用的端口上的转发显示"端口被占用"。
 * - 操作记录按操作筛，记录里的对象是链接；标签改名。
 *
 * 下载的目标是这台机器上一个临时的 HTTP 服务，监听在它的局域网地址上：出口把它当局域网目标，不连外网。
 */

const NET = '/networks/1'
const MB = 1024 * 1024
const BLOB = 4 * MB

interface ApiNode {
  id: number
  name: string
  virtual_ip: string
  tags: string[]
  last_seen_at: number | null
}

let controller: Controller
let page: Page
let http: HttpServer
let busy: TcpServer
let httpPort = 0
let busyPort = 0
/** 这台机器的一个私有 IPv4 地址：出口打开"允许访问本机局域网"后，经出口访问它算局域网目标。没有时跳过要它的测试。 */
let lanIp: string | undefined
const nodes = {} as Record<'gw' | 'laptop', { handle: NodeHandle; id: number; ip: string }>
let credentialId = 0

function privateIpv4(): string | undefined {
  const candidates = Object.values(networkInterfaces())
    .flat()
    .filter((address) => address !== undefined && address.family === 'IPv4' && !address.internal)
    .map((address) => address?.address ?? '')
  const isPrivate = (ip: string) =>
    ip.startsWith('192.168.') ||
    /^172\.(1[6-9]|2\d|3[01])\./.test(ip) ||
    (ip.startsWith('10.') && !ip.startsWith('10.66.'))
  return candidates.find((ip) => ip.startsWith('192.168.')) ?? candidates.find(isPrivate)
}

function listen(server: HttpServer | TcpServer, host: string): Promise<number> {
  return new Promise((resolve, reject) => {
    server.once('error', reject)
    server.listen(0, host, () => {
      const address = server.address()
      if (address !== null && typeof address === 'object') resolve(address.port)
      else reject(new Error('拿不到端口'))
    })
  })
}

/** 一个现在空闲的端口：占一下就放开。 */
async function freePort(): Promise<number> {
  const server = createTcpServer()
  const port = await listen(server, '127.0.0.1')
  await new Promise((resolve) => server.close(resolve))
  return port
}

/**
 * 执行 curl，等它结束。不能用同步的写法：下载的目标（HTTP 服务）就在这个进程里，同步地等 curl 会把它也堵住。
 */
function curl(args: string[]): Promise<{ code: number; stdout: string; stderr: string }> {
  return new Promise((resolve) => {
    execFile('curl', args, { encoding: 'utf8' }, (error, stdout, stderr) => {
      const code = error === null ? 0 : typeof error.code === 'number' ? error.code : -1
      resolve({ code, stdout, stderr })
    })
  })
}

/** 用 curl 经一个 SOCKS5 入口下载那个文件。失败时 `stderr` 里是 curl 说的原因（SOCKS5 的应答码等）。 */
async function download(proxy: string): Promise<{ code: number; stderr: string }> {
  if (lanIp === undefined) throw new Error('没有局域网地址')
  return curl(['-sS', '-m', '20', '-o', '/dev/null', '-x', proxy, `http://${lanIp}:${httpPort}/blob`])
}

test.describe.configure({ mode: 'serial' })

test.beforeAll(async ({ browser }) => {
  test.setTimeout(120_000)
  lanIp = privateIpv4()
  const blob = Buffer.alloc(BLOB, 'i')
  http = createHttpServer((request, response) => {
    if (request.url === '/blob') {
      response.writeHead(200, { 'Content-Type': 'application/octet-stream', 'Content-Length': BLOB })
      response.end(blob)
    } else {
      response.writeHead(200, { 'Content-Type': 'text/plain' })
      response.end('hello from the target\n')
    }
  })
  httpPort = await listen(http, '0.0.0.0')
  // 一个占着端口的"别的程序"：转发要监听在它上面时，节点会报端口被占用。
  busy = createTcpServer()
  busyPort = await listen(busy, '127.0.0.1')

  controller = await Controller.start()
  page = await browser.newPage()
})

test.afterAll(async () => {
  await page.close()
  controller.stop()
  http.close()
  busy.close()
})

test('准备：出口 gw 和入口 laptop 跑起来；还有一个只注册过、从没跑起来的节点', async () => {
  test.setTimeout(120_000)
  await controller.api('POST', '/networks', { name: 'home' })
  const gw = await controller.enroll({ flags: ['--can-exit', '--can-forward'] })
  const laptop = await controller.enroll({ tags: ['laptops'] })
  await controller.api('PATCH', `${NET}/nodes/1`, { name: 'gw' })
  await controller.api('PATCH', `${NET}/nodes/2`, { name: 'laptop' })
  nodes.gw = { handle: gw, id: 1, ip: '10.66.0.1' }
  nodes.laptop = { handle: laptop, id: 2, ip: '10.66.0.2' }
  await controller.run(gw)
  await controller.run(laptop)
  await controller.waitOnline(2)
  // 只用 i3t join 注册过、没有跑起来的节点：从来没有真的连上过。
  await controller.enroll()
  const listed = await controller.api<ApiNode[]>('GET', `${NET}/nodes`)
  const spare = listed.find((node) => node.id === 3)
  expect(spare?.last_seen_at).toBeNull()

  await signIn(page, controller)
  await goTo(page, '节点')
  await expect(page.getByTestId(`node-row-${spare?.name ?? ''}`).getByTestId('never-connected')).toHaveText('从未连接')
})

test('标签改名：整个网络里一起改', async () => {
  await goTo(page, '访问控制')
  await page.getByRole('button', { name: '展开标签 laptops 的成员' }).click()
  await page.getByRole('button', { name: '给标签 laptops 改名' }).click()
  const dialog = page.getByRole('dialog', { name: '给标签 laptops 改名' })
  await expect(dialog.getByText('现在有 1 个节点带着它，0 条规则用到它。')).toBeVisible()
  await dialog.getByRole('textbox', { name: '新名字' }).fill('notebooks')
  await expect(dialog.getByText('新名字原来没有人用：谁能访问谁不变。')).toBeVisible()
  await dialog.getByRole('button', { name: '改名', exact: true }).click()
  await expectToast(page, /标签 laptops 已改名为 notebooks/)
  await expect(dialog).toBeHidden()
  const listed = await controller.api<ApiNode[]>('GET', `${NET}/nodes`)
  expect(listed.find((node) => node.id === nodes.laptop.id)?.tags).toEqual(['notebooks'])
  await expect(page.getByRole('button', { name: '收起标签 notebooks 的成员' })).toBeVisible()
  await clearToasts(page)
})

test('出口配额：在节点详情里配上，下载之后出口页有本账期的用量', async () => {
  test.skip(lanIp === undefined, '这台机器没有私有 IPv4 地址，经出口访问局域网目标的测试做不了')
  test.setTimeout(90_000)
  await page.goto(`${controller.baseUrl}/n/1/nodes/${nodes.gw.id}`)
  const exitRow = page.getByTestId('capability-exit')
  await exitRow.getByRole('switch', { name: '把 gw 启用为出口' }).click()
  await exitRow.getByRole('textbox', { name: /^流量配额/ }).fill('1')
  await exitRow.getByRole('combobox', { name: '流量配额的单位' }).selectOption('GB')
  await exitRow.getByRole('spinbutton', { name: '账期从每月几号开始' }).fill('5')
  await exitRow.getByRole('radio', { name: '只连 IPv4' }).click()
  await exitRow.getByRole('switch', { name: /^允许访问它所在的局域网/ }).click()
  await exitRow.getByRole('button', { name: '启用出口' }).click()
  // 打开"允许访问局域网"要确认一次。
  await page.getByRole('alertdialog').getByRole('button', { name: '允许访问局域网' }).click()
  await expectToast(page, 'gw 已启用为出口')
  await expect(exitRow).toContainText('配额 1.0 GB/月（每月 5 日起）')
  await expect(exitRow).toContainText('只连 IPv4')
  const exits = await controller.api<{ settings: { quota_bytes: number; quota_reset_day: number; family: string } }[]>(
    'GET',
    `${NET}/exits`,
  )
  expect(exits[0]?.settings).toMatchObject({ quota_bytes: 1024 * MB, quota_reset_day: 5, family: 'ipv4' })
  await clearToasts(page)

  // laptop 上的程序经它本机的入口、用户名写 gw，下载一个文件：从 gw 出去。
  const done = await download(`socks5h://${nodes.gw.ip}@127.0.0.1:${nodes.laptop.handle.socksPort ?? 0}`)
  expect(done.code, done.stderr).toBe(0)

  await goTo(page, '出口')
  const usage = page.getByTestId('exit-usage-gw')
  const meter = usage.getByRole('meter', { name: '出口 gw 本账期的配额' })
  await expect(meter).toBeVisible({ timeout: 40_000 })
  await expect(usage).toContainText(/MB \/ 1\.0 GB/, { timeout: 40_000 })
  await expect(usage).toContainText(/日重置/)
  // 对照：出口报的和 laptop 看到的并排放着。
  await page.getByRole('button', { name: '出口 gw 的计数和用它的节点看到的' }).click()
  const table = page.getByRole('table', { name: '出口 gw 的计数和用它的节点看到的' })
  await expect(table.getByRole('columnheader', { name: '出口自己报的' })).toBeVisible()
  await expect(table.getByRole('columnheader', { name: 'laptop 看到的' })).toBeVisible({ timeout: 30_000 })
  await expect(page.getByTestId('exit-health-gw')).toBeVisible()
})

test('凭据：经要求凭据的监听器用过之后有用量；改上限让它用完；吊销；再彻底删掉', async () => {
  test.skip(lanIp === undefined, '这台机器没有私有 IPv4 地址，经出口访问局域网目标的测试做不了')
  test.setTimeout(120_000)
  await goTo(page, '接入凭据')
  await page.getByRole('button', { name: '签发凭据' }).first().click()
  const form = page.getByRole('dialog', { name: '签发凭据' })
  await form.getByLabel('名字', { exact: true }).fill('phone')
  await form.getByRole('button', { name: '签发', exact: true }).click()
  const secretDialog = page.getByTestId('secret-dialog')
  const secret = ((await secretDialog.getByTestId('secret-value').textContent()) ?? '').trim()
  await secretDialog.getByRole('button', { name: '我已保存，关闭' }).click()
  const row = page.getByTestId('credential-row-phone')
  await expect(row).toContainText('有效')
  await expect(row).toContainText(`${secret.slice(0, 6)}••••`)
  await expect(row).toContainText('从未使用')
  credentialId = (await controller.api<{ id: number }[]>('GET', `${NET}/credentials`))[0]?.id ?? 0

  // 一个对外的、要求凭据的监听器，开在 laptop 上；只收本机来的连接。
  const port = await freePort()
  await controller.api('POST', `${NET}/listeners`, {
    node: nodes.laptop.id,
    name: 'lan',
    bind: `0.0.0.0:${port}`,
    protocol: 'socks5',
    allow_from: ['127.0.0.0/8'],
    require_credential: true,
  })
  // 等它监听上，再用凭据经它下载。
  await goTo(page, '监听器')
  await expect(page.getByTestId('listener-state-lan')).toContainText('正在监听', { timeout: 30_000 })
  // 节点自带的回环入口也列出来。
  await expect(page.locator('[data-testid^="builtin-row-laptop-"]').first()).toBeVisible()
  const done = await download(`socks5h://${nodes.gw.ip}:${secret}@127.0.0.1:${port}`)
  expect(done.code, done.stderr).toBe(0)

  // 入口每 10 秒报一次用量。
  await goTo(page, '接入凭据')
  await expect(row).toContainText(/\d(\.\d)? MB/, { timeout: 45_000 })
  await expect(row).not.toContainText('从未使用')

  // 改上限，不用重新签发：比已经用掉的还少，保存之前就说它会马上停用。
  await page.getByRole('button', { name: 'phone 的操作' }).click()
  await page.getByRole('menuitem', { name: /修改上限/ }).click()
  const limits = page.getByRole('dialog', { name: '修改凭据 phone 的上限' })
  await limits.getByRole('textbox', { name: /^流量上限/ }).fill('1')
  await limits.getByRole('combobox', { name: '流量上限的单位' }).selectOption('MB')
  await expect(limits.getByText('新的流量上限比已经用掉的还少')).toBeVisible()
  await limits.getByRole('button', { name: '保存上限' }).click()
  await expectToast(page, /凭据 phone 的上限已保存/)
  await expect(row).toContainText('已用完')
  await expect(page.getByText(/流量上限用完了：入口拒绝它的新连接/)).toBeVisible()
  // 入口收到新的上限之后真的拒绝它。
  await expect
    .poll(async () => (await download(`socks5h://${nodes.gw.ip}:${secret}@127.0.0.1:${port}`)).code, {
      timeout: 45_000,
    })
    .not.toBe(0)
  await clearToasts(page)

  // 吊销：危险确认，行留着，看得到用过多少。
  await page.getByRole('button', { name: 'phone 的操作' }).click()
  await page.getByRole('menuitem', { name: '吊销…' }).click()
  const revoke = page.getByRole('alertdialog', { name: '吊销凭据 phone？' })
  await expect(revoke.getByRole('button', { name: '取消' })).toBeFocused()
  await revoke.getByRole('button', { name: '吊销凭据' }).click()
  await expectToast(page, '凭据 phone 已吊销')
  await expect(row).toContainText('已吊销')
  await expect(row).toContainText(/MB/)
  const listed = await controller.api<{ state: string }[]>('GET', `${NET}/credentials`)
  expect(listed[0]?.state).toBe('revoked')
  await clearToasts(page)

  // 再删一次才彻底删掉。
  await page.getByRole('button', { name: 'phone 的操作' }).click()
  await expect(page.getByRole('menuitem', { name: '吊销…' })).toHaveCount(0)
  await page.getByRole('menuitem', { name: '删除记录…' }).click()
  await page
    .getByRole('alertdialog', { name: '彻底删掉凭据 phone？' })
    .getByRole('button', { name: '删除记录' })
    .click()
  await expectToast(page, '凭据 phone 的记录已删除')
  await expect(row).toHaveCount(0)
  expect(await controller.api<unknown[]>('GET', `${NET}/credentials`)).toHaveLength(0)
  await clearToasts(page)
})

test('端口写 0 的转发：节点挑一个端口，列表里写出来，真的能连', async () => {
  test.setTimeout(90_000)
  await goTo(page, '端口转发')
  await page.getByRole('button', { name: '新建转发' }).first().click()
  const dialog = page.getByRole('dialog', { name: '新建转发' })
  const listenSide = dialog.getByRole('group', { name: '在哪里监听' })
  const targetSide = dialog.getByRole('group', { name: '转发到哪里' })
  await listenSide.getByRole('combobox', { name: '节点' }).click()
  await page.getByRole('option', { name: /10\.66\.0\.2(?!\d)/ }).click()
  await expect(page.getByRole('listbox')).toHaveCount(0)
  await targetSide.getByRole('combobox', { name: '节点' }).click()
  await page.getByRole('option', { name: /10\.66\.0\.1(?!\d)/ }).click()
  await expect(listenSide.getByRole('textbox', { name: '端口' })).toHaveAttribute('placeholder', '留空自动选')
  await targetSide.getByRole('textbox', { name: '端口' }).fill(String(httpPort))
  await dialog.getByLabel(/^名字/).fill('picked')
  await dialog.getByRole('button', { name: '新建转发' }).click()
  await expectToast(page, /挑定端口之后，地址显示在列表里/)

  const [forward] = await controller.api<{ id: number; bind: string }[]>('GET', `${NET}/forwards`)
  expect(forward?.bind).toBe('127.0.0.1:0')
  const picked = page.getByTestId(`forward-picked-${forward?.id ?? 0}`)
  await expect(picked).toContainText(/节点挑的端口 \d+/, { timeout: 40_000 })
  const port = Number(/节点挑的端口 (\d+)/.exec((await picked.textContent()) ?? '')?.[1])
  expect(port).toBeGreaterThan(0)
  // 经这个端口真的连到了 gw 上的服务。
  const result = await curl(['-s', '-m', '10', '--noproxy', '*', `http://127.0.0.1:${port}/hello`])
  expect(result.stdout).toBe('hello from the target\n')
  await expect(page.getByTestId('forward-row-picked')).toContainText('正常')
  await clearToasts(page)
})

test('监听在被占用的端口上的转发：显示端口被占用、是哪个端口、怎么办', async () => {
  test.setTimeout(90_000)
  await controller.api('POST', `${NET}/forwards`, {
    name: 'busy',
    protocol: 'tcp',
    listen_node: nodes.laptop.id,
    bind: `127.0.0.1:${busyPort}`,
    target_node: nodes.gw.id,
    target_addr: `127.0.0.1:${httpPort}`,
  })
  const row = page.getByTestId('forward-row-busy')
  await expect(row).toContainText('端口被占用', { timeout: 40_000 })
  await expect(page.getByText(new RegExp(`端口被占用：laptop 上 ${busyPort} 已经有别的程序在监听`))).toBeVisible()
  // 节点详情的"监听"里同样看得到，连同自带的回环入口。
  await page.goto(`${controller.baseUrl}/n/1/nodes/${nodes.laptop.id}?tab=listen`)
  await expect(page.getByTestId('node-listen-local-socks5')).toContainText('正在监听')
  await expect(page.getByText(new RegExp(`端口被占用：laptop 上 ${busyPort}`))).toBeVisible()
})

test('操作记录：按操作筛，记录里的对象是链接；点过去对象已经删掉了会说', async () => {
  await goToAdmin(page, '设置')
  await page.getByRole('link', { name: '操作记录' }).click()
  await page.getByRole('combobox', { name: '按操作筛选' }).selectOption('credential')
  await expect(page).toHaveURL(/action=credential/)
  test.skip(lanIp === undefined, '前面关于凭据的测试被跳过了，没有可以点的记录')
  const rows = page.locator('[data-testid^="log-row-"]')
  // 签发、改上限、吊销、删除（可能还有一条"用完了"），都是凭据的，新的在前。
  await expect(rows.first()).toContainText('删除凭据的记录')
  await expect(rows.last()).toContainText('签发凭据')
  for (const text of await rows.allTextContents()) expect(text).toMatch(/凭据/)

  // 只看和这个凭据有关的。
  await rows
    .first()
    .getByRole('button', { name: `只看和凭据 #${credentialId}有关的记录` })
    .click()
  await expect(page).toHaveURL(new RegExp(`ref=credential%3A${credentialId}`))
  await expect(page.getByTestId('log-ref-filter')).toContainText(`对象：凭据 #${credentialId}`)

  // 记录里的节点写名字，点了去节点详情。
  await page.getByRole('combobox', { name: '按操作筛选' }).selectOption('')
  await page.getByRole('button', { name: `去掉筛选：凭据 #${credentialId}` }).click()
  await page.getByRole('combobox', { name: '按操作筛选' }).selectOption('node.exit.enable')
  await page.getByRole('link', { name: '节点 gw' }).first().click()
  await expect(page).toHaveURL(new RegExp(`/n/1/nodes/${nodes.gw.id}$`))

  // 点一个已经彻底删掉的凭据：列表页上说一句。
  await page.goto(`${controller.baseUrl}/settings/log?action=credential.create`)
  await page.getByRole('link', { name: `凭据 #${credentialId}` }).click()
  await expect(page).toHaveURL(new RegExp(`/n/1/credentials\\?focus=${credentialId}`))
  await expect(page.getByText(`操作记录里说的凭据 #${credentialId} 已经不在了`)).toBeVisible()
})
