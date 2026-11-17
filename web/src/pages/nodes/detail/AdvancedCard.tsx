import { useQuery } from '@tanstack/react-query'
import { useId } from 'react'
import { Link } from 'react-router'

import { api } from '@/api/endpoints'
import { q, useAction } from '@/api/queries'
import { type Network, type NodeDetail, type NodeId, type Reach, type RoutingMode, type UpdateNode } from '@/api/types'
import { BoxCard, BoxCardHeader } from '@/components/ui/card'
import { NativeSelect } from '@/components/ui/input'
import { Switch } from '@/components/ui/switch'
import { t } from '@/i18n'
import { MAX_PINNED_RELAYS } from '@/lib/product'

import { RelayPicker } from './RelayPicker'
import { SettingRow } from './SettingRow'

const d = t.nodes.detail

const routingLabel = (mode: RoutingMode): string =>
  mode === 'direct_first' ? d.routingDirectFirst : d.routingLowestCost
const reachLabel = (reach: Reach): string =>
  reach === 'any' ? d.reachAny : reach === 'direct_only' ? d.reachDirectOnly : d.reachRelayOnly

/** "跟随网络"在下拉里的值。 */
const INHERIT = 'inherit'

/** 高级设置，在概览上一直展开。页面上的单个控件：改了立即生效。 */
export function AdvancedCard({ network, node }: { network: Network; node: NodeDetail }) {
  const nodes = useQuery(q.nodes(network.id))
  const routingId = useId()
  const reachId = useId()
  const dialId = useId()
  const dialHintId = useId()
  const pinnedId = useId()
  const autoUpdateId = useId()
  const autoUpdateHintId = useId()
  // 全局的自动更新关着时，节点自己的开关开着也不会自动更新：说明里写出来，并给去设置的链接。
  const overview = useQuery(q.update())
  const globalOff = overview.data?.auto_update === false
  const save = useAction(network.id, {
    mutationFn: (body: UpdateNode) => api.updateNode(network.id, node.id, body),
    success: d.settingSaved,
  })

  const setPinned = (pinned: NodeId[]) => save.mutate({ pinned_relays: pinned })

  return (
    <BoxCard>
      <BoxCardHeader title={d.advanced} />
      <div className="divide-y">
        <SettingRow label={d.routingLabel} hint={d.routingHint} htmlFor={routingId}>
          <NativeSelect
            id={routingId}
            className="w-56"
            value={node.routing ?? INHERIT}
            disabled={save.isPending}
            onChange={(event) =>
              save.mutate({ routing: event.target.value === INHERIT ? null : (event.target.value as RoutingMode) })
            }
          >
            <option value={INHERIT}>{d.routingInherit(routingLabel(network.routing))}</option>
            <option value="direct_first">{d.routingDirectFirst}</option>
            <option value="lowest_cost">{d.routingLowestCost}</option>
          </NativeSelect>
        </SettingRow>

        <SettingRow label={d.reachLabel} hint={d.reachHint} htmlFor={reachId}>
          <NativeSelect
            id={reachId}
            className="w-56"
            value={node.reach ?? INHERIT}
            disabled={save.isPending}
            onChange={(event) =>
              save.mutate({ reach: event.target.value === INHERIT ? null : (event.target.value as Reach) })
            }
          >
            <option value={INHERIT}>{d.reachInherit(reachLabel(network.reach))}</option>
            <option value="any">{d.reachAny}</option>
            <option value="direct_only">{d.reachDirectOnly}</option>
            <option value="relay_only">{d.reachRelayOnly}</option>
          </NativeSelect>
        </SettingRow>

        <SettingRow label={d.dialOutOnly} description={d.dialOutOnlyHint} descriptionId={dialHintId} htmlFor={dialId}>
          <Switch
            id={dialId}
            aria-describedby={dialHintId}
            checked={node.dial_out_only}
            disabled={save.isPending}
            onCheckedChange={(checked) => save.mutate({ dial_out_only: checked })}
          />
        </SettingRow>

        <SettingRow label={d.pinnedLabel} hint={d.pinnedHint(MAX_PINNED_RELAYS)} htmlFor={pinnedId}>
          <RelayPicker
            id={pinnedId}
            nodes={nodes.data ?? []}
            self={node.id}
            value={node.pinned_relays}
            onChange={setPinned}
            busy={save.isPending}
          />
        </SettingRow>

        {/* 0.1.12 起的 controller 才有这个开关；更早的没有这个字段，不显示。 */}
        {node.auto_update !== undefined ? (
          <SettingRow
            label={d.autoUpdate}
            description={
              globalOff ? (
                <>
                  {d.autoUpdateGlobalOff}{' '}
                  <Link to="/settings/updates" className="text-primary hover:underline">
                    {d.autoUpdateSettings}
                  </Link>
                </>
              ) : (
                d.autoUpdateHint
              )
            }
            descriptionId={autoUpdateHintId}
            htmlFor={autoUpdateId}
          >
            <Switch
              id={autoUpdateId}
              aria-describedby={autoUpdateHintId}
              checked={node.auto_update}
              disabled={save.isPending}
              onCheckedChange={(checked) => save.mutate({ auto_update: checked })}
            />
          </SettingRow>
        ) : null}
      </div>
    </BoxCard>
  )
}
