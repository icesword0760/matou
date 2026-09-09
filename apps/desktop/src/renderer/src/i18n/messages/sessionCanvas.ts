import type { CatalogShape } from '../catalog'
import { hudEn } from './hud'

export const sessionCanvasZhCN = {
  // SessionCanvas
  canvas: '会话画布',
  emptyCanvas: '当前画布没有活跃会话',

  // SessionCard
  sessionCard: (title: string) => `会话：${title}`,

  // SessionCarousel
  siblingSessions: '同级会话列表',
  claudeRestoreFailed: 'Claude 恢复失败',
  childSessionCount: (count: number) => `子会话 ${count}`,
  /** Keyed by SessionWorkStatus. */
  compactStatus: {
    starting: '运行中',
    idle: '空闲',
    running: '运行中',
    'needs-input': '等待输入',
    error: '异常',
    interrupted: '中断',
    exited: '已停止'
  },

  // SessionBreadcrumb
  sessionLevel: '会话层级',
  childSessionsOf: (title: string) => `${title} 的子会话`,
  rootSessions: '根会话',
  sessionCount: (count: number) => `${count} 个会话`,
  returnToParent: '返回父会话',

  // ParentProjection
  /** Keyed by SessionWorkStatus. */
  parentStatus: {
    starting: '运行中',
    idle: '空闲',
    running: '运行中',
    'needs-input': '等待输入',
    error: '异常',
    interrupted: '已中断',
    exited: '空闲'
  },
  parentPullReady: '已到达 · 松手返回父会话',
  parentPullHint: '右拉至目标，松手返回父会话',
  parentPullProgress: '返回父会话进度',

  // ChildSessionBadge
  childBadge: {
    viewChildren: (count: number) => `查看 ${count} 个子会话`,
    branches: (count: number) => `${count} 分支`,
    summaryError: (count: number) => `${count} 异常`,
    summaryNeedsInput: (count: number) => `${count} 待输入`,
    summaryRunning: (count: number) => `${count} 运行中`,
    summaryStarting: (count: number) => `${count} 准备中`,
    detailRunning: (count: number) => `运行中 ${count}`,
    detailStarting: (count: number) => `准备中 ${count}`,
    detailNeedsInput: (count: number) => `待输入 ${count}`,
    detailError: (count: number) => `错误 ${count}`,
    /** Separates the mode counts from the status counts inside the tooltip. */
    detailJoin: '；'
  },

  // ForkProgressOverlay
  fork: {
    label: (stage: string) => `正在创建分支：${stage}`,
    stageCounter: (current: number, total: number) => `第 ${current} / ${total} 阶段`
  },
  /** Keyed by ForkStage. */
  forkStage: {
    queued: '等待创建分支',
    'creating-worktree': '正在创建独立工作目录',
    'applying-setup': '正在准备分支环境',
    'binding-session': '正在绑定分支会话',
    'restoring-provider': '正在恢复智能体会话',
    'starting-window': '正在打开分支窗口',
    succeeded: '分支已就绪',
    failed: '分支创建失败'
  },
  /** Keyed by ForkStage. */
  forkStageDescription: {
    queued: '前面的分支任务完成后会自动继续',
    'creating-worktree': '正在隔离文件修改，其他会话可继续工作',
    'applying-setup': '正在执行此工作空间需要的准备步骤',
    'binding-session': '正在把新会话连接到独立工作目录',
    'restoring-provider': '正在续接原会话上下文并启动分支任务',
    'starting-window': '分支窗口即将可以输入和查看输出',
    succeeded: '现在可以继续分支任务',
    failed: '请查看失败原因后重试'
  },

  // StoppedSessionCard
  stoppedCard: {
    restoring: '正在恢复会话…',
    removeNodeOf: (title: string) => `移除节点…：${title}`
  },

  // BranchDialog
  branchDialog: {
    /** Keyed by the dialog's relation mode. */
    title: { child: '创建子会话分支', sibling: '创建同级分支', peer: 'Fork 会话' },
    peerDescription: (title: string) => `复制“${title}”的当前对话并加入当前列表`,
    branchDescription: (title: string) => `从“${title}”继续一条独立工作路径`,
    close: '关闭创建分支',
    name: '分支名称',
    namePlaceholder: '例如：修复登录流程',
    nameRequired: '请输入分支名称',
    nameTooLong: '分支名称最多 64 个字符',
    worktreeSection: '工作目录',
    useCurrentWorktree: '使用当前工作树',
    currentWorktreeHint: {
      peer: '和当前会话使用同一目录，适合连续处理同一份改动',
      branch: '和父会话使用同一目录，适合连续处理同一份改动'
    },
    useNewWorktree: '从新工作树创建',
    newWorktreeHint: '创建隔离的 Git worktree，适合多个功能并行开发',
    needsGitRepository: '需要 Git 仓库',
    newWorktreeNotice: '原目录中的未提交修改会保留在原处；新工作树从当前 HEAD 创建。',
    creating: '正在创建分支…',
    create: '创建分支'
  },

  // RemoveNodeDialog
  removeDialog: {
    title: (name: string) => `移除节点“${name}”？`,
    running: (count: number) => `${count} 个运行中`,
    needsInput: (count: number) => `${count} 个待输入`,
    /** Joins the two affected-session counts inside `activity`. */
    activityJoin: '、',
    ownedWorktrees: (count: number) => `${count} 个自有 Worktree`,
    impactSessions: (count: number) => `影响 ${count} 个会话`,
    impactWorktrees: (worktrees: string) => `、${worktrees}`,
    impactActivity: (activity: string) => `；其中 ${activity}`,
    leafBody: (impact: string) => `${impact}。移除后，该会话会从会话列表和 DAG 中消失。`,
    filesUnchanged: '项目文件和自有 Worktree 保持原样。',
    chooseScope: '请选择移除范围。项目文件和自有 Worktree 保持原样。',
    scopeLabel: '移除范围',
    nodeOnly: '仅移除当前节点',
    reparentDescendants: '后代会话将重连到当前节点的父级。',
    descendantsBecomeRoots: '直接后代会话将成为根节点。',
    nodeAndDescendants: '移除当前节点及全部后代',
    descendantsRemoved: (count: number) => `当前节点与 ${count} 个后代会话会全部移除。`,
    activeWarning: (activity: string) => `其中 ${activity}的会话将先停止。`,
    remove: '移除',
    removeNodeOnly: '移除当前节点',
    removeSessions: (count: number) => `移除 ${count} 个会话`
  },

  // SessionLoaderDialog
  loader: {
    loadInto: (title: string) => `载入到“${title}”`,
    close: '关闭会话管理',
    resumableSessions: '可恢复会话',
    filterSessions: '筛选左侧会话',
    filterSessionsPlaceholder: '筛选左侧：标题、路径、模型或会话 ID',
    clearSessionFilter: '清除会话筛选',
    searching: '正在查找…',
    sessionsOf: (shown: number, total: number) => `${shown} / ${total} 个会话`,
    scrollForMore: '滚动加载更多',
    allLoaded: '已加载全部',
    previewSession: (title: string) => `预览会话：${title}`,
    entries: (count: number) => `${count} 条内容`,
    loadedHere: '已载入当前卡片',
    loadedElsewhere: (card: string) => `已载入${card}`,
    /**
     * Wraps a session or card name the user chose. The generic fallbacks below carry their own
     * quotes so English can leave `another card` / `this session` unquoted.
     */
    quoted: (name: string) => `“${name}”`,
    otherCard: '“其他卡片”',
    loadingMore: '正在载入更多会话…',
    noMatchingSessions: '左侧没有匹配的会话',
    noSessions: '当前工作空间内没有 Claude Code 会话',
    preview: '会话预览',
    selectSession: '选择会话查看内容',
    searchContent: '查找右侧会话内容',
    searchContentPlaceholder: '查找右侧内容',
    clearContentSearch: '清除内容查找',
    matchNav: '右侧内容匹配位置',
    previousMatch: '上一个匹配',
    nextMatch: '下一个匹配',
    matches: (count: number) => `全文共 ${count} 处匹配`,
    entriesLoaded: (shown: number, total: number) => `已加载 ${shown} / ${total} 条`,
    loadingEarlier: '正在载入更早内容…',
    tool: '工具',
    you: '你',
    loadingPreview: '正在载入预览…',
    runningWarning: '当前卡片正在运行，继续会结束当前进程。',
    loading: '正在载入…',
    endAndLoad: '结束当前运行并载入',
    loadHere: '载入到当前卡片',
    duplicateTitle: '会话已在当前工作空间载入',
    duplicateBody: (session: string, card: string) =>
      `${session}已载入到${card}。仍然可以载入到当前卡片，两张卡片将关联同一个 Claude Code 会话。`,
    thisSession: '“该会话”',
    loadAnyway: '仍然载入',
    /** Keyed by ClaudeSessionPermissionMode. */
    permission: {
      default: '默认权限',
      auto: '自动模式',
      acceptEdits: '自动接受编辑',
      plan: '计划模式',
      bypassPermissions: '开放所有权限'
    },
    timeUnknown: '时间未知',
    justNow: '刚刚',
    hoursAgo: (hours: number) => `${hours} 小时前`,
    daysAgo: (days: number) => `${days} 天前`
  }
}

