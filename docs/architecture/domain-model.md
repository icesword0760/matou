# Matou 领域模型

状态：已定稿  
日期：2026-08-24

## 1. 核心层级

```text
Workspace
└── Task
    ├── ExecutionContext
    │   └── Worktree | PlainDirectory
    ├── Session
    ├── SessionRelation
    ├── Annotation
    ├── Artifact
    └── ValidationRun
```

Workspace 是项目容器，Task 是用户工作的核心单位，Session 是某个 Task 下的一次 Shell 或 Agent 交互。DAG 是 SessionRelation 的投影，不是另一套数据模型。

## 2. 聚合与实体

### 2.1 Workspace

```ts
interface Workspace {
  id: WorkspaceId
  name: string
  rootDirectory: string
  taskOrder: TaskId[]
  createdAt: string
  updatedAt: string
}
```

约束：

- `rootDirectory` 代表用户选择的项目根目录，可以暂时失效；
- 路径失效不删除 Task、Session 或历史数据；
- Task 排序属于 Workspace 元数据。

### 2.2 Task

```ts
type TaskStatus = 'planned' | 'active' | 'blocked' | 'completed' | 'archived'

interface Task {
  id: TaskId
  workspaceId: WorkspaceId
  parentTaskId?: TaskId
  title: string
  status: TaskStatus
  executionContextId: ExecutionContextId
  createdAt: string
  updatedAt: string
}
```

约束：

- fork 默认创建子 Task，并为子 Task 创建独立 ExecutionContext；
- 同一 Task 下可包含多个 Session，例如主 Agent、测试 Shell 和日志 Shell；
- Task 状态独立于任何单个 PTY 是否存活。

### 2.3 ExecutionContext

```ts
type ExecutionContext = WorktreeContext | PlainDirectoryContext

interface WorktreeContext {
  kind: 'git-worktree'
  id: ExecutionContextId
  workspaceId: WorkspaceId
  worktreeId: WorktreeId
  cwd: string
}

interface PlainDirectoryContext {
  kind: 'plain-directory'
  id: ExecutionContextId
  workspaceId: WorkspaceId
  cwd: string
}
```

ExecutionContext 是 Session 运行位置的唯一权威来源。Renderer 只提交 `executionContextId`，Runtime 解析实际 cwd。

### 2.4 Worktree

```ts
type WorktreeState = 'creating' | 'ready' | 'dirty' | 'merging' | 'retained' | 'removing' | 'broken'

interface Worktree {
  id: WorktreeId
  repositoryRoot: string
  path: string
  branch: string
  baseRevision: string
  state: WorktreeState
  createdAt: string
}
```

生命周期约束：

- dirty worktree 默认进入 `retained`，不执行强制删除；
- worktree 删除必须先结束绑定的运行 Session；
- merge/rebase 是显式命令，Session 退出不触发自动合并；
- 非 Git Workspace 使用 PlainDirectoryContext；
- secrets、依赖和 gitignored 文件的初始化由可审计 setup policy 处理。

### 2.5 Session

```ts
type SessionKind = 'shell' | 'claude-code' | 'codex' | 'agent-team-member'
type SessionStatus = 'created' | 'starting' | 'running' | 'waiting' | 'interrupted' | 'exited'

interface Session {
  id: SessionId
  taskId: TaskId
  executionContextId: ExecutionContextId
  kind: SessionKind
  status: SessionStatus
  createdAt: string
  lastActivityAt: string
}
```

Claude Code/Codex 原生 resume 身份保存在独立 `ProviderBinding` 中。它不是 Matou Session 的主键；一次 Session 可有多次 `SessionRun`，PTY 重建后 Matou SessionId 保持稳定。

### 2.6 SessionRelation

```ts
type RelationKind = 'forked-from' | 'derived-from' | 'depends-on' | 'supports' | 'blocks' | 'references' | 'team-member-of'

interface SessionRelation {
  id: RelationId
  taskId: TaskId
  fromSessionId: SessionId
  toSessionId: SessionId
  kind: RelationKind
  createdAt: string
}
```

约束：

