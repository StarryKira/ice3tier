import { type ExitInput, type ExitSettings, type IpFamily, type ServiceClass } from '@/api/types'
import { type ByteUnit, bytesToField } from '@/lib/format'
import { DEFAULT_EXIT_PORTS } from '@/lib/product'
import {
  parseOptionalBytes,
  parseOptionalPositiveInt,
  parsePortList,
  parseResetDay,
  validateSocketAddr,
} from '@/lib/validate'

/** 出口设置的表单。节点详情页和出口页的"启用出口"对话框用的是同一个。 */
export interface ExitFormState {
  limitPorts: boolean
  ports: string
  bandwidth: string
  lan: boolean
  upstream: string
  quota: string
  quotaUnit: ByteUnit
  /** 原来的配额（字节）。输入框里的数是换算过的，没改动时原样提交它，免得四舍五入改掉了配额。 */
  quotaOriginal: number | null
  resetDay: string
  family: IpFamily
  connects: string
  serviceClass: ServiceClass | null
}

export function exitFormFrom(settings: ExitSettings | null): ExitFormState {
  const quota = bytesToField(settings?.quota_bytes ?? null)
  return {
    limitPorts: settings?.ports !== null,
    ports: (settings?.ports ?? DEFAULT_EXIT_PORTS).join(', '),
    bandwidth: settings?.bandwidth_mbps?.toString() ?? '',
    lan: settings?.lan ?? false,
    upstream: settings?.upstream_socks5 ?? '',
    quota: quota.value,
    quotaUnit: quota.unit,
    quotaOriginal: settings?.quota_bytes ?? null,
    resetDay: String(settings?.quota_reset_day ?? 1),
    family: settings?.family ?? 'any',
    connects: settings?.connects_per_minute?.toString() ?? '',
    serviceClass: settings?.class ?? null,
  }
}

export interface ExitFormProblems {
  ports: string | null
  bandwidth: string | null
  upstream: string | null
  quota: string | null
  resetDay: string | null
  connects: string | null
}

export function exitFormProblems(state: ExitFormState): ExitFormProblems {
  return {
    ports: state.limitPorts ? parsePortList(state.ports).error : null,
    bandwidth: parseOptionalPositiveInt(state.bandwidth).error,
    upstream: state.upstream.trim() === '' ? null : validateSocketAddr(state.upstream),
    quota: parseOptionalBytes(state.quota, state.quotaUnit).error,
    // 没有配额时账期无所谓，不拦着保存。
    resetDay: state.quota.trim() === '' ? null : parseResetDay(state.resetDay).error,
    connects: parseOptionalPositiveInt(state.connects).error,
  }
}

export function exitFormValid(state: ExitFormState): boolean {
  return Object.values(exitFormProblems(state)).every((problem) => problem === null)
}

/** 配额：输入框没动过时是原来的字节数，动过时按输入换算。 */
function quotaBytes(state: ExitFormState): number | null {
  const original = bytesToField(state.quotaOriginal)
  if (state.quota.trim() === original.value && state.quotaUnit === original.unit) return state.quotaOriginal
  return parseOptionalBytes(state.quota, state.quotaUnit).value
}

/** `PUT …/exit` 是整个替换：每一项都写上，不写的会回到默认值。 */
export function exitFormToInput(state: ExitFormState): ExitInput {
  const quota = quotaBytes(state)
  return {
    ports: state.limitPorts ? parsePortList(state.ports).ports : null,
    lan: state.lan,
    bandwidth_mbps: parseOptionalPositiveInt(state.bandwidth).value,
    upstream_socks5: state.upstream.trim() === '' ? null : state.upstream.trim(),
    quota_bytes: quota,
    quota_reset_day: quota === null ? 1 : parseResetDay(state.resetDay).value,
    family: state.family,
    connects_per_minute: parseOptionalPositiveInt(state.connects).value,
    class: state.serviceClass,
  }
}
