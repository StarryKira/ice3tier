import {
  type Node,
  type NodeUpdate,
  type PausedVersion,
  type ReleaseInfo,
  type SelfUpdateBlocker,
  type Version,
} from '@/api/types'
import { type Tone } from '@/components/status'
import { t } from '@/i18n'

import { formatBytes } from './format'
import { isOlder } from './version'

const u = t.updates

/**
 * 一个节点的更新情况在界面上怎么说。纯函数：只看节点自己的 `update` 字段和已知的发布，
 * 节点列表、节点详情和设置里的「节点更新」用的是同一套说法。
 *
 * 判断的顺序就是"用户最想先知道的"：正在进行的 > 已经下发、在等什么 > 失败 > 有新版本 > 落后 > 最新。
 */

export type UpdateKind =
  | 'never_connected'
  | 'up_to_date'
  | 'behind'
  | 'available'
  | 'manual'
  | 'failed'
  | 'in_progress'
  | 'waiting'
  | 'confirmed'
  | 'unknown'

export interface UpdateView {
  kind: UpdateKind
  tone: Tone
  /** 表格里的一个短语。 */
  label: string
  /** 一句话：现在怎样、为什么、下一步。没有要补充的是 `null`。 */
  detail: string | null
  /** 节点报的原话（失败的原因），作为补充显示；它是节点写的中文，界面不翻译。没有是 `null`。 */
  reason: string | null
  /** 下载的进度，0 到 1。不在下载时是 `null`。 */
  progress: number | null
}

/** 判断要用到的、节点之外的东西。 */
export interface UpdateContext {
  release: ReleaseInfo | null
  autoUpdate: boolean
  /** 被暂停自动更新的版本。 */
  paused: readonly Version[]
}

export const NO_UPDATE_CONTEXT: UpdateContext = { release: null, autoUpdate: true, paused: [] }

type UpdateSubject = Pick<Node, 'version' | 'platform' | 'online' | 'update'>

/** 不能自己更新的原因和手动更新的做法。能自己更新时是 `null`。`commands` 为真时界面上给出手动替换程序的命令。 */
export interface SelfUpdateBlock {
  reason: string
  howTo: string
  commands: boolean
}

/** 要不要给出手动替换程序的命令。文字每次现取（换了语言就是新的），所以这里只记这一点。 */
const NEEDS_COMMANDS = { container: false, package_manager: false, read_only_dir: true } satisfies Record<
  SelfUpdateBlocker,
  boolean
>

const isBlocker = (value: string): value is SelfUpdateBlocker => Object.hasOwn(NEEDS_COMMANDS, value)

export function selfUpdateBlock(update: NodeUpdate): SelfUpdateBlock | null {
  if (update.self_update === null) return { reason: u.blocker.tooOld, howTo: u.blocker.tooOldHow, commands: true }
  if (update.self_update.able) return null
  // 更新的 controller 可能报出这里还不认识的原因：按"不知道为什么不能"处理。
  const blocker: string | null = update.self_update.blocker
  if (blocker === null || !isBlocker(blocker))
    return { reason: u.blocker.unknown, howTo: u.blocker.unknownHow, commands: true }
  return { reason: u.blocker[blocker], howTo: u.blocker[`${blocker}How`], commands: NEEDS_COMMANDS[blocker] }
}

const view = (
  kind: UpdateKind,
  tone: Tone,
  label: string,
  detail: string | null = null,
  reason: string | null = null,
): UpdateView => ({
  kind,
  tone,
  label,
  detail,
  reason,
  progress: null,
})

function fromStatus(update: NodeUpdate): UpdateView | null {
  const { status } = update
  switch (status.state) {
    case 'idle':
      return null
    case 'downloading': {
      const progress = status.total > 0 ? Math.min(1, status.received / status.total) : 0
      return {
        ...view(
          'in_progress',
          'relay',
          u.state.downloading(Math.floor(progress * 100)),
          u.state.downloadingDetail(status.version, formatBytes(status.received), formatBytes(status.total)),
        ),
        progress,
      }
    }
    case 'verifying':
      return view('in_progress', 'relay', u.state.verifying, u.state.stepDetail(status.version))
    case 'self_check':
      return view('in_progress', 'relay', u.state.selfCheck, u.state.stepDetail(status.version))
    case 'installing':
      return view('in_progress', 'relay', u.state.installing, u.state.stepDetail(status.version))
    case 'restarting':
      return view('in_progress', 'relay', u.state.restarting, u.state.restartingDetail(status.version))
    case 'waiting_for_idle':
      return view('waiting', 'relay', u.state.waitingForIdle, u.state.waitingForIdleDetail(status.version))
    case 'draining':
      return view('waiting', 'relay', u.state.draining, u.state.drainingDetail(status.version))
    case 'confirmed':
      return view('confirmed', 'ok', u.state.confirmed(status.version))
    case 'failed':
      return view(
        'failed',
        'danger',
        u.state.failed,
        status.kind === 'version_rejected' ? u.state.failedRejected(status.version ?? '') : u.state.failedNotInstalled,
        status.reason,
      )
    default:
      return view('unknown', 'idle', u.state.unknownState)
  }
}

