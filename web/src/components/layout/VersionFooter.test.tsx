import { screen } from '@testing-library/react'
import { describe, expect, it } from 'vitest'

import { t } from '@/i18n'
import { WEB_VERSION } from '@/lib/version'
import { AboutTab } from '@/pages/settings/AboutTab'
import { makeStatus } from '@/test/fixtures'
import { fakeApi, renderWithApp } from '@/test/harness'

import { VersionFooter } from './VersionFooter'

describe('版本的显示', () => {
  it('侧栏底部写出界面的版本，就是构建时写进来的那一个', () => {
    renderWithApp(<VersionFooter status={makeStatus({ version: WEB_VERSION })} />)
    expect(screen.getByTestId('web-version')).toHaveTextContent(t.version.footerWeb(__APP_VERSION__))
    expect(screen.getByTestId('version-footer')).toHaveTextContent(t.version.footerController(WEB_VERSION))
    // 同一次构建：不提。
    expect(screen.queryByText(t.version.mismatchShort)).not.toBeInTheDocument()
    expect(screen.queryByTestId('newer-version')).not.toBeInTheDocument()
  })

  it('controller 的状态还没取到时，也写界面的版本', () => {
    renderWithApp(<VersionFooter status={undefined} />)
    expect(screen.getByTestId('web-version')).toHaveTextContent(WEB_VERSION)
  })

  it('两边版本不一样时直接写出来；有更新的正式版时有一个去节点更新的提示', () => {
    renderWithApp(<VersionFooter status={makeStatus({ version: '9.9.9', newer_version: '10.0.0' })} />)
    expect(screen.getByText(t.version.mismatchShort)).toBeVisible()
    const newer = screen.getByTestId('newer-version')
    expect(newer).toHaveTextContent(t.version.newer('10.0.0'))
    expect(newer).toHaveAttribute('href', '/settings/updates')
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
