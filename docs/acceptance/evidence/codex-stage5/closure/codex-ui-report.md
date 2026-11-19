> 本页保留首次两项执行现场；#8后续独立补测已通过，见 [补测结果](codex-restart-recheck/results/summary.md)。#5的Default审批恢复失败仍保留。

# Codex 两项收口实测

2026-11-15，原最终包 `app.asar` SHA-256 实测为 `69e4cc57bf526447b0e697a5df184f0364ff68d5b11418da9cfdc4761962d597`。两个指定场景各执行一次，无自动重试；结果 **2 failed，退出码 1**。本报告只记录本批现场，不将未执行步骤判为通过。

## #5 Bypass / Default：失败，33.122 秒

会话 UUID `01a1df3c-c979-79a0-9604-29720adf41ce` 保持一致。Bypass 切换前后 Codex PID 从 `1721` 变为 `3381`，其新 turn `01a1df3c-f11b-7612-a358-b93dcc99444a` 的问题未包含暗号，final_answer 和 task_complete 都是 `river-slate-842`；真实 turn_context 的 sandbox 为 `danger-full-access`，approval 为 `never`。

切回 Default 后 PID 为 `4685`，同 UUID 的新 turn `01a1df3d-15b4-7702-8c7b-712b3862a025` 也以不含暗号的问题取得相同真实答案；sandbox `read-only` 断言通过，但 approval 实际 `never`，测试期望 `on-request`，第 666 行失败。参数的产品含义与预期由主代理核对；本批不能判该场景通过。逐 turn 记录 `codex/results/turns-modes-closure-iKAgg0.json`，Bypass 阶段 `codex/results/closure-modes.json`，截图 `codex/closure-05-bypass.png`、`codex/last-modes-closure-iKAgg0.png`，trace `.cache/c8/codex-test-results/codex-acceptance-Codex-pac-3a750-tain-a-real-new-turn-memory/trace.zip`。

## #8 正常静置重启：失败，43.18 秒

重启前会话 UUID `01a1df3d-4b35-7611-895a-673fa189d9c1`，turn `01a1df3d-4c65-7aa3-90a7-72c100ff7574` 的 final_answer 和 task_complete 均为 `RESTART_READY`。真实重启后静置 10 秒，`work_status=starting` 断言通过；但未找到测试要求的“正在恢复 Codex 会话”横幅，第 695 行失败。失败截图 `codex/last-restart-closure-FCTs8a.png` 显示终端历史与输入提示，未显示该横幅。

因此本批**未执行**重启后不带暗号的新问题、记忆接回及横幅消失检查；不能凭历史回显判定接回。trace `.cache/c8/codex-test-results/codex-acceptance-Codex-pac-cfbd5-nds-then-recalls-a-codeword/trace.zip`，重启前 turn 记录 `codex/results/turns-restart-closure-FCTs8a.json`。

逐项机器结果 `codex/results/codex-playwright.json`，原始退出码 `codex/results/codex.exit`，日志 `codex/results/codex.log`。自主决定：失败后保留原始证据，不做额外重试或自行修改脚本；未执行 Antigravity。本批隔离 auth 副本和相关 App / PTY 进程检查均无残留，未改真实配置或 CLI。
