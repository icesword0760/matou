# Matou 事件与流协议

状态：已定稿  
协议版本：`1`

## 1. 设计原则

1. Control、Terminal Data、Semantic Event 在类型和队列上隔离。
2. 所有连接先握手，V1 只接受精确版本匹配。
3. 每个 Session 内严格有序；不同 Session 不建立全局输出顺序。
4. Journal sequence 是恢复游标，不是终端屏幕坐标。
5. Runtime 先持久化，再把 Terminal Data 交给 live subscriber。
6. Renderer 的 ACK 表示 xterm parser 已处理，不表示 GPU 已完成呈现。

## 2. Envelope

除二进制 payload 外，消息使用 discriminated object：

```ts
interface BaseMessage {
  type: string
  protocolVersion: 1
}
```

未知 `type`、缺失字段、超限字符串、无权限的 sessionId 均被拒绝。协议错误通过 `protocol.error` 返回；连续协议违规会关闭端口。

## 3. Handshake

### Renderer -> Runtime

```ts
interface ProtocolHello {
  type: 'protocol.hello'
  protocolVersion: 1
  clientId: string
}
```

### Runtime -> Renderer

```ts
interface ProtocolReady {
  type: 'protocol.ready'
  protocolVersion: 1
  runtimeId: string
  capabilities: Array<'terminal-v1' | 'semantic-events-v1' | 'replay-v1' | 'domain-rpc-v1' | 'projection-v1'>
}
```

版本不一致：

```ts
interface ProtocolError {
  type: 'protocol.error'
  protocolVersion: 1
  code: 'VERSION_MISMATCH' | 'INVALID_MESSAGE' | 'SESSION_FORBIDDEN' | 'INTERNAL_ERROR'
  message: string
}
```

## 4. Terminal Control

```ts
interface TerminalSpawn {
  type: 'terminal.spawn'
  protocolVersion: 1
  sessionId: string
  executionContextId: string
  profile: 'shell' | 'claude-code' | 'codex'
  cols: number
  rows: number
}

interface TerminalInput {
  type: 'terminal.input'
  protocolVersion: 1
  sessionId: string
  data: string
}

interface TerminalResize {
  type: 'terminal.resize'
  protocolVersion: 1
  sessionId: string
  resizeId: number
  cols: number
  rows: number
}

interface TerminalResized {
  type: 'terminal.resized'
  protocolVersion: 1
  sessionId: string
  resizeId: number
  cols: number
  rows: number
}

interface TerminalDispose {
  type: 'terminal.dispose'
  protocolVersion: 1
  sessionId: string
}
```

限制：

- `cols` 范围 `2..1000`，`rows` 范围 `1..500`；
- `resizeId` 在单个 Session 内递增；Runtime 仅在 node-pty 应用该尺寸后回传同 ID 的 `terminal.resized`，用于最终尺寸屏障；
- 单条 input 默认上限 1 MiB；粘贴大文本由 Renderer 分片；
- Runtime 根据 profile 和 ExecutionContext 选择可执行文件与 cwd；
- Renderer 不传 shell path、任意 argv 或任意 cwd。

## 5. Terminal Data 与 ACK

### Runtime -> Renderer

```ts
interface TerminalData {
  type: 'terminal.data'
  protocolVersion: 1
  sessionId: string
  sequence: number
  data: Uint8Array
}
```

`sequence` 是每个 Session 单调递增的 frame 序号。一个 frame 对应一个 Journal frame，可包含批量 PTY 输出。

### Renderer -> Runtime

```ts
interface TerminalAck {
  type: 'terminal.ack'
  protocolVersion: 1
  sessionId: string
  throughSequence: number
}
```

ACK 是累计确认。重复或较旧 ACK 是幂等操作；超过已发送 sequence 的 ACK 是协议错误。

### Credit Window

每个连接的每个 Session 维护：

```text
sentBytes
acknowledgedBytes
unackedBytes = sentBytes - acknowledgedBytes
```

初始建议值：

- high watermark：1 MiB；
- low watermark：512 KiB；
- 单帧目标上限：64 KiB；
- Runtime 还有跨 Session 全局队列上限。

当 live subscriber 超过 high watermark 时，Runtime 停止继续向该 subscriber 投递。PTY 输出仍写 Journal。订阅者恢复后使用 replay 衔接；只有 Journal writer 本身超过高水位才暂停 PTY。

## 6. Replay

