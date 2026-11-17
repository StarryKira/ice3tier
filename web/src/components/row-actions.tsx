import { EllipsisVertical } from 'lucide-react'
import { type ReactNode } from 'react'

import { Button } from '@/components/ui/button'
import { DropdownMenu, DropdownMenuContent, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'

/** 表格每行最右边的"⋮"：点开是这一行的操作。`label` 是给读屏的名字（"gw-hk 的操作"）。 */
export function RowActions({ label, children }: { label: string; children: ReactNode }) {
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button variant="ghost" size="icon-sm" aria-label={label}>
          <EllipsisVertical aria-hidden />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent>{children}</DropdownMenuContent>
    </DropdownMenu>
  )
}
