import { type ChildProcess, execFile, spawn } from 'node:child_process'
import { readFileSync } from 'node:fs'
import { createServer as createHttpServer, type Server as HttpServer, type ServerResponse } from 'node:http'
import { createServer as createTcpServer } from 'node:net'
import { join } from 'node:path'

import { expect, type Page, test } from '@playwright/test'

import { Controller, type NodeHandle } from './controller'
import { clearToasts, expectToast, goToAdmin, literal, signIn, t } from './helpers'

/**
 * 独占 TCP（lane）：三个真的跑着的节点，都在这台机器的回环地址上。
 *
 * - gw 启用了 relay，TCP 地址是它在回环上的节点端口；laptop 和 gw 这一对在界面上设成"总是用"：生效，laptop 拨号。
 * - 经 laptop 上的一条端口转发真的走流量（目标是 gw 上的一个 HTTP 服务）：一条一直开着的连接让"在用"不是 0，
 *   下载之后详情里有加密搬运的字节；节点详情里有和 gw 之间的那一行，连回节点对页。再改回"不用"。
 * - laptop 和 nas 都不是 relay：设置不起作用，界面按原因码说为什么，给出去启用 relay 的那一步。
 */

const NET = '/networks/1'
const MB = 1024 * 1024
const l = t.pairs.lane

interface ApiPair {
  a: number
  b: number
  keep_alive: boolean
  lane: string
  lane_effect: { effective: boolean; dialers: number[] } | null
  lanes: { node: number; lanes: { in_use: number; sealed_bytes: number } }[]
}

let controller: Controller
let page: Page
let http: HttpServer
let httpPort = 0
let forwardPort = 0
let hold: ChildProcess | undefined
/** 一直不结束的应答：结束测试时一起关掉。 */
const holding: ServerResponse[] = []
const nodes = {} as Record<'gw' | 'laptop' | 'nas', { handle: NodeHandle; id: number }>

function listen(server: HttpServer, host: string): Promise<number> {
  return new Promise((resolve, reject) => {
    server.once('error', reject)
    server.listen(0, host, () => {
      const address = server.address()
      if (address !== null && typeof address === 'object') resolve(address.port)
      else reject(new Error('拿不到端口'))
    })
  })
}

async function freePort(): Promise<number> {
  const server = createTcpServer()
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve))
  const address = server.address()
  await new Promise((resolve) => server.close(resolve))
  return address !== null && typeof address === 'object' ? address.port : 0
}

/** 节点的端口（UDP；relay 在同一个端口号上听 TCP），`Controller.run` 写在它的配置里。 */
function nodePort(node: NodeHandle): number {
  const match = /^port = (\d+)$/m.exec(readFileSync(join(node.dir, 'config.toml'), 'utf8'))
  return Number(match?.[1] ?? 0)
}

/** 经转发下载那个文件。异步地等：目标就在这个进程里。 */
function download(): Promise<number> {
  return new Promise((resolve) => {
    execFile(
      'curl',
      ['-s', '-m', '20', '-o', '/dev/null', '--noproxy', '*', `http://127.0.0.1:${forwardPort}/blob`],
      (error) => resolve(error === null ? 0 : typeof error.code === 'number' ? error.code : -1),
    )
  })
}

async function pair(a: number, b: number): Promise<ApiPair | undefined> {
  const list = await controller.api<ApiPair[]>('GET', `${NET}/pairs`)
  return list.find((item) => item.a === Math.min(a, b) && item.b === Math.max(a, b))
}

test.describe.configure({ mode: 'serial' })

test.beforeAll(async ({ browser }) => {
  test.setTimeout(120_000)
  const blob = Buffer.alloc(4 * MB, 'l')
  http = createHttpServer((request, response) => {
    if (request.url === '/hold') {
      // 一条一直开着的连接：它在的时候，经它的那条独占 TCP 一直在用。
      response.writeHead(200, { 'Content-Type': 'text/plain' })
      response.write('holding\n')
      holding.push(response)
      return
    }
    response.writeHead(200, { 'Content-Type': 'application/octet-stream', 'Content-Length': blob.length })
    response.end(blob)
  })
  httpPort = await listen(http, '127.0.0.1')
  controller = await Controller.start()
  page = await browser.newPage({ locale: 'zh-CN' })
})

