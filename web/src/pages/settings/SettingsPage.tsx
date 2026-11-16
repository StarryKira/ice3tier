import { useQuery } from '@tanstack/react-query'
import { History, LogOut, Search } from 'lucide-react'
import { useMemo, useState } from 'react'
import { Link, Navigate, NavLink, useNavigate, useParams } from 'react-router'

import { q } from '@/api/queries'
import { session } from '@/auth/session'
import { CodeBlock, CopyText } from '@/components/copy'
import { InfoRow, Page, PageHeader } from '@/components/page'
import { EmptyState, ErrorState, NoMatches, SkeletonBlock, SkeletonRows } from '@/components/states'
import { RelativeTime } from '@/components/status'
import { Table, Td, Th, Tr } from '@/components/table'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card, CardBody, CardHeader } from '@/components/ui/card'
import { NativeSelect } from '@/components/ui/input'
import { t } from '@/i18n'
import { useDocumentTitle } from '@/lib/useDocumentTitle'
import { cn } from '@/lib/utils'
import { networkPath } from '@/pages/networks/context'
import { PlannedContent } from '@/pages/planned/PlannedPage'
import { PLANNED } from '@/pages/planned/planned'

const s = t.settings

const TABS = [
  { id: 'session', label: s.tabSession, planned: false },
  { id: 'updates', label: PLANNED.updates.title, planned: true },
  { id: 'log', label: s.tabLog, planned: false },
  { id: 'about', label: s.tabAbout, planned: false },
] as const
type TabId = (typeof TABS)[number]['id']

function SessionTab() {
  const navigate = useNavigate()
  return (
    <div className="space-y-5">
      <Card>
        <CardHeader title={s.session.title} />
        <CardBody className="space-y-3">
          <p className="text-sm">{s.session.signedIn}</p>
          <p className="text-[13px] text-muted-foreground">{s.session.oneAdmin}</p>
          <p className="text-[13px] text-muted-foreground">{s.session.stored}</p>
          <Button
            onClick={() => {
              session.signOut('manual')
              void navigate('/login', { replace: true })
            }}
          >
            <LogOut aria-hidden />
            {s.session.signOut}
          </Button>
        </CardBody>
      </Card>
      <Card>
        <CardHeader title={s.session.resetTitle} />
        <CardBody className="space-y-3">
          <p className="text-[13px] text-muted-foreground">{s.session.resetBody}</p>
          <CodeBlock copy={t.signIn.lostCommand}>{t.signIn.lostCommand}</CodeBlock>
        </CardBody>
      </Card>
    </div>
  )
}

/** 一次取多少条操作记录。接口一次最多给一千条。 */
const LOG_STEPS = [100, 300, 1000] as const

function LogTab() {
  const [step, setStep] = useState(0)
  const limit = LOG_STEPS[step] ?? 100
  const log = useQuery({ ...q.log(limit), placeholderData: (previous) => previous })
  const networks = useQuery(q.networks())
  const [search, setSearch] = useState('')
  const [network, setNetwork] = useState('all')
  const [kind, setKind] = useState('all')

  const entries = useMemo(() => log.data ?? [], [log.data])
  const rows = useMemo(() => {
    const needle = search.trim().toLowerCase()
    return entries.filter((entry) => {
      if (network === 'global' && entry.network !== null) return false
      if (network !== 'all' && network !== 'global' && entry.network !== Number(network)) return false
      if (kind !== 'all' && !entry.action.startsWith(`${kind}.`)) return false
      if (
        needle !== '' &&
        !`${entry.detail} ${s.log.actions[entry.action] ?? entry.action}`.toLowerCase().includes(needle)
      )
        return false
      return true
    })
  }, [entries, search, network, kind])

  const filtered = search.trim() !== '' || network !== 'all' || kind !== 'all'
  const clear = () => {
    setSearch('')
    setNetwork('all')
    setKind('all')
  }

  const networkLabel = (id: number | null) => {
    if (id === null) return <span className="text-muted-foreground">{s.log.global}</span>
    const found = networks.data?.find((candidate) => candidate.id === id)
    return found === undefined ? (
      <span className="text-muted-foreground">{s.log.deletedNetwork(id)}</span>
    ) : (
      <Link to={networkPath(found.id)} className="rounded-sm hover:text-primary hover:underline">
        {found.name}
      </Link>
    )
  }

  return (
    <Card>
      <CardHeader title={s.log.title} description={s.log.description} />
      {log.isError && log.data === undefined ? (
        <ErrorState title={s.log.loadFailed} error={log.error} onRetry={() => void log.refetch()} />
      ) : log.isSuccess && entries.length === 0 ? (
        <EmptyState icon={History} title={s.log.emptyTitle} />
      ) : (
        <>
          <div className="flex flex-wrap items-center gap-2.5 border-b px-4 py-3">
            <div className="relative w-64">
              <Search
                className="pointer-events-none absolute top-1/2 left-2.5 size-4 -translate-y-1/2 text-muted-foreground"
                aria-hidden
              />
              <input
                type="search"
                value={search}
                onChange={(event) => setSearch(event.target.value)}
                placeholder={s.log.searchPlaceholder}
                aria-label={s.log.searchPlaceholder}
                autoComplete="off"
                className="h-9 w-full rounded-md border border-input bg-card pr-3 pl-8.5 text-sm placeholder:text-muted-foreground/80"
              />
            </div>
            <NativeSelect
              value={network}
              onChange={(event) => setNetwork(event.target.value)}
              aria-label={s.log.filterNetwork}
              className="w-40"
            >
              <option value="all">{s.log.allNetworks}</option>
              {(networks.data ?? []).map((candidate) => (
                <option key={candidate.id} value={candidate.id}>
                  {candidate.name}
                </option>
              ))}
              <option value="global">{s.log.global}</option>
            </NativeSelect>
            <NativeSelect
              value={kind}
              onChange={(event) => setKind(event.target.value)}
              aria-label={s.log.filterKind}
              className="w-36"
            >
              <option value="all">{s.log.allKinds}</option>
              {Object.entries(s.log.kinds).map(([id, label]) => (
                <option key={id} value={id}>
                  {label}
                </option>
              ))}
            </NativeSelect>
          </div>
          <Table>
            <thead>
              <tr>
                <Th className="w-36">{s.log.colTime}</Th>
                <Th className="w-28">{s.log.colNetwork}</Th>
                <Th className="w-52">{s.log.colAction}</Th>
                <Th>{s.log.colDetail}</Th>
              </tr>
            </thead>
            <tbody>
              {log.isPending ? <SkeletonRows columns={4} rows={6} /> : null}
              {rows.map((entry) => (
                <Tr key={entry.id}>
                  <Td className="text-[13px] whitespace-nowrap text-muted-foreground">
                    <RelativeTime at={entry.at} />
                  </Td>
                  <Td className="text-[13px]">{networkLabel(entry.network)}</Td>
                  <Td className="text-[13px] font-medium">
                    {s.log.actions[entry.action] ?? (
                      <code className="font-mono text-xs font-normal">{entry.action}</code>
                    )}
                  </Td>
                  <Td className="text-[13px] text-muted-foreground">{entry.detail}</Td>
                </Tr>
              ))}
            </tbody>
          </Table>
          {log.isSuccess && rows.length === 0 ? <NoMatches message={s.log.noMatches} onClear={clear} /> : null}
          <footer className="flex items-center justify-between gap-4 border-t px-4 py-2.5 text-[13px] text-muted-foreground">
            <span className="tabular">
              {filtered ? s.log.showing(rows.length, entries.length) : s.log.showing(entries.length, entries.length)}
            </span>
            {entries.length >= limit && step < LOG_STEPS.length - 1 ? (
              <Button size="sm" pending={log.isFetching} onClick={() => setStep(step + 1)}>
                {s.log.loadMore}
              </Button>
            ) : entries.length >= limit ? (
              <span>{s.log.noMore}</span>
            ) : null}
          </footer>
        </>
      )}
    </Card>
  )
}

