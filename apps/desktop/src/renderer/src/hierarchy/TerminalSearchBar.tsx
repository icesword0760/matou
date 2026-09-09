import { useEffect, useRef, useState } from 'react'

import { useMessages } from '../i18n/LocaleProvider'

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
  const m = useMessages().hierarchyTerminal.searchBar
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
    <input ref={inputRef} aria-label={m.search} placeholder={m.search}
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
    <button className={options.caseSensitive ? 'is-active' : ''} title={m.matchCase} aria-label={m.matchCase}
      onClick={() => changeOption('caseSensitive')}>Aa</button>
    <button className={options.regex ? 'is-active' : ''} title={m.regex} aria-label={m.regex}
      onClick={() => changeOption('regex')}>.*</button>
    <button className={options.wholeWord ? 'is-active' : ''} title={m.wholeWord} aria-label={m.wholeWord}
      onClick={() => changeOption('wholeWord')}>ab</button>
    <button title={m.previousHint} aria-label={m.previous} onClick={onPrevious}>↑</button>
    <button title={m.nextHint} aria-label={m.next} onClick={onNext}>↓</button>
    <button title={m.closeHint} aria-label={m.close} onClick={onClose}>×</button>
  </div>
}
