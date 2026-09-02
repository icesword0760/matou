import { useEffect, useMemo, useRef, useState, type CSSProperties, type PointerEvent, type WheelEvent } from 'react'

import type { SessionGraphNodeView, SessionGraphView } from '../hierarchy/hierarchy-types'
import { DagSearch } from './DagSearch'
import { layoutGraph, visibleLayers } from './dag-layout'
import { buildDagRenderModel, type DagAggregateItem } from './dag-render-model'

export interface DagTransform { x: number; y: number; scale: number }

export function DagCanvas(props: {
  graph: SessionGraphView
  focusedSessionId: string
  onSelect(sessionId: string): void
  notifiedSessionIds?: string[]
  initialTransform?: DagTransform
  onTransformChange?(transform: DagTransform): void
}) {
  const { graph, focusedSessionId, onSelect, notifiedSessionIds = [], initialTransform, onTransformChange } = props
  const notified = new Set(notifiedSessionIds)
  const layout = useMemo(() => layoutGraph(graph), [graph])
  const viewportRef = useRef<HTMLDivElement>(null)
  const dragRef = useRef<{ id: number; x: number; y: number; originX: number; originY: number } | null>(null)
  const [transform, setTransform] = useState<DagTransform>(initialTransform ?? { x: 70, y: 40, scale: 1 })
  const transformRef = useRef(transform)
  const pendingTransform = useRef<DagTransform | null>(null)
  const transformFrame = useRef<number | null>(null)
  const onTransformChangeRef = useRef(onTransformChange)
  onTransformChangeRef.current = onTransformChange
  const [previewSessionId, setPreviewSessionId] = useState(focusedSessionId)
  const baseVisibility = visibleLayers(layout, previewSessionId)
  const centerWorldX = ((viewportRef.current?.clientWidth ?? 1000) / 2 - transform.x) / transform.scale
  const centerDepth = Math.max(0, Math.min(layout.depthCount - 1, Math.round((centerWorldX - 50) / 370)))
  const previewDepth = layout.nodeById.get(previewSessionId)?.depth ?? 0
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
  const centerWorldY = (viewportHeight / 2 - transform.y) / transform.scale
  const renderModel = buildDagRenderModel({
    layout, fullDepths, worldBounds, centerWorldY, previewSessionId
  })
  const renderedNodes = renderModel.realNodes
  const commitTransform = (next: DagTransform) => {
    transformRef.current = next
    setTransform(next)
    onTransformChangeRef.current?.(next)
  }
  const update = (next: DagTransform) => {
    if (transformFrame.current !== null) {
      cancelAnimationFrame(transformFrame.current)
      transformFrame.current = null
      pendingTransform.current = null
    }
    commitTransform(next)
  }
  const scheduleUpdate = (next: DagTransform) => {
    transformRef.current = next
    pendingTransform.current = next
    if (transformFrame.current !== null) return
    transformFrame.current = requestAnimationFrame(() => {
      transformFrame.current = null
      const pending = pendingTransform.current
      pendingTransform.current = null
      if (pending) commitTransform(pending)
    })
  }
  const flushScheduledUpdate = () => {
    const pending = pendingTransform.current
    if (!pending) return
    if (transformFrame.current !== null) cancelAnimationFrame(transformFrame.current)
    transformFrame.current = null
    pendingTransform.current = null
    commitTransform(pending)
  }
  useEffect(() => {
    if (initialTransform) {
      transformRef.current = initialTransform
      setTransform(initialTransform)
    }
  }, [initialTransform])
  useEffect(() => () => {
    if (transformFrame.current !== null) cancelAnimationFrame(transformFrame.current)
  }, [])
  const focusNode = (sessionId: string, animate = true) => {
    const node = layout.nodeById.get(sessionId)
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
  const restoreViewport = () => {
    // Reset means returning to the stable graph origin, not merely changing
    // scale around an arbitrary pan position. Keeping the top inset also
    // leaves the first visible row clear of the floating toolbar.
    update({ x: 70, y: 40, scale: 1 })
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
        event.preventDefault(); restoreViewport()
      } else if (event.key.toLocaleLowerCase() === 'f') {
        event.preventDefault(); viewportRef.current?.querySelector<HTMLInputElement>('.dag-search input')?.focus()
      }
    }
    window.addEventListener('keydown', keyDown)
    return () => window.removeEventListener('keydown', keyDown)
  }, [transform])

  const wheel = (event: WheelEvent<HTMLDivElement>) => {
    event.preventDefault()
    const current = transformRef.current
    if (event.ctrlKey || event.metaKey) {
      const rect = event.currentTarget.getBoundingClientRect()
      const point = { x: event.clientX - rect.left, y: event.clientY - rect.top }
      const factor = Math.exp(-event.deltaY * 0.002)
      scheduleUpdate(zoomAt(current, current.scale * factor, point))
      return
    }
    scheduleUpdate({ ...current, x: current.x - event.deltaX, y: current.y - event.deltaY })
  }
  const pointerDown = (event: PointerEvent<HTMLDivElement>) => {
    if (event.button !== 0 || (event.target as HTMLElement).closest('button,input,.dag-node-card,.dag-aggregate-card')) return
    const current = transformRef.current
    dragRef.current = { id: event.pointerId, x: event.clientX, y: event.clientY, originX: current.x, originY: current.y }
    event.currentTarget.setPointerCapture?.(event.pointerId)
  }
  const pointerMove = (event: PointerEvent<HTMLDivElement>) => {
    const drag = dragRef.current
    if (!drag || drag.id !== event.pointerId) return
    scheduleUpdate({
      ...transformRef.current,
      x: drag.originX + event.clientX - drag.x,
      y: drag.originY + event.clientY - drag.y
    })
  }
  const pointerEnd = (event: PointerEvent<HTMLDivElement>) => {
    if (dragRef.current?.id !== event.pointerId) return
    dragRef.current = null
    flushScheduledUpdate()
    event.currentTarget.releasePointerCapture?.(event.pointerId)
  }

  return <div className="dag-canvas" ref={viewportRef} role="application" aria-label="会话 DAG 画布"
    data-scale={round(transform.scale)} data-pan={`${round(transform.x)},${round(transform.y)}`}
    data-rendered-node-count={renderedNodes.length}
    data-rendered-aggregate-count={renderModel.aggregates.length}
    onWheel={wheel} onPointerDown={pointerDown} onPointerMove={pointerMove}
    onPointerUp={pointerEnd} onPointerCancel={pointerEnd}>
    <header className="dag-toolbar">
      <DagSearch nodes={graph.nodes} onPreview={(sessionId) => focusNode(sessionId)} onChoose={onSelect} />
      <div className="dag-toolbar__zoom" aria-label="画布缩放">
        <button aria-label="缩小" onClick={() => update(zoomAt(transform, transform.scale - .1, center(viewportRef.current)))}>−</button>
        <button aria-label="恢复 100%" onClick={restoreViewport}>{Math.round(transform.scale * 100)}%</button>
        <button aria-label="放大" onClick={() => update(zoomAt(transform, transform.scale + .1, center(viewportRef.current)))}>＋</button>
        <button aria-label="聚焦当前节点" onClick={() => focusNode(previewSessionId)}>⌖</button>
      </div>
      <div className="dag-relation-legend" aria-label="关系说明">
        <span className="fork">Fork：继承对话</span>
        <span className="derived">普通关联：不继承对话</span>
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
        {(layout.nodesByDepth.get(0) ?? []).filter(({ y, height }) =>
          y + height >= worldBounds.top && y <= worldBounds.bottom
        ).map((root) =>
          <path key={`root:${root.sessionId}`} className="dag-root-guide"
            d={`M 12 ${layout.height / 2} C 28 ${layout.height / 2}, 32 ${root.y + root.height / 2}, ${root.x} ${root.y + root.height / 2}`} />)}
        {renderModel.edges.map((edge) => {
          const mid = (edge.from.x + edge.to.x) / 2
          return <path key={`${edge.fromSessionId}:${edge.toSessionId}`}
            className={`dag-edge relation-${edge.relationKind}`}
            data-relation-label={edge.relationKind === 'forked-from'
              ? 'Fork 分支：继承父会话对话上下文'
              : '普通父子关联：共享层级，不继承对话上下文'}
            d={`M ${edge.from.x} ${edge.from.y} C ${mid} ${edge.from.y}, ${mid} ${edge.to.y}, ${edge.to.x} ${edge.to.y}`} />
        })}
      </svg>
      {renderedNodes.map((positioned) =>
        <DagNodeCard key={positioned.sessionId} node={positioned.node}
          focused={positioned.sessionId === previewSessionId}
          notified={notified.has(positioned.sessionId)}
          style={{ left: positioned.x, top: positioned.y, width: positioned.width, height: positioned.height }}
          onClick={() => {
            setPreviewSessionId(positioned.sessionId)
            onSelect(positioned.sessionId)
          }} />)}
      {renderModel.aggregates.map((aggregate) => <DagAggregateCard key={aggregate.key}
        aggregate={aggregate}
        style={{ left: aggregate.x, top: aggregate.y, width: aggregate.width, height: aggregate.height }}
        onClick={() => focusNode(aggregate.targetSessionId)} />)}
    </div>
  </div>
}

