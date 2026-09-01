# Terminal Stream, Journal, and Interaction Robustness Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在不改变 Matou 核心会话模型的前提下，让大量会话、高频终端输出、异常磁盘、恢复、焦点、拖放、resize 和通知场景达到可预测、可恢复、可度量的内部发布质量。

**Architecture:** Runtime 继续持有所有后台 PTY；Renderer 只为当前前台 Scene 的当前横向会话层绑定 xterm，横向滚出视野的卡片仍保持绑定。Journal 采用 16 MiB segment、每会话最近 256 MiB raw 热窗口、异步压缩旧 segment，并通过 checkpoint + 10,000 行 tail index 完成每卡即时恢复；更旧压缩输出通过现有终端搜索入口查询。所有用户可感知变化先由 Electron/真实 PTY 测试固定，再写最小实现。

**Tech Stack:** TypeScript 7、Electron 43、React、xterm.js、node-pty、MessagePort、Node.js fs/zlib/worker_threads、Vitest、Playwright Electron E2E、SQLite。

**Spec:** 本文“Global Constraints”中的已确认产品决策，以及 `docs/audits/` 下本轮内部版审计证据。

## Global Constraints

- 后台 PTY 持续运行，但非前台 xterm 解绑。
- 当前横向列表中的会话即使滑出视野，仍属于前台并保持 xterm 绑定；不得按 viewport 虚拟化 PTY/xterm。
- 每张卡片独立显示恢复遮罩；一张卡恢复不得阻塞其他卡片输入、滑动或渲染。
- 最近 10,000 行即时恢复。
- 每会话最近 256 MiB raw journal；更旧输出异步压缩并保持可查询。
- Journal 写失败立即暂停单个会话执行；不得拖垮其他会话或 Runtime。
- App 回焦恢复离开前的原焦点；不得默认抢回终端。
- 大粘贴按 reference product：无提示、透明、保持字节顺序地进行 UTF-8 安全分块。
- 拖入普通路径和含空格路径的可见文本与 reference product 一致；含特殊字符路径必须安全引用。
- resize 最多 60Hz 合并，相同 cols/rows 去重。
- 通知每工作空间最多 1,000 条；已读通知保留 30 天后清理。
- macOS 是本计划的真实 PTY 和执行暂停验收平台；Windows 构建不得因 POSIX 信号实现而编译失败，并通过平台适配器测试。
- 不把任意 timeout 增大作为竞态修复；所有等待使用状态、ACK、进程或 DOM 条件。
- 每个生产代码任务遵循 RED → GREEN → REFACTOR；先看到目标测试因缺少行为而失败。

---

## File and Interface Map

### 新建的职责单一文件

- `apps/desktop/src/renderer/src/terminal/terminal-input-chunker.ts`：UTF-8 安全输入分块。
- `apps/desktop/src/renderer/src/terminal/shell-path-quote.ts`：终端拖入路径的可见文本和安全引用。
- `apps/desktop/src/renderer/src/terminal/resize-coalescer.ts`：60Hz resize 合并及相同尺寸去重。
- `apps/desktop/src/renderer/src/hierarchy/focus-restoration.ts`：窗口失焦/回焦的原焦点恢复。
- `apps/runtime/src/journal/journal-policy.ts`：16 MiB segment、256 MiB raw 热窗口和压缩候选规则。
- `apps/runtime/src/journal/journal-tail-index.ts`：按 terminal sequence 维护最近 10,000 行边界。
- `apps/runtime/src/journal/journal-history-reader.ts`：同时读取 raw 与压缩 segment，提供 page/search 流式接口。
- `apps/runtime/src/journal/journal-compressor.ts`：worker thread 异步压缩与原子替换。
- `apps/runtime/src/session/session-execution-pause.ts`：单会话执行暂停/恢复的平台适配器。
- `tests/e2e/fixtures/runtime-stress-fixture.ts`：真实 PTY 输出、事件循环延迟、Runtime/Renderer RSS 和 MessagePort 计数工具。

### 主要修改边界

- `apps/runtime/src/journal/segment-journal.ts`：委托 policy、tail index、history reader 和 compressor，不继续扩张为多职责类。
- `apps/runtime/src/session/pty-session.ts`：有界输出队列、Journal fatal 状态、暂停单会话。
- `apps/runtime/src/runtime-server.ts`：checkpoint、即时 replay、历史搜索、错误与恢复消息路由。
- `packages/contracts/src/protocol.ts`：新增恢复、checkpoint、history 和 journal-fatal 消息契约。
- `apps/desktop/src/renderer/src/terminal/TerminalSurface.tsx`：前台绑定、checkpoint、即时 replay、大粘贴、拖放、resize。
- `apps/desktop/src/renderer/src/hierarchy/TerminalPane.tsx`：每卡恢复/磁盘故障遮罩。
- `apps/desktop/src/renderer/src/hierarchy/HierarchyShell.tsx`：Scene 前台身份与焦点恢复。
- `apps/desktop/src/renderer/src/notifications/AgentNotificationStore.ts`：每工作空间容量和已读 TTL。

---

### Task 1: UTF-8 Safe Transparent Input Chunking

