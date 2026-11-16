import { expect, type Page } from '@playwright/test'

import { type Controller } from './controller'

/** 在登录页填管理令牌登录。 */
export async function signIn(page: Page, controller: Controller): Promise<void> {
  await page.goto(`${controller.baseUrl}/`)
  await page.getByLabel('管理令牌', { exact: true }).fill(controller.adminToken)
  await page.getByRole('button', { name: '登录', exact: true }).click()
  await expect(page).not.toHaveURL(/\/login/)
}

/** 右下角的通知里出现了这句话。 */
export async function expectToast(page: Page, text: string | RegExp): Promise<void> {
  await expect(page.getByRole('list', { name: '通知' }).getByText(text).first()).toBeVisible()
}

/** 把通知都关掉，免得挡住截图或者后面的断言认错。 */
export async function clearToasts(page: Page): Promise<void> {
  const buttons = page.getByRole('button', { name: '关掉这条通知' })
  while ((await buttons.count()) > 0) await buttons.first().click()
}

/** 侧栏里点一项。 */
export async function goTo(page: Page, name: string): Promise<void> {
  await page.getByRole('navigation', { name: '主导航' }).getByRole('link', { name, exact: true }).click()
}

export async function goToAdmin(page: Page, name: string): Promise<void> {
  await page.getByRole('complementary').getByRole('link', { name, exact: true }).click()
}
