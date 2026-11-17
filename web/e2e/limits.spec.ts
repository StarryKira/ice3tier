import { execFile } from 'node:child_process'
import { createServer as createHttpServer, type Server as HttpServer } from 'node:http'
import { createServer as createTcpServer, type Server as TcpServer } from 'node:net'
import { networkInterfaces } from 'node:os'

import { expect, type Page, test } from '@playwright/test'

import { Controller, type NodeHandle } from './controller'
import { clearToasts, expectToast, goTo, goToAdmin, literal, signIn, t } from './helpers'

/**
 * 限额、用量和状态：两个真的跑着的节点（gw 是出口和转发的目标，laptop 是入口），流量真的经它们走。
 *
 * - 凭据经一个要求凭据的监听器用过之后有用量；改上限让它用完，吊销，再彻底删掉。
 * - 出口配了配额，经它下载一个文件之后，出口页有本账期的用量。
 * - 端口写 0 的转发显示节点挑定的端口，并且真的能连；监听在被占用的端口上的转发显示"端口被占用"。
 * - 操作记录按操作筛，记录里的对象是链接。
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
  page = await browser.newPage({ locale: 'zh-CN' })
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
  const laptop = await controller.enroll()
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
  await goTo(page, t.nav.nodes)
  await expect(page.getByTestId(`node-row-${spare?.name ?? ''}`).getByTestId('never-connected')).toHaveText(
    t.nodes.never,
  )
})

test('出口配额：在节点详情里配上，下载之后出口页有本账期的用量', async () => {
  test.skip(lanIp === undefined, '这台机器没有私有 IPv4 地址，经出口访问局域网目标的测试做不了')
  test.setTimeout(90_000)
  await page.goto(`${controller.baseUrl}/n/1/nodes/${nodes.gw.id}?tab=exit`)
  const exitRow = page.getByTestId('capability-exit')
  const d = t.nodes.detail
  await exitRow.getByRole('switch', { name: d.exitSwitch('gw') }).click()
  const form = page.getByRole('dialog', { name: d.exitSwitch('gw') })
  await form.getByRole('textbox', { name: new RegExp(`^${literal(d.exitFormQuota)}`) }).fill('1')
  await form.getByRole('combobox', { name: d.exitFormQuotaUnit }).selectOption('GB')
  await form.getByRole('spinbutton', { name: d.exitFormResetDay }).fill('5')
  await form.getByRole('radio', { name: d.exitFormFamilyIpv4 }).click()
  await form.getByRole('switch', { name: new RegExp(`^${literal(d.exitFormLan)}`) }).click()
  await form.getByRole('button', { name: d.exitEnable }).click()
  // 打开"允许访问局域网"要确认一次。
  await page.getByRole('alertdialog').getByRole('button', { name: d.exitLanConfirm }).click()
  await expectToast(page, d.exitEnabledToast('gw'))
  await expect(exitRow).toContainText(d.rowQuotaValue('1.0 GB', 5))
  await expect(exitRow).toContainText(d.exitFamilyIpv4)
  const exits = await controller.api<{ settings: { quota_bytes: number; quota_reset_day: number; family: string } }[]>(
    'GET',
    `${NET}/exits`,
  )
  expect(exits[0]?.settings).toMatchObject({ quota_bytes: 1024 * MB, quota_reset_day: 5, family: 'ipv4' })
  await clearToasts(page)

  // laptop 上的程序经它本机的入口、用户名写 gw，下载一个文件：从 gw 出去。
  const done = await download(`socks5h://${nodes.gw.ip}@127.0.0.1:${nodes.laptop.handle.socksPort ?? 0}`)
  expect(done.code, done.stderr).toBe(0)

  await goTo(page, t.nav.exits)
  const usage = page.getByTestId('exit-usage-gw')
  const meter = usage.getByRole('meter', { name: t.exits.quotaLabel('gw') })
  await expect(meter).toBeVisible({ timeout: 40_000 })
  await expect(usage).toContainText(/MB \/ 1\.0 GB/, { timeout: 40_000 })
  await expect(usage).toContainText(t.exits.quotaResets('').trim())
  // 对照：出口报的和 laptop 看到的并排放着。
  await page.getByRole('button', { name: t.exits.detailsFor('gw') }).click()
  const table = page.getByRole('table', { name: t.exits.detailsFor('gw') })
  await expect(table.getByRole('columnheader', { name: t.exits.exitReported, exact: true })).toBeVisible()
  await expect(table.getByRole('columnheader', { name: 'laptop', exact: true })).toBeVisible({ timeout: 30_000 })
  await expect(page.getByTestId('exit-health-gw')).toBeVisible()
})

test('凭据：经要求凭据的监听器用过之后有用量；改上限让它用完；吊销；再彻底删掉', async () => {
  test.skip(lanIp === undefined, '这台机器没有私有 IPv4 地址，经出口访问局域网目标的测试做不了')
  test.setTimeout(120_000)
  const c = t.credentials
  await goTo(page, t.nav.credentials)
  await page.getByRole('button', { name: c.issue }).first().click()
  const form = page.getByRole('dialog', { name: c.createTitle })
  await form.getByLabel(c.nameLabel, { exact: true }).fill('phone')
  await form.getByRole('button', { name: c.submit, exact: true }).click()
  const secretDialog = page.getByTestId('secret-dialog')
  const secret = ((await secretDialog.getByTestId('secret-value').textContent()) ?? '').trim()
  await secretDialog.getByRole('button', { name: c.savedClose }).click()
  const row = page.getByTestId('credential-row-phone')
  await expect(row).toContainText(c.state.active)
  await expect(row).toContainText(`${secret.slice(0, 6)}••••`)
  await expect(row).toContainText(c.neverUsed)
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
  await goTo(page, t.nav.listeners)
  await expect(page.getByTestId('listener-state-lan')).toContainText(t.listen.listening, { timeout: 30_000 })
  // 节点自带的回环入口也列出来。
  await expect(page.locator('[data-testid^="builtin-row-laptop-"]').first()).toBeVisible()
  const done = await download(`socks5h://${nodes.gw.ip}:${secret}@127.0.0.1:${port}`)
  expect(done.code, done.stderr).toBe(0)

  // 入口每 10 秒报一次用量。
  await goTo(page, t.nav.credentials)
  await expect(row).toContainText(/\d(\.\d)? MB/, { timeout: 45_000 })
  await expect(row).not.toContainText(c.neverUsed)

  // 改上限，不用重新新建：比已经用掉的还少，保存之前就说它会马上停用。
  await page.getByRole('button', { name: t.common.actionsFor('phone') }).click()
  await page.getByRole('menuitem', { name: c.menuEdit }).click()
  const limits = page.getByRole('dialog', { name: c.editTitle('phone') })
  await limits.getByRole('textbox', { name: new RegExp(`^${literal(c.maxBytesLabel)}`) }).fill('1')
  await limits.getByRole('combobox', { name: c.unitLabel }).selectOption('MB')
  await expect(limits.getByText(c.editWillExhaust)).toBeVisible()
  await limits.getByRole('button', { name: c.editSubmit }).click()
  await expectToast(page, c.edited('phone'))
  await expect(row).toContainText(c.state.exhausted)
  await expect(page.getByText(c.state.exhaustedDetail)).toBeVisible()
  // 入口收到新的上限之后真的拒绝它。
  await expect
    .poll(async () => (await download(`socks5h://${nodes.gw.ip}:${secret}@127.0.0.1:${port}`)).code, {
      timeout: 45_000,
    })
    .not.toBe(0)
  await clearToasts(page)

  // 吊销：危险确认，行留着，看得到用过多少。
  await page.getByRole('button', { name: t.common.actionsFor('phone') }).click()
  await page.getByRole('menuitem', { name: c.menuRevoke }).click()
  const revoke = page.getByRole('alertdialog', { name: c.revokeTitle('phone') })
  await expect(revoke.getByRole('button', { name: t.common.cancel })).toBeFocused()
  await revoke.getByRole('button', { name: c.revokeConfirm }).click()
  await expectToast(page, c.revoked('phone'))
  await expect(row).toContainText(c.state.revoked)
  await expect(row).toContainText(/MB/)
  const listed = await controller.api<{ state: string }[]>('GET', `${NET}/credentials`)
  expect(listed[0]?.state).toBe('revoked')
  await clearToasts(page)

  // 再删一次才彻底删掉。
  await page.getByRole('button', { name: t.common.actionsFor('phone') }).click()
  await expect(page.getByRole('menuitem', { name: c.menuRevoke })).toHaveCount(0)
  await page.getByRole('menuitem', { name: c.menuRemove }).click()
  await page
    .getByRole('alertdialog', { name: c.removeTitle('phone') })
    .getByRole('button', { name: c.removeConfirm })
    .click()
  await expectToast(page, c.removed('phone'))
  await expect(row).toHaveCount(0)
  expect(await controller.api<unknown[]>('GET', `${NET}/credentials`)).toHaveLength(0)
  await clearToasts(page)
})

test('端口写 0 的转发：节点挑一个端口，列表里写出来，真的能连', async () => {
  test.setTimeout(90_000)
  await goTo(page, t.nav.forwards)
  const f = t.forwards
  const d = f.dialog
  await page.getByRole('button', { name: f.create }).first().click()
  const dialog = page.getByRole('dialog', { name: d.createTitle })
  const listenSide = dialog.getByRole('group', { name: d.listenSide })
  const targetSide = dialog.getByRole('group', { name: d.targetSide })
  await listenSide.getByRole('combobox', { name: d.node }).click()
  await page.getByRole('option', { name: /10\.66\.0\.2(?!\d)/ }).click()
  await expect(page.getByRole('listbox')).toHaveCount(0)
  await targetSide.getByRole('combobox', { name: d.node }).click()
  await page.getByRole('option', { name: /10\.66\.0\.1(?!\d)/ }).click()
  await expect(listenSide.getByRole('textbox', { name: d.port })).toHaveAttribute('placeholder', d.portPlaceholder)
  await targetSide.getByRole('textbox', { name: d.targetPort }).fill(String(httpPort))
  await dialog.getByLabel(new RegExp(`^${literal(d.name)}`)).fill('picked')
  await dialog.getByRole('button', { name: d.submitCreate }).click()
  await expectToast(page, d.createdPicked('laptop'))

  const [forward] = await controller.api<{ id: number; bind: string }[]>('GET', `${NET}/forwards`)
  expect(forward?.bind).toBe('127.0.0.1:0')
  const picked = page.getByTestId(`forward-picked-${forward?.id ?? 0}`)
  // "节点挑的端口 N"：把端口那一段换成数字的模式，从文字里取出端口。
  const pickedText = new RegExp(literal(f.picked(65535)).replace('65535', '(\\d+)'))
  await expect(picked).toContainText(pickedText, { timeout: 40_000 })
  const port = Number(pickedText.exec((await picked.textContent()) ?? '')?.[1])
  expect(port).toBeGreaterThan(0)
  // 经这个端口真的连到了 gw 上的服务。
  const result = await curl(['-s', '-m', '10', '--noproxy', '*', `http://127.0.0.1:${port}/hello`])
  expect(result.stdout).toBe('hello from the target\n')
  await expect(page.getByTestId('forward-row-picked')).toContainText(f.status.active)
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
  const detail = t.listen.portInUseDetail('laptop', String(busyPort))
  await expect(row).toContainText(t.listen.portInUse, { timeout: 40_000 })
  await expect(page.getByText(detail)).toBeVisible()
  // 节点详情的"监听"里同样看得到，连同自带的回环入口。
  await page.goto(`${controller.baseUrl}/n/1/nodes/${nodes.laptop.id}?tab=listen`)
  await expect(page.getByTestId('node-listen-local-socks5')).toContainText(t.listen.listening)
  await expect(page.getByText(detail)).toBeVisible()
})

test('操作记录：按操作筛，记录里的对象是链接；点过去对象已经删掉了会说', async () => {
  await goToAdmin(page, t.nav.settings)
  const l = t.settings.log
  const credentialRef = (id: number) => l.refLabels.credential(id)
  await page.getByRole('link', { name: t.settings.tabLog }).click()
  await page.getByRole('combobox', { name: l.filterAction }).selectOption('credential')
  await expect(page).toHaveURL(/action=credential/)
  test.skip(lanIp === undefined, '前面关于凭据的测试被跳过了，没有可以点的记录')
  const rows = page.locator('[data-testid^="log-row-"]')
  // 签发、改上限、吊销、删除（可能还有一条"用完了"），都是凭据的，新的在前。
  await expect(rows.first()).toContainText(l.actions['credential.delete'])
  await expect(rows.last()).toContainText(l.actions['credential.create'])
  for (const text of await rows.allTextContents()) expect(text).toContain(credentialRef(credentialId))

  // 只看和这个凭据有关的。
  await rows
    .first()
    .getByRole('button', { name: l.filterByRef(credentialRef(credentialId)) })
    .click()
  await expect(page).toHaveURL(new RegExp(`ref=credential%3A${credentialId}`))
  await expect(page.getByTestId('log-ref-filter')).toContainText(l.refChip(credentialRef(credentialId)))

  // 记录里的节点写名字，点了去节点详情。
  await page.getByRole('combobox', { name: l.filterAction }).selectOption('')
  await page.getByRole('button', { name: l.refRemove(credentialRef(credentialId)) }).click()
  await page.getByRole('combobox', { name: l.filterAction }).selectOption('node.exit.enable')
  await page
    .getByRole('link', { name: l.refLabels.node(nodes.gw.id, 'gw') })
    .first()
    .click()
  await expect(page).toHaveURL(new RegExp(`/n/1/nodes/${nodes.gw.id}$`))

  // 点一个已经彻底删掉的凭据：列表页上说一句。
  await page.goto(`${controller.baseUrl}/settings/log?action=credential.create`)
  await page.getByRole('link', { name: credentialRef(credentialId) }).click()
  await expect(page).toHaveURL(new RegExp(`/n/1/credentials\\?focus=${credentialId}`))
  await expect(page.getByText(t.credentials.focusMissing(credentialId))).toBeVisible()
})
