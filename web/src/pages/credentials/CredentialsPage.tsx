import { useQuery } from '@tanstack/react-query'
import { Ban, EllipsisVertical, Eraser, KeyRound, Plus, SlidersHorizontal, Trash2 } from 'lucide-react'
import { Fragment, useState } from 'react'
import { Link } from 'react-router'

import { fieldError, formError, formFailure } from '@/api/formErrors'
import { api } from '@/api/endpoints'
import { q, useAction } from '@/api/queries'
import { type CreatedCredential, type Credential, type Network } from '@/api/types'
import { useConfirm } from '@/components/confirm'
import { CodeBlock } from '@/components/copy'
import { FormDialog } from '@/components/form-dialog'
import { nodeName } from '@/lib/nodes'
import { Notice } from '@/components/notice'
import { Page, PageHeader } from '@/components/page'
import { SecretDialog } from '@/components/secret-dialog'
import { EmptyState, ErrorState, SkeletonRows } from '@/components/states'
import { RelativeTime, StatusText } from '@/components/status'
import { Table, TableNote, Td, Th, Tr } from '@/components/table'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { Field } from '@/components/ui/field'
import { Input } from '@/components/ui/input'
import { Tooltip } from '@/components/ui/tooltip'
import { BytesInput, UsageBar } from '@/components/usage'
import { t } from '@/i18n'
import { listenerUsage } from '@/lib/commands'
import { type ByteUnit, bytesToField, formatBytes } from '@/lib/format'
import { FOCUSED_ROW, useFocusRow } from '@/lib/useFocusRow'
import { useDocumentTitle } from '@/lib/useDocumentTitle'
import { cn } from '@/lib/utils'
import { parseOptionalBytes, parseOptionalPositiveInt, validateTitle } from '@/lib/validate'
import { networkPath, useNetwork } from '@/pages/networks/context'

import { credentialPrefix, credentialView } from './credentialState'

const c = t.credentials

interface IssueDialogProps {
  network: Network
  open: boolean
  onClose: () => void
  onIssued: (credential: CreatedCredential) => void
}

function IssueDialog({ network, open, onClose, onIssued }: IssueDialogProps) {
  const [name, setName] = useState('')
  const [max, setMax] = useState('')
  const [bytes, setBytes] = useState<{ value: string; unit: ByteUnit }>({ value: '', unit: 'GB' })
  const [touched, setTouched] = useState(false)
  const maxParsed = parseOptionalPositiveInt(max)
  const bytesParsed = parseOptionalBytes(bytes.value, bytes.unit)
  const issue = useAction(network.id, {
    mutationFn: () =>
      api.createCredential(network.id, {
        name: name.trim(),
        max_concurrent: maxParsed.value ?? undefined,
        max_bytes: bytesParsed.value ?? undefined,
      }),
    toastError: false,
  })
  const failure = formFailure(issue.error, { name_taken: 'name' } as const)
  const nameProblem = validateTitle(name, true)

  return (
    <FormDialog
      open={open}
      onClose={onClose}
      title={c.createTitle}
      size="sm"
      dirty={name !== '' || max !== '' || bytes.value !== ''}
      submitLabel={c.submit}
      pending={issue.isPending}
      error={formError(failure)}
      onSubmit={() => {
        setTouched(true)
        if (nameProblem !== null || maxParsed.error !== null || bytesParsed.error !== null) return
        issue.mutate(undefined, { onSuccess: onIssued })
      }}
    >
      <Field
        label={c.nameLabel}
        hint={c.nameHint}
        error={(touched ? nameProblem : null) ?? fieldError(failure, 'name')}
      >
        {(props) => (
          <Input
            {...props}
            name="name"
            value={name}
            autoFocus
            onChange={(event) => {
              setName(event.target.value)
              issue.reset()
            }}
          />
        )}
      </Field>
      <Field label={c.maxLabel} hint={c.maxHint} optional={t.common.optional} error={touched ? maxParsed.error : null}>
        {(props) => (
          <Input
            {...props}
            name="max"
            inputMode="numeric"
            className="w-28 tabular"
            value={max}
            onChange={(event) => setMax(event.target.value)}
          />
        )}
      </Field>
      <Field
        label={c.maxBytesLabel}
        hint={c.maxBytesHint}
        optional={t.common.optional}
        error={touched ? bytesParsed.error : null}
      >
        {(props) => (
          <BytesInput
            {...props}
            value={bytes.value}
            unit={bytes.unit}
            unitLabel={c.unitLabel}
            onChange={(value, unit) => setBytes({ value, unit })}
          />
        )}
      </Field>
      <p className="text-[13px] text-muted-foreground">{c.scope}</p>
    </FormDialog>
  )
}

