import { CircleArrowUp, ShieldAlert } from 'lucide-react'
import { useState } from 'react'

import { type NodeDetail } from '@/api/types'
import { DetailRow } from '@/components/detail-list'
import { ForceUpdateDialog } from '@/components/force-update'
import { IceStack } from '@/components/layout/BrandMark'
import { ManualUpdate, ReasonButton, UpdateStateView } from '@/components/update'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Dialog, DialogBody, DialogContent, DialogFooter } from '@/components/ui/dialog'
import { Tooltip } from '@/components/ui/tooltip'
import { t } from '@/i18n'
import { forceAbility, selfUpdateBlock, type UpdateKind } from '@/lib/nodeUpdate'
import { useUpdateView } from '@/lib/useUpdateView'

const u = t.updates

/** 版本下面要多写一行的情况：正在更新、在等时机、刚换上、上次失败了。有新版本时右边的按钮已经说了，不再写。 */
const SHOWN: readonly UpdateKind[] = ['in_progress', 'waiting', 'confirmed', 'failed']

/**
 * 概览信息卡上"版本"那一行：现在的版本，有新版本时右边一个升级按钮；正在更新、上次失败时版本下面写一行状态
 * （失败带节点报的原因）。
 */
export function VersionRow({ node }: { node: NodeDetail }) {
  const view = useUpdateView(node)
  const state = SHOWN.includes(view.kind)
  return (
    <DetailRow
      icon={<IceStack className="size-4" />}
      label={t.nodes.detail.version}
      multiline={state}
      actions={<UpgradeButton node={node} />}
    >
      <span className="tabular">{node.version === '' ? '—' : node.version}</span>
      {state ? (
        <div
          data-testid="node-update-state"
          className="mt-1 text-right whitespace-normal [&_[role=progressbar]]:ml-auto"
        >
          <UpdateStateView view={view} />
        </div>
      ) : null}
    </DetailRow>
  )
}

/**
 * 版本右边的"升级到 x"，有新版本时才有。安全更新（它的版本有已知的安全问题）是红色的，悬停说明。
 * 能自己更新的点了是确认框；不能自己更新的点了是手动更新的做法；离线、正在换程序时置灰，悬停写原因。
 */
function UpgradeButton({ node }: { node: NodeDetail }) {
  const [open, setOpen] = useState(false)
  const version = node.update.available
  const security = node.update.security === true
  // 有安全问题、却没有能装的版本（发布里没有它的平台的包等）：只能手动换，没有可以点的。
  if (version === null) {
    if (!security) return null
    return (
      <Tooltip content={u.detail.securityManual}>
        <span tabIndex={0} className="ml-2 inline-flex shrink-0 cursor-help rounded-md">
          <Badge tone="danger">
            <ShieldAlert aria-hidden className="size-3.5" />
            {u.detail.securityBadge}
          </Badge>
        </span>
      </Tooltip>
    )
  }

  const block = selfUpdateBlock(node.update)
  const ability = forceAbility(node)
  // 不能自己更新时按钮照样能点，点开是手动的做法；别的原因（离线、正在换）才置灰。
  const reason = block !== null || ability.ok ? null : ability.reason
  const button = (
    <ReasonButton
      reason={reason}
      size="sm"
      variant={security ? 'danger' : 'primary'}
      className="ml-2 shrink-0"
      onClick={() => setOpen(true)}
    >
      {security ? <ShieldAlert aria-hidden /> : <CircleArrowUp aria-hidden />}
      {u.detail.upgrade(version)}
    </ReasonButton>
  )

  return (
    <>
      {security && reason === null ? <Tooltip content={u.detail.securityTip}>{button}</Tooltip> : button}
      {open && block === null ? <ForceUpdateDialog node={node} onClose={() => setOpen(false)} /> : null}
      {open && block !== null ? (
        <Dialog open onOpenChange={(next) => (next ? undefined : setOpen(false))}>
          <DialogContent title={u.detail.manualTitle(node.name, version)} size="sm">
            <DialogBody data-testid="node-self-update">
              <ManualUpdate block={block} platform={node.platform} />
            </DialogBody>
            <DialogFooter>
              <Button onClick={() => setOpen(false)}>{t.common.close}</Button>
            </DialogFooter>
          </DialogContent>
        </Dialog>
      ) : null}
    </>
  )
}
