import { type NameService, type VnetCause, type VnetDrops, type VnetReport } from '@/api/types'
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
      // 文档早先写成过单个字符串：收到字符串也照样认。
      const missing: readonly string[] = Array.isArray(cause.missing) ? cause.missing : [String(cause.missing)]
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

const DROP_KEYS = ['not_allowed', 'unreachable', 'bad_source', 'bad_destination', 'malformed'] as const

/** 丢掉的包：只列不是 0 的，按要紧的在前（策略不允许、送不到最常见）。 */
export function droppedList(
  dropped: VnetDrops | null | undefined,
): { key: (typeof DROP_KEYS)[number]; count: number }[] {
  if (dropped === null || dropped === undefined) return []
  return DROP_KEYS.map((key) => ({ key, count: dropped[key] })).filter(({ count }) => count > 0)
}
