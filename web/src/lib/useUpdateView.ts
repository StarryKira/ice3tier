import { useQuery } from '@tanstack/react-query'
import { useMemo } from 'react'

import { q } from '@/api/queries'
import { type Node } from '@/api/types'

import { describeUpdate, NO_UPDATE_CONTEXT, type UpdateContext, type UpdateView } from './nodeUpdate'
import { formatRelative } from './time'
import { useNow } from './useNow'

/** 判断节点更新情况要用到的全局信息：已知的发布、自动更新开没开、暂停的版本。取不到时按"什么都不知道"处理。 */
export function useUpdateContext(): UpdateContext {
  const overview = useQuery(q.update())
  return useMemo(
    () =>
      overview.data === undefined
        ? NO_UPDATE_CONTEXT
        : {
            release: overview.data.release,
            autoUpdate: overview.data.auto_update,
            paused: overview.data.paused.map((paused) => paused.version),
          },
    [overview.data],
  )
}

/** 一个节点的更新情况，节点列表、详情、设置三处同一套说法。 */
export function useUpdateView(node: Pick<Node, 'version' | 'platform' | 'online' | 'update'>): UpdateView {
  const context = useUpdateContext()
  const now = useNow()
  return describeUpdate(node, context, (at) => formatRelative(at, now))
}
