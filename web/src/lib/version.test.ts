import manifest from '../../package.json?raw'
import { describe, expect, it } from 'vitest'

import { compareVersions, isOlder, WEB_VERSION } from './version'

describe('界面自己的版本号', () => {
  it('就是 package.json 里的 version：构建时写进来，只有这一个来处', () => {
    const { version } = JSON.parse(manifest) as { version: string }
    expect(WEB_VERSION).toBe(version)
    expect(WEB_VERSION).toBe(__APP_VERSION__)
    expect(WEB_VERSION).toMatch(/^\d+\.\d+\.\d+/)
  })
})

describe('compareVersions：和 controller 一样按语义化版本比', () => {
  it('先比三个数字，按数值而不是按字符串', () => {
    expect(compareVersions('0.10.0', '0.9.9')).toBe(1)
    expect(compareVersions('0.2.0', '0.2.0')).toBe(0)
    expect(compareVersions('1.0.0', '0.99.99')).toBe(1)
    expect(compareVersions('0.1.9', '0.1.10')).toBe(-1)
  })

  it('数字相同时预发布版比正式版小，预发布标识逐段比', () => {
    expect(compareVersions('0.3.0-rc.1', '0.3.0')).toBe(-1)
    expect(compareVersions('0.3.0', '0.3.0-rc.1')).toBe(1)
    expect(compareVersions('0.3.0-rc.2', '0.3.0-rc.10')).toBe(-1)
    // 纯数字的段比带字母的段小；前面都相同时段数多的大。
    expect(compareVersions('0.3.0-1', '0.3.0-alpha')).toBe(-1)
    expect(compareVersions('0.3.0-alpha', '0.3.0-alpha.1')).toBe(-1)
    expect(compareVersions('0.3.0-beta', '0.3.0-alpha.9')).toBe(1)
  })

  it('写法不对的比不了；比不了时不算旧，不报警', () => {
    expect(compareVersions('', '0.1.0')).toBeNull()
    expect(compareVersions('0.1', '0.1.0')).toBeNull()
    expect(isOlder('', '0.2.0')).toBe(false)
    expect(isOlder('0.1.0', '0.2.0')).toBe(true)
    expect(isOlder('0.2.0', '0.2.0')).toBe(false)
  })
})
