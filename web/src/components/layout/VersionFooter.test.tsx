import { screen } from '@testing-library/react'
import { describe, expect, it } from 'vitest'

import { t } from '@/i18n'
import { WEB_VERSION } from '@/lib/version'
import { AboutTab } from '@/pages/settings/AboutTab'
import { makeStatus } from '@/test/fixtures'
import { fakeApi, renderWithApp } from '@/test/harness'

import { VersionBadge } from './VersionFooter'

describe('版本的显示', () => {
  it('顶栏的版本号就是构建时写进来的那一个；同一次构建时说出两边的版本，不提醒', () => {
    renderWithApp(<VersionBadge status={makeStatus({ version: WEB_VERSION })} />)
    expect(screen.getByTestId('web-version')).toHaveTextContent(`v${__APP_VERSION__}`)
    const badge = screen.getByTestId('version-badge')
    expect(badge).toHaveAccessibleName(
      `${t.version.footerController(WEB_VERSION)} · ${t.version.footerWeb(WEB_VERSION)}`,
    )
    expect(badge).not.toHaveAttribute('data-mismatch')
    expect(badge).not.toHaveAttribute('href')
  })

  it('controller 的状态还没取到时，也写界面的版本', () => {
    renderWithApp(<VersionBadge status={undefined} />)
    expect(screen.getByTestId('web-version')).toHaveTextContent(WEB_VERSION)
  })

  it('两边版本不一样时直接说出来；有更新的正式版时点了去节点更新', () => {
    renderWithApp(<VersionBadge status={makeStatus({ version: '9.9.9', newer_version: '10.0.0' })} />)
    const badge = screen.getByTestId('version-badge')
    expect(badge).toHaveAttribute('data-mismatch', 'true')
    expect(badge).toHaveAttribute('href', '/settings/updates')
    expect(badge).toHaveAccessibleName(expect.stringContaining(t.version.newer('10.0.0')) as string)
  })

  it('只是版本不一样：说的是不一致的那句话', () => {
    renderWithApp(<VersionBadge status={makeStatus({ version: '9.9.9' })} />)
    expect(screen.getByTestId('version-badge')).toHaveAccessibleName(t.version.mismatch(WEB_VERSION, '9.9.9'))
  })

  it('关于页：两边的版本，不一样时说清楚是两次构建', async () => {
    fakeApi({ 'GET /status': makeStatus({ version: '9.9.9', newer_version: '10.0.0' }) })
    renderWithApp(<AboutTab />)
    expect(await screen.findByTestId('about-controller-version')).toHaveTextContent('9.9.9')
    expect(screen.getByTestId('about-web-version')).toHaveTextContent(WEB_VERSION)
    expect(screen.getByTestId('about-version-mismatch')).toHaveTextContent(t.version.mismatch(WEB_VERSION, '9.9.9'))
    expect(screen.getByText(t.version.newer('10.0.0'))).toBeVisible()
    // 指标的地址和界面同源，不在 /api/v1 下面。
    expect(screen.getByRole('button', { name: t.common.copyValue(`${window.location.origin}/metrics`) })).toBeVisible()
  })

  it('关于页：同一次构建时不提醒', async () => {
    fakeApi({ 'GET /status': makeStatus({ version: WEB_VERSION }) })
    renderWithApp(<AboutTab />)
    expect(await screen.findByTestId('about-web-version')).toHaveTextContent(WEB_VERSION)
    expect(screen.queryByTestId('about-version-mismatch')).not.toBeInTheDocument()
  })
})
