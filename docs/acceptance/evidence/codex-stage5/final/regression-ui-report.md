# 最终回归 UI 现场

2026-11-15，按顺序各执行一次，均 `--retries=0`。最终包 `app.asar` SHA-256 为 `69e4cc57bf526447b0e697a5df184f0364ff68d5b11418da9cfdc4761962d597`，runtime 为 `50bd93fc1419318c7ef2b837503a65b0f90ab8655f1ccd1c7c284f190ee4180f`，运行前实测一致。四份 Claude 规格使用项目**开发态 Electron fixture**；Antigravity 规格使用**最终打包 App**。没有重新构建，也没有本轮重跑 main。

## Claude：11 passed / 2 failed，退出码 1，1.6 分钟

| 规格与场景 | 结果 | 耗时 |
| --- | --- | ---: |
| prd-01 通知：未读轨迹和返回来源 | 通过 | 3.86 秒 |
| prd-01 通知：聚焦已读与未读高亮 | 通过 | 2.84 秒 |
| prd-01 通知：关闭与声音偏好 | 通过 | 3.20 秒 |
| prd-01 通知：两个工作空间队列 | 通过 | 4.00 秒 |
| prd-02 HUD：Shell 环境、cwd 与 Git | 通过 | 4.07 秒 |
| prd-02 HUD：Git 控件 | 通过 | 5.04 秒 |
| prd-02 HUD：Agent 权限与模型 | 通过 | 6.82 秒 |
| prd-06：新 Fork 的恢复确认 | 通过 | 9.13 秒 |
| prd-06：重试 Fork 的恢复确认 | 通过 | 10.96 秒 |
| prd-06：分支隔离与重启 | 失败 | 7.31 秒 |
| prd-06：失败 Fork 保留 | 失败 | 20.38 秒 |
| session-load：载入长会话并输入 | 通过 | 4.32 秒 |
| session-load：左筛选与右搜索 | 通过 | 12.36 秒 |

两个失败分别为第 187 行 `relations` 实际为空，及第 276 行失败卡数量为 0；相应断言后的步骤未执行。它们与 `docs/acceptance/antigravity-reference-matrix.md` 所载历史 main `6132501` 失败相符；这只是历史对照，不能据此宣称当前 main 已重新验证或证明没有回归。完整逐项记录 `results/claude-playwright.json`、日志 `results/claude.log`，失败 trace 分别在 `claude-regression/prd-06-session-fork-PRD-06-85a27-ry-branch-and-restores-once/trace.zip` 和 `claude-regression/prd-06-session-fork-PRD-06-484ad-Shell-or-reusing-the-source/trace.zip`。

测试覆盖的旧 prd-02 七项与 prd-06 一张截图已先复制到 `claude-regression/prd-02/matou/`、`claude-regression/prd-06/matou/`，再按主代理指示仅恢复这八个历史路径。

## Antigravity：4 passed / 1 failed，退出码 1，4.8 分钟

| 场景 | 结果 | 耗时 |
| --- | --- | ---: |
| 真实生命周期、HUD、通知、权限模式与模型 | 通过 | 53.55 秒 |
| 自动标题锁与恢复、重启接回、3 次状态延迟 | 通过 | 55.50 秒 |
| DAG、当前 / 新 Worktree / 父运行中 Fork 与载入 | 失败 | 12.61 秒 |
| 真实 mt 跨卡控制 | 通过 | 152.45 秒 |
| 受控未安装、未登录与恢复失败 | 通过 | 15.00 秒 |

失败发生在第 876 行：Fork 选择器同时匹配会话历史按钮和创建子分支按钮，触发严格模式错误；此测试的 Fork、载入和后续检查未走到，不能判为通过。trace 在 `antigravity-regression/playwright-artifacts/antigravity-acceptance-Ant-ce073-ktree-Running-SessionLoader/trace.zip`。完整逐项记录 `results/antigravity-playwright.json`、日志 `results/antigravity.log`；截图及结构化数据在 `antigravity-regression/`。状态事件到 UI 的本次三次采样为 running `2 / 1 / 2 ms`、idle `2 / 2 / 3 ms`，详见 `antigravity-regression/results/core-and-latency.json`。

自主决定：保留所有失败及未执行步骤，没有单条调试或重跑。核查本轮进程时发现 prd02 受控替身孤儿 PID 96373，核实路径归属后停止，子进程也已退出；未见其他本批隔离 App / PTY，也未发现 final-agy 临时 auth 文件。真实 HOME 未替换，继承的 `MATOU_CONTROL_*` 已剔除。
