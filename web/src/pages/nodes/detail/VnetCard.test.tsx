import { screen } from '@testing-library/react'
import { describe, expect, it } from 'vitest'

import { type NodeDetail, type VnetReport } from '@/api/types'
import { setLocale, t } from '@/i18n'
import { firewallText, namesText, vnetCauseText, vnetState } from '@/lib/vnet'
import { makeNode } from '@/test/fixtures'
import { renderWithApp } from '@/test/harness'

import { VnetCard } from './VnetCard'

const v = t.nodes.detail.vnet

const INSTALLED: VnetReport = {
  state: 'installed',
  device: 'i3t0',
  mtu: 1280,
  names: { state: 'resolved' },
  firewall: { accept: { state: 'on' }, guard: { state: 'on' } },
  sent_packets: 3400,
  sent_bytes: 1_258_291,
  received_packets: 2100,
  received_bytes: 524_288,
  dropped: { bad_source: 0, bad_destination: 0, not_allowed: 12, unreachable: 0, malformed: 0 },
}

const detail = (extra: Partial<NodeDetail> = {}): NodeDetail => ({
  ...makeNode(1, 'laptop', { online: true }),
  endpoints: [],
  observed_endpoint: null,
  mapping_varies: false,
  flows: 0,
  tx_bytes: 0,
  rx_bytes: 0,
  reported_secs_ago: 3,
  ...extra,
})

describe('虚拟网卡的说法', () => {
  it('不认识的、没有报的都是未知', () => {
    expect(vnetState(null)).toBe('unknown')
    expect(vnetState(undefined)).toBe('unknown')
    expect(vnetState({ ...INSTALLED, state: 'later' as VnetReport['state'] })).toBe('unknown')
    expect(vnetState(INSTALLED)).toBe('installed')
  })

  it('原因：管理员、root 写成怎么运行；能力名原样；地址段重叠带上重叠的那个', () => {
    expect(vnetCauseText({ code: 'no_permission', missing: ['Administrator'] })).toBe(v.causes.administrator)
    expect(vnetCauseText({ code: 'no_permission', missing: ['root'] })).toBe(v.causes.root)
    expect(vnetCauseText({ code: 'no_permission', missing: ['CAP_NET_ADMIN'] })).toBe(v.causes.missing('CAP_NET_ADMIN'))
    expect(vnetCauseText({ code: 'no_permission', missing: ['CAP_NET_BIND_SERVICE'] })).toBe(
      v.causes.missing('CAP_NET_BIND_SERVICE'),
    )
    expect(vnetCauseText({ code: 'range_in_use', with: '192.168.1.5/24 dev eth0' })).toBe(
      v.causes.rangeInUse('192.168.1.5/24 dev eth0'),
    )
    expect(vnetCauseText({ code: 'program_missing', program: 'nft' })).toBe(
      v.causes.programMissingPackage('nft', 'nftables'),
    )
    expect(vnetCauseText({ code: 'later' } as never)).toBe(v.causes.unknown)
  })

  it('防火墙：两条一样时合起来写，不一样时逐条写，装不上的带原因', () => {
    expect(firewallText(INSTALLED.firewall)).toEqual({ text: v.firewallOn, problem: false })
    expect(firewallText({ accept: { state: 'off' }, guard: { state: 'off' } }).text).toBe(v.firewallOff)
    expect(firewallText({ accept: { state: 'unsupported' }, guard: { state: 'unsupported' } }).text).toBe(
      v.firewallUnsupported,
    )
    const mixed = firewallText({
      accept: { state: 'on' },
      guard: { state: 'failed', cause: { code: 'program_missing', program: 'iptables' } },
    })
    expect(mixed.problem).toBe(true)
    expect(mixed.text).toBe(`${v.accept} ${v.rule.on} · ${v.ruleFailed(v.guard, v.causes.programMissing('iptables'))}`)
  })

  it('名字', () => {
    expect(namesText({ state: 'resolved' })).toBe(v.namesResolved)
    expect(namesText({ state: 'no_resolver' })).toBe(v.namesNoResolver)
    expect(namesText(undefined)).toBeNull()
  })
})

describe('概览上的虚拟网卡', () => {
  it('装好了：网卡和 MTU、名字、防火墙、流量；丢掉的包只写不是 0 的；底边写怎么关', () => {
    renderWithApp(<VnetCard node={detail({ vnet: INSTALLED })} />)
    const card = screen.getByTestId('vnet')
    expect(card).toHaveAttribute('data-state', 'installed')
    expect(card).toHaveTextContent(v.state.installed)
    expect(card).toHaveTextContent(v.deviceValue('i3t0', 1280))
    expect(card).toHaveTextContent(v.namesResolved)
    expect(screen.getByTestId('vnet-firewall')).toHaveTextContent(v.firewallOn)
    expect(screen.getByTestId('vnet-dropped')).toHaveTextContent(v.dropValue(v.drop.not_allowed, '12'))
    expect(screen.getByTestId('vnet-dropped')).not.toHaveTextContent(v.drop.unreachable)
    expect(screen.getByTestId('vnet-how')).toHaveTextContent(v.howOff)
  })

  it('装不上：原因醒目', () => {
    renderWithApp(
      <VnetCard
        node={detail({
          vnet: {
            ...INSTALLED,
            state: 'failed',
            cause: { code: 'no_permission', missing: ['CAP_NET_ADMIN'] },
            firewall: { accept: { state: 'off' }, guard: { state: 'off' } },
          },
        })}
      />,
    )
    expect(screen.getByTestId('vnet')).toHaveTextContent(v.state.failed)
    expect(screen.getByTestId('vnet-cause')).toHaveTextContent(v.causes.missing('CAP_NET_ADMIN'))
    expect(screen.getByTestId('vnet-cause')).toHaveClass('text-danger')
  })

  it('关着是本机有意的：写怎么打开', () => {
    renderWithApp(<VnetCard node={detail({ vnet: { ...INSTALLED, state: 'off' } })} />)
    expect(screen.getByTestId('vnet')).toHaveTextContent(v.offDesc)
    expect(screen.getByTestId('vnet-how')).toHaveTextContent(v.howOn)
  })

  it('没有报：未知，写为什么，不当成关着', () => {
    renderWithApp(<VnetCard node={detail({ vnet: null })} />)
    expect(screen.getByTestId('vnet')).toHaveAttribute('data-state', 'unknown')
    expect(screen.getByTestId('vnet')).toHaveTextContent(v.unknownOld)
    expect(screen.queryByTestId('vnet-how')).not.toBeInTheDocument()
  })

  it('离线：上线后显示', () => {
    renderWithApp(<VnetCard node={detail({ online: false, reported_secs_ago: null })} />)
    expect(screen.getByTestId('vnet')).toHaveTextContent(v.unknownOffline)
  })
})

describe('英文', () => {
  it('卡片里没有中文', () => {
    setLocale('en', { remember: false })
    renderWithApp(<VnetCard node={detail({ vnet: INSTALLED })} />)
    expect(screen.getByTestId('vnet')).toHaveTextContent('Virtual network')
    expect(document.body.textContent).not.toMatch(/\p{Script=Han}/u)
  })
})
