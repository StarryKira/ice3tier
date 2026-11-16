import { useQuery } from '@tanstack/react-query'
import { Plus, Ticket, Undo2 } from 'lucide-react'
import { useState } from 'react'

import { api } from '@/api/endpoints'
import { q, useAction } from '@/api/queries'
import { type EnrollTokenInfo } from '@/api/types'
import { useConfirm } from '@/components/confirm'
import { nodeName } from '@/lib/nodes'
import { Page, PageHeader } from '@/components/page'
import { EmptyState, ErrorState, SkeletonRows } from '@/components/states'
import { RelativeTime, StatusText } from '@/components/status'
import { Table, Td, Th, Tr } from '@/components/table'
import { TagChip } from '@/components/tags'
import { Button } from '@/components/ui/button'
import { Card } from '@/components/ui/card'
import { t } from '@/i18n'
import { LIVE_INTERVAL } from '@/lib/polling'
import { formatRemaining } from '@/lib/time'
import { useDocumentTitle } from '@/lib/useDocumentTitle'
import { useNow } from '@/lib/useNow'
import { useNetwork } from '@/pages/networks/context'
import { AddNodeDialog } from '@/pages/nodes/AddNodeDialog'

const k = t.tokens

/**
 * 注册令牌。放在侧栏底部的管理区：令牌是为了加节点而存在的配置，不是网络里的一个对象；主路径是"添加节点"。
 * "新建令牌"打开的就是"添加节点"对话框：单独一个令牌没有用，用户总是需要令牌和命令一起。
 */
export function TokensPage() {
  useDocumentTitle(k.title)
  const network = useNetwork()
  const confirm = useConfirm()
  const now = useNow()
  // 令牌随时可能被别处的机器用掉，跟着轮询。
  const tokens = useQuery({ ...q.tokens(network.id), refetchInterval: LIVE_INTERVAL })
  const nodes = useQuery(q.nodes(network.id))
  const [adding, setAdding] = useState(false)

  const revoke = useAction(network.id, {
    mutationFn: (token: EnrollTokenInfo) => api.deleteToken(network.id, token.id),
    success: (_data, token) => k.revoked(token.id),
  })

  const onRevoke = async (token: EnrollTokenInfo) => {
    const confirmed = await confirm({
      tone: 'danger',
      title: k.revokeTitle(token.id),
      body: k.revokeBody,
      confirmLabel: k.revokeConfirm,
    })
    if (confirmed) revoke.mutate(token)
  }

  const empty = tokens.isSuccess && tokens.data.length === 0

  return (
    <Page>
      <PageHeader
        title={k.title}
        description={k.description}
        actions={
          empty ? undefined : (
            <Button variant="primary" onClick={() => setAdding(true)}>
              <Plus aria-hidden />
              {k.create}
            </Button>
          )
        }
      />
      <Card>
        {tokens.isError ? (
          <ErrorState title={k.loadFailed} error={tokens.error} onRetry={() => void tokens.refetch()} />
        ) : empty ? (
          <EmptyState
            icon={Ticket}
            title={k.emptyTitle}
            description={k.emptyBody}
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
                  <Th className="w-28" />
                </tr>
              </thead>
              <tbody>
                {tokens.isPending ? <SkeletonRows columns={5} rows={2} /> : null}
                {(tokens.data ?? []).map((token) => {
                  const remaining = formatRemaining(token.expires_at, now)
                  const hasPreset = token.tags.length > 0 || token.virtual_ip !== null || token.successor_of !== null
                  return (
                    <Tr key={token.id} data-testid={`token-row-${token.id}`}>
                      <Td className="font-mono text-[13px] font-medium tabular">{k.tokenId(token.id)}</Td>
                      <Td>
                        {hasPreset ? (
                          <span className="inline-flex flex-wrap items-center gap-1.5 text-[13px]">
                            {token.tags.map((tag) => (
                              <TagChip key={tag} tag={tag} />
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
                      <Td className="text-[13px]">
                        {remaining === null ? (
                          <StatusText tone="idle">{k.expired}</StatusText>
                        ) : (
                          <StatusText tone="relay">{k.unused(remaining)}</StatusText>
                        )}
                      </Td>
                      <Td className="text-[13px] text-muted-foreground">
                        <RelativeTime at={token.created_at} />
                      </Td>
                      <Td className="text-right">
                        <Button
                          size="sm"
                          variant="danger-outline"
                          aria-label={k.revokeFor(token.id)}
                          onClick={() => void onRevoke(token)}
                        >
                          <Undo2 aria-hidden />
                          {k.revoke}
                        </Button>
                      </Td>
                    </Tr>
                  )
                })}
              </tbody>
            </Table>
            <p className="border-t px-4 py-2.5 text-[13px] text-muted-foreground">
              {k.textHidden} {k.usedNote}
            </p>
          </>
        )}
      </Card>
      <AddNodeDialog network={network} open={adding} onClose={() => setAdding(false)} />
    </Page>
  )
}