test.afterAll(async () => {
  hold?.kill('SIGKILL')
  for (const response of holding) response.destroy()
  await page.close()
  controller.stop()
  http.close()
})

test('准备：gw 是 relay（TCP 地址在回环上），laptop 上一条到 gw 的端口转发；nas 不是 relay', async () => {
  test.setTimeout(120_000)
  await controller.api('POST', '/networks', { name: 'home' })
  const gw = await controller.enroll({ flags: ['--can-relay', '--can-forward'] })
  const laptop = await controller.enroll()
  const nas = await controller.enroll()
  nodes.gw = { handle: gw, id: 1 }
  nodes.laptop = { handle: laptop, id: 2 }
  nodes.nas = { handle: nas, id: 3 }
  for (const name of ['gw', 'laptop', 'nas'] as const) {
    await controller.api('PATCH', `${NET}/nodes/${nodes[name].id}`, { name })
    await controller.run(nodes[name].handle)
  }
  await controller.waitOnline(3)
  const port = nodePort(gw)
  await controller.api('PUT', `${NET}/nodes/${nodes.gw.id}/relay`, {
    udp: [`127.0.0.1:${port}`],
    tcp: [`127.0.0.1:${port}`],
  })

  forwardPort = await freePort()
  await controller.api('POST', `${NET}/forwards`, {
    name: 'blob',
    protocol: 'tcp',
    listen_node: nodes.laptop.id,
    bind: `127.0.0.1:${forwardPort}`,
    target_node: nodes.gw.id,
    target_addr: `127.0.0.1:${httpPort}`,
  })
  // 等转发真的通了（先走共用的连接）：之后再看独占 TCP。
  await expect.poll(download, { timeout: 40_000 }).toBe(0)
})

test('添加节点对，设成"总是用"：生效，laptop 拨号', async () => {
  await signIn(page, controller)
  await goToAdmin(page, t.nav.pairs)
  await page.getByRole('button', { name: t.pairs.add }).first().click()
  const dialog = page.getByRole('dialog', { name: t.pairs.dialogTitle })
  await dialog.getByRole('combobox', { name: t.pairs.nodeA }).click()
  await page.getByRole('option', { name: /10\.66\.0\.2(?!\d)/ }).click()
  await expect(page.getByRole('listbox')).toHaveCount(0)
  await dialog.getByRole('combobox', { name: t.pairs.nodeB }).click()
  await page.getByRole('option', { name: /10\.66\.0\.1(?!\d)/ }).click()
  await expect(page.getByRole('listbox')).toHaveCount(0)
  // 只要独占 TCP：连接常驻关掉。
  await dialog.getByRole('switch', { name: t.pairs.colKeepAlive }).click()
  await dialog.getByRole('radio', { name: l.modes.always }).click()
  await dialog.getByRole('button', { name: t.pairs.submit, exact: true }).click()
  await expectToast(page, t.pairs.added('laptop', 'gw'))

  const saved = await pair(nodes.gw.id, nodes.laptop.id)
  expect(saved).toMatchObject({ lane: 'always', lane_effect: { effective: true, dialers: [nodes.laptop.id] } })
  const row = page.getByTestId('pair-row-gw-laptop')
  await expect(row.getByRole('combobox', { name: l.select('gw', 'laptop') })).toHaveValue('always')
  await expect(row.getByTestId('lane-effect-gw-laptop')).toContainText(l.effective)
  await expect(row.getByTestId('lane-effect-gw-laptop')).toContainText(l.dials('laptop'))
  await expect(row.getByRole('switch', { name: t.pairs.toggle('gw', 'laptop') })).not.toBeChecked()
  await expect(page.getByText(l.routeNote)).toBeVisible()
  await clearToasts(page)
})

