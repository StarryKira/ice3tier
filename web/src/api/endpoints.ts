import { request } from './client'
import {
  type AccessCheck,
  type AccessGraph,
  type CreateCredential,
  type CreatedCredential,
  type CreatedEnrollToken,
  type CreateEnrollToken,
  type CreateNetwork,
  type CreateRule,
  type Credential,
  type EnrollTokenInfo,
  type ExitInput,
  type ForcedUpdates,
  type ForceUpdate,
  type Forward,
  type ForwardInput,
  type Listener,
  type ListenerInput,
  type LogEntry,
  type Network,
  type NetworkId,
  type Node,
  type NodeAccess,
  type NodeDetail,
  type NodeId,
  type NodeUpdate,
  type Overview,
  type PairInput,
  type PairSetting,
  type PathView,
  type RelayInput,
  type ResumeUpdate,
  type RouteMatrix,
  type RouteTable,
  type Rule,
  type Status,
  type Succeed,
  type UpdateNetwork,
  type UpdateNode,
  type UpdateOverview,
  type UpdateRule,
  type UpdateSettings,
} from './types'

/**
 * 管理 API 的每个接口一个函数，名字和 `docs/admin-api.md` §13 的表一一对应。
 *
 * 唯一没有对应函数的是 `GET /metrics`（§16）：它不在 `/api/v1` 下面，是给 Prometheus 抓的文本，界面只写出它的地址。
 */

const net = (network: NetworkId) => `/networks/${network}`

