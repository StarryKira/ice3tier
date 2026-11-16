import { useQuery } from '@tanstack/react-query'
import { EllipsisVertical, KeyRound, Plus, Trash2 } from 'lucide-react'
import { useState } from 'react'
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
import { RelativeTime } from '@/components/status'
import { Table, Td, Th, Tr } from '@/components/table'
import { Button } from '@/components/ui/button'
import { Card } from '@/components/ui/card'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import { Field } from '@/components/ui/field'
import { Input } from '@/components/ui/input'
import { t } from '@/i18n'
import { listenerUsage } from '@/lib/commands'
import { useDocumentTitle } from '@/lib/useDocumentTitle'
import { parseOptionalPositiveInt, validateTitle } from '@/lib/validate'
import { networkPath, useNetwork } from '@/pages/networks/context'

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
  const [touched, setTouched] = useState(false)
  const maxParsed = parseOptionalPositiveInt(max)
  const issue = useAction(network.id, {
    mutationFn: () =>
      api.createCredential(network.id, { name: name.trim(), max_concurrent: maxParsed.value ?? undefined }),
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
      dirty={name !== '' || max !== ''}
      submitLabel={c.submit}
      pending={issue.isPending}
      error={formError(failure)}
      onSubmit={() => {
        setTouched(true)
        if (nameProblem !== null || maxParsed.error !== null) return
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
      <p className="text-[13px] text-muted-foreground">{c.scope}</p>
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

  const revoke = useAction(network.id, {
    mutationFn: (credential: Credential) => api.deleteCredential(network.id, credential.id),
    success: (_data, credential) => c.revoked(credential.name),
  })

  const onRevoke = async (credential: Credential) => {
    const confirmed = await confirm({
      tone: 'danger',
      title: c.revokeTitle(credential.name),
      body: c.revokeBody,
      confirmLabel: c.revokeConfirm,
    })
    if (confirmed) revoke.mutate(credential)
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
      <Card>
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
                  <Th numeric>{c.colMax}</Th>
                  <Th>{c.colCreated}</Th>
                  <Th className="w-12">
                    <span className="sr-only">{t.common.moreActions}</span>
                  </Th>
                </tr>
              </thead>
              <tbody>
                {credentials.isPending ? <SkeletonRows columns={4} rows={3} /> : null}
                {(credentials.data ?? []).map((credential) => (
                  <Tr key={credential.id} data-testid={`credential-row-${credential.name}`}>
                    <Td className="font-medium">{credential.name}</Td>
                    <Td numeric>
                      {credential.max_concurrent ?? <span className="text-muted-foreground">{t.common.unlimited}</span>}
                    </Td>
                    <Td className="text-[13px] text-muted-foreground">
                      <RelativeTime at={credential.created_at} />
                    </Td>
                    <Td className="pl-0 text-right">
                      <DropdownMenu>
                        <DropdownMenuTrigger asChild>
                          <Button variant="ghost" size="icon-sm" aria-label={t.common.actionsFor(credential.name)}>
                            <EllipsisVertical aria-hidden />
                          </Button>
                        </DropdownMenuTrigger>
                        <DropdownMenuContent>
                          <DropdownMenuItem danger onSelect={() => void onRevoke(credential)}>
                            <Trash2 aria-hidden />
                            {c.menuRevoke}
                          </DropdownMenuItem>
                        </DropdownMenuContent>
                      </DropdownMenu>
                    </Td>
                  </Tr>
                ))}
              </tbody>
            </Table>
            <p className="border-t px-4 py-2.5 text-[13px] text-muted-foreground">{c.tokenHidden}</p>
          </>
        )}
      </Card>

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
    </Page>
  )
}
