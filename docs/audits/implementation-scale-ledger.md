# 大规模实施台账

| Task | 状态 | 当前证据 | 用户结果 |
|---|---|---|---|
| 1 真实规模基准与指标合同 | **已闭合** | 50/200/1000 Electron、5000 深链、10000 DAG、PID/RSS/PTY/DOM/SQL/Long Task 指标 | 性能退化可被自动门禁拦截 |
| 2 Session / 布局索引 | **已闭合** | `session-graph-index`、`scene-layout-index`、迭代遍历与 cycle guard | 深层关系恢复不栈溢出，列表操作不重复全图扫描 |
| 3 Projection 集合查询 | **已闭合** | 批量 repository 读取；规模门禁记录 SQL statement profile | 大量会话恢复不按实体逐条查询 |
| 4 增量投影与结构共享 | **已闭合** | 正常事件增量应用；仅 gap/reconnect 重建；DAG 使用事件订阅 | 后台输出和关系更新不触发全窗口快照风暴 |
| 5 解绑视图但保留 PTY | **已闭合** | `terminal.view-detach`、consumer 引用计数、Runtime registry | 离开 Scene 后任务继续，返回时恢复原会话 |
| 6 Journal range/checkpoint | **已闭合** | `iterateFrames()` 范围流式重放；checkpoint + tail index；6×320 MiB 压力门禁 | 长历史恢复受控，不先物化完整 Journal |
| 7 有界恢复队列 | **已闭合** | 并发 4、活动/前台优先、8:1 防饥饿、失败隔离 | 当前现场先可用，其余会话不会同时抢占资源 |
| 8 Scene 挂载与整卡 Loading | **已闭合** | 非活动 Scene 卸载 xterm；当前横向层保留 consumer 与 VT model；逐卡遮罩 | 恢复不挡住整个 App，滑出视野的同级会话仍是前台 |
| 9 统一节点移除范围 | **已闭合** | `node-only` / `node-and-descendants`、Worktree 影响计数、重启不复活 | 用户可判断移除范围和后果 |
| 10 Carousel / Resize | **已闭合** | 卡片 DOM 窗口化与 VT model 解耦；60Hz resize；81 会话连续离屏往返 3 次 | 1000 会话仍可平滑滚动，返回卡片不丢行或重行 |
| 11 DAG 迭代布局与可视窗口 | **已闭合** | rAF 手势、增量事件、远层聚合；真实 10000 节点门禁 | 大图可搜索、缩放、平移和定位 |
| 12 发布容量收口 | **已闭合** | 完整 unit/type/build/E2E/scale/stress/package 矩阵 | 当前 macOS 内部验收候选版达到本轮发布门槛 |

## 状态口径

- **已完成**：仅表示该任务自身的合同与证据完整，不替代 Task 12 的发布级结论。
- **已实现（专项验证）**：核心用户结果已落地且专项门槛有真实代码/测试或验收报告支撑，仍需进入最终全量门禁。
- **部分实现**：已有可测收益，但仍缺少原任务的一部分，或与已确认产品规则存在冲突。
- **待最终门禁**：专项数字已存在，完整版本的一致性、交互和打包门禁仍未同时闭合。

## 已验证规模结果与边界

权威专项记录为 `docs/acceptance/scale-dag-performance.md`。当前可据此确认的用户结果：

- 最终门禁中，32 个工作空间、249 个事项、1,992 个会话和 249 个画布恢复用时 1,638 ms；跨工作空间事项切换 325 ms，均低于预算。
- 1,000 会话持续滚动 frame p95 9.1 ms、max 16.7 ms；卡片 DOM 窗口化不再卸载前台终端的 consumer 与 xterm VT model。
- 真实 81 会话离屏终端保持同一 PID，返回时只从模型最后已显示序列之后补齐；连续 3 次未出现丢行或重行。
- 10,000 节点 DAG 首次可操作 276 ms、连续手势 frame p95 9.1 ms、远端搜索 16.38 ms；远层聚合保持总数和关键状态守恒。
- 完整数字、压力异常复验和发布边界见 `docs/audits/2026-09-02-internal-hardening-final.md`。

## 最终门禁结论

Task 12 已在同一集成分支完成以下门禁：

1. Task 4、6、7、8、9、10、11 均有生产代码、单元测试和真实场景回归。
2. `pnpm typecheck`、`pnpm test`、`pnpm test:e2e`、`pnpm test:scale`、压力测试和打包运行全部通过。
3. 自动 Electron 窗口均位于副屏 `Color LCD`，测试退出后清理 Runtime 与 PTY。
4. 原始规模、p50/p95/max、RSS、DOM、SQL、PTY、Long Task 与交互结果汇总在 `2026-09-02-internal-hardening-final.md`。

## Task 1 RED → GREEN 记录

1. RED：新增 `scale-benchmark.spec.ts` 后运行 build + Playwright；build 成功，测试因 `scale-database` / `scale-metrics` 与 fixture 扩展不存在而失败。
2. GREEN：新增确定性 SQLite/Journal seed、真实 Electron frame/longtask/DOM/RSS 采集、Runtime 权威 PID/PTY/statement 请求链和进程退出审计。
3. 定向单元：RuntimeDatabase statement reset/read、RuntimeSessionRegistry PTY authority、RuntimeHost 相关请求全部通过。
4. 真实规模：`pnpm test:scale --grep "harness" --reporter=line`，3 passed。
5. Seed：5000 深链 + 10000 DAG 重复 seed 测试通过；Journal payload 重复 seed 测试通过。
6. Post-review：seed 通过 Runtime storage authority 访问 SQLite，dependency-boundary 通过；v22 environment binding 存在时 `test:scale` 5/5 通过。