**Files:**
- Create: `apps/desktop/src/renderer/src/terminal/terminal-input-chunker.ts`
- Create: `apps/desktop/src/renderer/src/terminal/terminal-input-chunker.test.ts`
- Modify: `apps/desktop/src/renderer/src/runtime/RuntimeClient.ts`
- Modify: `apps/desktop/src/renderer/src/runtime/RuntimeClient.test.ts`
- Modify: `apps/desktop/src/renderer/src/terminal/TerminalSurface.tsx`
- Test: `tests/e2e/terminal-channel.spec.ts`

**Dependencies:** 无。

**Interfaces:**
- Produces: `splitUtf8ForTransport(value: string, maxBytes?: number): string[]`，默认 `maxBytes = 256 * 1024`。
- Produces: `RuntimeClient.sendTerminalInput(sessionId, data)` 依次同步 `postMessage` 所有 chunk；顺序与原始 UTF-8 字节完全一致。

- [ ] **Step 1: 写纯函数 RED 测试**
  - 用 ASCII、四字节 emoji、组合字符、代理对跨 256 KiB 边界构造输入。
  - 断言每个 chunk 的 `TextEncoder().encode(chunk).byteLength <= 256 * 1024`，拼接后严格等于原字符串，且不存在孤立 surrogate。

- [ ] **Step 2: 运行 RED**
  - Run: `pnpm --filter @matou/desktop test -- terminal-input-chunker.test.ts`
  - Expected: FAIL，模块或导出不存在。

- [ ] **Step 3: 写最小 chunker**
  - 按 Unicode code point 累计 UTF-8 byte length；达到上限前切片。
  - 空字符串返回空数组；不得使用会切断多字节字符的 Buffer 字节下标直接 slice 字符串。

- [ ] **Step 4: 写 RuntimeClient RED 测试**
  - 发送 `1 MiB + emoji + 128 KiB` 输入，记录 fake port 消息。
  - 断言产生多个 `terminal.input`，全部通过现有协议上限，顺序拼接一致，不产生 UI 错误事件。

- [ ] **Step 5: 接入 RuntimeClient 并跑 GREEN**
  - Run: `pnpm --filter @matou/desktop test -- terminal-input-chunker.test.ts RuntimeClient.test.ts`
  - Expected: PASS。

- [ ] **Step 6: 添加真实 PTY Electron 验收**
  - E2E 将 2.5 MiB、包含 emoji 的内容粘贴给执行 `python3 -c 'import sys; d=sys.stdin.buffer.readline(); print(len(d), d[-8:].hex())'` 的真实 PTY。
  - 断言终端输出的 byte count 和尾部 hex 与原输入一致；页面中没有 toast、dialog 或错误 banner。

- [ ] **Step 7: 运行验收**
  - Run: `pnpm exec playwright test tests/e2e/terminal-channel.spec.ts --grep "transparently chunks a large UTF-8 paste" --workers=1`
  - Expected: PASS，且 PTY 只收到一次连续逻辑输入。

---

### Task 2: reference product-Compatible and Safe Dropped Paths

**Files:**
- Create: `apps/desktop/src/renderer/src/terminal/shell-path-quote.ts`
- Create: `apps/desktop/src/renderer/src/terminal/shell-path-quote.test.ts`
- Modify: `apps/desktop/src/renderer/src/terminal/TerminalSurface.tsx`
- Modify: `apps/desktop/src/renderer/src/terminal/TerminalSurface.test.tsx`
- Test: `tests/e2e/terminal-channel.spec.ts`

**Dependencies:** 无。

**Interfaces:**
- Produces: `quoteDroppedPath(path: string): string`。
- 规则：普通路径原样；仅含空格而无其他 shell 特殊字符时使用 reference product 的双引号形式；含 `'"`、反引号、`$`、反斜线、换行、回车、分号、管道、重定向或 glob 字符时使用 POSIX 单引号安全形式，单引号编码为 `'\''`。

- [x] **Step 1: 写 quoting RED 测试**
  - 固定样例：`/tmp/a.txt` → `/tmp/a.txt`；`/tmp/a b.txt` → `"/tmp/a b.txt"`；`/tmp/a$(touch PWN).txt` 必须成为单个 shell argv；`/tmp/a'b.txt` 必须正确引用。
  - 使用 `/bin/zsh -fc 'python3 -c ... -- <quoted>'` 验证特殊路径最终 argv 与原始字符串逐字一致，而不是只断言字符串形状。

- [x] **Step 2: 运行 RED**
  - Run: `pnpm --filter @matou/desktop test -- shell-path-quote.test.ts`
  - Expected: FAIL，模块不存在。

- [x] **Step 3: 写最小实现并替换 TerminalSurface 内联逻辑**
  - `terminalDropPaths()` 对 `DataTransfer.files` 使用 `quoteDroppedPath`。
  - reference product file-tree MIME 提供的结构化文件路径逐项引用；不得把含换行的任意 `text/plain` 直接透传为 shell 命令片段。

- [x] **Step 4: 跑 unit GREEN**
  - Run: `pnpm --filter @matou/desktop test -- shell-path-quote.test.ts TerminalSurface.test.tsx`
  - Expected: PASS。

