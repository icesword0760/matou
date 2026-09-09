import type { CatalogShape } from '../catalog'

export const controlZhCN = {
  /** Window titles reported inside Host Control result paths. */
  windowTitle: {
    detachedTerminal: '独立终端窗口',
    main: '主窗口'
  },

  // host-navigation-broker.ts
  navigation: {
    targetNotReady: '目标窗口当前未就绪，请稍后重试',
    requestInProgress: '导航请求正在处理中，请稍后重试',
    sendFailed: '导航请求发送失败，请稍后重试',
    rendererRejected: '目标窗口未完成导航，请重试',
    invalidAck: '目标窗口返回的导航结果无效，请重试',
    timeout: '目标窗口响应超时，请重试',
    closed: '导航服务已停止，请稍后重试'
  },

  // host-control-client.ts
  client: {
    requestTooLarge: 'Host Control 请求超过大小限制',
    mismatchedRequestId: 'Host Control 返回了不匹配的请求标识',
    requestFailed: 'Host Control 请求失败',
    invalidResponse: 'Matou Host Control 返回了无效响应；请重试',
    connectTimeout: '连接 Matou Host Control 超时；请确认 Matou 仍在运行后重试',
    notConnected: '未连接到 Matou Host Control；请在 Matou 托管终端中重试',
    responseTimeout: '等待 Matou Host Control 响应超时；请确认 Matou 仍在运行后重试',
    responseTooLarge: 'Host Control 响应超过大小限制',
    connectionLost: '与 Matou Host Control 的连接已中断；请重试'
  },

  // host-topology-projector.ts
  topology: {
    callerNotInGraph: '当前调用会话已不在 Matou 会话图中',
    sessionNotInGraph: '指定会话已不在 Matou 会话图中',
    refNotInGraph: (ref: string) => `目标 ${ref} 已不在 Matou 会话图中`,
    siblingOutOfRange: (ordinal: number) => `当前层级没有第 ${ordinal} 个会话`,
    noSessionLeft: '当前会话左侧没有同层会话',
    noSessionRight: '当前会话右侧没有同层会话',
    noParent: '当前会话没有父会话',
    parentNotInGraph: '父会话已不在 Matou 会话图中',
    childOutOfRange: (ordinal: number) => `当前会话没有第 ${ordinal} 个子会话`
  },

  // host-action-target-resolver.ts
  target: {
    plainDirectoryOnly: '普通目录只能继续使用当前执行环境',
    worktreeRefInvalid: '提交的 Worktree 引用无效',
    worktreeUnavailable: '指定的 Worktree 已不可用',
    worktreeBranchMismatch: (branch: string) => `Worktree 当前分支与提交的 ${branch} 不一致`,
    branchExists: (branch: string) => `分支 ${branch} 已存在`,
    staleProjection: '目标列表已更新，请重新列举后再执行',
    ambiguous: (ref: string) => `目标 ${ref} 匹配多个层级位置`,
    notFound: (ref: string) => `目标 ${ref} 不存在`,
    callerSessionMissing: '当前调用会话不存在',
    targetSessionMissing: '目标会话不存在',
    sourceContextRefInvalid: '来源执行环境引用无效',
    sourceSessionMissing: '来源会话不存在',
    branchCheckFailed: (branch: string) => `仓库分支状态校验失败: ${branch}`,
    impactTargetMissing: '影响目标已不存在'
  },

  // host-action-confirmation-service.ts (shared with runtime-host-action-facade.ts)
  confirmation: {
    duplicateRef: '生成了重复的确认引用',
    required: '确认已失效，请先重新预览',
    noneForRun: '当前运行没有可用的确认',
    expired: '确认已过期，请重新预览',
    stale: '确认对应的目标或影响已变化，请重新预览',
    actionChanged: '确认对应的操作已变化，请重新预览'
  },

  // provider-ready-registry.ts
  providerReady: {
    waitTimeout: (sessionId: string) => `等待会话 ${sessionId} 的 Provider 就绪超时`,
    waitCancelled: (sessionId: string) => `等待会话 ${sessionId} 就绪已取消`,
    allCancelled: 'Provider 就绪等待已取消'
  },

  // runtime-control-backend.ts
  backend: {
    taskError: '事项出错',
    noInputTerminal: '目标会话当前没有可输入的终端进程'
  },

  // fork-settlement-waiter.ts
  forkSettlement: {
    missing: 'Fork 状态记录不可用',
    timeout: 'Fork 状态确认超时'
  },

  // fork-batch-coordinator.ts
  forkBatch: {
    inputMismatch: (batchKey: string) => `批次 ${batchKey} 与已提交输入不一致`,
    /**
     * Substring shared with the facade error classifier; it must stay a literal
     * part of `inputMismatch` in every locale.
     */
    inputMismatchFragment: '与已提交输入不一致',
    singleItemRequired: '已接受的单节点 Fork 必须只有一个项目',
    attemptMissingItem: (attemptId: string, itemKey: string) => `重试 ${attemptId} 缺少项目 ${itemKey}`,
    failureRecordMissing: (itemKey: string) => `项目 ${itemKey} 的失败 Fork 记录缺失`,
    noCreatedSession: 'Fork 未返回已创建的会话',
    createFailed: 'Fork 创建失败',
    deliveryUncertain: (reason: string) => `节点已创建，任务投递结果待确认：${reason}`,
    startPending: (reason: string) => `节点已创建，任务仍待启动：${reason}`,
    deliveryUncertainShort: '节点已创建，任务投递结果待确认',
    itemCountMismatch: (batchKey: string) => `批次 ${batchKey} 的持久条目数量不一致`,
    noRetryableResult: (batchKey: string) => `批次 ${batchKey} 没有可重试的上一轮结果`,
    missingItem: (batchKey: string, itemKey: string) => `批次 ${batchKey} 缺少项目 ${itemKey}`,
    retryTicketMismatch: (batchKey: string) => `批次 ${batchKey} 的重试凭据与请求不一致`,
    retryOnlyFailed: (itemKeys: string) => `仅可重试上一轮失败的项目：${itemKeys}`,
    attemptNotWritten: (attemptId: string) => `重试 ${attemptId} 未写入`,
    settlementTimeout: 'Fork 状态确认超时，请稍后仅重试此项',
    settlementMissing: 'Fork 状态记录不可用，请仅重试此项',
    settlementFailed: 'Fork 状态确认失败，请稍后仅重试此项'
  },

  // runtime-host-action-facade.ts
  facade: {
    submissionKeyReused: 'submission key 已被不同输入使用',
    acceptedForkMissingPath: '已接受的 Fork 结果缺少稳定会话路径',
    forkNodeNotReady: 'Fork 节点尚未准备完成',
    noMainWindow: '目标当前没有可用的主窗口',
    noTaskInWorkspace: '目标工作空间没有可用事项',
    noCanvasInTask: '目标事项没有可用画布',
    noWorkspaceAfterAction: '操作完成后没有可用工作空间',
    noSessionAnchor: '目标画布没有可用会话锚点',
    canvasUsesCloseAction: '画布使用关闭画布操作',
    defaultWorkspaceKept: '默认工作空间会保留在侧栏中',
    entityKindMismatch: (kind: string) => `目标类型不匹配，需要 ${kind}`,
    workspaceDirectoryUnavailable: (path: string) => `工作空间目录不可用: ${path}`,
    invalidRequest: '请求参数不符合动作约束',
    unrecognizedKeys: (fields: string) => `请求参数包含不支持的字段: ${fields}`,
    invalidType: (field: string) => `请求参数 ${field} 缺失或类型不正确`,
    invalidValue: (field: string) => `请求参数 ${field} 的值不受支持`,
    invalidUnion: (field: string) => `请求参数 ${field} 不符合可用选择器格式`,
    invalidConstraint: (field: string) => `请求参数 ${field} 不符合约束`
  }
}

