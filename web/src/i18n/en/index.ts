import type { Messages } from '../messages'
import { access, rules } from './access'
import { addNode } from './addNode'
import { common, confirm, errors, select, tags, time, validate } from './common'
import { capability, declare } from './declare'
import { credentials, exits, forwards, listen, listeners, pairs, serviceClass, tokens } from './entry'
import { networks } from './networks'
import { nodes } from './nodes'
import { overview } from './overview'
import { path, routes, topology } from './routes'
import { settings } from './settings'
import { nav, planned, signIn, switcher } from './shell'
import { updates, version } from './updates'

/** English. 和 `zh-CN/` 同样按页面分文件，在这里合成一个对象。 */
export const en: Messages = {
  common,
  errors,
  confirm,
  validate,
  time,
  tags,
  select,
  signIn,
  nav,
  switcher,
  planned,
  version,
  capability,
  declare,
  networks,
  nodes,
  addNode,
  rules,
  access,
  overview,
  exits,
  credentials,
  listeners,
  forwards,
  listen,
  serviceClass,
  pairs,
  tokens,
  settings,
  updates,
  routes,
  topology,
  path,
}
