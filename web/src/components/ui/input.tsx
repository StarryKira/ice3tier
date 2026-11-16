import { type ComponentProps } from 'react'

import { cn } from '@/lib/utils'

/**
 * 输入框、下拉、文本框共用的外观：白底、细边、没有阴影。聚焦时边变成强调色，外面一圈淡的同色光晕。
 * 别处自己画输入框时（搜索框、标签输入）也用它。
 */
export const fieldBase =
  'w-full rounded-md border border-input bg-card px-3 text-sm text-foreground transition-colors placeholder:text-muted-foreground hover:border-border-strong disabled:bg-muted disabled:text-muted-foreground focus-visible:border-primary focus-visible:outline-3 focus-visible:outline-offset-0 focus-visible:outline-primary/20 aria-invalid:border-danger aria-invalid:focus-visible:outline-danger/20'

function Input({ className, type, ...props }: ComponentProps<'input'>) {
  return (
    <input
      type={type ?? 'text'}
      autoComplete="off"
      spellCheck={false}
      className={cn(fieldBase, 'h-9', className)}
      {...props}
    />
  )
}

function Textarea({ className, ...props }: ComponentProps<'textarea'>) {
  return (
    <textarea spellCheck={false} className={cn(fieldBase, 'min-h-18 py-2 leading-relaxed', className)} {...props} />
  )
}

function NativeSelect({ className, children, ...props }: ComponentProps<'select'>) {
  return (
    <select
      className={cn(
        fieldBase,
        'h-9 appearance-none bg-[url("data:image/svg+xml,%3Csvg%20xmlns=%27http://www.w3.org/2000/svg%27%20width=%2716%27%20height=%2716%27%20fill=%27none%27%20stroke=%27%236b6b74%27%20stroke-width=%272%27%20stroke-linecap=%27round%27%20stroke-linejoin=%27round%27%3E%3Cpath%20d=%27m4%206%204%204%204-4%27/%3E%3C/svg%3E")] bg-[position:right_0.6rem_center] bg-no-repeat pr-8',
        className,
      )}
      {...props}
    >
      {children}
    </select>
  )
}

export { Input, NativeSelect, Textarea }
