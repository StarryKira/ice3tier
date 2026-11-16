/**
 * 管理 API 的数据结构。照着 `docs/admin-api.md` 手写，和 `crates/i3t-model/src/admin.rs` 一一对应。
 *
 * 约定（文档 §1）：时间是 Unix 秒；应答里的字段总是全的，没有值写 `null`；请求里标了"可省略"的字段可以不写。
 * 名字以 `Create`、`Update` 开头或以 `Input` 结尾的是请求体，其余是应答体。
 */

/** Unix 秒。 */
export type Timestamp = number

export type NetworkId = number
export type NodeId = number

/** 43 个字符的 base64url。 */
export type PublicKey = string

export type RoutingMode = 'direct_first' | 'lowest_cost'
export type Reach = 'any' | 'direct_only' | 'relay_only'
export type Direction = 'both' | 'one_way'
export type ListenerProtocol = 'socks5' | 'http'
export type ForwardProtocol = 'tcp' | 'udp'

/** 文档 §2 列出的错误码。以后可能增加，所以收到的值按字符串处理，不认识的按 HTTP 状态显示。 */
export type KnownErrorCode =
  | 'unauthorized'
  | 'not_found'
  | 'invalid_request'
  | 'address_out_of_range'
  | 'name_taken'
  | 'name_cooling_down'
  | 'address_taken'
  | 'address_cooling_down'
  | 'range_overlaps'
  | 'range_exhausted'
  | 'range_not_superset'
  | 'network_not_empty'
  | 'not_declared'
  | 'predecessor_online'
  | 'conflict'
  | 'internal'

export interface ErrorBody {
  error: { code: string; message: string }
}

export interface NodeRef {
  id: NodeId
  name: string
  virtual_ip: string
}

export interface Status {
  version: string
  controller_key: PublicKey
  public_addr: string
  networks: number
  nodes: number
  online: number
}

export interface LogEntry {
  id: number
  at: Timestamp
  network: NetworkId | null
  action: string
  detail: string
}

export interface Network {
  id: NetworkId
  name: string
  domain: string
  cidr: string
  routing: RoutingMode
  max_relay_hops: number
  reach: Reach
  created_at: Timestamp
  nodes: number
}

export interface CreateNetwork {
  name: string
  cidr?: string
  routing?: RoutingMode
  max_relay_hops?: number
  reach?: Reach
}

export interface UpdateNetwork {
  cidr?: string
  routing?: RoutingMode
  max_relay_hops?: number
  reach?: Reach
}

export interface NodePair {
  a: NodeRef
  b: NodeRef
}

export interface Overview {
  network: NetworkId
  nodes: number
  online: number
  relays: number
  exits: number
  rules: number
  listeners: number
  forwards: number
  credentials: number
  pending_tokens: number
  online_nodes: NodeRef[]
  unreachable_pairs: NodePair[]
}

export interface Declared {
  exit: boolean
  relay: boolean
  forward: boolean
}

export interface RelaySettings {
  udp: string[]
  tcp: string[]
  region: string
  bandwidth_mbps: number | null
}

export interface RelayInput {
  udp?: string[]
  tcp?: string[]
  region?: string
  bandwidth_mbps?: number | null
}

export interface ExitSettings {
  /** `null` 表示不限制端口。 */
  ports: number[] | null
  lan: boolean
  bandwidth_mbps: number | null
  upstream_socks5: string | null
}

export interface ExitInput {
  /** 省略时是 `[80, 443]`；写 `null` 表示不限制。 */
  ports?: number[] | null
  lan?: boolean
  bandwidth_mbps?: number | null
  upstream_socks5?: string | null
}

export interface Node {
  id: NodeId
  network: NetworkId
  key: PublicKey
  name: string
  fqdn: string
  virtual_ip: string
  tags: string[]
  declared: Declared
  relay: RelaySettings | null
  exit: ExitSettings | null
  dial_out_only: boolean
  routing: RoutingMode | null
  reach: Reach | null
  pinned_relays: NodeId[]
  platform: string
  version: string
  online: boolean
  last_seen_at: Timestamp | null
  created_at: Timestamp
  netmap_version: number
  sent_version: number
  acked_version: number
}