function AboutTab() {
  const status = useQuery(q.status())
  return (
    <Card>
      <CardHeader title={s.about.title} />
      {status.isError ? (
        <ErrorState title={s.about.loadFailed} error={status.error} onRetry={() => void status.refetch()} />
      ) : status.data === undefined ? (
        <SkeletonBlock lines={5} />
      ) : (
        <CardBody className="py-2">
          <dl className="divide-y">
            <InfoRow label={s.about.version}>
              <span className="tabular">{status.data.version}</span>
            </InfoRow>
            <InfoRow label={s.about.publicAddr}>
              <CopyText value={status.data.public_addr} />
              <span className="mt-0.5 block text-[13px] text-muted-foreground">{s.about.publicAddrHint}</span>
            </InfoRow>
            <InfoRow label={s.about.key}>
              <CopyText value={status.data.controller_key} wrap />
              <span className="mt-0.5 block text-[13px] text-muted-foreground">{s.about.keyHint}</span>
            </InfoRow>
            <InfoRow label={s.about.totals}>
              <span className="tabular">
                {s.about.totalsValue(status.data.networks, status.data.nodes, status.data.online)}
              </span>
            </InfoRow>
            <InfoRow label={s.about.backup}>{s.about.backupBody}</InfoRow>
            <InfoRow label={s.about.offline}>{s.about.offlineBody}</InfoRow>
          </dl>
        </CardBody>
      )}
    </Card>
  )
}

/** 全局设置。左侧竖排的分区，每个分区有自己的地址。 */
export function SettingsPage() {
  const params = useParams()
  const tab = TABS.find((candidate) => candidate.id === params.tab)
  useDocumentTitle(tab === undefined ? s.title : `${tab.label} · ${s.title}`)
  if (tab === undefined) return <Navigate to="/settings/session" replace />
  const active: TabId = tab.id

  return (
    <Page>
      <PageHeader title={s.title} description={s.description} />
      <div className="grid grid-cols-[11rem_minmax(0,1fr)] items-start gap-6">
        <nav aria-label={s.tabsLabel}>
          <ul className="space-y-0.5">
            {TABS.map((candidate) => (
              <li key={candidate.id}>
                <NavLink
                  to={`/settings/${candidate.id}`}
                  className={({ isActive }) =>
                    cn(
                      'flex h-8.5 items-center justify-between gap-2 rounded-md px-2.5 text-sm',
                      isActive ? 'bg-accent font-medium text-accent-foreground' : 'text-foreground/85 hover:bg-muted',
                    )
                  }
                >
                  {candidate.label}
                  {candidate.planned ? <Badge tone="outline">{t.common.planned}</Badge> : null}
                </NavLink>
              </li>
            ))}
          </ul>
        </nav>
        <div className="min-w-0">
          {active === 'session' ? <SessionTab /> : null}
          {active === 'updates' ? <PlannedContent id="updates" /> : null}
          {active === 'log' ? <LogTab /> : null}
          {active === 'about' ? <AboutTab /> : null}
        </div>
      </div>
    </Page>
  )
}
