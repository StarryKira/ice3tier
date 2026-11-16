import { describe, expect, it } from 'vitest'

import { t } from '@/i18n'

import {
  MAX_NAME_LEN,
  MAX_TAG_LEN,
  MAX_TITLE_LEN,
  normalizeCidrList,
  normalizeTag,
  parseOptionalPositiveInt,
  parsePortList,
  splitList,
  validateCidrList,
  validateName,
  validateNetworkCidr,
  validatePort,
  validateSocketAddr,
  validateTag,
  validateTitle,
  validateVirtualIp,
} from './validate'

const v = t.validate

/** 这里的规则照着 docs/admin-api.md §1.3：界面提前指出来的，都是 controller 会拒绝的。 */

describe('网络和节点的名字', () => {
  it('字母、数字、连字符都可以', () => {
    for (const name of ['home', 'nas-01', 'a', 'gw-hk-2', '0a', 'a'.repeat(MAX_NAME_LEN)]) {
      expect(validateName(name), name).toBeNull()
    }
  })

  it('大写会被 controller 转成小写，所以不算错；首尾的空白不算', () => {
    expect(validateName('NAS-01')).toBeNull()
    expect(validateName('  home  ')).toBeNull()
  })

  it('不能是空的', () => {
    expect(validateName('')).toBe(v.nameRequired)
    expect(validateName('   ')).toBe(v.nameRequired)
  })

  it('最长 63 个字符', () => {
    expect(MAX_NAME_LEN).toBe(63)
    expect(validateName('a'.repeat(64))).toBe(v.nameTooLong(63))
  })

  it('别的字符不行：下划线、点、空格、中文', () => {
    for (const name of ['my_nas', 'nas.home', 'my nas', '家里', 'nas/01']) {
      expect(validateName(name), name).toBe(v.nameCharset)
    }
  })

  it('不能以连字符开头或结尾', () => {
    expect(validateName('-nas')).toBe(v.nameHyphen)
    expect(validateName('nas-')).toBe(v.nameHyphen)
  })

  it('不能全是数字', () => {
    expect(validateName('123')).toBe(v.nameAllDigits)
    expect(validateName('12-3')).toBeNull()
  })
})

describe('标签', () => {
  it('字母、数字、连字符、下划线', () => {
    for (const tag of ['servers', 'home_lab', 'dev-2', '42', 'a'.repeat(MAX_TAG_LEN)]) {
      expect(validateTag(tag), tag).toBeNull()
    }
  })

  it('转成小写，去掉首尾空白', () => {
    expect(normalizeTag('  Servers ')).toBe('servers')
    expect(validateTag('  Servers ')).toBeNull()
  })

  it('不能是空的，最长 32 个字符', () => {
    expect(MAX_TAG_LEN).toBe(32)
    expect(validateTag('  ')).toBe(v.tagRequired)
    expect(validateTag('a'.repeat(33))).toBe(v.tagTooLong(32))
  })

  it('别的字符不行', () => {
    for (const tag of ['a b', 'a.b', '服务器', 'a,b']) {
      expect(validateTag(tag), tag).toBe(v.tagCharset)
    }
  })
})

describe('规则、凭据、监听器、端口转发的名字', () => {
  it('可以是任何一小段文字，中文也行', () => {
    expect(validateTitle('手机用的凭据', true)).toBeNull()
    expect(validateTitle('ssh → servers', true)).toBeNull()
  })

  it('要求填的不能是空的；不要求的可以', () => {
    expect(validateTitle('   ', true)).toBe(v.titleRequired)
    expect(validateTitle('', false)).toBeNull()
  })

  it('最长 64 个字符，按字符数不按字节数', () => {
    expect(MAX_TITLE_LEN).toBe(64)
    expect(validateTitle('名'.repeat(64), true)).toBeNull()
    expect(validateTitle('名'.repeat(65), true)).toBe(v.titleTooLong(64))
    // 首尾的空白会被 controller 去掉，不占长度。
    expect(validateTitle(`  ${'a'.repeat(64)}  `, true)).toBeNull()
  })
})

describe('网络的地址段', () => {
  it('CIDR 的写法', () => {
    expect(validateNetworkCidr('10.66.0.0/16')).toBeNull()
    expect(validateNetworkCidr(' 192.168.50.0/24 ')).toBeNull()
    expect(validateNetworkCidr('10.66.0.0/30')).toBeNull()
  })

  it('不能是空的', () => {
    expect(validateNetworkCidr('  ')).toBe(v.cidrRequired)
  })

  it('写法不对', () => {
    for (const cidr of [
      '10.66.0.0',
      '10.66.0/16',
      '10.66.0.0/33',
      '10.66.0.256/16',
      'fd00::/64',
      '10.66.0.0/',
      'home',
    ]) {
      expect(validateNetworkCidr(cidr), cidr).toBe(v.cidrFormat)
    }
  })

  it('太小，放不下两个节点', () => {
    expect(validateNetworkCidr('10.66.0.0/31')).toBe(v.cidrTooSmall(30))
    expect(validateNetworkCidr('10.66.0.1/32')).toBe(v.cidrTooSmall(30))
  })
})