export interface Attachment {
  relay: NodeId
  rtt_ms: number
}

export interface NodeDetail extends Node {
  endpoints: string[]
  observed_endpoint: string | null
  attached: Attachment[]
  mapping_varies: boolean | null
  flows: number
  tx_bytes: number
  rx_bytes: number
}

export interface UpdateNode {
  name?: string
  tags?: string[]
  virtual_ip?: string
  dial_out_only?: boolean
  /** 写 `null` 表示改回用网络的默认值。 */
  routing?: RoutingMode | null
  /** 写 `null` 表示改回用网络的默认值。 */
  reach?: Reach | null
  pinned_relays?: NodeId[]
}

export interface Succeed {
  predecessor: NodeId
}

export type Route = { via: 'none' } | { via: 'direct'; addr: string } | { via: 'relay'; relays: NodeId[] }

export interface NodeRoute {
  peer: NodeRef
  route: Route
  rtt_ms: number | null
}

export interface RouteTable {
  node: NodeRef
  online: boolean
  routes: NodeRoute[]
}

export interface AccessEdge {
  node: NodeRef
  rules: number[]
}

export interface NodeAccess {
  node: NodeRef
  can_reach: AccessEdge[]
  reachable_by: AccessEdge[]
  acknowledged: boolean
}

export interface EnrollTokenInfo {
  id: number
  network: NetworkId
  created_at: Timestamp
  expires_at: Timestamp
  tags: string[]
  virtual_ip: string | null
  successor_of: NodeId | null
}

export interface CreateEnrollToken {
  expires_in_secs?: number
  tags?: string[]
  virtual_ip?: string
  successor_of?: NodeId
}

/** `token` 只在创建的应答里出现这一次。 */
export interface CreatedEnrollToken extends EnrollTokenInfo {
  token: string
}

export interface Rule {
  id: number
  name: string
  src: string
  dst: string
  direction: Direction
  enabled: boolean
}

export interface CreateRule {
  name?: string
  src: string
  dst: string
  direction?: Direction
  enabled?: boolean
}

export type UpdateRule = Partial<Omit<Rule, 'id'>>

export interface TagMembers {
  tag: string
  nodes: NodeRef[]
}

export interface AccessGraph {
  tags: TagMembers[]
  rules: Rule[]
  unconnected: NodeRef[]
  unacknowledged: NodeRef[]
}

export interface AccessCheck {
  from: NodeRef
  to: NodeRef
  allowed: boolean
  rules: Rule[]
}

export interface Credential {
  id: number
  name: string
  max_concurrent: number | null
  created_at: Timestamp
}

export interface CreateCredential {
  name: string
  max_concurrent?: number
}

/** `token` 只在创建的应答里出现这一次。 */
export interface CreatedCredential extends Credential {
  token: string
}

export type DefaultAction = { action: 'local' } | { action: 'reject' } | { action: 'exit'; node: NodeId }

export interface Listener {
  id: number
  node: NodeId
  name: string
  bind: string
  protocol: ListenerProtocol
  allow_from: string[]
  require_credential: boolean
  default_action: DefaultAction | null
  fixed_exit: NodeId | null
}

export interface ListenerInput {
  node: NodeId
  name: string
  bind: string
  protocol: ListenerProtocol
  allow_from?: string[]
  require_credential?: boolean
  default_action?: DefaultAction | null
  fixed_exit?: NodeId | null
}

export type ForwardStatus =
  'target_not_declared' | 'disabled' | 'access_denied' | 'listener_offline' | 'target_offline' | 'active'

export interface Forward {
  id: number
  name: string
  protocol: ForwardProtocol
  listen_node: NodeId
  bind: string
  target_node: NodeId
  target_addr: string
  allow_from: string[]
  enabled: boolean
  status: ForwardStatus
}

export interface ForwardInput {
  name?: string
  protocol: ForwardProtocol
  listen_node: NodeId
  bind: string
  target_node: NodeId
  target_addr: string
  allow_from?: string[]
  enabled?: boolean
}

export interface PairSetting {
  a: NodeId
  b: NodeId
  keep_alive: boolean
}

export interface PairInput {
  keep_alive: boolean
}
