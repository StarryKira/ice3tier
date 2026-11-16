import { describe, expect, it } from 'vitest'

import { type Node } from '@/api/types'
import { t } from '@/i18n'
import { nodeIssues } from '@/lib/nodeIssues'
import { ruleLabel } from '@/lib/rules'
import { formatRelative, formatRemaining } from '@/lib/time'
import { makeNode } from '@/test/fixtures'

import {
  DEFAULT_SORT,
  hasActiveFilters,
  matchesFilters,
  type NodeFilters,
  parseFilters,
  sortNodes,
  writeFilters,
} from './filter'

const node = (id: number, name: string, extra: Partial<Node> = {}): Node => makeNode(id, name, extra)

const RELAY = { udp: ['203.0.113.7:20330'], tcp: [], region: 'hk', bandwidth_mbps: null }
const EXIT = { ports: [80, 443], lan: false, bandwidth_mbps: null, upstream_socks5: null }
const NONE: NodeFilters = { search: '', status: 'all', tags: [], roles: [], platforms: [] }

describe('节点列表的筛选写在地址里', () => {
  it('写进去再读出来是原样', () => {
    const filters: NodeFilters = {
      search: 'nas',
      status: 'offline',
      tags: ['servers', 'home'],
      roles: ['relay', 'exit'],
      platforms: ['linux-x86_64'],
    }
    const params = writeFilters(filters)
    expect(params.toString()).toBe(
      'q=nas&status=offline&tag=servers&tag=home&role=relay&role=exit&platform=linux-x86_64',
    )
    expect(parseFilters(params)).toEqual(filters)
  })

  it('没有筛选时地址里什么都不写', () => {
    expect(writeFilters(NONE).toString()).toBe('')
    expect(hasActiveFilters(NONE)).toBe(false)
    expect(hasActiveFilters({ ...NONE, search: '  ' })).toBe(false)
    expect(hasActiveFilters({ ...NONE, tags: ['servers'] })).toBe(true)
  })

  it('地址里手写的、不认识的值当作没写', () => {
    const parsed = parseFilters(new URLSearchParams('status=broken&role=admin&role=exit'))
    expect(parsed.status).toBe('all')
    expect(parsed.roles).toEqual(['exit'])
  })
})

describe('筛选节点', () => {
  const nas = node(2, 'nas-home', { tags: ['servers', 'home'], exit: EXIT })

  it('搜索匹配名字、全名、虚拟 IP 和标签，不分大小写', () => {
    for (const search of ['NAS', 'home.i3t', '10.66.0.2', 'serv']) {
      expect(matchesFilters(nas, { ...NONE, search }, false), search).toBe(true)
    }
    expect(matchesFilters(nas, { ...NONE, search: 'laptop' }, false)).toBe(false)
  })

  it('条件之间是"并且"', () => {
    expect(matchesFilters(nas, { ...NONE, tags: ['servers', 'home'], roles: ['exit'] }, false)).toBe(true)
    expect(matchesFilters(nas, { ...NONE, tags: ['servers', 'laptops'] }, false)).toBe(false)
    expect(matchesFilters(nas, { ...NONE, roles: ['exit', 'relay'] }, false)).toBe(false)
  })

  it('在线、离线、需要处理', () => {
    expect(matchesFilters(nas, { ...NONE, status: 'online' }, false)).toBe(true)
    expect(matchesFilters(nas, { ...NONE, status: 'offline' }, false)).toBe(false)
    expect(matchesFilters(nas, { ...NONE, status: 'issues' }, false)).toBe(false)
    expect(matchesFilters(nas, { ...NONE, status: 'issues' }, true)).toBe(true)
  })

  it('选了几个平台是"其中之一"', () => {
    expect(matchesFilters(nas, { ...NONE, platforms: ['windows-x86_64', 'linux-x86_64'] }, false)).toBe(true)
    expect(matchesFilters(nas, { ...NONE, platforms: ['windows-x86_64'] }, false)).toBe(false)
  })
})

