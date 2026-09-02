export interface TerminalHistoryCursor {
  sequence: number
  lineIndex: number
}

export interface TerminalHistoryLine {
  sequence: number
  cursor: TerminalHistoryCursor
  text: string
}

export interface TerminalHistoryGap {
  segmentIndex: number
  code: 'CORRUPT_SEGMENT' | 'MISSING_SEGMENT'
  message: string
}

export interface TerminalHistoryPage {
  lines: TerminalHistoryLine[]
  gaps: TerminalHistoryGap[]
  hasMore: boolean
}

export interface TerminalHistorySearchOptions {
  caseSensitive: boolean
  regex: boolean
  wholeWord: boolean
}

export interface TerminalHistorySearchResult {
  matches: TerminalHistoryLine[]
  gaps: TerminalHistoryGap[]
  hasMore: boolean
}