export const api = {
  /** `token` 给了就用它而不是已登录的令牌：登录页用来检查令牌对不对。 */
  status: (token?: string, signal?: AbortSignal) => request<Status>('GET', '/status', { token, signal }),
  log: (limit: number) => request<LogEntry[]>('GET', '/log', { query: { limit } }),

  networks: () => request<Network[]>('GET', '/networks'),
  createNetwork: (body: CreateNetwork) => request<Network>('POST', '/networks', { body }),
  network: (network: NetworkId) => request<Network>('GET', net(network)),
  updateNetwork: (network: NetworkId, body: UpdateNetwork) => request<Network>('PATCH', net(network), { body }),
  deleteNetwork: (network: NetworkId) => request<undefined>('DELETE', net(network)),
  overview: (network: NetworkId) => request<Overview>('GET', `${net(network)}/overview`),

  nodes: (network: NetworkId) => request<Node[]>('GET', `${net(network)}/nodes`),
  node: (network: NetworkId, node: NodeId) => request<NodeDetail>('GET', `${net(network)}/nodes/${node}`),
  updateNode: (network: NetworkId, node: NodeId, body: UpdateNode) =>
    request<Node>('PATCH', `${net(network)}/nodes/${node}`, { body }),
  deleteNode: (network: NetworkId, node: NodeId) => request<undefined>('DELETE', `${net(network)}/nodes/${node}`),
  enableRelay: (network: NetworkId, node: NodeId, body: RelayInput) =>
    request<Node>('PUT', `${net(network)}/nodes/${node}/relay`, { body }),
  disableRelay: (network: NetworkId, node: NodeId) => request<Node>('DELETE', `${net(network)}/nodes/${node}/relay`),
  enableExit: (network: NetworkId, node: NodeId, body: ExitInput) =>
    request<Node>('PUT', `${net(network)}/nodes/${node}/exit`, { body }),
  disableExit: (network: NetworkId, node: NodeId) => request<Node>('DELETE', `${net(network)}/nodes/${node}/exit`),
  succeed: (network: NetworkId, node: NodeId, body: Succeed) =>
    request<Node>('POST', `${net(network)}/nodes/${node}/succeed`, { body }),
  routes: (network: NetworkId, node: NodeId) => request<RouteTable>('GET', `${net(network)}/nodes/${node}/routes`),
  nodeAccess: (network: NetworkId, node: NodeId) => request<NodeAccess>('GET', `${net(network)}/nodes/${node}/access`),

  tokens: (network: NetworkId) => request<EnrollTokenInfo[]>('GET', `${net(network)}/tokens`),
  createToken: (network: NetworkId, body: CreateEnrollToken) =>
    request<CreatedEnrollToken>('POST', `${net(network)}/tokens`, { body }),
  deleteToken: (network: NetworkId, id: number) => request<undefined>('DELETE', `${net(network)}/tokens/${id}`),

  rules: (network: NetworkId) => request<Rule[]>('GET', `${net(network)}/rules`),
  createRule: (network: NetworkId, body: CreateRule) => request<Rule>('POST', `${net(network)}/rules`, { body }),
  updateRule: (network: NetworkId, id: number, body: UpdateRule) =>
    request<Rule>('PATCH', `${net(network)}/rules/${id}`, { body }),
  deleteRule: (network: NetworkId, id: number) => request<undefined>('DELETE', `${net(network)}/rules/${id}`),

  accessGraph: (network: NetworkId) => request<AccessGraph>('GET', `${net(network)}/access/graph`),
  accessCheck: (network: NetworkId, from: NodeId, to: NodeId) =>
    request<AccessCheck>('GET', `${net(network)}/access/check`, { query: { from, to } }),

  credentials: (network: NetworkId) => request<Credential[]>('GET', `${net(network)}/credentials`),
  createCredential: (network: NetworkId, body: CreateCredential) =>
    request<CreatedCredential>('POST', `${net(network)}/credentials`, { body }),
  deleteCredential: (network: NetworkId, id: number) =>
    request<undefined>('DELETE', `${net(network)}/credentials/${id}`),

  listeners: (network: NetworkId, node?: NodeId) =>
    request<Listener[]>('GET', `${net(network)}/listeners`, { query: { node } }),
  createListener: (network: NetworkId, body: ListenerInput) =>
    request<Listener>('POST', `${net(network)}/listeners`, { body }),
  replaceListener: (network: NetworkId, id: number, body: ListenerInput) =>
    request<Listener>('PUT', `${net(network)}/listeners/${id}`, { body }),
  deleteListener: (network: NetworkId, id: number) => request<undefined>('DELETE', `${net(network)}/listeners/${id}`),

  forwards: (network: NetworkId) => request<Forward[]>('GET', `${net(network)}/forwards`),
  createForward: (network: NetworkId, body: ForwardInput) =>
    request<Forward>('POST', `${net(network)}/forwards`, { body }),
  replaceForward: (network: NetworkId, id: number, body: ForwardInput) =>
    request<Forward>('PUT', `${net(network)}/forwards/${id}`, { body }),
  deleteForward: (network: NetworkId, id: number) => request<undefined>('DELETE', `${net(network)}/forwards/${id}`),

  nodeUpdate: (network: NetworkId, node: NodeId) => request<NodeUpdate>('GET', `${net(network)}/nodes/${node}/update`),
  forceNodeUpdate: (network: NetworkId, node: NodeId, body: ForceUpdate) =>
    request<NodeUpdate>('POST', `${net(network)}/nodes/${node}/update`, { body }),

  routeMatrix: (network: NetworkId) => request<RouteMatrix>('GET', `${net(network)}/routes`),
  /** controller 先向两端各问一次，问不到的方向退回到上报的数据，所以最多要等 3 秒左右。 */
  path: (network: NetworkId, from: NodeId, to: NodeId, signal?: AbortSignal) =>
    request<PathView>('GET', `${net(network)}/path`, { query: { from, to }, signal }),

  pairs: (network: NetworkId) => request<PairSetting[]>('GET', `${net(network)}/pairs`),
  setPair: (network: NetworkId, a: NodeId, b: NodeId, body: PairInput) =>
    request<PairSetting>('PUT', `${net(network)}/pairs/${a}/${b}`, { body }),
  deletePair: (network: NetworkId, a: NodeId, b: NodeId) =>
    request<undefined>('DELETE', `${net(network)}/pairs/${a}/${b}`),

  updateOverview: () => request<UpdateOverview>('GET', '/update'),
  updateSettings: (body: UpdateSettings) => request<UpdateOverview>('PATCH', '/update', { body }),
  /** 查完才应答，连不上发布源时最多等 30 秒左右。查不到不算请求失败，原因在 `check_error` 里。 */
  checkUpdate: () => request<UpdateOverview>('POST', '/update/check'),
  resumeUpdate: (body: ResumeUpdate) => request<UpdateOverview>('POST', '/update/resume', { body }),
  forceUpdateAll: (body: ForceUpdate) => request<ForcedUpdates>('POST', '/update/force', { body }),
}
