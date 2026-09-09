import type { CatalogShape } from '../catalog'

export const hierarchyTerminalZhCN = {
  /** Resolved from `TerminalLoadingPresentation.labelKey`, which stays locale free. */
  loading: { loading: '加载中', recovering: '恢复中' },
  pane: {
    newNotification: '新通知',
    sharedWorktree: '共享工作树',
    sharedDirectory: '共享目录',
    loadClaudeSessionInto: (title: string) => `载入 Claude Code 会话到“${title}”`,
    loadClaudeSession: '载入 Claude Code 会话',
    forkChildFrom: (title: string) => `从“${title}”创建子分支`,
    forkChild: '创建子分支',
    forkSiblingFrom: (title: string) => `从共同父会话创建“${title}”的兄弟分支`,
    forkSibling: '从共同父会话 Fork 兄弟分支',
    removeNodeOf: (title: string) => `移出节点：${title}`,
    removeNode: '移出节点',
    forkReadyAfterReply: '当前回复完成后即可 Fork',
    forkNeedsReply: '在当前会话输入一次，并等待 Claude Code 完成回复后，即可创建分支',
    forkReadinessHint: '创建子分支条件说明',
    recoveryBlockedReason: '当前终端仍在恢复',
    retryFork: '重试创建分支',
    removeNodeAction: '移除节点…',
    parentConversationExpired: '原 Claude Code 对话已失效',
    claudeRestoreFailed: 'Claude Code 恢复失败',
    switchedToShell: '当前已切换到 Shell，可继续使用终端',
    restoring: '正在恢复…',
    retryRestore: '重试恢复',
    startFreshClaude: '新开 Claude Code',
    restoringClaudeSession: '正在恢复 Claude Code 会话…',
    sessionStartFailed: '会话启动失败',
    terminalProcessFailed: '终端进程未能启动',
    retryStart: '重试启动',
    removeFailedSession: '移除失败会话',
    recoveryFailedFor: (title: string) => `终端恢复失败：${title}`,
    recoveryFailed: '终端恢复失败',
    recoveryFailedBody: '本会话恢复未完成，其他会话仍可继续使用。',
    retryRecoveryFor: (title: string) => `重试恢复终端：${title}`,
    rename: '重命名…',
    restoreAutoTitle: '恢复 Claude 自动标题',
    forkPeer: '创建 Fork 会话',
    forkPeerAction: '⑂ Fork 会话',
    detach: '↗ 独立窗口',
    renameSessionTitle: '重命名会话',
    sessionName: '会话名称',
    sessionNameEmpty: '会话名称不能为空',
    renameFailed: '重命名失败，请稍后重试',
    environmentActionIncomplete: '运行环境操作未完成',
    environmentActionFailed: '运行环境操作失败'
  },
  environmentOverlay: {
    label: (title: string) => `运行环境${title}`,
    title: {
      recovering: '正在恢复运行环境',
      handoff: '正在交接运行环境',
      missing: 'Worktree 需要恢复',
      fallback: '运行环境需要处理'
    },
    description: {
      recovering: '会话历史仍然保留，恢复完成后将自动重新进入终端。',
      handoff: '正在停止旧进程并进入目标目录，请稍候。',
      worktree: '会话和历史仍然保留。恢复、定位原 Worktree，或交接到 Local 后可继续输入。',
      local: '会话和历史仍然保留。请先交接到可用环境后继续输入。'
    },
    restoringWorktree: '正在恢复原 Worktree…',
    restoreWorktree: '恢复 Worktree',
    locatingWorktree: '正在定位 Worktree…',
    locateDirectory: '定位目录',
    handingOffToLocal: '正在交接到 Local…',
    handoffToLocal: '交接到 Local',
    handingOffToWorktree: '正在交接到 Worktree…',
    handoffToWorktree: '交接到 Worktree'
  },
  forkFailure: {
    parentExpiredTitle: '父会话已失效',
    parentExpiredReason:
      '原 Claude Code 对话身份已失效，本次分支没有创建成功。请返回父会话继续，或移除此失败节点后新建空会话。',
    title: '分支创建失败'
  },
  git: {
    unavailable: 'Git 不可用',
    notARepository: '当前目录不是可用的 Git 工作区',
    branch: (branch: string, dirty: boolean) => `Git 分支 ${branch}${dirty ? '，有未提交修改' : ''}`,
    detachedHead: (head: string, dirty: boolean) => `Git detached HEAD ${head}${dirty ? '，有未提交修改' : ''}`
  },
  searchBar: {
    search: '搜索当前 Tab 的终端内容',
    matchCase: '大小写敏感',
    regex: '正则表达式',
    wholeWord: '全词匹配',
    previousHint: '上一个匹配项 (Shift+Enter)',
    previous: '上一个匹配项',
    nextHint: '下一个匹配项 (Enter)',
    next: '下一个匹配项',
    closeHint: '关闭 (Esc)',
    close: '关闭搜索'
  },
  storageFault: {
    label: (title: string) => `终端记录写入异常：${title}`,
    title: '终端已暂停：输出记录写入失败',
    quotaExceeded: '磁盘空间或存储配额不足',
    readOnly: '终端记录目录当前只读或没有写入权限',
    unavailable: '存储设备暂时不可用',
    retained: (size: string) => `已暂存 ${size} 输出，其他会话不受影响`,
    verifying: '正在验证存储并补写输出…',
    ending: '正在安全结束会话…',
    retryWrite: '重试写入',
    endSession: '结束会话'
  },
  recoveryWater: {
    status: { loading: '正在加载终端', recovery: '正在恢复终端' },
    detail: {
      loading: '正在加载终端内容与运行状态',
      recovery: '正在恢复最近的终端内容与运行状态'
    },
    label: (status: string, title: string) => `${status}：${title}`
  },
  teamMember: {
    summary: '队友会话摘要',
    heading: 'Claude Code 队友',
    waiting: '等待队友更新…',
    hint: '队友会话由 Claude Code 团队管理，在此查看状态与最新摘要。',
    workStatus: {
      starting: '运行中',
      idle: '空闲',
      running: '运行中',
      'needs-input': '待输入',
      error: '异常',
      interrupted: '异常',
      exited: '已结束'
    }
  },
  detachedPlaceholder: {
    detached: '已脱出',
    body: '终端正在独立窗口中运行，进程与会话保持不变。',
    returnToTab: '归还到当前页签'
  },
  detachedWindow: {
    defaultTitle: '独立终端',
    teamMember: '独立窗口 · 队友摘要',
    readOnly: '独立窗口 · 只读历史',
    running: '独立窗口 · 会话保持运行'
  },
  closeFlow: {
    taskDeleteBody: (taskName: string) =>
      `删除 "${taskName}" 会丢失该事项下所有终端会话，但不会删除本地目录。 是否继续？`,
    closeCanvas: '关闭画布',
    runningSessions: (count: number) => `${count} 个运行中会话`,
    needsInputSessions: (count: number) => `${count} 个待输入会话`,
    /** Joins the two affected-session counts inside `activity`. */
    affectedJoin: '和 ',
    /** Ends with its own sentence separator so the body can concatenate it directly. */
    activity: (affected: string) => `其中 ${affected}将停止。`,
    sceneCloseBody: (sceneName: string, sessionCount: number, activity: string) =>
      `关闭后，“${sceneName}”下的 ${sessionCount} 个会话会全部从界面移除。${activity}项目文件和工作树保持原样。`,
    stopSession: '停止会话',
    running: '正在运行',
    idle: '当前空闲',
    childSessions: (count: number) => `，并有 ${count} 个子会话`,
    currentSession: '当前会话',
    sessionDeleteBody: (title: string, activity: string, descendants: string) =>
      `“${title}”${activity}${descendants}。停止后，该节点会在会话列表和 DAG 中保持为“已停止”，子会话继续工作。`
  }
}

