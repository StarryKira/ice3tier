/** 版本和节点更新：设置里的「节点更新」、节点的更新状态、立即更新的确认框。 */

const timingLabel = { idle: '空闲时', now: '马上', after_drain: '排空后' } as const

export const version = {
  /** 侧栏底部。 */
  footerController: (version: string) => `controller ${version}`,
  footerWeb: (version: string) => `界面 ${version}`,
  mismatch: (web: string, controller: string) =>
    `界面是 ${web}，controller 是 ${controller}：两者不是同一次构建。界面可能显示不了 controller 新加的东西，或者用到它还没有的接口。`,
  mismatchShort: '和 controller 不是同一次构建',
  newer: (version: string) => `有新版本 ${version}`,
  newerHint: (version: string) =>
    `已知的最新正式版 ${version} 比 controller 新。controller 不会自己更新，要手动下载替换；节点由 controller 下发更新。`,
  newerLink: '去看节点更新',
  /** 关于页。 */
  aboutController: 'controller 版本',
  aboutWeb: '界面版本',
  aboutWebHint: '这个页面自己的版本，构建时写进来的。',
  aboutSame: '和 controller 是同一次构建。',
  metrics: '指标',
  metricsHint: 'Prometheus 的文本格式，不需要管理令牌，只有合计的数，没有节点的名字和地址。',
}

