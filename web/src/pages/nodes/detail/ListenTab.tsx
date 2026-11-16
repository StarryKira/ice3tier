import { useQuery } from '@tanstack/react-query'
import { RadioTower } from 'lucide-react'
import { Fragment, type ReactNode } from 'react'
import { Link } from 'react-router'

import { q } from '@/api/queries'
import { type Network, type NodeDetail, type NodeForwardListen, type NodeListener } from '@/api/types'
import { CopyText } from '@/components/copy'
import { ListenStateCell } from '@/components/listen'
import { EmptyState, ErrorState, SkeletonRows } from '@/components/states'
import { Table, Td, Th, Tr } from '@/components/table'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card, CardHeader } from '@/components/ui/card'
import { Tooltip } from '@/components/ui/tooltip'
import { t } from '@/i18n'
import { listenView, type ListenView } from '@/lib/listenState'
import { formatAge } from '@/lib/time'
import { forwardLabel } from '@/pages/forwards/status'
import { PROTOCOL_LABEL } from '@/pages/listeners/describe'
import { networkPath } from '@/pages/networks/context'

const d = t.nodes.detail

function DetailRow({ view }: { view: ListenView }) {
  if (!view.needsAttention || view.detail === null) return null
  return (
    <tr className="border-b last:border-b-0">
      <td colSpan={4} className="px-4 pt-0 pb-3 text-[13px] leading-snug text-muted-foreground">
        {view.detail}
      </td>
    </tr>
  )
}

/** 地址那一格：端口是节点挑的时，配置里写的 0 后面跟着实际的那个。 */
function Address({ bind, view }: { bind: string; view: ListenView }) {
  if (view.picked !== null && view.local !== null) {
    return (
      <span className="inline-flex flex-wrap items-center gap-x-1.5">
        <CopyText value={view.local} />
        <span className="text-xs text-muted-foreground">{t.forwards.picked(view.picked)}</span>
      </span>
    )
  }
  return <CopyText value={bind} />
}

function Section({ title, action, children }: { title: string; action?: ReactNode; children: ReactNode }) {
  return (
    <Card>
      <CardHeader title={title} action={action} />
      {children}
    </Card>
  )
}

/**
 * 节点上开着的端口：controller 下发的代理监听器、它按本机配置文件自己开的回环入口、端口转发的监听一侧。
 * 都取自它最近的上报，所以离线的节点这里没有内容。
 */
export function ListenTab({ network, node }: { network: Network; node: NodeDetail }) {
  const report = useQuery(q.nodeListeners(network.id, node.id))
  const forwards = useQuery(q.forwards(network.id))
  const data = report.data

  const forwardName = (item: NodeForwardListen) => {
    const forward = forwards.data?.find((candidate) => candidate.id === item.id)
    return forward === undefined ? `#${item.id}` : forwardLabel(forward)
  }
  const viewOf = (item: NodeListener | NodeForwardListen) =>
    listenView(item, { node: node.name, bind: item.bind, online: true })

  return (
    <div className="space-y-3">
      <div className="flex flex-wrap items-baseline justify-between gap-x-6 gap-y-1">
        <p className="text-sm text-muted-foreground">{d.listenLead}</p>
        {data?.reported_secs_ago != null ? (
          <p className="text-[13px] text-muted-foreground" data-testid="listen-updated">
            {t.common.reportedAgo(formatAge(data.reported_secs_ago))}
          </p>
        ) : null}
      </div>
      {report.isError ? (
        <Card>
          <ErrorState title={d.listenLoadFailed} error={report.error} onRetry={() => void report.refetch()} />
        </Card>
      ) : data?.online === false ? (
        <Card>
          <EmptyState icon={RadioTower} title={d.listenOffline} />
        </Card>
      ) : data?.reported_secs_ago === null ? (
        <Card>
          <EmptyState icon={RadioTower} title={d.listenNotReported} />
        </Card>
      ) : data?.listeners.length === 0 && data.forwards.length === 0 ? (
        <Card>
          <EmptyState icon={RadioTower} title={d.listenEmpty} />
        </Card>
      ) : (
        <>
          <Section
            title={d.listenListeners}
            action={
              <Button asChild size="sm">
                <Link to={networkPath(network.id, 'listeners')}>{d.listenManage}</Link>
              </Button>
            }
          >
            <Table>
              <thead>
                <tr>
                  <Th className="w-[36%]">{d.colListenName}</Th>
                  <Th className="w-[34%]">{d.colListenBind}</Th>
                  <Th>{d.colListenState}</Th>
                  <Th className="w-0" />
                </tr>
              </thead>
              <tbody>
                {data === undefined ? <SkeletonRows columns={3} rows={2} /> : null}
                {(data?.listeners ?? []).map((item) => {
                  const view = viewOf(item)
                  return (
                    <Fragment key={item.id ?? `builtin-${item.name}`}>
                      <Tr
                        className={view.needsAttention ? 'border-b-0' : undefined}
                        data-testid={`node-listen-${item.name}`}
                      >
                        <Td>
                          <span className="font-medium">{item.name}</span>
                          <Badge className="ml-2">{PROTOCOL_LABEL[item.protocol]}</Badge>
                          {item.builtin ? (
                            <Tooltip content={t.listen.builtinTip}>
                              <Badge tone="outline" tabIndex={0} className="ml-1.5">
                                {t.listen.builtin}
                              </Badge>
                            </Tooltip>
                          ) : null}
                        </Td>
                        <Td>
                          <Address bind={item.bind} view={view} />
                        </Td>
                        <Td>
                          <ListenStateCell view={view} />
                        </Td>
                        <Td />
                      </Tr>
                      <DetailRow view={view} />
                    </Fragment>
                  )
                })}
              </tbody>
            </Table>
          </Section>

          <Section
            title={d.listenForwards}
            action={
              <Button asChild size="sm">
                <Link to={networkPath(network.id, 'forwards')}>{d.listenManage}</Link>
              </Button>
            }
          >
            {data?.forwards.length === 0 ? (
              <p className="px-5 py-4 text-sm text-muted-foreground">{d.listenForwardsEmpty}</p>
            ) : (
              <Table>
                <thead>
                  <tr>
                    <Th className="w-[36%]">{d.colListenName}</Th>
                    <Th className="w-[34%]">{d.colListenBind}</Th>
                    <Th>{d.colListenState}</Th>
                    <Th className="w-0" />
                  </tr>
                </thead>
                <tbody>
                  {data === undefined ? <SkeletonRows columns={3} rows={1} /> : null}
                  {(data?.forwards ?? []).map((item) => {
                    const view = viewOf(item)
                    return (
                      <Fragment key={item.id}>
                        <Tr
                          className={view.needsAttention ? 'border-b-0' : undefined}
                          data-testid={`node-forward-${item.id}`}
                        >
                          <Td>
                            <Link
                              to={`${networkPath(network.id, 'forwards')}?focus=${item.id}`}
                              className="rounded-sm font-medium hover:text-primary hover:underline"
                            >
                              {forwardName(item)}
                            </Link>
                            <Badge className="ml-2">
                              {item.protocol === 'tcp' ? t.forwards.dialog.tcp : t.forwards.dialog.udp}
                            </Badge>
                          </Td>
                          <Td>
                            <Address bind={item.bind} view={view} />
                          </Td>
                          <Td>
                            <ListenStateCell view={view} udp={item.protocol === 'udp'} />
                          </Td>
                          <Td />
                        </Tr>
                        <DetailRow view={view} />
                      </Fragment>
                    )
                  })}
                </tbody>
              </Table>
            )}
          </Section>
        </>
      )}
    </div>
  )
}