- `forked-from` 在同一条 fork 链上不能形成环；
- `depends-on` 不能形成环；每个 Session 同时只能有一个有效 `forked-from` 父节点；
- 兄弟关系由共同的有效 fork 父节点查询派生，不持久化 `sibling` 边；
- supports/blocks/references 等非层级关系允许表达一般图；
- 平铺、卡片和 DAG 都读取同一关系集合。

### 2.7 Annotation

```ts
type AnnotationKind = 'todo' | 'bookmark' | 'note'

interface Annotation {
  id: AnnotationId
  taskId: TaskId
  sessionId: SessionId
  kind: AnnotationKind
  text: string
  anchor: Anchor
  status: 'open' | 'done' | 'archived'
  createdAt: string
}
```

Annotation 的文本是创建时快照；锚点失效时仍可查看快照。

### 2.8 Artifact 与 ValidationRun

```ts
interface Artifact {
  id: ArtifactId
  taskId: TaskId
  producedBySessionId?: SessionId
  path: string
  mediaType?: string
  state: 'present' | 'modified' | 'deleted'
  observedAt: string
}

interface ValidationRun {
  id: ValidationRunId
  taskId: TaskId
  sessionId?: SessionId
  checkId: string
  status: 'queued' | 'running' | 'passed' | 'failed' | 'cancelled'
  startedAt?: string
  finishedAt?: string
}
```

产物和检查是 Task 投影，可由多个 Session 共同贡献。

## 3. 锚点

```ts
type Anchor = SemanticAnchor | CommandOutputAnchor | ScreenCaptureAnchor
```

### 3.1 SemanticAnchor

```ts
interface SemanticAnchor {
  kind: 'semantic-event'
  sessionId: SessionId
  eventId: EventId
  sourceRef?: {
    provider: 'claude-code' | 'codex' | 'generic'
    providerEventId: string
  }
  contentPath?: string
}
```

用于工具调用、权限请求、待办、文件变更和其他结构化 Agent 事件。`tool_use_id` 进入 `sourceRef`，Runtime 生成跨 provider 一致的 EventId。

### 3.2 CommandOutputAnchor

```ts
interface CommandOutputAnchor {
  kind: 'command-output'
  sessionId: SessionId
  commandId: CommandId
  startSequence: number
  endSequence: number
  logicalLineRange?: { start: number; end: number }
}
```

命令边界优先来自 OSC 633/133。sequence 是 Journal frame 序号，不是 xterm buffer 行号。

### 3.3 ScreenCaptureAnchor

```ts
interface ScreenCaptureAnchor {
  kind: 'screen-capture'
  sessionId: SessionId
  screenEpoch: number
  sequence: number
  geometry: { cols: number; rows: number }
  range: { startX: number; startY: number; endX: number; endY: number }
  capturedText: string
}
```

用于 alternate screen 或不具备语义/命令边界的自由选择。跳转优先级：当前画面定位、历史 Checkpoint 重建、只读捕获内容。

## 4. 领域事件

所有权威修改产生不可变事件：

```text
workspace.created
task.created
task.status-changed
execution-context.created
worktree.state-changed
session.created
session.status-changed
session.relation-created
annotation.created
annotation.status-changed
artifact.observed
validation.status-changed
```

语义事件与终端字节分别排序。一个语义事件可以引用相关 Journal sequence，但终端输出本身不自动成为领域事件。

## 5. 删除和保留

- 删除 Workspace 是显式高风险操作，先终止运行 Session，再处理 worktree 和本地历史。
- 归档 Task 保留 Session、Relation、Annotation、Artifact 与 Journal。
- 删除 Session 默认删除运行资源但保留历史；彻底删除是单独操作。
- Annotation 不因 xterm Marker 被 dispose 而删除。
- Journal retention 由用户设置和磁盘配额决定，元数据记录历史是否已截断。

## 6. 投影规则

Renderer store 是由 Runtime snapshot 与 events 构建的 projection：

- Scene Projection：Session + Relation + focus state；
- Notification Projection：waiting/error/permission 事件；
- Artifact Projection：Artifact 当前状态；
- Validation Projection：ValidationRun 当前状态；
- Terminal Projection：Checkpoint + Journal tail + live frames。

Projection 丢失时可以重建，因此 React store、xterm Marker 和 DOM 节点均不进入 SQLite 权威模型。
