import ts from 'typescript'
import { describe, expect, it } from 'vitest'

/**
 * 换语言是就地改写 `t` 再重新渲染，不刷新页面。所以不能在模块加载时就把目录里的一句话取出来存着
 * （`const LABEL = t.nodes.title`、`{ label: t.nav.nodes }`），那样换了语言它也不变。
 * 取整个分区的简写（`const d = t.nodes.detail`）可以：它拿着的是会被就地改写的对象。
 *
 * 这里扫一遍 `src/` 下的源码，找出所有不在函数里的、对目录的读取。
 */
// 源码的原文，由 Vite 在测试时读进来（不用 Node 的文件接口，测试和源码用同一套类型）。
const SOURCES = import.meta.glob<string>(
  ['/src/**/*.{ts,tsx}', '!/src/**/*.test.*', '!/src/i18n/**', '!/src/test/**'],
  {
    query: '?raw',
    import: 'default',
    eager: true,
  },
)

function staleReads(file: string, text: string): string[] {
  if (!text.includes("from '@/i18n'")) return []
  const kind = file.endsWith('.tsx') ? ts.ScriptKind.TSX : ts.ScriptKind.TS
  const source = ts.createSourceFile(file, text, ts.ScriptTarget.Latest, true, kind)

  const rootOf = (node: ts.Expression): ts.Expression => {
    let current = node
    while (ts.isPropertyAccessExpression(current) || ts.isElementAccessExpression(current)) current = current.expression
    return current
  }
  // `t` 和模块顶层取整个分区的简写。
  const roots = new Set(['t'])
  for (const statement of source.statements) {
    if (!ts.isVariableStatement(statement)) continue
    for (const declaration of statement.declarationList.declarations) {
      const init = declaration.initializer
      if (init === undefined || !ts.isIdentifier(declaration.name)) continue
      const root = rootOf(init)
      if (root !== init && ts.isIdentifier(root) && roots.has(root.text)) roots.add(declaration.name.text)
    }
  }

  const found: string[] = []
  const visit = (node: ts.Node, inFunction: boolean): void => {
    const inside = inFunction || ts.isFunctionLike(node)
    if (!inside && (ts.isPropertyAccessExpression(node) || ts.isElementAccessExpression(node))) {
      const parent = node.parent
      const outermost = !(
        (ts.isPropertyAccessExpression(parent) || ts.isElementAccessExpression(parent)) &&
        parent.expression === node
      )
      const root = rootOf(node)
      const isAlias =
        ts.isVariableDeclaration(parent) && parent.initializer === node && parent.parent.parent.parent === source
      if (outermost && !isAlias && ts.isIdentifier(root) && roots.has(root.text)) {
        const { line } = source.getLineAndCharacterOfPosition(node.getStart())
        found.push(`${file}:${line + 1} ${node.getText()}`)
      }
    }
    ts.forEachChild(node, (child) => visit(child, inside))
  }
  visit(source, false)
  return found
}

describe('界面文字跟着语言走', () => {
  it('没有在模块加载时取出来存着的文字', () => {
    expect(Object.entries(SOURCES).flatMap(([file, text]) => staleReads(file, text))).toEqual([])
  })
})
