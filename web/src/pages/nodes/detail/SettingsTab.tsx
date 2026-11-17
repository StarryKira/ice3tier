import { useQuery } from '@tanstack/react-query'
import { ChevronDown, X } from 'lucide-react'
import { useCallback, useId, useRef } from 'react'

import { api } from '@/api/endpoints'
import { q, useAction } from '@/api/queries'
import { type Network, type NodeDetail, type NodeId, type Reach, type RoutingMode, type UpdateNode } from '@/api/types'
import { Combobox } from '@/components/combobox'
import { InfoTip } from '@/components/info-tip'
import { OnlineDot } from '@/components/status'
import { Badge } from '@/components/ui/badge'
import { BoxCard, BoxCardBody, BoxCardHeader } from '@/components/ui/card'
import { NativeSelect } from '@/components/ui/input'
import { Switch } from '@/components/ui/switch'
import { t } from '@/i18n'
import { nodeName } from '@/lib/nodes'
import { MAX_PINNED_RELAYS } from '@/lib/product'

import { ExitRow, ForwardTargetRow, RelayRow } from './Capabilities'

const d = t.nodes.detail

const routingLabel = (mode: RoutingMode): string =>
  mode === 'direct_first' ? d.routingDirectFirst : d.routingLowestCost
const reachLabel = (reach: Reach): string =>
  reach === 'any' ? d.reachAny : reach === 'direct_only' ? d.reachDirectOnly : d.reachRelayOnly

/** "跟随网络"在下拉里的值。 */
const INHERIT = 'inherit'

interface TabProps {
  network: Network
  node: NodeDetail
}

/** 高级设置。页面上的单个控件：改了立即生效。每一项旁边一句话，说的是后果。 */
function AdvancedCard({ network, node }: TabProps) {
  const nodes = useQuery(q.nodes(network.id))
  const routingId = useId()
  const reachId = useId()
  const dialId = useId()
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
      <details className="group">
        <summary className="flex min-h-12 cursor-pointer list-none items-center justify-between gap-4 rounded-xl px-5 py-2.5 [&::-webkit-details-marker]:hidden">
          <span className="text-[15px] leading-6 font-semibold">{d.advanced}</span>
          <ChevronDown className="size-4 shrink-0 text-faint transition-transform group-open:rotate-180" aria-hidden />
        </summary>
        <div className="space-y-5 border-t px-5 py-5">
          <div className="grid grid-cols-[minmax(0,1fr)_15rem] items-start gap-x-4 gap-y-1">
            <div className="flex h-9 items-center gap-1.5">
              <label htmlFor={routingId} className="text-sm font-medium">
                {d.routingLabel}
              </label>
              <InfoTip>{d.routingHint}</InfoTip>
            </div>
            <NativeSelect
              id={routingId}
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
          </div>

          <div className="grid grid-cols-[minmax(0,1fr)_15rem] items-start gap-x-4 gap-y-1">
            <div className="flex h-9 items-center gap-1.5">
              <label htmlFor={reachId} className="text-sm font-medium">
                {d.reachLabel}
              </label>
              <InfoTip>{d.reachHint}</InfoTip>
            </div>
            <NativeSelect
              id={reachId}
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
          </div>

          <div className="flex items-center justify-between gap-4">
            <div className="flex items-center gap-1.5">
              <label htmlFor={dialId} className="text-sm font-medium">
                {d.dialOutOnly}
              </label>
              <InfoTip>{d.dialOutOnlyHint}</InfoTip>
            </div>
            <Switch
              id={dialId}
              checked={node.dial_out_only}
              disabled={save.isPending}
              onCheckedChange={(checked) => save.mutate({ dial_out_only: checked })}
            />
          </div>

          <div>
            <p className="flex items-center gap-1.5">
              <span id={pinnedId} className="text-sm font-medium">
                {d.pinnedLabel}
              </span>
              <InfoTip>{d.pinnedHint(MAX_PINNED_RELAYS)}</InfoTip>
            </p>
            <ul aria-labelledby={pinnedId} className="mt-2 flex flex-wrap items-center gap-1.5">
              {node.pinned_relays.map((id) => {
                const relay = (nodes.data ?? []).find((other) => other.id === id)
                const name = nodeName(nodes.data ?? [], id)
                return (
                  <li key={id} className="inline-flex h-7 items-center gap-1 rounded-md bg-muted pr-1 pl-2.5 text-sm">
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
          </div>
        </div>
      </details>
    </BoxCard>
  )
}

interface SettingsTabProps extends TabProps {
  /** 能力卡片里有一行正在等节点上报声明。 */
  onWaiting: (waiting: boolean) => void
}

/** 设置：它承担什么（relay、出口、端口转发目标），和怎么选路、连谁。改了立即生效。 */
export function SettingsTab({ network, node, onWaiting }: SettingsTabProps) {
  // 三行各自报告是不是在等；有任何一行在等，页面就把轮询调勤。
  const waitingRows = useRef(new Set<string>())
  const report = useCallback(
    (key: string, value: boolean) => {
      if (value) waitingRows.current.add(key)
      else waitingRows.current.delete(key)
      onWaiting(waitingRows.current.size > 0)
    },
    [onWaiting],
  )
  const onRelay = useCallback((value: boolean) => report('relay', value), [report])
  const onExit = useCallback((value: boolean) => report('exit', value), [report])
  const onForward = useCallback((value: boolean) => report('forward', value), [report])

  return (
    // 开关离名字太远就对不上了：表单不铺满整个宽度。
    <div className="max-w-5xl space-y-5">
      <BoxCard>
        <BoxCardHeader title={d.capabilities} tip={d.withdrawnNote} />
        <BoxCardBody className="divide-y">
          <RelayRow network={network} node={node} onWaiting={onRelay} />
          <ExitRow network={network} node={node} onWaiting={onExit} />
          <ForwardTargetRow network={network} node={node} onWaiting={onForward} />
        </BoxCardBody>
      </BoxCard>

      <AdvancedCard network={network} node={node} />
    </div>
  )
}
