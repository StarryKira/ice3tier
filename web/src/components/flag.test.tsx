import { screen } from '@testing-library/react'
import { describe, expect, it } from 'vitest'

import { renderWithApp } from '@/test/harness'

import { Flag } from './flag'

describe('国旗', () => {
  it('两个大写字母：画那个国家的圆形国旗（SVG），名字是那个国家', () => {
    renderWithApp(<Flag code="JP" />)
    const flag = screen.getByRole('img')
    expect(flag).toHaveAttribute('src', '/flags/jp.svg')
    expect(flag).toHaveAccessibleName(
      new Intl.DisplayNames([document.documentElement.lang], { type: 'region' }).of('JP') ?? 'JP',
    )
  })

  it('没有、不知道、格式不对：什么都不画', () => {
    for (const code of [null, undefined, '', 'jp', 'JPN']) {
      const { unmount } = renderWithApp(<Flag code={code} />)
      expect(screen.queryByRole('img')).not.toBeInTheDocument()
      unmount()
    }
  })
})
