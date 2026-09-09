import type { CatalogShape } from '../catalog'

export const dagZhCN = {
  // DagWindowApp
  window: {
    title: '会话 DAG',
    stale: '会话信息暂时未更新',
    loading: '正在载入会话关系…',
    reconnectingHint: '正在重新连接，已有会话与关系不会丢失。',
    retryingHint: '正在重试载入会话关系。',
    retryNow: '立即重试',
    readOnlyBody: '会话关系仍可浏览和选择；画布位置变化仅在本次窗口内保留。',
    reconnectingNotice: '正在重新连接；当前关系图保留，连接恢复后会自动刷新。',
    retryingNotice: '正在重试更新；当前显示上一次成功载入的关系。'
  },

  // DagCanvas
  canvas: {
    label: '会话 DAG 画布',
    zoom: '画布缩放',
    zoomOut: '缩小',
    resetZoom: '恢复 100%',
    zoomIn: '放大',
    focusCurrent: '聚焦当前节点',
    legend: '关系说明',
    legendFork: 'Fork：继承对话',
    legendDerived: '普通关联：不继承对话',
    /** Keyed by the edge's relation kind; shown as an edge tooltip. */
    relationLabel: {
      'forked-from': 'Fork 分支：继承父会话对话上下文',
      'derived-from': '普通父子关联：共享层级，不继承对话上下文'
    }
  },

  // DagNodeCard
  nodeCard: {
    open: (title: string) => `打开会话：${title}`,
    newNotification: (title: string) => `新通知：${title}`,
    /** Keyed by SessionWorkStatus. */
    status: {
      starting: '运行中',
      idle: '空闲',
      running: '运行中',
      'needs-input': '等待输入',
      error: '异常',
      interrupted: '中断',
      exited: '已停止'
    },
    /** Keyed by the session's current mode. */
    mode: {
      shell: 'Shell',
      'claude-code': 'Claude',
      codex: 'Codex',
      'agent-team-member': '队友'
    },
    awaitingOutput: '等待会话输出…',
    childCount: (count: number) => `子会话 ${count}`,
    stoppedSuffix: ' · 已停止',
    activitySequence: (sequence: number) => `活动记录 #${sequence}`,
    lastActivity: (time: string) => `最近活动 ${time}`,
    sharedWorktree: '共享工作树',
    sharedDirectory: '共享目录'
  },

  // DagAggregateCard
  aggregateCard: {
    expand: (summary: string) => `展开远层会话：${summary}`,
    summary: (sessionCount: number, running: number, needsInput: number, error: number) =>
      `共 ${sessionCount} 个会话，运行中 ${running}，等待输入 ${needsInput}，异常 ${error}`,
    /** Keyed by the aggregate kind. */
    eyebrow: { branch: '远层分支', 'layer-overflow': '远层层级' },
    sessionTotal: (count: number) => `共 ${count} 个会话`,
    depthRange: (from: number, to: number) => `第 ${from}–${to} 层 · 点击展开`,
    countRunning: (count: number) => `运行中 ${count}`,
    countNeedsInput: (count: number) => `等待输入 ${count}`,
    countError: (count: number) => `异常 ${count}`
  },

  // DagSearch
  search: {
    label: '搜索会话',
    placeholder: '搜索名称、路径、分支或输出…',
    results: '搜索结果',
    empty: '没有匹配的会话'
  }
}

export const dagEn: CatalogShape<typeof dagZhCN> = {
  window: {
    title: 'Session DAG',
    stale: 'Session information is temporarily out of date',
    loading: 'Loading the session graph…',
    reconnectingHint: 'Reconnecting. Existing sessions and their relationships are kept.',
    retryingHint: 'Retrying to load the session graph.',
    retryNow: 'Retry now',
    readOnlyBody:
      'The session graph can still be browsed and selected. Canvas position changes are kept only inside this window.',
    reconnectingNotice:
      'Reconnecting. The current graph is kept, and it refreshes automatically once the connection is back.',
    retryingNotice: 'Retrying the update. The graph shown is the last one that loaded successfully.'
  },

  canvas: {
    label: 'Session DAG canvas',
    zoom: 'Canvas zoom',
    zoomOut: 'Zoom out',
    resetZoom: 'Reset to 100%',
    zoomIn: 'Zoom in',
    focusCurrent: 'Focus the current node',
    legend: 'Relationship legend',
    legendFork: 'Fork: inherits the conversation',
    legendDerived: 'Link: does not inherit the conversation',
    relationLabel: {
      'forked-from': 'Fork: inherits the parent session conversation context',
      'derived-from': 'Link: shares the level, does not inherit the conversation context'
    }
  },

  nodeCard: {
    open: (title: string) => `Open session: ${title}`,
    newNotification: (title: string) => `New notification: ${title}`,
    status: {
      starting: 'Running',
      idle: 'Idle',
      running: 'Running',
      'needs-input': 'Needs input',
      error: 'Error',
      interrupted: 'Interrupted',
      exited: 'Stopped'
    },
    mode: {
      shell: 'Shell',
      'claude-code': 'Claude',
      codex: 'Codex',
      'agent-team-member': 'Teammate'
    },
    awaitingOutput: 'Waiting for session output…',
    childCount: (count: number) => (count === 1 ? '1 child session' : `${count} child sessions`),
    stoppedSuffix: ' · Stopped',
    activitySequence: (sequence: number) => `Activity record #${sequence}`,
    lastActivity: (time: string) => `Last activity ${time}`,
    sharedWorktree: 'Shared worktree',
    sharedDirectory: 'Shared directory'
  },

  aggregateCard: {
    expand: (summary: string) => `Expand distant sessions: ${summary}`,
    summary: (sessionCount: number, running: number, needsInput: number, error: number) =>
      `${sessionCount === 1 ? '1 session' : `${sessionCount} sessions`} in total, ${running} running, ` +
      `${needsInput === 1 ? '1 needs input' : `${needsInput} need input`}, ` +
      `${error === 1 ? '1 error' : `${error} errors`}`,
    eyebrow: { branch: 'Distant branch', 'layer-overflow': 'Distant level' },
    sessionTotal: (count: number) => (count === 1 ? '1 session in total' : `${count} sessions in total`),
    depthRange: (from: number, to: number) =>
      from === to ? `Level ${from} · click to expand` : `Levels ${from}–${to} · click to expand`,
    countRunning: (count: number) => `${count} running`,
    countNeedsInput: (count: number) => (count === 1 ? '1 needs input' : `${count} need input`),
    countError: (count: number) => (count === 1 ? '1 error' : `${count} errors`)
  },

  search: {
    label: 'Search sessions',
    placeholder: 'Search by name, path, branch or output…',
    results: 'Search results',
    empty: 'No matching sessions'
  }
}
