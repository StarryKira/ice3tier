import type { Messages } from '../messages'

export const capability: Messages['capability'] = {
  relay: { name: 'relay', full: 'Can relay', short: 'Relay' },
  exit: { name: 'exit', full: 'Can be an exit', short: 'Exit' },
  forward: { name: 'port forward target', full: 'Can be a port forward target', short: 'Forward target' },
}

export const declare: Messages['declare'] = {
  howTo: 'How to declare',
  hide: 'Hide',
  notDeclared: (node) => `Not declared on ${node}`,
  step1: (node) => `On ${node}, run:`,
  step2: 'Restart i3t',
  step3: 'This becomes available when the declaration arrives',
  orConfig: 'Or add to config.toml',
  waiting: 'Waiting for the declaration…',
  arrived: 'Declared. Ready to enable',
  copyCommand: 'Copy command',
}
