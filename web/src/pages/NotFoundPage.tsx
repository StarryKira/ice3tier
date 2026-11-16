import { Compass } from 'lucide-react'
import { Link } from 'react-router'

import { Page } from '@/components/page'
import { EmptyState } from '@/components/states'
import { Button } from '@/components/ui/button'
import { Card } from '@/components/ui/card'
import { t } from '@/i18n'

export function NotFoundPage() {
  return (
    <Page>
      <Card>
        <EmptyState
          icon={Compass}
          title={t.errors.notFoundTitle}
          action={
            <Button asChild variant="primary">
              <Link to="/">{t.errors.backHome}</Link>
            </Button>
          }
        />
      </Card>
    </Page>
  )
}
