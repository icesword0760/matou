import type { CatalogShape } from '../catalog'

export const terminalZhCN = {
  /**
   * Written straight into the xterm buffer when Runtime reports a replay gap,
   * so it has to be resolved at write time rather than at module load.
   */
  historyCorrupted: '部分终端历史损坏，已继续显示实时输出',
  historyTrimmed: '较早的终端历史已清理，已继续显示实时输出',

  /** Shared by the archived-search meta line and the history footer. */
  historyGaps: (count: number) => `${count} 处历史缺口`,

  archivedSearch: {
    label: '归档历史搜索结果',
    source: '归档历史',
    unreadableRange: '该范围存在不可读的历史片段'
  },

  historyContext: {
    region: '终端历史记录',
    title: '历史记录',
    readOnly: '只读',
    returnToLive: '返回实时终端',
    loading: '正在读取历史上下文…',
    moreBefore: '上方还有更早记录',
    moreAfter: '下方还有更新记录',
    contextRange: '命中行前后各最多 250 行',
    escapeHint: 'Esc 返回'
  }
}

export const terminalEn: CatalogShape<typeof terminalZhCN> = {
  historyCorrupted: 'Part of the terminal history is damaged; live output continues below',
  historyTrimmed: 'Older terminal history was cleaned up; live output continues below',

  historyGaps: (count: number) => (count === 1 ? '1 history gap' : `${count} history gaps`),

  archivedSearch: {
    label: 'Archived history search result',
    source: 'Archived history',
    unreadableRange: 'This range contains unreadable history segments'
  },

  historyContext: {
    region: 'Terminal history',
    title: 'History',
    readOnly: 'Read-only',
    returnToLive: 'Back to the live terminal',
    loading: 'Loading the history context…',
    moreBefore: 'Earlier records above',
    moreAfter: 'Newer records below',
    contextRange: 'Up to 250 lines before and after the match',
    escapeHint: 'Esc to return'
  }
}
