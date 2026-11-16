import { type QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { type ReactNode } from 'react'

import { ConfirmProvider } from '@/components/confirm'
import { ErrorBoundary } from '@/components/error-boundary'
import { Toaster } from '@/components/ui/toaster'
import { TooltipProvider } from '@/components/ui/tooltip'
import { useLocale } from '@/i18n'

import { AppRoutes } from './routes'

/** 页面用到的各种上下文。测试里也用它把单个组件包起来。 */
export function Providers({ client, children }: { client: QueryClient; children: ReactNode }) {
  return (
    <QueryClientProvider client={client}>
      <TooltipProvider delayDuration={250} skipDelayDuration={150}>
        <ConfirmProvider>
          {children}
          <Toaster />
        </ConfirmProvider>
      </TooltipProvider>
    </QueryClientProvider>
  )
}

/**
 * 应用的根。它订阅语言：换语言时它重新渲染，下面的元素都是在这里新建的，所以整棵树跟着用新的文字重新渲染，
 * 而不是重新挂载，页面上的状态都留着。
 */
export function App({ client }: { client: QueryClient }) {
  useLocale()
  return (
    <ErrorBoundary>
      <Providers client={client}>
        <AppRoutes />
      </Providers>
    </ErrorBoundary>
  )
}
