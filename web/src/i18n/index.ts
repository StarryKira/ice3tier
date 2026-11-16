import { zhCN } from './zh-CN'

/**
 * 界面上的文字都在 `src/i18n/` 下面，组件里不直接写中文。
 *
 * 现在只有简体中文一种。要加第二种语言：照着 `zh-CN/` 再写一个目录，导出同样形状的对象（类型是 {@link Messages}），
 * 然后在这里按用户的选择决定 `t` 指向哪一个。没有做语言切换。
 *
 * 带参数的句子写成函数（`(n) => \`${n} 个节点\``），量词和语序由各语言自己安排。
 */
export type Messages = typeof zhCN

export const t: Messages = zhCN
