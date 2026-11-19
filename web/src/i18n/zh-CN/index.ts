import { access, rules } from './access'
import { addNode } from './addNode'
import { common, confirm, errors, select, time, validate } from './common'
import { capability, declare } from './declare'
import { diagnosis } from './diagnosis'
import { exits, forwards, listen, pairs, serviceClass } from './entry'
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

/** 简体中文的全部界面文字。按页面分在同一个目录的几个文件里，在这里合成一个对象。 */
export const zhCN = {
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
  settings,
  updates,
  routes,
  topology,
  path,
  tests,
  diagnosis,
  proxies,
}
