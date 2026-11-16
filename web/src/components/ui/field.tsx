import { type ReactNode, useId } from 'react'

import { cn } from '@/lib/utils'

interface FieldRenderProps {
  id: string
  'aria-describedby': string | undefined
  'aria-invalid': true | undefined
}

interface FieldProps {
  label: ReactNode
  /** 输入框下面常驻的一句说明，说的是后果或者要求。 */
  hint?: ReactNode
  /** 这个字段的错误：提交之前的校验，或者服务器针对这个字段的拒绝理由。 */
  error?: string | null
  optional?: string
  className?: string
  children: (props: FieldRenderProps) => ReactNode
}

/** 表单里的一项：标签、控件、说明、错误。标签和说明通过 id 关联到控件上。 */
function Field({ label, hint, error, optional, className, children }: FieldProps) {
  const id = useId()
  const hintId = `${id}-hint`
  const errorId = `${id}-error`
  const hasError = error !== undefined && error !== null && error !== ''
  const describedBy = [hasError ? errorId : null, hint !== undefined ? hintId : null].filter(Boolean).join(' ')

  return (
    <div className={cn('flex flex-col gap-1.5', className)}>
      <label htmlFor={id} className="text-[13px] font-medium text-foreground">
        {label}
        {optional !== undefined ? <span className="ml-1.5 font-normal text-muted-foreground">{optional}</span> : null}
      </label>
      {children({
        id,
        'aria-describedby': describedBy === '' ? undefined : describedBy,
        'aria-invalid': hasError ? true : undefined,
      })}
      {hasError ? (
        <p id={errorId} role="alert" className="text-[13px] text-danger">
          {error}
        </p>
      ) : null}
      {hint !== undefined ? (
        <p id={hintId} className="text-[13px] text-muted-foreground">
          {hint}
        </p>
      ) : null}
    </div>
  )
}

/** 整个表单的错误：服务器的拒绝理由说不清属于哪个字段时显示在按钮上方。 */
function FormError({ message, className }: { message: string | null | undefined; className?: string }) {
  if (message === null || message === undefined || message === '') return null
  return (
    <p role="alert" className={cn('rounded-md bg-danger-soft px-3 py-2 text-[13px] text-danger', className)}>
      {message}
    </p>
  )
}

export { Field, FormError }
