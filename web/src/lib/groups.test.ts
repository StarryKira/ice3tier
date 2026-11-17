import { describe, expect, it } from 'vitest'

import { isAllGroup } from './groups'

describe('认内置的 All', () => {
  it('看 controller 给的 builtin，不看名字', () => {
    expect(isAllGroup({ name: 'All', builtin: true })).toBe(true)
    expect(isAllGroup({ name: 'Everyone', builtin: true })).toBe(true)
    expect(isAllGroup({ name: 'All', builtin: false })).toBe(false)
  })

  it('旧的 controller 不给 builtin：按名字认', () => {
    expect(isAllGroup({ name: 'All' })).toBe(true)
    expect(isAllGroup({ name: 'servers' })).toBe(false)
  })
})
