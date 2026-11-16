import { Component, type ErrorInfo, type ReactNode } from 'react'

import { Button } from '@/components/ui/button'
import { t } from '@/i18n'

interface State {
  failed: boolean
}

/** 界面自己出了错（不是请求失败）：不留一张白页，说一句并给一个刷新的按钮。 */
export class ErrorBoundary extends Component<{ children: ReactNode }, State> {
  override state: State = { failed: false }

  static getDerivedStateFromError(): State {
    return { failed: true }
  }

  override componentDidCatch(error: Error, info: ErrorInfo): void {
    console.error(error, info.componentStack)
  }

  override render(): ReactNode {
    if (!this.state.failed) return this.props.children
    return (
      <main
        role="alert"
        className="mx-auto flex min-h-screen max-w-md flex-col items-center justify-center px-6 text-center"
      >
        <h1 className="text-lg font-semibold">{t.errors.crashedTitle}</h1>
        <Button variant="primary" className="mt-5" onClick={() => window.location.reload()}>
          {t.errors.reload}
        </Button>
      </main>
    )
  }
}
