import { ChevronDown, Languages } from 'lucide-react'

import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { type Locale, LOCALES, setLocale, t, useLocale } from '@/i18n'
import { cn } from '@/lib/utils'

const nameOf = (code: Locale) => LOCALES.find((locale) => locale.code === code)?.name ?? code

/** 每种语言一项，用它自己的文字写名字（`lang` 标上，读屏软件按那种语言读）。 */
export function LanguageItems() {
  const locale = useLocale()
  return (
    <DropdownMenuRadioGroup
      value={locale}
      onValueChange={(value) => {
        const next = LOCALES.find((entry) => entry.code === value)
        if (next !== undefined) setLocale(next.code)
      }}
    >
      {LOCALES.map((entry) => (
        <DropdownMenuRadioItem key={entry.code} value={entry.code} lang={entry.code}>
          {entry.name}
        </DropdownMenuRadioItem>
      ))}
    </DropdownMenuRadioGroup>
  )
}

/** 登录页右上角的语言菜单：图标、现在的语言、向下的箭头。 */
export function LanguageMenu({ className }: { className?: string }) {
  const locale = useLocale()
  return (
    <DropdownMenu>
      <DropdownMenuTrigger
        aria-label={`${t.common.language}: ${nameOf(locale)}`}
        className={cn(
          'inline-flex h-8 items-center gap-1.5 rounded-md px-2.5 text-[13px] text-foreground hover:bg-muted data-[state=open]:bg-muted',
          className,
        )}
      >
        <Languages className="size-4 text-muted-foreground" aria-hidden />
        {nameOf(locale)}
        <ChevronDown className="size-3.5 text-muted-foreground" aria-hidden />
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="min-w-40">
        <LanguageItems />
      </DropdownMenuContent>
    </DropdownMenu>
  )
}
