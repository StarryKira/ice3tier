import { type Network } from '@/api/types'
import { isNumber, readStored } from '@/lib/storage'

import { LAST_NETWORK_KEY } from './context'

/**
 * "当前网络"：地址里写着哪个就是哪个；不在某个网络的页面上（首页、设置）时用上次看的那个，再不行用第一个。
 * 一个网络都没有时返回 `undefined`。
 */
export function pickCurrentNetwork(networks: readonly Network[], fromUrl?: number): Network | undefined {
  const first = networks[0]
  if (first === undefined) return undefined
  const remembered = readStored<number>(LAST_NETWORK_KEY, first.id, isNumber)
  return (
    networks.find((network) => network.id === fromUrl) ?? networks.find((network) => network.id === remembered) ?? first
  )
}
