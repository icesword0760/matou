# Model Provider Routing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
> 本项目执行方式优先采用 agent-foreman；Claude 定方案并验收，Codex 实现。用户已授权连续推进，不重复请求阶段批准。不得自动 commit。

**Goal:** 让用户在码头为 Claude Code 的 Opus、Sonnet、Haiku 独立指定已有供应商与模型，实际请求使用正确目标，现有进程不被保存操作改变。

**Architecture:** 在现有 agent.modelPreferences 增加可选分配，使用专用局部保存 RPC；新进程启动时捕获目标快照。复用 Claude 请求网关与订阅路由，三个档位使用稳定的虚拟模型 ID，按精确模型分流，跨目标替换鉴权并保留压缩优先级。出口监控扩展为同一会话关联多个官方目标，不新增网关、数据库或依赖。

**Tech Stack:** TypeScript / React / Electron / Node HTTP / SQLite preferences / Vitest / Playwright，本地现有订阅 router。

**Spec:** `docs/superpowers/specs/2026-11-17-model-provider-routing-design.md`

## Global Constraints
- 仅已有 Claude Code 可用供应商、官方账号和订阅账号；不新增任意 OpenAI API 转换。
- 当前进程保持启动快照；新建、停止后恢复、fork、崩溃重建等新进程读取新分配。attach 活进程不变。不引入跨重启永久绑定。
- 无设置时保持全部旧行为。保存分配不触发 provider activate、重启或改当前会话绑定。
- 精确账号/模型失败明确报错，不能偷偷回落。绝不发送源供应商密钥给目标。
- 原型视觉基准在原仓库 `docs/prd/mockups/model-provider-routing-2026-11-17/`，只读；不把演示条/假数据带入产品。
- 默认供应商已有主动切换行为保持不变；本文仅规定分配保存不重启。
- 不提交、不发布、不安装 /Applications 应用、不访问真实密钥做测试；外部付费模型请求需另行批准。
- 命名使用自有/中性术语，运行 `pnpm check:identifiers`。

## Review Focus
1. 两个档位用同名模型但不同目标：虚拟档位身份不能丢失。
2. 保存与首次请求并发：旧进程不可在首次请求时读到新 endpoint、model 或分配。
3. 同域不同 key / path：普通和计数请求均必须清除源认证，并使用精确目标认证。
4. 一个会话包含多个受保护出口：A 恢复不能解冻仍受 B 保护的会话，单目标豁免不能放行另一个。
5. 目录获取失败/账号过期/供应商删除：保留选择、明确错误，不把空目录误当有效配置或自动替换。

---

## Task 1: 配置持久化与精确目标准备

**Files**
- Modify: `packages/contracts/src/agent-model-settings.ts`, `packages/contracts/src/protocol.ts`
- Modify: `apps/runtime/src/session/agent-model-settings.ts`, `apps/runtime/src/rpc/runtime-rpc-router.ts`
- Modify: `apps/runtime/src/model-router/subscription-accounts.ts`
- Create: `apps/runtime/src/session/claude-role-assignments.ts`（快照与角色解析，不放UI或HTTP）
- Tests: 对应 `.test.ts`；协议测试按现有结构。

**Interfaces**
- `ClaudeModelRole = 'opus' | 'sonnet' | 'haiku'`
- `ClaudeRoleAssignments = Partial<Record<ClaudeModelRole, { providerId: string; model: string }>>`
- `AgentModelPreferences.claudeRoleAssignments?: ClaudeRoleAssignments`，缺项沿用默认，{} 清空分配。
- `agent-model-settings.patch-assignments` payload `{ assignments: ClaudeRoleAssignments }` 返回 `AgentModelSettingsSnapshot`。
- `AgentModelSettings.patchAssignments(input: { assignments: ClaudeRoleAssignments }): Promise<AgentModelSettingsSnapshot>`。
- `SubscriptionAccounts.prepareModelTarget(providerConfigId: string, model: string, scope?: string)` 返回精确目标的 `lease, baseUrl, headers, model, metadata`，model 为现有 prefix + raw catalog ID，不重写共享 role aliases。
- `ResolvedClaudeRoleSnapshot` 仅 runtime 内：冻结的 preferences、三个角色目标、可选 custom compaction target、实际能力、官方目标 family 集合。凭据不出 runtime、不落盘；API连接信息启动捕获；OAuth允许原账号正常刷新，不允许换账号。

