import { describe, expect, it } from 'vitest'

import { type ReleaseInfo, type UpdateStatus } from '@/api/types'
import { setLocale, t } from '@/i18n'
import { en } from '@/i18n/en'
import { makeNode, makeUpdate } from '@/test/fixtures'

import { nodeIssues } from './nodeIssues'
import {
  describeUpdate,
  forceAbility,
  groupPaused,
  isUpdating,
  NO_UPDATE_CONTEXT,
  selfUpdateBlock,
  type UpdateContext,
  updateMark,
} from './nodeUpdate'
import { sourceKind, validateSource } from './releaseSource'

const u = t.updates

const RELEASE: ReleaseInfo = {
  version: '0.3.0',
  published: 1_794_000_000,
  min_controller: '0.2.0',
  controller_too_old: false,
  newer_than_controller: false,
  packages: [{ platform: 'linux-x86_64', file: 'i3t-linux-x86_64', size: 18_000_000, state: 'cached', error: null }],
}
const AUTO: UpdateContext = { release: RELEASE, autoUpdate: true, paused: [] }
const MANUAL: UpdateContext = { release: RELEASE, autoUpdate: false, paused: [] }
const when = () => t.time.minutesAgo(3)

const withStatus = (status: UpdateStatus) => makeNode(1, 'nas', { update: makeUpdate({ status }) })

describe('describeUpdate：节点的更新情况怎么说', () => {
  it('没有可以更新到的版本、也不比已知的发布旧：已是最新，灰色', () => {
    const view = describeUpdate(makeNode(1, 'nas', { version: '0.3.0' }), AUTO, when)
    expect(view).toMatchObject({ kind: 'up_to_date', tone: 'idle', label: u.state.upToDate })
  })

  it('有新版本：琥珀色；下一步按自动更新开没开、在不在线、暂停没暂停来写', () => {
    const node = makeNode(1, 'nas', { update: makeUpdate({ available: '0.3.0' }) })
    expect(describeUpdate(node, AUTO, when)).toMatchObject({
      kind: 'available',
      tone: 'warn',
      label: u.state.available('0.3.0'),
      detail: u.state.availableAuto,
    })
    expect(describeUpdate(node, MANUAL, when).detail).toBe(u.state.availableManual)
    // 设置里开关就在表的上面：不再每行都说"自动更新已关闭"。
    expect(describeUpdate(node, MANUAL, when, { autoSwitchShown: true }).detail).toBeNull()
    expect(describeUpdate({ ...node, online: false }, AUTO, when).detail).toBe(u.state.availableOffline)
    expect(describeUpdate(node, { ...AUTO, paused: ['0.3.0'] }, when).detail).toBe(u.state.availablePaused('0.3.0'))
  })

  it('有新版本但不能自己更新：灰色，写原因和做法', () => {
    const node = makeNode(1, 'nas', {
      update: makeUpdate({ available: '0.3.0', self_update: { able: false, blocker: 'container' } }),
    })
    const view = describeUpdate(node, AUTO, when)
    expect(view).toMatchObject({ kind: 'manual', tone: 'idle', label: u.state.cannotSelf })
    expect(view.detail).toContain(u.blocker.container)
    expect(view.detail).toContain(u.blocker.containerHow)
  })

  it('下载中带进度；准备好之后是"等待空闲"或"排空中"，都是进行中的颜色', () => {
    const downloading = describeUpdate(
      withStatus({ state: 'downloading', version: '0.3.0', received: 4_500_000, total: 18_000_000 }),
      AUTO,
      when,
    )
    expect(downloading).toMatchObject({ kind: 'in_progress', tone: 'relay', label: u.state.downloading(25) })
    expect(downloading.progress).toBeCloseTo(0.25)
    expect(describeUpdate(withStatus({ state: 'waiting_for_idle', version: '0.3.0' }), AUTO, when)).toMatchObject({
      kind: 'waiting',
      tone: 'relay',
      label: u.state.waitingForIdle,
    })
    expect(describeUpdate(withStatus({ state: 'draining', version: '0.3.0' }), AUTO, when).label).toBe(u.state.draining)
    expect(describeUpdate(withStatus({ state: 'confirmed', version: '0.3.0' }), AUTO, when)).toMatchObject({
      kind: 'confirmed',
      tone: 'ok',
    })
  })

  it('失败：红色。版本不能用的说"不会再自动装"，没装成的说"以后会再试"；节点的原话单独放着', () => {
    const rejected = describeUpdate(
      withStatus({ state: 'failed', version: '0.3.0', kind: 'version_rejected', reason: '自检没有通过' }),
      AUTO,
      when,
    )
    expect(rejected).toMatchObject({
      kind: 'failed',
      tone: 'danger',
      detail: u.state.failedRejected('0.3.0'),
      reason: '自检没有通过',
    })
    const notInstalled = describeUpdate(
      withStatus({ state: 'failed', version: null, kind: 'not_installed', reason: '磁盘满了' }),
      AUTO,
      when,
    )
    expect(notInstalled).toMatchObject({ detail: u.state.failedNotInstalled, reason: '磁盘满了' })
  })

  it('上次装这个版本失败过：比"有新版本"优先，写出时间和原因', () => {
    const node = makeNode(1, 'nas', {
      update: makeUpdate({
        available: '0.3.0',
        failure: { version: '0.3.0', kind: 'version_rejected', reason: '连不上 controller', at: 1_794_000_000 },
      }),
    })
    expect(describeUpdate(node, AUTO, when)).toMatchObject({
      kind: 'failed',
      label: u.state.lastFailed,
      detail: u.state.lastFailedDetail('0.3.0', t.time.minutesAgo(3)),
      reason: '连不上 controller',
    })
  })

  it('已经下发、在等包、在等轮到它（relay）：写出在等什么', () => {
    const ordered = makeNode(1, 'nas', {
      update: makeUpdate({ available: '0.3.0', ordered: { version: '0.3.0', timing: 'after_drain' } }),
    })
    expect(describeUpdate(ordered, AUTO, when).detail).toBe(u.state.ordered('0.3.0', 'after_drain'))
    const pkg = makeNode(1, 'nas', { update: makeUpdate({ available: '0.3.0', waiting: 'package' }) })
    expect(describeUpdate(pkg, AUTO, when)).toMatchObject({
      kind: 'waiting',
      label: u.state.waitingPackage,
      detail: u.state.waitingPackageDetail('linux-x86_64'),
    })
    const turn = makeNode(1, 'hk', { update: makeUpdate({ available: '0.3.0', waiting: 'relay_turn' }) })
    expect(describeUpdate(turn, AUTO, when).label).toBe(u.state.waitingRelayTurn)
  })

  it('比已知的发布旧、却没有可以更新到的版本：说出是缺包还是 controller 太旧', () => {
    const arm = makeNode(1, 'pi', { platform: 'linux-aarch64' })
    expect(describeUpdate(arm, AUTO, when)).toMatchObject({
      kind: 'behind',
      tone: 'warn',
      label: u.state.behind('0.3.0'),
      detail: u.state.behindNoPackage('0.3.0', 'linux-aarch64'),
    })
    const tooOld = { ...AUTO, release: { ...RELEASE, controller_too_old: true } }
    expect(describeUpdate(makeNode(1, 'nas'), tooOld, when).detail).toBe(
      u.state.behindControllerTooOld('0.3.0', '0.2.0'),
    )
  })

  it('不知道发布、节点没报版本、进度不认识：不乱说', () => {
    expect(describeUpdate(makeNode(1, 'nas'), NO_UPDATE_CONTEXT, when).kind).toBe('up_to_date')
    expect(describeUpdate(makeNode(1, 'nas', { version: '' }), AUTO, when).kind).toBe('never_connected')
    expect(describeUpdate(withStatus({ state: 'unknown' }), AUTO, when)).toMatchObject({
      kind: 'unknown',
      tone: 'idle',
    })
  })
})

