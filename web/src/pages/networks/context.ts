import { createContext, use } from 'react'

import { type Network } from '@/api/types'

/** 当前页面所在的网络。`/n/:network/…` 下面的页面都能拿到。 */
export const NetworkContext = createContext<Network | null>(null)

export function useNetwork(): Network {
  const network = use(NetworkContext)
  if (network === null) throw new Error('useNetwork 要在某个网络的页面里用')
  return network
}

const LAST_NETWORK_KEY = 'lastNetwork'
export { LAST_NETWORK_KEY }

/** 某个网络下某个页面的地址。网络写在路径里，刷新和分享链接都不会串到别的网络。 */
export function networkPath(network: number, page = 'overview'): string {
  return `/n/${network}/${page}`
}
