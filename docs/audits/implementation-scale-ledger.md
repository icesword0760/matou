# 大规模实施台账

| Task | 状态 | 当前证据 | 后续门槛 |
|---|---|---|---|
| 1 真实规模基准与指标合同 | 已完成 | 50/200/1000 真实 Electron harness；5000 深链、10000 DAG、真实 Journal seed；2 warm + 5 measured；PID/RSS/PTY/DOM/longtask/statement；退出 PID 清理 | 不应用最终容量门槛 |
| 2 迭代 Session/布局索引 | 待实施 | Task 1 seed 可复用 | 5000 深链无栈溢出，结构计算 p95 ≤ 50ms |
| 3 projection 集合查询 | 待实施 | statement measurement window 已可 reset/read | 1000/1000 snapshot p95 ≤ 75ms，单次 statements ≤ 40 |
| 4 增量投影与结构共享 | 待实施 | frame/RSS/DOM contract 可复用 | 10000 增量 batch p95 ≤ 16.7ms |
| 5 恢复调度器 | 待实施 | Runtime 权威 PTY 指标可复用 | 并发 ≤ 4，活动 Scene 优先且后台不饿死 |
| 6 Journal range/checkpoint | 待实施 | `journalBytesPerSession` 真实 Journal V2 seed | 恢复时间和峰值 RSS 门槛 |
| 7–11 前台挂载、Scene 切换、Carousel、DAG | 待实施 | DOM、PTY PID、frame、longtask contract 可复用 | 按实施计划逐项闭合 |
| 12 发布容量收口 | 待实施 | Task 1 只提供 baseline | 50/200/1000、5000、10000 全部硬门槛 |

## Task 1 RED → GREEN 记录

1. RED：新增 `scale-benchmark.spec.ts` 后运行 build + Playwright；build 成功，测试因 `scale-database` / `scale-metrics` 与 fixture 扩展不存在而失败。
2. GREEN：新增确定性 SQLite/Journal seed、真实 Electron frame/longtask/DOM/RSS 采集、Runtime 权威 PID/PTY/statement 请求链和进程退出审计。
3. 定向单元：RuntimeDatabase statement reset/read、RuntimeSessionRegistry PTY authority、RuntimeHost 相关请求全部通过。
4. 真实规模：`pnpm test:scale --grep "harness" --reporter=line`，3 passed。
5. Seed：5000 深链 + 10000 DAG 重复 seed 测试通过；Journal payload 重复 seed 测试通过。
6. Post-review：seed 通过 Runtime storage authority 访问 SQLite，dependency-boundary 通过；v22 environment binding 存在时 `test:scale` 5/5 通过。
