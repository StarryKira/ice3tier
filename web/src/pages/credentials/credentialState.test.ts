import { describe, expect, it } from 'vitest'

import { t } from '@/i18n'
import { makeCredential } from '@/test/fixtures'

import { credentialPrefix, credentialView } from './credentialState'

const s = t.credentials.state
const GB = 1024 ** 3

describe('凭据的状态', () => {
  it('有效、没有上限：绿色，没有要说的；删除这一步是吊销', () => {
    const view = credentialView(
      makeCredential(1, 'phone', { usage: { bytes: 5 * GB, connections: 2, last_used_at: 1 } }),
    )
    expect(view).toMatchObject({ tone: 'ok', label: s.active, detail: null, revoked: false, editable: true })
    expect(view.deleteStep).toBe('revoke')
    expect(view.traffic.ratio).toBeNull()
    expect(view.connections).toBe('2')
  })

  it('有效但流量快用完了：琥珀色，说已用多少', () => {
    const view = credentialView(
      makeCredential(1, 'phone', { max_bytes: 10 * GB, usage: { bytes: 9 * GB, connections: 0, last_used_at: null } }),
    )
    expect(view.tone).toBe('warn')
    expect(view.detail).toBe(s.nearlyDetail(90))
  })

  it('用完了：红色，说清客户端看到什么、怎么恢复；还能改上限、清零', () => {
    const view = credentialView(
      makeCredential(1, 'phone', {
        state: 'exhausted',
        max_bytes: GB,
        usage: { bytes: GB, connections: 0, last_used_at: 1 },
      }),
    )
    expect(view).toMatchObject({ tone: 'danger', label: s.exhausted, detail: s.exhaustedDetail, editable: true })
    expect(view.deleteStep).toBe('revoke')
    expect(view.traffic.exhausted).toBe(true)
  })

  it('吊销了：灰色，记录留着；不能再改，删除这一步是彻底删掉', () => {
    const view = credentialView(makeCredential(1, 'phone', { state: 'revoked', revoked_at: 1_794_000_500 }))
    expect(view).toMatchObject({ tone: 'idle', label: s.revoked, revoked: true, editable: false })
    expect(view.deleteStep).toBe('remove')
  })

  it('不认识的状态：不当成吊销', () => {
    expect(credentialView(makeCredential(1, 'phone', { state: 'unknown' }))).toMatchObject({
      label: s.unknown,
      deleteStep: 'revoke',
    })
  })

  it('并发：有上限时写"现在 / 上限"', () => {
    const view = credentialView(
      makeCredential(1, 'phone', { max_concurrent: 8, usage: { bytes: 0, connections: 3, last_used_at: null } }),
    )
    expect(view.connections).toBe('3 / 8')
  })
})

describe('令牌的前缀', () => {
  it('开头 6 个字符加几个点；这个字段出现之前签发的没有', () => {
    expect(credentialPrefix('I7bRtu')).toBe('I7bRtu••••')
    expect(credentialPrefix('')).toBeNull()
  })
})
