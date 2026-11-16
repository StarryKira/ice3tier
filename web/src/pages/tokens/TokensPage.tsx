import { useQuery } from '@tanstack/react-query'
import { Plus, Ticket, Trash2, Undo2 } from 'lucide-react'
import { useState } from 'react'
import { useSearchParams } from 'react-router'

import { api } from '@/api/endpoints'
import { q, useAction } from '@/api/queries'
import { type EnrollTokenInfo } from '@/api/types'
import { useConfirm } from '@/components/confirm'
import { GroupChip } from '@/components/groups'
import { NodeLink } from '@/components/node'
import { Notice } from '@/components/notice'
import { Page, PageHeader } from '@/components/page'
import { EmptyState, ErrorState, SkeletonRows } from '@/components/states'
import { RelativeTime, StatusText } from '@/components/status'
import { Table, TableNote, Td, Th, Tr } from '@/components/table'
import { Button } from '@/components/ui/button'
import { Segmented } from '@/components/ui/segmented'
import { Tooltip } from '@/components/ui/tooltip'
import { t } from '@/i18n'
import { nodeName } from '@/lib/nodes'
import { FOCUSED_ROW, useFocusRow } from '@/lib/useFocusRow'
import { useDocumentTitle } from '@/lib/useDocumentTitle'
import { useNow } from '@/lib/useNow'
import { cn } from '@/lib/utils'
import { useNetwork } from '@/pages/networks/context'
import { AddNodeDialog } from '@/pages/nodes/AddNodeDialog'

import { sortTokens, tokenLabel, tokenView } from './tokenState'

const k = t.tokens

type Show = 'pending' | 'all'

/**
 * 注册令牌。放在侧栏底部的管理区：令牌是为了加节点而存在的配置，不是网络里的一个对象；主路径是"添加节点"。
 * "新建令牌"打开的就是"添加节点"对话框：单独一个令牌没有用，用户总是需要令牌和命令一起。
 *
 * 默认只列还能用的；切到"全部"连最近 30 天里用掉的、过期的也列出，看得到是哪个节点用掉的。
 */
