import { useEffect, useMemo, useRef, useState, type CSSProperties, type PointerEvent, type WheelEvent } from 'react'

import type { SessionGraphNodeView, SessionGraphView } from '../hierarchy/hierarchy-types'
import { DagSearch } from './DagSearch'
import { layoutGraph, visibleLayers } from './dag-layout'

export interface DagTransform { x: number; y: number; scale: number }

export function DagCanvas(props: {
  graph: SessionGraphView
  focusedSessionId: string
  onSelect(sessionId: string): void
  initialTransform?: DagTransform
  onTransformChange?(transform: DagTransform): void
}) {
  const { graph, focusedSessionId, onSelect, initialTransform, onTransformChange } = props
  const layout = useMemo(() => layoutGraph(graph), [graph])
  const viewportRef = useRef<HTMLDivElement>(null)
  const dragRef = useRef<{ id: number; x: number; y: number; originX: number; originY: number } | null>(null)
  const [transform, setTransform] = useState<DagTransform>(initialTransform ?? { x: 70, y: 40, scale: 1 })
  const [previewSessionId, setPreviewSessionId] = useState(focusedSessionId)
  const baseVisibility = visibleLayers(layout, previewSessionId)
  const centerWorldX = ((viewportRef.current?.clientWidth ?? 1000) / 2 - transform.x) / transform.scale
  const centerDepth = Math.max(0, Math.min(layout.depthCount - 1, Math.round((centerWorldX - 50) / 370)))
  const previewDepth = layout.nodes.find(({ sessionId }) => sessionId === previewSessionId)?.depth ?? 0
  const viewportVisibility = Math.abs(centerDepth - previewDepth) > 1
    ? depthsAround(centerDepth, layout.depthCount)
    : baseVisibility.fullDepths
  const fullDepths = new Set(viewportVisibility)
  const viewportWidth = viewportRef.current?.clientWidth || 1000
  const viewportHeight = viewportRef.current?.clientHeight || 700
  const worldBounds = {
    left: -transform.x / transform.scale - 360,
    right: (viewportWidth - transform.x) / transform.scale + 360,
    top: -transform.y / transform.scale - 260,
    bottom: (viewportHeight - transform.y) / transform.scale + 260
  }
  const renderedNodes = layout.nodes.filter(({ depth, x, y, width, height, sessionId }) =>
    fullDepths.has(depth) && (
      sessionId === previewSessionId ||
      (x + width >= worldBounds.left && x <= worldBounds.right &&
        y + height >= worldBounds.top && y <= worldBounds.bottom)
    ))
  const renderedNodeIds = new Set(renderedNodes.map(({ sessionId }) => sessionId))
  const update = (next: DagTransform) => {
    setTransform(next)
    onTransformChange?.(next)
  }
  useEffect(() => {
    if (initialTransform) setTransform(initialTransform)
  }, [initialTransform])
  const focusNode = (sessionId: string, animate = true) => {
    const node = layout.nodes.find((candidate) => candidate.sessionId === sessionId)
    const viewport = viewportRef.current
    if (!node || !viewport) return
    setPreviewSessionId(sessionId)
    const next = {
      ...transform,
      x: viewport.clientWidth / 2 - (node.x + node.width / 2) * transform.scale,
      y: viewport.clientHeight / 2 - (node.y + node.height / 2) * transform.scale
    }
    if (animate && !reducedMotion()) viewport.classList.add('is-animating')
    update(next)
    window.setTimeout(() => viewport.classList.remove('is-animating'), reducedMotion() ? 1 : 240)
  }
  useEffect(() => {
    if (initialTransform) return
    const frame = requestAnimationFrame(() => focusNode(focusedSessionId, false))
    return () => cancelAnimationFrame(frame)
    // Initial focus only; live summary refresh must preserve the user's viewport.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [focusedSessionId, initialTransform])
  useEffect(() => {
    const keyDown = (event: KeyboardEvent) => {
      if (!(event.metaKey || event.ctrlKey)) return
      if (event.key === '+' || event.key === '=') {
        event.preventDefault(); update(zoomAt(transform, transform.scale + .1, center(viewportRef.current)))
      } else if (event.key === '-') {
        event.preventDefault(); update(zoomAt(transform, transform.scale - .1, center(viewportRef.current)))
      } else if (event.key === '0') {
        event.preventDefault(); update({ ...transform, scale: 1 })
      } else if (event.key.toLocaleLowerCase() === 'f') {
        event.preventDefault(); viewportRef.current?.querySelector<HTMLInputElement>('.dag-search input')?.focus()
      }
    }
    window.addEventListener('keydown', keyDown)
    return () => window.removeEventListener('keydown', keyDown)
  }, [transform])

  const wheel = (event: WheelEvent<HTMLDivElement>) => {
    event.preventDefault()
    if (event.ctrlKey || event.metaKey) {
      const rect = event.currentTarget.getBoundingClientRect()
      const point = { x: event.clientX - rect.left, y: event.clientY - rect.top }
      const factor = Math.exp(-event.deltaY * 0.002)
      update(zoomAt(transform, transform.scale * factor, point))
      return
    }
    update({ ...transform, x: transform.x - event.deltaX, y: transform.y - event.deltaY })
  }
  const pointerDown = (event: PointerEvent<HTMLDivElement>) => {
    if (event.button !== 0 || (event.target as HTMLElement).closest('button,input,.dag-node-card')) return
    dragRef.current = { id: event.pointerId, x: event.clientX, y: event.clientY, originX: transform.x, originY: transform.y }
    event.currentTarget.setPointerCapture?.(event.pointerId)
  }
  const pointerMove = (event: PointerEvent<HTMLDivElement>) => {
    const drag = dragRef.current
    if (!drag || drag.id !== event.pointerId) return
    update({ ...transform, x: drag.originX + event.clientX - drag.x, y: drag.originY + event.clientY - drag.y })
  }
  const pointerEnd = (event: PointerEvent<HTMLDivElement>) => {
    if (dragRef.current?.id !== event.pointerId) return
    dragRef.current = null
    event.currentTarget.releasePointerCapture?.(event.pointerId)
  }

  return <div className="dag-canvas" ref={viewportRef} role="application" aria-label="会话 DAG 画布"
    data-scale={round(transform.scale)} data-pan={`${round(transform.x)},${round(transform.y)}`}
    data-rendered-node-count={renderedNodes.length}
    onWheel={wheel} onPointerDown={pointerDown} onPointerMove={pointerMove}
    onPointerUp={pointerEnd} onPointerCancel={pointerEnd}>
    <header className="dag-toolbar">
      <DagSearch nodes={graph.nodes} onPreview={(sessionId) => focusNode(sessionId)} onChoose={onSelect} />
      <div className="dag-toolbar__zoom" aria-label="画布缩放">
        <button aria-label="缩小" onClick={() => update(zoomAt(transform, transform.scale - .1, center(viewportRef.current)))}>−</button>
        <button aria-label="恢复 100%" onClick={() => update({ ...transform, scale: 1 })}>{Math.round(transform.scale * 100)}%</button>
        <button aria-label="放大" onClick={() => update(zoomAt(transform, transform.scale + .1, center(viewportRef.current)))}>＋</button>
        <button aria-label="聚焦当前节点" onClick={() => focusNode(previewSessionId)}>⌖</button>
      </div>
    </header>
    <div className="dag-world" style={{
      width: layout.width, height: layout.height,
      transform: `translate3d(${transform.x}px,${transform.y}px,0) scale(${transform.scale})`
    }}>
      <svg className="dag-edges" width={layout.width} height={layout.height} aria-hidden="true">
        <defs><marker id="dag-arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="5" markerHeight="5" orient="auto-start-reverse">
          <path d="M 0 0 L 10 5 L 0 10 z" className="dag-arrow-head" />
        </marker></defs>
        {layout.nodes.filter(({ depth }) => depth === 0).map((root) =>
          <path key={`root:${root.sessionId}`} className="dag-root-guide"
            d={`M 12 ${layout.height / 2} C 28 ${layout.height / 2}, 32 ${root.y + root.height / 2}, ${root.x} ${root.y + root.height / 2}`} />)}
        {layout.edges.filter((edge) => {
          const fromDepth = layout.nodes.find(({ sessionId }) => sessionId === edge.fromSessionId)?.depth
          const toDepth = layout.nodes.find(({ sessionId }) => sessionId === edge.toSessionId)?.depth
          return fromDepth !== undefined && toDepth !== undefined &&
            (fullDepths.has(fromDepth) || fullDepths.has(toDepth)) &&
            (renderedNodeIds.has(edge.fromSessionId) || renderedNodeIds.has(edge.toSessionId))
        }).map((edge) => {
          const mid = (edge.from.x + edge.to.x) / 2
          return <path key={`${edge.fromSessionId}:${edge.toSessionId}`}
            className={`dag-edge relation-${edge.relationKind}`}
            d={`M ${edge.from.x} ${edge.from.y} C ${mid} ${edge.from.y}, ${mid} ${edge.to.y}, ${edge.to.x} ${edge.to.y}`} />
        })}
      </svg>
      {renderedNodes.map((positioned) =>
        <DagNodeCard key={positioned.sessionId} node={positioned.node}
          focused={positioned.sessionId === previewSessionId}
          style={{ left: positioned.x, top: positioned.y, width: positioned.width, height: positioned.height }}
          onClick={() => {
            setPreviewSessionId(positioned.sessionId)
            onSelect(positioned.sessionId)
          }} />)}
      {Array.from({ length: layout.depthCount }, (_, depth) => depth)
        .filter((depth) => !fullDepths.has(depth)).map((depth) => {
        const count = layout.nodes.filter((node) => node.depth === depth).length
        return <div key={depth} className="dag-ghost-layer" style={{ left: 50 + depth * 370, top: 70 }}>
          <strong>{count} 个会话</strong><span>平移到此处查看第 {depth + 1} 层</span>
        </div>
      })}
    </div>
  </div>
}

function DagNodeCard(props: {
  node: SessionGraphNodeView
  focused: boolean
  style: CSSProperties
  onClick(): void
}) {
  const { node, focused, style, onClick } = props
  const branch = node.worktree?.branch ?? node.git?.branch
  const shared = node.sharedWorkingDirectory === true || node.worktree?.shared === true
  return <button type="button" className={`dag-node-card status-${node.workStatus}${focused ? ' is-focused' : ''}`}
    style={style} aria-label={`打开会话：${node.title}`} onClick={onClick}>
    <span className="dag-node-card__top"><i />{statusLabel(node.workStatus)}<em>{node.currentMode === 'claude-code' ? 'Claude' : 'Shell'}</em></span>
    <strong>{node.title}</strong>
    <span className="dag-node-card__path" title={node.cwd}>
      {branch ? `${branch}${node.git?.dirty ? '*' : ''}` : node.cwd}
    </span>
    <pre>{node.latestLines.slice(-4).join('\n') || '等待会话输出…'}</pre>
    <span className="dag-node-card__meta">子会话 {node.activeChildCount}{node.archivedAt ? ' · 历史节点' : ''} · {activityLabel(node)}</span>
    {shared && <span className="dag-node-card__shared">{branch ? '共享工作树' : '共享目录'}</span>}
  </button>
}

export function clampDagScale(scale: number): number {
  return Math.max(.4, Math.min(2, scale))
}

export function zoomAt(transform: DagTransform, scale: number, point: { x: number; y: number }): DagTransform {
  const nextScale = clampDagScale(scale)
  const worldX = (point.x - transform.x) / transform.scale
  const worldY = (point.y - transform.y) / transform.scale
  return {
    x: point.x - worldX * nextScale,
    y: point.y - worldY * nextScale,
    scale: nextScale
  }
}

function center(element: HTMLElement | null) {
  return { x: (element?.clientWidth ?? 0) / 2, y: (element?.clientHeight ?? 0) / 2 }
}

function statusLabel(status: SessionGraphNodeView['workStatus']) {
  if (status === 'needs-input') return '等待输入'
  if (status === 'running' || status === 'starting') return '运行中'
  if (status === 'error') return '异常'
  if (status === 'interrupted') return '中断'
  if (status === 'exited') return '历史'
  return '空闲'
}

function activityLabel(node: SessionGraphNodeView) {
  if (!node.lastActivityAt) return `活动记录 #${node.lastUserInteractionSeq}`
  const date = new Date(node.lastActivityAt)
  return `最近活动 ${String(date.getHours()).padStart(2, '0')}:${String(date.getMinutes()).padStart(2, '0')}`
}

function reducedMotion() {
  return typeof matchMedia === 'function' && matchMedia('(prefers-reduced-motion: reduce)').matches
}

function round(value: number) { return Math.round(value * 100) / 100 }

function depthsAround(centerDepth: number, depthCount: number): number[] {
  return [centerDepth - 1, centerDepth, centerDepth + 1]
    .filter((depth) => depth >= 0 && depth < depthCount)
}
