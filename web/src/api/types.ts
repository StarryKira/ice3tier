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

/** 语义化版本号：`主.次.修订`，预发布版后面跟 `-标识`，例如 `0.3.0-rc.1`。 */
export type Version = string

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
  | 'node_offline'
  | 'cannot_self_update'
  | 'no_update'
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
  /** 已知的最新正式版比 controller 自己新时是那个版本号，否则 `null`。controller 不会自己更新。 */
  newer_version: Version | null
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
  /** 节点上报的主机名，原样；旧版本的节点不报，是 `""`。主机名后来变了，`name` 不跟着变。 */
  hostname: string
  update: NodeUpdate
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

/* 节点更新（文档 §14）。设置和发布全局只有一份，不分网络。 */

/** 节点不能自己更新的原因。 */
export type SelfUpdateBlocker = 'container' | 'package_manager' | 'read_only_dir'

export interface SelfUpdate {
  able: boolean
  /** `able` 是 `false` 时的原因；节点报了一个 controller 不认识的原因时是 `null`。 */
  blocker: SelfUpdateBlocker | null
}

/** 换程序的时机。`idle` 是自动更新用的；管理员强制更新只能选另外两种。 */
export type UpdateTiming = 'idle' | 'now' | 'after_drain'
export type ForceTiming = Exclude<UpdateTiming, 'idle'>

/** `not_installed`：没装成，和版本好坏无关，以后会再试。`version_rejected`：这个版本在这台机器上不能用。 */
export type FailureKind = 'not_installed' | 'version_rejected'

/** 新程序已经准备好或者正在准备，只带版本号的那几种进度。 */
export type UpdateStepState =
  'verifying' | 'self_check' | 'waiting_for_idle' | 'draining' | 'installing' | 'restarting' | 'confirmed'

/** 节点本次连接期间最近一次报的更新进度。`unknown` 是比 controller 新的节点报上来的、controller 不认识的状态。 */
export type UpdateStatus =
  | { state: 'idle' }
  | { state: 'downloading'; version: Version; received: number; total: number }
  | { state: UpdateStepState; version: Version }
  | { state: 'failed'; version: Version | null; kind: FailureKind; reason: string }
  | { state: 'unknown' }

export interface UpdateOrder {
  version: Version
  timing: UpdateTiming
}

/** controller 想让节点更新、但还没有下发指令的原因。 */
export type UpdateWait = 'package' | 'relay_turn' | 'unknown'

export interface UpdateFailure {
  version: Version
  kind: FailureKind
  reason: string
  at: Timestamp
}

/** 一个节点的程序更新。`Node.update` 和 `GET …/nodes/{node}/update` 是同一个东西。 */
export interface NodeUpdate {
  /** `null`：节点的版本太旧，不懂更新，只能手动换程序。 */
  self_update: SelfUpdate | null
  /** 现在可以让它更新到的版本：发布比它新、有它的平台的包、controller 也够新。不管它能不能自己更新、在不在线都给出。 */
  available: Version | null
  status: UpdateStatus
  ordered: UpdateOrder | null
  waiting: UpdateWait | null
  failure: UpdateFailure | null
}

export type PackageState = 'not_needed' | 'fetching' | 'cached' | 'failed' | 'unknown'

export interface PackageInfo {
  platform: string
  file: string
  /** 字节数。 */
  size: number
  state: PackageState
  /** 最近一次取这个包失败的原因。 */
  error: string | null
}

export interface ReleaseInfo {
  version: Version
  published: Timestamp
  min_controller: Version
  /** controller 比 `min_controller` 旧：这个版本不会下发给任何节点。 */
  controller_too_old: boolean
  /** 这个版本比 controller 自己新：controller 要手动更新。 */
  newer_than_controller: boolean
  packages: PackageInfo[]
}

export interface PausedVersion {
  version: Version
  /** 报告失败的节点；后来被删掉了是 `null`。 */
  node: NodeRef | null
  network: NetworkId | null
  reason: string
  at: Timestamp
}

/** 全部网络的节点按更新情况数出来的数。一个节点只算在一项里。 */
export interface UpdateCounts {
  total: number
  updating: number
  failed: number
  manual: number
  outdated: number
  up_to_date: number
}

export interface UpdateOverview {
  auto_update: boolean
  /** `http(s)://` 地址前缀，或者 controller 所在机器上一个目录的绝对路径。 */
  source: string
  source_is_default: boolean
  controller_version: Version
  release: ReleaseInfo | null
  checked_at: Timestamp | null
  /** 最近一次查发布源失败的原因；失败时 `release` 保持上一次查到的。 */
  check_error: string | null
  paused: PausedVersion[]
  nodes: UpdateCounts
}

export interface UpdateSettings {
  auto_update?: boolean
  /** 写 `null` 表示改回项目默认的发布源。 */
  source?: string | null
}

export interface ForceUpdate {
  timing: ForceTiming
}

export interface ResumeUpdate {
  version: Version
}

export interface SkippedUpdate {
  node: NodeRef
  /** 给人看的一句话。 */
  reason: string
}

/** "全部更新"的结果。 */
export interface ForcedUpdates {
  /** 没有可以下发的版本时是 `null`，这时节点都在 `skipped` 里。 */
  version: Version | null
  ordered: NodeRef[]
  /** 稍后下发：在等包取回来，或者是 relay 在等轮到它。 */
  queued: NodeRef[]
  skipped: SkippedUpdate[]
}

/* 路由视图（文档 §15）。数据都来自节点的上报；路是发送方选的，两个方向可以不一样。 */

export interface MatrixNode extends NodeRef {
  online: boolean
}

/** 矩阵里的一格：`from` 到 `to` 这个方向现在怎么走。 */
export interface MatrixEntry {
  from: NodeId
  to: NodeId
  route: Route
  rtt_ms: number | null
  /** `from` 最近一次上报到现在过了多少秒。 */
  age_secs: number
  /** 不是 `from` 现在这次连接报的：它不在线，或者刚重新连上还没有上报。 */
  stale: boolean
}

export interface RouteMatrix {
  /** 网络里的全部节点，按编号排。 */
  nodes: MatrixNode[]
  /** 有数据的格子，按 `from`、`to` 排。没出现的格子是没有数据。 */
  entries: MatrixEntry[]
}

/**
 * 一个方向的数据从哪里来。`query`：刚问的发送方；`report`：问不到，用它最近一次的上报；
 * `not_peers`：发送方说对端不在它的配置里；`none`：没有数据。`unknown` 是更新的 controller 才有的取值。
 */
export type PathSource = 'query' | 'report' | 'not_peers' | 'none' | 'unknown'

export interface RouteSince {
  secs_ago: number
  reason: string
}

export interface PathLeg {
  from: NodeRef
  to: NodeRef
  source: PathSource
  route: Route | null
  rtt_ms: number | null
  /** 刚问到的是 0；没有数据是 `null`。 */
  age_secs: number | null
  /** 只有 `source` 是 `query`、并且节点这次启动以来换过这条路时才有。 */
  since: RouteSince | null
}

export interface PathView {
  /** `from` 到 `to`。 */
  forward: PathLeg
  /** `to` 到 `from`。 */
  reverse: PathLeg
}
