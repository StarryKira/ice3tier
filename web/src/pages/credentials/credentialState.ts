import { type Credential } from '@/api/types'
import { type Tone } from '@/components/status'
import { t } from '@/i18n'
import { usageView, type UsageView } from '@/lib/usage'

const s = t.credentials.state

export interface CredentialView {
  tone: Tone
  /** 有效 / 已用完 / 已吊销。 */
  label: string
  /** 为什么是这个状态、怎么恢复；有效时是 `null`。 */
  detail: string | null
  revoked: boolean
  /** 吊销了的不能再改上限、清零用量（controller 回 `invalid_request`）。 */
  editable: boolean
  /**
   * "删除"这一步是什么：第一次是吊销（记录留着，看得到它用过多少），对已经吊销的再删一次才彻底删掉。
   */
  deleteStep: 'revoke' | 'remove'
  traffic: UsageView
  /** "2 / 8"；不限时只写现在的数。 */
  connections: string
}

/** 一个凭据的状态、用量和下一步怎么说。 */
export function credentialView(credential: Credential): CredentialView {
  const traffic = usageView(credential.usage.bytes, credential.max_bytes)
  const connections =
    credential.max_concurrent === null
      ? String(credential.usage.connections)
      : `${credential.usage.connections} / ${credential.max_concurrent}`
  const base = { traffic, connections }
  switch (credential.state) {
    case 'active':
      return {
        ...base,
        tone: traffic.nearly ? 'warn' : 'ok',
        label: s.active,
        detail: traffic.nearly ? s.nearlyDetail(traffic.percent ?? 0) : null,
        revoked: false,
        editable: true,
        deleteStep: 'revoke',
      }
    case 'exhausted':
      return {
        ...base,
        tone: 'danger',
        label: s.exhausted,
        detail: s.exhaustedDetail,
        revoked: false,
        editable: true,
        deleteStep: 'revoke',
      }
    case 'revoked':
      return {
        ...base,
        tone: 'idle',
        label: s.revoked,
        detail: s.revokedDetail,
        revoked: true,
        editable: false,
        deleteStep: 'remove',
      }
    case 'unknown':
      return {
        ...base,
        tone: 'idle',
        label: s.unknown,
        detail: s.unknownDetail,
        revoked: false,
        editable: true,
        deleteStep: 'revoke',
      }
  }
}

/**
 * 列表里用来认出是哪一个凭据的写法：令牌开头的 6 个字符加几个点，和调用者填在密码字段里的开头对得上。
 * 这个字段出现之前签发的凭据没有前缀，返回 `null`。
 */
export function credentialPrefix(prefix: string): string | null {
  return prefix === '' ? null : `${prefix}••••`
}
