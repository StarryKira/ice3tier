import type { zhCN } from './zh-CN'

/**
 * 一种语言的全部界面文字的形状。以简体中文的目录为准：每种语言都要满足它，少一个键、多一个键、
 * 函数的参数不一样都是编译错误。带参数的句子写成函数，量词、单复数和语序由各语言自己安排。
 */
export type Messages = typeof zhCN
