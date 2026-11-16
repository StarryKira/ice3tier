import { t } from '@/i18n'

/** 平台的名字：认识的写成"Linux x86_64""macOS（Apple 芯片）"，不认识的原样。 */
export function platformLabel(platform: string): string {
  if (platform === '') return '—'
  const names: Record<string, string> = t.addNode.platform
  return Object.hasOwn(names, platform) ? (names[platform] ?? platform) : platform
}
