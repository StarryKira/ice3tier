import { useQuery } from '@tanstack/react-query'
import { CircleCheck, LoaderCircle, RotateCw, TicketPlus } from 'lucide-react'
import { type ReactNode, useEffect, useId, useMemo, useRef, useState } from 'react'
import { Link } from 'react-router'

import { errorMessage } from '@/api/client'
import { api } from '@/api/endpoints'
import { q, useAction } from '@/api/queries'
import { type CreatedEnrollToken, type Network, type Node, type NodeId } from '@/api/types'
import { CodeBlock, CopyButton } from '@/components/copy'
import { NodeSelect } from '@/components/node'
import { nodePath } from '@/lib/nodes'
import { Notice } from '@/components/notice'
import { TagInput } from '@/components/tags'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Field, FormError } from '@/components/ui/field'
import { Input, NativeSelect } from '@/components/ui/input'
import { Segmented } from '@/components/ui/segmented'
import { t } from '@/i18n'
import { buildJoinCommand, installCommands, type JoinDeclarations, NO_DECLARATIONS, runCommand } from '@/lib/commands'
import { WAITING_INTERVAL } from '@/lib/polling'
import { type Capability, DEFAULT_TOKEN_TTL, PLATFORMS, type PlatformId, RELEASES_PAGE } from '@/lib/product'
import { formatRemaining } from '@/lib/time'
import { useNow } from '@/lib/useNow'
import { cn } from '@/lib/utils'
import { validateVirtualIp } from '@/lib/validate'

const TTL_CHOICES = [
  { secs: 60 * 60, label: t.addNode.ttl1h },
  { secs: DEFAULT_TOKEN_TTL, label: t.addNode.ttl24h },
  { secs: 7 * 24 * 60 * 60, label: t.addNode.ttl7d },
] as const

/** 等了多久之后给出"还没连上？"的排查提示。 */
const STUCK_AFTER_SECS = 60

const DECLARE_ROWS: readonly { capability: Capability; label: string; hint: string }[] = [
  { capability: 'relay', label: t.addNode.declareRelay, hint: t.addNode.declareRelayHint },
  { capability: 'exit', label: t.addNode.declareExit, hint: t.addNode.declareExitHint },
  { capability: 'forward', label: t.addNode.declareForward, hint: t.addNode.declareForwardHint },
]

function Step({ index, title, children }: { index: number; title: string; children: ReactNode }) {
  return (
    <li className="grid grid-cols-[1.75rem_minmax(0,1fr)] gap-x-3">
      <span
        aria-hidden
        className="flex size-7 items-center justify-center rounded-full bg-muted text-[13px] font-semibold text-foreground tabular"
      >
        {index}
      </span>
      <div className="min-w-0 pb-1">
        <h3 className="flex h-7 items-center text-sm font-semibold">{title}</h3>
        <div className="mt-2 space-y-2.5">{children}</div>
      </div>
    </li>
  )
}

export interface AddNodeState {
  /** 令牌已经生成、节点还没用它加入：这时关掉界面，令牌就再也看不到了。 */
  tokenOutstanding: boolean
}

interface AddNodeFlowProps {
  network: Network
  /** 引导里的提示，显示在选项上方（例如"其中一台有公网 IP 的话，勾上 relay"）。 */
  intro?: ReactNode
  onStateChange?: (state: AddNodeState) => void
  /** 点"查看节点"时调用（对话框借此把自己关掉）。 */
  onNavigate?: () => void
}

/**
 * 添加节点的全部内容：选项、三个编号的步骤、等待区。对话框和总览页的引导用的是同一个。
 *
 * 密钥是流程里的一步，不是另一个页面上的事：令牌在第 2 步就地生成，生成之前第 3 步的命令里是占位符，
 * 让人先看懂命令的结构。节点一加入，等待区自己变成结果，不用去刷新列表。
 *
 * "可以承担"的三个勾选是命令的参数，不属于令牌，所以生成令牌之后仍然可以改；
 * 标签、有效期、预设 IP、继任写在令牌上，生成之后变为只读。
 */
