import { type NameService, type RuleState, type VnetCause, type VnetDrops, type VnetReport } from '@/api/types'
import { type Tone } from '@/components/status'
import { t } from '@/i18n'

/** 虚拟网卡的状态。比界面新的节点报的、和没有报的（`null`）都是 `unknown`：不当成关着。 */
export type VnetState = VnetReport['state'] | 'unknown'

const KNOWN: readonly string[] = ['installed', 'off', 'failed', 'unsupported']

export function vnetState(report: VnetReport | null | undefined): VnetState {
  return report !== null && report !== undefined && KNOWN.includes(report.state) ? report.state : 'unknown'
}

/** 状态的圆点：装好了绿，装不上红，关着、不支持、未知是灰圈（关着是本机有意的，不是毛病）。 */
export function vnetTone(state: VnetState): Tone {
  return state === 'installed' ? 'ok' : state === 'failed' ? 'danger' : 'idle'
}

/** 一个原因写成一句话。`no_permission` 的 `Administrator`、`root` 写成"以管理员 / root 运行"，别的是缺的能力名。 */
export function vnetCauseText(cause: VnetCause | null | undefined): string {
  const c = t.nodes.detail.vnet.causes
  if (cause === null || cause === undefined) return c.unknown
  switch (cause.code) {
    case 'no_permission': {
      const missing = Array.isArray(cause.missing) ? cause.missing : [cause.missing]
      if (missing.includes('Administrator')) return c.administrator
      if (missing.includes('root')) return c.root
      return c.missing(missing.join(t.common.listSeparator))
    }
    case 'no_device':
      return c.noDevice(cause.detail)
    case 'driver_missing':
      return c.driverMissing(cause.path)
    case 'device_failed':
      return c.deviceFailed(cause.detail)
    case 'program_missing': {
      const pkg = Object.hasOwn(c.programPackage, cause.program) ? c.programPackage[cause.program] : undefined
      return pkg === undefined ? c.programMissing(cause.program) : c.programMissingPackage(cause.program, pkg)
    }
    case 'command_failed':
      return c.commandFailed(cause.program, cause.detail)
    case 'port_in_use':
      return c.portInUse(cause.address)
    case 'bind_failed':
      return c.bindFailed(cause.address, cause.detail)
    case 'range_in_use':
      return c.rangeInUse(cause.with)
    default:
      return c.unknown
  }
}

/** 名字怎么解析。不认识的 `state` 返回 `null`（不写这一行）。 */
export function namesText(names: NameService | null | undefined): string | null {
  const v = t.nodes.detail.vnet
  switch (names?.state) {
    case 'resolved':
      return v.namesResolved
    case 'no_resolver':
      return v.namesNoResolver
    case 'failed':
      return v.namesFailed(vnetCauseText(names.cause))
    default:
      return null
  }
}

type RuleKey = keyof typeof RULE_KEYS
const RULE_KEYS = { on: true, off: true, failed: true, unsupported: true } as const

function ruleState(rule: RuleState | null | undefined): RuleKey | 'unknown' {
  return rule !== null && rule !== undefined && Object.hasOwn(RULE_KEYS, rule.state) ? rule.state : 'unknown'
}

/** 防火墙的两条规则写成一句：两条一样时合起来写，不一样时逐条写，装不上的带原因。`problem` 是有一条装不上。 */
export function firewallText(firewall: VnetReport['firewall'] | null | undefined): { text: string; problem: boolean } {
  const v = t.nodes.detail.vnet
  const accept = ruleState(firewall?.accept)
  const guard = ruleState(firewall?.guard)
  if (accept === guard) {
    if (accept === 'on') return { text: v.firewallOn, problem: false }
    if (accept === 'off') return { text: v.firewallOff, problem: false }
    if (accept === 'unsupported') return { text: v.firewallUnsupported, problem: false }
  }
  const one = (label: string, state: RuleKey | 'unknown', rule: RuleState | undefined) =>
    rule?.state === 'failed' ? v.ruleFailed(label, vnetCauseText(rule.cause)) : `${label} ${v.rule[state]}`
  return {
    text: [one(v.accept, accept, firewall?.accept), one(v.guard, guard, firewall?.guard)].join(' · '),
    problem: accept === 'failed' || guard === 'failed',
  }
}

const DROP_KEYS = ['not_allowed', 'unreachable', 'bad_source', 'bad_destination', 'malformed'] as const

/** 丢掉的包：只列不是 0 的，按要紧的在前（策略不允许、送不到最常见）。 */
export function droppedList(dropped: VnetDrops | null | undefined): { key: (typeof DROP_KEYS)[number]; count: number }[] {
  if (dropped === null || dropped === undefined) return []
  return DROP_KEYS.map((key) => ({ key, count: dropped[key] })).filter(({ count }) => count > 0)
}
