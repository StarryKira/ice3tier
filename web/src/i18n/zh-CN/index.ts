import { access, rules } from './access'
import { addNode } from './addNode'
import { common, confirm, errors, select, tags, time, validate } from './common'
import { capability, declare } from './declare'
import { credentials, exits, forwards, listeners, pairs, tokens } from './entry'
import { networks } from './networks'
import { nodes } from './nodes'
import { overview } from './overview'
import { settings } from './settings'
import { nav, planned, signIn, switcher } from './shell'

/** 简体中文的全部界面文字。按页面分在同一个目录的几个文件里，在这里合成一个对象。 */
export const zhCN = {
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
  pairs,
  tokens,
  settings,
}
