import { Check, ChevronsUpDown, Plus, Settings2 } from 'lucide-react'
import { useLocation, useNavigate } from 'react-router'

import { type Network } from '@/api/types'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { Tooltip } from '@/components/ui/tooltip'
import { t } from '@/i18n'
import { networkPath } from '@/pages/networks/context'
import { cn } from '@/lib/utils'

import { currentPage } from './nav'

interface NetworkSwitcherProps {
  networks: readonly Network[]
  current: Network
  collapsed: boolean
  onCreate: () => void
}

/**
 * 侧栏顶部的网络切换器。只有一个网络时照样显示名字：让用户知道"网络"这个概念存在。
 * 按钮上只写名字，地址段和节点数在展开的列表里。
 * 切换时停留在同一类页面上：在节点页切换，到的是另一个网络的节点页。
 */
export function NetworkSwitcher({ networks, current, collapsed, onCreate }: NetworkSwitcherProps) {
  const navigate = useNavigate()
  const location = useLocation()
  const page = currentPage(location.pathname) ?? 'overview'

  const trigger = (
    <DropdownMenuTrigger
      aria-label={`${t.switcher.label}（${t.switcher.current}：${current.name}）`}
      className={cn(
        'flex w-full items-center gap-3 rounded-[10px] text-left transition-colors hover:bg-card/70 data-[state=open]:bg-card/70',
        collapsed ? 'justify-center p-1' : 'px-2 py-1.5',
      )}
    >
      <span
        aria-hidden
        className="flex size-8 shrink-0 items-center justify-center rounded-[10px] bg-foreground text-[13px] font-semibold text-background uppercase"
      >
        {current.name.slice(0, 1)}
      </span>
      {collapsed ? null : (
        <>
          <span className="min-w-0 flex-1 truncate text-[15px] leading-5 font-semibold">{current.name}</span>
          <ChevronsUpDown className="size-3.5 shrink-0 text-faint" aria-hidden />
        </>
      )}
    </DropdownMenuTrigger>
  )

  return (
    <DropdownMenu>
      {collapsed ? (
        <Tooltip content={`${t.switcher.current}：${current.name}`} side="right">
          {trigger}
        </Tooltip>
      ) : (
        trigger
      )}
      <DropdownMenuContent align="start" className="w-64">
        <DropdownMenuLabel>{t.switcher.heading}</DropdownMenuLabel>
        {networks.map((network) => (
          <DropdownMenuItem
            key={network.id}
            onSelect={() => {
              if (network.id !== current.id) void navigate(networkPath(network.id, page))
            }}
          >
            <span className="min-w-0 flex-1">
              <span className="block truncate font-medium">{network.name}</span>
              <span className="block truncate text-xs text-muted-foreground tabular">
                {network.cidr} · {t.switcher.nodes(network.nodes)}
              </span>
            </span>
            {network.id === current.id ? <Check className="text-primary!" aria-label={t.switcher.current} /> : null}
          </DropdownMenuItem>
        ))}
        <DropdownMenuSeparator />
        <DropdownMenuItem onSelect={onCreate}>
          <Plus aria-hidden />
          {t.switcher.create}
        </DropdownMenuItem>
        <DropdownMenuItem onSelect={() => void navigate(networkPath(current.id, 'network'))}>
          <Settings2 aria-hidden />
          {t.switcher.manage}
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  )
}
