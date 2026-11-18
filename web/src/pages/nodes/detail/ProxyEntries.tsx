import { useQuery } from '@tanstack/react-query'
import { Pencil, Plus, Trash2 } from 'lucide-react'
import { useState } from 'react'
import { Link } from 'react-router'

import { api } from '@/api/endpoints'
import { fieldError, formError, formErrorDetail, formFailure } from '@/api/formErrors'
import { q, useAction } from '@/api/queries'
import { type Listener, type ListenerProtocol, type Network, type NodeDetail, type NodeId } from '@/api/types'
import { useConfirm } from '@/components/confirm'
import { FormDialog } from '@/components/form-dialog'
import { NodeLink, NodeSelect } from '@/components/node'
import { Notice } from '@/components/notice'
import { RowActions } from '@/components/row-actions'
import { SkeletonBlock } from '@/components/states'
import { StatusText } from '@/components/status'
import { Button } from '@/components/ui/button'
import { BoxCard, BoxCardHeader } from '@/components/ui/card'
import { DropdownMenuItem, DropdownMenuSeparator } from '@/components/ui/dropdown-menu'
import { Field } from '@/components/ui/field'
import { Input } from '@/components/ui/input'
import { Segmented } from '@/components/ui/segmented'
import { t } from '@/i18n'
import { formatSocketAddr, isLoopbackHost, parseIpv4, parseSocketAddr } from '@/lib/cidr'
import { formatBytes } from '@/lib/format'
import { listenView } from '@/lib/listenState'
import { nodeName } from '@/lib/nodes'
import { exitOf } from '@/lib/proxy'
import { validatePort } from '@/lib/validate'
import { networkPath } from '@/pages/networks/context'

const p = t.nodes.detail.proxy

const PROTOCOL: Record<ListenerProtocol, string> = { socks5: 'SOCKS5', http: 'HTTP' }

type Where = 'local' | 'all' | 'mesh' | 'custom'

/** 监听地址怎么选：本机（只有这台机器上的程序能用）、所有地址（局域网里的设备也能用）、虚拟 IP（组网里的节点能用）、自己填。 */
function whereOf(host: string, virtualIp: string | undefined): Where {
  if (isLoopbackHost(host)) return 'local'
  if (host === '0.0.0.0') return 'all'
  if (virtualIp !== undefined && host === virtualIp) return 'mesh'
  return 'custom'
}

interface ProxyDialogProps {
  network: Network
  /** 代理从这个出口出去。 */
  exit: NodeDetail
  /** 要改的那个；新建时是 `null`。 */
  listener: Listener | null
  onClose: () => void
}

/**
 * 添加或修改一个代理：开在哪台节点上、SOCKS5 还是 HTTP、监听哪个地址、端口。就这几项。
 * 改的时候其余的设置（名字、来源白名单、QoS 这些）原样留着。
 */