- [x] **Step 5: 写真实 Electron drop 验收**
  - 创建普通、空格、单引号、`$()` 四个真实文件名；拖入后不按 Enter，读取 xterm 当前行可见文本。
  - 普通和空格路径截图文本与 reference product 基线一致；特殊字符路径执行 `python3 -c` argv 回显后严格等于原路径，且副作用文件不存在。

---

### Task 3: 60Hz Resize Coalescing and Deduplication

**Files:**
- Create: `apps/desktop/src/renderer/src/terminal/resize-coalescer.ts`
- Create: `apps/desktop/src/renderer/src/terminal/resize-coalescer.test.ts`
- Modify: `apps/desktop/src/renderer/src/terminal/TerminalSurface.tsx`
- Modify: `apps/runtime/src/session/pty-session.ts`
- Modify: `apps/runtime/src/session/pty-session.test.ts`
- Test: `tests/e2e/session-canvas-navigation.spec.ts`

**Dependencies:** 无。

**Interfaces:**
- Produces: `ResizeCoalescer.offer(cols, rows)`、`flush()`、`dispose()`。
- 保证同一 animation frame 只发送最后一个尺寸；相邻已发送尺寸相同则不发送；发送频率不超过 60Hz。

- [ ] **Step 1: 写 fake-rAF RED 测试**
  - 一个 frame 内 offer 100 次只发最后一次；连续两个 frame 相同尺寸只发一次；dispose 后不发送。

- [ ] **Step 2: 运行 RED**
  - Run: `pnpm --filter @matou/desktop test -- resize-coalescer.test.ts`
  - Expected: FAIL。

- [ ] **Step 3: 写最小 coalescer 并接入 ResizeObserver**
  - ResizeObserver 仍立即 `fit.fit()`，但 Runtime resize 经 coalescer 发送。
  - unmount 前 flush 最后有效尺寸，再 dispose。

- [ ] **Step 4: Runtime 二次去重 RED/GREEN**
  - 在 `PtySession.resize` 测试连续相同 cols/rows 不调用 node-pty resize，也不追加 journal frame。
  - Run: `pnpm --filter @matou/runtime test -- pty-session.test.ts`

- [ ] **Step 5: 真实 Electron 验收**
  - 16 个前台 sibling 下连续拖动窗口 2 秒，拦截 `terminal.resize` 计数。
  - 每 session 每秒不超过 60 条；最终 PTY `stty size` 与 xterm rows/cols 一致；拖动期间输入回显不中断。

---

### Task 4: Restore the Original Focus on App Refocus

**Files:**
- Create: `apps/desktop/src/renderer/src/hierarchy/focus-restoration.ts`
- Create: `apps/desktop/src/renderer/src/hierarchy/focus-restoration.test.ts`
- Modify: `apps/desktop/src/renderer/src/hierarchy/HierarchyShell.tsx`
- Modify: `apps/desktop/src/renderer/src/terminal/TerminalSurface.tsx`
- Modify: `apps/desktop/src/renderer/src/hierarchy/HierarchyShell.test.tsx`
- Test: `tests/e2e/prd-05-hierarchy.spec.ts`

**Dependencies:** 无。

**Interfaces:**
- Produces: `FocusRestorationCoordinator.capture(element)` 和 `restore(root)`。
- 使用稳定身份：优先 `data-focus-key`；其次 dialog/search input 的 aria-label；终端使用 sessionId 对应的 helper textarea。

- [ ] **Step 1: 写 RED 测试**
  - 覆盖终端、重命名 input、搜索 input、Fork dialog button、已卸载元素。
  - 已卸载目标只把焦点落到所属 dialog 的第一个可聚焦元素，不跳到终端。

- [ ] **Step 2: 运行 RED**
  - Run: `pnpm --filter @matou/desktop test -- focus-restoration.test.ts HierarchyShell.test.tsx`

- [ ] **Step 3: 替换无条件 terminalFocusRequest**
  - window blur/visibility hidden 时 capture；focus/visible 时下一 frame restore。
  - 删除“App 回焦即增加 terminalFocusRequest”的路径；CC 回复仍只能在原先终端拥有焦点时保留焦点。

- [ ] **Step 4: 跑 GREEN**
  - Run: `pnpm --filter @matou/desktop test -- focus-restoration.test.ts HierarchyShell.test.tsx TerminalSurface.test.tsx`

- [ ] **Step 5: 真实 Electron 验收**
  - 分别在终端、Task 重命名、终端搜索、Fork dialog 中调用 BrowserWindow.blur()/focus()。
  - 每次回焦断言原控件仍 focused；在 input 输入 marker 后断言 shell 屏幕未出现 marker。

---

### Task 5: Notification Capacity and Read TTL

**Files:**
- Modify: `apps/desktop/src/renderer/src/notifications/AgentNotificationStore.ts`
- Modify: `apps/desktop/src/renderer/src/notifications/AgentNotificationStore.test.ts`
- Test: `tests/e2e/prd-01-agent-notifications.spec.ts`

**Dependencies:** 无。

