/** 版本和节点更新：设置里的「节点更新」、节点的更新状态、立即更新的确认框。 */

/** 换上新程序的时机，接在"更新到"前面。 */
const timing = { idle: '空闲时', now: '立即', after_drain: '排空后' }

export const version = {
  /** 侧栏底部。 */
  footerController: (version: string) => `controller ${version}`,
  footerWeb: (version: string) => `界面 ${version}`,
  mismatch: (web: string, controller: string) =>
    `版本不一致：界面 ${web}，controller ${controller}。部分功能可能不可用。`,
  mismatchShort: '版本不一致',
  newer: (version: string) => `有新版本 ${version}`,
  newerHint: (version: string) => `controller 需手动更新到 ${version}，节点由 controller 更新`,
  newerLink: '查看节点更新',
  /** 关于页。 */
  aboutController: 'controller 版本',
  aboutWeb: '界面版本',
  metrics: '指标',
  metricsHint: 'Prometheus 格式，无需管理令牌，只含汇总数据',
}

export const updates = {
  tab: '节点更新',
  loadFailed: '无法加载更新状态',
  pausedDot: '有暂停的版本',

  paused: {
    title: (version: string, node: string) => `${version} 已暂停：在 ${node} 上更新失败`,
    titleNodeGone: (version: string) => `${version} 已暂停：在一个已删除的节点上更新失败`,
    titleMany: (version: string, networks: number) => `${version} 已在 ${networks} 个网络中暂停`,
    scope: (networks: string) => `仅在 ${networks} 中暂停，其他网络照常更新`,
    reason: '查看原因',
    hideReason: '收起原因',
    entry: (network: string, node: string) => `${node}（${network}）`,
    entryNodeGone: (network: string) => `已删除的节点（${network}）`,
    networkGone: (id: number) => `#${id}（已删除）`,
    resume: '继续更新',
    resumeTitle: (version: string) => `继续 ${version} 的自动更新？`,
    resumeBody: (version: string, node: string | null) =>
      node === null
        ? `其余节点将在空闲时更新到 ${version}。`
        : `其余节点将在空闲时更新到 ${version}。${node} 不会自动重试。`,
    resumeBodyMany: (version: string, networks: string) =>
      `${networks} 中的其余节点将在空闲时更新到 ${version}。失败过的节点不会自动重试。`,
    resumeConfirm: '继续更新',
    resumed: (version: string) => `${version} 已恢复自动更新`,
  },

  release: {
    title: '版本',
    latest: (version: string) => `最新版本 ${version}`,
    none: '尚无发布',
    published: (age: string) => `发布于 ${age}`,
    checkedAt: (age: string) => `${age}检查`,
    neverChecked: '尚未检查',
    check: '立即检查',
    checking: '检查中…',
    upToDate: '已是最新',
    found: (version: string, n: number) => `最新版本 ${version}，${n} 个节点可更新`,
    nothingPublished: '发布源上还没有正式版',
    checkFailed: '检查失败',
    checkFailedHint: '沿用上次查到的版本',
    controllerNewer: (controller: string, version: string) =>
      `controller ${controller} 可更新到 ${version}，需手动替换`,
    controllerTooOld: (min: string, controller: string) =>
      `此版本需要 controller ${min} 或更新（当前 ${controller}），在此之前不会下发给节点`,
    packages: '安装包',
    colPlatform: '平台',
    colFile: '文件',
    colSize: '大小',
    colState: '状态',
    packageState: {
      not_needed: '不需要',
      fetching: '下载中',
      cached: '已缓存',
      failed: '失败',
      unknown: '未知',
    },
    packageStateHint: {
      not_needed: '没有节点需要此平台',
      fetching: '正在从发布源下载',
      cached: '已下载并校验',
      failed: '下次检查时重试',
      unknown: '无法识别的状态',
    },
  },

  auto: {
    title: '自动更新',
    label: '自动更新节点',
    on: '节点空闲时更新，不中断连接；relay 逐个更新',
    off: '新版本只标出，不自动安装',
    turnedOn: '自动更新已开启',
    turnedOff: '自动更新已关闭',
  },

  source: {
    title: '发布源',
    summaryDefault: '默认（GitHub Release）',
    summaryUrl: (url: string) => `镜像 ${url}`,
    summaryDir: (dir: string) => `本机目录 ${dir}`,
    trust: '安装包和清单都经签名校验，镜像和目录无需可信',
    kindLabel: '发布源',
    kindDefault: '默认',
    kindDefaultHint: 'GitHub Release，每小时检查',
    kindUrl: '镜像',
    kindUrlHint: '目录结构与 Release 相同的 HTTP(S) 地址',
    kindDir: '本机目录',
    kindDirHint: 'controller 所在机器上的目录',
    urlLabel: '镜像地址',
    urlPlaceholder: 'https://mirror.example.com/ice3tier/releases',
    dirLabel: '目录路径',
    dirPlaceholder: '/srv/ice3tier/releases',
    urlInvalid: '需以 http:// 或 https:// 开头',
    dirInvalid: '需为绝对路径',
    required: '必填',
    save: '保存',
    saved: '发布源已保存',
    layout: '需包含 latest/download/manifest.json(.sig) 和 download/v<版本>/',
  },

  nodes: {
    title: '节点',
    summary: (counts: {
      total: number
      up_to_date: number
      updating: number
      outdated: number
      failed: number
      manual: number
    }) =>
      [
        `${counts.total} 个节点`,
        counts.up_to_date > 0 ? `${counts.up_to_date} 个最新` : null,
        counts.updating > 0 ? `${counts.updating} 个更新中` : null,
        counts.outdated > 0 ? `${counts.outdated} 个有新版本` : null,
        counts.failed > 0 ? `${counts.failed} 个失败` : null,
        counts.manual > 0 ? `${counts.manual} 个需手动更新` : null,
      ]
        .filter((part): part is string => part !== null)
        .join(' · '),
    filter: '按更新状态筛选',
    filterAll: '全部',
    filterBehind: (n: number) => `待更新 (${n})`,
    colNode: '节点',
    colVersion: '版本',
    colState: '状态',
    empty: '还没有节点',
    noneBehind: '全部已是最新',
    updateAll: '全部更新…',
    updateAllDisabled: '没有可立即更新的节点',
  },

  state: {
    neverConnected: '从未连接',
    upToDate: '已是最新',
    behind: (version: string) => `落后于 ${version}`,
    behindNoPackage: (version: string, platform: string) => `${version} 没有 ${platform} 的包，需手动更新`,
    behindControllerTooOld: (version: string, min: string) => `${version} 需要 controller ${min} 或更新`,
    behindUnknown: (version: string) => `最新发布 ${version}，暂无可用更新`,
    available: (version: string) => `有新版本 ${version}`,
    availableAuto: '空闲时自动更新',
    availableManual: '自动更新已关闭',
    availableOffline: '节点离线，上线后更新',
    availablePaused: (version: string) => `${version} 的自动更新已暂停`,
    cannotSelf: '需手动更新',
    downloading: (percent: number) => `下载中 ${percent}%`,
    downloadingDetail: (version: string, received: string, total: string) => `${version}：${received} / ${total}`,
    verifying: '校验中',
    selfCheck: '自检中',
    installing: '安装中',
    stepDetail: (version: string) => `更新到 ${version}`,
    waitingForIdle: '等待空闲',
    waitingForIdleDetail: (version: string) => `${version} 已就绪，等待节点空闲`,
    draining: '排空中',
    drainingDetail: (version: string) => `${version} 已就绪，等待现有的流结束`,
    restarting: '重启中',
    restartingDetail: (version: string) => `已换成 ${version}，即将重启`,
    confirmed: (version: string) => `已更新到 ${version}`,
    failed: '更新失败',
    failedRejected: (version: string) => `${version} 无法在此节点运行，不会再自动安装`,
    failedNotInstalled: '安装未完成，下次检查时重试',
    unknownState: '状态未知',
    ordered: (version: string, when: keyof typeof timing) => `${timing[when]}更新到 ${version}`,
    orderedLabel: '已下发',
    waitingPackage: '等待安装包',
    waitingPackageDetail: (platform: string) => `controller 正在下载 ${platform} 的包`,
    waitingRelayTurn: '排队中',
    waitingRelayTurnDetail: 'relay 逐个更新',
    waitingUnknown: '等待中',
    lastFailed: '上次失败',
    lastFailedDetail: (version: string, age: string) => `${age}安装 ${version} 失败，不会自动重试`,
  },

  blocker: {
    container: '在容器中运行',
    containerHow: '更新镜像并重建容器',
    package_manager: '由包管理器安装',
    package_managerHow: '用该包管理器更新',
    read_only_dir: '程序目录不可写',
    read_only_dirHow: '在这台机器上手动替换程序，然后重启 i3t',
    unknown: '无法自行更新',
    unknownHow: '手动替换程序，然后重启 i3t',
    tooOld: '版本过旧，不支持自行更新',
    tooOldHow: '手动替换程序并重启 i3t，之后可自行更新',
    able: '可自行更新',
  },

  force: {
    button: '立即更新…',
    buttonFor: (name: string) => `立即更新 ${name}…`,
    menu: '立即更新…',
    cannot: {
      none: '没有可更新的版本',
      offline: '节点离线',
      busy: '正在更新',
      alreadyNow: '已下发立即更新',
    },
    title: (name: string, version: string) => `将 ${name} 更新到 ${version}？`,
    timingLabel: '更新时机',
    now: '立即',
    nowFlows: (flows: number) => `${flows} 条流将中断`,
    afterDrain: '流结束后',
    afterDrainHint: '期间拒绝新的流',
    idleBody: '没有活跃的流。i3t 将重启，几秒后上线。',
    relayOthers: (relays: number) => `经它中转的连接将切换到其余 ${relays} 个 relay`,
    relayOnly: (pairs: number) =>
      pairs > 0 ? `它是唯一的 relay，${pairs} 对节点的连接将中断` : '它是唯一的 relay，重启期间不能直连的节点互不可达',
    controllerNote: '不受自动更新设置、暂停和以往失败的限制',
    loadingDetail: '正在读取流数',
    confirmNow: '立即更新',
    confirmDrain: '排空后更新',
    ordered: (name: string, version: string) => `${name} 将更新到 ${version}`,
    orderedWaitingPackage: (name: string) => `${name} 将在安装包就绪后更新`,
  },

  forceAll: {
    title: (version: string) => `将全部节点更新到 ${version}？`,
    body: (n: number) => `${n} 个节点可更新。离线的节点跳过，relay 逐个更新。`,
    flowsUnknown: '选择"立即"将中断所有正在进行的流',
    confirmNow: '全部立即更新',
    confirmDrain: '全部排空后更新',
    result: (ordered: number, queued: number, skipped: number) =>
      `已下发 ${ordered} 个，排队 ${queued} 个，跳过 ${skipped} 个`,
    resultTitle: '全部更新的结果',
    resultOrdered: '已下发',
    resultQueued: '排队中',
    resultSkipped: '已跳过',
    resultNoVersion: '没有可下发的版本',
    dismiss: '关闭',
  },

  detail: {
    tab: '更新',
    current: '当前版本',
    available: '可更新到',
    selfUpdate: '自行更新',
    state: '状态',
    order: '已下发',
    orderValue: (version: string, when: keyof typeof timing) => `${timing[when]}更新到 ${version}`,
    failure: '上次失败',
    none: '无',
    manualDownload: (platform: string) => `在这台机器上运行（${platform}）：`,
    manualNoPlatform: '从发布页下载对应的程序，替换现有的程序',
    goSettings: '更新设置',
  },

  /** 节点列表版本一列的图标。 */
  icon: {
    available: (version: string) => `有新版本 ${version}`,
  },

  issue: {
    failed: '更新失败',
    manual: (version: string, why: string) => `${version} 需手动更新（${why}）`,
  },
}
