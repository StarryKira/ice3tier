import { screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'

import { t } from '@/i18n'
import { makeNode } from '@/test/fixtures'
import { renderWithApp } from '@/test/harness'

import { NodeSelect } from './node'

const NODES = [
  makeNode(1, 'h1', { country: 'DE', online: true }),
  makeNode(2, 'tx', { country: 'CN', online: true }),
  makeNode(3, 'lab', { country: null, platform: 'linux-x86_64', online: false }),
]

describe('NodeSelect', () => {
  it('每一项有国旗；不能选的排在后面，原因写在同一行，选不了', async () => {
    const user = userEvent.setup()
    const onChange = vi.fn()
    renderWithApp(
      <NodeSelect
        aria-label="exit"
        nodes={NODES}
        value={null}
        onChange={onChange}
        disabledReason={(node) => (node.name === 'tx' ? 'Exit is off' : null)}
      />,
    )
    await user.click(screen.getByRole('combobox', { name: 'exit' }))

    const options = await screen.findAllByRole('option')
    expect(options.map((option) => within(option).getByText(/^(h1|tx|lab)$/).textContent)).toEqual(['h1', 'lab', 'tx'])

    const h1 = screen.getByRole('option', { name: /h1/ })
    const lab = screen.getByRole('option', { name: /lab/ })
    const tx = screen.getByRole('option', { name: /tx/ })
    expect(within(h1).getByRole('img')).toHaveAttribute('src', '/flags/de.svg')
    // 不知道国家的节点没有国旗（画的是系统的图标），离线写在虚拟 IP 后面。
    expect(within(lab).queryByRole('img')).toBeNull()
    expect(within(lab).getByText(`10.66.0.3 · ${t.common.offline}`)).toBeVisible()

    expect(tx).toHaveAttribute('aria-disabled', 'true')
    expect(within(tx).getByText('Exit is off')).toBeVisible()
    expect(within(tx).queryByText('10.66.0.2')).toBeNull()
    await user.click(tx)
    expect(onChange).not.toHaveBeenCalled()

    await user.click(h1)
    expect(onChange).toHaveBeenCalledWith(1)
  })

  it('按虚拟 IP 也搜得到，不能选的也搜得到', async () => {
    const user = userEvent.setup()
    renderWithApp(
      <NodeSelect
        aria-label="node"
        nodes={NODES}
        value={null}
        onChange={vi.fn()}
        disabledReason={(node) => (node.name === 'tx' ? 'Exit is off' : null)}
      />,
    )
    await user.click(screen.getByRole('combobox', { name: 'node' }))
    await user.type(screen.getByRole('textbox', { name: t.select.nodeSearch }), '10.66.0.2')
    expect(screen.getAllByRole('option')).toHaveLength(1)
    expect(screen.getByRole('option', { name: /tx/ })).toHaveAttribute('aria-disabled', 'true')
  })
})
