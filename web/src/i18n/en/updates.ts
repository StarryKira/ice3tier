import type { Messages } from '../messages'

/** When the new binary goes in; follows "update to …". */
const timing = { idle: 'when idle', now: 'now', after_drain: 'after draining' }

const count = (n: number, one: string, many: string) => (n === 1 ? `1 ${one}` : `${n} ${many}`)

export const version: Messages['version'] = {
  footerController: (version) => `controller ${version}`,
  footerWeb: (version) => `Web UI ${version}`,
  mismatch: (web, controller) =>
    `Version mismatch: Web UI ${web}, controller ${controller}. Some features may not work.`,
  mismatchShort: 'Version mismatch',
  newer: (version) => `${version} available`,
  newerHint: (version) => `Update the controller to ${version} manually; nodes are updated by the controller`,
  newerLink: 'View node updates',
  aboutController: 'Controller version',
  aboutWeb: 'Web UI version',
  metrics: 'Metrics',
  metricsHint: 'Prometheus format. No admin token needed; totals only',
}

export const updates: Messages['updates'] = {
  tab: 'Node updates',
  loadFailed: "Couldn't load update status",
  pausedDot: 'Paused version',

  paused: {
    title: (version, node) => `${version} paused: update failed on ${node}`,
    titleNodeGone: (version) => `${version} paused: update failed on a deleted node`,
    titleMany: (version, networks) => `${version} paused in ${count(networks, 'network', 'networks')}`,
    scope: (networks) => `Paused only in ${networks}; other networks keep updating`,
    reason: 'Show reason',
    hideReason: 'Hide reason',
    entry: (network, node) => `${node} (${network})`,
    entryNodeGone: (network) => `Deleted node (${network})`,
    networkGone: (id) => `#${id} (deleted)`,
    resume: 'Resume updates',
    resumeTitle: (version) => `Resume automatic updates to ${version}?`,
    resumeBody: (version, node) =>
      node === null
        ? `Other nodes update to ${version} when idle.`
        : `Other nodes update to ${version} when idle. ${node} is not retried automatically.`,
    resumeBodyMany: (version, networks) =>
      `Other nodes in ${networks} update to ${version} when idle. Nodes that failed are not retried automatically.`,
    resumeConfirm: 'Resume updates',
    resumed: (version) => `Updates to ${version} resumed`,
  },

  release: {
    title: 'Release',
    latest: (version) => `Latest ${version}`,
    none: 'No release yet',
    published: (age) => `Published ${age}`,
    checkedAt: (age) => `Checked ${age}`,
    neverChecked: 'Never checked',
    check: 'Check now',
    checking: 'Checking…',
    upToDate: 'Up to date',
    found: (version, n) => `Latest is ${version}; ${count(n, 'node', 'nodes')} can update`,
    nothingPublished: 'No stable release published yet',
    checkFailed: 'Check failed',
    checkFailedHint: 'Showing the last known release',
    controllerNewer: (controller, version) => `Controller ${controller} can be updated to ${version} manually`,
    controllerTooOld: (min, controller) =>
      `This release needs controller ${min} or later (running ${controller}); nodes won't get it until then`,
    packages: 'Packages',
    colPlatform: 'Platform',
    colFile: 'File',
    colSize: 'Size',
    colState: 'Status',
    packageState: {
      not_needed: 'Not needed',
      fetching: 'Downloading',
      cached: 'Cached',
      failed: 'Failed',
      unknown: 'Unknown',
    },
    packageStateHint: {
      not_needed: 'No node needs this platform',
      fetching: 'Downloading from the release source',
      cached: 'Downloaded and verified',
      failed: 'Retried at the next check',
      unknown: 'Unrecognized state',
    },
  },

  auto: {
    title: 'Automatic updates',
    label: 'Update nodes automatically',
    on: 'Nodes update when idle, without dropping connections; relays one at a time',
    off: 'New versions are flagged, not installed',
    turnedOn: 'Automatic updates on',
    turnedOff: 'Automatic updates off',
  },

  source: {
    title: 'Release source',
    summaryDefault: 'Default (GitHub Releases)',
    summaryUrl: (url) => `Mirror ${url}`,
    summaryDir: (dir) => `Local directory ${dir}`,
    trust: 'Packages and manifests are signature-checked, so mirrors and directories need not be trusted',
    kindLabel: 'Release source',
    kindDefault: 'Default',
    kindDefaultHint: 'GitHub Releases, checked hourly',
    kindUrl: 'Mirror',
    kindUrlHint: 'HTTP(S) URL with the same layout as Releases',
    kindDir: 'Local directory',
    kindDirHint: 'A directory on the controller host',
    urlLabel: 'Mirror URL',
    urlPlaceholder: 'https://mirror.example.com/ice3tier/releases',
    dirLabel: 'Directory path',
    dirPlaceholder: '/srv/ice3tier/releases',
    urlInvalid: 'Must start with http:// or https://',
    dirInvalid: 'Must be an absolute path',
    required: 'Required',
    save: 'Save',
    saved: 'Release source saved',
    layout: 'Must contain latest/download/manifest.json(.sig) and download/v<version>/',
  },

  nodes: {
    title: 'Nodes',
    summary: (counts) =>
      [
        count(counts.total, 'node', 'nodes'),
        counts.up_to_date > 0 ? `${counts.up_to_date} up to date` : null,
        counts.updating > 0 ? `${counts.updating} updating` : null,
        counts.outdated > 0 ? `${counts.outdated} with update` : null,
        counts.failed > 0 ? `${counts.failed} failed` : null,
        counts.manual > 0 ? `${counts.manual} manual` : null,
      ]
        .filter((part): part is string => part !== null)
        .join(' · '),
    filter: 'Filter by update status',
    filterAll: 'All',
    filterBehind: (n) => `Outdated (${n})`,
    colNode: 'Node',
    colVersion: 'Version',
    colState: 'Status',
    empty: 'No nodes yet',
    noneBehind: 'All nodes up to date',
    updateAll: 'Update all…',
    updateAllDisabled: 'No nodes can be updated now',
  },

  state: {
    neverConnected: 'Never connected',
    upToDate: 'Up to date',
    behind: (version) => `Behind ${version}`,
    behindNoPackage: (version, platform) => `${version} has no ${platform} package; update manually`,
    behindControllerTooOld: (version, min) => `${version} needs controller ${min} or later`,
    behindUnknown: (version) => `Latest release ${version}; no update offered`,
    available: (version) => `${version} available`,
    availableAuto: 'Updates automatically when idle',
    availableManual: 'Automatic updates off',
    availableOffline: 'Offline; updates when back online',
    availablePaused: (version) => `Automatic updates to ${version} paused`,
    cannotSelf: 'Manual update',
    downloading: (percent) => `Downloading ${percent}%`,
    downloadingDetail: (version, received, total) => `${version}: ${received} / ${total}`,
    verifying: 'Verifying',
    selfCheck: 'Self-checking',
    installing: 'Installing',
    stepDetail: (version) => `Updating to ${version}`,
    waitingForIdle: 'Waiting for idle',
    waitingForIdleDetail: (version) => `${version} ready; waiting for the node to be idle`,
    draining: 'Draining',
    drainingDetail: (version) => `${version} ready; waiting for active flows to end`,
    restarting: 'Restarting',
    restartingDetail: (version) => `Now on ${version}; restarting`,
    confirmed: (version) => `Updated to ${version}`,
    failed: 'Update failed',
    failedRejected: (version) => `${version} can't run on this node; it won't be installed automatically`,
    failedNotInstalled: "Install didn't finish; retried at the next check",
    unknownState: 'Unknown state',
    ordered: (version, when) => `Update to ${version} ${timing[when]}`,
    orderedLabel: 'Ordered',
    waitingPackage: 'Waiting for package',
    waitingPackageDetail: (platform) => `Controller is downloading the ${platform} package`,
    waitingRelayTurn: 'Queued',
    waitingRelayTurnDetail: 'Relays update one at a time',
    waitingUnknown: 'Waiting',
    lastFailed: 'Last update failed',
    lastFailedDetail: (version, age) => `Installing ${version} failed ${age}; not retried automatically`,
  },

  blocker: {
    container: 'Running in a container',
    containerHow: 'Update the image and recreate the container',
    package_manager: 'Installed by a package manager',
    package_managerHow: 'Update it with that package manager',
    read_only_dir: 'Program directory is read-only',
    read_only_dirHow: 'Replace the binary on the machine, then restart i3t',
    unknown: "Can't update itself",
    unknownHow: 'Replace the binary, then restart i3t',
    tooOld: 'Too old to update itself',
    tooOldHow: 'Replace the binary and restart i3t; later updates are automatic',
    able: 'Can update itself',
  },

  force: {
    button: 'Update now…',
    buttonFor: (name) => `Update ${name} now…`,
    menu: 'Update now…',
    cannot: {
      none: 'No update available',
      offline: 'Node is offline',
      busy: 'Update in progress',
      alreadyNow: 'Already ordered',
    },
    title: (name, version) => `Update ${name} to ${version}?`,
    timingLabel: 'When to update',
    now: 'Now',
    nowFlows: (flows) => `${count(flows, 'active flow', 'active flows')} will drop`,
    afterDrain: 'After flows end',
    afterDrainHint: 'New flows are refused meanwhile',
    idleBody: 'No active flows. i3t restarts and is back in a few seconds.',
    relayOthers: (relays) => `Relayed connections move to the other ${count(relays, 'relay', 'relays')}`,
    relayOnly: (pairs) =>
      pairs > 0
        ? `It is the only relay; ${count(pairs, 'relayed pair', 'relayed pairs')} will disconnect`
        : 'It is the only relay; nodes without a direct path are unreachable during the restart',
    controllerNote: 'Ignores the automatic update setting, pauses and past failures',
    loadingDetail: 'Loading active flows',
    confirmNow: 'Update now',
    confirmDrain: 'Update after draining',
    ordered: (name, version) => `${name} updating to ${version}`,
    orderedWaitingPackage: (name) => `${name} updates once the package is ready`,
  },

  forceAll: {
    title: (version) => `Update all nodes to ${version}?`,
    body: (n) => `${count(n, 'node', 'nodes')} can update. Offline nodes are skipped; relays update one at a time.`,
    flowsUnknown: '"Now" drops all active flows',
    confirmNow: 'Update all now',
    confirmDrain: 'Update all after draining',
    result: (ordered, queued, skipped) => `${ordered} ordered, ${queued} queued, ${skipped} skipped`,
    resultTitle: 'Update all: result',
    resultOrdered: 'Ordered',
    resultQueued: 'Queued',
    resultSkipped: 'Skipped',
    resultNoVersion: 'No version to roll out',
    dismiss: 'Dismiss',
  },

  detail: {
    upgrade: (version) => `Upgrade to ${version}`,
    securityTip: 'This version fixes a security problem. Upgrade soon',
    manualTitle: (name, version) => `Update ${name} to ${version} by hand`,
    manualDownload: (platform) => `On the machine (${platform}), run:`,
    manualNoPlatform: 'Download the binary from the releases page and replace the current one',
  },

  icon: {
    available: (version) => `${version} available`,
  },

  issue: {
    failed: 'Update failed',
    manual: (version, why) => `${version} needs a manual update (${why})`,
  },
}
