import { describe, expect, it } from 'vitest'

import { type ListenStatus } from '@/api/types'
import { t } from '@/i18n'

import { listenView } from './listenState'

const s = t.listen
const traffic = { connections: 2, bytes_up: 1024, bytes_down: 40960 }
const at = (state: Partial<ListenStatus> & Pick<ListenStatus, 'state'>): ListenStatus =>
  ({ ...traffic, ...state }) as ListenStatus
const context = { node: 'laptop', bind: '127.0.0.1:8080', online: true }

describe('一个监听的状态', () => {
  it('正在监听：带着连接数和流量，没有要做的事', () => {
    const view = listenView(at({ state: 'listening', local: '127.0.0.1:8080' }), context)
    expect(view).toMatchObject({ tone: 'ok', label: s.listening, detail: null, needsAttention: false })
    expect(view.local).toBe('127.0.0.1:8080')
    expect(view.picked).toBeNull()
    expect(view.traffic).toEqual(traffic)
  })

  it('端口写 0：节点挑定的端口从实际的地址里取出来', () => {
    const view = listenView(at({ state: 'listening', local: '127.0.0.1:41873' }), { ...context, bind: '127.0.0.1:0' })
    expect(view.picked).toBe(41873)
    expect(view.local).toBe('127.0.0.1:41873')
  })

  it('端口被占用：说是哪台机器上的哪个端口、系统怎么说的、怎么办', () => {
    const view = listenView(at({ state: 'port_in_use', detail: 'Address already in use (os error 98)' }), context)
    expect(view.tone).toBe('danger')
    expect(view.needsAttention).toBe(true)
    expect(view.label).toBe(s.portInUse)
    expect(view.detail).toBe(s.portInUseDetail('laptop', '8080', 'Address already in use (os error 98)'))
    expect(view.local).toBeNull()
    // 没有在监听：连接数和流量的 0 没有意思，不写。
    expect(view.traffic).toBeNull()
  })

  it('端口写 0 而端口被占用（挑定的那个后来被占了）：原因里写配置的地址', () => {
    const view = listenView(at({ state: 'port_in_use', detail: 'in use' }), { ...context, bind: '127.0.0.1:0' })
    expect(view.detail).toContain('127.0.0.1:0')
  })

  it('没有绑上、不支持：红色，带系统给的原因', () => {
    const failed = listenView(at({ state: 'failed', detail: 'Cannot assign requested address' }), {
      ...context,
      bind: '192.168.9.9:1080',
    })
    expect(failed).toMatchObject({ tone: 'danger', label: s.failed, needsAttention: true })
    expect(failed.detail).toBe(s.failedDetail('laptop', '192.168.9.9:1080', 'Cannot assign requested address'))
    const unsupported = listenView(at({ state: 'unsupported', detail: '虚拟 IP 上不能写 0' }), context)
    expect(unsupported).toMatchObject({ tone: 'danger', label: s.unsupported, needsAttention: true })
  })

  it('监听在虚拟 IP 上：正常，不占系统端口', () => {
    expect(listenView(at({ state: 'virtual' }), context)).toMatchObject({
      tone: 'ok',
      label: s.virtual,
      needsAttention: false,
    })
  })

  it('节点报了不认识的状态：不当成故障', () => {
    expect(listenView(at({ state: 'unknown' }), context)).toMatchObject({ tone: 'idle', needsAttention: false })
  })

  it('还没有上报：节点在线时是在等它报，离线时直说', () => {
    expect(listenView(null, context)).toMatchObject({ tone: 'idle', label: s.pending, traffic: null })
    expect(listenView(null, { ...context, online: false })).toMatchObject({ label: s.offline })
  })
})
