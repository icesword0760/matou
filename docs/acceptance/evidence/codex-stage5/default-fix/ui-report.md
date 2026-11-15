# Default 权限修复：打包版真实验收

2026-11-15，对应源码 `3f71baa75256aafe797b6c2ae687c31fccd6d0bd` 的打包 App 执行一次指定场景，**通过**，退出码 0，整次 61.5 秒，无重试。执行前实测 app.asar SHA-256 `69e4cc57bf526447b0e697a5df184f0364ff68d5b11418da9cfdc4761962d597`，runtime SHA-256 `1d076a0b0a17687d51f680c4cb963189ff5bdd9986beedb4be674f80dee16488`。测试独立 CODEX_HOME 的顶层默认配置为 `on-request` / `workspace-write`。

四次切换均启动新 PID，并保持会话 UUID `01a1df4f-b04c-73c3-a4a2-21e8f79eda99`。每次切换后的新问题均未包含暗号；真实 rollout 中对应 turn 的 final_answer 和 task_complete 都为 `river-slate-842`。

| 切换 | PID 前 → 后 | 真实 turn_context：approval / sandbox | 新 turn |
| --- | --- | --- | --- |
| Bypass Permissions | 12341 → 14448 | never / danger-full-access | `01a1df4f-dcd4-7102-91e1-078a2b039e1a` |
| Default | 14448 → 15999 | on-request / workspace-write | `01a1df50-0165-72f3-860c-3c80ccc035bd` |
| Plan Mode | 15999 → 17840 | on-request / read-only | `01a1df50-2fd1-77b2-ae9d-a111e08e7e49` |
| Default | 17840 → 19680 | on-request / workspace-write | `01a1df50-5de1-75f0-a4eb-e96c4aedffae` |

结构化证据为 `ui/results/default-fix-modes.json` 和 `ui/results/turns-default-fix-CCsnqM.json`；四步截图为 `ui/default-fix-1-bypass-permissions.png` 至 `ui/default-fix-4-default.png`。机器结果 `ui/results/playwright.json`、退出码 `ui/results/exit`、日志 `ui/results/run.log`；测试通过，按 `retain-on-failure` 未产生失败 trace。

自主决定：仅运行授权的 #5 场景一次，不执行其他场景或追加调试；未修改产品、测试脚本、全局配置、CLI 或登录。结束后检查本批隔离 auth 副本与相关 App / PTY 进程，均无残留。