```ts
interface TerminalReplayRequest {
  type: 'terminal.replay-request'
  protocolVersion: 1
  sessionId: string
  fromSequence: number
}

interface TerminalReplayStart {
  type: 'terminal.replay-start'
  protocolVersion: 1
  sessionId: string
  checkpointSequence?: number
  checkpoint?: {
    terminalSequence: number
    domainEventSequence: number
    screenEpoch: number
    snapshot: Uint8Array
  }
  availableFromSequence: number
  liveSequence: number
}

interface TerminalReplayComplete {
  type: 'terminal.replay-complete'
  protocolVersion: 1
  sessionId: string
  throughSequence: number
}

interface TerminalGap {
  type: 'terminal.gap'
  protocolVersion: 1
  sessionId: string
  requestedFromSequence: number
  availableFromSequence: number
  reason: 'retention' | 'corruption'
}
```

恢复顺序：

1. Runtime 选择不晚于请求位置的最近 Checkpoint；
2. Renderer 恢复 Checkpoint；
3. Runtime 按 sequence 补播 Journal frames；
4. replay 同样受 1 MiB/512 KiB 累计 ACK credit window 约束；
5. 追平期间新写入的 Journal tail，直到稳定水位；
6. 发送 `replay-complete`；
7. 原子切换到 live frames，sequence 不重置。

## 7. Journal Frame

Journal 是每个 Session 的分段 append-only 文件。逻辑 frame：

```ts
type JournalFrame =
  | { kind: 'output'; sequence: number; timestamp: number; data: Uint8Array }
  | { kind: 'resize'; sequence: number; timestamp: number; cols: number; rows: number }
  | { kind: 'reset'; sequence: number; timestamp: number }
  | { kind: 'encoding'; sequence: number; timestamp: number; encoding: 'utf-8' }
  | { kind: 'exit'; sequence: number; timestamp: number; exitCode: number; signal?: number }
```

Checkpoint 单独存储终端序列化状态并记录：

```text
sessionId
terminalSequence
domainEventSequence
screenEpoch
serializedTerminalState
formatVersion
checksum
```

活动 segment 不压缩；关闭后的 segment 可压缩。SQLite 保存 segment path、sequence range、file offset、checksum 和 checkpoint 索引，不保存大体量原始 PTY stream。

## 8. Semantic Events

```ts
interface SemanticEvent<T = unknown> {
  type: 'semantic.event'
  protocolVersion: 1
  eventId: string
  sessionId: string
  taskId: string
  sequence: number
  occurredAt: string
  name:
    | 'agent.message'
    | 'agent.todo'
    | 'agent.tool-started'
    | 'agent.tool-finished'
    | 'agent.permission-requested'
    | 'file.changed'
    | 'artifact.observed'
    | 'validation.status-changed'
  sourceRef?: {
    provider: 'claude-code' | 'codex' | 'generic'
    providerEventId: string
  }
  payload: T
  relatedTerminalSequence?: number
}
```

语义 sequence 与 Terminal sequence 分开计数。`relatedTerminalSequence` 仅做相关性链接，不改变任一流的排序。

Claude Code Adapter 优先从 hooks、transcript 或结构化 stream-json 获取事件；屏幕文本解析只作为明确标记为低置信度的兼容路径。

## 9. Ordering 与幂等

- Runtime 为每个 Session 的 Terminal frame 分配唯一 sequence。
- Domain/Semantic event 使用唯一 eventId，消费者按 eventId 幂等。
- reconnect 不重用 clientId；Session sequence 延续。
- Runtime 先写 Journal frame，再发布对应 live data。
- PTY exit 与最后一批 output 保持 Journal 顺序，ExitFrame 最后写入。

## 10. 关闭

正常关闭：

1. Renderer 发送 `terminal.dispose`；
2. Runtime 停止接收新的 input；
3. Runtime 结束 PTY，写入剩余 output 和 ExitFrame；
4. flush segment manifest；
5. 发布 session exited 事件。

端口意外关闭时 Runtime 将订阅者移除，但不立即结束 PTY。是否继续运行由 Session/Task 生命周期决定。

## 11. 四级层级 RPC 与布局数据

- Workspace、Task、Scene、Session 的结构性修改走 `hierarchy.*` RPC，并与对应领域事件在一个事务中提交；
- Renderer 每次修改后以 Runtime snapshot/Outbox 重建投影，不导出完整权威对象快照；
- Scene 分屏树使用 `layoutRevision` 做结构性 CAS；过期的树替换会被拒绝；
- 分割线比例等几何状态使用 `geometry.put`，以 100 ms 防抖写入 `scene_geometry`，不进入 Outbox；同一结构版本允许连续覆盖，低于当前结构版本的写入被拒绝；
- `window_task_placements` 约束一个 Task 同时只属于一个主窗口显示槽位；迁移事件只改变窗口投影，不改变 Workspace、ExecutionContext、Session 或 PTY 身份；
- 工作区路径无效时，Runtime 在 spawn/input 和所有执行型层级命令两端都执行拦截，不使用其它 cwd 代替。