function ProxyDialog({ network, exit, listener, onClose }: ProxyDialogProps) {
  const nodes = useQuery(q.nodes(network.id))
  const all = nodes.data ?? []
  const parsed = listener === null ? null : parseSocketAddr(listener.bind)
  const [node, setNode] = useState<NodeId | null>(listener?.node ?? null)
  const host = all.find((candidate) => candidate.id === node)
  const [protocol, setProtocol] = useState<ListenerProtocol>(listener?.protocol ?? 'socks5')
  const [where, setWhere] = useState<Where>(parsed === null ? 'local' : whereOf(parsed.host, host?.virtual_ip))
  const [custom, setCustom] = useState(
    parsed !== null && whereOf(parsed.host, host?.virtual_ip) === 'custom' ? parsed.host : '',
  )
  const [port, setPort] = useState(parsed?.port.toString() ?? (protocol === 'http' ? '8080' : '1080'))
  const [touched, setTouched] = useState(false)
  const [dirty, setDirty] = useState(false)

  const address =
    where === 'local'
      ? '127.0.0.1'
      : where === 'all'
        ? '0.0.0.0'
        : where === 'mesh'
          ? (host?.virtual_ip ?? '')
          : custom.trim()
  const nodeProblem = node === null ? t.validate.pickNode : null
  const portProblem = validatePort(port)
  const addressProblem =
    where === 'custom' && parseIpv4(address) === null && !address.includes(':') ? t.validate.ipFormat : null
  const valid = nodeProblem === null && portProblem === null && addressProblem === null

  const save = useAction(network.id, {
    mutationFn: (input: Parameters<typeof api.createListener>[1]) =>
      listener === null ? api.createListener(network.id, input) : api.replaceListener(network.id, listener.id, input),
    success: () => (listener === null ? p.added : p.saved),
    toastError: false,
  })
  const failure = formFailure(save.error, { name_taken: 'port', conflict: 'port' } as const)

  const change =
    <T,>(setter: (value: T) => void) =>
    (value: T) => {
      setter(value)
      setDirty(true)
      save.reset()
    }

  const submit = () => {
    setTouched(true)
    if (!valid || node === null) return
    const bind = formatSocketAddr(address, Number(port))
    const loopback = isLoopbackHost(address)
    save.mutate(
      {
        node,
        // 名字在一个节点上唯一：用协议和端口，一个端口上只能有一个监听。
        name: listener?.name ?? `${protocol}-${port}`,
        bind,
        protocol,
        allow_from: listener?.allow_from ?? [],
        // 开在本机以外的地址时 controller 要求凭据。
        require_credential: !loopback || (listener?.require_credential ?? false),
        default_action: listener?.default_action ?? null,
        fixed_exit: exit.id,
        class: listener?.class ?? null,
        optimistic: listener?.optimistic ?? null,
      },
      { onSuccess: onClose },
    )
  }

  const shown = (problem: string | null) => (touched ? problem : null)
  const whereOptions: { value: Where; label: string }[] = [
    { value: 'local', label: p.whereLocal },
    { value: 'all', label: p.whereAll },
    { value: 'mesh', label: p.whereMesh },
    { value: 'custom', label: p.whereCustom },
  ]

  return (
    <FormDialog
      open
      onClose={onClose}
      title={listener === null ? p.addTitle(exit.name) : p.editTitle}
      size="sm"
      dirty={dirty}
      requireDirty={listener !== null}
      submitLabel={listener === null ? p.add : t.common.save}
      pending={save.isPending}
      error={formError(failure)}
      errorDetail={formErrorDetail(failure)}
      onSubmit={submit}
    >
      <Field label={p.node} hint={p.nodeHint(exit.name)} error={shown(nodeProblem)}>
        {(props) => <NodeSelect {...props} nodes={all} value={node} onChange={change(setNode)} />}
      </Field>
      <Field label={p.protocol}>
        {() => (
          <Segmented
            aria-label={p.protocol}
            value={protocol}
            onChange={change(setProtocol)}
            options={[
              { value: 'socks5', label: PROTOCOL.socks5 },
              { value: 'http', label: PROTOCOL.http },
            ]}
          />
        )}
      </Field>
      <Field label={p.address} error={shown(addressProblem)}>
        {() => (
          <div className="space-y-2">
            <Segmented aria-label={p.address} value={where} onChange={change(setWhere)} options={whereOptions} />
            {where === 'custom' ? (
              <Input
                aria-label={p.whereCustom}
                value={custom}
                placeholder="192.168.1.5"
                onChange={(event) => change(setCustom)(event.target.value)}
              />
            ) : null}
            <p className="text-[13px] text-muted-foreground">{p.whereHint[where]}</p>
          </div>
        )}
      </Field>
      <Field label={p.port} error={shown(portProblem) ?? fieldError(failure, 'port')}>
        {(props) => (
          <Input
            {...props}
            inputMode="numeric"
            className="w-32"
            value={port}
            onChange={(event) => change(setPort)(event.target.value)}
          />
        )}
      </Field>
      {where !== 'local' ? (
        <Notice tone="info">
          {p.needsCredential}{' '}
          <Link to={networkPath(network.id, 'credentials')} className="text-primary hover:underline">
            {p.goCredentials}
          </Link>
        </Notice>
      ) : null}
    </FormDialog>
  )
}

