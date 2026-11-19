> 本页为旧包及收口前的历史现场记录。最终包结论见 [最终 UI 报告](final/ui-report.md) 与 [主矩阵](../../codex-reference-matrix.md)，不把下列旧状态作为当前结论。

# Codex 第五关 UI 与回归现场记录

## 当前进度

Codex 打包验收核心项首次执行失败，尚未运行其余四项。主窗口截图显示真实 Codex v0.162.1 到达输入提示、卡标题为 Codex，但 DAG 等待显示 Codex 时仍为 Shell 并在第 503 行超时；尚未提交首轮问题，原因待主代理判断。截图 `last-core-K6G3sy.png`，日志 `results/codex-core.log`，trace `.cache/c5/codex-core-results/codex-acceptance-Codex-pac-70b9e-odes-restart-and-latency-3x/trace.zip`。不能把第四关开发态结果当作本关打包验收；性能六个数值尚未取得。启动前发现的升级提示处理、静置时长与逐轮回复核对问题已由主代理修订。

核心第二次按修订脚本执行：先看到 Codex 卡并完成真实首轮，turn `01a1df0e-bc70-7261-920d-0431bfcbd460` 的 final_answer 和 task_complete 均为 `ACK_1`（`results/turns-core-WxLFu7.json`），但首轮未找到 running Runtime 事件，断言失败；未产生完整六次延迟。日志 `results/codex-core-run2.log`，trace `.cache/c5/codex-core-run2-results/`，截图 `last-core-WxLFu7.png`。原先先打开 DAG 后仍显示 Shell 的差异保留，尚未证实原因。

核心 run4 完成了三轮状态采样：running `3/2/2 ms`、idle `3/3/3 ms`，原始记录 `results/latency-run4.json`；之后在关闭通知中心时测试 selector 同时匹配打开和关闭按钮而失败，尚未执行后续模式切换和重启。原始 trace `.cache/c5/codex-core-run4-results/`、日志 `results/codex-core-run4.log`。

核心 run5 在真实打包版完成前半部分：`core-front.json` 包含 3 轮 ACK、工具回复与标题锁回复的同 turn final_answer/task_complete；真实 HUD、AGENTS.md 保存、自动标题/锁/恢复、完成通知均走到脚本断言通过。首个 Accept Edits 切换后等待终端 PID 改变 40 秒超时，截图 `last-core-SkFqgZ.png`，trace `.cache/c5/codex-core-run5-results/`；模式接回和本关静置重启未完成。截图中的 TUI 显示 Plan mode、HUD AE，原因待查，不能判定切换成功。

其余 5 个独立场景：1 passed / 4 failed（`results/codex-independent.log`、退出码 1，trace 在 `.cache/c5/codex-independent-results/`）：

- 当前目录 Fork 成功并取得新 UUID；新 Worktree Fork 停在 restoring-provider，进度层 60 秒未消失；该项后续载入未运行。
- mt 创建与回答后，预期的 rollout function_call 筛选结果为空，跨卡命令未获证明。
- 真实审批、等待通知、继续和中断场景通过。
- 真实缺失 rollout 后显示恢复失败横幅，但 work_status 为 exited 而脚本期待 error；后续重试和新开未运行。
- 受控未安装样本未显示预期启动失败卡；后续受控未登录样本未运行。

测试完成后没有发现本轮隔离 app/PTY 进程或隔离 `auth.json`。原因待主代理查清；上述失败不视为产品通过。

## Claude 四项规格

首次使用长隔离根的批次：前两条均打开 Electron 窗口后等待“横向新增 Shell”超时，第三条中断，其余未运行；批次退出码 130。日志 `results/claude-initial.log`，trace 在 `results/claude-initial-traces/`。没有捕获 socket 长度错误明文，不据此宣称已定位。

换短隔离根后，首条独立验证通过（1 passed，4.9 秒；`results/claude-check.log`）；余下 12 条 9 passed / 3 failed（`results/claude-rest.log`，退出码 1，trace 在 `.cache/c5/claude-rest-results/`）。其中 `prd-02:14` 因项目内临时目录向上继承项目 Git 仓库而与“outside 没有 Git”测试条件冲突，加入 `GIT_CEILING_DIRECTORIES` 后仅复核该项 1 passed（`results/claude-git-recheck.log`）。`prd-06` 第 187 行 relations 为空、第 276 行 failure-card 数为 0，均与 `docs/acceptance/antigravity-reference-matrix.md` 记录的 main `6132501` 历史对照一致；本轮未重新运行 main。`prd-01` 四项、`session-load-existing-claude` 两项通过。

## Antigravity 打包版单次回归

单次无重试：2 passed / 3 failed，退出码 1，日志 `results/antigravity.log`，证据在 `antigravity-regression/`，失败 trace 在 `.cache/c5/agy-test-results/`。

- 生命周期、HUD、通知、模式和模型：通过。
- 自动标题、恢复、延迟：新建后等待 DAG idle 90 秒超时，原因待查。
- DAG、Fork、载入：选择器同时匹配历史按钮与创建子分支按钮，严格模式失败，后续 Fork/载入未执行。
- mt 跨卡：通过。
- 受控未安装、未登录与恢复失败：第 2 张卡未出现，后续场景未执行，原因待查。

上述失败不可宣称无回归。所有相关测试的退出码及失败证据保留；自行清理了确认为本轮 prd02 测试替身的孤儿进程 PID 43127，没有操作其他项目进程。