export const hierarchyTerminalEn: CatalogShape<typeof hierarchyTerminalZhCN> = {
  loading: { loading: 'Loading', recovering: 'Recovering' },
  pane: {
    newNotification: 'New notification',
    sharedWorktree: 'Shared worktree',
    sharedDirectory: 'Shared directory',
    loadClaudeSessionInto: (title: string) => `Load a Claude Code session into "${title}"`,
    loadClaudeSession: 'Load a Claude Code session',
    forkChildFrom: (title: string) => `Fork a child session from "${title}"`,
    forkChild: 'Fork a child session',
    forkSiblingFrom: (title: string) => `Fork a sibling of "${title}" from the shared parent session`,
    forkSibling: 'Fork a sibling from the shared parent session',
    removeNodeOf: (title: string) => `Remove node: ${title}`,
    removeNode: 'Remove node',
    forkReadyAfterReply: 'You can fork once the current reply finishes',
    forkNeedsReply:
      'Send one message in this session and wait for Claude Code to finish replying, then you can fork',
    forkReadinessHint: 'When a child session can be forked',
    recoveryBlockedReason: 'This terminal is still recovering.',
    retryFork: 'Retry fork',
    removeNodeAction: 'Remove node…',
    parentConversationExpired: 'The original Claude Code conversation expired',
    claudeRestoreFailed: 'Claude Code recovery failed',
    switchedToShell: 'Switched to a shell; the terminal still works',
    restoring: 'Recovering…',
    retryRestore: 'Retry recovery',
    startFreshClaude: 'New Claude Code session',
    restoringClaudeSession: 'Recovering the Claude Code session…',
    sessionStartFailed: 'Session failed to start',
    terminalProcessFailed: 'The terminal process could not start',
    retryStart: 'Retry start',
    removeFailedSession: 'Remove failed session',
    recoveryFailedFor: (title: string) => `Terminal recovery failed: ${title}`,
    recoveryFailed: 'Terminal recovery failed',
    recoveryFailedBody: 'This session did not finish recovering. Other sessions still work.',
    retryRecoveryFor: (title: string) => `Retry terminal recovery: ${title}`,
    rename: 'Rename…',
    restoreAutoTitle: 'Restore the Claude auto title',
    forkPeer: 'Fork this session',
    forkPeerAction: '⑂ Fork session',
    detach: '↗ Detached window',
    renameSessionTitle: 'Rename session',
    sessionName: 'Session name',
    sessionNameEmpty: 'The session name cannot be empty',
    renameFailed: 'Rename failed. Try again shortly.',
    environmentActionIncomplete: 'The environment action did not finish',
    environmentActionFailed: 'The environment action failed'
  },
  environmentOverlay: {
    label: (title: string) => `Environment: ${title}`,
    title: {
      recovering: 'Recovering the environment',
      handoff: 'Handing off the environment',
      missing: 'The worktree needs recovery',
      fallback: 'The environment needs attention'
    },
    description: {
      recovering: 'The session history is kept. The terminal reopens automatically once recovery finishes.',
      handoff: 'Stopping the old process and moving into the target directory. This takes a moment.',
      worktree:
        'The session and its history are kept. Restore or locate the original worktree, or hand off to local, to keep typing.',
      local: 'The session and its history are kept. Hand off to an available environment to keep typing.'
    },
    restoringWorktree: 'Restoring the original worktree…',
    restoreWorktree: 'Restore worktree',
    locatingWorktree: 'Locating the worktree…',
    locateDirectory: 'Locate directory',
    handingOffToLocal: 'Handing off to local…',
    handoffToLocal: 'Hand off to local',
    handingOffToWorktree: 'Handing off to the worktree…',
    handoffToWorktree: 'Hand off to worktree'
  },
  forkFailure: {
    parentExpiredTitle: 'The parent session expired',
    parentExpiredReason:
      'The original Claude Code conversation identity expired, so this fork was not created. Go back to the parent session, or remove this failed node and create an empty session.',
    title: 'Fork failed'
  },
  git: {
    unavailable: 'Git unavailable',
    notARepository: 'This directory is not a Git repository',
    branch: (branch: string, dirty: boolean) =>
      `Git branch ${branch}${dirty ? ', uncommitted changes' : ''}`,
    detachedHead: (head: string, dirty: boolean) =>
      `Git detached HEAD ${head}${dirty ? ', uncommitted changes' : ''}`
  },
  searchBar: {
    search: 'Search terminal',
    matchCase: 'Match case',
    regex: 'Regular expression',
    wholeWord: 'Match whole word',
    previousHint: 'Previous match (Shift+Enter)',
    previous: 'Previous match',
    nextHint: 'Next match (Enter)',
    next: 'Next match',
    closeHint: 'Close (Esc)',
    close: 'Close search'
  },
  storageFault: {
    label: (title: string) => `Terminal log write failed: ${title}`,
    title: 'Terminal paused: writing the output log failed',
    quotaExceeded: 'Not enough disk space or storage quota',
    readOnly: 'The terminal log directory is read-only or not writable',
    unavailable: 'The storage device is temporarily unavailable',
    retained: (size: string) => `${size} of output is buffered. Other sessions are unaffected.`,
    verifying: 'Verifying storage and writing the buffered output…',
    ending: 'Ending the session safely…',
    retryWrite: 'Retry write',
    endSession: 'End session'
  },
  recoveryWater: {
    status: { loading: 'Loading the terminal', recovery: 'Recovering the terminal' },
    detail: {
      loading: 'Loading the terminal content and run state',
      recovery: 'Recovering the most recent terminal content and run state'
    },
    label: (status: string, title: string) => `${status}: ${title}`
  },
  teamMember: {
    summary: 'Teammate session summary',
    heading: 'Claude Code teammate',
    waiting: 'Waiting for the teammate to update…',
    hint: 'The Claude Code team manages this teammate session. Its status and latest summary appear here.',
    workStatus: {
      starting: 'Running',
      idle: 'Idle',
      running: 'Running',
      'needs-input': 'Needs input',
      error: 'Error',
      interrupted: 'Error',
      exited: 'Ended'
    }
  },
  detachedPlaceholder: {
    detached: 'Detached',
    body: 'The terminal is running in a separate window. Its process and session are unchanged.',
    returnToTab: 'Return to this tab'
  },
  detachedWindow: {
    defaultTitle: 'Detached terminal',
    teamMember: 'Detached window · Teammate summary',
    readOnly: 'Detached window · Read-only history',
    running: 'Detached window · Session keeps running'
  },
  closeFlow: {
    taskDeleteBody: (taskName: string) =>
      `Deleting "${taskName}" discards every terminal session in this task, but does not delete the local directory. Continue?`,
    closeCanvas: 'Close canvas',
    runningSessions: (count: number) => count === 1 ? '1 running session' : `${count} running sessions`,
    needsInputSessions: (count: number) =>
      count === 1 ? '1 session waiting for input' : `${count} sessions waiting for input`,
    affectedJoin: ' and ',
    activity: (affected: string) => `${affected} will stop. `,
    sceneCloseBody: (sceneName: string, sessionCount: number, activity: string) =>
      `Closing it removes ${sessionCount === 1 ? '1 session' : `all ${sessionCount} sessions`} in "${sceneName}" from the view. ${activity}Project files and worktrees stay as they are.`,
    stopSession: 'Stop session',
    running: 'is running',
    idle: 'is idle',
    childSessions: (count: number) =>
      count === 1 ? ' and has 1 child session' : ` and has ${count} child sessions`,
    currentSession: 'This session',
    sessionDeleteBody: (title: string, activity: string, descendants: string) =>
      `"${title}" ${activity}${descendants}. After it stops, the node stays "Stopped" in the session list and the DAG, and its child sessions keep working.`
  }
}