export function TokensPage() {
  useDocumentTitle(k.title)
  const network = useNetwork()
  const confirm = useConfirm()
  const now = useNow()
  const [params, setParams] = useSearchParams()
  const show: Show = params.get('show') === 'all' ? 'all' : 'pending'
  const tokens = useQuery(q.tokens(network.id, show === 'all'))
  const nodes = useQuery(q.nodes(network.id))
  const [adding, setAdding] = useState(false)
  const focus = useFocusRow(tokens.data)

  const remove = useAction(network.id, {
    mutationFn: (token: EnrollTokenInfo) => api.deleteToken(network.id, token.id),
    success: (_data, token) =>
      tokenView(token, now).removal === 'revoke' ? k.revoked(tokenLabel(token)) : k.forgotten(tokenLabel(token)),
  })

  const onRemove = async (token: EnrollTokenInfo) => {
    const label = tokenLabel(token)
    // 还能用的令牌吊销之后拿着它的机器就加入不了了，不能撤回；用掉的和过期的只是删掉一条记录。
    const confirmed =
      tokenView(token, now).removal === 'revoke'
        ? await confirm({
            tone: 'danger',
            title: k.revokeTitle(label),
            body: k.revokeBody,
            confirmLabel: k.revokeConfirm,
          })
        : await confirm({ title: k.forgetTitle(label), body: k.forgetBody, confirmLabel: k.forgetConfirm })
    if (confirmed) remove.mutate(token)
  }

  const setShow = (next: Show) => {
    const updated = new URLSearchParams(params)
    if (next === 'all') updated.set('show', 'all')
    else updated.delete('show')
    updated.delete('focus')
    setParams(updated, { replace: true })
  }

  const list = sortTokens(tokens.data ?? [])
  const empty = tokens.isSuccess && list.length === 0

  return (
    <Page>
      <PageHeader
        title={k.title}
        description={k.subtitle}
        actions={
          <Button variant="primary" onClick={() => setAdding(true)}>
            <Plus aria-hidden />
            {k.create}
          </Button>
        }
      />
      <div className="mb-4 flex flex-wrap items-center justify-between gap-3">
        <Segmented
          aria-label={k.showLabel}
          value={show}
          onChange={setShow}
          options={[
            { value: 'pending', label: k.showPending },
            { value: 'all', label: k.showAll },
          ]}
        />
        {show === 'all' ? <p className="text-[13px] text-muted-foreground">{k.historyNote}</p> : null}
      </div>
      {focus.missing ? (
        <Notice className="mb-4" tone="warn">
          {k.focusMissing(focus.focus ?? 0)}
        </Notice>
      ) : null}
      <div>
        {tokens.isError ? (
          <ErrorState title={k.loadFailed} error={tokens.error} onRetry={() => void tokens.refetch()} />
        ) : empty ? (
          <EmptyState
            icon={Ticket}
            title={show === 'all' ? k.emptyAllTitle : k.emptyTitle}
            action={
              <Button variant="primary" onClick={() => setAdding(true)}>
                <Plus aria-hidden />
                {k.addNode}
              </Button>
            }
          />
        ) : (
          <>
            <Table>
              <thead>
                <tr>
                  <Th>{k.colToken}</Th>
                  <Th>{k.colPreset}</Th>
                  <Th>{k.colStatus}</Th>
                  <Th>{k.colCreated}</Th>
                  <Th className="w-32" />
                </tr>
              </thead>
              <tbody>
                {tokens.isPending ? <SkeletonRows columns={5} rows={2} /> : null}
                {list.map((token) => {
                  const view = tokenView(token, now)
                  const label = tokenLabel(token)
                  const hasPreset = token.groups.length > 0 || token.virtual_ip !== null || token.successor_of !== null
                  return (
                    <Tr
                      key={token.id}
                      ref={focus.rowRef(token.id)}
                      dimmed={token.state !== 'pending'}
                      className={cn(focus.focus === token.id && FOCUSED_ROW)}
                      data-testid={`token-row-${token.id}`}
                      data-state={token.state}
                    >
                      <Td>
                        <Tooltip content={token.prefix === '' ? undefined : k.prefixTip}>
                          <span
                            tabIndex={token.prefix === '' ? undefined : 0}
                            className="rounded-sm font-mono text-[13px] font-medium tabular"
                          >
                            {label}
                          </span>
                        </Tooltip>
                      </Td>
                      <Td>
                        {hasPreset ? (
                          <span className="inline-flex flex-wrap items-center gap-1.5 text-[13px]">
                            {token.groups.map((group) => (
                              <GroupChip key={group.id} name={group.name} />
                            ))}
                            {token.virtual_ip !== null ? <span>{k.presetIp(token.virtual_ip)}</span> : null}
                            {token.successor_of !== null ? (
                              <span>{k.presetSuccessor(nodeName(nodes.data ?? [], token.successor_of))}</span>
                            ) : null}
                          </span>
                        ) : (
                          <span className="text-[13px] text-muted-foreground">{k.presetNone}</span>
                        )}
                      </Td>
                      <Td className="text-[13px]" data-keep>
                        <span className="inline-flex flex-wrap items-center gap-x-1.5">
                          <StatusText tone={view.tone}>{view.label}</StatusText>
                          {view.usedBy !== null ? (
                            <>
                              <span className="text-muted-foreground" aria-hidden>
                                {k.usedArrow}
                              </span>
                              <NodeLink
                                network={network.id}
                                node={view.usedBy}
                                online={nodes.data?.find((node) => node.id === view.usedBy?.id)?.online}
                              />
                            </>
                          ) : null}
                        </span>
                        {view.usedByDeleted ? (
                          <span className="block text-xs text-muted-foreground">{k.usedByDeleted}</span>
                        ) : null}
                        {view.usedAt !== null ? (
                          <RelativeTime at={view.usedAt} className="mt-0.5 block w-fit text-xs text-muted-foreground" />
                        ) : null}
                      </Td>
                      <Td className="text-[13px] text-muted-foreground">
                        <RelativeTime at={token.created_at} />
                      </Td>
                      <Td className="text-right" data-keep>
                        {view.removal === 'revoke' ? (
                          <Button
                            size="sm"
                            variant="outline"
                            aria-label={k.revokeFor(label)}
                            onClick={() => void onRemove(token)}
                          >
                            <Undo2 aria-hidden />
                            {k.revoke}
                          </Button>
                        ) : (
                          <Button
                            size="sm"
                            variant="ghost"
                            aria-label={k.forgetFor(label)}
                            onClick={() => void onRemove(token)}
                          >
                            <Trash2 aria-hidden />
                            {k.forget}
                          </Button>
                        )}
                      </Td>
                    </Tr>
                  )
                })}
              </tbody>
            </Table>
            <TableNote>{k.textHidden}</TableNote>
          </>
        )}
      </div>
      <AddNodeDialog network={network} open={adding} onClose={() => setAdding(false)} />
    </Page>
  )
}
