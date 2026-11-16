import { type MatrixEntry, type Node, type NodeUpdate, type Route, type Status, type UpdateOverview } from '@/api/types'

/** 测试里用到的节点、controller 的情况：字段全的默认值，测试只写它关心的那几项。 */

export const IDLE_UPDATE: NodeUpdate = {
  self_update: { able: true, blocker: null },
  available: null,
  status: { state: 'idle' },
  ordered: null,
  waiting: null,
  failure: null,
}

export function makeUpdate(extra: Partial<NodeUpdate> = {}): NodeUpdate {
  return { ...IDLE_UPDATE, ...extra }
}

export function makeNode(id: number, name: string, extra: Partial<Node> = {}): Node {
  return {
    id,
    network: 1,
    key: 'AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE',
    name,
    fqdn: `${name}.home.i3t`,
    virtual_ip: `10.66.0.${id}`,
    tags: [],
    declared: { exit: false, relay: false, forward: false },
    relay: null,
    exit: null,
    dial_out_only: false,
    routing: null,
    reach: null,
    pinned_relays: [],
    platform: 'linux-x86_64',
    version: '0.1.0',
    online: true,
    last_seen_at: 1_794_000_000,
    created_at: 1_794_000_000,
    netmap_version: 1,
    sent_version: 1,
    acked_version: 1,
    hostname: name,
    update: IDLE_UPDATE,
    ...extra,
  }
}

export function makeStatus(extra: Partial<Status> = {}): Status {
  return {
    version: '0.1.0',
    controller_key: 'LltlBKa6eUXymVVonwneDvQzlnt_0z1jqdmwdPsruNY',
    public_addr: 'ctl.example.com:20331',
    networks: 1,
    nodes: 0,
    online: 0,
    newer_version: null,
    ...extra,
  }
}

/** 还没有查到发布的更新现状。 */
export function makeOverview(extra: Partial<UpdateOverview> = {}): UpdateOverview {
  return {
    auto_update: true,
    source: 'https://github.com/fuqiuluo/ice3tier/releases',
    source_is_default: true,
    controller_version: '0.1.0',
    release: null,
    checked_at: null,
    check_error: null,
    paused: [],
    nodes: { total: 0, updating: 0, failed: 0, manual: 0, outdated: 0, up_to_date: 0 },
    ...extra,
  }
}

export const direct = (addr = '203.0.113.7:20330'): Route => ({ via: 'direct', addr })
export const viaRelays = (...relays: number[]): Route => ({ via: 'relay', relays })
export const noRoute: Route = { via: 'none' }

export function makeEntry(from: number, to: number, route: Route, extra: Partial<MatrixEntry> = {}): MatrixEntry {
  return { from, to, route, rtt_ms: 10, age_secs: 4, stale: false, ...extra }
}