**Interfaces:**
- Extend `AgentNotificationStoreOptions` with `maxPerWorkspace?: number` default 1000 and `readRetentionMs?: number` default `30 * 24 * 60 * 60 * 1000`。
- `workspaceId === null` 使用独立的 `__unassigned__` bucket，容量同为 1000。

- [ ] **Step 1: 写 RED 测试**
  - 同工作空间 push 1005 条后保留最新 1000 条；另一工作空间不受影响。
  - 未读通知即使超过 30 天仍保留到容量淘汰；已读超过 30 天在下一次 push/snapshot 时清理。
  - replacementKey 更新不额外占容量；cooldown map 对已不存在通知的 key 同步清理。

- [ ] **Step 2: 运行 RED**
  - Run: `pnpm --filter @matou/desktop test -- AgentNotificationStore.test.ts`

- [ ] **Step 3: 写最小 prune 实现**
  - 每次 push、mark-read 和 snapshot 前调用同一个 `#prune(now)`；按 timestamp+id 确定性淘汰。

- [ ] **Step 4: 跑 GREEN 和 10k 性能断言**
  - 10 个工作空间各 1000 条，单次 push/prune 在本机测试进程中小于 20ms；不对不稳定 CI 绝对时间做硬断言，记录 benchmark 结果并断言最终容量。

- [ ] **Step 5: Electron 验收**
  - 通过 E2E bridge 向两个工作空间注入通知，检查 badge、导航和清理后通知中心数量。

---

### Task 6: Journal Hot-Window Policy (256 MiB Raw per Session)

**Files:**
- Create: `apps/runtime/src/journal/journal-policy.ts`
- Create: `apps/runtime/src/journal/journal-policy.test.ts`
- Modify: `apps/runtime/src/journal/segment-journal.ts`
- Modify: `apps/runtime/src/journal/segment-journal.test.ts`
- Modify: `apps/runtime/src/retention/retention-manager.ts`
- Modify: `apps/runtime/src/retention/retention-manager.test.ts`

**Dependencies:** 无。

**Interfaces:**
- Produces constants `SEGMENT_BYTES = 16 * 1024 * 1024`、`RAW_HOT_BYTES = 256 * 1024 * 1024`。
- Produces `selectCompressionCandidates(segments): SegmentDescriptor[]`；始终保护 active segment、最近 16 个 sealed raw segments、checkpoint 覆盖所需 segment。
- 文件状态：active/raw 使用 `.mtj`；冷 segment 使用 `.mtj.gz`；同一 index 不得同时作为有效 raw 和 gzip 被读取两次。

- [ ] **Step 1: 写 policy RED 测试**
  - 17、18、40 个 16 MiB segment 时分别只选超出最近 16 个的 sealed raw segment。
  - active segment 和 checkpoint protected segment 永不入选。

- [ ] **Step 2: 运行 RED**
  - Run: `pnpm --filter @matou/runtime test -- journal-policy.test.ts`

- [ ] **Step 3: 实现 policy 并调整 SegmentJournal 轮转**
  - 轮转只 seal/close raw 文件，不在实时路径调用同步 gzip。
  - 返回 compression candidate 给异步 compressor；写入路径不等待压缩完成。

- [ ] **Step 4: 修改 RetentionManager 语义测试**
  - quota 不再删除仅因超过 256 MiB 的输出；先压缩旧 raw，压缩文件仍可查询。
  - 只有显式 session purge 才删除全部历史。

- [ ] **Step 5: 跑 GREEN**
  - Run: `pnpm --filter @matou/runtime test -- journal-policy.test.ts segment-journal.test.ts retention-manager.test.ts`

---

### Task 7: Production Checkpoints and the 10,000-Line Tail Index

**Files:**
- Create: `apps/runtime/src/journal/journal-tail-index.ts`
- Create: `apps/runtime/src/journal/journal-tail-index.test.ts`
- Modify: `apps/runtime/src/checkpoints/checkpoint-manager.ts`
- Modify: `apps/runtime/src/checkpoints/checkpoint-manager.test.ts`
- Modify: `apps/runtime/src/runtime-server.ts`
- Modify: `apps/runtime/src/runtime-server.test.ts`
- Modify: `packages/contracts/src/protocol.ts`
- Modify: `packages/contracts/src/protocol.test.ts`
- Modify: `apps/desktop/package.json`
- Modify: `apps/desktop/src/renderer/src/terminal/TerminalSurface.tsx`
- Modify: `apps/desktop/src/renderer/src/terminal/TerminalSurface.test.tsx`

**Dependencies:** Task 6。

**Interfaces:**
- Produces `JournalTailIndex.record(sequence, bytes)`、`tailStart(maxLines: 10000)`、`snapshot()`。
- 新增 renderer message `terminal.checkpoint`：`sessionId`、`throughSequence`、`screenEpoch`、UTF-8 serialized xterm snapshot。
- 新增 runtime replay metadata：`source: 'checkpoint' | 'tail'`、`fromSequence`、`throughSequence`、`instantLineLimit: 10000`。
- 使用 `@xterm/addon-serialize` 序列化当前 screen + 最多 10,000 行 scrollback。

