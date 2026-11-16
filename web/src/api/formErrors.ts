import { ApiError, errorDetail, errorMessage } from './client'
import { type KnownErrorCode } from './types'

export interface FormFailure<F extends string> {
  /** 这个错误说的是哪个字段；说不清时是 `null`，显示在整个表单上。 */
  field: F | null
  /** 按错误码写的一句，用当前的语言。 */
  message: string
  /** controller 的原话，码太笼统时作为补充显示（见 `errorDetail`）。 */
  detail: string | null
}

/**
 * 把一次失败的提交归到表单的某个字段上。
 *
 * controller 的错误码能说明问题出在哪一类东西上（名字被占用、地址段重叠），
 * `map` 写明这个表单里每种错误码对应哪个字段；没有列出的错误显示在表单底部。
 */
export function formFailure<F extends string>(
  error: unknown,
  map: Partial<Record<KnownErrorCode, F>>,
): FormFailure<F> | null {
  if (error === null || error === undefined) return null
  const message = errorMessage(error)
  const detail = errorDetail(error)
  if (error instanceof ApiError) {
    const field = (map as Partial<Record<string, F>>)[error.code]
    if (field !== undefined) return { field, message, detail }
  }
  return { field: null, message, detail }
}

/** 某个字段该显示的服务器错误。 */
export function fieldError<F extends string>(failure: FormFailure<F> | null, field: F): string | null {
  return failure !== null && failure.field === field ? failure.message : null
}

/** 表单底部该显示的服务器错误。 */
export function formError<F extends string>(failure: FormFailure<F> | null): string | null {
  return failure !== null && failure.field === null ? failure.message : null
}

/** 表单底部那条错误的补充：controller 的原话。 */
export function formErrorDetail<F extends string>(failure: FormFailure<F> | null): string | null {
  return failure !== null && failure.field === null ? failure.detail : null
}