export const updates = {
  tab: '节点更新',
  loadFailed: '更新的情况没有取到',
  pausedDot: '有被暂停的版本',

  paused: {
    title: (version: string, node: string) => `${version} 在 ${node} 上更新失败，已暂停这个版本的自动更新。`,
    titleNodeGone: (version: string) => `${version} 在一个后来删掉的节点上更新失败，已暂停这个版本的自动更新。`,
    reason: '查看原因',
    hideReason: '收起原因',
    reported: '节点报的原因',
    resume: '继续更新其余节点',
    resumeTitle: (version: string) => `继续 ${version} 的自动更新？`,
    resumeBody: (version: string, node: string | null) =>
      node === null
        ? `其余有新版本的节点会在空闲时更新到 ${version}。`
        : `其余有新版本的节点会在空闲时更新到 ${version}。${node} 不会再被自动更新到这个版本，要对它点"立即更新"才会再试。`,
    resumeConfirm: '继续更新',
    titleMany: (version: string, networks: number) =>
      `${version} 在 ${networks} 个网络里都有节点更新失败，这些网络暂停了这个版本的自动更新。`,
    scope: (networks: string) => `暂停只在网络 ${networks} 里生效，别的网络照常更新。`,
    entry: (network: string, node: string) => `网络 ${network}：${node} 上失败`,
    entryNodeGone: (network: string) => `网络 ${network}：一个后来删掉的节点上失败`,
    networkGone: (id: number) => `#${id}（已删除）`,
    resumeBodyMany: (version: string, networks: string) =>
      `这个版本在网络 ${networks} 里被暂停，会一起继续：这些网络里其余有新版本的节点会在空闲时更新到 ${version}。失败过的节点不会再被自动更新到它，要对它们点"立即更新"才会再试。`,
    resumed: (version: string) => `已继续 ${version} 的自动更新`,
  },

  release: {
    title: '版本',
    latest: (version: string) => `最新版本 ${version}`,
    none: '还没有查到发布',
    published: (at: string) => `发布于 ${at}`,
    /** 跟在"3 分钟前"后面。 */
    checked: '检查',
    neverChecked: '还没有检查过',
    check: '立即检查',
    checking: '正在查发布源…',
    upToDate: '查过了：已是最新，没有节点可以更新。',
    found: (version: string, n: number) => `查过了：最新版本是 ${version}，${n} 个节点可以更新。`,
    nothingPublished: '查过了：发布源上还没有正式版。',
    checkFailed: (reason: string) => `查不到发布：${reason}`,
    checkFailedHint: '可以在下面换一个发布源。失败时保留上一次查到的版本。',
    controllerNewer: (controller: string, version: string) =>
      `controller 自己是 ${controller}，有新版本 ${version}。controller 不会自己更新，请手动下载替换它。`,
    controllerTooOld: (min: string, controller: string) =>
      `这个版本的节点需要 controller ${min} 或更新，现在是 ${controller}。先手动更新 controller，在那之前这个版本不会下发给任何节点。`,
    packages: '各平台的包',
    colPlatform: '平台',
    colFile: '文件',
    colSize: '大小',
    colState: '在 controller 上',
    packageState: {
      not_needed: '用不到',
      fetching: '正在取',
      cached: '已取回',
      failed: '没取成',
      unknown: '不认识的状态',
    } as Record<string, string>,
    packageStateHint: {
      not_needed: '没有要更新的节点用这个平台，所以没有取。',
      fetching: '有节点要用，正在从发布源取，或者排着队。',
      cached: '已经取回来，核对过大小和 SHA-256。',
      failed: '没有取成，下一次查发布源时再试。',
      unknown: '',
    } as Record<string, string>,
  },

  auto: {
    title: '自动更新',
    label: '自动更新节点',
    on: '节点上没有活跃的流时才更新，不会打断正在进行的连接。relay 一个一个来。',
    off: '节点不会自己更新。有新版本时在节点列表上标出，由你来点"立即更新"。',
    turnedOn: '自动更新已打开',
    turnedOff: '自动更新已关闭',
  },

  source: {
    title: '发布源',
    summaryDefault: '项目的 GitHub Release（默认）',
    summaryUrl: (url: string) => `镜像 ${url}`,
    summaryDir: (dir: string) => `本机目录 ${dir}`,
    lead: '发布包和清单都有签名，节点只认签名，所以镜像和目录都不需要可信。',
    kindLabel: '发布源',
    kindDefault: '默认',
    kindDefaultHint: '项目在 GitHub 上的 Release。controller 每小时查一次。',
    kindUrl: '镜像',
    kindUrlHint: '连不上 GitHub 时用。一个 http:// 或 https:// 的地址前缀，目录结构和 Release 一样。',
    kindDir: '本机目录',
    kindDirHint: 'controller 所在机器上的一个目录，目录结构和 Release 一样。',
    urlLabel: '镜像地址',
    urlPlaceholder: 'https://mirror.example.com/ice3tier/releases',
    dirLabel: '目录的绝对路径',
    dirPlaceholder: '/srv/ice3tier/releases',
    urlInvalid: '要以 http:// 或 https:// 开头。',
    dirInvalid: '要写绝对路径，例如 /srv/ice3tier/releases。',
    required: '这一项要填。',
    save: '保存发布源',
    saved: '发布源已保存，controller 正在后台查一次',
    layout:
      '目录结构：latest/download/ 下放 manifest.json 和 manifest.json.sig，download/v<版本号>/ 下放清单里列出的文件。',
  },

  nodes: {
    title: '节点的更新状态',
    summary: (counts: {
      total: number
      up_to_date: number
      updating: number
      outdated: number
      failed: number
      manual: number
    }) => {
      const parts = [
        counts.up_to_date > 0 ? `${counts.up_to_date} 个已是最新` : null,
        counts.updating > 0 ? `${counts.updating} 个正在更新` : null,
        counts.outdated > 0 ? `${counts.outdated} 个有新版本` : null,
        counts.failed > 0 ? `${counts.failed} 个上次更新失败` : null,
        counts.manual > 0 ? `${counts.manual} 个要手动更新` : null,
      ].filter((part): part is string => part !== null)
      return `${counts.total} 个节点${parts.length > 0 ? `：${parts.join('，')}` : ''}。`
    },
    filter: '按更新情况筛选',
    filterAll: '全部',
    filterBehind: (n: number) => `没跟上 (${n})`,
    colNode: '节点',
    colVersion: '版本',
    colState: '更新状态',
    empty: '还没有节点。',
    noneBehind: '所有节点都跟上了。',
    updateAll: '全部更新…',
    updateAllDisabled: '没有能立即更新的节点：要有新版本、在线、能自己更新。',
  },

  state: {
    neverConnected: '还没有连上过',
    upToDate: '已是最新',
    behind: (version: string) => `落后于 ${version}`,
    behindNoPackage: (version: string, platform: string) => `发布 ${version} 里没有 ${platform} 的包，只能手动更新。`,
    behindControllerTooOld: (version: string, min: string) =>
      `${version} 需要 controller ${min} 或更新：先手动更新 controller。`,
    behindUnknown: (version: string) => `已知的发布是 ${version}，controller 没有给出可以更新到的版本。`,
    available: (version: string) => `有新版本 ${version}`,
    availableAuto: '自动更新开着：节点空闲时自己更新。',
    availableManual: '自动更新关着：点"立即更新"。',
    availableOffline: '节点不在线，上线之后再更新。',
    availablePaused: (version: string) =>
      `${version} 的自动更新已暂停，见设置里的「节点更新」。要更新这一个，点"立即更新"。`,
    cannotSelf: '不能自己更新',
    downloading: (percent: number) => `下载中 ${percent}%`,
    downloadingDetail: (version: string, received: string, total: string) =>
      `正在从 controller 取 ${version} 的包：${received} / ${total}`,
    verifying: '核对中',
    verifyingDetail: (version: string) => `${version} 的包取完了，正在核对大小和 SHA-256。`,
    selfCheck: '自检中',
    selfCheckDetail: (version: string) => `正在运行 ${version} 的自检。`,
    waitingForIdle: '等待空闲',
    waitingForIdleDetail: (version: string) => `${version} 已经准备好，等节点上没有活跃的流再换上。`,
    draining: '排空中',
    drainingDetail: (version: string) => `${version} 已经准备好。节点不再接新的流，手上的流结束之后换上。`,
    installing: '替换中',
    installingDetail: (version: string) => `正在把程序换成 ${version}。`,
    restarting: '重启中',
    restartingDetail: (version: string) => `程序已经换成 ${version}，节点马上重启。`,
    confirmed: (version: string) => `已更新到 ${version}`,
    failed: '更新失败',
    failedRejected: (version: string, reason: string) =>
      `${version} 在这台机器上不能用：${reason}。自动更新不会再让它装这个版本；要再试，点"立即更新"。`,
    failedNotInstalled: (reason: string) => `没有装成：${reason}。这和版本好坏无关，下一次查发布源时会再试。`,
    unknownState: '进度不明',
    unknownStateDetail: '节点报了一个这个界面不认识的进度，它可能比 controller 新。',
    ordered: (version: string, timing: keyof typeof timingLabel) => `已下发：${timingLabel[timing]}更新到 ${version}`,
    orderedLabel: '已下发',
    waitingPackage: '等包',
    waitingPackageDetail: (platform: string) => `controller 正在从发布源取 ${platform} 的包，取回来就下发，不用再点。`,
    waitingRelayTurn: '排队中',
    waitingRelayTurnDetail: '它是 relay：同一个网络里的 relay 一个一个更新，在等前一个更新完、重新上线。',
    waitingUnknown: '等待中',
    lastFailed: '上次失败',
    lastFailedDetail: (version: string, reason: string, at: string) =>
      `${at}装 ${version} 失败：${reason}。自动更新不会再让它装这个版本；要再试，点"立即更新"。`,
  },

  blocker: {
    container: '在容器里运行',
    containerHow: '要更新就换成新版本的镜像，再重建容器。容器里换掉的程序文件，容器重建之后又会回到镜像里的版本。',
    package_manager: '是系统的包管理器装的',
    package_managerHow: '用装它的那个包管理器更新。自己换文件会和包管理器记的对不上。',
    read_only_dir: '程序所在的目录不可写',
    read_only_dirHow:
      '换一个服务账号可写的安装位置，以后就能自己更新；或者手动替换程序文件。下面是在这台机器上手动替换的命令，换好之后重启 i3t。',
    unknown: '节点说它不能自己更新',
    unknownHow: '原因这个版本的 controller 不认识，或者节点上更新的记录读不了。手动替换程序文件，换好之后重启 i3t。',
    tooOld: '版本太旧，不懂自己更新',
    tooOldHow: '只能手动替换程序文件，换好之后重启 i3t。更新到新版本之后，以后就能自己更新。',
    able: '能自己更新',
    ableHint: '节点报告它能替换自己的程序文件。',
  },

  force: {
    button: '立即更新…',
    menu: '立即更新…',
    cannot: {
      none: '没有可以更新到的版本。',
      offline: '节点不在线。让节点立即更新要它连着 controller。',
      busy: '正在换程序，马上就好。',
      alreadyNow: '已经让它立即更新了。',
    },
    title: (name: string, version: string) => `立即更新 ${name} 到 ${version}？`,
    timingLabel: '什么时候换上新程序',
    now: '马上更新',
    nowFlows: (flows: number) => `现在有 ${flows} 条流，会断开。`,
    afterDrain: '等手上的流结束再更新',
    afterDrainHint: '期间这个节点不接受新的流，发给它的新请求被拒绝，调用方要换别的出口。',
    idleBody: '它现在没有活跃的流。更新会重启它上面的 i3t，几秒钟之后以新版本上线。',
    relayOthers: (relays: number) => `它是 relay。重启之前会通知经它中转的节点换到另外 ${relays} 个 relay 上。`,
    relayOnly: (pairs: number) =>
      pairs > 0
        ? `它是这个网络里唯一的 relay，重启会断开经它中转的 ${pairs} 对节点之间的连接。`
        : '它是这个网络里唯一的 relay，重启期间打不通直连的节点之间不可达。',
    controllerNote: '强制更新不看自动更新的开关、暂停和以前的失败。controller 重启的话，还没做完的要重新点。',
    loadingDetail: '正在取它现在的流数…',
    confirmNow: '立即更新',
    confirmDrain: '排空后更新',
    ordered: (name: string, version: string) => `已让 ${name} 更新到 ${version}`,
    orderedWaitingPackage: (name: string) => `${name} 的包还没有取回来；取回来就下发，不用再点`,
  },

  forceAll: {
    title: (version: string) => `全部更新到 ${version}？`,
    body: (n: number) =>
      `有新版本、能自己更新的节点有 ${n} 个，分布在全部网络里。不在线的这次会跳过。relay 一个一个更新。`,
    flowsUnknown: '这里算不出各个节点现在有多少条流：选"马上更新"时，正在进行的流都会断开。',
    confirmNow: '全部马上更新',
    confirmDrain: '全部排空后更新',
    result: (ordered: number, queued: number, skipped: number) =>
      `已下发 ${ordered} 个，稍后下发 ${queued} 个，跳过 ${skipped} 个`,
    resultTitle: '全部更新的结果',
    resultOrdered: '已下发',
    resultQueued: '稍后下发（等包取回来，或者 relay 在排队）',
    resultSkipped: '跳过',
    resultNoVersion: '没有可以下发的版本，节点都跳过了。',
    dismiss: '关掉这个结果',
  },

  detail: {
    tab: '更新',
    current: '现在的版本',
    available: '可以更新到',
    availableNone: '没有',
    selfUpdate: '能不能自己更新',
    state: '更新状态',
    order: '下发的指令',
    orderNone: '没有',
    orderValue: (version: string, timing: keyof typeof timingLabel) => `${timingLabel[timing]}更新到 ${version}`,
    failure: '最近一次失败',
    failureNone: '没有',
    manualDownload: (platform: string) => `在这台机器上执行（${platform}）：`,
    manualNoPlatform: '这个平台没有现成的下载命令，从发布页下载对应的程序文件，替换掉现在的那一个。',
    history: '更新的历史记录 controller 现在还不保存，这里只有最近一次。',
    goSettings: '自动更新和发布源在设置里',
  },

  /** 节点列表版本一列的图标。 */
  icon: {
    available: (version: string) => `有新版本 ${version}`,
  },

  issue: {
    failed: (reason: string) => `更新失败：${reason}。在节点详情的「更新」里看原因。`,
    manual: (version: string, why: string) => `有新版本 ${version}，但${why}，要手动更新。做法在节点详情的「更新」里。`,
  },
}