/**
 * 改上限：不用重新签发令牌。只提交改过的那一项：没动的流量上限原样放回去会被四舍五入。
 * 新的流量上限比已用的少时它马上停用，比已用的多而它正用完着时它恢复；这两件事在保存之前就说出来。
 */
function LimitsDialog({
  network,
  credential,
  onClose,
}: {
  network: Network
  credential: Credential
  onClose: () => void
}) {
  const initialBytes = bytesToField(credential.max_bytes)
  const [max, setMax] = useState(credential.max_concurrent?.toString() ?? '')
  const [bytes, setBytes] = useState(initialBytes)
  const [touched, setTouched] = useState(false)
  const maxParsed = parseOptionalPositiveInt(max)
  const bytesParsed = parseOptionalBytes(bytes.value, bytes.unit)
  const maxChanged = max.trim() !== (credential.max_concurrent?.toString() ?? '')
  const bytesChanged = bytes.value.trim() !== initialBytes.value || bytes.unit !== initialBytes.unit
  const save = useAction(network.id, {
    mutationFn: () =>
      api.updateCredential(network.id, credential.id, {
        ...(maxChanged ? { max_concurrent: maxParsed.value } : {}),
        ...(bytesChanged ? { max_bytes: bytesParsed.value } : {}),
      }),
    success: c.edited(credential.name),
    toastError: false,
  })

  const used = credential.usage.bytes
  const nextLimit = bytesChanged ? bytesParsed.value : credential.max_bytes
  const willExhaust = bytesParsed.error === null && nextLimit !== null && nextLimit <= used
  const willRestore = credential.state === 'exhausted' && bytesParsed.error === null && !willExhaust

  return (
    <FormDialog
      open
      onClose={onClose}
      title={c.editTitle(credential.name)}
      description={c.editLead}
      size="sm"
      dirty={maxChanged || bytesChanged}
      requireDirty
      submitLabel={c.editSubmit}
      pending={save.isPending}
      error={save.isError ? formError(formFailure(save.error, {})) : null}
      onSubmit={() => {
        setTouched(true)
        if (maxParsed.error !== null || bytesParsed.error !== null) return
        save.mutate(undefined, { onSuccess: onClose })
      }}
    >
      <Field label={c.maxLabel} hint={c.maxHint} optional={t.common.optional} error={touched ? maxParsed.error : null}>
        {(props) => (
          <Input
            {...props}
            name="max"
            inputMode="numeric"
            className="w-28 tabular"
            placeholder={t.common.unlimited}
            value={max}
            autoFocus
            onChange={(event) => {
              setMax(event.target.value)
              save.reset()
            }}
          />
        )}
      </Field>
      <Field
        label={c.maxBytesLabel}
        hint={c.maxBytesHint}
        optional={t.common.optional}
        error={touched ? bytesParsed.error : null}
      >
        {(props) => (
          <BytesInput
            {...props}
            value={bytes.value}
            unit={bytes.unit}
            unitLabel={c.unitLabel}
            onChange={(value, unit) => {
              setBytes({ value, unit })
              save.reset()
            }}
          />
        )}
      </Field>
      <p className="text-[13px] text-muted-foreground">{c.editUsed(formatBytes(used))}</p>
      {bytesChanged && willExhaust ? <Notice tone="warn">{c.editWillExhaust}</Notice> : null}
      {willRestore && (bytesChanged || nextLimit === null) ? <Notice>{c.editWillRestore}</Notice> : null}
    </FormDialog>
  )
}

