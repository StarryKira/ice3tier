import { useEffect } from 'react'

import { t } from '@/i18n'

/** 浏览器标签页上的标题："节点 · ice3tier"。 */
export function useDocumentTitle(title: string): void {
  useEffect(() => {
    document.title = `${title} · ${t.common.appName}`
    return () => {
      document.title = t.common.appName
    }
  }, [title])
}