- [ ] **Step 1: 写 TailIndex RED 测试**
  - 覆盖 `\n`、`\r\n`、Unicode 分块、一个换行跨 frame、超长单行、alternate-screen 控制序列不计为换行。
  - 10,001 行时 tailStart 指向倒数第 10,000 行起始 sequence；不足 10,000 行返回 journal 开始。

- [ ] **Step 2: 运行 RED**
  - Run: `pnpm --filter @matou/runtime test -- journal-tail-index.test.ts`

- [ ] **Step 3: 实现 index 和原子 sidecar**
  - sidecar 保存 sequence→累计行数的稀疏检查点，每 256 frame 一项；写临时文件后 rename。
  - sidecar 损坏时从 raw/gzip segment 流式重建，不阻断其他 session。

- [ ] **Step 4: 写 checkpoint protocol RED 测试**
  - 非法 sessionId、超大 snapshot、倒退 sequence 必须被拒绝；合法 checkpoint 可由 CheckpointManager 读取。

- [ ] **Step 5: 接入 SerializeAddon 和 CheckpointManager**
  - live xterm 在输出静默 500ms、切出前台或完成 replay 后创建 checkpoint；同一 session 同时最多一个 checkpoint in flight。
  - Runtime 只接受不小于已存 watermark 的 checkpoint。

- [ ] **Step 6: 写 replay RED/GREEN**
  - 有 checkpoint：先发 snapshot，再流式发 checkpoint 后至当前的 frame。
  - 无 checkpoint：从 TailIndex 的 10,000 行起点开始。
  - Run: `pnpm --filter @matou/runtime test -- journal-tail-index.test.ts checkpoint-manager.test.ts runtime-server.test.ts`

---

### Task 8: Async Compression and Queryable Cold History

**Files:**
- Create: `apps/runtime/src/journal/journal-compressor.ts`
- Create: `apps/runtime/src/journal/journal-compressor.test.ts`
- Create: `apps/runtime/src/journal/journal-history-reader.ts`
- Create: `apps/runtime/src/journal/journal-history-reader.test.ts`
- Modify: `apps/runtime/src/journal/segment-journal.ts`
- Modify: `apps/runtime/src/runtime-server.ts`
- Modify: `apps/runtime/src/runtime-server.test.ts`
- Modify: `packages/contracts/src/protocol.ts`
- Modify: `packages/contracts/src/protocol.test.ts`

**Dependencies:** Tasks 6 and 7。

**Interfaces:**
- `JournalCompressor.schedule(sessionId, segment): void`，worker thread 中 gzip，fsync 临时文件，rename 成 `.mtj.gz`，最后删除 raw。
- `JournalHistoryReader.page({sessionId,beforeSequence,lineLimit})` 返回最多 1,000 行的 async iterable page。
- `JournalHistoryReader.search({sessionId,query,beforeSequence,limit})` 返回 sequence、行预览和是否还有更早匹配。
- 新增 RPC `terminal.history-page` 和 `terminal.history-search`；结果不得一次物化完整 session 历史。

- [ ] **Step 1: 写 compressor RED 测试**
  - 压缩前后 frame/CRC/sequence 完全一致；进程在 temp write、rename、raw delete 三个阶段中断后，重启只选择一个有效副本且不丢 frame。

- [ ] **Step 2: 写 history reader RED 测试**
  - 查询跨 raw/gzip 边界；Unicode query 跨 frame；倒序分页无重复、无跳号；损坏 segment 只产生该 session 的 gap 结果。

- [ ] **Step 3: 实现 worker compressor 和流式 reader**
  - 禁止在 Runtime 主线程使用 `gzipSync/gunzipSync`。
  - reader 使用 zlib stream 和 frame iterator，内存上限由单页 1,000 行控制。

- [ ] **Step 4: 接入 RuntimeServer RPC 并跑 GREEN**
  - Run: `pnpm --filter @matou/runtime test -- journal-compressor.test.ts journal-history-reader.test.ts runtime-server.test.ts`

- [ ] **Step 5: 性能验收**
  - 6 session × 320 MiB 输出，至少产生 64 MiB/session 冷压缩历史。
  - 压缩期间 Runtime event-loop max delay < 50ms；查询第一页峰值 RSS 增量 < 64 MiB；其他 session 输入回显 p95 < 100ms。

---

### Task 9: Bounded PTY Backpressure and Single-Session Journal Pause

**Files:**
- Create: `apps/runtime/src/session/session-execution-pause.ts`
- Create: `apps/runtime/src/session/session-execution-pause.test.ts`
- Modify: `apps/runtime/src/session/pty-session.ts`
- Modify: `apps/runtime/src/session/pty-session.test.ts`
- Modify: `apps/runtime/src/runtime-server.ts`
- Modify: `apps/runtime/src/runtime-server.test.ts`
- Modify: `packages/contracts/src/protocol.ts`
- Modify: `packages/contracts/src/protocol.test.ts`
- Modify: `apps/desktop/src/renderer/src/hierarchy/TerminalPane.tsx`
- Modify: `apps/desktop/src/renderer/src/hierarchy/TerminalPane.test.tsx`

**Dependencies:** Tasks 6–8。