/** 凭据签发之后的"下一步"：如果网络里已经有要求凭据的监听器，例子里的地址和端口用真实的那个。 */
function Usage({ network, token }: { network: Network; token: string }) {
  const listeners = useQuery(q.listeners(network.id))
  const nodes = useQuery(q.nodes(network.id))
  const listener = (listeners.data ?? []).find((candidate) => candidate.require_credential)
  // 例子里用真实的值：网络里有出口就写它的虚拟 IP；监听器固定了出口时用户名不起作用，留空。
  const exit = (nodes.data ?? []).find((node) => node.exit !== null)?.virtual_ip ?? c.usageExitPlaceholder
  const usage =
    listener === undefined
      ? ''
      : listenerUsage(listener, {
          exit: listener.fixed_exit !== null ? '' : exit,
          credential: token,
          hostPlaceholder: c.usageHostPlaceholder,
        })

  return (
    <div>
      <h3 className="text-sm font-semibold">{c.usageTitle}</h3>
      {listener === undefined ? (
        <div className="mt-2 space-y-3">
          <Notice>{c.usageNoListener}</Notice>
          <Button asChild size="sm">
            <Link to={networkPath(network.id, 'listeners')}>{c.newListener}</Link>
          </Button>
        </div>
      ) : (
        <>
          <p className="mt-1 text-[13px] text-muted-foreground">
            {c.usageReal(nodeName(nodes.data ?? [], listener.node))}
          </p>
          <CodeBlock className="mt-2" copy={usage}>
            {usage}
          </CodeBlock>
        </>
      )}
    </div>
  )
}

