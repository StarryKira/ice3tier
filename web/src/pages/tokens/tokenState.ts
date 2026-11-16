import { type EnrollTokenInfo, type NodeRef, type Timestamp } from '@/api/types'
import { type Tone } from '@/components/status'
import { t } from '@/i18n'
import { formatRemaining } from '@/lib/time'

const k = t.tokens

export interface TokenView {
  tone: Tone
  label: string
  /** 用它加入的节点；没用过、或者那个节点后来被删掉了是 `null`。 */
  usedBy: NodeRef | null
  usedAt: Timestamp | null
  /** 用掉了，但那个节点后来被删掉了。 */
  usedByDeleted: boolean
  /**
   * 删除这一步是什么：还能用的是"收回"（拿着它的机器就加入不了了）；用掉的和过期的只是删掉这条记录。
   */
  removal: 'revoke' | 'forget'
}

/** 一个注册令牌现在的情况。controller 说还能用、而浏览器的钟已经过了有效期时，按过期写。 */
export function tokenView(token: EnrollTokenInfo, now: Timestamp): TokenView {
  const base = { usedBy: null, usedAt: null, usedByDeleted: false }
  switch (token.state) {
    case 'pending': {
      const remaining = formatRemaining(token.expires_at, now)
      return remaining === null
        ? { ...base, tone: 'idle', label: k.expired, removal: 'revoke' }
        : { ...base, tone: 'relay', label: k.unused(remaining), removal: 'revoke' }
    }
    case 'used':
      return {
        tone: 'ok',
        label: k.used,
        usedBy: token.used_by,
        usedAt: token.used_at,
        usedByDeleted: token.used_by === null,
        removal: 'forget',
      }
    case 'expired':
      return { ...base, tone: 'idle', label: k.expired, removal: 'forget' }
    case 'unknown':
      return { ...base, tone: 'idle', label: k.unknownState, removal: 'forget' }
  }
}

/**
 * 令牌在界面上的叫法：文本的开头 18 个字符加省略号，和管理员手里的令牌文本的开头对得上。
 * 这个字段出现之前生成的令牌没有前缀，用编号。
 */
export function tokenLabel(token: Pick<EnrollTokenInfo, 'id' | 'prefix'>): string {
  return token.prefix === '' ? k.tokenId(token.id) : `${token.prefix}…`
}

/** 列全部的时候：还能用的在前面，其余的新的在前。 */
export function sortTokens(tokens: readonly EnrollTokenInfo[]): EnrollTokenInfo[] {
  return [...tokens].sort((a, b) => Number(b.state === 'pending') - Number(a.state === 'pending') || b.id - a.id)
}
