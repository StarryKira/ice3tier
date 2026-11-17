import { type RelayAddress, type RelayAddressSource, type RelaySettings } from '@/api/types'

const SOURCES: readonly RelayAddressSource[] = ['declared', 'observed', 'local']

/**
 * 中继的地址，两种接口都认：0.1.10 起的 `addresses`（带来源）；更早的人填的 `udp`、`tcp`（合在一起去重，来源不知道）。
 * 来源是不认识的值时当作 `unknown`。
 */
export function relayAddresses(relay: RelaySettings): RelayAddress[] {
  if (relay.addresses !== undefined) {
    return relay.addresses.map((entry) => ({
      address: entry.address,
      source: SOURCES.includes(entry.source) ? entry.source : 'unknown',
    }))
  }
  return [...new Set([...(relay.udp ?? []), ...(relay.tcp ?? [])])].map((address) => ({ address, source: 'unknown' }))
}

/** 中继有没有能用 TCP 连的地址：0.1.10 起每个地址 UDP 和 TCP 都用；更早的看 `tcp` 那一组。 */
export function relayHasTcp(relay: RelaySettings): boolean {
  return relay.addresses !== undefined ? relay.addresses.length > 0 : (relay.tcp?.length ?? 0) > 0
}