describe('selfUpdateBlock：不能自己更新的原因和做法', () => {
  it('三种已知的原因各有做法；容器和包管理器不给替换命令，目录不可写的给', () => {
    const block = (blocker: 'container' | 'package_manager' | 'read_only_dir') =>
      selfUpdateBlock(makeUpdate({ self_update: { able: false, blocker } }))
    expect(block('container')).toMatchObject({ reason: u.blocker.container, commands: false })
    expect(block('package_manager')).toMatchObject({ reason: u.blocker.package_manager, commands: false })
    expect(block('read_only_dir')).toMatchObject({ reason: u.blocker.read_only_dir, commands: true })
  })

  it('能更新时没有；版本太旧不懂更新、原因不认识的，都让手动替换', () => {
    expect(selfUpdateBlock(makeUpdate())).toBeNull()
    expect(selfUpdateBlock(makeUpdate({ self_update: null }))).toMatchObject({
      reason: u.blocker.tooOld,
      commands: true,
    })
    expect(selfUpdateBlock(makeUpdate({ self_update: { able: false, blocker: null } }))).toMatchObject({
      reason: u.blocker.unknown,
    })
    // 更新的 controller 报出这里不认识的原因。
    const future = JSON.parse('{"able":false,"blocker":"immutable_os"}') as { able: false; blocker: null }
    expect(selfUpdateBlock(makeUpdate({ self_update: future }))).toMatchObject({ reason: u.blocker.unknown })
  })

  it('原因和做法跟着语言走', () => {
    setLocale('en', { remember: false })
    expect(selfUpdateBlock(makeUpdate({ self_update: { able: false, blocker: 'container' } }))).toMatchObject({
      reason: en.updates.blocker.container,
      howTo: en.updates.blocker.containerHow,
    })
  })
})

