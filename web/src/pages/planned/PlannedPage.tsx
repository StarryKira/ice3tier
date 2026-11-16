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
    <Card className="flex flex-col items-center px-8 py-14 text-center">
      <span className="mb-4 flex size-11 items-center justify-center rounded-full bg-muted text-muted-foreground">
        <Icon className="size-5" aria-hidden />
      </span>
      <p className="max-w-2xl text-[15px] leading-relaxed">{entry.summary}</p>
      <p className="mt-4 max-w-2xl text-[13px] text-muted-foreground">{t.planned.note}</p>
      <p className="mt-1 max-w-2xl text-[13px] text-muted-foreground">{entry.needs}</p>
      <p className="mt-5 max-w-2xl rounded-md bg-muted px-3.5 py-2 text-[13px]">
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
    <Page>
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
