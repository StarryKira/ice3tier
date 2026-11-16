import type { Messages } from '../messages'

export const signIn: Messages['signIn'] = {
  title: 'Sign in to ice3tier',
  tokenLabel: 'Admin token',
  show: 'Show token',
  hide: 'Hide token',
  remember: 'Stay signed in',
  submit: 'Sign in',
  empty: 'Enter the admin token',
  expired: 'Session expired',
  signedOut: 'Signed out',
  lostTitle: 'Lost the token?',
  lostBody: 'Stop the controller, then run it once as below. It prints a new token; the old one stops working.',
  lostCommand: 'i3t-controller --data-dir <data-dir> --public-addr <host:port> --reset-admin-token',
}

export const nav: Messages['nav'] = {
  label: 'Main navigation',
  overview: 'Overview',
  nodes: 'Nodes',
  access: 'Access control',
  groupAccess: 'Traffic',
  exits: 'Exits',
  listeners: 'Listeners',
  credentials: 'Credentials',
  forwards: 'Port forwards',
  groupDiagnose: 'Diagnostics',
  topology: 'Topology',
  diagnosis: 'Path diagnosis',
  tests: 'Connectivity tests',
  tokens: 'Enrollment tokens',
  pairs: 'Node pairs',
  network: 'Network',
  settings: 'Settings',
  collapse: 'Collapse sidebar',
  expand: 'Expand sidebar',
  toggleGroup: (name, open) => `${open ? 'Collapse' : 'Expand'} ${name}`,
  account: 'Admin',
  accountMenu: 'Account menu',
  signOut: 'Sign out',
  skipToContent: 'Skip to content',
  breadcrumb: 'Breadcrumb',
}

export const switcher: Messages['switcher'] = {
  label: 'Switch network',
  current: 'Current network',
  nodes: (n) => (n === 1 ? '1 node' : `${n} nodes`),
  create: 'Create network',
  manage: 'Network settings',
  heading: 'Networks',
  switched: (name) => `Switched to ${name}`,
}

export const planned: Messages['planned'] = {
  badge: 'Planned',
  note: 'The controller does not provide the needed API yet',
  meanwhile: 'For now',
  tests: {
    title: 'Connectivity tests',
    summary:
      'Controller-initiated tests between node pairs: whether they connect directly, latency direct and via relay, with history.',
    needs: 'Missing API: start a test, read results',
    meanwhile: 'Path diagnosis for one pair, Topology for the whole network',
  },
}