function DagNodeCard(props: {
  node: SessionGraphNodeView
  focused: boolean
  notified: boolean
  style: CSSProperties
  onClick(): void
}) {
  const { node, focused, notified, style, onClick } = props
  const branch = node.worktree?.branch ?? node.git?.branch
  const shared = node.sharedWorkingDirectory === true || node.worktree?.shared === true
  const legacyStopped = node.archivedAt !== undefined
  const visualStatus = legacyStopped ? 'exited' : node.workStatus
  return <button type="button" className={`dag-node-card status-${visualStatus}${legacyStopped ? ' is-stopped' : ''}${focused ? ' is-focused' : ''}${notified ? ' has-notification' : ''}`}
    style={style} data-session-id={node.sessionId} aria-label={`打开会话：${node.title}`} onClick={onClick}>
    <span className="dag-node-card__top"><i />{statusLabel(visualStatus)}<em>{modeLabel(node.currentMode)}</em></span>
    <strong>{node.title}</strong>
    <span className="dag-node-card__path" title={node.cwd}>
      {branch ? `${branch}${node.git?.dirty ? '*' : ''}` : node.cwd}
    </span>
    {branch && <span className="dag-node-card__cwd" title={node.cwd}>{compactPath(node.cwd)}</span>}
    <pre>{node.latestLines.slice(-4).join('\n') || '等待会话输出…'}</pre>
    <span className="dag-node-card__meta">子会话 {node.activeChildCount + node.stoppedChildCount}{legacyStopped ? ' · 已停止' : ''} · {activityLabel(node)}</span>
    {shared && <span className="dag-node-card__shared">{branch ? '共享工作树' : '共享目录'}</span>}
  </button>
}