describe('节点的虚拟 IP', () => {
  const cidr = '10.66.0.0/24'

  it('地址段里的地址', () => {
    expect(validateVirtualIp('10.66.0.9', cidr)).toBeNull()
    expect(validateVirtualIp('10.66.0.254', cidr)).toBeNull()
  })

  it('不是 IPv4 地址', () => {
    for (const ip of ['', '10.66.0', '10.66.0.256', '10.66.0.01', 'nas', '10.66.0.9/24']) {
      expect(validateVirtualIp(ip, cidr), ip).toBe(v.ipFormat)
    }
  })

  it('不在地址段里', () => {
    expect(validateVirtualIp('10.67.0.9', cidr)).toBe(v.ipOutOfRange(cidr))
    expect(validateVirtualIp('10.66.1.1', cidr)).toBe(v.ipOutOfRange(cidr))
  })

  it('网络地址和广播地址不能分给节点', () => {
    expect(validateVirtualIp('10.66.0.0', cidr)).toBe(v.ipReserved)
    expect(validateVirtualIp('10.66.0.255', cidr)).toBe(v.ipReserved)
    expect(validateVirtualIp('10.66.255.255', '10.66.0.0/16')).toBe(v.ipReserved)
    // /16 里 10.66.0.255 是普通地址。
    expect(validateVirtualIp('10.66.0.255', '10.66.0.0/16')).toBeNull()
  })
})

describe('端口和地址', () => {
  it('端口是 1 到 65535', () => {
    expect(validatePort('1')).toBeNull()
    expect(validatePort('65535')).toBeNull()
    expect(validatePort(' 8080 ')).toBeNull()
    for (const port of ['0', '65536', '-1', '80a', '', '8 0', '123456']) {
      expect(validatePort(port), port).toBe(v.portFormat)
    }
  })

  it('地址加端口；IPv6 加方括号', () => {
    expect(validateSocketAddr('203.0.113.7:20330')).toBeNull()
    expect(validateSocketAddr('[2001:db8::1]:20330')).toBeNull()
    expect(validateSocketAddr('127.0.0.1:5432')).toBeNull()
  })

  it('写法不对：缺端口、主机名、不带方括号的 IPv6', () => {
    for (const addr of [
      '203.0.113.7',
      'example.com:80',
      '2001:db8::1:20330',
      '203.0.113.7:99999',
      ':80',
      '[203.0.113.7]:80',
    ]) {
      expect(validateSocketAddr(addr), addr).toBe(v.socketAddrFormat)
    }
  })

  it('端口不能是 0，地址不能是 0.0.0.0', () => {
    expect(validateSocketAddr('203.0.113.7:0')).toBe(v.portFormat)
    expect(validateSocketAddr('0.0.0.0:80')).toBe(v.socketAddrUnspecified)
    expect(validateSocketAddr('[::]:80')).toBe(v.socketAddrUnspecified)
  })
})

describe('来源白名单', () => {
  it('一行一个，或者用逗号、空格、分号隔开', () => {
    expect(splitList('192.168.1.0/24, 10.0.0.1\n10.0.0.2；10.0.0.3，10.0.0.4 ')).toEqual([
      '192.168.1.0/24',
      '10.0.0.1',
      '10.0.0.2',
      '10.0.0.3',
      '10.0.0.4',
    ])
    expect(splitList('  \n ')).toEqual([])
  })

  it('每一项是地址段或者单个地址', () => {
    expect(validateCidrList(['192.168.1.0/24', '10.0.0.1'])).toBeNull()
    expect(validateCidrList([])).toBeNull()
  })

  it('指出第一处写错的', () => {
    expect(validateCidrList(['192.168.1.0/24', 'office', '300.1.1.1'])).toBe(v.cidrListItem('office'))
    expect(validateCidrList(['192.168.1.0/40'])).toBe(v.cidrListItem('192.168.1.0/40'))
  })

  it('IPv6 的写法不细查，留给 controller', () => {
    expect(validateCidrList(['fd00::/8', '2001:db8::1'])).toBeNull()
  })

  it('单个 IPv4 地址补成 /32', () => {
    expect(normalizeCidrList(['10.0.0.1', '192.168.1.0/24', '2001:db8::1'])).toEqual([
      '10.0.0.1/32',
      '192.168.1.0/24',
      '2001:db8::1',
    ])
  })
})

describe('出口允许的端口', () => {
  it('去重并排好序', () => {
    expect(parsePortList('443, 80 443\n8080')).toEqual({ ports: [80, 443, 8080], error: null })
  })

  it('有一项不是端口', () => {
    expect(parsePortList('80, https')).toEqual({ ports: [], error: v.portListItem('https') })
    expect(parsePortList('80, 70000')).toEqual({ ports: [], error: v.portListItem('70000') })
  })

  it('一个都没填', () => {
    expect(parsePortList('  ')).toEqual({ ports: [], error: v.portListEmpty })
  })
})

describe('可以留空的正整数', () => {
  it('留空表示不限', () => {
    expect(parseOptionalPositiveInt('  ')).toEqual({ value: null, error: null })
  })

  it('正整数', () => {
    expect(parseOptionalPositiveInt(' 8 ')).toEqual({ value: 8, error: null })
  })

  it('0、负数、小数、文字都不行', () => {
    for (const raw of ['0', '-3', '1.5', 'many', '1e3']) {
      expect(parseOptionalPositiveInt(raw), raw).toEqual({ value: null, error: v.positiveInt })
    }
  })
})