export function AddNodeFlow({ network, intro, onStateChange, onNavigate }: AddNodeFlowProps) {
  const now = useNow()
  const platformLabelId = useId()
  const declareLabelId = useId()

  const [platform, setPlatform] = useState<PlatformId>('linux-x86_64')
  const [declare, setDeclare] = useState<JoinDeclarations>(NO_DECLARATIONS)
  const [tags, setTags] = useState<string[]>([])
  const [ttl, setTtl] = useState<number>(DEFAULT_TOKEN_TTL)
  const [virtualIp, setVirtualIp] = useState('')
  const [successorOf, setSuccessorOf] = useState<NodeId | null>(null)

  const [created, setCreated] = useState<CreatedEnrollToken | null>(null)
  const [createdAt, setCreatedAt] = useState(0)
  /** 生成令牌那一刻已经在网络里的节点。之后多出来的就是新加入的。 */
  const [knownNodes, setKnownNodes] = useState<ReadonlySet<NodeId>>(new Set())
  const [joined, setJoined] = useState<Node | null>(null)

  const waiting = created !== null && joined === null
  const nodes = useQuery({ ...q.nodes(network.id), refetchInterval: waiting ? WAITING_INTERVAL : false })
  const tokens = useQuery({
    ...q.tokens(network.id),
    refetchInterval: waiting ? WAITING_INTERVAL : false,
    enabled: waiting,
  })
  const graph = useQuery({ ...q.graph(network.id), refetchInterval: false })

  const knownTags = useMemo(() => (graph.data?.tags ?? []).map((entry) => entry.tag), [graph.data])
  const offlineNodes = useMemo(() => (nodes.data ?? []).filter((node) => !node.online), [nodes.data])

  const ipProblem = virtualIp.trim() === '' ? null : validateVirtualIp(virtualIp, network.cidr)

  const generate = useAction(network.id, {
    mutationFn: async (previous: CreatedEnrollToken | null) => {
      // 重新生成：先收回刚才那个，免得外面留着一个没人管的有效令牌。它要是已经不在了也无妨。
      if (previous !== null) await api.deleteToken(network.id, previous.id).catch(() => undefined)
      const current = await api.nodes(network.id)
      const token = await api.createToken(network.id, {
        expires_in_secs: ttl,
        tags,
        virtual_ip: successorOf === null && virtualIp.trim() !== '' ? virtualIp.trim() : undefined,
        successor_of: successorOf ?? undefined,
      })
      return { token, known: new Set(current.map((node) => node.id)) }
    },
    success: (_data, previous) => (previous === null ? null : t.addNode.regenerated),
    toastError: false,
  })

  // 令牌从"没用掉的"列表里消失，同时网络里多出一个节点：就是它加入了。
  const tokenPending = tokens.data?.some((token) => token.id === created?.id) ?? true
  const newcomer = useMemo(
    () => (waiting ? (nodes.data ?? []).find((node) => !knownNodes.has(node.id)) : undefined),
    [waiting, nodes.data, knownNodes],
  )
  // 在渲染时直接记下（而不是放进 effect）：这是从取到的数据推出来的状态，记下之后轮询随即停止。
  if (waiting && newcomer !== undefined && !tokenPending) setJoined(newcomer)

  const expired = waiting && now >= created.expires_at
  const gone = waiting && !expired && tokens.data !== undefined && !tokenPending && newcomer === undefined
  const stuck = waiting && !expired && !gone && now - createdAt >= STUCK_AFTER_SECS

  useEffect(() => {
    onStateChange?.({ tokenOutstanding: waiting && !expired && !gone })
  }, [onStateChange, waiting, expired, gone])

  // 结果写在最下面。对话框比屏幕高时那里在视野之外，不把它带进来，用户会以为什么都没发生。
  const outcomeRef = useRef<HTMLElement>(null)
  const settled = joined !== null || expired || gone
  useEffect(() => {
    if (settled) outcomeRef.current?.scrollIntoView({ block: 'nearest' })
  }, [settled])

  const onGenerate = (previous: CreatedEnrollToken | null) => {
    if (ipProblem !== null) return
    generate.mutate(previous, {
      onSuccess: ({ token, known }) => {
        setCreated(token)
        setKnownNodes(known)
        setCreatedAt(Math.floor(Date.now() / 1000))
        setJoined(null)
      },
    })
  }

  const addAnother = () => {
    setCreated(null)
    setJoined(null)
    setVirtualIp('')
    setSuccessorOf(null)
    generate.reset()
  }

  const locked = created !== null
  const install = installCommands(platform)
  const joinCommand = buildJoinCommand(created?.token ?? { placeholder: t.addNode.tokenPlaceholder }, declare, platform)
  const isWindows = PLATFORMS.find((candidate) => candidate.id === platform)?.windows === true
  const remaining = created === null ? null : formatRemaining(created.expires_at, now)

  return (
    <div className="space-y-6">
      {intro}

      <section aria-label={t.addNode.optionsTitle} className="space-y-4">
        <Field label={t.addNode.tagsLabel} hint={t.addNode.tagsHint} optional={t.common.optional}>
          {(props) => <TagInput {...props} value={tags} onChange={setTags} suggestions={knownTags} disabled={locked} />}
        </Field>

        <div>
          <p id={declareLabelId} className="text-[13px] font-medium">
            {t.addNode.declareLabel}
          </p>
          <div role="group" aria-labelledby={declareLabelId} className="mt-2 space-y-2">
            {DECLARE_ROWS.map((row) => (
              <label key={row.capability} className="flex items-start gap-2.5">
                <Checkbox
                  checked={declare[row.capability]}
                  onCheckedChange={(checked) =>
                    setDeclare((current) => ({ ...current, [row.capability]: checked === true }))
                  }
                  className="mt-0.5"
                />
                <span className="min-w-0">
                  <span className="block text-sm">{row.label}</span>
                  <span className="block text-[13px] text-muted-foreground">{row.hint}</span>
                </span>
              </label>
            ))}
          </div>
          <p className="mt-2 text-[13px] text-muted-foreground">{t.addNode.declareNote}</p>
        </div>

        <details className="group">
          <summary className="w-fit rounded-sm text-[13px] font-medium text-muted-foreground hover:text-foreground">
            {t.addNode.advanced}
          </summary>
          <div className="mt-4 grid grid-cols-2 gap-4 pb-1">
            <Field label={t.addNode.ttlLabel}>
              {(props) => (
                <NativeSelect
                  {...props}
                  value={ttl}
                  disabled={locked}
                  onChange={(event) => setTtl(Number(event.target.value))}
                >
                  {TTL_CHOICES.map((choice) => (
                    <option key={choice.secs} value={choice.secs}>
                      {choice.label}
                    </option>
                  ))}
                </NativeSelect>
              )}
            </Field>
            <Field
              label={t.addNode.ipLabel}
              optional={t.common.optional}
              hint={successorOf !== null ? t.addNode.successorNoIp : t.addNode.ipHint}
              error={successorOf === null ? ipProblem : null}
            >
              {(props) => (
                <Input
                  {...props}
                  className="font-mono"
                  value={successorOf !== null ? '' : virtualIp}
                  disabled={locked || successorOf !== null}
                  placeholder={network.cidr.replace(/\.0\/\d+$/, '.9')}
                  onChange={(event) => setVirtualIp(event.target.value)}
                />
              )}
            </Field>
            <Field
              label={t.addNode.successorLabel}
              optional={t.common.optional}
              hint={t.addNode.successorHint}
              className="col-span-2"
            >
              {(props) => (
                <div className="flex items-center gap-2">
                  <div className="min-w-0 flex-1">
                    <NodeSelect
                      {...props}
                      nodes={offlineNodes}
                      value={successorOf}
                      onChange={setSuccessorOf}
                      disabled={locked || offlineNodes.length === 0}
                      placeholder={
                        offlineNodes.length === 0 ? t.addNode.successorEmpty : t.addNode.successorPlaceholder
                      }
                    />
                  </div>
                  {successorOf !== null && !locked ? (
                    <Button size="sm" variant="ghost" onClick={() => setSuccessorOf(null)}>
                      {t.addNode.successorNone}
                    </Button>
                  ) : null}
                </div>
              )}
            </Field>
          </div>
        </details>
        {locked ? <p className="text-[13px] text-muted-foreground">{t.addNode.lockedNote}</p> : null}
      </section>

      <ol className="space-y-7 border-t pt-6">
        <Step index={1} title={t.addNode.step1}>
          <span id={platformLabelId} className="sr-only">
            {t.addNode.platformLabel}
          </span>
          <Segmented
            aria-label={t.addNode.platformLabel}
            size="sm"
            value={platform}
            onChange={setPlatform}
            options={PLATFORMS.map((candidate) => ({ value: candidate.id, label: t.addNode.platform[candidate.id] }))}
          />
          {isWindows ? <p className="text-[13px] text-muted-foreground">{t.addNode.step1Windows}</p> : null}
          <CodeBlock copy={install.join(' && ')} copyLabel={t.addNode.copyInstall}>
            {install.join('\n')}
          </CodeBlock>
          <p className="text-[13px] text-muted-foreground">
            {t.addNode.step1Manual}
            <a
              href={RELEASES_PAGE}
              target="_blank"
              rel="noreferrer"
              className="rounded-sm text-primary underline-offset-4 hover:underline"
            >
              {t.addNode.step1Releases}
            </a>
          </p>
        </Step>

        <Step index={2} title={t.addNode.step2}>
          {created === null ? (
            <>
              <Button
                variant="primary"
                pending={generate.isPending}
                disabled={ipProblem !== null}
                onClick={() => onGenerate(null)}
              >
                <TicketPlus aria-hidden />
                {t.addNode.generate}
              </Button>
              <FormError message={generate.isError ? errorMessage(generate.error) : null} />
            </>
          ) : (
            <>
              <div className="flex flex-wrap items-center gap-x-3 gap-y-1.5">
                <p className="flex items-center gap-1.5 text-sm">
                  <CircleCheck className="size-4 text-ok" aria-hidden />
                  {remaining === null ? t.addNode.expired : t.addNode.generated(remaining)}
                </p>
                {joined === null ? (
                  <Button
                    size="sm"
                    pending={generate.isPending}
                    onClick={() => onGenerate(created)}
                    title={t.addNode.regenerateHint}
                  >
                    <RotateCw aria-hidden />
                    {t.addNode.regenerate}
                  </Button>
                ) : null}
              </div>
              <FormError message={generate.isError ? errorMessage(generate.error) : null} />
            </>
          )}
        </Step>

        <Step index={3} title={t.addNode.step3}>
          <p className="text-[13px] text-muted-foreground">
            {created === null ? t.addNode.step3Before : t.addNode.step3Join}
          </p>
          <CodeBlock
            copy={created === null ? null : joinCommand}
            copyLabel={t.addNode.copyCommand}
            disabledReason={t.addNode.copyDisabled}
            label={t.addNode.joinCommandLabel}
            testId="join-command"
            className={cn(created !== null && joined === null && 'bg-accent')}
          >
            {joinCommand}
          </CodeBlock>
          {created !== null && joined === null ? (
            <div className="flex flex-wrap items-center justify-between gap-2">
              <p className="text-[13px] font-medium text-warn">{t.addNode.shownOnce}</p>
              <CopyButton value={created.token} size="sm" variant="ghost">
                {t.addNode.copyToken}
              </CopyButton>
            </div>
          ) : null}
          <p className="pt-1 text-[13px] text-muted-foreground">{t.addNode.step3Run}</p>
          <CodeBlock copy={runCommand(platform)}>{runCommand(platform)}</CodeBlock>
          <p className="text-[13px] text-muted-foreground">{t.addNode.step3RunHint}</p>
        </Step>
      </ol>

      {created !== null ? (
        <section ref={outcomeRef} aria-live="polite" className="scroll-my-5 border-t pt-5">
          {joined !== null ? (
            <div className="flex flex-wrap items-center justify-between gap-3">
              <div>
                <p className="flex items-center gap-2 text-sm font-semibold" data-testid="node-joined">
                  <CircleCheck className="size-4 text-ok" aria-hidden />
                  {t.addNode.joined(joined.name, joined.virtual_ip)}
                </p>
                {joined.hostname !== '' ? (
                  <p className="mt-0.5 pl-6 text-[13px] text-muted-foreground" data-testid="node-joined-hostname">
                    {t.addNode.joinedHostname(joined.hostname)}
                  </p>
                ) : null}
                <p className="mt-0.5 pl-6 text-[13px] text-muted-foreground">{t.addNode.joinedHint}</p>
              </div>
              <div className="flex items-center gap-2">
                <Button onClick={addAnother}>{t.addNode.addAnother}</Button>
                <Button asChild variant="primary">
                  <Link to={nodePath(network.id, joined.id)} onClick={onNavigate}>
                    {t.addNode.viewNode}
                  </Link>
                </Button>
              </div>
            </div>
          ) : expired || gone ? (
            <div className="flex flex-wrap items-center justify-between gap-3">
              <p className="text-sm font-medium text-warn">{expired ? t.addNode.expired : t.addNode.gone}</p>
              <Button size="sm" pending={generate.isPending} onClick={() => onGenerate(null)}>
                <RotateCw aria-hidden />
                {t.addNode.regenerate}
              </Button>
            </div>
          ) : (
            <>
              <p className="flex items-center gap-2 text-sm font-medium">
                <LoaderCircle className="size-4 animate-spin text-muted-foreground" aria-hidden />
                {t.addNode.waiting}
              </p>
              <p className="mt-0.5 pl-6 text-[13px] text-muted-foreground">{t.addNode.waitingHint}</p>
              {stuck ? (
                <details className="mt-2.5 pl-6 text-[13px]">
                  <summary className="rounded-sm font-medium">{t.addNode.stuck}</summary>
                  <ul className="mt-1.5 list-disc space-y-1 pl-4 text-muted-foreground">
                    <li>{t.addNode.stuckCauses[0]}</li>
                    <li>
                      {t.addNode.stuckCauses[1]}
                      <ControllerAddress />
                    </li>
                    <li>{t.addNode.stuckCauses[2]}</li>
                  </ul>
                </details>
              ) : null}
            </>
          )}
        </section>
      ) : null}

      {nodes.isError ? <Notice tone="warn">{errorMessage(nodes.error)}</Notice> : null}
    </div>
  )
}

/** 令牌里写着的 controller 地址。节点连不上时，多半是这个地址从那台机器上到不了。 */
function ControllerAddress() {
  const status = useQuery(q.status())
  if (status.data === undefined) return null
  return (
    <code className="ml-1 rounded-sm bg-muted px-1 py-px font-mono text-xs text-foreground">
      {status.data.public_addr}
    </code>
  )
}
