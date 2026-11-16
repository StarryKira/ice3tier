import { Link } from 'react-router'

import { type NodeDetail } from '@/api/types'
import { InfoRow } from '@/components/page'
import { RelativeTime, StatusText } from '@/components/status'
import { ManualUpdate, UpdateStateView } from '@/components/update'
import { Card, CardBody, CardHeader } from '@/components/ui/card'
import { t } from '@/i18n'
import { selfUpdateBlock } from '@/lib/nodeUpdate'
import { useUpdateView } from '@/lib/useUpdateView'

const u = t.updates

/**
 * 节点详情的「更新」：现在的版本、可以更新到的版本、能不能自己更新（不能时写出做法）、
 * 更新进行到哪一步、下发的指令、最近一次失败（节点报的原因是原话，作为补充）。"立即更新"在页面顶上，这里不再放一个。
 */
export function UpdateTab({ node }: { node: NodeDetail }) {
  const view = useUpdateView(node)
  const block = selfUpdateBlock(node.update)
  const { update } = node

  return (
    <div className="grid grid-cols-1 items-start gap-x-14 gap-y-12 @4xl:grid-cols-[minmax(0,5fr)_minmax(0,6fr)]">
      <Card>
        <CardHeader title={u.detail.tab} />
        <CardBody>
          <dl>
            <InfoRow label={u.detail.current}>
              <span className="tabular">{node.version === '' ? '—' : node.version}</span>
              <span className="ml-2 text-[13px] text-muted-foreground">{node.platform}</span>
            </InfoRow>
            <InfoRow label={u.detail.available}>
              {update.available === null ? (
                <span className="text-muted-foreground">{u.detail.none}</span>
              ) : (
                <span className="font-medium tabular">{update.available}</span>
              )}
            </InfoRow>
            <InfoRow label={u.detail.state}>
              <div data-testid="node-update-state">
                <UpdateStateView view={view} />
              </div>
            </InfoRow>
            <InfoRow label={u.detail.order}>
              {update.ordered === null ? (
                <span className="text-muted-foreground">{u.detail.none}</span>
              ) : (
                u.detail.orderValue(update.ordered.version, update.ordered.timing)
              )}
            </InfoRow>
            <InfoRow label={u.detail.failure}>
              {update.failure === null ? (
                <span className="text-muted-foreground">{u.detail.none}</span>
              ) : (
                <span>
                  <StatusText tone="danger">{update.failure.version}</StatusText>{' '}
                  <RelativeTime at={update.failure.at} className="text-[13px] text-muted-foreground" />
                  <span className="mt-0.5 block font-mono text-xs break-words text-muted-foreground">
                    {update.failure.reason}
                  </span>
                </span>
              )}
            </InfoRow>
          </dl>
          <p className="pt-4 text-[13px]">
            <Link to="/settings/updates" className="text-primary hover:underline">
              {u.detail.goSettings}
            </Link>
          </p>
        </CardBody>
      </Card>

      <Card>
        <CardHeader title={u.detail.selfUpdate} />
        <CardBody data-testid="node-self-update">
          {block === null ? (
            <StatusText tone="ok">{u.blocker.able}</StatusText>
          ) : (
            <ManualUpdate block={block} platform={node.platform} />
          )}
        </CardBody>
      </Card>
    </div>
  )
}
