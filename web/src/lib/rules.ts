import { type Rule } from '@/api/types'

/** 规则在文字里的叫法：有名字用名字，没有就写成 `dev ⇄ srv`。 */
export function ruleLabel(rule: Pick<Rule, 'name' | 'src' | 'dst' | 'direction'>): string {
  if (rule.name.trim() !== '') return rule.name
  return `${rule.src.name} ${rule.direction === 'both' ? '⇄' : '→'} ${rule.dst.name}`
}