**Interfaces:**
- `SessionExecutionPause.pause(pid, reason)` / `resume(pid)`；POSIX 对 PTY process group 发 SIGSTOP/SIGCONT。
- `PtySession` 输出待写队列上限 2 MiB；信用耗尽或待写达到上限时暂停 node-pty read，恢复至 512 KiB 后继续。
- 新 runtime message `terminal.journal-fatal`：`sessionId`、稳定错误码 `DISK_FULL | PERMISSION | IO`、`paused: true`。
- 新 renderer message `terminal.resume-after-journal-error`，仅在 journal 探测写成功后 SIGCONT。

- [ ] **Step 1: 写 backpressure RED 测试**
  - fake PTY 瞬间输出 64 MiB、consumer 不 ACK；断言待写内存不超过 2 MiB、PTY read 被暂停、其他 session 正常发送。

- [ ] **Step 2: 写 ENOSPC RED 测试**
  - journal 第二次 append 抛 ENOSPC；断言该 session 进入 fatal paused、后续输入被拒绝、onExit 不冒充正常完成、另一 session 可写可 ACK。

- [ ] **Step 3: 运行 RED**
  - Run: `pnpm --filter @matou/runtime test -- pty-session.test.ts session-execution-pause.test.ts runtime-server.test.ts`

- [ ] **Step 4: 实现有界队列与暂停适配器**
  - 所有 `#writeChain` 拒绝在 session 内消费并转换为一次 fatal 状态；不得形成 unhandled rejection。
  - Runtime 本身继续 ready；只从 attached/focused 状态移除故障 session 的可写权限。

- [ ] **Step 5: 实现每卡磁盘故障遮罩**
  - 文案固定为“终端已暂停：输出记录写入失败”；提供“重试写入”按钮。
  - 重试先写探针 frame 并 fsync，成功后恢复执行；失败保持暂停且更新原因。

- [ ] **Step 6: 真实 PTY 验收**
  - 两个 session 同时运行计数器；只让 A 的 journal 注入 ENOSPC。
  - A 的计数停止增长，B 持续增长；恢复可写后点击重试，A 从暂停点继续而非重启新 PID。

---

### Task 10: Foreground xterm Binding Without Stopping Background PTYs

**Files:**
- Modify: `apps/desktop/src/renderer/src/hierarchy/HierarchyShell.tsx`
- Modify: `apps/desktop/src/renderer/src/hierarchy/HierarchyShell.test.tsx`
- Modify: `apps/desktop/src/renderer/src/hierarchy/TerminalPane.tsx`
- Modify: `apps/desktop/src/renderer/src/hierarchy/TerminalPane.test.tsx`
- Modify: `apps/desktop/src/renderer/src/terminal/TerminalSurface.tsx`
- Modify: `apps/desktop/src/renderer/src/terminal/TerminalSurface.test.tsx`
- Modify: `apps/desktop/src/renderer/src/hierarchy/hierarchy.css`
- Test: `tests/e2e/prd-04-session-recovery.spec.ts`
- Test: `tests/e2e/session-canvas-navigation.spec.ts`

**Dependencies:** Task 7 提供快速重新绑定；Task 9 保证解绑期间背压有界。

**Interfaces:**
- `TerminalPane` 新增 `foreground: boolean`，含义是“当前 active Scene/current level”，与 `visible/cardVisible` 分离。
- `TerminalSurface` 新增 `bound: boolean`；`bound=false` 不创建 Terminal/addons/ResizeObserver，不 attach MessagePort consumer，但不 dispose Runtime PTY。

- [ ] **Step 1: 写组件 RED 测试**
  - inactive Scene 的 pane 存在历史卡片 DOM，但没有 `.xterm`；切为 active 后创建 xterm。
  - 同一 active Scene 当前横向列表的 20 个 sibling，无论 `data-in-viewport`，均有 xterm binding。

- [ ] **Step 2: 运行 RED**
  - Run: `pnpm --filter @matou/desktop test -- HierarchyShell.test.tsx TerminalPane.test.tsx TerminalSurface.test.tsx`

- [ ] **Step 3: 实现 foreground/bound 传播**
  - `HierarchyShell` 只根据 active Scene/current level计算 foreground；不得把 SessionCarousel 的 cardVisible 传成 bound。
  - 修正 `[hidden]` 样式，但不以 hidden attribute 决定 Runtime PTY 生命周期。

- [ ] **Step 4: 真实 Electron 验收**
  - Scene A 创建 20 sibling，Scene B 创建 20 sibling。
  - 在 A：20 个 xterm/20 个稳定 PID；切到 B：A 的 xterm 数为 0，A 的 PID 仍存在并继续输出；B 为 20 个 xterm。
  - 切回 A 后每卡使用 checkpoint/tail 恢复，PID 不变；横向滚出视野的 A 卡 xterm 不减少。

---

### Task 11: Per-Card Recovery Overlay and Independent Readiness

**Files:**
- Modify: `apps/desktop/src/renderer/src/terminal/TerminalSurface.tsx`
- Modify: `apps/desktop/src/renderer/src/terminal/TerminalSurface.test.tsx`
- Modify: `apps/desktop/src/renderer/src/hierarchy/TerminalPane.tsx`
- Modify: `apps/desktop/src/renderer/src/hierarchy/TerminalPane.test.tsx`
- Modify: `apps/desktop/src/renderer/src/hierarchy/hierarchy.css`
- Test: `tests/e2e/prd-04-session-recovery.spec.ts`

