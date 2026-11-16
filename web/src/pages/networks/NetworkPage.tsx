import { useQuery } from '@tanstack/react-query'
import { Check, Plus } from 'lucide-react'
import { useId, useState } from 'react'
import { Link, useNavigate } from 'react-router'

import { fieldError, formError, formErrorDetail, formFailure } from '@/api/formErrors'
import { api } from '@/api/endpoints'
import { q, useAction } from '@/api/queries'
import { type Network, type Reach, type RoutingMode, type ServiceClass, type UpdateNetwork } from '@/api/types'
import { useConfirm } from '@/components/confirm'
import { CopyText } from '@/components/copy'
import { FormDialog } from '@/components/form-dialog'
import { InfoRow, Page, PageHeader } from '@/components/page'
import { RelativeTime } from '@/components/status'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card, CardBody, CardHeader } from '@/components/ui/card'
import { Field, FormError } from '@/components/ui/field'
import { Input, NativeSelect } from '@/components/ui/input'
import { RadioCards } from '@/components/ui/radio-cards'
import { ClassSelect } from '@/components/usage'
import { t } from '@/i18n'
import { cidrContains, formatCidr, parseCidr } from '@/lib/cidr'
import { MAX_RELAY_HOPS } from '@/lib/product'
import { useDocumentTitle } from '@/lib/useDocumentTitle'
import { cn } from '@/lib/utils'
import { validateNetworkCidr } from '@/lib/validate'

import { networkPath, useNetwork } from './context'
import { CreateNetworkDialog } from './CreateNetworkDialog'

const n = t.networks

/** 扩大地址段。有节点之后地址段只能扩大：新的必须把原来的完整地包含在内。 */
function ExpandDialog({ network, open, onClose }: { network: Network; open: boolean; onClose: () => void }) {
  const [value, setValue] = useState(network.cidr)
  const [touched, setTouched] = useState(false)
  const expand = useAction(network.id, {
    mutationFn: () => api.updateNetwork(network.id, { cidr: value.trim() }),
    success: (updated) => n.expanded(updated.cidr),
    toastError: false,
  })
  const failure = formFailure(expand.error, { range_overlaps: 'cidr', range_not_superset: 'cidr' } as const)

  const current = parseCidr(network.cidr)
  const next = parseCidr(value)
  const dirty = next !== null && current !== null && formatCidr(next) !== formatCidr(current)
  let problem = validateNetworkCidr(value)
  if (problem === null && next !== null && current !== null && !cidrContains(next, current))
    problem = n.expandNotSuperset(network.cidr)

  return (
    <FormDialog
      open={open}
      onClose={onClose}
      title={n.expandTitle(network.name)}
      description={n.expandLead(network.cidr)}
      size="sm"
      dirty={dirty}
      requireDirty
      submitLabel={n.expandSubmit}
      pending={expand.isPending}
      error={formError(failure)}
      errorDetail={formErrorDetail(failure)}
      onSubmit={() => {
        setTouched(true)
        if (problem === null) expand.mutate(undefined, { onSuccess: onClose })
      }}
    >
      <Field label={n.expandLabel} error={(touched || dirty ? problem : null) ?? fieldError(failure, 'cidr')}>
        {(props) => (
          <Input
            {...props}
            className="font-mono"
            value={value}
            autoFocus
            onChange={(event) => {
              setValue(event.target.value)
              expand.reset()
            }}
          />
        )}
      </Field>
    </FormDialog>
  )
}

/**
 * 网络的设置。多个字段一起改，所以是整站少数显式保存的表单：点"保存"才生效，没有改动时"保存"是灰的。
 */
