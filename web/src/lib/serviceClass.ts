import { type ServiceClass } from '@/api/types'
import { t } from '@/i18n'

export const SERVICE_CLASSES: readonly ServiceClass[] = ['auto', 'interactive', 'reliable', 'bulk']

/** 服务等级在界面上的名字。 */
export function classLabel(value: ServiceClass): string {
  return t.serviceClass[value]
}
