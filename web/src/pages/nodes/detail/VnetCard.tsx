import { ArrowDownUp, AtSign, Ban, CircleAlert, MemoryStick, Ruler } from 'lucide-react'

import { type NodeDetail } from '@/api/types'
import { DetailList, DetailRow } from '@/components/detail-list'
import { InfoTip } from '@/components/info-tip'
import { StatusText } from '@/components/status'
import { BoxCard, BoxCardHeader } from '@/components/ui/card'
import { Tooltip } from '@/components/ui/tooltip'
import { t } from '@/i18n'
import { formatBytes, formatNumber } from '@/lib/format'
import { droppedList, namesText, vnetCauseText, vnetState, vnetTone } from '@/lib/vnet'

/** 名字加一个悬停的说明。 */
function Label({ text, hint }: { text: string; hint?: string }) {
  return (
    <span className="inline-flex items-center gap-1.5">
      {text}
      {hint !== undefined ? <InfoTip>{hint}</InfoTip> : null}
    </span>
  )
}

/**
 * 虚拟网卡（0.1.26 起每个节点默认一块）：标题条右边是状态（装好了写 Ready，两种语言一样；关着、装不上、不支持、未知）。
 * 装好了：网卡、MTU（在节点上用 `i3t set mtu` 改，默认 1280）、名字怎么解析、这次运行以来的流量、丢掉的包（只有不是 0 时）。
 * 防火墙的规则不在这里：那是节点本机的事，在节点上用 `i3t network status` 看。
 * 装不上：原因，醒目。关着是本机有意的，不是毛病。没有报（旧版本、离线、还没上报）写"未知"，不当成关着。
 * 没有管理接口的开关：底边一句在节点上怎么开关。
 */
export function VnetCard({ node }: { node: NodeDetail }) {
  const v = t.nodes.detail.vnet
  const report = node.vnet ?? null
  const state = vnetState(report)
  const count = (value: number) => formatNumber(value)
  const names = namesText(report?.names)
  const dropped = droppedList(report?.dropped)
  const unknownWhy = !node.online
    ? v.unknownOffline
    : node.reported_secs_ago === null || node.reported_secs_ago === undefined
      ? v.unknownWaiting
      : v.unknownOld
  const how = state === 'off' ? v.howOn : state === 'installed' || state === 'failed' ? v.howOff : null

  return (
    <BoxCard data-testid="vnet" data-state={state}>
      <BoxCardHeader
        title={v.title}
        action={
          <StatusText tone={vnetTone(state)} className="text-[13px]">
            {v.state[state]}
          </StatusText>
        }
      />
      {report === null ? (
        <p className="px-5 py-3 text-[13px] text-muted-foreground">{unknownWhy}</p>
      ) : state === 'unknown' ? null : state === 'off' || state === 'unsupported' ? (
        <p className="px-5 py-3 text-[13px] text-muted-foreground">{state === 'off' ? v.offDesc : v.unsupportedDesc}</p>
      ) : state === 'failed' ? (
        <DetailList>
          <DetailRow icon={<CircleAlert />} label={v.cause} multiline>
            <span className="text-danger" data-testid="vnet-cause">
              {vnetCauseText(report.cause)}
            </span>
            <span className="block text-xs">{v.failedDesc}</span>
          </DetailRow>
        </DetailList>
      ) : (
        <DetailList>
          {report.device !== undefined ? (
            <DetailRow icon={<MemoryStick />} label={v.device}>
              <span className="font-mono text-[13px] text-foreground">{report.device}</span>
            </DetailRow>
          ) : null}
          {report.mtu !== undefined ? (
            <DetailRow icon={<Ruler />} label={<Label text={v.mtu} hint={v.mtuHint} />}>
              <span className="text-foreground tabular" data-testid="vnet-mtu">
                {v.mtuValue(report.mtu)}
              </span>
            </DetailRow>
          ) : null}
          {names !== null ? (
            <DetailRow
              icon={<AtSign />}
              label={
                <Label
                  text={v.names}
                  hint={report.names?.state === 'no_resolver' ? v.namesNoResolverHint : undefined}
                />
              }
              multiline={report.names?.state === 'failed'}
            >
              <span className={report.names?.state === 'failed' ? 'text-warn' : undefined}>{names}</span>
            </DetailRow>
          ) : null}
          <DetailRow icon={<ArrowDownUp />} label={v.traffic} multiline>
            <span className="block tabular">{v.sent(formatBytes(report.sent_bytes), count(report.sent_packets))}</span>
            <span className="block tabular">
              {v.received(formatBytes(report.received_bytes), count(report.received_packets))}
            </span>
          </DetailRow>
          {dropped.length > 0 ? (
            <DetailRow icon={<Ban />} label={v.dropped} multiline>
              <span className="inline-flex flex-wrap justify-end gap-x-2" data-testid="vnet-dropped">
                {dropped.map(({ key, count: value }) => (
                  <Tooltip key={key} content={v.dropHint[key]}>
                    <span tabIndex={0} className="rounded-sm tabular">
                      {v.dropValue(v.drop[key], count(value))}
                    </span>
                  </Tooltip>
                ))}
              </span>
            </DetailRow>
          ) : null}
        </DetailList>
      )}
      {how !== null ? (
        <p className="border-t px-5 py-2.5 text-xs text-muted-foreground" data-testid="vnet-how">
          {how}
        </p>
      ) : null}
    </BoxCard>
  )
}