function SettingsForm({ network }: { network: Network }) {
  const nodes = useQuery(q.nodes(network.id))
  const [routing, setRouting] = useState<RoutingMode>(network.routing)
  const [hops, setHops] = useState(network.max_relay_hops)
  const [reach, setReach] = useState<Reach>(network.reach)
  // `null` 和 `auto` 是一回事；表单里一律写成 `auto`。
  const [serviceClass, setServiceClass] = useState<ServiceClass>(network.class ?? 'auto')
  const routingLabel = useId()
  const reachLabel = useId()

  const save = useAction(network.id, {
    mutationFn: (body: UpdateNetwork) => api.updateNetwork(network.id, body),
    success: n.settingsSaved,
    toastError: false,
  })

  const dirty =
    routing !== network.routing ||
    hops !== network.max_relay_hops ||
    reach !== network.reach ||
    serviceClass !== (network.class ?? 'auto')
  const overriding = (nodes.data ?? []).filter((node) => node.routing !== null).length
  const reset = () => {
    setRouting(network.routing)
    setHops(network.max_relay_hops)
    setReach(network.reach)
    setServiceClass(network.class ?? 'auto')
    save.reset()
  }

  return (
    <form
      noValidate
      onSubmit={(event) => {
        event.preventDefault()
        if (dirty) save.mutate({ routing, max_relay_hops: hops, reach, class: serviceClass })
      }}
    >
      <CardBody className="space-y-7">
        <div>
          <p id={routingLabel} className={cn('text-[13px] font-medium', overriding === 0 && 'mb-2')}>
            {n.routingLabel}
          </p>
          {overriding > 0 ? (
            <p className="mb-2 text-[13px] text-muted-foreground">{n.routingOverridden(overriding)}</p>
          ) : null}
          <RadioCards
            aria-labelledby={routingLabel}
            value={routing}
            onChange={setRouting}
            columns={2}
            options={[
              { value: 'direct_first', title: n.routingDirectFirst, description: n.routingDirectFirstDesc },
              { value: 'lowest_cost', title: n.routingLowestCost, description: n.routingLowestCostDesc },
            ]}
          />
        </div>
        <Field label={n.hopsLabel} className="max-w-xs">
          {(props) => (
            <NativeSelect
              {...props}
              value={hops}
              onChange={(event) => setHops(Number(event.target.value))}
              className="w-32"
            >
              {Array.from(
                { length: MAX_RELAY_HOPS.max - MAX_RELAY_HOPS.min + 1 },
                (_, index) => MAX_RELAY_HOPS.min + index,
              ).map((value) => (
                <option key={value} value={value}>
                  {n.hops(value)}
                </option>
              ))}
            </NativeSelect>
          )}
        </Field>
        <div>
          <p id={reachLabel} className="mb-2 text-[13px] font-medium">
            {n.reachLabel}
          </p>
          <RadioCards
            aria-labelledby={reachLabel}
            value={reach}
            onChange={setReach}
            columns={3}
            options={[
              { value: 'any', title: n.reachAny, description: n.reachAnyDesc },
              { value: 'direct_only', title: n.reachDirectOnly, description: n.reachDirectOnlyDesc },
              { value: 'relay_only', title: n.reachRelayOnly, description: n.reachRelayOnlyDesc },
            ]}
          />
        </div>
        <Field label={n.classLabel} hint={n.classHint}>
          {(props) => (
            <ClassSelect
              {...props}
              value={serviceClass}
              inheritLabel={null}
              onChange={(value) => setServiceClass(value ?? 'auto')}
            />
          )}
        </Field>
        <FormError
          message={save.isError ? formError(formFailure(save.error, {})) : null}
          detail={save.isError ? formErrorDetail(formFailure(save.error, {})) : null}
        />
      </CardBody>
      <footer className="mt-8 flex items-center justify-end gap-2 border-t pt-4">
        <Button onClick={reset} disabled={!dirty || save.isPending}>
          {t.common.cancel}
        </Button>
        <Button type="submit" variant="primary" disabled={!dirty} pending={save.isPending}>
          {n.saveSettings}
        </Button>
      </footer>
    </form>
  )
}

/**
 * 删除网络：整站唯一需要输入名字的确认，因为它一次毁掉一批东西且无法撤回。
 * controller 只删空的网络，所以网络里还有节点时按钮是灰的，旁边写明要先做什么，不等提交了再报错。
 */
