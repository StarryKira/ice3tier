import { describe, expect, it } from 'vitest'

import { relayAddresses, relayHasTcp } from './relay'

describe('中继的地址', () => {
  it('0.1.10 起：按给的顺序，来源照写，不认识的来源当作 unknown', () => {
    const relay = {
      addresses: [
        { address: '203.0.113.10:20330', source: 'declared' as const },
        { address: '198.51.100.20:20330', source: 'observed' as const },
        { address: '192.0.2.30:20330', source: 'mystery' as never },
      ],
      bandwidth_mbps: null,
    }
    expect(relayAddresses(relay).map((entry) => entry.source)).toEqual(['declared', 'observed', 'unknown'])
    expect(relayHasTcp(relay)).toBe(true)
    expect(relayHasTcp({ addresses: [], bandwidth_mbps: null })).toBe(false)
  })

  it('更早的 controller：人填的 UDP、TCP 两组合在一起去重，来源不知道；TCP 看 tcp 那一组', () => {
    const relay = { udp: ['203.0.113.7:20330'], tcp: ['203.0.113.7:20330', '203.0.113.7:443'], bandwidth_mbps: null }
    expect(relayAddresses(relay)).toEqual([
      { address: '203.0.113.7:20330', source: 'unknown' },
      { address: '203.0.113.7:443', source: 'unknown' },
    ])
    expect(relayHasTcp({ udp: ['203.0.113.7:20330'], tcp: [], bandwidth_mbps: null })).toBe(false)
  })
})
