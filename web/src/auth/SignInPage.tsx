import { Eye, EyeOff } from 'lucide-react'
import { type SyntheticEvent, useState } from 'react'
import { Navigate, useNavigate, useSearchParams } from 'react-router'

import { ApiError, errorMessage } from '@/api/client'
import { api } from '@/api/endpoints'
import { BrandMark } from '@/components/layout/BrandMark'
import { Notice } from '@/components/notice'
import { Button } from '@/components/ui/button'
import { Card } from '@/components/ui/card'
import { Checkbox } from '@/components/ui/checkbox'
import { Field, FormError } from '@/components/ui/field'
import { Input } from '@/components/ui/input'
import { t } from '@/i18n'
import { useDocumentTitle } from '@/lib/useDocumentTitle'

import { session, type SignOutReason, useToken } from './session'

/** 登录之后回到原来要去的地址。只接受站内的路径，免得被人拿一个带 `next` 的链接引到别处。 */
function safeNext(raw: string | null): string {
  if (raw === null || !raw.startsWith('/') || raw.startsWith('//') || raw.startsWith('/login')) return '/'
  return raw
}

/**
 * 登录就是填管理令牌。提交时拿它去取一次 controller 的状态，取得到就算登录成功。
 */
export function SignInPage() {
  useDocumentTitle(t.signIn.submit)
  const navigate = useNavigate()
  const [search] = useSearchParams()
  const next = safeNext(search.get('next'))
  const existing = useToken()

  const [token, setToken] = useState('')
  const [visible, setVisible] = useState(false)
  const [remember, setRemember] = useState(true)
  const [pending, setPending] = useState(false)
  const [tokenError, setTokenError] = useState<string | null>(null)
  const [formError, setFormError] = useState<string | null>(null)
  const [reason] = useState<SignOutReason | null>(() => session.takeSignOutReason())

  if (existing !== null && !pending) return <Navigate to={next} replace />

  const submit = async (event: SyntheticEvent) => {
    event.preventDefault()
    const value = token.trim()
    setFormError(null)
    if (value === '') {
      setTokenError(t.signIn.empty)
      return
    }
    setPending(true)
    try {
      await api.status(value)
      session.signIn(value, remember)
      void navigate(next, { replace: true })
    } catch (error) {
      // 令牌不对是这个输入框的错；别的（连不上、controller 出错）写在表单上。
      if (error instanceof ApiError && error.is('unauthorized')) setTokenError(error.message)
      else setFormError(errorMessage(error))
    } finally {
      setPending(false)
    }
  }

  return (
    <main className="flex min-h-screen flex-col items-center px-6 pt-[16vh] pb-10">
      <Card className="w-full max-w-sm">
        <BrandMark />
        <p className="mt-2 mb-12 text-[13px] text-muted-foreground">{t.signIn.tagline}</p>
        <form onSubmit={(event) => void submit(event)} noValidate className="space-y-5">
          <div>
            <h1 className="text-xl font-semibold tracking-tight">{t.signIn.title}</h1>
            <p className="mt-1.5 text-[13px] leading-relaxed text-muted-foreground">{t.signIn.lead}</p>
          </div>
          {reason !== null ? (
            <Notice tone={reason === 'expired' ? 'warn' : 'info'}>
              {reason === 'expired' ? t.signIn.expired : t.signIn.signedOut}
            </Notice>
          ) : null}
          <Field label={t.signIn.tokenLabel} error={tokenError}>
            {(props) => (
              <div className="relative">
                <Input
                  {...props}
                  name="token"
                  type={visible ? 'text' : 'password'}
                  value={token}
                  autoFocus
                  autoComplete="current-password"
                  placeholder={t.signIn.tokenPlaceholder}
                  className="pr-10 font-mono"
                  onChange={(event) => {
                    setToken(event.target.value)
                    setTokenError(null)
                  }}
                />
                <button
                  type="button"
                  onClick={() => setVisible((value) => !value)}
                  aria-label={visible ? t.signIn.hide : t.signIn.show}
                  aria-pressed={visible}
                  className="absolute top-1/2 right-1.5 flex size-7 -translate-y-1/2 items-center justify-center rounded text-muted-foreground hover:bg-muted hover:text-foreground"
                >
                  {visible ? <EyeOff className="size-4" aria-hidden /> : <Eye className="size-4" aria-hidden />}
                </button>
              </div>
            )}
          </Field>
          <label className="flex items-start gap-2.5">
            <Checkbox
              checked={remember}
              onCheckedChange={(checked) => setRemember(checked === true)}
              className="mt-0.5"
            />
            <span>
              <span className="block text-sm">{t.signIn.remember}</span>
              <span className="block text-[13px] text-muted-foreground">{t.signIn.rememberHint}</span>
            </span>
          </label>
          <FormError message={formError} />
          <Button type="submit" variant="primary" pending={pending} className="w-full">
            {t.signIn.submit}
          </Button>
        </form>
        <details className="mt-10 border-t pt-5 text-[13px] text-muted-foreground">
          <summary className="rounded-sm font-medium text-foreground">{t.signIn.lostTitle}</summary>
          <p className="mt-2">{t.signIn.lostBody}</p>
          <pre className="mt-2 overflow-x-auto rounded-md bg-muted px-3 py-2 font-mono text-xs break-all whitespace-pre-wrap text-foreground">
            {t.signIn.lostCommand}
          </pre>
        </details>
      </Card>
    </main>
  )
}
