import { Eye, EyeOff } from 'lucide-react'
import { type SyntheticEvent, useState } from 'react'
import { Navigate, useNavigate, useSearchParams } from 'react-router'

import { ApiError, errorDetail, errorMessage } from '@/api/client'
import { api } from '@/api/endpoints'
import { LanguageMenu } from '@/components/language'
import { BrandMark } from '@/components/layout/BrandMark'
import { Notice } from '@/components/notice'
import { Button } from '@/components/ui/button'
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
 *
 * 版面：左上角只有标志，右上角是语言；中间一栏窄窄的表单，标题、令牌、保持登录、按钮，
 * 下面一个安静的"令牌丢失？"。没有副标题和介绍。
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
  const [formError, setFormError] = useState<unknown>(null)
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
      if (error instanceof ApiError && error.is('unauthorized')) setTokenError(errorMessage(error))
      else setFormError(error)
    } finally {
      setPending(false)
    }
  }

  return (
    <div className="flex min-h-screen flex-col">
      <header className="flex h-16 shrink-0 items-center justify-between px-6">
        <BrandMark markOnly />
        <LanguageMenu />
      </header>
      <main className="flex flex-1 justify-center px-6 pt-[9vh] pb-16">
        <div className="w-full max-w-[20rem]">
          <h1 className="text-center text-xl font-semibold tracking-tight">{t.signIn.title}</h1>
          <form onSubmit={(event) => void submit(event)} noValidate className="mt-8 space-y-4">
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
                    spellCheck={false}
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
            <label className="flex items-center gap-2 text-[13px]">
              <Checkbox checked={remember} onCheckedChange={(checked) => setRemember(checked === true)} />
              {t.signIn.remember}
            </label>
            <FormError
              message={formError === null ? null : errorMessage(formError)}
              detail={formError === null ? null : errorDetail(formError)}
            />
            <Button type="submit" variant="primary" pending={pending} className="w-full">
              {t.signIn.submit}
            </Button>
          </form>
          <details className="group mt-6 text-[13px]">
            <summary className="mx-auto w-fit cursor-pointer list-none rounded-sm text-muted-foreground hover:text-foreground hover:underline [&::-webkit-details-marker]:hidden">
              {t.signIn.lostTitle}
            </summary>
            <div className="mt-3 text-muted-foreground">
              <p>{t.signIn.lostBody}</p>
              <pre className="mt-2 overflow-x-auto rounded-md bg-muted px-3 py-2 font-mono text-xs break-words whitespace-pre-wrap text-foreground">
                {t.signIn.lostCommand}
              </pre>
            </div>
          </details>
        </div>
      </main>
    </div>
  )
}