function DagAggregateCard(props: {
  aggregate: DagAggregateItem
  style: CSSProperties
  onClick(): void
}) {
  const { aggregate, style, onClick } = props
  const { counts } = aggregate
  const label = `共 ${aggregate.sessionCount} 个会话，运行中 ${counts.running}，等待输入 ${counts.needsInput}，异常 ${counts.error}`
  return <button type="button"
    className={`dag-aggregate-card${counts.running > 0 ? ' has-running' : ''}${counts.needsInput > 0 ? ' has-needs-input' : ''}${counts.error > 0 ? ' has-error' : ''}`}
    style={style}
    data-aggregate-key={aggregate.key}
    data-aggregate-kind={aggregate.kind}
    data-aggregate-count={aggregate.sessionCount}
    data-direction={aggregate.direction}
    aria-label={`展开远层会话：${label}`}
    onClick={onClick}>
    <span className="dag-aggregate-card__eyebrow">{aggregate.kind === 'branch' ? '远层分支' : '远层层级'}</span>
    <strong>共 {aggregate.sessionCount} 个会话</strong>
    <span className="dag-aggregate-card__range">第 {aggregate.minimumDepth + 1}–{aggregate.maximumDepth + 1} 层 · 点击展开</span>
    <span className="dag-aggregate-card__counts">
      <i className="running" />运行中 {counts.running}
      <i className="needs-input" />等待输入 {counts.needsInput}
      <i className="error" />异常 {counts.error}
    </span>
  </button>
}

function modeLabel(mode: SessionGraphNodeView['currentMode']): string {
  if (mode === 'claude-code') return 'Claude'
  if (mode === 'agent-team-member') return '队友'
  if (mode === 'codex') return 'Codex'
  return 'Shell'
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
  if (status === 'exited') return '已停止'
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

function compactPath(value: string): string {
  if (value.length <= 42) return value
  const parts = value.split('/').filter(Boolean)
  return parts.length > 1 ? `…/${parts.slice(-2).join('/')}` : value
}

function depthsAround(centerDepth: number, depthCount: number): number[] {
  return [centerDepth - 1, centerDepth, centerDepth + 1]
    .filter((depth) => depth >= 0 && depth < depthCount)
}