export const controlEn: CatalogShape<typeof controlZhCN> = {
  windowTitle: {
    detachedTerminal: 'Detached terminal window',
    main: 'Main window'
  },

  navigation: {
    targetNotReady: 'The target window is not ready; try again shortly',
    requestInProgress: 'A navigation request is already running; try again shortly',
    sendFailed: 'The navigation request could not be sent; try again shortly',
    rendererRejected: 'The target window did not complete the navigation; try again',
    invalidAck: 'The target window returned an invalid navigation result; try again',
    timeout: 'The target window did not respond in time; try again',
    closed: 'The navigation service has stopped; try again shortly'
  },

  client: {
    requestTooLarge: 'The Host Control request exceeds the size limit',
    mismatchedRequestId: 'Host Control returned a mismatched request id',
    requestFailed: 'The Host Control request failed',
    invalidResponse: 'Matou Host Control returned an invalid response; try again',
    connectTimeout: 'Connecting to Matou Host Control timed out; make sure Matou is still running and try again',
    notConnected: 'Not connected to Matou Host Control; try again from a terminal managed by Matou',
    responseTimeout: 'Waiting for the Matou Host Control response timed out; make sure Matou is still running and try again',
    responseTooLarge: 'The Host Control response exceeds the size limit',
    connectionLost: 'The connection to Matou Host Control was lost; try again'
  },

  topology: {
    callerNotInGraph: 'The calling session is no longer in the Matou session graph',
    sessionNotInGraph: 'That session is no longer in the Matou session graph',
    refNotInGraph: (ref) => `Target ${ref} is no longer in the Matou session graph`,
    siblingOutOfRange: (ordinal) => `This level has no session number ${ordinal}`,
    noSessionLeft: 'There is no sibling session to the left of the current session',
    noSessionRight: 'There is no sibling session to the right of the current session',
    noParent: 'The current session has no parent session',
    parentNotInGraph: 'The parent session is no longer in the Matou session graph',
    childOutOfRange: (ordinal) => `The current session has no child session number ${ordinal}`
  },

  target: {
    plainDirectoryOnly: 'A plain directory can only keep using the current execution environment',
    worktreeRefInvalid: 'The submitted worktree reference is invalid',
    worktreeUnavailable: 'The requested worktree is no longer available',
    worktreeBranchMismatch: (branch) => `The worktree is not on the submitted branch ${branch}`,
    branchExists: (branch) => `Branch ${branch} already exists`,
    staleProjection: 'The target list changed; list the targets again before running this',
    ambiguous: (ref) => `Target ${ref} matches more than one place in the hierarchy`,
    notFound: (ref) => `Target ${ref} does not exist`,
    callerSessionMissing: 'The calling session does not exist',
    targetSessionMissing: 'The target session does not exist',
    sourceContextRefInvalid: 'The source execution environment reference is invalid',
    sourceSessionMissing: 'The source session does not exist',
    branchCheckFailed: (branch) => `Repository branch check failed: ${branch}`,
    impactTargetMissing: 'The affected target no longer exists'
  },

  confirmation: {
    duplicateRef: 'Generated a duplicate confirmation reference',
    required: 'The confirmation is no longer valid; preview again first',
    noneForRun: 'This run has no confirmation available',
    expired: 'The confirmation has expired; preview again',
    stale: 'The target or the impact behind this confirmation changed; preview again',
    actionChanged: 'The action behind this confirmation changed; preview again'
  },

  providerReady: {
    waitTimeout: (sessionId) => `Timed out waiting for the provider of session ${sessionId} to become ready`,
    waitCancelled: (sessionId) => `Waiting for session ${sessionId} to become ready was cancelled`,
    allCancelled: 'Waiting for the provider to become ready was cancelled'
  },

  backend: {
    taskError: 'Task error',
    noInputTerminal: 'The target session has no terminal process that can accept input'
  },

  forkSettlement: {
    missing: 'The fork status record is unavailable',
    timeout: 'Fork status confirmation timed out'
  },

  forkBatch: {
    inputMismatch: (batchKey) => `Batch ${batchKey} does not match the submitted input`,
    inputMismatchFragment: 'does not match the submitted input',
    singleItemRequired: 'An accepted single-node fork must contain exactly one item',
    attemptMissingItem: (attemptId, itemKey) => `Retry ${attemptId} is missing item ${itemKey}`,
    failureRecordMissing: (itemKey) => `The failed fork record for item ${itemKey} is missing`,
    noCreatedSession: 'The fork did not return the created session',
    createFailed: 'Fork creation failed',
    deliveryUncertain: (reason) => `The node was created; the task delivery result is unconfirmed: ${reason}`,
    startPending: (reason) => `The node was created; the task has not started yet: ${reason}`,
    deliveryUncertainShort: 'The node was created; the task delivery result is unconfirmed',
    itemCountMismatch: (batchKey) => `Batch ${batchKey} has a different number of stored items`,
    noRetryableResult: (batchKey) => `Batch ${batchKey} has no previous result to retry`,
    missingItem: (batchKey, itemKey) => `Batch ${batchKey} is missing item ${itemKey}`,
    retryTicketMismatch: (batchKey) => `The retry ticket of batch ${batchKey} does not match the request`,
    retryOnlyFailed: (itemKeys) => `Only items that failed in the previous round can be retried: ${itemKeys}`,
    attemptNotWritten: (attemptId) => `Retry ${attemptId} was not written`,
    settlementTimeout: 'Fork status confirmation timed out; retry just this item later',
    settlementMissing: 'The fork status record is unavailable; retry just this item',
    settlementFailed: 'Fork status confirmation failed; retry just this item later'
  },

  facade: {
    submissionKeyReused: 'The submission key was already used for a different request',
    acceptedForkMissingPath: 'The accepted fork result has no stable session path',
    forkNodeNotReady: 'The fork node is not ready yet',
    noMainWindow: 'The target has no main window available',
    noTaskInWorkspace: 'The target workspace has no task available',
    noCanvasInTask: 'The target task has no canvas available',
    noWorkspaceAfterAction: 'No workspace is available after the action',
    noSessionAnchor: 'The target canvas has no session anchor available',
    canvasUsesCloseAction: 'Use the close-canvas action for a canvas',
    defaultWorkspaceKept: 'The default workspace stays in the sidebar',
    entityKindMismatch: (kind) => `Target type mismatch; ${kind} is required`,
    workspaceDirectoryUnavailable: (path) => `The workspace directory is unavailable: ${path}`,
    invalidRequest: 'The request parameters do not satisfy the action constraints',
    unrecognizedKeys: (fields) => `The request parameters include unsupported fields: ${fields}`,
    invalidType: (field) => `Request parameter ${field} is missing or has the wrong type`,
    invalidValue: (field) => `Request parameter ${field} has an unsupported value`,
    invalidUnion: (field) => `Request parameter ${field} does not match an available selector format`,
    invalidConstraint: (field) => `Request parameter ${field} does not satisfy its constraint`
  }
}
