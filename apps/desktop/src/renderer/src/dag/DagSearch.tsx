import { useMemo, useState } from 'react'

import type { SessionGraphNodeView, SessionGraphView } from '../hierarchy/hierarchy-types'
import { searchGraph } from './dag-layout'

export function DagSearch(props: {
  nodes: SessionGraphNodeView[]
  onPreview(sessionId: string): void
  onChoose(sessionId: string): void
}) {
  const { nodes, onPreview, onChoose } = props
  const [query, setQuery] = useState('')
  const [selected, setSelected] = useState(0)
  const results = useMemo(() => searchGraph({ sceneId: nodes[0]?.sceneId ?? '', nodes, edges: [] }, query), [nodes, query])
  const choose = (sessionId: string) => {
    onPreview(sessionId)
    requestAnimationFrame(() => onChoose(sessionId))
  }
  return <div className="dag-search">
    <input type="search" role="searchbox" aria-label="搜索会话" placeholder="搜索名称、路径、分支或输出…"
      value={query} onChange={(event) => { setQuery(event.currentTarget.value); setSelected(0) }}
      onKeyDown={(event) => {
        if (event.key === 'ArrowDown' && results.length > 0) {
          event.preventDefault(); setSelected((value) => (value + 1) % results.length)
        } else if (event.key === 'ArrowUp' && results.length > 0) {
          event.preventDefault(); setSelected((value) => (value - 1 + results.length) % results.length)
        } else if (event.key === 'Enter' && results[selected]) {
          event.preventDefault(); choose(results[selected]!.sessionId)
        }
      }} />
    {query && <div className="dag-search__results" role="listbox" aria-label="搜索结果">
      {results.length === 0 && <p>没有匹配的会话</p>}
      {results.slice(0, 12).map((node, index) => <button key={node.sessionId} role="option"
        aria-selected={index === selected} onPointerEnter={() => setSelected(index)}
        onClick={() => choose(node.sessionId)}>
        <strong>{node.title}</strong><span>{node.currentMode === 'claude-code' ? 'Claude Code' : 'Shell'} · {node.cwd}</span>
      </button>)}
    </div>}
  </div>
}
