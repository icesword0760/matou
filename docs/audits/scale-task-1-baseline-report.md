# Scale Task 1 基准报告：真实 Electron 规模 Harness

- 日期：2026-09-01
- 状态：Task 1 harness 已闭合；后续性能门槛仍由 Task 2–12 实施
- 运行环境：Apple M4 Pro，14 CPU，48 GiB，macOS 15.6（Darwin 24.6.0）

## 用户与产品结论

Matou 现在具备可重复的 50 / 200 / 1000 Session 真实启动基准。每次运行都先由真实 Electron 完成数据库 migration，再关闭应用、向真实 SQLite 写入固定 ID 和固定时间戳的数据，最后重启真实 Electron、Runtime 与 node-pty。结果不再依赖临时脚本或 Renderer mock。

本任务只建立测量尺，不把现状包装成已经达到发布容量：

1. 三档数据均能出现应用工作区和全部 Session 卡片，并完成 2 次预热、5 次正式测量。
2. 5000 深链、10000 节点 DAG 和可选真实 Journal payload 已有确定性 seed；算法、绘制与恢复门槛仍在后续任务验收。
3. 当前三档启动后 Runtime 权威 PTY 数均为 1。它证明现有实现仍只为可见卡片保留终端，不符合“当前横向列表全部 Session 都是前台”的已确认目标；后续挂载/恢复任务需让该指标随前台 Session 合同变化。
4. 1000 Session 的 Renderer RSS 和 DOM 已明显上升，SQLite statement 总数也随 Session 数增长。Task 1 将其保留为 baseline evidence；集合查询、结构共享与投影优化完成前，不以本次数据宣称容量达标。

## 固定测量合同

每个数据集执行：

- warm-up：2 × 120 个连续 `requestAnimationFrame` frame delta；不进入正式统计。
- measured：5 × 120 个 frame delta，共 600 个正式样本。
- Renderer：`PerformanceObserver(type=longtask)`、DOM 节点数、Renderer PID/RSS。
- Electron：主进程 PID、`app.getAppMetrics()` 的真实 working set。
- Runtime：仅在 `MATOU_E2E_SCALE=1` 时通过 main ↔ Runtime 请求读取 Runtime PID、Runtime 权威 PTY PID 列表/数量，以及可 reset/read 的真实 SQLite statement counter。
- 退出：记录 Electron、Renderer、Runtime 与全部 PTY PID；fixture 关闭后轮询确认所有记录进程均退出，再删除数据目录。
- 统计：记录 p50、p95、max；不采用单次最好值。

默认产品路径不暴露指标入口。SQLite counter 只有收到 scale reset 请求后才开始累计，普通运行不分配测量窗口，也不递增计数。

## 2026-09-01 baseline evidence

命令：

```bash
pnpm test:scale --grep "harness" --reporter=line
```

结果：3 passed，24.3s。

| 数据集 | 正式 frames | frame p50 / p95 / max | Renderer RSS | Runtime RSS | Runtime PTY | DOM | 正式窗口 statement | Long Task |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| 50 Session | 600 | 6.9 / 7.7 / 7.9 ms | 185.48 MiB | 99.28 MiB | 1 | 282 | 2,010 | 0 |
| 200 Session | 600 | 6.9 / 7.7 / 7.9 ms | 209.48 MiB | 124.78 MiB | 1 | 731 | 6,510 | 0 |
| 1000 Session | 600 | 6.9 / 7.7 / 7.9 ms | 387.13 MiB | 194.89 MiB | 1 | 3,131 | 33,561 | 0 |

这些 frame 数据是在静置后的 warm measurement 中采集，不等同于滑动、hover、DAG pan/zoom 或恢复并发门槛。statement 是五轮正式窗口内的总执行数，不等同于单次 `projection.snapshot` 查询预算；Task 3 仍需单独证明单次 snapshot ≤ 40 statements。

## Seed 证据

同一 migrated SQLite 连续执行两次 seed，实体和关系数量保持一致：

| 数据 | 实体 | 关系 |
|---|---:|---:|
| 当前横向 Session | 50 / 200 / 1000 | 0 |
| 深链 | 5,000 | 4,999 |
| DAG | 10,000 | 9,999 |
| Scene | 可指定；证据 fixture 为 3 | - |
| Journal | 每个 Session 可指定 payload bytes；重复 seed 不追加旧帧 | 真实 Journal V2 frame |

## 对照边界

Task 1 没有新增用户可见界面或操作入口，因此 reference product 交互矩阵无新增场景。产品可感知差异只记录为后续容量工作输入：当前前台列表 PTY 数为 1，与已确认的“全部当前横向 Session 保持即时输入”目标不同。
