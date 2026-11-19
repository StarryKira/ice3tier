import type { Messages } from '../messages'
import { access, rules } from './access'
import { addNode } from './addNode'
import { common, confirm, errors, select, time, validate } from './common'
import { capability, declare } from './declare'
import { diagnosis } from './diagnosis'
import { exits, forwards, listen, pairs, serviceClass, tokens } from './entry'
import { groups } from './groups'
import { networks } from './networks'
import { nodes } from './nodes'
import { overview } from './overview'
import { path, routes, topology } from './routes'
import { settings } from './settings'
import { nav, signIn, switcher } from './shell'
import { proxies } from './proxies'
import { tests } from './tests'
import { updates, version } from './updates'

/** English. 和 `zh-CN/` 同样按页面分文件，在这里合成一个对象。 */
export const en: Messages = {
  common,
  errors,
  confirm,
  validate,
  time,
  select,
  signIn,
  nav,
  switcher,
  version,
  capability,
  declare,
  networks,
  nodes,
  addNode,
  groups,
  rules,
  access,
  overview,
  exits,
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
  tests,
  diagnosis,
  proxies,
}
