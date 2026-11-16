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
        'flex w-full items-center gap-2.5 rounded-lg border bg-card text-left shadow-card transition-colors hover:bg-muted/60',
        collapsed ? 'size-9 justify-center' : 'px-2.5 py-2',
      )}
    >
      <span
        aria-hidden
        className="flex size-6 shrink-0 items-center justify-center rounded-md bg-primary text-xs font-semibold text-primary-foreground uppercase"
      >
        {current.name.slice(0, 1)}
      </span>
      {collapsed ? null : (
        <>
          <span className="min-w-0 flex-1">
            <span className="block truncate text-sm leading-tight font-semibold">{current.name}</span>
            <span className="block truncate text-xs leading-tight text-muted-foreground tabular">
              {current.cidr} · {t.switcher.nodes(current.nodes)}
            </span>
          </span>
          <ChevronsUpDown className="size-3.5 shrink-0 text-muted-foreground" aria-hidden />
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
