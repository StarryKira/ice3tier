import { chmodSync, mkdirSync, rmSync, writeFileSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

import { expect, type Page, test } from '@playwright/test'

import { Controller, type NodeHandle } from './controller'
import { clearToasts, goTo, goToAdmin, signIn } from './helpers'
import { type ReleaseKit, releaseKit, ReleaseSource } from './release'

/**
 * 把每个页面在"没有数据"和"有数据"时的样子截下来，放到 web/screenshots/，给人看版面有没有问题。
 * 数据都是真的：真的 controller，真的节点用真的令牌加入，其中三个真的跑着；
 * 发布源是一个本机目录，里面有一个用测试钥匙签的新版本（要先 `pnpm e2e:kit`）。
 */

const outDir = resolve(dirname(fileURLToPath(import.meta.url)), '..', 'screenshots')

let controller: Controller
let page: Page
let kit: ReleaseKit
let release: ReleaseSource
let counter = 0
/** 用界面或接口加进来的节点，按用途起了名字。 */
const nodes: Record<string, NodeHandle> = {}

async function shot(name: string, options: { fullPage?: boolean } = {}): Promise<void> {
  counter += 1
  await page.waitForLoadState('networkidle')
  // 整页截图要从页面顶端开始截：滚动过的话，粘在视口上的侧栏和顶栏会画在半中间。
  if (options.fullPage ?? true) await page.evaluate(() => window.scrollTo(0, 0))
  // 等进场动画走完，免得截到半透明的对话框。
  await page.waitForTimeout(250)
  await page.screenshot({
    path: join(outDir, `${String(counter).padStart(2, '0')}-${name}.png`),
    fullPage: options.fullPage ?? true,
  })
}

test.describe.configure({ mode: 'serial' })

test.beforeAll(async ({ browser }) => {
  test.setTimeout(120_000)
  // 每次都从空目录开始：改过名字或者删掉的截图不会留下旧的一份。
  rmSync(outDir, { recursive: true, force: true })
  mkdirSync(outDir, { recursive: true })
  kit = releaseKit()
  controller = await Controller.start({ bins: kit.oldBins })
  page = await browser.newPage()
})

test.afterAll(async () => {
  await page.close()
  controller.stop()
})

test('登录和第一个网络', async () => {
  await page.goto(`${controller.baseUrl}/`)
  await expect(page.getByRole('heading', { name: '登录 ice3tier' })).toBeVisible()
  await shot('sign-in')

  await page.getByLabel('管理令牌', { exact: true }).fill('not-the-token')
  await page.getByRole('button', { name: '登录', exact: true }).click()
  await expect(page.getByText('管理令牌不对')).toBeVisible()
  await shot('sign-in-wrong-token')

  await signIn(page, controller)
  await expect(page.getByRole('heading', { name: '建第一个网络' })).toBeVisible()
  await shot('welcome-first-network')

  await page.getByRole('button', { name: '创建网络' }).click()
  await expect(page.getByRole('heading', { name: '把前两个节点连起来' })).toBeVisible()
})

test('空的网络：每个页面没有数据时的样子', async () => {
  await clearToasts(page)
  await shot('overview-guide-empty')

  await goTo(page, '节点')
  await expect(page.getByText('这个网络里还没有节点')).toBeVisible()
  await shot('nodes-empty')

  await page.getByRole('button', { name: '添加节点' }).click()
  await expect(page.getByRole('dialog', { name: '添加节点' })).toBeVisible()
  await shot('add-node-before-token')
  await page.getByRole('button', { name: '完成' }).click()

  await goTo(page, '访问控制')
  await expect(page.getByTestId('tag-graph')).toBeVisible()
  await shot('access-default-rule-only')

  for (const [name, file, marker] of [
    ['出口', 'exits-empty', '还没有出口'],
    ['监听器', 'listeners-empty', '还没有通过 controller 配置的监听器'],
    ['接入凭据', 'credentials-empty', '还没有凭据'],
    ['端口转发', 'forwards-empty', '还没有端口转发'],
  ] as const) {
    await goTo(page, name)
    await expect(page.getByText(marker).first()).toBeVisible()
    await shot(file)
  }
  for (const [name, file, marker] of [
    ['注册', 'tokens-empty', '没有未使用的令牌'],
    ['节点对', 'pairs-empty', '所有节点对都在用默认设置'],
  ] as const) {
    await goToAdmin(page, name)
    await expect(page.getByText(marker).first()).toBeVisible()
    await shot(file)
  }
})

test('添加节点：令牌、命令、节点加入', async () => {
  await goTo(page, '总览')
  await page.getByRole('checkbox', { name: /可以作为 relay/ }).check()
  await page.getByRole('button', { name: '生成注册令牌' }).click()
  const command = page.getByTestId('join-command')
  await expect(command).toContainText('echo i3t-')
  await shot('overview-guide-token-generated')

  const first = controller.join((await command.textContent()) ?? '')
  // 节点的名字取自主机名（取不出来才叫 node-<编号>），所以不按名字认。
  await expect(page.locator('[data-testid^="guide-node-"]')).toBeVisible()
  await shot('overview-guide-one-node')

  // 第一个节点跑起来；后面再准备其余的。
  await controller.run(first)
  nodes.gw = first
})

test('准备数据', async () => {
  test.setTimeout(180_000)
  const net = '/networks/1'
  // gw-hk 是第一个节点，已经声明了 relay；再补上出口和转发目标的声明。声明在节点重启后才上报，所以这里在它启动之前就改好了的不算，
  // 直接用加入时的参数准备其余几个。
  nodes.nas = await controller.enroll({ tags: ['servers', 'home'], flags: ['--can-forward', '--can-exit'] })
  nodes.laptop = await controller.enroll({ tags: ['laptops'] })
  nodes.office = await controller.enroll({ tags: ['laptops'] })
  nodes.spare = await controller.enroll()
  // nas 以为自己在容器里：它不能自己更新，界面上要写出做法。
  await controller.run(nodes.nas, { env: { container: 'podman' } })
  await controller.run(nodes.laptop)
  await controller.waitOnline(3)

  // 发布源上有一个新版本。自动更新关着：截图的时候节点不要自己换程序重启。
  await controller.api('PATCH', '/update', { auto_update: false })
  release = new ReleaseSource(join(controller.dir, 'releases'), kit)
  release.publish(kit.newVersion, kit.newProgram)
  await controller.api('PATCH', '/update', { source: release.root })
  await controller.api('POST', '/update/check')

  await controller.api('PATCH', `${net}/nodes/1`, { name: 'gw-hk', tags: ['servers'] })
  await controller.api('PATCH', `${net}/nodes/2`, { name: 'nas-home' })
  await controller.api('PATCH', `${net}/nodes/3`, { name: 'laptop', pinned_relays: [] })
  await controller.api('PATCH', `${net}/nodes/4`, { name: 'office-pc', routing: 'lowest_cost' })

  await controller.api('PUT', `${net}/nodes/1/relay`, {
    udp: ['203.0.113.7:20330'],
    tcp: ['203.0.113.7:20330'],
    region: 'hk',
    bandwidth_mbps: 200,
  })
  await controller.api('PUT', `${net}/nodes/2/exit`, { ports: [80, 443], lan: true })
  await controller.api('PATCH', `${net}/nodes/3`, { pinned_relays: [1] })

  await controller.api('POST', `${net}/rules`, { name: 'ssh', src: 'laptops', dst: 'servers', direction: 'one_way' })
  await controller.api('POST', `${net}/rules`, { name: '', src: 'servers', dst: 'servers', direction: 'both' })
  await controller.api('POST', `${net}/rules`, {
    name: 'media',
    src: 'laptops',
    dst: 'home',
    direction: 'both',
    enabled: false,
  })

  await controller.api('POST', `${net}/credentials`, { name: 'phone', max_concurrent: 8 })
  await controller.api('POST', `${net}/credentials`, { name: 'tablet' })

  await controller.api('POST', `${net}/listeners`, {
    node: 1,
    name: 'lan',
    bind: '192.168.1.5:1080',
    protocol: 'socks5',
    allow_from: ['192.168.1.0/24'],
    require_credential: true,
    default_action: { action: 'exit', node: 2 },
  })
  await controller.api('POST', `${net}/listeners`, {
    node: 3,
    name: 'fixed-nas',
    bind: '127.0.0.1:8118',
    protocol: 'http',
    fixed_exit: 2,
  })

  await controller.api('POST', `${net}/forwards`, {
    name: 'web',
    protocol: 'tcp',
    listen_node: 3,
    bind: '127.0.0.1:8080',
    target_node: 2,
    target_addr: '192.168.1.10:80',
  })
  await controller.api('POST', `${net}/forwards`, {
    name: 'db',
    protocol: 'tcp',
    listen_node: 4,
    bind: '127.0.0.1:5432',
    target_node: 2,
    target_addr: '127.0.0.1:5432',
  })
  await controller.api('POST', `${net}/forwards`, {
    name: 'dns',
    protocol: 'udp',
    listen_node: 3,
    bind: '127.0.0.1:5353',
    target_node: 2,
    target_addr: '192.168.1.1:53',
    enabled: false,
  })
  await controller.api('POST', `${net}/forwards`, {
    name: 'printer',
    protocol: 'tcp',
    listen_node: 2,
    bind: '0.0.0.0:9100',
    target_node: 3,
    target_addr: '127.0.0.1:9100',
    allow_from: ['192.168.1.0/24'],
    enabled: false,
  })
  await controller.api('POST', `${net}/forwards`, {
    name: 'spare-web',
    protocol: 'tcp',
    listen_node: 5,
    bind: '127.0.0.1:8081',
    target_node: 2,
    target_addr: '127.0.0.1:80',
  })

  await controller.api('PUT', `${net}/pairs/2/3`, { keep_alive: true })
  await controller.api('POST', `${net}/tokens`, { tags: ['servers'], expires_in_secs: 3600 })
  await controller.api('POST', `${net}/tokens`, { virtual_ip: '10.66.0.50', expires_in_secs: 7 * 86400 })
  await controller.api('POST', '/networks', { name: 'lab' })

  // 让跑着的三个节点互相连一下，路由表里才有东西；节点每 10 秒上报一次。
  controller.touch(nodes.laptop, '10.66.0.1')
  controller.touch(nodes.laptop, '10.66.0.2')
  controller.touch(nodes.nas, '10.66.0.1')
  await expect
    .poll(async () => (await controller.api<{ routes: unknown[] }>('GET', `${net}/nodes/3/routes`)).routes.length, {
      timeout: 30_000,
    })
    .toBeGreaterThan(0)
})

test('有数据时：总览、节点', async () => {
  await page.reload()
  await goTo(page, '总览')
  await expect(page.getByTestId('overview-status')).toBeVisible()
  await shot('overview')

  await goTo(page, '节点')
  await expect(page.getByTestId('node-row-gw-hk')).toBeVisible()
  await shot('nodes')

  await page.getByRole('searchbox').fill('lap')
  await page.getByRole('button', { name: '筛选' }).click()
  await page.getByRole('checkbox', { name: 'laptops' }).check()
  await page.keyboard.press('Escape')
  await expect(page.getByTestId('node-row-gw-hk')).toBeHidden()
  await shot('nodes-filtered')
  await page.getByRole('button', { name: '清除全部' }).click()

  await page.getByRole('searchbox').fill('no-such-node')
  await expect(page.getByText('没有符合条件的节点。')).toBeVisible()
  await shot('nodes-no-matches')
  await page.getByRole('button', { name: '清除筛选' }).click()

  await page.getByRole('checkbox', { name: '选中 laptop' }).check()
  await page.getByRole('checkbox', { name: '选中 office-pc' }).check()
  await shot('nodes-selection-bar', { fullPage: false })
  await page.getByRole('button', { name: '打标签…' }).click()
  await expect(page.getByRole('dialog', { name: '给 2 个节点打标签' })).toBeVisible()
  await shot('nodes-batch-tag', { fullPage: false })
  await page.getByRole('button', { name: '取消', exact: true }).click()
  await page.getByRole('button', { name: '取消选择' }).click()

  await page.getByRole('button', { name: '添加节点' }).click()
  await page.getByRole('combobox', { name: '标签' }).fill('servers')
  await page.keyboard.press('Enter')
  await page.getByRole('checkbox', { name: /可以作为出口/ }).check()
  await page.getByRole('button', { name: '生成注册令牌' }).click()
  await expect(page.getByTestId('join-command')).toContainText('echo i3t-')
  await shot('add-node-waiting', { fullPage: false })
  const extra = controller.join((await page.getByTestId('join-command').textContent()) ?? '')
  nodes.extra = extra
  await expect(page.getByTestId('node-joined')).toBeVisible()
  await shot('add-node-joined', { fullPage: false })
  await page.getByRole('button', { name: '完成' }).click()
})

test('有数据时：节点详情', async () => {
  await page.getByRole('link', { name: 'gw-hk', exact: true }).click()
  await expect(page.getByTestId('capability-relay')).toBeVisible()
  await shot('node-detail-relay')

  await goTo(page, '节点')
  await page.getByRole('link', { name: 'nas-home', exact: true }).click()
  await expect(page.getByTestId('capability-exit')).toBeVisible()
  await page.getByText('高级', { exact: true }).click()
  await shot('node-detail-exit-and-advanced')

  await goTo(page, '节点')
  await page.getByRole('link', { name: 'laptop', exact: true }).click()
  await page.getByTestId('capability-exit').getByRole('button', { name: '怎么声明' }).click()
  await expect(page.getByTestId('declare-command')).toBeVisible()
  await shot('node-detail-not-declared')

  await page.getByRole('tab', { name: '路由' }).click()
  await expect(page.getByRole('columnheader', { name: '路径' })).toBeVisible()
  await shot('node-detail-routes')

  await page.getByRole('tab', { name: '访问' }).click()
  await expect(page.getByText(/它可以访问的节点/)).toBeVisible()
  await shot('node-detail-access')

  await page.getByRole('tab', { name: '关联配置' }).click()
  await expect(page.getByText('和它有关的端口转发')).toBeVisible()
  await shot('node-detail-related')

  await page.getByRole('tab', { name: '概览' }).click()
  await page.getByRole('button', { name: '改虚拟 IP' }).click()
  await expect(page.getByRole('dialog')).toBeVisible()
  await shot('node-edit-ip', { fullPage: false })
  await page.getByRole('button', { name: '取消', exact: true }).click()

  await page.getByRole('button', { name: '更多操作' }).click()
  await page.getByRole('menuitem', { name: '删除…' }).click()
  await expect(page.getByRole('alertdialog')).toBeVisible()
  await shot('node-delete-confirm', { fullPage: false })
  await page.getByRole('button', { name: '取消', exact: true }).click()

  await goTo(page, '节点')
  await page.getByRole('link', { name: 'office-pc', exact: true }).click()
  await page.getByRole('tab', { name: '路由' }).click()
  await expect(page.getByText('节点离线，没有路由表。')).toBeVisible()
  await shot('node-detail-routes-offline')

  // 更新：nas-home 在容器里，不能自己更新；laptop 能，"立即更新"的确认框。
  await goTo(page, '节点')
  await page.getByRole('link', { name: 'nas-home', exact: true }).click()
  await page.getByRole('tab', { name: '更新' }).click()
  await expect(page.getByTestId('node-self-update')).toContainText('在容器里运行')
  await shot('node-detail-update-manual')

  await goTo(page, '节点')
  await page.getByRole('link', { name: 'laptop', exact: true }).click()
  await page.getByRole('tab', { name: '更新' }).click()
  await expect(page.getByTestId('node-update-state')).toContainText('有新版本')
  await shot('node-detail-update')
  await page.getByRole('button', { name: '立即更新…' }).first().click()
  await expect(page.getByRole('dialog')).toContainText('它现在没有活跃的流')
  await shot('node-force-update-dialog', { fullPage: false })
  await page.getByRole('button', { name: '取消', exact: true }).click()
})

test('有数据时：拓扑和路径诊断', async () => {
  await goTo(page, '拓扑')
  const graph = page.getByTestId('topology-graph')
  await expect(graph.locator('.react-flow__node')).toHaveCount(6)
  await expect(graph.locator('.react-flow__edge').first()).toBeAttached()
  await shot('topology')

  await page.getByRole('combobox', { name: '节点 A' }).click()
  await page.getByRole('option', { name: /10\.66\.0\.3$/ }).click()
  await expect(page.getByRole('listbox')).toHaveCount(0)
  await page.getByRole('combobox', { name: '节点 B' }).click()
  await page.getByRole('option', { name: /nas-home/ }).click()
  await expect(page.getByTestId('pair-panel')).toBeVisible()
  await shot('topology-pair')

  await page.getByRole('radio', { name: '路由矩阵' }).click()
  await expect(page.getByTestId('route-matrix')).toBeVisible()
  await shot('topology-matrix')

  await page.getByTestId('pair-panel').getByRole('link', { name: '路径诊断' }).click()
  await expect(page.getByTestId('path-forward')).toHaveAttribute('data-source', /query|report/)
  await shot('diagnosis')
})

test('有数据时：访问控制', async () => {
  await goTo(page, '访问控制')
  await expect(page.getByTestId('tag-graph')).toBeVisible()
  await expect(page.getByTestId('rule-row-2')).toBeVisible()
  await shot('access-all-open')

  // 停用默认规则，按标签收紧。
  await page.getByRole('switch', { name: '启用规则 default' }).click()
  await expect(page.getByRole('alertdialog')).toBeVisible()
  await shot('access-disable-default-confirm', { fullPage: false })
  await page.getByRole('button', { name: '停用规则' }).click()
  await expect(page.getByTestId('unconnected')).toContainText('没有被任何规则连通的节点')
  await clearToasts(page)
  await shot('access-tightened')

  await page.getByRole('button', { name: '展开标签 servers 的成员' }).click()
  await shot('access-tag-panel-open')

  await page.getByRole('button', { name: '新建规则' }).click()
  const dialog = page.getByRole('dialog', { name: '新建规则' })
  await dialog.getByRole('combobox', { name: '标签 a' }).click()
  await page.getByRole('option', { name: /laptops/ }).click()
  await dialog.getByRole('combobox', { name: '标签 b' }).click()
  await page.getByRole('option', { name: /home/ }).click()
  await dialog.getByRole('radio', { name: '只能 a 访问 b' }).click()
  await expect(page.getByTestId('rule-impact')).toBeVisible()
  await shot('access-rule-dialog', { fullPage: false })
  await dialog.getByRole('button', { name: '取消', exact: true }).click()
  await page.getByRole('button', { name: '放弃改动' }).click()

  await page.getByRole('radio', { name: '按节点' }).click()
  await page.getByRole('combobox', { name: '焦点节点' }).click()
  await page.getByRole('option', { name: /gw-hk/ }).click()
  await expect(page.getByTestId('node-graph').getByText('gw-hk')).toBeVisible()
  await shot('access-node-view')

  await page.getByRole('button', { name: '访问检查' }).click()
  const check = page.getByTestId('access-check')
  await check.getByRole('combobox', { name: '从' }).click()
  await page.getByRole('option', { name: /laptop/ }).click()
  await check.getByRole('combobox', { name: '到' }).click()
  await page.getByRole('option', { name: /nas-home/ }).click()
  await expect(page.getByTestId('check-laptop-nas-home')).toContainText('可以')
  await shot('access-check')
  await page.getByRole('button', { name: '收起访问检查' }).click()
  await page.getByRole('radio', { name: '按标签' }).click()
})

test('有数据时：接入', async () => {
  await goTo(page, '出口')
  await expect(page.getByTestId('exit-row-nas-home')).toBeVisible()
  await shot('exits')
  await page.getByRole('button', { name: '启用出口' }).click()
  await expect(page.getByRole('dialog', { name: '启用出口' })).toBeVisible()
  await shot('exits-enable-dialog', { fullPage: false })
  await page.getByRole('button', { name: '取消', exact: true }).click()

  await goTo(page, '监听器')
  await expect(page.getByTestId('listener-row-lan')).toBeVisible()
  await shot('listeners')
  await page.getByRole('button', { name: '新建监听器' }).click()
  const listener = page.getByRole('dialog', { name: '新建监听器' })
  await listener.getByRole('radio', { name: /给别的机器用/ }).click()
  await shot('listeners-dialog')
  await listener.getByRole('button', { name: '取消', exact: true }).click()
  await page.getByRole('button', { name: '放弃改动' }).click()

  await goTo(page, '接入凭据')
  await expect(page.getByTestId('credential-row-phone')).toBeVisible()
  await shot('credentials')
  await page.getByRole('button', { name: '签发凭据' }).click()
  await page.getByRole('dialog', { name: '签发凭据' }).getByLabel('名字').fill('guest')
  await page.getByRole('button', { name: '签发', exact: true }).click()
  await expect(page.getByTestId('secret-dialog')).toBeVisible()
  await shot('credentials-secret-shown-once', { fullPage: false })
  await page.getByRole('button', { name: '我已保存，关闭' }).click()

  await goTo(page, '端口转发')
  await expect(page.getByTestId('forward-row-web')).toBeVisible()
  await shot('forwards')
  await page.getByRole('button', { name: '新建转发' }).click()
  const forward = page.getByRole('dialog', { name: '新建转发' })
  await forward.getByRole('group', { name: '在哪里监听' }).getByRole('combobox', { name: '节点' }).click()
  await page.getByRole('option', { name: /laptop/ }).click()
  await forward.getByRole('group', { name: '转发到哪里' }).getByRole('combobox', { name: '节点' }).click()
  await page.getByRole('option', { name: /nas-home/ }).click()
  await forward.getByRole('radio', { name: /它所在的局域网，或者任何能连到它的人/ }).click()
  await expect(page.getByTestId('forward-check')).toBeVisible()
  await shot('forwards-dialog')
  await forward.getByRole('button', { name: '取消', exact: true }).click()
  await page.getByRole('button', { name: '放弃改动' }).click()
})

test('有数据时：管理区和设置', async () => {
  await goToAdmin(page, '注册')
  await expect(page.getByTestId('token-row-7')).toBeVisible()
  await shot('tokens')

  await goToAdmin(page, '节点对')
  await expect(page.getByTestId('pair-row-nas-home-laptop')).toBeVisible()
  await shot('pairs')

  await goToAdmin(page, '网络')
  await expect(page.getByRole('heading', { name: 'home 的设置' })).toBeVisible()
  await shot('network')
  await page.getByRole('button', { name: '新建网络' }).click()
  await expect(page.getByRole('dialog', { name: '新建网络' })).toBeVisible()
  await shot('network-create-dialog', { fullPage: false })
  await page.getByRole('button', { name: '取消', exact: true }).click()

  await page.getByRole('button', { name: /切换网络/ }).click()
  await shot('network-switcher', { fullPage: false })
  await page.keyboard.press('Escape')

  await goToAdmin(page, '设置')
  await expect(page.getByText('已用管理令牌登录。')).toBeVisible()
  await shot('settings-session')
  await page.getByRole('link', { name: /节点更新/ }).click()
  await expect(page.getByTestId('release-version')).toHaveText(`最新版本 ${kit.newVersion}`)
  await shot('settings-updates')
  await page.getByRole('button', { name: '全部更新…' }).click()
  await expect(page.getByRole('dialog')).toBeVisible()
  await shot('settings-updates-force-all', { fullPage: false })
  await page.getByRole('button', { name: '取消', exact: true }).click()

  // 一个在这台机器上自检不过的版本：让 laptop 立即更新到它，自检失败，这个版本被暂停。
  const broken = kit.oldVersion.replace(/\d+$/, (patch) => String(Number(patch) + 7))
  const program = join(controller.dir, 'broken-i3t')
  writeFileSync(program, "#!/bin/sh\necho '这个程序在这台机器上跑不了：缺一个库' >&2\nexit 127\n")
  chmodSync(program, 0o755)
  release.publish(broken, program)
  await controller.api('POST', '/update/check')
  await controller.api('POST', '/networks/1/nodes/3/update', { timing: 'now' })
  await expect
    .poll(async () => (await controller.api<{ paused: unknown[] }>('GET', '/update')).paused.length, {
      timeout: 60_000,
    })
    .toBe(1)
  await page.reload()
  await page.getByRole('button', { name: '查看原因' }).click()
  await expect(page.getByTestId(`paused-reason-${broken}`)).toContainText('缺一个库')
  await shot('settings-updates-paused')

  await page.getByRole('link', { name: '操作记录' }).click()
  await expect(page.getByRole('columnheader', { name: '说明' })).toBeVisible()
  await shot('settings-log')
  await page.getByRole('link', { name: '关于' }).click()
  await expect(page.getByText('controller 公钥')).toBeVisible()
  await shot('settings-about')
})

test('计划中的页面，和找不到的东西', async () => {
  await page
    .getByRole('navigation', { name: '主导航' })
    .getByRole('link', { name: /连通性测试/ })
    .click()
  await expect(page.getByRole('heading', { name: /连通性测试/ })).toBeVisible()
  await shot('planned-tests')
  await page.goto(`${controller.baseUrl}/n/1/nodes/999`)
  await expect(page.getByText('找不到这个节点')).toBeVisible()
  await shot('node-not-found')
  await page.goto(`${controller.baseUrl}/n/42/overview`)
  await expect(page.getByText('找不到这个网络')).toBeVisible()
  await shot('network-not-found')
})

test('窄一些的笔记本宽度', async () => {
  await page.setViewportSize({ width: 1024, height: 720 })
  await page.goto(`${controller.baseUrl}/n/1/nodes`)
  await expect(page.getByTestId('node-row-gw-hk')).toBeVisible()
  await shot('narrow-nodes')
  await page.getByRole('link', { name: 'gw-hk', exact: true }).click()
  await expect(page.getByTestId('capability-relay')).toBeVisible()
  await shot('narrow-node-detail')
  await goTo(page, '访问控制')
  await expect(page.getByTestId('rule-row-2')).toBeVisible()
  await shot('narrow-access-rules')
  await page.getByRole('radio', { name: '关系图' }).click()
  await expect(page.getByTestId('tag-graph')).toBeVisible()
  await shot('narrow-access-graph')
  await goTo(page, '端口转发')
  await expect(page.getByTestId('forward-row-web')).toBeVisible()
  await shot('narrow-forwards')
  await page.goto(`${controller.baseUrl}/n/1/topology?a=3&b=2`)
  await expect(page.getByTestId('pair-panel')).toBeVisible()
  await shot('narrow-topology-pair')
  await page.goto(`${controller.baseUrl}/n/1/diagnosis?a=3&b=2`)
  await expect(page.getByTestId('path-forward')).toBeVisible()
  await shot('narrow-diagnosis')
  await page.goto(`${controller.baseUrl}/settings/updates`)
  await expect(page.getByTestId('updates-tab')).toBeVisible()
  await shot('narrow-settings-updates')
  await page.getByRole('button', { name: '收起侧栏' }).click()
  await goTo(page, '总览')
  await expect(page.getByTestId('overview-status')).toBeVisible()
  await shot('narrow-overview-sidebar-collapsed')
})