test('流量真的走独占 TCP：在用的连接、详情里的字节数；节点详情里有这一行', async () => {
  test.setTimeout(120_000)
  // 两个节点各自开好控制连接、报上来之后再开流：之前开的流按规矩改走共用的连接。
  await expect
    .poll(async () => (await pair(nodes.gw.id, nodes.laptop.id))?.lanes.length ?? 0, { timeout: 40_000 })
    .toBe(2)
  hold = spawn(
    'curl',
    ['-s', '-m', '110', '-o', '/dev/null', '--noproxy', '*', `http://127.0.0.1:${forwardPort}/hold`],
    {
      stdio: 'ignore',
    },
  )
  expect(await download()).toBe(0)

  // 节点每 10 秒上报一次。
  const inUse = page.getByTestId('lane-in-use-gw-laptop')
  await expect(inUse).toHaveText(/^[1-9]\d*$/, { timeout: 45_000 })
  await page.getByRole('button', { name: l.detailsFor('gw', 'laptop') }).click()
  const details = page.getByRole('table', { name: l.detailsFor('gw', 'laptop') })
  await expect(details.getByRole('columnheader', { name: 'gw', exact: true })).toBeVisible()
  await expect(details.getByRole('columnheader', { name: 'laptop', exact: true })).toBeVisible()
  const sealed = details.getByRole('row', { name: new RegExp(`^${literal(l.row.sealed)}`) })
  await expect(sealed).toContainText(/\d(\.\d)? MB/, { timeout: 45_000 })
  await expect(details.getByRole('row', { name: new RegExp(`^${literal(l.row.control)}`) })).toContainText(
    l.row.controlUp,
  )

  // 节点详情：和 gw 之间的那一行，本节点拨号，有流量；连回节点对页，那一对的详情已经展开。
  await page.goto(`${controller.baseUrl}/n/1/nodes/${nodes.laptop.id}?tab=lanes`)
  await expect(page.getByRole('tab', { name: t.nodes.detail.tabLanes })).toHaveAttribute('aria-selected', 'true')
  const lane = page.getByTestId(`node-lane-${nodes.gw.id}`)
  await expect(lane).toContainText(t.nodes.detail.lanesDials)
  await expect(lane).toContainText(l.row.controlUp)
  await expect(page.getByTestId(`node-lane-traffic-${nodes.gw.id}`)).toContainText(/MB/)
  await lane.getByRole('link', { name: t.nodes.detail.lanesPair }).click()
  await expect(page).toHaveURL(new RegExp(`/n/1/pairs\\?a=${nodes.laptop.id}&b=${nodes.gw.id}$`))
  await expect(page.getByRole('table', { name: l.detailsFor('gw', 'laptop') })).toBeVisible()
})

test('改回"不用"：只发这一项，状态不再显示', async () => {
  const row = page.getByTestId('pair-row-gw-laptop')
  await row.getByRole('combobox', { name: l.select('gw', 'laptop') }).selectOption('off')
  await expectToast(page, l.changed('gw', 'laptop', l.modes.off))
  await expect(row).not.toContainText(l.effective)
  expect(await pair(nodes.gw.id, nodes.laptop.id)).toMatchObject({ lane: 'off', lane_effect: null, keep_alive: false })
  hold?.kill('SIGKILL')
  hold = undefined
  await clearToasts(page)
})

test('两个节点都不是 relay：设置不起作用，说为什么，给出启用 relay 的那一步', async () => {
  await controller.api('PUT', `${NET}/pairs/${nodes.laptop.id}/${nodes.nas.id}`, { lane: 'udp_fallback' })
  await page.reload()
  const row = page.getByTestId('pair-row-laptop-nas')
  await expect(row.getByRole('combobox', { name: l.select('laptop', 'nas') })).toHaveValue('udp_fallback')
  await expect(row.getByTestId('lane-effect-laptop-nas')).toContainText(l.blocked)
  const blocked = page.getByTestId('lane-blocked-laptop-nas')
  await expect(blocked).toContainText(l.reason.noRelay('laptop', 'nas'))
  const fix = blocked.getByRole('link', { name: l.fix.enableRelay('laptop') })
  await expect(fix).toHaveAttribute('href', `/n/1/nodes/${nodes.laptop.id}?tab=settings`)
  await fix.click()
  await expect(page.getByTestId('capability-relay')).toBeVisible()
})
