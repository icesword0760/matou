# 最终打包版 Codex 验收现场

2026-11-15，使用最终包运行完整 `codex-acceptance.spec.ts`，启用一次自动重试；结果 **5 passed / 1 failed，退出码 1**。包内 `app.asar` SHA-256 为 `69e4cc57bf526447b0e697a5df184f0364ff68d5b11418da9cfdc4761962d597`，runtime SHA-256 为 `50bd93fc1419318c7ef2b837503a65b0f90ab8655f1ccd1c7c284f190ee4180f`，执行前实测一致。测试使用真实 HOME、隔离 CODEX_HOME、临时工作目录及空 ZDOTDIR；未修改产品或测试代码、全局配置、CLI 版本，也未重新打包。

| 场景 | 首次尝试 | 自动重试 | 实际结果及未走到的步骤 |
| --- | --- | --- | --- |
| 生命周期、HUD、通知、标题、模式、重启与性能 | 失败，108 秒 | 失败，100 秒 | 两次均完成前三轮真实对话与性能采样、HUD、AGENTS.md 保存、自动标题锁与恢复、完成通知；Accept Edits 与 Plan Mode 均记录 PID 变化、启动参数和同 UUID 暗号回复。Bypass 确认弹窗出现后脚本没有确认，等待 PID 变化超时；Bypass、Default 与本场景正常重启静置接回未验证。现场 `last-core-aG70yG.png`、`last-core-HdYFOo.png`。 |
| 当前目录 / 新 Worktree Fork、父子隔离、搜索载入 | 通过，67 秒 | 未触发 | 两个子会话获得不同 UUID，记录父 UUID、对应 cwd、继承暗号和父卡不含子暗号的真实回复；父卡正常退出后搜索并载入父 UUID，再次通过新 turn 回答。`results/fork-loader.json`、`results/turns-fork-load-M4nFbe.json`、`09-fork-current.png`、`09-fork-new.png`、`07-loader.png`、`07-loaded-memory.png`。 |
| mt 创建 Codex 卡与读取兄弟卡 | 通过，22 秒 | 未触发 | 真实 Codex 回答与兄弟卡测试值一致，脚本检查真实命令执行记录；`results/mt.json`、`results/turns-mt-1RMsrH.json`、`11-mt-cross-card.png`。 |
| 真实审批、等待通知、继续与中断 | 通过，39 秒 | 未触发 | `results/approval-interrupt.json`，截图 `02-approval-notification.png`、`02-approval-terminal.png`、`02-interrupted.png`。 |
| 真实缺失本地 rollout 的恢复失败、重试与新会话 | 通过，47 秒 | 未触发 | 仅暂移本次隔离环境的真实 rollout；恢复失败后还原并重试取回暗号，再次暂移后新建会话得到新 UUID。`results/recovery-failure.json`、`results/turns-recovery-failure-wi7nsO.json` 和 `08-real-*.png`。这项不替代核心场景的正常静置重启验收。 |
| 未安装与未登录 | 通过，10 秒 | 未触发 | 均为受控 fixture：不存在的命令、输出登录提示的替身，并非修改真实登录；`results/fixture-missing.json`、`results/fixture-unauthenticated.json` 和对应截图。 |

最终核心重试中，从 Runtime 状态事件到 DAG DOM 变化的三次采样：running **2 / 2 / 1 ms**，idle **2 / 2 / 12 ms**，全部不超过 1000 ms；原始事件和变化时间保存在 `results/latency.json` 与 `results/latency-round-*.json`。`attempt0-latency.json` 是自动重试进行时复制的部分数据，不能作为首轮完整性能证据；首轮结果如需引用，应从其 trace 和数据库原始事件重建。

完整逐次结果为 `results/codex-playwright.json`，首次失败和重试 trace 分别在 `.cache/c5/final-codex-test-results/codex-acceptance-Codex-pac-70b9e-odes-restart-and-latency-3x/` 和同名 `-retry1/`；命令日志为监督目录 `logs/stage5-final-codex.log`。真实回复以相同 turn 的用户问题、assistant final_answer 与 task_complete 核对，不把历史回显作为新回复。

自主决定：按用户指定的整套顺序执行，无额外调试或额外重试；核心失败后继续其他独立项。检查确认本批隔离 CODEX_HOME 下没有残留 `auth.json`，进程命令行筛查未见本批隔离 App 或 PTY。未执行步骤四的回归；完整测试仍因核心项失败而未通过。
