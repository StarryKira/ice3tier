import { Badge } from '@/components/ui/badge'
import { Card } from '@/components/ui/card'
import { Page, PageHeader } from '@/components/page'
import { t } from '@/i18n'
import { useDocumentTitle } from '@/lib/useDocumentTitle'

import { PLANNED, type PlannedId } from './planned'

/** 计划中的页面的全部内容：一句话说明将来这里是什么。没有数据，也不用假数据占位。 */
export function PlannedContent({ id }: { id: PlannedId }) {
  const entry = PLANNED[id]
  const Icon = entry.icon
  return (
    <Card className="flex flex-col items-center px-8 py-16 text-center">
      <Icon className="mb-5 size-7 text-faint" aria-hidden />
      <p className="max-w-xl text-base leading-relaxed">{entry.summary}</p>
      <p className="mt-4 max-w-xl text-[13px] text-muted-foreground">{t.planned.note}</p>
      <p className="mt-1 max-w-xl text-[13px] text-muted-foreground">{entry.needs}</p>
      <p className="mt-8 max-w-xl border-t pt-5 text-[13px]">
        <span className="font-medium">{t.planned.meanwhile}：</span>
        {entry.meanwhile}
      </p>
    </Card>
  )
}

export function PlannedPage({ id }: { id: PlannedId }) {
  const entry = PLANNED[id]
  useDocumentTitle(entry.title)
  return (
    <Page size="narrow">
      <PageHeader
        title={
          <>
            {entry.title}
            <Badge tone="outline">{t.planned.badge}</Badge>
          </>
        }
      />
      <PlannedContent id={id} />
    </Page>
  )
}
