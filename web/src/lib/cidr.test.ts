import { describe, expect, it } from 'vitest'

import {
  cidrContains,
  cidrContainsAddress,
  cidrOverlaps,
  formatCidr,
  formatIpv4,
  formatSocketAddr,
  isLoopbackBind,
  isNetworkOrBroadcast,
  parseCidr,
  parseIpv4,
  parseSocketAddr,
  SHARED_ADDRESS_SPACE,
  suggestCidr,
} from './cidr'

function cidr(text: string) {
  const parsed = parseCidr(text)
  if (parsed === null) throw new Error(`不是地址段：${text}`)
  return parsed
}

function ip(text: string) {
  const parsed = parseIpv4(text)
  if (parsed === null) throw new Error(`不是地址：${text}`)
  return parsed
}

describe('IPv4 地址', () => {
  it('解析再写回来是原样，最高位是 1 的地址也不会变成负数', () => {
    for (const text of ['0.0.0.0', '10.66.0.5', '192.168.1.10', '255.255.255.255']) {
      expect(formatIpv4(ip(text))).toBe(text)
    }
    expect(ip('255.255.255.255')).toBe(0xffffffff)
  })

  it('不接受的写法', () => {
    for (const text of [
      '',
      '10.66.0',
      '10.66.0.1.2',
      '10.66.0.256',
      '10.66.0.-1',
      '10.66.0.01',
      '10.66.0.x',
      '0x0a.0.0.1',
    ]) {
      expect(parseIpv4(text), text).toBeNull()
    }
  })
})

describe('地址段', () => {
  it('主机位不是 0 的规整成网络地址', () => {
    expect(formatCidr(cidr('10.66.3.7/16'))).toBe('10.66.0.0/16')
    expect(formatCidr(cidr('0.0.0.0/0'))).toBe('0.0.0.0/0')
    expect(formatCidr(cidr('192.168.1.200/25'))).toBe('192.168.1.128/25')
  })

  it('包含某个地址', () => {
    expect(cidrContainsAddress(cidr('10.66.0.0/16'), ip('10.66.255.1'))).toBe(true)
    expect(cidrContainsAddress(cidr('10.66.0.0/16'), ip('10.67.0.1'))).toBe(false)
    expect(cidrContainsAddress(cidr('192.168.0.0/16'), ip('192.168.1.10'))).toBe(true)
  })

  it('一个段完整地包含另一个（扩大地址段时要满足）', () => {
    expect(cidrContains(cidr('10.66.0.0/15'), cidr('10.66.0.0/16'))).toBe(true)
    expect(cidrContains(cidr('10.66.0.0/16'), cidr('10.66.0.0/16'))).toBe(true)
    expect(cidrContains(cidr('10.66.0.0/16'), cidr('10.66.0.0/15'))).toBe(false)
    expect(cidrContains(cidr('10.64.0.0/16'), cidr('10.66.0.0/16'))).toBe(false)
  })

  it('重叠：一个包含另一个就算，不相交不算', () => {
    expect(cidrOverlaps(cidr('10.66.0.0/16'), cidr('10.66.4.0/24'))).toBe(true)
    expect(cidrOverlaps(cidr('10.66.4.0/24'), cidr('10.66.0.0/16'))).toBe(true)
    expect(cidrOverlaps(cidr('10.66.0.0/16'), cidr('10.67.0.0/16'))).toBe(false)
  })

  it('运营商级 NAT 的保留段是 100.64.0.0/10', () => {
    expect(formatCidr(SHARED_ADDRESS_SPACE)).toBe('100.64.0.0/10')
    expect(cidrOverlaps(SHARED_ADDRESS_SPACE, cidr('100.100.0.0/16'))).toBe(true)
    expect(cidrOverlaps(SHARED_ADDRESS_SPACE, cidr('100.128.0.0/16'))).toBe(false)
  })

  it('网络地址和广播地址', () => {
    const range = cidr('10.66.0.0/24')
    expect(isNetworkOrBroadcast(range, ip('10.66.0.0'))).toBe(true)
    expect(isNetworkOrBroadcast(range, ip('10.66.0.255'))).toBe(true)
    expect(isNetworkOrBroadcast(range, ip('10.66.0.1'))).toBe(false)
  })
})

describe('新建网络时预填的地址段', () => {
  it('第一个网络是 10.66.0.0/16', () => {
    expect(suggestCidr([])).toBe('10.66.0.0/16')
  })

  it('跳过和已有网络重叠的', () => {
    expect(suggestCidr(['10.66.0.0/16'])).toBe('10.67.0.0/16')
    expect(suggestCidr(['10.66.0.0/16', '10.67.4.0/24'])).toBe('10.68.0.0/16')
    // 一个大段把后面的都盖住了。
    expect(suggestCidr(['10.64.0.0/14'])).toBe('10.68.0.0/16')
  })

  it('已有的写法读不懂时不影响', () => {
    expect(suggestCidr(['not-a-cidr'])).toBe('10.66.0.0/16')
  })

  it('都被占了就不预填', () => {
    expect(suggestCidr(['10.0.0.0/8'])).toBe('')
  })
})

describe('地址加端口', () => {
  it('IPv4 和带方括号的 IPv6', () => {
    expect(parseSocketAddr('203.0.113.7:20330')).toEqual({ host: '203.0.113.7', port: 20330, ipv6: false })
    expect(parseSocketAddr(' [2001:db8::1]:443 ')).toEqual({ host: '2001:db8::1', port: 443, ipv6: true })
  })

  it('写回去时 IPv6 加方括号', () => {
    expect(formatSocketAddr('203.0.113.7', 80)).toBe('203.0.113.7:80')
    expect(formatSocketAddr('2001:db8::1', 80)).toBe('[2001:db8::1]:80')
  })

  it('只有本机能连上的监听地址', () => {
    expect(isLoopbackBind('127.0.0.1:1080')).toBe(true)
    expect(isLoopbackBind('127.8.8.8:1080')).toBe(true)
    expect(isLoopbackBind('[::1]:1080')).toBe(true)
    expect(isLoopbackBind('0.0.0.0:1080')).toBe(false)
    expect(isLoopbackBind('192.168.1.5:1080')).toBe(false)
    // 读不懂的按"不是"算，宁可多提醒一句。
    expect(isLoopbackBind('localhost:1080')).toBe(false)
  })
})
