import { useQuery } from '@tanstack/react-query'
import { LogOut } from 'lucide-react'
import { useNavigate } from 'react-router'

import { q } from '@/api/queries'
import { session } from '@/auth/session'
import { CodeBlock } from '@/components/copy'
import { SkeletonBlock } from '@/components/states'
import { Button } from '@/components/ui/button'
import { Card, CardHeader } from '@/components/ui/card'
import { t } from '@/i18n'
import { resetTokenCommands } from '@/lib/commands'

const s = t.settings.session

/**
 * 登录：这个浏览器里的管理令牌（退出登录），和令牌丢了怎么重置。
 *
 * 内容排成一栏，宽度是最长的那条命令一行写下来要的（窗口窄时才折行），不跟着面板拉宽：
 * 退出登录的按钮和命令的右边对齐，而不是远在面板的另一头。
 *
 * 重置的命令填好了这个 controller 的数据目录，复制了就能执行；
 * 取不到（controller 没应答、不是从数据目录打开的）时退回登录页上那条带占位符的。
 */
export function SessionTab() {
  const navigate = useNavigate()
  const status = useQuery(q.status())
  const dataDir = status.data?.data_dir ?? null
  const commands = dataDir === null ? null : resetTokenCommands(dataDir)
  const command = commands?.join('\n') ?? t.signIn.lostCommand
  // 三条的那一种自己带着停和启动，说明里不再让人去停。
  const steps = commands !== null && commands.length > 1

  return (
    <div className="w-fit max-w-full space-y-12">
      <Card>
        <CardHeader
          title={s.title}
          description={session.remembered() ? s.stored : s.tabOnly}
          className="mb-0"
          action={
            <Button
              size="sm"
              onClick={() => {
                session.signOut('manual')
                void navigate('/login', { replace: true })
              }}
            >
              <LogOut aria-hidden />
              {s.signOut}
            </Button>
          }
        />
      </Card>
      <Card>
        <CardHeader title={s.resetTitle} description={steps ? s.resetService : t.signIn.lostBody} />
        {status.isPending ? (
          <SkeletonBlock lines={1} />
        ) : (
          <CodeBlock copy={command} label={s.resetTitle} testId="reset-token-command">
            {command}
          </CodeBlock>
        )}
      </Card>
    </div>
  )
}
