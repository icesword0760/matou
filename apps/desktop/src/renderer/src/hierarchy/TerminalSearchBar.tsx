import { useEffect, useRef, useState } from 'react'

export interface TerminalSearchOptions {
  caseSensitive: boolean
  regex: boolean
  wholeWord: boolean
}

export function TerminalSearchBar({ open, themeKey, resultIndex, resultCount, onSearch, onNext, onPrevious, onClose }: {
  open: boolean
  themeKey: 'dark' | 'light'
  resultIndex: number
  resultCount: number
  onSearch(query: string, options: TerminalSearchOptions): void
  onNext(): void
  onPrevious(): void
  onClose(): void
}) {
  const inputRef = useRef<HTMLInputElement>(null)
  const [query, setQuery] = useState('')
  const [options, setOptions] = useState<TerminalSearchOptions>({ caseSensitive: false, regex: false, wholeWord: false })
  useEffect(() => { if (open) requestAnimationFrame(() => inputRef.current?.focus()) }, [open])
  if (!open) return null
  const changeOption = (key: keyof TerminalSearchOptions) => {
    const next = { ...options, [key]: !options[key] }
    setOptions(next)
    onSearch(query, next)
  }
  const isMac = /Mac/.test(navigator.platform ?? '') || /Mac/.test(navigator.userAgent ?? '')
  return <div className={`terminal-search-bar theme-${themeKey}`} role="search">
    <input ref={inputRef} aria-label="搜索当前 Tab 的终端内容" placeholder="搜索当前 Tab 的终端内容"
      value={query} onChange={(event) => { setQuery(event.target.value); onSearch(event.target.value, options) }}
      onKeyDown={(event) => {
        if (event.key === 'Escape') {
          event.preventDefault()
          event.stopPropagation()
          onClose()
          return
        }
        const optionModifier = isMac ? event.metaKey : event.altKey
        if (optionModifier && event.key === 'c') {
          event.preventDefault()
          event.stopPropagation()
          changeOption('caseSensitive')
          return
        }
        if (optionModifier && event.key === 'r') {
          event.preventDefault()
          event.stopPropagation()
          changeOption('regex')
          return
        }
        if (event.key === 'Enter') {
          event.preventDefault()
          event.shiftKey ? onPrevious() : onNext()
        }
      }} />
    <span className="terminal-search-bar__count">{query ? (resultCount ? `${resultIndex + 1}/${resultCount}` : '0/0') : ''}</span>
    <button className={options.caseSensitive ? 'is-active' : ''} title="大小写敏感" aria-label="大小写敏感"
      onClick={() => changeOption('caseSensitive')}>Aa</button>
    <button className={options.regex ? 'is-active' : ''} title="正则表达式" aria-label="正则表达式"
      onClick={() => changeOption('regex')}>.*</button>
    <button className={options.wholeWord ? 'is-active' : ''} title="全词匹配" aria-label="全词匹配"
      onClick={() => changeOption('wholeWord')}>ab</button>
    <button title="上一个匹配项 (Shift+Enter)" aria-label="上一个匹配项" onClick={onPrevious}>↑</button>
    <button title="下一个匹配项 (Enter)" aria-label="下一个匹配项" onClick={onNext}>↓</button>
    <button title="关闭 (Esc)" aria-label="关闭搜索" onClick={onClose}>×</button>
  </div>
}