/**
 * 代理入口：经这个出口出去的代理。每一行是开在某台节点上的一个 SOCKS5 或 HTTP 代理，
 * 设备把代理设成它，流量经组网从这台节点的网络出去。行里写开在哪、地址、监听着没有、连接和流量。
 */
export function ProxyEntriesCard({ network, node }: { network: Network; node: NodeDetail }) {
  const listeners = useQuery(q.listeners(network.id))
  const nodes = useQuery(q.nodes(network.id))
  const all = nodes.data ?? []
  const confirm = useConfirm()
  const [editing, setEditing] = useState<{ listener: Listener | null } | null>(null)
  const entries = (listeners.data ?? []).filter((listener) => exitOf(listener) === node.id)

  const remove = useAction(network.id, {
    mutationFn: (listener: Listener) => api.deleteListener(network.id, listener.id),
    success: p.removed,
  })
  const onRemove = async (listener: Listener) => {
    const confirmed = await confirm({
      tone: 'danger',
      title: p.removeTitle(nodeName(all, listener.node), listener.bind),
      body: p.removeBody,
      confirmLabel: p.remove,
    })
    if (confirmed) remove.mutate(listener)
  }

  return (
    <BoxCard data-testid="proxy-entries">
      <BoxCardHeader
        title={p.title}
        tip={p.tip}
        action={
          <Button size="sm" onClick={() => setEditing({ listener: null })}>
            <Plus aria-hidden />
            {p.add}
          </Button>
        }
      />
      {listeners.isPending ? (
        <div className="px-5 py-4">
          <SkeletonBlock lines={2} />
        </div>
      ) : entries.length === 0 ? (
        <p className="px-5 py-6 text-center text-[13px] text-muted-foreground">{p.empty(node.name)}</p>
      ) : (
        <ul className="divide-y">
          {entries.map((listener) => {
            const host = all.find((candidate) => candidate.id === listener.node)
            const view = listenView(listener.status, {
              node: host?.name ?? '',
              bind: listener.bind,
              online: host?.online ?? false,
            })
            return (
              <li
                key={listener.id}
                className="flex items-center gap-4 px-5 py-3"
                data-testid={`proxy-entry-${listener.id}`}
              >
                <div className="min-w-0 flex-1">
                  <div className="flex items-center gap-2 text-sm">
                    <NodeLink
                      network={network.id}
                      node={{ id: listener.node, name: nodeName(all, listener.node) }}
                      online={host?.online}
                    />
                    <span className="rounded-sm bg-muted px-1.5 text-xs leading-5 text-muted-foreground">
                      {PROTOCOL[listener.protocol]}
                    </span>
                    <span className="font-mono text-[13px]">{view.local ?? listener.bind}</span>
                  </div>
                  <div className="mt-0.5 flex items-center gap-3 text-xs text-muted-foreground">
                    <StatusText tone={view.tone} className="text-xs">
                      {view.label}
                    </StatusText>
                    {listener.status !== null ? (
                      <span className="tabular">
                        {p.connections(listener.status.connections)} · ↑ {formatBytes(listener.status.bytes_up)} ↓{' '}
                        {formatBytes(listener.status.bytes_down)}
                      </span>
                    ) : null}
                    {listener.require_credential ? <span>{p.withCredential}</span> : null}
                  </div>
                  {view.detail !== null && view.needsAttention ? (
                    <p className="mt-1 text-xs text-warn">{view.detail}</p>
                  ) : null}
                </div>
                <RowActions label={t.common.actionsFor(listener.bind)}>
                  <DropdownMenuItem onSelect={() => setEditing({ listener })}>
                    <Pencil aria-hidden />
                    {p.edit}
                  </DropdownMenuItem>
                  <DropdownMenuSeparator />
                  <DropdownMenuItem danger onSelect={() => void onRemove(listener)}>
                    <Trash2 aria-hidden />
                    {p.remove}
                  </DropdownMenuItem>
                </RowActions>
              </li>
            )
          })}
        </ul>
      )}
      {editing !== null ? (
        <ProxyDialog network={network} exit={node} listener={editing.listener} onClose={() => setEditing(null)} />
      ) : null}
    </BoxCard>
  )
}
