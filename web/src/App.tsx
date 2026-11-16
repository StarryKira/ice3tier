import { type QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { type ReactNode } from 'react'

import { ConfirmProvider } from '@/components/confirm'
import { ErrorBoundary } from '@/components/error-boundary'
import { Toaster } from '@/components/ui/toaster'
import { TooltipProvider } from '@/components/ui/tooltip'

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

export function App({ client }: { client: QueryClient }) {
  return (
    <ErrorBoundary>
      <Providers client={client}>
        <AppRoutes />
      </Providers>
    </ErrorBoundary>
  )
}