- [ ] 增加测试：旧设置不丢、仅patch分配、三档同名不同provider、恢复默认、未知role/非法输入、失效选择读回、set/patch异步交错不丢分配。
- [ ] 运行新增测试确认红灯，最小实现配置和专用RPC。旧 `set` 只更新原context/compaction字段并保留最新 assignments，校验结束后同步读最新值合并写入；不得跨 await 持有待写旧值。
- [ ] 快照投影或父组件已有 provider/account 信息供UI显示默认和失效状态，不重复轮询额度。订阅缺失模型拒绝；API显式目录缺失保留为不可用，旧无目录自定义model沿用既有能力，不额外探测模型。
- [ ] 实现精确订阅准备并测试：原role mapping之外的合法目录模型可选、不修改共享映射、严格credential与注册目录校验、失败/关闭释放lease。复用原预检，不复制整个 prepareLaunch。
- [ ] 实现角色快照准备与解析测试：冻结endpoint/model及API key、正常OAuth刷新边界、同名模型不逆推角色、默认目标解析、未知能力保留unknown。仅新功能启用走新路径。
- [ ] 运行 `pnpm build:packages` 和本任务相关runtime tests、types及命名门禁，交回接口实现与测试记录，不commit。

## Task 2: 请求路由与新进程集成

**Files**
- Modify: `apps/runtime/src/session/claude-model-routing.ts`, `claude-request-gateway.ts`, `pty-session.ts`
- Modify: `apps/runtime/src/runtime-server.ts`, `apps/runtime/src/session/agent-model-settings.ts`
- Tests: `claude-model-routing.test.ts`, `claude-request-gateway.test.ts`, `runtime-server.test.ts`, subscription fixture tests。

**Interfaces**
- Gateway resolve 输入兼容原调用并增加请求类型 `kind?: 'message' | 'count_tokens'`；count 保留 baseUrl/headers/model，只剥除生成参数覆盖。
- 新角色虚拟ID统一 `matou-role/opus|sonnet|haiku`，与旧 `matou-api`、订阅alias分开；支持 `[1m]`归一化、Fable→Opus。
- 三档启用分配时均保留身份；无分配沿用原路径。主模型按既有启动默认语义解析，不能把所有default强行猜作Opus。非档位显式自定义model保留原供应商语义，不伪装成目标档位。
- 默认语义补充（Task2已裁决）：总provider.model只表示既有启动主模型。未分配且API该档无modelMapping时，捕获该档native resolvedModel并仍发送原供应商；显式mapping/assignment优先。不借官方能力；显式目录不支持时提示具体档位与供应商配置模型，不fallback。

- [ ] 先写消息/计数跨目标fixture测试：两个上游录请求，断言URL、模型、key正确，原key/cookie不泄漏；流式/取消/上游错误不回退。
- [ ] 修改count路由，count不使用custom compaction、不添加effort/thinking/maxOutputTokens；按实际选中role路由。
- [ ] 路由改用启动快照：custom压缩覆盖优先；follow按输入role走实际目标。目标输出max_tokens按metadata限幅，effort以目标provider当前该档设置为基础且校验实际模型能力，不盲传源provider设置。
- [ ] 新进程启动捕获一次preferences并贯穿model恢复、能力、routing和digest；创建网关/lease失败统一释放；晚完成准备遇close立即release。
- [ ] 以已知目标窗口最小值约束会话，尊重偏好但不超过已知能力；有未知时不继承源provider的大窗口声明，使用CLI原生默认且记录unknown；已知最小值仍可作为上限，不声称未知目标安全。保留现有CLI最小支持窗口限制，错误指出对应档位，不擅自放宽。每目标输出单独约束。
- [ ] 更新虚拟role环境/model picker/Fable literal override，保证 /model 和子代理两者都保持档位身份，不改共享订阅aliases。
- [ ] 生命周期测试：保存不变PID与旧路由，新spawn/停止后resume读新配置、attach旧进程保留、同名不同目标、default和Fable、custom/follow压缩、端点修改后旧进程不变。
- [ ] 定点tests并使用本地假上游+隔离CLI验证实际selector和token-count路径；固定router集成可用则跑，无二进制/账号条件则标明未覆盖，不调用真实付费上游。

## Task 3: 多目标出口保护

**Files**
- Modify: `apps/runtime/src/egress/egress-monitor.ts`, `apps/runtime/src/runtime-server.ts`
- Modify: `packages/contracts/src/egress.ts` 及实际消费会话family的renderer小范围逻辑（由查找确认，禁止扩展重构）。
- Tests: `egress-monitor.test.ts`, `runtime-server.test.ts`、必要renderer tests。

