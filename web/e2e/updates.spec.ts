import { chmodSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'

import { expect, type Page, test } from '@playwright/test'

import { Controller, type NodeHandle } from './controller'
import { clearToasts, expectToast, goTo, goToAdmin, signIn } from './helpers'
import { platformName, type ReleaseKit, releaseKit, ReleaseSource } from './release'

/**
 * 节点更新（白皮书 §10.5），对着一个发布源是本机目录的 controller：
 * 发布用 `i3t-release` 和一把测试钥匙签，controller 和节点都认这把钥匙（`pnpm e2e:kit` 编的）。
 *
 * 三个真的跑着的节点：able 能自己更新（有自己的一份程序文件，由看护的循环拉起来，换了程序能重启）；
 * boxed 以为自己在容器里；locked 的程序在一个不可写的目录里。
 * 走一遍：换发布源、立即检查、节点列表和详情上的标记与做法、确认框、全部更新（able 真的换成新版本重启）、
 * 一个自检不过的版本被暂停、看原因、继续。
 */

const NET = '/networks/1'

interface ApiNode {
  id: number
  name: string
  version: string
  update: { failure: { version: string } | null }
}

let controller: Controller
let page: Page
let kit: ReleaseKit
let release: ReleaseSource
const nodes = {} as Record<'able' | 'boxed' | 'locked', { handle: NodeHandle; id: number }>

/** 版本号的修订号加几：自检不过的那个"坏"版本用。 */
function bump(version: string, by: number): string {
  const [major, minor, patch] = version.split('.')
  return `${major}.${minor}.${Number(patch) + by}`
}

async function apiNode(id: number): Promise<ApiNode> {
  return controller.api<ApiNode>('GET', `${NET}/nodes/${id}`)
}

async function openUpdates(): Promise<void> {
  await goToAdmin(page, '设置')
  await page.getByRole('link', { name: '节点更新' }).click()
  await expect(page.getByTestId('updates-tab')).toBeVisible()
}

test.describe.configure({ mode: 'serial' })

test.beforeAll(async ({ browser }) => {
  test.setTimeout(120_000)
  kit = releaseKit()
  controller = await Controller.start({ bins: kit.oldBins })
  page = await browser.newPage()
})

test.afterAll(async () => {
  await page.close()
  controller.stop()
})

test('准备：三个节点，一个能自己更新、一个在容器里、一个程序目录不可写', async () => {
  await controller.api('POST', '/networks', { name: 'home' })
  for (const [index, name] of (['able', 'boxed', 'locked'] as const).entries()) {
    const handle = await controller.enroll()
    const id = index + 1
    await controller.api('PATCH', `${NET}/nodes/${id}`, { name })
    nodes[name] = { handle, id }
  }
  await controller.run(nodes.able.handle, { program: controller.ownProgram(nodes.able.handle), supervised: true })
  await controller.run(nodes.boxed.handle, {
    program: controller.ownProgram(nodes.boxed.handle),
    env: { container: 'podman' },
  })
  await controller.run(nodes.locked.handle, { program: controller.ownProgram(nodes.locked.handle, { readOnly: true }) })
  await controller.waitOnline(3)
  release = new ReleaseSource(join(controller.dir, 'releases'), kit)
  release.publish(kit.newVersion, kit.newProgram)
})

test('关掉自动更新，把发布源换成本机目录，立即检查：查到新版本和各平台的包', async () => {
  await signIn(page, controller)
  await openUpdates()

  await page.getByRole('switch', { name: '自动更新节点' }).click()
  await expectToast(page, '自动更新已关闭')
  await expect(page.getByText('节点不会自己更新。有新版本时在节点列表上标出')).toBeVisible()
  expect((await controller.api<{ auto_update: boolean }>('GET', '/update')).auto_update).toBe(false)

  // 默认的发布源查不到时（这里没有发布，或者连不上）这一块自己展开着；否则点开。
  const dirChoice = page.getByRole('radio', { name: /本机目录/ })
  if (!(await dirChoice.isVisible())) await page.locator('summary', { hasText: '发布源' }).click()
  await dirChoice.click()
  const input = page.getByLabel('目录的绝对路径')
  await input.fill('releases')
  await expect(page.getByText('要写绝对路径，例如 /srv/ice3tier/releases。')).toBeVisible()
  await expect(page.getByRole('button', { name: '保存发布源' })).toBeDisabled()
  await input.fill(release.root)
  await page.getByRole('button', { name: '保存发布源' }).click()
  await expectToast(page, '发布源已保存')
  await expect(page.getByTestId('source-summary')).toHaveText(`本机目录 ${release.root}`)
  await clearToasts(page)

  await page.getByRole('button', { name: '立即检查' }).click()
  await expect(page.getByTestId('check-result')).toHaveText(`查过了：最新版本是 ${kit.newVersion}，3 个节点可以更新。`)
  await expect(page.getByTestId('release-version')).toHaveText(`最新版本 ${kit.newVersion}`)
  await expect(page.getByTestId(`package-${platformName()}`)).toContainText(`i3t-${platformName()}`)
  // 发布比 controller 新：controller 要手动更新，界面提醒，侧栏底部也有。
  await expect(page.getByText(`controller 自己是 ${kit.oldVersion}，有新版本 ${kit.newVersion}`)).toBeVisible()
  await expect(page.getByTestId('newer-version')).toHaveText(`有新版本 ${kit.newVersion}`)
  await expect(page.getByTestId('update-summary')).toHaveText('3 个节点：1 个有新版本，2 个要手动更新。')
  await expect(page.getByTestId('update-row-boxed')).toContainText('不能自己更新')
  await expect(page.getByTestId('update-row-boxed').getByRole('button', { name: '立即更新… boxed' })).toBeDisabled()
  await expect(page.getByTestId('update-row-able').getByRole('button', { name: '立即更新… able' })).toBeEnabled()
})

test('节点列表标出有新版本和要手动更新的；详情里写出不能自己更新的原因和做法', async () => {
  await goTo(page, '节点')
  await expect(page.getByTestId('node-row-able').getByTestId('update-mark-available')).toBeVisible()
  await expect(page.getByTestId('node-row-boxed').getByTestId('update-mark-manual')).toBeVisible()
  // "不能自己更新而有新版本"上"需要处理"。
  await expect(page.getByRole('radio', { name: '需要处理 (2)' })).toBeVisible()

  await page.getByRole('link', { name: 'boxed', exact: true }).click()
  await page.getByRole('tab', { name: '更新' }).click()
  await expect(page.getByTestId('node-self-update')).toContainText('在容器里运行')
  await expect(page.getByTestId('node-self-update')).toContainText('换成新版本的镜像')
  await expect(page.getByRole('button', { name: '立即更新…' }).first()).toBeDisabled()

  await page.goto(`${controller.baseUrl}/n/1/nodes/${nodes.locked.id}?tab=update`)
  await expect(page.getByTestId('node-self-update')).toContainText('程序所在的目录不可写')
  await expect(page.getByTestId('node-self-update').locator('pre')).toContainText('curl -fLo i3t')
})

test('立即更新的确认框写出后果；取消就什么都不发', async () => {
  await page.goto(`${controller.baseUrl}/n/1/nodes/${nodes.able.id}?tab=update`)
  await expect(page.getByTestId('node-self-update')).toContainText('能自己更新')
  await expect(page.getByTestId('node-update-state')).toContainText(`有新版本 ${kit.newVersion}`)
  await page.getByRole('button', { name: '立即更新…' }).first().click()
  const dialog = page.getByRole('dialog', { name: `立即更新 able 到 ${kit.newVersion}？` })
  await expect(dialog).toContainText('它现在没有活跃的流')
  await dialog.getByRole('button', { name: '取消', exact: true }).click()
  await expect(dialog).toBeHidden()
  expect((await apiNode(nodes.able.id)).version).toBe(kit.oldVersion)
})

test('全部更新（排空后）：跳过的节点写出原因；能更新的那个真的换成新版本重启', async () => {
  test.setTimeout(150_000)
  await openUpdates()
  await page.getByRole('button', { name: '全部更新…' }).click()
  const dialog = page.getByRole('dialog', { name: `全部更新到 ${kit.newVersion}？` })
  await expect(dialog).toContainText('有新版本、能自己更新的节点有 1 个')
  await dialog.getByRole('radio', { name: /等手上的流结束再更新/ }).click()
  await dialog.getByRole('button', { name: '全部排空后更新' }).click()
  await expect(dialog).toBeHidden()
  const result = page.getByTestId('force-all-result')
  await expect(result).toContainText('able')
  await expect(result).toContainText('boxed')
  await expect(result).toContainText('locked')

  // 节点取包、自检、换程序、以"请重启我"退出，看护的循环把新程序拉起来，连上 controller 才算完。
  await expect
    .poll(async () => (await apiNode(nodes.able.id)).version, { timeout: 120_000, intervals: [1_000] })
    .toBe(kit.newVersion)
  await expect(page.getByTestId('update-row-able')).toContainText(kit.newVersion, { timeout: 20_000 })
  await expect(page.getByTestId('update-summary')).toContainText('1 个已是最新')
})

test('自检不过的版本：自动更新被暂停，看原因，继续', async () => {
  test.setTimeout(150_000)
  const broken = bump(kit.oldVersion, 7)
  const program = join(controller.dir, 'broken-i3t')
  writeFileSync(program, "#!/bin/sh\necho '这个程序在这台机器上跑不了：缺一个库' >&2\nexit 127\n")
  chmodSync(program, 0o755)
  release.publish(broken, program)

  await page.getByRole('button', { name: '立即检查' }).click()
  await expect(page.getByTestId('release-version')).toHaveText(`最新版本 ${broken}`)
  await page.getByRole('switch', { name: '自动更新节点' }).click()
  await expectToast(page, '自动更新已打开')
  await clearToasts(page)

  // able 取包、跑新程序的自检，不过；controller 暂停这个版本。
  const banner = page.getByText(`${broken} 在 able 上更新失败，已暂停这个版本的自动更新。`)
  await expect(banner).toBeVisible({ timeout: 90_000 })
  const reason = page.getByTestId(`paused-reason-${broken}`)
  await expect(reason).toBeHidden()
  await page.getByRole('button', { name: '查看原因' }).click()
  await expect(reason).toContainText('缺一个库')
  await expect(page.getByTestId('update-row-able')).toContainText('更新失败')
  await expect(page.getByTestId('update-row-able')).toContainText('缺一个库')
  // 设置里「节点更新」旁边有一个红点。
  await expect(page.getByRole('navigation', { name: '设置的分区' }).getByText('有被暂停的版本')).toBeAttached()
  // 节点没有换程序。
  expect((await apiNode(nodes.able.id)).version).toBe(kit.newVersion)

  await page.getByRole('button', { name: '继续更新其余节点' }).click()
  const question = page.getByRole('alertdialog', { name: `继续 ${broken} 的自动更新？` })
  await expect(question).toContainText('able 不会再被自动更新到这个版本')
  await question.getByRole('button', { name: '继续更新' }).click()
  await expectToast(page, `已继续 ${broken} 的自动更新`)
  await expect(banner).toHaveCount(0)
  expect((await controller.api<{ paused: unknown[] }>('GET', '/update')).paused).toEqual([])
})
