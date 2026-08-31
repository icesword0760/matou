# 载入 Claude Code 会话实现计划

**目标：** 用户从任意卡片顶栏打开会话管理器，检索当前工作空间内的可恢复 Claude Code 会话，并在保持 Matou 节点位置、层级和焦点不变的前提下，将所选会话及其权限载入当前卡片。

**产品边界：** 列表只展示 JSONL 记录中 `cwd` 与当前卡片工作目录一致的会话。已经被另一张仍在使用的 Matou 卡片绑定的 provider 会话不作为“可恢复”候选，避免同一 Claude 对话同时被两个活动终端写入。载入失败时，当前卡片的进程、类型和绑定保持原状。

**实现结构：** Runtime 负责扫描和解析 `~/.claude/projects`、全文检索、可恢复性校验与原子绑定；Renderer 只展示查询结果和发起显式载入。载入成功后权威投影把同一 Matou Session 的 profile 切到 `claude-code`，现有 Terminal attach 流程以稳定 sessionId 替换进程并使用 `--resume` 与原权限参数启动。

---

## 任务 1：定义会话目录契约

**文件：**
- 修改：`packages/contracts/src/protocol.ts`
- 修改：`apps/desktop/src/renderer/src/hierarchy/hierarchy-types.ts`

1. 为 Runtime RPC 增加 `claude-sessions.list`、`claude-sessions.detail`、`claude-sessions.load`。
2. 定义会话摘要、命中片段、预览事件、权限模式和载入结果的共享类型。
3. Typecheck contracts，确保 Renderer/Runtime 使用同一边界。

## 任务 2：测试先行实现 Claude 本地会话目录

**文件：**
- 新增测试：`apps/runtime/src/session/claude-session-catalog.test.ts`
- 新增实现：`apps/runtime/src/session/claude-session-catalog.ts`

1. 先写失败测试：仅匹配当前 cwd；解析标题、时间、模型、权限；跳过损坏行；全文检索消息与 tool use；返回精确事件索引和上下文片段。
2. 运行单测并确认因模块缺失或行为缺失而失败。
3. 实现路径编码、JSONL 流式读取、规范化消息和稳定排序。
4. 再跑单测，覆盖 default/acceptEdits/plan/bypassPermissions。

## 任务 3：测试先行实现原位载入事务

**文件：**
- 修改测试：`apps/runtime/src/session-canvas/provider-mode-service.test.ts`
- 修改实现：`apps/runtime/src/session-canvas/provider-mode-service.ts`

1. 先写失败测试：Shell 和 Claude 卡片都可载入；旧目标绑定失效；所选 provider identity 绑定到同一 Matou session；权限写入 metadata；节点关系、scene membership、focusedSessionId 不变。
2. 增加冲突测试：所选 provider identity 被另一活动卡片占用时拒绝，且目标卡片完全不变。
3. 实现单事务 `loadClaudeSession`，发出 mode/restore 事件并返回更新后的图。
4. 跑 ProviderModeService 测试确认原子性。

## 任务 4：测试先行接通 RPC 与运行切换

**文件：**
- 修改测试：`apps/runtime/src/rpc/runtime-rpc-router.test.ts`
- 修改：`apps/runtime/src/rpc/runtime-rpc-router.ts`
- 修改：`apps/runtime/src/index.ts`

1. 先写失败测试：list/detail 只接受目标 Session 的权威 cwd；load 使用目录解析出的权限，不接受 Renderer 伪造权限；载入后投影类型为 Claude。
2. 给 Router 注入可测试的 Claude projects root，接入目录服务和载入事务。
3. 让载入成功触发已有投影刷新；Renderer profile 改变后沿现有 RuntimeServer profile replacement 路径结束旧 PTY 并启动 `claude --resume`。
4. 跑 Router 与 RuntimeServer 相关测试。

## 任务 5：测试先行实现会话管理浮框

**文件：**
- 新增测试：`apps/desktop/src/renderer/src/session-canvas/SessionLoaderDialog.test.tsx`
- 新增实现：`apps/desktop/src/renderer/src/session-canvas/SessionLoaderDialog.tsx`
- 修改样式：`apps/desktop/src/renderer/src/session-canvas/session-canvas.css`

1. 先写失败测试：居中两栏、单一紧凑搜索、聚合结果、片段跳转、右侧命中上下切换、显式载入、行点击只预览、加载错误内联保留浮框。
2. 实现 180ms 查询防抖、请求序列防旧响应覆盖、键盘上下选择、Escape 关闭、⌘F 切换当前会话范围。
3. 对运行中卡片增加一次明确确认；其他状态直接载入。
4. 跑组件测试并检查窄窗口布局。

## 任务 6：将入口接入所有卡片并保持画布状态

**文件：**
- 修改测试：`apps/desktop/src/renderer/src/hierarchy/TerminalPane.test.tsx`
- 修改：`apps/desktop/src/renderer/src/hierarchy/TerminalPane.tsx`
- 修改：`apps/desktop/src/renderer/src/hierarchy/HierarchyShell.tsx`
- 修改：`apps/desktop/src/renderer/src/hierarchy/hierarchy-commands.ts`

1. 先写失败测试：Shell、Claude 顶栏均显示“载入会话”；点击只打开管理器；成功后关闭并将焦点还给同一卡片。
2. 接入 list/detail/load commands，并由 HierarchyShell 承载浮框状态以保证相对会话区域居中。
3. 保存打开前的 sessionId/sceneId；成功后重新聚焦同一 sessionId，不改 SessionCanvas 的 level/reveal/scroll 状态。
4. 跑 Desktop 单测。

## 任务 7：集成验收与提交

**文件：**
- 新增：`tests/e2e/session-load-existing-claude.spec.ts`
- 修改：`package.json`（将新用例加入 `test:e2e`）

1. 用临时 Claude projects root 构造 default 与 bypassPermissions 会话。
2. 验证打开、搜索、片段定位、载入、同节点保持、失败不变和权限继承。
3. 运行目标单测、`pnpm typecheck`、目标 E2E 与构建。
4. 检查 `git diff --check`，提交到 `codex/load-session-mockup`。
