import {
  type HostReport,
  type NodeDetail,
  type NodeId,
  type PathResult,
  type PathView,
  type Route,
  type TestItemState,
  type TestView,
} from '@/api/types'
import { t } from '@/i18n'

/**
 * 路径诊断用到的几样小东西：测了的各条路、最快的和吞吐最高的那条、路的说法、一端的主机快照、进度。纯函数。
 * 逐项检查在 `checks.ts`，报告的文字在 `report.ts`。
 */

export type Named = readonly { id: NodeId; name: string }[]

export interface DiagnosisInput {
  a: NodeDetail | undefined
  b: NodeDetail | undefined
  path: PathView | undefined
  test: TestView | undefined
  /** 认得的名字：节点列表里的，加上测试里带的。 */
  nodes: Named
}

const nameOf = (nodes: Named, id: NodeId) => nodes.find((node) => node.id === id)?.name ?? t.common.nodeMissing(id)

/** 一条路的一句话，和拓扑、路由矩阵一样的说法："直连"、"经 h1 → h2"，没有路时"没有能走的路"。 */
export function routeName(route: Route | null | undefined, nodes: Named): string {
  if (route === null || route === undefined || route.via === 'none') return t.tests.noRoute
  if (route.via === 'direct') return t.routes.direct
  return t.routes.via(route.relays.map((id) => nameOf(nodes, id)).join(t.routes.hopSeparator))
}

/** 测了的一条路，和它是不是直连那一条（路还不知道时靠这个起名字）。 */
export interface Measured {
  result: PathResult
  direct: boolean
}

export function measuredPaths(test: TestView | undefined): Measured[] {
  if (test === undefined) return []
  return [
    ...(test.direct_quality === null ? [] : [{ result: test.direct_quality, direct: true }]),
    ...test.relay_quality.map((result) => ({ result, direct: false })),
  ]
}

/** 一条测了的路叫什么："直连"、"经 h2"。 */
export function pathLabel(path: Measured, nodes: Named): string {
  if (path.direct) return t.tests.qualityDirect
  return path.result.route === null ? t.tests.qualityRelay : routeName(path.result.route, nodes)
}

/** 平均延迟最低的、测完了的那条。 */
export function fastest(paths: readonly Measured[]): Measured | null {
  let best: Measured | null = null
  for (const path of paths) {
    const avg = path.result.latency?.avg_us
    if (path.result.state !== 'done' || avg === null || avg === undefined) continue
    const current = best?.result.latency?.avg_us
    if (best === null || current === null || current === undefined || avg < current) best = path
  }
  return best
}

/** 吞吐最高的那条：按上下行里慢的那个比。 */
export function widest(paths: readonly Measured[]): Measured | null {
  let best: Measured | null = null
  let bestRate = -1
  for (const path of paths) {
    const throughput = path.result.throughput
    if (throughput?.state !== 'done') continue
    const rate = Math.min(
      throughput.up?.bits_per_second ?? Number.POSITIVE_INFINITY,
      throughput.down?.bits_per_second ?? Number.POSITIVE_INFINITY,
    )
    if (Number.isFinite(rate) && rate > bestRate) {
      best = path
      bestRate = rate
    }
  }
  return best
}

export function hostOf(test: TestView | undefined, node: NodeId | undefined): HostReport | undefined {
  return test?.hosts?.find((host) => host.node === node)
}

/* 进度：要了的每一项现在到哪一步。 */

export type StepKey = 'direct' | 'direct_quality' | 'relay_quality' | 'route' | 'throughput' | 'host'

export interface Step {
  key: StepKey
  state: TestItemState
}

/** 几个结果合成一项：还有在测的就是在测；有一个成了就是成了；都没成看有没有失败的。 */
function merge(states: readonly TestItemState[]): TestItemState | null {
  if (states.length === 0) return null
  if (states.includes('pending')) return 'pending'
  if (states.includes('done')) return 'done'
  if (states.includes('failed')) return 'failed'
  return 'skipped'
}

export function steps(test: TestView): Step[] {
  const list: { key: StepKey; state: TestItemState | null }[] = [
    { key: 'direct', state: test.direct?.state ?? null },
    { key: 'direct_quality', state: test.direct_quality?.state ?? null },
    { key: 'relay_quality', state: merge(test.relay_quality.map((path) => path.state)) },
    { key: 'route', state: test.route?.state ?? null },
    {
      key: 'throughput',
      state: test.request.throughput
        ? merge(
            measuredPaths(test).flatMap(({ result }) =>
              result.throughput === null
                ? result.state === 'pending'
                  ? ['pending' as const]
                  : []
                : [result.throughput.state],
            ),
          )
        : null,
    },
    { key: 'host', state: merge((test.hosts ?? []).map((host) => host.state)) },
  ]
  return list.filter((step): step is Step => step.state !== null)
}