export function CredentialsPage() {
  useDocumentTitle(c.title)
  const network = useNetwork()
  const confirm = useConfirm()
  const credentials = useQuery(q.credentials(network.id))
  const listeners = useQuery(q.listeners(network.id))
  const [issuing, setIssuing] = useState(false)
  const [issued, setIssued] = useState<CreatedCredential | null>(null)
  const [editing, setEditing] = useState<Credential | null>(null)
  const focus = useFocusRow(credentials.data)

  // DELETE 第一次是吊销，对已经吊销的再删一次才彻底删掉：同一个接口，界面上是两个动作。
  const remove = useAction(network.id, {
    mutationFn: (credential: Credential) => api.deleteCredential(network.id, credential.id),
    success: (_data, credential) =>
      credential.state === 'revoked' ? c.removed(credential.name) : c.revoked(credential.name),
  })
  const reset = useAction(network.id, {
    mutationFn: (credential: Credential) => api.updateCredential(network.id, credential.id, { reset_usage: true }),
    success: (_data, credential) => c.resetDone(credential.name),
  })

  const onRevoke = async (credential: Credential) => {
    const confirmed = await confirm({
      tone: 'danger',
      title: c.revokeTitle(credential.name),
      body: (
        <>
          <p>{c.revokeBody}</p>
          <p>{c.revokeKeeps}</p>
        </>
      ),
      confirmLabel: c.revokeConfirm,
    })
    if (confirmed) remove.mutate(credential)
  }

  const onRemove = async (credential: Credential) => {
    const confirmed = await confirm({
      title: c.removeTitle(credential.name),
      body: c.removeBody(credential.name),
      confirmLabel: c.removeConfirm,
    })
    if (confirmed) remove.mutate(credential)
  }

  const onReset = async (credential: Credential) => {
    const confirmed = await confirm({
      title: c.resetTitle(credential.name),
      body: (
        <>
          <p>{c.resetBody(formatBytes(credential.usage.bytes))}</p>
          {credential.state === 'exhausted' ? <p>{c.resetRestores}</p> : null}
        </>
      ),
      confirmLabel: c.resetConfirm,
    })
    if (confirmed) reset.mutate(credential)
  }

  const requiring = (listeners.data ?? []).filter((listener) => listener.require_credential).length
  const empty = credentials.isSuccess && credentials.data.length === 0

  return (
    <Page>
      <PageHeader
        title={c.title}
        description={c.description}
        actions={
          empty ? undefined : (
            <Button variant="primary" onClick={() => setIssuing(true)}>
              <Plus aria-hidden />
              {c.issue}
            </Button>
          )
        }
      />
      {focus.missing ? (
        <Notice className="mb-4" tone="warn">
          {c.focusMissing(focus.focus ?? 0)}
        </Notice>
      ) : null}
      <div>
        {credentials.isError ? (
          <ErrorState title={c.loadFailed} error={credentials.error} onRetry={() => void credentials.refetch()} />
        ) : empty ? (
          <EmptyState
            icon={KeyRound}
            title={c.emptyTitle}
            description={requiring > 0 ? c.emptyWithListener(requiring) : c.emptyNoListener}
            action={
              // 还没有对外的监听器时凭据用不上：签发只是次要的入口，先指给监听器。
              requiring > 0 ? (
                <Button variant="primary" onClick={() => setIssuing(true)}>
                  <Plus aria-hidden />
                  {c.issue}
                </Button>
              ) : (
                <>
                  <Button onClick={() => setIssuing(true)}>{c.issue}</Button>
                  <Button asChild variant="primary">
                    <Link to={networkPath(network.id, 'listeners')}>{c.goListeners}</Link>
                  </Button>
                </>
              )
            }
          />
        ) : (
          <>
            <Table>
              <thead>
                <tr>
                  <Th>{c.colName}</Th>
                  <Th>{c.colState}</Th>
                  <Th numeric>
                    <Tooltip content={c.colConnectionsTip}>
                      <span
                        tabIndex={0}
                        className="cursor-help rounded-sm underline decoration-dotted underline-offset-4"
                      >
                        {c.colConnections}
                      </span>
                    </Tooltip>
                  </Th>
                  <Th className="w-56">{c.colTraffic}</Th>
                  <Th>{c.colLastUsed}</Th>
                  <Th className="w-12">
                    <span className="sr-only">{t.common.moreActions}</span>
                  </Th>
                </tr>
              </thead>
              <tbody>
                {credentials.isPending ? <SkeletonRows columns={6} rows={3} /> : null}
                {(credentials.data ?? []).map((credential) => {
                  const view = credentialView(credential)
                  const prefix = credentialPrefix(credential.prefix)
                  const attention = credential.state === 'exhausted'
                  const focused = focus.focus === credential.id
                  return (
                    <Fragment key={credential.id}>
                      <Tr
                        ref={focus.rowRef(credential.id)}
                        dimmed={view.revoked}
                        className={cn(attention && 'border-b-0', focused && FOCUSED_ROW)}
                        data-testid={`credential-row-${credential.name}`}
                        data-state={credential.state}
                      >
                        <Td>
                          <span className="font-medium">{credential.name}</span>
                          {prefix !== null ? (
                            <Tooltip content={c.prefixTip}>
                              <span
                                tabIndex={0}
                                className="mt-0.5 block w-fit rounded-sm font-mono text-xs text-muted-foreground"
                              >
                                {prefix}
                              </span>
                            </Tooltip>
                          ) : null}
                        </Td>
                        <Td data-keep>
                          <Tooltip content={attention ? undefined : (view.detail ?? undefined)}>
                            <span tabIndex={view.detail !== null && !attention ? 0 : undefined} className="rounded-sm">
                              <StatusText tone={view.tone} className="text-[13px] whitespace-nowrap">
                                {view.label}
                              </StatusText>
                            </span>
                          </Tooltip>
                          {credential.revoked_at !== null ? (
                            <span className="mt-0.5 block text-xs text-muted-foreground">
                              {c.revokedAt}
                              <RelativeTime at={credential.revoked_at} />
                            </span>
                          ) : null}
                        </Td>
                        <Td numeric className="text-[13px]">
                          {view.connections}
                          {credential.max_concurrent === null ? (
                            <span className="block text-xs text-muted-foreground">{t.common.unlimited}</span>
                          ) : null}
                        </Td>
                        <Td data-keep={view.revoked ? true : undefined}>
                          <UsageBar view={view.traffic} label={`${credential.name} ${c.colTraffic}`} />
                          {credential.max_bytes === null ? (
                            <span className="block text-xs text-muted-foreground">{t.common.unlimited}</span>
                          ) : null}
                        </Td>
                        <Td className="text-[13px] text-muted-foreground">
                          {credential.usage.last_used_at === null ? (
                            c.neverUsed
                          ) : (
                            <RelativeTime at={credential.usage.last_used_at} />
                          )}
                        </Td>
                        <Td className="pl-0 text-right" data-keep>
                          <DropdownMenu>
                            <DropdownMenuTrigger asChild>
                              <Button variant="ghost" size="icon-sm" aria-label={t.common.actionsFor(credential.name)}>
                                <EllipsisVertical aria-hidden />
                              </Button>
                            </DropdownMenuTrigger>
                            <DropdownMenuContent>
                              <DropdownMenuItem disabled={!view.editable} onSelect={() => setEditing(credential)}>
                                <SlidersHorizontal aria-hidden />
                                <span>
                                  {c.menuEdit}
                                  {view.editable ? null : (
                                    <span className="block text-xs text-muted-foreground">{c.revokedNoEdit}</span>
                                  )}
                                </span>
                              </DropdownMenuItem>
                              <DropdownMenuItem
                                disabled={!view.editable || credential.usage.bytes === 0}
                                onSelect={() => void onReset(credential)}
                              >
                                <Eraser aria-hidden />
                                {c.menuReset}
                              </DropdownMenuItem>
                              <DropdownMenuSeparator />
                              {view.deleteStep === 'revoke' ? (
                                <DropdownMenuItem danger onSelect={() => void onRevoke(credential)}>
                                  <Ban aria-hidden />
                                  {c.menuRevoke}
                                </DropdownMenuItem>
                              ) : (
                                <DropdownMenuItem danger onSelect={() => void onRemove(credential)}>
                                  <Trash2 aria-hidden />
                                  {c.menuRemove}
                                </DropdownMenuItem>
                              )}
                            </DropdownMenuContent>
                          </DropdownMenu>
                        </Td>
                      </Tr>
                      {/* 用完了要人处理：原因和下一步单独占一行，是一句完整的话。 */}
                      {attention ? (
                        <tr className={cn('border-b last:border-b-0', focused && FOCUSED_ROW)}>
                          <td colSpan={6} className="px-3 pt-0 pb-3 text-[13px] leading-snug text-muted-foreground">
                            {view.detail}
                          </td>
                        </tr>
                      ) : null}
                    </Fragment>
                  )
                })}
              </tbody>
            </Table>
            <TableNote>{c.tokenHidden}</TableNote>
          </>
        )}
      </div>

      {issuing ? (
        <IssueDialog
          network={network}
          open
          onClose={() => setIssuing(false)}
          onIssued={(credential) => {
            setIssuing(false)
            setIssued(credential)
          }}
        />
      ) : null}
      {issued !== null ? (
        <SecretDialog
          open
          title={c.secretTitle(issued.name)}
          warning={c.secretWarning}
          warningRest={c.secretWarningRest}
          secret={issued.token}
          secretLabel={c.secretLabel}
          copyLabel={c.copySecret}
          closeLabel={c.savedClose}
          onClose={() => setIssued(null)}
        >
          <Usage network={network} token={issued.token} />
        </SecretDialog>
      ) : null}
      {editing !== null ? (
        <LimitsDialog key={editing.id} network={network} credential={editing} onClose={() => setEditing(null)} />
      ) : null}
    </Page>
  )
}