**Dependencies:** Tasks 7–10。

**Interfaces:**
- Extend `RuntimeStatus` with `recovering-instant`。
- 遮罩只覆盖所属 `.terminal-pane` 内容区域；header、其他卡、Scene tabs、横向滚动保持可操作。
- 收到 checkpoint/tail 的 `terminal.replay-complete` 后立即撤遮罩；冷历史仍在后台可查询，不延长遮罩。

- [ ] **Step 1: 写 RED 组件测试**
  - 两卡中 A recovering、B streaming；断言仅 A 有 `aria-label="正在恢复终端：A"`，B textarea 可 focus/input。
  - A replay complete 后遮罩消失且焦点不自动从 B 跳回 A。

- [ ] **Step 2: 运行 RED**
  - Run: `pnpm --filter @matou/desktop test -- TerminalPane.test.tsx TerminalSurface.test.tsx`

- [ ] **Step 3: 实现最小 overlay/status**
  - 遮罩文案固定为“正在恢复最近 10,000 行…”；不显示全局 loading。
  - active card 恢复完成后仅当它在恢复前拥有焦点才恢复 helper textarea 焦点。

- [ ] **Step 4: 真实 Electron 验收**
  - 12 卡各写入至少 10,500 行后重启；人为延迟其中一张卡 replay。
  - 其余卡可在 1 秒内输入并回显；慢卡独立显示遮罩；完成后最后 10,000 行边界正确。

---

### Task 12: Search Compressed History Through the Existing Terminal Search

**Files:**
- Modify: `apps/desktop/src/renderer/src/hierarchy/TerminalSearchBar.tsx`
- Modify: `apps/desktop/src/renderer/src/hierarchy/HierarchyShell.tsx`
- Modify: `apps/desktop/src/renderer/src/terminal/TerminalSurface.tsx`
- Modify: `apps/desktop/src/renderer/src/terminal/TerminalSurface.test.tsx`
- Modify: `apps/desktop/src/renderer/src/runtime/RuntimeClient.ts`
- Modify: `apps/desktop/src/renderer/src/runtime/RuntimeClient.test.ts`
- Test: `tests/e2e/prd-04-session-recovery.spec.ts`

**Dependencies:** Task 8 history RPC；Task 11 overlay。

**Interfaces:**
- 搜索顺序：先 xterm 最近 10,000 行；无匹配或继续“上一个”越过最早 live match 时调用 `terminal.history-search`。
- 冷历史命中在搜索栏显示“更早输出 · N”；选中后只加载命中前后各 250 行的只读历史视图，不替换 live PTY。
- 按 Escape 或“返回实时终端”恢复 live xterm；PTY 输入和输出在历史视图期间继续，回到 live 后通过 sequence 补齐。

- [ ] **Step 1: 写 RED 测试**
  - query 只在压缩 segment 中存在时显示 archived result；进入历史视图后输入禁用且 live sequence 继续累计；退出后补齐并聚焦原终端。

- [ ] **Step 2: 运行 RED**
  - Run: `pnpm --filter @matou/desktop test -- TerminalSurface.test.tsx RuntimeClient.test.ts`

- [ ] **Step 3: 实现最小历史视图**
  - 使用独立只读 xterm instance 或清晰隔离的 buffer；不得把历史 frame 写入 live xterm 导致当前 shell 屏幕状态改变。

- [ ] **Step 4: 真实 Electron 验收**
  - 输出 20,000 行，确保目标 marker 只在第 500 行并已进入 gzip。
  - 重启即时只恢复最后 10,000 行；现有搜索入口仍找到第 500 行；退出历史视图后实时 shell PID、cwd 和当前输入行不变。

---

### Task 13: Unicode, Long-Line, Alternate-Screen, Gap, and Corruption Recovery

**Files:**
- Modify: `apps/runtime/src/journal/journal-history-reader.test.ts`
- Modify: `apps/runtime/src/runtime-server.test.ts`
- Modify: `apps/desktop/src/renderer/src/terminal/TerminalSurface.test.tsx`
- Test: `tests/e2e/prd-04-session-recovery.spec.ts`
- Test: `tests/e2e/session-canvas-recovery.spec.ts`

**Dependencies:** Tasks 7–12。

**Interfaces:** 使用已定义 checkpoint、tail、gap、history view，不新增第三套恢复路径。

- [ ] **Step 1: 增加 RED fixtures**
  - UTF-8 四字节字符跨 frame；20 MiB 单行；CR-only progress；100k ANSI color transitions；进入/退出 alternate screen；运行中停留 alternate screen 后杀 Runtime；segment CRC 损坏；tail sidecar 截断。

- [ ] **Step 2: 运行 RED**
  - Run: `pnpm --filter @matou/runtime test -- journal-history-reader.test.ts runtime-server.test.ts`
  - Run: `pnpm --filter @matou/desktop test -- TerminalSurface.test.tsx`