function DangerZone({ network }: { network: Network }) {
  const confirm = useConfirm()
  const navigate = useNavigate()
  const overview = useQuery(q.overview(network.id))
  const remove = useAction(null, {
    mutationFn: () => api.deleteNetwork(network.id),
    success: n.deleted(network.name),
  })

  const onDelete = async () => {
    const counts = overview.data
    const confirmed = await confirm({
      tone: 'danger',
      title: n.deleteTitle(network.name),
      body: counts === undefined ? n.dangerBody : n.deleteBody(counts.rules, counts.credentials, counts.pending_tokens),
      confirmLabel: n.deleteConfirm,
      typeToConfirm: network.name,
    })
    if (!confirmed) return
    remove.mutate(undefined, { onSuccess: () => void navigate('/', { replace: true }) })
  }

  return (
    <Card className="border-t pt-6">
      <div className="flex items-center justify-between gap-6">
        <div>
          <h2 className="text-base font-semibold">{n.dangerTitle}</h2>
          <p className="mt-0.5 text-[13px] text-muted-foreground">
            {network.nodes > 0 ? n.dangerNotEmpty(network.nodes) : n.dangerBody}
          </p>
          {network.nodes > 0 ? (
            <Link
              to={networkPath(network.id, 'nodes')}
              className="mt-1 inline-block rounded-sm text-[13px] text-primary underline-offset-4 hover:underline"
            >
              {n.viewNodes}
            </Link>
          ) : null}
        </div>
        <Button
          variant="danger-outline"
          disabled={network.nodes > 0}
          pending={remove.isPending}
          onClick={() => void onDelete()}
        >
          {n.deleteButton}
        </Button>
      </div>
    </Card>
  )
}

export function NetworkPage() {
  useDocumentTitle(n.title)
  const network = useNetwork()
  const networks = useQuery(q.networks())
  const [creating, setCreating] = useState(false)
  const [expanding, setExpanding] = useState(false)

  return (
    <Page className="@container">
      <PageHeader
        title={n.title}
        description={n.subtitle}
        actions={
          <Button variant="primary" onClick={() => setCreating(true)}>
            <Plus aria-hidden />
            {n.create}
          </Button>
        }
      />

      <section aria-label={n.listTitle} className="-mx-3 mb-12 divide-y border-y">
        {(networks.data ?? []).map((other) => {
          const current = other.id === network.id
          return (
            <Link
              key={other.id}
              to={networkPath(other.id, 'network')}
              aria-current={current ? 'true' : undefined}
              aria-label={current ? n.currentName(other.name) : n.switchTo(other.name)}
              className={cn(
                'grid grid-cols-[minmax(0,1fr)_10rem_6rem] items-center gap-4 px-3 py-3 transition-colors hover:bg-muted/50',
                current && 'bg-accent/50 hover:bg-accent/70',
              )}
            >
              <span className="flex min-w-0 items-center gap-2">
                <span className="truncate text-sm font-semibold">{other.name}</span>
                {current ? (
                  <Badge tone="accent">
                    <Check aria-hidden />
                    {n.current}
                  </Badge>
                ) : null}
              </span>
              <span className="font-mono text-[13px] text-muted-foreground">{other.cidr}</span>
              <span className="text-right text-[13px] text-muted-foreground tabular">{n.nodes(other.nodes)}</span>
            </Link>
          )
        })}
      </section>

      <div className="space-y-12">
        <Card>
          <CardHeader title={n.settingsTitle(network.name)} />
          <CardBody>
            <h3 className="sr-only">{n.basics}</h3>
            <dl>
              <InfoRow label={n.nameLabel}>
                <CopyText value={network.name} mono={false} />
              </InfoRow>
              <InfoRow label={n.domainLabel}>
                <CopyText value={network.domain} />
              </InfoRow>
              <InfoRow label={n.cidrLabel}>
                <span className="inline-flex flex-wrap items-center gap-3">
                  <CopyText value={network.cidr} />
                  <Button size="sm" variant="link" onClick={() => setExpanding(true)}>
                    {n.expand}
                  </Button>
                </span>
              </InfoRow>
              <InfoRow label={n.createdAt}>
                <RelativeTime at={network.created_at} />
              </InfoRow>
            </dl>
          </CardBody>
          <CardHeader title={n.routing} className="mt-10 mb-5" />
          {/* key：切换网络或者保存之后，表单从最新的设置重新开始。 */}
          <SettingsForm
            key={`${network.id}:${network.routing}:${network.max_relay_hops}:${network.reach}:${network.class ?? 'auto'}`}
            network={network}
          />
        </Card>

        <DangerZone network={network} />
      </div>

      <CreateNetworkDialog open={creating} onClose={() => setCreating(false)} />
      {expanding ? <ExpandDialog network={network} open onClose={() => setExpanding(false)} /> : null}
    </Page>
  )
}
