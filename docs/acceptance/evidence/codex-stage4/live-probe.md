# Codex 第四关真实界面探针：中止

2026-11-15，开发态 Matou 的真实 Electron 窗口成功启动，并在隔离 `CODEX_HOME` 中打开了真实 Codex 的目录信任提示。没有完成有效对话；载入、重启记忆、原生 Fork 和父子隔离均未验证。未登录恢复样本也未执行。

临时操作脚本先检查目录信任、后处理升级提示，但实际顺序是先出现升级提示，跳过后才出现目录信任；第一轮等待回复超时。第二次操作时，前一次遗留的启动状态使输入中的 Enter 误触发升级提示的默认选项，启动了 `npm install -g @openai/codex`。该全局更新不在授权范围。发现后立即中止测试；没有继续启动探针。

只读核查显示原全局 `/Users/icesword/.nvm/versions/node/v22.16.0/bin/codex` 当前缺失，安装目录 `package.json` 显示版本 `0.169.0`，此前探针版本为 `0.162.1`；说明中止时全局 CLI 处于不完整状态。已立即报告主代理协调修复，未自行重装或覆盖其他正在运行的会话。真实 `~/.codex/config.toml` 和 `~/.codex/hooks.json` 前后 SHA-256 分别保持 `f261c9af1f2e09e71276d97adb784f5df5ec2acfd9e06ae9ace8a4b1710b95af`、`bf46565e936c07ce36c4f4beb5df09fbc11607ff8af3b1079458c517c1c8c0a1`。隔离复制的 `auth.json` 已删除。未产出含账户的公开截图。

自主决定：发现全局更新后中止当前工作，保留未完成状态如实报告，不把启动界面当成功能验收；为避免干扰其他运行中的 Codex 会话，交由主代理协调全局 CLI 修复。

## 误触发的原始证据与进程核查

升级前本次使用的 CLI 路径为 `/Users/icesword/.nvm/versions/node/v22.16.0/bin/codex`，此前实际 `codex --version` 输出 `codex-cli 0.162.1`。第二次测试终端输出的非敏感原文摘录：

```text
Updating Codex via `npm install -g @openai/codex`...
npm warn Unknown env config "verify-deps-before-run". This will stop working in the next major version of npm.
(node:28444) Warning: The 'NO_COLOR' env is ignored due to the 'FORCE_COLOR' env being set.
```

输出中仅有 Node 警告标识 PID `28444`；没有捕获独立的 npm 进程树或其他 PID，不能断言它就是更新进程的完整 PID 集合。中止后 `ps -p 28444 -o pid=,ppid=,command=` 无输出；按 `npm install -g @openai/codex` 和本任务包装脚本路径筛查进程也无输出。这证明上述 PID 当前已不存在，且筛查未发现相应命令行进程；不证明所有可能的后代进程均被枚举。测试进程本身以 Ctrl-C 结束，工具返回退出码 130。

已知变化路径：隔离探针在项目 `.cache/c4/` 建立了临时配置、操作脚本及运行数据；全局 `/Users/icesword/.nvm/versions/node/v22.16.0/lib/node_modules/@openai/codex/` 的包内容发生变化，目录及其中 `package.json` 修改时间均为 2026-11-15 14:48:39；原 `/Users/icesword/.nvm/versions/node/v22.16.0/bin/codex` 当前缺失。未对全局目录做完整前后清单，其他 npm 缓存或临时目录变化未知，不据此推断。已按要求停止全部测试、未再执行 Codex/npm、未进行任何全局修复；隔离 `auth.json` 已删除，其余非敏感证据保留。

## 恢复 CLI 后续实测（现场保留）

主代理按监督批准恢复 CLI 后，隔离包装器增加单次 `-c check_for_update_on_startup=false`，本机二进制字符串存在该键。新启动实际看到的是真实 `OpenAI Codex (v0.162.1)`、`GPT-6-Astra medium`，没有升级提示；仅对隔离 CODEX_HOME 的临时工作目录接受信任。当前只读核查全局 bin 链接指向 `../lib/node_modules/@openai/codex/bin/codex.js`，包版本为 `0.162.1`。

真实父会话 UUID `01a1ddfb-09b3-7e41-a492-19e8854bf29e`：第一轮请求记住测试暗号 `amber-orbit-482`，turn `01a1ddfb-357c-7070-80d5-79198aa06be8` 的 assistant `final_answer` 与 `task_complete` 都是 `ACK`。真实重启 Matou 后，父卡绑定仍为同一个 UUID；未在后续问题中给出暗号，新的 turn `01a1ddfb-d116-7ab1-a039-632e43d4f326` 的 `final_answer` 与 `task_complete` 均返回 `amber-orbit-482`，因此不是仅凭 UI 显示历史字符串判定记忆。rollout 位于 `.cache/c4/codex-home/sessions/2026/11/15/rollout-2026-11-15T14-52-59-01a1ddfb-09b3-7e41-a492-19e8854bf29e.jsonl`，截图 `.cache/c4/before-fork.png`。

从空闲父卡创建当前工作树 Fork 后，界面先显示“等待创建分支 / 前面的分支任务完成后会自动继续 / 第 1 / 2 阶段”，随后数据库 intent 由 `restoring-provider` 变 `failed`。子卡 `9a80b19d-9062-442e-a77f-7cb54e5c24d6` 的临时 binding 为父 UUID、状态 `unknown`，没有确认新 UUID；不得视为 Fork 成功。触发前后父 binding 保持 `available`、UUID 不变，父标题为 `Remember secret phrase`，父 rollout SHA-256 保持 `d03abc10db03edbffa3c309e8f31236714c9fad104afb337dce9332e4483ff3a`。创建期间窗口截图 `.cache/c4/fork-result.png`。父子独立暗号隔离、载入弹窗搜索预览与指定 UUID 接回，以及未登录异常样本均未验证。

自主决定：按明确的升级提示安全规则逐屏确认，每一轮以新 turn 的 `final_answer` 和同 turn `task_complete` 判定；Fork 出现真实异常后停止重试、保留窗口和数据供主代理排查。隔离 auth 在保留现场期间暂留本机，待现场检查结束后删除；不提供或提交凭据及原始敏感屏幕。

## run-5 裁决后的重测

唯一候选发现、首次真实钩子校验及启动队列修正后，开发态载入、重启接回、当前工作树 Fork 和父子隔离已取得新的真实对话证据。详见 [ui-run5.md](ui-run5.md) 与 [verification.md](verification.md)。上述早期失败和升级事故记录继续保留。隔离凭据及大体积临时副本在本轮验收结束后已删除，数据库与 rollout 保留；未进行第五关打包或完整交互矩阵。
