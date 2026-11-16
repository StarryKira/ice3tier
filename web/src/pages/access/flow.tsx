import {
  type AriaLabelConfig,
  Controls,
  type Edge,
  type Node,
  ReactFlow,
  type ReactFlowProps,
  useReactFlow,
  useStore,
} from '@xyflow/react'
import { useEffect } from 'react'

import { t } from '@/i18n'

const f = t.access.graph.flow

const DIRECTIONS: Record<string, string> = { up: '上', down: '下', left: '左', right: '右' }

/** 画图的库自带的读屏文字是英文的，这里换成中文。 */
const ARIA: Partial<AriaLabelConfig> = {
  'node.a11yDescription.default': f.nodeDefault,
  'node.a11yDescription.keyboardDisabled': f.nodeKeyboardDisabled,
  'node.a11yDescription.ariaLiveMessage': ({ direction, x, y }) =>
    f.nodeMoved(DIRECTIONS[direction] ?? direction, Math.round(x), Math.round(y)),
  'edge.a11yDescription.default': f.edgeDefault,
  'controls.ariaLabel': f.controls,
  'controls.zoomIn.ariaLabel': f.zoomIn,
  'controls.zoomOut.ariaLabel': f.zoomOut,
  'controls.fitView.ariaLabel': f.fitView,
  'controls.interactive.ariaLabel': f.interactive,
  'minimap.ariaLabel': f.minimap,
  'handle.ariaLabel': f.handle,
}

const FIT = { padding: 0.08, maxZoom: 1.1 } as const

/**
 * 画布的大小一变就重新缩放到刚好看全。
 * 不只是为了窗口缩放：窗口窄时关系图在另一个标签页里，画布挂载的那一刻是藏着的、大小是 0，
 * 那时算出来的缩放是错的，要等它真的显示出来再算一次。
 */
function FitOnResize() {
  const { fitView } = useReactFlow()
  const container = useStore((state) => state.domNode)

  useEffect(() => {
    if (container === null) return undefined
    let frame = 0
    const observer = new ResizeObserver(() => {
      cancelAnimationFrame(frame)
      frame = requestAnimationFrame(() => void fitView(FIT))
    })
    observer.observe(container)
    return () => {
      observer.disconnect()
      cancelAnimationFrame(frame)
    }
  }, [container, fitView])

  return null
}

/**
 * 两种关系图共用的画布设置：只看不连线；滚轮滚动的是页面而不是缩放画布（缩放用角上的按钮）；
 * 点和线都能用键盘聚焦。
 */
export function GraphCanvas<N extends Node, E extends Edge>(props: ReactFlowProps<N, E> & { label: string }) {
  const { label, children, ...rest } = props
  return (
    <ReactFlow<N, E>
      aria-label={label}
      ariaLabelConfig={ARIA}
      fitView
      fitViewOptions={FIT}
      minZoom={0.35}
      maxZoom={1.5}
      nodesConnectable={false}
      zoomOnScroll={false}
      zoomOnDoubleClick={false}
      preventScrolling={false}
      deleteKeyCode={null}
      {...rest}
    >
      {/* 左上角：标签沿一个圆排开，四个角是空的；左下角留给"未打标签"的那个点，右下角有画图库的署名。 */}
      <Controls showInteractive={false} position="top-left" />
      <FitOnResize />
      {children}
    </ReactFlow>
  )
}
