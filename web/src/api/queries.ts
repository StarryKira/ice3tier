import { QueryClient, queryOptions, useMutation, useQueryClient } from '@tanstack/react-query'

import { toast } from '@/components/ui/toast'
import { LIVE_INTERVAL } from '@/lib/polling'

import { ApiError, errorMessage } from './client'
import { api } from './endpoints'
import { type NetworkId, type NodeId } from './types'

/**
 * 服务器上的状态都经过 TanStack Query。这里是全部的查询键和查询的写法；页面里只写 `useQuery(q.nodes(network))`。
 *
 * 一个网络里的东西互相牵连（删一个节点，转发、监听器、关系图、总览都跟着变），
 * 所以查询键都以 `['n', 网络编号]` 开头，改了任何东西之后把这个网络下的查询一起作废，不逐个去想谁受影响。
 */
export const keys = {
  status: ['status'] as const,
  log: (limit: number) => ['log', limit] as const,
  networks: ['networks'] as const,
  scope: (network: NetworkId) => ['n', network] as const,
  overview: (network: NetworkId) => ['n', network, 'overview'] as const,
  nodes: (network: NetworkId) => ['n', network, 'nodes'] as const,
  node: (network: NetworkId, node: NodeId) => ['n', network, 'node', node] as const,
  routes: (network: NetworkId, node: NodeId) => ['n', network, 'node', node, 'routes'] as const,
  nodeAccess: (network: NetworkId, node: NodeId) => ['n', network, 'node', node, 'access'] as const,
  tokens: (network: NetworkId) => ['n', network, 'tokens'] as const,
  rules: (network: NetworkId) => ['n', network, 'rules'] as const,
  graph: (network: NetworkId) => ['n', network, 'graph'] as const,
  check: (network: NetworkId, from: NodeId, to: NodeId) => ['n', network, 'check', from, to] as const,
  credentials: (network: NetworkId) => ['n', network, 'credentials'] as const,
  listeners: (network: NetworkId) => ['n', network, 'listeners'] as const,
  forwards: (network: NetworkId) => ['n', network, 'forwards'] as const,
  pairs: (network: NetworkId) => ['n', network, 'pairs'] as const,
}

/** 带 `refetchInterval` 的是显示实时状态的查询；标签页不在前台时它们不轮询（见 `lib/polling.ts`）。 */
export const q = {
  status: () =>
    queryOptions({ queryKey: keys.status, queryFn: ({ signal }) => api.status(undefined, signal), staleTime: 30_000 }),
  log: (limit: number) => queryOptions({ queryKey: keys.log(limit), queryFn: () => api.log(limit) }),
  networks: () => queryOptions({ queryKey: keys.networks, queryFn: api.networks }),
  overview: (network: NetworkId) =>
    queryOptions({
      queryKey: keys.overview(network),
      queryFn: () => api.overview(network),
      refetchInterval: LIVE_INTERVAL,
    }),
  nodes: (network: NetworkId) =>
    queryOptions({ queryKey: keys.nodes(network), queryFn: () => api.nodes(network), refetchInterval: LIVE_INTERVAL }),
  node: (network: NetworkId, node: NodeId) =>
    queryOptions({
      queryKey: keys.node(network, node),
      queryFn: () => api.node(network, node),
      refetchInterval: LIVE_INTERVAL,
    }),
  routes: (network: NetworkId, node: NodeId) =>
    queryOptions({
      queryKey: keys.routes(network, node),
      queryFn: () => api.routes(network, node),
      refetchInterval: LIVE_INTERVAL,
    }),
  nodeAccess: (network: NetworkId, node: NodeId) =>
    queryOptions({ queryKey: keys.nodeAccess(network, node), queryFn: () => api.nodeAccess(network, node) }),
  tokens: (network: NetworkId) => queryOptions({ queryKey: keys.tokens(network), queryFn: () => api.tokens(network) }),
  rules: (network: NetworkId) => queryOptions({ queryKey: keys.rules(network), queryFn: () => api.rules(network) }),
  graph: (network: NetworkId) =>
    queryOptions({
      queryKey: keys.graph(network),
      queryFn: () => api.accessGraph(network),
      refetchInterval: LIVE_INTERVAL,
    }),
  check: (network: NetworkId, from: NodeId, to: NodeId) =>
    queryOptions({ queryKey: keys.check(network, from, to), queryFn: () => api.accessCheck(network, from, to) }),
  credentials: (network: NetworkId) =>
    queryOptions({ queryKey: keys.credentials(network), queryFn: () => api.credentials(network) }),
  listeners: (network: NetworkId) =>
    queryOptions({ queryKey: keys.listeners(network), queryFn: () => api.listeners(network) }),
  forwards: (network: NetworkId) =>
    queryOptions({
      queryKey: keys.forwards(network),
      queryFn: () => api.forwards(network),
      refetchInterval: LIVE_INTERVAL,
    }),
  pairs: (network: NetworkId) => queryOptions({ queryKey: keys.pairs(network), queryFn: () => api.pairs(network) }),
}

export function createQueryClient(): QueryClient {
  return new QueryClient({
    defaultOptions: {
      queries: {
        staleTime: 5_000,
        refetchOnWindowFocus: true,
        // 只有"连不上"值得自动重试；controller 明确拒绝的请求再试也是一样的结果。
        retry: (failures, error) => error instanceof ApiError && error.isNetwork && failures < 2,
      },
      mutations: { retry: false },
    },
  })
}

interface ActionOptions<TData, TVars> {
  mutationFn: (vars: TVars) => Promise<TData>
  /** 成功之后弹的提示。不写、或者函数返回 `null` 就不弹：紧接着有自己的结果要显示时用。 */
  success?: string | ((data: TData, vars: TVars) => string | null)
  /**
   * 出错时弹一条不会自己消失的提示，内容是 controller 给的原因。默认开。
   * 表单把错误显示在字段旁边时关掉，免得同一句话出现两次。
   */
  toastError?: boolean
}

/**
 * 一次修改。成功之后把这个网络下的查询全部作废并等它们取回来，
 * 所以调用方 `await mutateAsync()` 回来时，页面上的列表已经是改过之后的样子。
 */
export function useAction<TData, TVars = void>(network: NetworkId | null, options: ActionOptions<TData, TVars>) {
  const client = useQueryClient()
  const { success, toastError = true } = options
  return useMutation<TData, unknown, TVars>({
    mutationFn: options.mutationFn,
    onSuccess: async (data, vars) => {
      const message = typeof success === 'function' ? success(data, vars) : success
      if (message !== undefined && message !== null) toast.success(message)
      const pending = [
        client.invalidateQueries({ queryKey: keys.networks }),
        client.invalidateQueries({ queryKey: ['log'] }),
        client.invalidateQueries({ queryKey: keys.status }),
      ]
      if (network !== null) pending.push(client.invalidateQueries({ queryKey: keys.scope(network) }))
      await Promise.all(pending)
    },
    onError: (error) => {
      if (toastError) toast.error(errorMessage(error))
    },
  })
}
