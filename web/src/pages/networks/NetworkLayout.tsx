import { useQuery } from '@tanstack/react-query'
import { Unplug } from 'lucide-react'
import { useEffect } from 'react'
import { Link, Outlet, useParams } from 'react-router'

import { q } from '@/api/queries'
import { Page } from '@/components/page'
import { EmptyState } from '@/components/states'
import { Button } from '@/components/ui/button'
import { Card } from '@/components/ui/card'
import { t } from '@/i18n'
import { writeStored } from '@/lib/storage'

import { LAST_NETWORK_KEY, NetworkContext, networkPath } from './context'

/** `/n/:network/…` 这一层：确认地址里的网络存在，把它交给下面的页面。 */
export function NetworkLayout() {
  const params = useParams()
  const id = Number(params.network)
  // 外壳已经取过网络列表，这里直接用缓存。
  const networks = useQuery(q.networks())
  const network = networks.data?.find((candidate) => candidate.id === id)

  useEffect(() => {
    if (network !== undefined) writeStored(LAST_NETWORK_KEY, network.id)
  }, [network])

  if (network === undefined) {
    const fallback = networks.data?.[0]
    return (
      <Page>
        <Card>
          <EmptyState
            icon={Unplug}
            title={t.networks.notFoundTitle}
            description={t.networks.notFoundBody}
            action={
              fallback === undefined ? undefined : (
                <Button asChild variant="primary">
                  <Link to={networkPath(fallback.id)}>{t.networks.goTo(fallback.name)}</Link>
                </Button>
              )
            }
          />
        </Card>
      </Page>
    )
  }
  // key：换网络时把页面里的本地状态（打开的对话框、选中的行）一并丢掉。
  return (
    <NetworkContext value={network}>
      <Outlet key={network.id} />
    </NetworkContext>
  )
}