**Interfaces**
- `registerSession(sessionId, family: EgressFamily | readonly EgressFamily[])` 兼容现有单值调用，内部去重集合。
- 保留旧wire单family字段兼容，并增加可选 `sessionFamilySets`（名称若现有风格不同可统一改并报告）；新消费逻辑优先集合。
- `assertTargetAllowed(family?: EgressFamily)` 使用现有监控状态与用户override，不发新探测。未启用保护或无family沿用旧行为。

- [x] 先写多family冻结/恢复、豁免、删除清理、单family回归测试。
- [x] 会话关联role与custom压缩实际family集合；任一异常冻结，仅全部可用解冻。禁止伪造sessionId或重复注册覆盖。
- [x] 网关发送前检查对应目标状态，失败回可操作保护提示，上游请求数必须0。复用已有安全时区/隐藏策略；多目标时不凭第三方默认供应商推断官方出口。
- [x] 运行相关egress和生命周期测试，确认既有单目标保护/手动豁免不回归。

Task3 本地fixture交付与未验证边界见 `2026-11-17-model-provider-routing-task3.md`；完成本批不代表整体功能产品验收。

## Task 4: 正式设置界面

**Files**
- Create: `apps/desktop/src/renderer/src/hierarchy/ClaudeRoleAssignments.tsx`, `claude-role-assignments.css`, `ClaudeRoleAssignments.test.tsx`
- Modify: `apps/desktop/src/renderer/src/hierarchy/ModelSwitchSettings.tsx`；必要locale文件沿用项目模式。

**Interfaces**
- 新组件消费现有client、activeProviderId、provider配置与已加载account状态，读snapshot、仅调用patch-assignments；不用整份set覆盖压缩。

- [x] 测试初始默认/持久化值、三行选择、供应商变化清模型、缺模型禁存、保存成功焦点、失败保留草稿、恢复默认需保存。
- [x] 按已确认Mockup插入供应商列表与压缩区之间，复用现有字号/色值/间距/控件；不重新设计，不委派DSH探索。
- [x] 失效账号/模型/已删provider保留原选择并行内反馈；源读失败显示可重试状态，不错误删除选择；默认项显示实际目标。
- [x] 保存文案正式化：成功「模型分配已保存，新会话将使用此配置」；生效说明「新会话使用此分配，正在运行的会话不受影响」。真实错误不是原型演示状态。
- [x] 保持焦点、滚动、键盘Enter/Space操作；中英文一致；窄屏纵排不溢出。
- [x] 运行desktop定点tests；设置保存与压缩/上下文交错集成不互相覆盖。

Task4实现与组件验证已交回；真实键盘等尚未闭合，详见 `docs/prd/model-provider-routing/acceptance.md`。

## Task 5: 全链路验收与交付

**Files**
- Create: `tests/e2e/model-provider-routing.spec.ts`
- Create: `docs/prd/model-provider-routing/acceptance.md`（场景对照矩阵与证据）

- [ ] 本地假上游启动真runtime，配置不同role各发请求，证明不是只改UI；覆盖计数、压缩、自定义子代理档位、异常不回落。
- [ ] 按项目run技能启动隔离开发版，不使用生产用户数据；OpenCLI实操设置，截图默认/编辑/保存/错误；核对键盘、滚动、焦点、真实终端忙时响应。
- [ ] 对照Mockup记录差异；reference product同范围原始运行证据未取得则明确未闭合，不能宣称PRD整体验收通过。
- [ ] 跑build/typecheck与相关回归、命名门禁；执行独立代码审查（实现者不自审），Claude先看代码后看实物。
- [ ] 只报告实际覆盖范围、跳过项和运行方法，不提交/合并/发布/安装，除非用户另有明确授权。

Task5已新增并跑通OpenCLI+真实runtime本地E2E（1项）；不是完整Electron应用验收，未闭合项及后端下一批问题见验收文档，上述整体验收checkbox保持未闭合。

## 执行批次与自审
当前基线192通过，17集成前置跳过、131定点筛选排除；无既有测试失败。第一批Task1，第二批Task2+3按体量分批，第三批Task4+5。接口和快照依赖较强，先串行核心接口；接口稳定后才考虑不重叠目录的UI/运行时独立批次，避免为了并行而并行。每批以可验收代码+测试为交付，不以宽泛“进展”交回。
已自审：规格逐项在Task1-5覆盖；五类Review Focus分别有测试；使用同一assignment字段/patch名称/role前缀；默认和unknown边界已明确，无需再问产品取舍。
