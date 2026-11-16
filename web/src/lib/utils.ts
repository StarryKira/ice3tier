import { type ClassValue, clsx } from 'clsx'
import { twMerge } from 'tailwind-merge'

/** 拼接 class 名，后写的 Tailwind 类覆盖先写的同类。 */
export function cn(...inputs: ClassValue[]): string {
  return twMerge(clsx(inputs))
}

/** `switch` 穷尽性检查：编译时走不到这里，运行时遇到没见过的值就按 `fallback` 处理。 */
export function unreachable<T>(_value: never, fallback: T): T {
  return fallback
}
