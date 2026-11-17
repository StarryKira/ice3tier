import { useQuery } from '@tanstack/react-query'
import { ArrowLeftRight, ArrowRight, Copy, Pencil, Plus, Trash2 } from 'lucide-react'
import { Fragment, useState } from 'react'
import { Link } from 'react-router'

import { api } from '@/api/endpoints'
import { q, useAction } from '@/api/queries'
import { type Forward, type ForwardInput } from '@/api/types'
import { useConfirm } from '@/components/confirm'
import { CodeBlock, CopyText } from '@/components/copy'
import { TrafficText } from '@/components/usage'
import { NodeLink } from '@/components/node'
import { canBeForwardTarget, forwardTargetBySwitch, nodeName } from '@/lib/nodes'
import { Notice } from '@/components/notice'
import { Page, PageHeader } from '@/components/page'
import { EmptyState, ErrorState, SkeletonRows } from '@/components/states'
import { StatusText } from '@/components/status'
import { Table, TableNote, Td, Th, Tr } from '@/components/table'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { DropdownMenuItem, DropdownMenuSeparator } from '@/components/ui/dropdown-menu'
import { Switch } from '@/components/ui/switch'
import { toast } from '@/components/ui/toast'
import { Tooltip } from '@/components/ui/tooltip'
import { RowActions } from '@/components/row-actions'
import { t } from '@/i18n'
import { copyText } from '@/lib/clipboard'
import { parseSocketAddr } from '@/lib/cidr'
import { declareCommand } from '@/lib/commands'
import { FOCUSED_ROW, useFocusRow } from '@/lib/useFocusRow'
import { useDocumentTitle } from '@/lib/useDocumentTitle'
import { cn } from '@/lib/utils'
import { networkPath, useNetwork } from '@/pages/networks/context'

import { ForwardDialog, type ListenScope } from './ForwardDialog'
import { forwardAddress, forwardLabel, forwardStatusView, type ForwardStatusView } from './status'

const f = t.forwards

type Editor = { forward: Forward | null; scope?: ListenScope } | null

function toInput(forward: Forward, enabled: boolean): ForwardInput {
  return {
    name: forward.name,
    protocol: forward.protocol,
    listen_node: forward.listen_node,
    bind: forward.bind,
    target_node: forward.target_node,
    target_addr: forward.target_addr,
    allow_from: forward.allow_from,
    enabled,
    // PUT 是整个替换：不写的字段回到默认值，所以开关的时候要把等级原样带上。
    class: forward.class,
  }
}

/** 监听那一格：节点、地址和协议。端口写 0 的显示节点挑定的那个。 */
function ListenCell({ forward, view }: { forward: Forward; view: ForwardStatusView }) {
  const port = parseSocketAddr(forward.bind)?.port
  const picked = view.listen?.picked ?? null
  return (
    <div className="mt-0.5 flex flex-wrap items-center gap-x-1.5 gap-y-0.5">
      {port === 0 && picked !== null && view.listen?.local != null ? (
        <span className="inline-flex items-center gap-1.5" data-testid={`forward-picked-${forward.id}`}>
          <CopyText value={view.listen.local} className="text-xs" />
          <Tooltip content={f.pickedHint}>
            <span tabIndex={0} className="cursor-help rounded-sm text-xs text-muted-foreground">
              {f.picked(picked)}
            </span>
          </Tooltip>
        </span>
      ) : port === 0 ? (
        <span className="text-xs text-muted-foreground">
          <code className="font-mono">{forward.bind}</code> · {f.pickedPending}
        </span>
      ) : (
        <CopyText value={forward.bind} className="text-xs text-muted-foreground" />
      )}
      <Badge className="text-[11px]">{forward.protocol === 'tcp' ? f.dialog.tcp : f.dialog.udp}</Badge>
    </div>
  )
}

