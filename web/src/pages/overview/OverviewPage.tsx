import { useQuery } from '@tanstack/react-query'

import { q } from '@/api/queries'
import { Page } from '@/components/page'
import { ErrorState, SkeletonBlock } from '@/components/states'
import { t } from '@/i18n'
import { GUIDE_INTERVAL } from '@/lib/polling'
import { useDocumentTitle } from '@/lib/useDocumentTitle'
import { useNetwork } from '@/pages/networks/context'

import { AccessMap } from './AccessMap'
import { GettingStarted } from './GettingStarted'

const o = t.overview

/**
 * 总览：整个面板是访问地图——看一个节点（或者一个标签）能和谁互访、靠的是哪几条规则。
 * 右上角一句话说整个网络的状态，有问题时点开是那几件事。节点不足两个时，整页是引导。
 */
export function OverviewPage() {
  useDocumentTitle(o.title)
  const network = useNetwork()

  const nodes = useQuery(q.nodes(network.id))
  const few = nodes.data !== undefined && nodes.data.length < 2
  // 引导期间一直在等新节点出现，取得勤一些。
  const guideNodes = useQuery({ ...q.nodes(network.id), refetchInterval: GUIDE_INTERVAL, enabled: few })

  if (nodes.isError && nodes.data === undefined) {
    return (
      <Page>
        <ErrorState title={o.loadFailed} error={nodes.error} onRetry={() => void nodes.refetch()} />
      </Page>
    )
  }
  if (nodes.data === undefined) {
    return (
      <Page>
        <SkeletonBlock lines={6} />
      </Page>
    )
  }
  if (few) return <GettingStarted network={network} nodes={guideNodes.data ?? nodes.data} />

  return (
    <div className="flex h-full flex-col">
      <AccessMap network={network} nodes={nodes.data} />
    </div>
  )
}
