import { useQuery } from '@tanstack/react-query'
import { RadioTower, X } from 'lucide-react'
import { type ReactNode, useId } from 'react'

import { api } from '@/api/endpoints'
import { q, useAction } from '@/api/queries'
import { type Network, type NodeDetail, type NodeId, type Reach, type RoutingMode, type UpdateNode } from '@/api/types'
import { Combobox } from '@/components/combobox'
import { InfoTip } from '@/components/info-tip'
import { OnlineDot } from '@/components/status'
import { Badge } from '@/components/ui/badge'
import { BoxCard, BoxCardHeader } from '@/components/ui/card'
import { NativeSelect } from '@/components/ui/input'
import { Switch } from '@/components/ui/switch'
import { t } from '@/i18n'
import { nodeName } from '@/lib/nodes'
import { MAX_PINNED_RELAYS } from '@/lib/product'

const d = t.nodes.detail

const routingLabel = (mode: RoutingMode): string =>
  mode === 'direct_first' ? d.routingDirectFirst : d.routingLowestCost
const reachLabel = (reach: Reach): string =>
  reach === 'any' ? d.reachAny : reach === 'direct_only' ? d.reachDirectOnly : d.reachRelayOnly

/** "跟随网络"在下拉里的值。 */
const INHERIT = 'inherit'

interface SettingRowProps {
  label: string
  /** 悬停在 ⓘ 上看的说明。 */
  hint?: string
  /** 直接写在名字下面的说明：不看就不懂这一项时用它，不用 `hint`。控件用 `aria-describedby` 指向 `descriptionId`。 */
  description?: string
  descriptionId?: string
  /** 名字是哪个控件的 `<label>`；不给时名字是一段带 `labelId` 的文字（控件用 `aria-labelledby` 指向它）。 */
  htmlFor?: string
  labelId?: string
  /** 控件放在名字下面（可以选好几个的那种），而不是右边。 */
  stacked?: boolean
  children: ReactNode
}

/** 一项设置：左边是名字和悬停的说明，右边是控件。行和信息卡一样高、一样的字。 */
function SettingRow({
  label,
  hint,
  description,
  descriptionId,
  htmlFor,
  labelId,
  stacked = false,
  children,
}: SettingRowProps) {
  const name = (
    <div className="min-w-0">
      <div className="flex items-center gap-1.5 text-sm">
        {htmlFor !== undefined ? <label htmlFor={htmlFor}>{label}</label> : <span id={labelId}>{label}</span>}
        {hint !== undefined ? <InfoTip>{hint}</InfoTip> : null}
      </div>
      {description !== undefined ? (
        <p id={descriptionId} className="mt-0.5 text-[13px] text-muted-foreground">
          {description}
        </p>
      ) : null}
    </div>
  )
  if (stacked) {
    return (
      <div className="px-5 py-3">
        <div className="flex min-h-7 items-center">{name}</div>
        <div className="mt-2">{children}</div>
      </div>
    )
  }
  return (
    <div className="flex min-h-14 items-center justify-between gap-4 px-5 py-2.5">
      {name}
      <div className="shrink-0">{children}</div>
    </div>
  )
}

/** 高级设置，在概览上一直展开。页面上的单个控件：改了立即生效。 */
export function AdvancedCard({ network, node }: { network: Network; node: NodeDetail }) {
  const nodes = useQuery(q.nodes(network.id))
  const routingId = useId()
  const reachId = useId()
  const dialId = useId()
  const dialHintId = useId()
  const pinnedId = useId()
  const save = useAction(network.id, {
    mutationFn: (body: UpdateNode) => api.updateNode(network.id, node.id, body),
    success: d.settingSaved,
  })

  const relays = (nodes.data ?? []).filter((other) => other.relay !== null && other.id !== node.id)
  const available = relays.filter((relay) => !node.pinned_relays.includes(relay.id))
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

        <SettingRow label={d.pinnedLabel} hint={d.pinnedHint(MAX_PINNED_RELAYS)} labelId={pinnedId} stacked>
          <ul aria-labelledby={pinnedId} className="flex flex-wrap items-center gap-1.5">
            {node.pinned_relays.map((id) => {
              const relay = (nodes.data ?? []).find((other) => other.id === id)
              const name = nodeName(nodes.data ?? [], id)
              return (
                <li
                  key={id}
                  className="inline-flex h-8 items-center gap-1.5 rounded-md bg-muted pr-1 pl-2 text-[13px] [&>svg]:size-3.5"
                >
                  <RadioTower className="shrink-0 text-muted-foreground" aria-hidden />
                  {name}
                  {relay?.relay == null ? <Badge tone="warn">{d.pinnedNotRelay}</Badge> : null}
                  <button
                    type="button"
                    disabled={save.isPending}
                    onClick={() => setPinned(node.pinned_relays.filter((other) => other !== id))}
                    aria-label={d.pinnedRemove(name)}
                    className="flex size-5 items-center justify-center rounded text-muted-foreground hover:bg-foreground/10 hover:text-foreground"
                  >
                    <X className="size-3.5" aria-hidden />
                  </button>
                </li>
              )
            })}
            {node.pinned_relays.length < MAX_PINNED_RELAYS ? (
              <li className="w-56">
                <Combobox<NodeId>
                  value={null}
                  onChange={(id) => setPinned([...node.pinned_relays, id])}
                  options={available.map((relay) => ({
                    value: relay.id,
                    label: relay.name,
                    hint: relay.relay?.region === '' ? relay.virtual_ip : relay.relay?.region,
                    leading: <OnlineDot online={relay.online} />,
                  }))}
                  placeholder={d.pinnedAdd}
                  searchPlaceholder={t.select.nodeSearch}
                  emptyText={relays.length === 0 ? d.pinnedNone : t.select.nodeEmpty}
                  disabled={save.isPending}
                  aria-label={d.pinnedAdd}
                />
              </li>
            ) : null}
          </ul>
        </SettingRow>
      </div>
    </BoxCard>
  )
}
