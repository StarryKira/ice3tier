import { expect, type Page } from '@playwright/test'

import { zhCN as t } from '../src/i18n/zh-CN'
import { type Controller } from './controller'

/**
 * 端到端测试在简体中文下跑（`playwright.config.ts` 的浏览器语言是 zh-CN），界面上的字从同一份目录里取，
 * 不在测试里抄。英文的截图另开一个浏览器语言是 en-US 的页面。
 */
export { t }

/** 正则里按字面匹配的一段。 */
export function literal(text: string): string {
  return text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
}

/** 在登录页填管理令牌登录。 */
export async function signIn(page: Page, controller: Controller): Promise<void> {
  await page.goto(`${controller.baseUrl}/`)
  await page.getByLabel(t.signIn.tokenLabel, { exact: true }).fill(controller.adminToken)
  await page.getByRole('button', { name: t.signIn.submit, exact: true }).click()
  await expect(page).not.toHaveURL(/\/login/)
}

/** 右下角的通知里出现了这句话。 */
export async function expectToast(page: Page, text: string | RegExp): Promise<void> {
  await expect(page.getByRole('list', { name: t.common.notifications }).getByText(text).first()).toBeVisible()
}

/** 把通知都关掉，免得挡住截图或者后面的断言认错。 */
export async function clearToasts(page: Page): Promise<void> {
  const buttons = page.getByRole('button', { name: t.common.dismiss })
  while ((await buttons.count()) > 0) await buttons.first().click()
}

/** 侧栏里点一项。 */
export async function goTo(page: Page, name: string): Promise<void> {
  await page.getByRole('navigation', { name: t.nav.label }).getByRole('link', { name, exact: true }).click()
}

export async function goToAdmin(page: Page, name: string): Promise<void> {
  await page.getByRole('complementary').getByRole('link', { name, exact: true }).click()
}
