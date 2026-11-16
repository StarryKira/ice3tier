import { type ListenView } from '@/lib/listenState'
import { t } from '@/i18n'

import { StatusText } from './status'
import { Tooltip } from './ui/tooltip'
import { TrafficText } from './usage'

/**
 * 一个监听的状态格：状态的圆点和字，正在监听时下面是连接数和流量。
 * 原因和下一步是一整句话，由调用方放在这一行下面，不挤在格子里。
 */
export function ListenStateCell({ view, udp = false }: { view: ListenView; udp?: boolean }) {
  const traffic = view.traffic
  return (
    <div className="text-[13px]">
      <Tooltip content={view.needsAttention ? undefined : (view.detail ?? undefined)}>
        <span tabIndex={view.detail !== null && !view.needsAttention ? 0 : undefined} className="rounded-sm">
          <StatusText tone={view.tone} className="whitespace-nowrap">
            {view.label}
          </StatusText>
        </span>
      </Tooltip>
      {traffic !== null ? (
        <span className="mt-0.5 block text-xs text-muted-foreground">
          <span className="block tabular">
            {udp ? t.listen.sessions(traffic.connections) : t.listen.connections(traffic.connections)}
          </span>
          <TrafficText up={traffic.bytes_up} down={traffic.bytes_down} tip={t.listen.trafficTip} />
        </span>
      ) : null}
    </div>
  )
}
