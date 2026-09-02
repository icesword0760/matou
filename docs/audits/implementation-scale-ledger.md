# 大规模实施台账

| Task | 状态 | 当前证据 | 后续门槛 |
|---|---|---|---|
| 1 真实规模基准与指标合同 | 已完成 | 50/200/1000 真实 Electron harness；5000 深链、10000 DAG、真实 Journal seed；2 warm + 5 measured；PID/RSS/PTY/DOM/longtask/statement；退出 PID 清理 | 不应用最终容量门槛 |
| 2 迭代 Session/布局索引 | **部分实现** | `dag-layout.ts` 已用显式路径栈处理 5,000 深链；`SessionCanvas` 后代遍历已改为显式栈；`dag-layout.test.ts` 与专项报告有 5,000/10,000 结果 | 通用 `session-graph-index` / `scene-layout-index` 尚未落地，Hierarchy/Canvas 仍有多处 `find/filter` 全图扫描 |
| 3 projection 集合查询 | **已实现（专项验证）** | `RuntimeRpcRouter.#snapshot()` 使用批量 repository 读取；500 Session 测试 `<40` statements；1,000 会话实测窗 463 statements，低于专项预算 1,500 | 最终门禁继续观察 snapshot 时延与序列化体积 |
| 4 增量投影与结构共享 | **待实施** | `RuntimeProjectionStore.applyBatch()` 可应用部分事件，但 `HierarchyShell` 在每个 `events.batch` 后仍调用 `refresh()`，commands 仍通过 `afterMutation` 全量刷新 | 正常事件/命令不再 full snapshot；gap/reconnect 才单飞重建；10,000 batch p95 ≤ 16.7ms |
| 5 解绑视图但保留 PTY | **已实现（定向验证）** | `terminal.view-detach` 协议、RuntimeClient listener 引用计数、Runtime headless start/reattach tests | 与 Task 8 一起验证非活动 Scene xterm=0、PID/Journal 连续 |
| 6 Journal range/checkpoint | **待实施** | checkpoint/tail 语义已落地，但 `RuntimeServer.#replay()` 仍调用 `readFrames()` / `readSessionFrames()` 并 `filter`，会先物化完整历史 | 32/256 MiB 从 checkpoint 后按 segment 范围流式读取；额外 RSS ≤16 MiB |
| 7 有界恢复队列 | **部分实现** | `RuntimeSessionRecoveryScheduler` 有并发上限、切换优先级、8:1 防饥饿、失败隔离 tests；Runtime 配置 concurrency=4 | reconnect 仍会直接重发 Renderer 已登记 terminal；初始前台按整个 Scene 而不是当前父级横向列表；需修正后做真实恢复门禁 |
| 8 Scene 挂载与整卡 Loading | **部分实现** | `TerminalPane` 仅在 `foreground && ready` 挂载 xterm；queued/restoring/failed 使用整卡遮罩；组件 tests 覆盖单卡隔离 | `SessionCarousel` 在同级会话超过 80 时窗口化，导致横向列表离屏会话不再保持 xterm 绑定，与已确认前台规则冲突 |
| 9 统一节点移除范围 | **待实施** | 当前 `removeSessionBranch` 仍在 `includeDescendants=false` 且存在后代时抛 `Session has descendants` | “仅当前节点”需事务重连直接子节点；“当前及后代”删除完整子树，并统一 UI 入口 |
| 10 Carousel / Resize | **部分实现（专项验证）** | Resize 已合并并有真实 16 会话 / 60Hz 验收；Carousel 具有稳定 hover/geometry 与 1,000 会话窗口化；专项报告记录滚动 p50/p95 6.9/7.7 ms | 先解决 Task 8 的前台规则冲突，再决定能同时满足“大列表 DOM 上限”和“所有同级 xterm 保持绑定”的承载方式 |
| 11 DAG 迭代布局与可视窗口 | **部分实现（专项验证）** | `layoutGraph()` 迭代深度推导、`nodeById/nodesByDepth`；深 DAG 卡片 DOM ≤5；报告记录 5,000 深链 12 ms、10,000 节点 layout p95 5.39 ms | 尚无 branch/layer aggregate render model；pan/zoom 仍逐事件 setState；`DagWindowApp` 仍 500ms 轮询并 stringify 全图 |
| 12 发布容量收口 | **待最终门禁** | `scale-dag-performance.md` 已记录专项测量：1,000 会话 DOM 151、SQL 463、层级恢复 445–516 ms、切换 79 ms、DAG 与滚动均低于专项预算 | 需在包含全部集成修复的同一提交上完成 typecheck、unit、完整真实 E2E、scale、打包运行和清理审计，再形成发布结论 |

