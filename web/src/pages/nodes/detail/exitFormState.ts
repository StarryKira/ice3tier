import { type ExitInput, type ExitSettings } from '@/api/types'
import { DEFAULT_EXIT_PORTS } from '@/lib/product'
import { parseOptionalPositiveInt, parsePortList, validateSocketAddr } from '@/lib/validate'

/** 出口设置的表单。节点详情页和出口页的"启用出口"对话框用的是同一个。 */
export interface ExitFormState {
  limitPorts: boolean
  ports: string
  bandwidth: string
  lan: boolean
  upstream: string
}

export function exitFormFrom(settings: ExitSettings | null): ExitFormState {
  return {
    limitPorts: settings?.ports !== null,
    ports: (settings?.ports ?? DEFAULT_EXIT_PORTS).join(', '),
    bandwidth: settings?.bandwidth_mbps?.toString() ?? '',
    lan: settings?.lan ?? false,
    upstream: settings?.upstream_socks5 ?? '',
  }
}

export interface ExitFormProblems {
  ports: string | null
  bandwidth: string | null
  upstream: string | null
}

export function exitFormProblems(state: ExitFormState): ExitFormProblems {
  return {
    ports: state.limitPorts ? parsePortList(state.ports).error : null,
    bandwidth: parseOptionalPositiveInt(state.bandwidth).error,
    upstream: state.upstream.trim() === '' ? null : validateSocketAddr(state.upstream),
  }
}

export function exitFormValid(state: ExitFormState): boolean {
  const problems = exitFormProblems(state)
  return problems.ports === null && problems.bandwidth === null && problems.upstream === null
}

export function exitFormToInput(state: ExitFormState): ExitInput {
  return {
    ports: state.limitPorts ? parsePortList(state.ports).ports : null,
    lan: state.lan,
    bandwidth_mbps: parseOptionalPositiveInt(state.bandwidth).value,
    upstream_socks5: state.upstream.trim() === '' ? null : state.upstream.trim(),
  }
}