describe('forceAbility：能不能点"立即更新"', () => {
  it('有新版本、能自己更新、在线才能点', () => {
    expect(forceAbility(makeNode(1, 'nas', { update: makeUpdate({ available: '0.3.0' }) }))).toEqual({
      ok: true,
      version: '0.3.0',
    })
  })

  it('不能点时给原因：没有新版本、不能自己更新、不在线、正在换程序、已经让它马上更新了', () => {
    const cases = [
      [makeNode(1, 'a'), u.force.cannot.none],
      [
        makeNode(1, 'a', {
          update: makeUpdate({ available: '0.3.0', self_update: { able: false, blocker: 'container' } }),
        }),
        u.blocker.container,
      ],
      [makeNode(1, 'a', { online: false, update: makeUpdate({ available: '0.3.0' }) }), t.common.offline],
      [
        makeNode(1, 'a', {
          update: makeUpdate({ available: '0.3.0', status: { state: 'restarting', version: '0.3.0' } }),
        }),
        u.force.cannot.busy,
      ],
      [
        makeNode(1, 'a', { update: makeUpdate({ available: '0.3.0', ordered: { version: '0.3.0', timing: 'now' } }) }),
        u.force.cannot.alreadyNow,
      ],
    ] as const
    for (const [node, short] of cases) {
      const ability = forceAbility(node)
      expect(ability.ok).toBe(false)
      if (!ability.ok) expect(ability.short).toBe(short)
    }
  })

  it('自动更新正在等空闲时可以强制：管理员要它现在就换', () => {
    const waiting = makeNode(1, 'a', {
      update: makeUpdate({
        available: '0.3.0',
        ordered: { version: '0.3.0', timing: 'idle' },
        status: { state: 'waiting_for_idle', version: '0.3.0' },
      }),
    })
    expect(forceAbility(waiting).ok).toBe(true)
    expect(isUpdating(waiting.update)).toBe(true)
    expect(isUpdating(makeUpdate())).toBe(false)
  })
})

describe('节点列表上的记号和"需要处理"', () => {
  it('版本一列的图标跟着状态走，最新的不标', () => {
    expect(updateMark('available')).toBe('available')
    expect(updateMark('in_progress')).toBe('updating')
    expect(updateMark('waiting')).toBe('updating')
    expect(updateMark('failed')).toBe('failed')
    expect(updateMark('manual')).toBe('manual')
    expect(updateMark('behind')).toBe('behind')
    expect(updateMark('up_to_date')).toBeNull()
  })

  it('更新失败排在最前，红色；不能自己更新而有新版本排在最后', () => {
    const node = makeNode(1, 'hk', {
      relay: { udp: ['203.0.113.7:20330'], tcp: [], region: 'hk', bandwidth_mbps: null },
      online: false,
      update: makeUpdate({
        available: '0.3.0',
        self_update: { able: false, blocker: 'read_only_dir' },
        failure: { version: '0.3.0', kind: 'version_rejected', reason: '自检没有通过', at: 1 },
      }),
    })
    const issues = nodeIssues(node, [node])
    expect(issues.map((issue) => issue.kind)).toEqual(['update_failed', 'relay_offline', 'manual_update'])
    expect(issues[0]).toMatchObject({ tone: 'danger', message: u.issue.failed })
  })

  it('没装成（和版本无关）的失败不上"需要处理"：它会自己再试', () => {
    const node = makeNode(1, 'a', {
      update: makeUpdate({ status: { state: 'failed', version: null, kind: 'not_installed', reason: '断网' } }),
    })
    expect(nodeIssues(node, [node])).toEqual([])
  })
})

describe('发布源', () => {
  it('从设置里认出是哪一种', () => {
    expect(sourceKind({ source: 'https://github.com/x/releases', source_is_default: true })).toBe('default')
    expect(sourceKind({ source: 'http://mirror.lan/releases', source_is_default: false })).toBe('url')
    expect(sourceKind({ source: '/srv/releases', source_is_default: false })).toBe('dir')
  })

  it('镜像要是 http(s) 地址，目录要是绝对路径', () => {
    expect(validateSource('default', '')).toBeNull()
    expect(validateSource('url', '')).toBe(u.source.required)
    expect(validateSource('url', 'mirror.example.com')).toBe(u.source.urlInvalid)
    expect(validateSource('url', 'https://mirror.example.com/ice3tier/releases')).toBeNull()
    expect(validateSource('dir', 'releases')).toBe(u.source.dirInvalid)
    expect(validateSource('dir', '/srv/ice3tier/releases')).toBeNull()
    expect(validateSource('dir', 'D:\\releases')).toBeNull()
  })
})

describe('被暂停的版本按版本归在一起', () => {
  const laptop = { id: 2, name: 'laptop', virtual_ip: '10.66.0.2' }
  const entry = (version: string, network: number, at: number) => ({ version, node: laptop, network, reason: 'x', at })

  it('同一个版本在几个网络里被暂停：一组，按时间先后', () => {
    const groups = groupPaused([entry('0.3.0', 2, 200), entry('0.2.9', 1, 50), entry('0.3.0', 1, 100)])
    expect(groups.map((group) => group.version)).toEqual(['0.2.9', '0.3.0'])
    expect(groups[1]?.entries.map((item) => item.network)).toEqual([1, 2])
  })

  it('没有暂停：没有组', () => {
    expect(groupPaused([])).toEqual([])
  })
})