/** 写法上的选择，看在哪里显示。 */
export interface DescribeOptions {
  /** 自动更新的开关就在上面（设置里的「节点更新」）："自动更新已关闭"不用每行都说一遍。 */
  autoSwitchShown?: boolean
}

/** `formatTime` 把失败的时间写成相对时间；放在参数里，这个函数就不必知道"现在"。 */
export function describeUpdate(
  node: UpdateSubject,
  context: UpdateContext,
  formatTime: (at: number) => string,
  options: DescribeOptions = {},
): UpdateView {
  const { update } = node
  const progress = fromStatus(update)
  if (progress !== null) return progress

  if (update.ordered !== null)
    return view(
      'waiting',
      'relay',
      u.state.orderedLabel,
      u.state.ordered(update.ordered.version, update.ordered.timing),
    )
  if (update.waiting === 'package')
    return view('waiting', 'relay', u.state.waitingPackage, u.state.waitingPackageDetail(node.platform))
  if (update.waiting === 'relay_turn')
    return view('waiting', 'relay', u.state.waitingRelayTurn, u.state.waitingRelayTurnDetail)
  if (update.waiting !== null) return view('waiting', 'relay', u.state.waitingUnknown)

  const { available, failure } = update
  if (available !== null) {
    if (failure !== null && failure.version === available)
      return view(
        'failed',
        'danger',
        u.state.lastFailed,
        u.state.lastFailedDetail(failure.version, formatTime(failure.at)),
        failure.reason,
      )
    const block = selfUpdateBlock(update)
    if (block !== null) return view('manual', 'idle', u.state.cannotSelf, t.common.sentences(block.reason, block.howTo))
    const detail = !node.online
      ? u.state.availableOffline
      : context.paused.includes(available)
        ? u.state.availablePaused(available)
        : context.autoUpdate
          ? u.state.availableAuto
          : options.autoSwitchShown === true
            ? null
            : u.state.availableManual
    return view('available', 'warn', u.state.available(available), detail)
  }

  if (node.version === '') return view('never_connected', 'idle', u.state.neverConnected)
  const { release } = context
  if (release !== null && isOlder(node.version, release.version)) {
    const detail = release.controller_too_old
      ? u.state.behindControllerTooOld(release.version, release.min_controller)
      : release.packages.some((pkg) => pkg.platform === node.platform)
        ? u.state.behindUnknown(release.version)
        : u.state.behindNoPackage(release.version, node.platform)
    return view('behind', 'warn', u.state.behind(release.version), detail)
  }
  return view('up_to_date', 'idle', u.state.upToDate)
}

/**
 * 能不能对它点"立即更新"。规矩和 controller 的一样（文档 §14）：有可以更新到的版本、能自己更新、在线。
 * 不能时给一句原因（按钮置灰、悬停显示它）和一个短语（菜单项里放得下的）。
 */
export type ForceAbility = { ok: true; version: Version } | { ok: false; reason: string; short: string }

export function forceAbility(node: Pick<Node, 'online' | 'update'>): ForceAbility {
  const { update } = node
  if (update.available === null) return { ok: false, reason: u.force.cannot.none, short: u.force.cannot.none }
  const block = selfUpdateBlock(update)
  if (block !== null) return { ok: false, reason: t.common.sentences(block.reason, block.howTo), short: block.reason }
  if (!node.online) return { ok: false, reason: u.force.cannot.offline, short: t.common.offline }
  if (update.status.state === 'installing' || update.status.state === 'restarting')
    return { ok: false, reason: u.force.cannot.busy, short: u.force.cannot.busy }
  if (update.ordered?.timing === 'now')
    return { ok: false, reason: u.force.cannot.alreadyNow, short: u.force.cannot.alreadyNow }
  return { ok: true, version: update.available }
}

/** 节点列表版本一列上的小图标。没有值得标的事时是 `null`。 */
export type UpdateMark = 'available' | 'updating' | 'failed' | 'manual' | 'behind'

export function updateMark(kind: UpdateKind): UpdateMark | null {
  switch (kind) {
    case 'available':
      return 'available'
    case 'in_progress':
    case 'waiting':
      return 'updating'
    case 'failed':
      return 'failed'
    case 'manual':
      return 'manual'
    case 'behind':
      return 'behind'
    default:
      return null
  }
}

/** 正在进行的更新：页面据此把轮询调勤。 */
export function isUpdating(update: NodeUpdate): boolean {
  return (
    update.ordered !== null ||
    update.waiting !== null ||
    !['idle', 'confirmed', 'failed', 'unknown'].includes(update.status.state)
  )
}

export interface PausedGroup {
  version: Version
  /** 每个暂停了这个版本的网络一条，按时间先后。 */
  entries: PausedVersion[]
}

/**
 * 被暂停的版本按版本归在一起：暂停按网络算，同一个版本在几个网络里被暂停时 `paused` 里有几条，
 * 而"继续"一次继续全部网络里的这个版本，所以界面上一个版本一条横幅。版本按第一次暂停的时间排。
 */
export function groupPaused(paused: readonly PausedVersion[]): PausedGroup[] {
  const groups = new Map<Version, PausedVersion[]>()
  for (const entry of [...paused].sort((a, b) => a.at - b.at)) {
    groups.set(entry.version, [...(groups.get(entry.version) ?? []), entry])
  }
  return [...groups].map(([version, entries]) => ({ version, entries }))
}
