import { Activity, type LucideIcon } from 'lucide-react'

import { t } from '@/i18n'

/**
 * 白皮书的页面清单里、管理 API 还支持不了的页面。它们在导航里占着位置，页面上只有一句话说明将来这里是什么。
 *
 * 接口有了之后要填上某一页：写好页面组件，把 `routes.tsx` 里那一条路由的 `<PlannedPage id="…" />` 换成它，
 * 再把这里的那一项删掉（导航上的"计划中"标记跟着消失）。拓扑、路径诊断、节点更新就是这样填上的。
 */
export interface PlannedEntry {
  title: string
  /** 一句话：将来这里是什么。 */
  summary: string
  /** 缺的是哪一类接口。 */
  needs: string
  /** 现在可以先去哪里看相近的东西。 */
  meanwhile: string
  icon: LucideIcon
}

const ICONS = { tests: Activity } as const satisfies Record<string, LucideIcon>

export type PlannedId = keyof typeof ICONS

/** 一个计划中的页面的文字和图标。每次现取，换了语言就是新的文字。 */
export function plannedEntry(id: PlannedId): PlannedEntry {
  return { ...t.planned[id], icon: ICONS[id] }
}

export function isPlanned(id: string): id is PlannedId {
  return id in ICONS
}