export function ForwardsPage() {
  useDocumentTitle(f.title)
  const network = useNetwork()
  const confirm = useConfirm()
  const forwards = useQuery(q.forwards(network.id))
  const nodes = useQuery(q.nodes(network.id))
  const [editor, setEditor] = useState<Editor>(null)
  const focus = useFocusRow(forwards.data)

  // 启用和停用没有单独的接口：取出这条转发，把 enabled 改掉再整个放回去。
  const setEnabled = useAction(network.id, {
    mutationFn: ({ forward, enabled }: { forward: Forward; enabled: boolean }) =>
      api.replaceForward(network.id, forward.id, toInput(forward, enabled)),
    success: (_data, { forward, enabled }) =>
      enabled ? f.enabledToast(forwardLabel(forward)) : f.disabledToast(forwardLabel(forward)),
  })
  const remove = useAction(network.id, {
    mutationFn: (forward: Forward) => api.deleteForward(network.id, forward.id),
    success: (_data, forward) => f.deleted(forwardLabel(forward)),
  })

  const all = nodes.data ?? []
  const list = forwards.data ?? []
  const empty = forwards.isSuccess && list.length === 0
  const noNodes = nodes.isSuccess && all.length === 0
  const noTargets = nodes.isSuccess && all.length > 0 && !all.some((node) => canBeForwardTarget(node))
  const targetsBySwitch = all.some((node) => forwardTargetBySwitch(node))

  const onDelete = async (forward: Forward) => {
    const connections = forward.listen?.connections ?? 0
    const confirmed = await confirm({
      title: f.deleteTitle(forwardLabel(forward)),
      body: (
        <>
          <p>{f.deleteBody(nodeName(all, forward.listen_node), forward.bind)}</p>
          {connections > 0 ? <p>{f.deleteActive(connections)}</p> : null}
        </>
      ),
      confirmLabel: f.deleteConfirm,
    })
    if (confirmed) remove.mutate(forward)
  }

  return (
    <Page>
      <PageHeader
        title={f.title}
        description={f.subtitle}
        actions={
          empty ? undefined : (
            <Button variant="primary" onClick={() => setEditor({ forward: null })}>
              <Plus aria-hidden />
              {f.create}
            </Button>
          )
        }
      />
      {focus.missing ? (
        <Notice className="mb-4" tone="warn">
          {f.focusMissing(focus.focus ?? 0)}
        </Notice>
      ) : null}
      <div>
        {forwards.isError ? (
          <ErrorState title={f.loadFailed} error={forwards.error} onRetry={() => void forwards.refetch()} />
        ) : empty && noNodes ? (
          <EmptyState
            icon={ArrowLeftRight}
            title={f.emptyTitle}
            description={f.emptyNoNodes}
            action={
              <Button asChild variant="primary">
                <Link to={networkPath(network.id, 'nodes')}>{t.exits.goNodes}</Link>
              </Button>
            }
          />
        ) : empty ? (
          <EmptyState icon={ArrowLeftRight} title={f.emptyTitle}>
            <div className="mt-5 grid w-full max-w-2xl grid-cols-2 gap-3 text-left">
              {(
                [
                  { scope: 'self', title: f.exampleLocal, description: f.exampleLocalDesc },
                  { scope: 'open', title: f.examplePublish, description: f.examplePublishDesc },
                ] as const
              ).map((example) => (
                <button
                  key={example.scope}
                  type="button"
                  onClick={() => setEditor({ forward: null, scope: example.scope })}
                  className="rounded-lg border border-border-strong bg-card p-4 text-left transition-colors hover:border-foreground/30 hover:bg-muted/40"
                >
                  <span className="block text-sm font-medium">{example.title}</span>
                  <span className="mt-1 block text-[13px] text-muted-foreground">{example.description}</span>
                </button>
              ))}
            </div>
            <Button variant="primary" className="mt-5" onClick={() => setEditor({ forward: null })}>
              <Plus aria-hidden />
              {f.create}
            </Button>
            {noTargets ? (
              <div className="mt-6 w-full max-w-2xl text-left">
                <Notice tone="warn" title={f.noTargets}>
                  {targetsBySwitch ? (
                    f.noTargetsSwitchBody
                  ) : (
                    <>
                      {f.noTargetsBody}
                      <CodeBlock copy={declareCommand('forward')} className="mt-2 bg-card/70">
                        {declareCommand('forward')}
                      </CodeBlock>
                    </>
                  )}
                </Notice>
              </div>
            ) : null}
          </EmptyState>
        ) : (
          <>
            <Table>
              <thead>
                <tr>
                  <Th>{f.colName}</Th>
                  <Th>{f.colListen}</Th>
                  <Th className="w-6 px-0" />
                  <Th>{f.colTarget}</Th>
                  <Th>{f.colStatus}</Th>
                  <Th>{f.colUse}</Th>
                  <Th>{f.colEnabled}</Th>
                  <Th className="w-12">
                    <span className="sr-only">{t.common.moreActions}</span>
                  </Th>
                </tr>
              </thead>
              <tbody>
                {forwards.isPending ? <SkeletonRows columns={8} rows={3} /> : null}
                {list.map((forward) => {
                  const view = forwardStatusView(forward, all)
                  const label = forwardLabel(forward)
                  const blocked = forward.status === 'target_not_declared'
                  // 停用的那一行已经变淡、开关关着，不再另起一行说它。
                  const hasDetail = view.tone !== 'ok' && forward.status !== 'disabled'
                  const reported = view.needsAttention ? (view.listen?.reported ?? null) : null
                  const focused = focus.focus === forward.id
                  const traffic = view.listen?.traffic ?? null
                  return (
                    <Fragment key={forward.id}>
                      <Tr
                        ref={focus.rowRef(forward.id)}
                        dimmed={!forward.enabled}
                        className={cn(hasDetail && 'border-b-0', focused && FOCUSED_ROW)}
                        data-testid={`forward-row-${label}`}
                        data-status={forward.status}
                      >
                        <Td className="font-medium">
                          {forward.name === '' ? (
                            <span className="font-normal text-muted-foreground">{f.unnamed}</span>
                          ) : (
                            forward.name
                          )}
                        </Td>
                        <Td>
                          <NodeLink
                            network={network.id}
                            node={{ id: forward.listen_node, name: nodeName(all, forward.listen_node) }}
                          />
                          <ListenCell forward={forward} view={view} />
                        </Td>
                        <Td className="px-0 text-faint">
                          <ArrowRight className="size-4" aria-label={f.dialog.arrow} />
                        </Td>
                        <Td>
                          <NodeLink
                            network={network.id}
                            node={{ id: forward.target_node, name: nodeName(all, forward.target_node) }}
                          />
                          <div className="mt-0.5">
                            <CopyText value={forward.target_addr} className="text-xs text-muted-foreground" />
                          </div>
                        </Td>
                        <Td data-keep>
                          <StatusText tone={view.tone} className="text-[13px] whitespace-nowrap">
                            {view.label}
                          </StatusText>
                        </Td>
                        <Td className="text-[13px]">
                          {traffic === null ? (
                            <span className="text-muted-foreground">—</span>
                          ) : (
                            <>
                              <span className="block tabular">
                                {forward.protocol === 'udp'
                                  ? t.listen.sessions(traffic.connections)
                                  : t.listen.connections(traffic.connections)}
                              </span>
                              <TrafficText
                                up={traffic.bytes_up}
                                down={traffic.bytes_down}
                                tip={t.listen.trafficTip}
                                className="text-xs text-muted-foreground"
                              />
                            </>
                          )}
                        </Td>
                        <Td data-keep>
                          <Tooltip content={blocked ? f.toggleBlocked : undefined}>
                            <span tabIndex={blocked ? 0 : undefined} className="inline-flex rounded-full">
                              <Switch
                                aria-label={f.toggle(label)}
                                checked={forward.enabled}
                                disabled={
                                  blocked || (setEnabled.isPending && setEnabled.variables.forward.id === forward.id)
                                }
                                onCheckedChange={(enabled) => setEnabled.mutate({ forward, enabled })}
                              />
                            </span>
                          </Tooltip>
                        </Td>
                        <Td className="pl-0 text-right" data-keep>
                          <RowActions label={t.common.actionsFor(label)}>
                            <DropdownMenuItem onSelect={() => setEditor({ forward })}>
                              <Pencil aria-hidden />
                              {f.menuEdit}
                            </DropdownMenuItem>
                            <DropdownMenuItem
                              onSelect={() =>
                                void copyText(forwardAddress(forward, view)).then((ok) =>
                                  ok ? toast.success(f.addressCopied) : toast.error(t.common.copyFailed),
                                )
                              }
                            >
                              <Copy aria-hidden />
                              {f.menuCopy}
                            </DropdownMenuItem>
                            <DropdownMenuSeparator />
                            <DropdownMenuItem danger onSelect={() => void onDelete(forward)}>
                              <Trash2 aria-hidden />
                              {f.menuDelete}
                            </DropdownMenuItem>
                          </RowActions>
                        </Td>
                      </Tr>
                      {/* 原因和下一步单独占一行，是一句完整的话，不挤在状态那一格里。 */}
                      {hasDetail ? (
                        <tr className={cn('border-b last:border-b-0', focused && FOCUSED_ROW)}>
                          <td colSpan={8} className="px-3 pt-0 pb-3 text-[13px] leading-snug text-muted-foreground">
                            {view.detail}
                            {view.linkToRules ? (
                              <Link
                                to={networkPath(network.id, 'access')}
                                className="ml-2 rounded-sm text-primary underline-offset-4 hover:underline"
                              >
                                {f.viewRules}
                              </Link>
                            ) : null}
                            {reported !== null ? (
                              <span className="mt-0.5 block text-xs break-words">{reported}</span>
                            ) : null}
                          </td>
                        </tr>
                      ) : null}
                    </Fragment>
                  )
                })}
              </tbody>
            </Table>
            <TableNote>{f.statusCaveat}</TableNote>
          </>
        )}
      </div>

      {editor !== null ? (
        <ForwardDialog
          key={editor.forward?.id ?? `new-${editor.scope ?? ''}`}
          network={network}
          forward={editor.forward}
          scope={editor.scope}
          open
          onClose={() => setEditor(null)}
        />
      ) : null}
    </Page>
  )
}
