import { useQuery } from '@tanstack/react-query'
import { type SyntheticEvent } from 'react'
import { Navigate, useNavigate } from 'react-router'

import { q } from '@/api/queries'
import { BrandMark } from '@/components/layout/BrandMark'
import { ErrorState, SkeletonBlock } from '@/components/states'
import { Button } from '@/components/ui/button'
import { Card } from '@/components/ui/card'
import { FormError } from '@/components/ui/field'
import { t } from '@/i18n'
import { useDocumentTitle } from '@/lib/useDocumentTitle'

import { networkPath } from './context'
import { useNetworkCreateForm } from './NetworkCreateForm'

/**
 * controller 里一个网络都没有时落到这里：一屏，两个字段，都有默认值，直接点"创建"就能过。
 * 其余的设置（选路模式等）不出现在这里，以后在"网络"页改。
 */
export function WelcomePage() {
  useDocumentTitle(t.networks.welcomeTitle)
  const navigate = useNavigate()
  const networks = useQuery(q.networks())
  const form = useNetworkCreateForm((network) => void navigate(networkPath(network.id), { replace: true }))

  const first = networks.data?.[0]
  if (first !== undefined && !form.pending) return <Navigate to={networkPath(first.id)} replace />

  const submit = (event: SyntheticEvent) => {
    event.preventDefault()
    form.submit()
  }

  return (
    <main className="flex min-h-screen flex-col items-center px-6 pt-[12vh] pb-10">
      <BrandMark className="mb-8" />
      <Card className="w-full max-w-lg">
        {networks.isPending ? (
          <SkeletonBlock lines={5} />
        ) : networks.isError ? (
          <ErrorState
            title={t.errors.loadFailed(t.networks.title)}
            error={networks.error}
            onRetry={() => void networks.refetch()}
          />
        ) : (
          <form onSubmit={submit} noValidate className="space-y-5 p-7">
            <div>
              <h1 className="text-lg font-semibold">{t.networks.welcomeTitle}</h1>
              <p className="mt-1.5 text-sm text-muted-foreground">{t.networks.welcomeLead}</p>
            </div>
            {form.fields}
            <FormError message={form.error} />
            <Button type="submit" variant="primary" pending={form.pending} className="w-full">
              {t.networks.createSubmit}
            </Button>
            <p className="text-[13px] text-muted-foreground">{t.networks.noteDefaultRule}</p>
          </form>
        )}
      </Card>
    </main>
  )
}