export const sessionCanvasEn: CatalogShape<typeof sessionCanvasZhCN> = {
  // SessionCanvas
  canvas: 'Session canvas',
  emptyCanvas: 'No active sessions on this canvas',

  // SessionCard
  sessionCard: (title: string) => `Session: ${title}`,

  // SessionCarousel
  siblingSessions: 'Sibling sessions',
  claudeRestoreFailed: 'Claude recovery failed',
  childSessionCount: (count: number) =>
    count === 1 ? '1 child session' : `${count} child sessions`,
  compactStatus: {
    starting: 'Running',
    idle: 'Idle',
    running: 'Running',
    'needs-input': 'Needs input',
    error: 'Error',
    interrupted: 'Interrupted',
    exited: 'Stopped'
  },

  // SessionBreadcrumb
  sessionLevel: 'Session level',
  childSessionsOf: (title: string) => `Child sessions of ${title}`,
  rootSessions: 'Root sessions',
  sessionCount: (count: number) => (count === 1 ? '1 session' : `${count} sessions`),
  returnToParent: 'Back to the parent session',

  // ParentProjection
  parentStatus: {
    starting: 'Running',
    idle: 'Idle',
    running: 'Running',
    'needs-input': 'Needs input',
    error: 'Error',
    interrupted: 'Interrupted',
    exited: 'Idle'
  },
  parentPullReady: 'Ready · release to return to the parent session',
  parentPullHint: 'Pull right to the target, then release to return to the parent session',
  parentPullProgress: 'Return to parent progress',

  // ChildSessionBadge
  childBadge: {
    viewChildren: (count: number) => (count === 1 ? 'View 1 fork' : `View ${count} forks`),
    branches: (count: number) => (count === 1 ? '1 fork' : `${count} forks`),
    summaryError: (count: number) => (count === 1 ? '1 error' : `${count} errors`),
    summaryNeedsInput: (count: number) =>
      count === 1 ? '1 needs input' : `${count} need input`,
    summaryRunning: (count: number) => `${count} running`,
    summaryStarting: (count: number) => `${count} starting`,
    detailRunning: (count: number) => `${count} running`,
    detailStarting: (count: number) => `${count} starting`,
    detailNeedsInput: (count: number) =>
      count === 1 ? '1 needs input' : `${count} need input`,
    detailError: (count: number) => (count === 1 ? '1 error' : `${count} errors`),
    detailJoin: '; '
  },

  // ForkProgressOverlay
  fork: {
    label: (stage: string) => `Creating the fork: ${stage}`,
    stageCounter: (current: number, total: number) => `Stage ${current} of ${total}`
  },
  forkStage: {
    queued: 'Waiting to fork',
    'creating-worktree': 'Creating an isolated working directory',
    'applying-setup': 'Preparing the fork environment',
    'binding-session': 'Binding the fork session',
    'restoring-provider': 'Restoring the agent session',
    'starting-window': 'Opening the fork window',
    succeeded: 'Fork ready',
    failed: 'Fork failed'
  },
  forkStageDescription: {
    queued: 'This fork continues automatically once the forks ahead of it finish',
    'creating-worktree': 'Isolating the file changes so other sessions can keep working',
    'applying-setup': 'Running the setup steps this workspace needs',
    'binding-session': 'Connecting the new session to its own working directory',
    'restoring-provider': 'Continuing the original conversation and starting the fork',
    'starting-window': 'The fork window is about to accept input and show output',
    succeeded: 'You can continue in the fork now',
    failed: 'Check why it failed, then retry'
  },

  // StoppedSessionCard
  stoppedCard: {
    restoring: 'Recovering the session…',
    removeNodeOf: (title: string) => `Remove node: ${title}`
  },

  // BranchDialog
  branchDialog: {
    title: {
      child: 'Fork a child session',
      sibling: 'Fork a sibling session',
      peer: 'Fork this session'
    },
    peerDescription: (title: string) =>
      `Copy the current conversation from "${title}" into this list`,
    branchDescription: (title: string) => `Continue a separate line of work from "${title}"`,
    close: 'Close the fork dialog',
    name: 'Fork name',
    namePlaceholder: 'e.g. Fix the login flow',
    nameRequired: 'Enter a fork name',
    nameTooLong: 'The fork name can be at most 64 characters',
    worktreeSection: 'Working directory',
    useCurrentWorktree: 'Use the current worktree',
    currentWorktreeHint: {
      peer: 'Same directory as the current session, for continuing one set of changes',
      branch: 'Same directory as the parent session, for continuing one set of changes'
    },
    useNewWorktree: 'Create from a new worktree',
    newWorktreeHint: 'Creates an isolated Git worktree, for building several features in parallel',
    needsGitRepository: 'Requires a Git repository',
    newWorktreeNotice:
      'Uncommitted changes stay in the original directory; the new worktree is created from the current HEAD.',
    creating: 'Creating the fork…',
    create: 'Create fork'
  },

  // RemoveNodeDialog
  removeDialog: {
    title: (name: string) => `Remove node "${name}"?`,
    running: (count: number) => (count === 1 ? '1 running session' : `${count} running sessions`),
    needsInput: (count: number) =>
      count === 1 ? '1 session waiting for input' : `${count} sessions waiting for input`,
    activityJoin: ' and ',
    ownedWorktrees: (count: number) =>
      count === 1 ? '1 owned worktree' : `${count} owned worktrees`,
    impactSessions: (count: number) =>
      count === 1 ? 'Affects 1 session' : `Affects ${count} sessions`,
    impactWorktrees: (worktrees: string) => `, ${worktrees}`,
    impactActivity: (activity: string) => `, including ${activity}`,
    leafBody: (impact: string) =>
      `${impact}. Once removed, the session disappears from the session list and the DAG.`,
    filesUnchanged: 'Project files and owned worktrees stay as they are.',
    chooseScope: 'Choose what to remove. Project files and owned worktrees stay as they are.',
    scopeLabel: 'Removal scope',
    nodeOnly: 'Remove this node only',
    reparentDescendants: 'Descendant sessions reattach to the parent of this node.',
    descendantsBecomeRoots: 'Direct descendant sessions become root nodes.',
    nodeAndDescendants: 'Remove this node and all descendants',
    descendantsRemoved: (count: number) =>
      count === 1
        ? 'This node and its 1 descendant session will be removed.'
        : `This node and its ${count} descendant sessions will be removed.`,
    activeWarning: (activity: string) => `${activity} will stop first.`,
    remove: 'Remove',
    removeNodeOnly: 'Remove this node',
    removeSessions: (count: number) =>
      count === 1 ? 'Remove 1 session' : `Remove ${count} sessions`
  },

  // SessionLoaderDialog
  loader: {
    loadInto: (title: string) => `Load into "${title}"`,
    close: 'Close the session loader',
    resumableSessions: 'Resumable sessions',
    filterSessions: 'Filter sessions',
    filterSessionsPlaceholder: 'Filter by title, path, model or session ID',
    clearSessionFilter: 'Clear the session filter',
    searching: 'Searching…',
    sessionsOf: (shown: number, total: number) =>
      `${shown} / ${total} ${total === 1 ? 'session' : 'sessions'}`,
    scrollForMore: 'Scroll to load more',
    allLoaded: 'All loaded',
    previewSession: (title: string) => `Preview session: ${title}`,
    entries: (count: number) => (count === 1 ? '1 entry' : `${count} entries`),
    loadedHere: 'Loaded in this card',
    loadedElsewhere: (card: string) => `Loaded in ${card}`,
    quoted: (name: string) => `"${name}"`,
    otherCard: 'another card',
    loadingMore: 'Loading more sessions…',
    noMatchingSessions: 'No matching sessions',
    noSessions: 'No Claude Code sessions in this workspace',
    preview: 'Session preview',
    selectSession: 'Select a session to see its content',
    searchContent: 'Search session content',
    searchContentPlaceholder: 'Search content',
    clearContentSearch: 'Clear the content search',
    matchNav: 'Match position',
    previousMatch: 'Previous match',
    nextMatch: 'Next match',
    matches: (count: number) =>
      count === 1 ? '1 match in the whole session' : `${count} matches in the whole session`,
    entriesLoaded: (shown: number, total: number) => `Loaded ${shown} / ${total} entries`,
    loadingEarlier: 'Loading earlier content…',
    tool: 'Tool',
    you: 'You',
    loadingPreview: 'Loading the preview…',
    runningWarning: 'This card is running. Continuing ends the current process.',
    loading: 'Loading…',
    endAndLoad: 'End the current run and load',
    loadHere: 'Load into this card',
    duplicateTitle: 'This session is already loaded in this workspace',
    duplicateBody: (session: string, card: string) =>
      `${session} is already loaded in ${card}. You can still load it into this card, and both cards will then share the same Claude Code session.`,
    thisSession: 'This session',
    loadAnyway: 'Load anyway',
    /** Claude Code's own mode names, shared with the HUD so the two cannot drift. */
    permission: hudEn.permission,
    timeUnknown: 'Time unknown',
    justNow: 'Just now',
    hoursAgo: (hours: number) => (hours === 1 ? '1 hour ago' : `${hours} hours ago`),
    daysAgo: (days: number) => (days === 1 ? '1 day ago' : `${days} days ago`)
  }
}