describe('节点的排序', () => {
  const nodes = [node(3, 'node-10'), node(1, 'node-2'), node(2, 'node-1', { online: false })]

  it('默认按名字，数字按大小而不是按字典序', () => {
    expect(sortNodes(nodes, DEFAULT_SORT).map((entry) => entry.name)).toEqual(['node-1', 'node-2', 'node-10'])
  })

  it('按虚拟 IP 排是按数值', () => {
    const byIp = sortNodes([node(1, 'a', { virtual_ip: '10.66.0.10' }), node(2, 'b', { virtual_ip: '10.66.0.9' })], {
      key: 'ip',
      direction: 'asc',
    })
    expect(byIp.map((entry) => entry.virtual_ip)).toEqual(['10.66.0.9', '10.66.0.10'])
  })

  it('排序键相同的按编号，轮询回来的数据不会让行换位置', () => {
    const same = [node(3, 'a'), node(1, 'a'), node(2, 'a')]
    expect(sortNodes(same, { key: 'version', direction: 'asc' }).map((entry) => entry.id)).toEqual([1, 2, 3])
    expect(sortNodes([...same].reverse(), { key: 'version', direction: 'asc' }).map((entry) => entry.id)).toEqual([
      1, 2, 3,
    ])
  })

  it('不改动传进来的数组', () => {
    const before = nodes.map((entry) => entry.id)
    sortNodes(nodes, { key: 'name', direction: 'desc' })
    expect(nodes.map((entry) => entry.id)).toEqual(before)
  })
})

describe('节点上需要处理的事', () => {
  it('在线、没有固定 relay 的普通节点没有事', () => {
    const laptop = node(3, 'laptop')
    expect(nodeIssues(laptop, [laptop])).toEqual([])
  })

  it('relay 或出口离线', () => {
    const gw = node(1, 'gw-hk', { relay: RELAY, exit: EXIT, online: false })
    expect(nodeIssues(gw, [gw]).map((issue) => issue.kind)).toEqual(['relay_offline', 'exit_offline'])
  })

  it('固定挂靠的 relay 已经不是 relay，或者已经不在了：写出是哪几个', () => {
    const former = node(1, 'gw-hk')
    const laptop = node(3, 'laptop', { pinned_relays: [1, 9] })
    const issues = nodeIssues(laptop, [former, laptop])
    expect(issues).toHaveLength(1)
    expect(issues[0]?.kind).toBe('pinned_gone')
    expect(issues[0]?.message).toBe(t.nodes.issue.pinnedGone('gw-hk、#9'))
  })

  it('固定挂靠的 relay 还在就没有事', () => {
    const gw = node(1, 'gw-hk', { relay: RELAY })
    const laptop = node(3, 'laptop', { pinned_relays: [1] })
    expect(nodeIssues(laptop, [gw, laptop])).toEqual([])
  })
})

describe('时间的写法', () => {
  const now = 1_794_800_000

  it('一周以内写相对时间', () => {
    expect(formatRelative(now - 10, now)).toBe(t.time.justNow)
    expect(formatRelative(now - 3 * 60, now)).toBe(t.time.minutesAgo(3))
    expect(formatRelative(now - 2 * 3600, now)).toBe(t.time.hoursAgo(2))
    expect(formatRelative(now - 3 * 86_400, now)).toBe(t.time.daysAgo(3))
  })

  it('刚发生的事不会因为钟差了几秒就写成一个日期', () => {
    // 界面上的"现在"每 20 秒才走一格，所以刚加入的节点的时间戳会比它晚一点。
    expect(formatRelative(now + 15, now)).toBe(t.time.justNow)
    expect(formatRelative(now + 120, now)).toBe(t.time.justNow)
    // 差得太多就不是钟的问题了，照实写出来。
    expect(formatRelative(now + 3600, now)).toMatch(/^\d{2}\/\d{2} \d{2}:\d{2}$/)
  })

  it('更早的写日期', () => {
    expect(formatRelative(now - 8 * 86_400, now)).toMatch(/^\d{2}\/\d{2} \d{2}:\d{2}$/)
    // 不是今年的带上年份。
    expect(formatRelative(now - 400 * 86_400, now)).toMatch(/^\d{4}\/\d{2}\/\d{2} \d{2}:\d{2}$/)
  })

  it('令牌还剩多久；过了就是 null', () => {
    expect(formatRemaining(now + 30, now)).toBe(t.time.lessThanMinute)
    expect(formatRemaining(now + 10 * 60, now)).toBe(t.time.minutes(10))
    expect(formatRemaining(now + 86_400, now)).toBe(t.time.hours(24))
    expect(formatRemaining(now + 7 * 86_400, now)).toBe(t.time.days(7))
    expect(formatRemaining(now, now)).toBeNull()
    expect(formatRemaining(now - 5, now)).toBeNull()
  })
})

describe('规则在文字里的叫法', () => {
  it('有名字用名字，没有就写成两个标签加方向', () => {
    expect(ruleLabel({ name: 'ssh', src: 'laptops', dst: 'servers', direction: 'one_way' })).toBe('ssh')
    expect(ruleLabel({ name: '', src: 'laptops', dst: 'servers', direction: 'one_way' })).toBe('laptops → servers')
    expect(ruleLabel({ name: '  ', src: 'dev', dst: 'srv', direction: 'both' })).toBe('dev ⇄ srv')
  })
})
