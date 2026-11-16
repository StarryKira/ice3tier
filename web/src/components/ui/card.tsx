import { type ComponentProps, type ReactNode } from 'react'

import { cn } from '@/lib/utils'

function Card({ className, ...props }: ComponentProps<'section'>) {
  return <section className={cn('rounded-lg border bg-card text-card-foreground shadow-card', className)} {...props} />
}

interface CardHeaderProps {
  title: ReactNode
  description?: ReactNode
  action?: ReactNode
  className?: string
}

function CardHeader({ title, description, action, className }: CardHeaderProps) {
  return (
    <header className={cn('flex items-start justify-between gap-4 border-b px-5 py-3.5', className)}>
      <div className="min-w-0">
        <h2 className="text-[15px] font-semibold">{title}</h2>
        {description !== undefined ? <p className="mt-0.5 text-[13px] text-muted-foreground">{description}</p> : null}
      </div>
      {action !== undefined ? <div className="flex shrink-0 items-center gap-2">{action}</div> : null}
    </header>
  )
}

function CardBody({ className, ...props }: ComponentProps<'div'>) {
  return <div className={cn('px-5 py-4', className)} {...props} />
}

export { Card, CardBody, CardHeader }
