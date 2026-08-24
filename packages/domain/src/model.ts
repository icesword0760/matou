export type WorkspaceId = string
export type TaskId = string
export type SessionId = string
export type ExecutionContextId = string
export type WorktreeId = string
export type RelationId = string
export type EventId = string
export type CommandId = string
export type SessionRunId = string
export type ProviderBindingId = string
export type SceneId = string
export type SceneNodeId = string
export type SessionMountId = string

export type WorkspacePathStatus = 'valid' | 'invalid'
export type WorkspacePathReason = '' | 'missing' | 'not-directory' | 'no-access' | 'unknown'

export interface WorkspacePathState {
  workspaceId: WorkspaceId
  status: WorkspacePathStatus
  reason: WorkspacePathReason
  checkedAt: number
  validationGeneration: number
}

export interface WindowNavigation {
  windowId: string
  activeWorkspaceId?: WorkspaceId
  taskByWorkspace: Record<WorkspaceId, TaskId>
  sceneByTask: Record<TaskId, SceneId>
  sessionByScene: Record<SceneId, SessionId>
}

export interface TaskPlacement {
  windowId: string
  taskId: TaskId
  ordinal: number
  updatedAt: number
}

export interface Workspace {
  id: WorkspaceId
  name: string
  rootDirectory: string
  pathIdentity?: string
  taskOrder: TaskId[]
  archivedAt?: number
  createdAt: number
  updatedAt: number
  version: number
}

export type TaskStatus = 'planned' | 'active' | 'blocked' | 'completed' | 'archived'

export interface Task {
  id: TaskId
  workspaceId: WorkspaceId
  parentTaskId?: TaskId
  title: string
  status: TaskStatus
  executionContextId: ExecutionContextId
  sortKey: string
  archivedAt?: number
  createdAt: number
  updatedAt: number
  version: number
}

export type ExecutionContext = WorktreeContext | PlainDirectoryContext

export interface WorktreeContext {
  kind: 'git-worktree'
  id: ExecutionContextId
  workspaceId: WorkspaceId
  worktreeId: WorktreeId
  cwd: string
  createdAt: number
  archivedAt?: number
}

export interface PlainDirectoryContext {
  kind: 'plain-directory'
  id: ExecutionContextId
  workspaceId: WorkspaceId
  cwd: string
  createdAt: number
  archivedAt?: number
}

export type WorktreeState =
  | 'creating'
  | 'ready'
  | 'dirty'
  | 'retained'
  | 'removing'
  | 'removed'
  | 'failed'

export interface Worktree {
  id: WorktreeId
  executionContextId: ExecutionContextId
  repositoryRoot: string
  path: string
  branch: string
  baseRevision?: string
  state: WorktreeState
  setupPolicy: unknown[]
  setupResult: unknown[]
  cleanupPolicy: 'retain-dirty'
  createdAt: number
  updatedAt: number
  retainedAt?: number
}

export type SessionKind = 'shell' | 'claude-code' | 'codex' | 'agent-team-member'
export type SessionStatus = 'created' | 'starting' | 'running' | 'waiting' | 'interrupted' | 'exited' | 'archived'

export interface Session {
  id: SessionId
  taskId: TaskId
  executionContextId: ExecutionContextId
  kind: SessionKind
  status: SessionStatus
  title: string
  createdAt: number
  updatedAt: number
  lastActivityAt: number
  archivedAt?: number
  version: number
}

export type SessionRunStatus = 'starting' | 'running' | 'exited' | 'failed' | 'interrupted'

export interface SessionRun {
  id: SessionRunId
  sessionId: SessionId
  ordinal: number
  runtimeGeneration: string
  profile: 'shell' | 'claude-code' | 'codex'
  pid?: number
  status: SessionRunStatus
  cols: number
  rows: number
  startedAt: number
  endedAt?: number
  exitCode?: number
  signal?: number
}

export interface ProviderBinding {
  id: ProviderBindingId
  sessionId: SessionId
  provider: 'claude-code' | 'codex' | 'generic'
  providerSessionId: string
  resumeState: 'unknown' | 'available' | 'resuming' | 'resumed' | 'failed' | 'expired'
  metadata: unknown
  createdAt: number
  updatedAt: number
  validatedAt?: number
  invalidatedAt?: number
}

export type RelationKind =
  | 'forked-from'
  | 'derived-from'
  | 'depends-on'
  | 'supports'
  | 'blocks'
  | 'references'
  | 'team-member-of'

export interface SessionRelation {
  id: RelationId
  taskId: TaskId
  fromSessionId: SessionId
  toSessionId: SessionId
  kind: RelationKind
  metadata: unknown
  createdAt: number
  updatedAt: number
}

export type SceneMode = 'tile' | 'card' | 'dag'

export interface Scene {
  id: SceneId
  taskId: TaskId
  name: string
  mode: SceneMode
  rootNodeId?: SceneNodeId
  titlePinned: boolean
  sortKey: string
  layoutRevision: number
  createdAt: number
  updatedAt: number
  archivedAt?: number
}

export interface SceneNode {
  id: SceneNodeId
  sceneId: SceneId
  parentNodeId?: SceneNodeId
  kind: 'root' | 'split' | 'mount' | 'group'
  direction?: 'horizontal' | 'vertical'
  ordinal: number
  createdAt: number
}

export interface SessionMount {
  id: SessionMountId
  sceneId: SceneId
  sceneNodeId?: SceneNodeId
  sceneWindowId?: string
  sessionId: SessionId
  createdAt: number
}

export interface SceneWindow {
  id: string
  sceneId: SceneId
  nativeWindowKey: string
  state: 'attached' | 'detached' | 'closed'
  createdAt: number
  updatedAt: number
}

export interface Annotation {
  id: string
  taskId: TaskId
  sessionId: SessionId
  kind: string
  textSnapshot: string
  anchor: Anchor
  status: 'active' | 'completed' | 'degraded' | 'archived'
  createdAt: number
  updatedAt: number
}

export interface Artifact {
  id: string
  taskId: TaskId
  producerSessionId?: SessionId
  pathIdentity: string
  mediaType?: string
  state: 'observed' | 'produced' | 'modified' | 'missing' | 'archived'
  metadata: unknown
  createdAt: number
  updatedAt: number
}

export interface ValidationRun {
  id: string
  taskId: TaskId
  sessionId?: SessionId
  checkId: string
  status: 'queued' | 'running' | 'passed' | 'failed' | 'cancelled' | 'error'
  summary: unknown
  startedAt?: number
  endedAt?: number
  createdAt: number
}

export type Anchor = SemanticAnchor | CommandOutputAnchor | ScreenCaptureAnchor

export interface SemanticAnchor {
  kind: 'semantic-event'
  sessionId: SessionId
  eventId: EventId
  sourceRef?: {
    provider: 'claude-code' | 'codex' | 'generic'
    providerEventId: string
  }
  contentPath?: string
}

export interface CommandOutputAnchor {
  kind: 'command-output'
  sessionId: SessionId
  commandId: CommandId
  startSequence: number
  endSequence: number
  logicalLineRange?: { start: number; end: number }
}

export interface ScreenCaptureAnchor {
  kind: 'screen-capture'
  sessionId: SessionId
  screenEpoch: number
  sequence: number
  geometry: { cols: number; rows: number }
  range: { startX: number; startY: number; endX: number; endY: number }
  capturedText: string
}