- [ ] **Step 3: 只修复共用 decoder/replay 路径**
  - 所有流使用持久 `TextDecoder(..., {stream:true})`；gap 明确终止当前 replay 并显示该卡恢复异常，不影响 live PTY attach。
  - 超长行按 chunk 流式传输，不在 history reader 中拼接整 session 或整行副本。

- [ ] **Step 4: 真实 PTY 验收**
  - 使用 `less`/`vim`/Python ANSI fixture 验证 alternate screen 恢复后的光标、当前屏、退出后的 shell prompt。
  - 损坏一个 session 的 segment 后，其他 11 个 session 正常恢复；损坏卡显示独立异常和“返回实时终端”。

---

### Task 14: Full-System Scale Gate and Regression Matrix

**Files:**
- Create: `tests/e2e/fixtures/runtime-stress-fixture.ts`
- Create: `tests/e2e/runtime-robustness-scale.spec.ts`
- Modify: `package.json`
- Modify: `docs/acceptance/` 下对应的运行证据清单文件

**Dependencies:** Tasks 1–13 全部完成。

**Interfaces:**
- `runtimeStressFixture` 提供 `createSessions(count)`、`startOutput(sessionId, bytesPerSecond)`、`runtimeMetrics()`、`rendererMetrics()`、`eventLoopDelay()`、`resizeMessageCount()`，全部读取真实 Electron/PTY 状态。

- [ ] **Step 1: 写规模 gate RED 测试**
  - 20 个当前层 sibling 同时 1 MiB/s 输出 30 秒；横向连续滚动；切 Scene 解绑/重绑；Runtime crash/restart；App blur/focus；大粘贴；resize；注入单 session ENOSPC。

- [ ] **Step 2: 固定验收阈值**
  - 前台 20 sibling xterm 全部保留；inactive Scene xterm 为 0、PTY PID 全部存活。
  - Runtime event-loop p99 < 50ms、max < 200ms（冷压缩 worker 不阻塞主循环）。
  - 连续输出期间 Renderer requestAnimationFrame p95 < 32ms；横向滚动无超过 100ms 的 long task。
  - 单 session 未 ACK 时 Runtime 有界队列 ≤ 2 MiB；总 RSS 不随生成字节持续线性增长。
  - 重启后每卡独立遮罩，最近 10,000 行可用；旧 marker 可通过搜索查到。
  - ENOSPC 只暂停目标 PID，其他 19 个 session 输入回显 p95 < 100ms。

- [ ] **Step 3: 运行 RED 并确认失败原因对应未完成行为**
  - Run: `pnpm exec playwright test tests/e2e/runtime-robustness-scale.spec.ts --workers=1 --reporter=line`

- [ ] **Step 4: 在全部任务 GREEN 后运行完整门禁**
  - Run: `pnpm build`
  - Run: `pnpm typecheck`
  - Run: `pnpm test`
  - Run: `pnpm exec playwright test tests/e2e/runtime-robustness-scale.spec.ts tests/e2e/terminal-channel.spec.ts tests/e2e/prd-04-session-recovery.spec.ts tests/e2e/session-canvas-navigation.spec.ts tests/e2e/prd-01-agent-notifications.spec.ts --workers=1`

- [ ] **Step 5: 闭合 reference product 与 Matou 验收证据**
  - 大粘贴：两侧均无提示、终端收到完整内容。
  - 普通/空格拖入：可见文本逐字符一致；特殊字符额外附安全执行证据。
  - 回焦、每卡恢复遮罩、历史查询、磁盘暂停、后台 PTY 继续等 Matou 新的异常行为记录用户结果、截图和实际 PID/sequence 证据。

---

## Dependency Order and Parallelization

1. 可并行第一组：Task 1、2、3、4、5。
2. Runtime 主链必须串行：Task 6 → Task 7 → Task 8 → Task 9。
3. Renderer 恢复链：Task 7、9 完成后执行 Task 10 → Task 11 → Task 12。
4. Task 13 等待 Tasks 7–12。
5. Task 14 最后执行，作为统一发布门禁。

并行任务不得同时修改 `TerminalSurface.tsx`：Task 1、2、3、4 即使逻辑独立，也应分别完成 review 后顺序合入，避免共享文件冲突。Runtime 任务不得让多个 agent 同时修改 `runtime-server.ts` 或 `protocol.ts`。

## Product Acceptance Summary

- 用户切换 Scene 后，后台命令继续；不可见 Scene 不消耗 xterm 渲染资源。
- 同一横向层滚出视野的会话仍是前台，会继续即时显示和保持 xterm 状态。
- 重启时每卡单独恢复，用户可立即操作已恢复卡；没有全屏统一阻塞。
- 默认只恢复最近 10,000 行；更旧输出仍从原搜索入口查到。
- 每会话 256 MiB raw 热历史之外的数据压缩保存；压缩不造成终端明显停顿。
- 磁盘故障只暂停目标会话，用户释放空间后可在原 PID 上继续。
- 切出/切回 App 后，焦点回到原控件。
- 大粘贴没有额外提示或步骤；Unicode 内容完整。
- 路径拖入普通场景与 reference product 一致，特殊路径不改变 shell 命令结构。
- resize、通知和大量会话均有确定的容量、频率和真实性能门禁。