## 状态口径

- **已完成**：仅表示该任务自身的合同与证据完整，不替代 Task 12 的发布级结论。
- **已实现（专项验证）**：核心用户结果已落地且专项门槛有真实代码/测试或验收报告支撑，仍需进入最终全量门禁。
- **部分实现**：已有可测收益，但仍缺少原任务的一部分，或与已确认产品规则存在冲突。
- **待最终门禁**：专项数字已存在，完整版本的一致性、交互和打包门禁仍未同时闭合。

## 已验证规模结果与边界

权威专项记录为 `docs/acceptance/scale-dag-performance.md`。当前可据此确认的用户结果：

- 32 个工作空间、249 个事项、1,992 个会话和 249 个画布从真实 SQLite 恢复到主画布用时 445–516 ms；跨工作空间事项切换 79 ms。
- 1,000 会话场景把 DOM 从 3,105 降到 151、测量窗 SQL statements 从 30,570 降到 463；持续滚动 p50/p95 为 6.9/7.7 ms，未记录 Long Task。
- 5,000 层关系布局以迭代方式完成，专项记录 12 ms；10,000 节点 DAG layout p50/p95 为 5.21/5.39 ms。
- 上述数字证明专项路径的性能收益，不证明完整产品已达到发布状态。尤其是当前 1,000 会话 DOM 数依赖 Carousel 窗口化，而已确认产品规则要求横向列表中滑出视野的会话仍定义为前台并保持 xterm 绑定；两者必须在 Task 8/10 收口时共同解决，不能用性能数字覆盖产品规则。

## 待最终门禁

Task 12 只有在同一最终提交上满足以下条件后才可改为“已完成”：

1. Task 4、6、7、8、9、10、11 的上述缺口全部有生产代码与回归测试闭合。
2. `pnpm typecheck`、`pnpm test`、`pnpm test:e2e`、`pnpm test:scale` 和打包运行在同一版本全部通过。
3. 真实 Electron 验证继续遵守副屏验收规则，且测试退出后 Electron、Runtime 与 PTY 进程清理完成。
4. 最终证据保留原始规模、p50/p95/max、RSS、DOM、SQL、PTY PID、long task 与交互轨迹；不得只引用本文件中的二次摘要。

## Task 1 RED → GREEN 记录

1. RED：新增 `scale-benchmark.spec.ts` 后运行 build + Playwright；build 成功，测试因 `scale-database` / `scale-metrics` 与 fixture 扩展不存在而失败。
2. GREEN：新增确定性 SQLite/Journal seed、真实 Electron frame/longtask/DOM/RSS 采集、Runtime 权威 PID/PTY/statement 请求链和进程退出审计。
3. 定向单元：RuntimeDatabase statement reset/read、RuntimeSessionRegistry PTY authority、RuntimeHost 相关请求全部通过。
4. 真实规模：`pnpm test:scale --grep "harness" --reporter=line`，3 passed。
5. Seed：5000 深链 + 10000 DAG 重复 seed 测试通过；Journal payload 重复 seed 测试通过。
6. Post-review：seed 通过 Runtime storage authority 访问 SQLite，dependency-boundary 通过；v22 environment binding 存在时 `test:scale` 5/5 通过。
