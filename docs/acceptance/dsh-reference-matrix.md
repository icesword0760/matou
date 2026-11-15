# DSH 接入对照矩阵：本轮续跑验收

2026-11-16，从现有断点继续，产品、单测与 spec 已提交 `479e620`（`feat/multi-agent-dsh`）。以下是本轮独立三套重跑结果；旧基线与上一轮证据保留在后面的历史记录，未拿旧结果代替本轮执行。

## 三套对照与失败集合

| 套件 | `a06812f` 基线 | 本轮结果 | 逐条对照结论 |
| --- | --- | --- | --- |
| DSH | 不适用（尚未接入） | **6 首次通过 + 1 重试通过；0 最终失败、0 跳过**（3分36秒） | 审批首试 `:187` 采样误读上一轮原生时间；重试独立完整三轮通过。恢复、模型错误等其余6项首次通过 |
| Antigravity | 4通过 / 1失败，`:876` 定位器歧义 | **4 首次通过 / 1 最终失败、0 跳过**（3分19秒） | 首试、重试均 `:876` 同一定位器歧义；后续 Fork/载入未到，属于基线同点问题 |
| Codex | 9通过 / 1失败，`:862` 新Worktree原生启动失败后误读继承ID | **9 首次通过 + 1 重试通过；0 最终失败、0 跳过**（11分44秒） | 首试 `:485`（调用 `:912`）在最终载入回忆超时，原生错误与基线同为路由发现超时；重试完整通过，两个子ID独立且父关联正确 |

三套各完整启动一次、1 worker、`--retries=1`。同一已核验打包 App；真实 HOME、空 ZDOTDIR、临时 Git 仓库、独立 Matou/Electron 数据和 Codex 配置/认证/会话目录。基线沿用已核验的独立 `a06812f` worktree 结果，没有重跑基线。[本轮逐用例与全部尝试](evidence/dsh-stage5/continuation/ui-regression.json)、[基线失败集合](evidence/dsh-stage5/followup/baseline-results.json)。

上一轮 Antigravity 的 `:497/:802/:408` 和 Codex 的两处首轮身份等待、Fork继承ID误判均未在本轮重现。结合共享源码审计和各自原生记录，未发现本轮由 DSH 共享代码变动造成的产品回归。Codex 本轮首试失败发生在更后的载入步骤，不能称为与基线“同一断言失败”；实际原生错误类别相同，重试完成了后续步骤。[本轮 Codex 首试现场审计](evidence/dsh-stage5/continuation/codex-first-attempt-audit.json)。两项重试通过都保留 flaky 分类，不写成稳定首过。

## Codex Fork 误报澄清

上一轮报“父子 UUID 相同”时，原生子进程启动失败、Fork intent=failed，没有生成子 rollout；测试却把继承的 unknown / 未验证父 ID 当成子身份。独立 `a06812f` 基线也发生原生路由发现超时，并被旧断言误读。它不能证明成功子会话绑定到了父会话。

修正后的测试先要求 Fork 成功、再读取 available/resumed 且 validated 的绑定，仍要求子 UUID 独立、原生 `session_meta.id` 与绑定一致、`forked_from_id` 指向父会话。用户已核验的上一轮两个子会话均满足这些条件。[原生与共享源码审计](evidence/dsh-stage5/followup/failure-source-audit.json)、[已核验父子证据](evidence/dsh-stage5/followup/codex-fork-verification.json)。本轮主代理再次核验两种 Fork 的成功 intent、validated 绑定、独立 UUID 及原生父关联均一致，重试后原父会话载入也找回暗号。[本轮逐字段核验](evidence/dsh-stage5/continuation/codex-fork-verification.json)。没有修改 Codex 原生路由或将失败占位绑定伪装成成功。

## 本轮 DSH reference product 交互对照矩阵

可运行原生 dsh-TUI 与现存代码为 provider 双重基线；Matou 使用已有卡片、HUD、通知和载入/恢复入口。只比较 CLI 模块；原生模式只读、卡片不提供一键 Fork 为已裁决范围。

| # | 原生基线 / Matou 行为 | 本轮真实 App 结果 | 状态与边界 |
| --- | --- | --- | --- |
| 1 新建 | 原生启动、manifest、Shell 升级、mt 建卡 | 新建、标签、缺 CLI 的可操作提示通过 | 首次通过；缺 CLI 是不存在命令夹具 |
| 2 工作状态 | 原生 turn/status/approval → 卡片和 DAG | 工作、空闲、三轮审批、三轮原生错误通过 | 审批整项重试通过；其它首次通过 |
| 3 通知 | 原生完成/审批/错误 → 共享通知中心 | 后台完成一次、审批三次、错误三次；原生 OSC 0 | 审批整项重试通过；未声明系统通知点击或音效签收 |
| 4 HUD | model.current / sessionProjections / 工具事件 | 模型/档位、占用、只读原生模式、工具、目录/Git 通过 | 首次通过；没有可靠来源的项目指令入口隐藏 |
| 5 执行模式 | 原生权限偏好会全局持久化 | 只读徽标可见，没有模式切换入口 | 已裁决范围首次通过 |
| 6 自动标题 | 原生 session/title、手动锁定、恢复自动 | 自动更新、手动锁定、恢复最新标题通过 | 首次通过；无新增标题模型请求 |
| 7 载入会话 | 原生 v4 历史、cwd、预览/搜索/精确 resume | 原生子会话搜索、预览、载入并回忆继承暗号通过 | 首次通过；未声明大规模目录 UI 压测 |
| 8 重启恢复 | 同 ID、持久化与真实 ready；失败重试/新开 | 正常重启、失败横幅、重试原 ID 找回暗号、新开不同 ID 通过 | 首次通过；五场景单测覆盖真实恢复协调器 |
| 9 Fork | 原生 /fork 独立 ID 与父关联 | 原生子会话搜索、载入与记忆通过；无卡片一键入口 | 已裁决范围首次通过；未复制上游编排 |
| 10 mt 跨卡 | 原生 system section + shellEnv，默认 workspace-write | 真实 identify/list/read；独立兄弟屏幕标记在工具结果与回复均出现 | 首次通过；不声明本轮逐项操作所有 mt 命令 |
| 11 不提供入口 | manifest 与共享 HUD/menu | 账户、费用/额度、供应商、团队与不可靠指令入口均未出现 | 首次通过 |

[本轮 DSH 汇总](evidence/dsh-stage5/continuation/dsh-acceptance.json)、[主代理原生交叉核验](evidence/dsh-stage5/continuation/native-cross-check.json)、[失败横幅](evidence/dsh-stage5/continuation/screenshots/08-restore-failed.png)、[重试与新开](evidence/dsh-stage5/continuation/screenshots/08-retry-new.png)。9 项受保护配置无变化。完整原生日志、数据库、截图及失败现场保留在 `.cache/dsh-continuation/`。

## 审批重试原因与实际性能

本轮审批首试在第二轮 `sample():187` 报 7240ms >1000ms。原始采样把第一轮的 `approval/asked` 时间用于第二轮；主代理只读解压完成后日志，第二轮原生时间为 1794768314361，实际 UI 为 1794768314467，对应 106ms，Runtime→UI 为3ms。这个核对只解释原因，不改写原始失败或把它计作首过。首试停在第二轮性能断言，第二轮审批决定、第三轮及最终恢复未执行；重试独立完成全部三轮和恢复。[采样审计](evidence/dsh-stage5/continuation/approval-sampling-audit.json)。采样在原生事件落盘前读取上一轮记录的竞争仍保留为未修问题，本轮不追加实验或重跑。

四类状态各3次，Runtime domain event 到实际 DAG DOM mutation 的门槛≤1000ms。每个场景只采用完整通过的那次尝试，不跨失败/重试拼接样本。

| 状态 | Runtime→UI（ms） | 原生→UI辅证（ms） | 判定 |
| --- | --- | --- | --- |
| 工作中 | 2 / 1 / 2 | UI早于原生 turn/start 8 / 6 / 3 | Runtime三次达标；负值不可作为原生延迟成功样本 |
| 回到空闲 | 2 / 2 / 3 | 196 / 9 / 11 | 两口径三次达标 |
| 需要输入 | 2 / 2 / 2 | 106 / 105 / 104 | 重试完整三轮的样本达标；首试采样失败仍保留 |
| 出错 | 6 / 7 / 15 | 14 / 17 / 25 | 两口径三次达标；原生 error ends 与独立通知各三次 |

错误夹具使用真实 DSH、已知模型和本 run 的回环 HTTP400端点/专用假 key，不伪造事件或改用户 profile。审批使用独立 printf 请求，前两轮 allow-once、最后拒绝并中断，默认权限未变。

## 已修改与自动验证

- 产品修复只调整 `runtime-server.ts`：已明确停放的 DSH provider 恢复失败释放外层终端挂载队列，让已有失败横幅接管重试/新开；binding 仍 failed、身份仍未确认，其它 Agent 的规则不变。
- 五种恢复单测接入真实 `RuntimeRecoveryCoordinator`，覆盖匹配、错 ID、超时重试、超时新开、进程早退重试。上一轮全部 Runtime server166/166通过；相关源码/单测未变，复用有效结果，没有重复跑整批。
- 本轮 `pnpm typecheck`、`pnpm check:identifiers` 与 diff 检查通过；Codex spec 单独严格检查的三条既有诊断在 `a06812f` 同样存在，不属于工作树 typecheck，不宣称它已修复。[本轮检查记录](evidence/dsh-stage5/continuation/source-checks.json)。
- 提交产品、单测、spec、既有证据和该任务 PRD 为 `479e620`。无新依赖、存储或产品运行时 LLM 步骤。
- 包内604个普通文件及已提交产品源码均与已有包匹配，复用清单 `278eace7af708014831f422a50a67540ce838db21343205b1d6edfec61c9a26a`；未重打包。[复用核验](evidence/dsh-stage5/continuation/package-reuse.json)。包未签名、未安装至 Applications、未发布或推送。

## 我自己做的决定

1. 主代理：从断点提交现有成果，新三套结果独立记录；基线和历史结果只作对照。
2. 主代理：产品与包相符，沿用同一包；不重复构建、基线或无变化的166项单测。
3. 主代理：用原生落盘事件解释审批7240ms采样误差，保留失败和重试分类；不在本轮追加修复或实验。
4. Terra界面子代理：按指定隔离与顺序运行三套，只清理核实归属的自有进程/认证副本/短链；无额外产品决定。

## 残余未闭合项与交回

- Antigravity 既有 `:876` 按钮定位器歧义仍阻断该旅程，当前目录/新Worktree/运行中父会话 Fork 与后续载入未执行；本轮未扩展修复。
- DSH 审批采样仍有原生事件落盘前误读上一轮时间的竞争，本轮审批为重试通过；没有把7240ms改写成首过。DSH 指定功能闭环均已取得真实事件与完整通过证据。
- Codex 原生路由发现超时在基线和本轮首试均出现；重试成功不代表该外部启动波动永久修复。原“父子UUID相同”误报已澄清，未观察到成功子会话绑回父身份。
- Codex spec 单独严格类型检查的三条既有诊断仍在；工作树 typecheck 已通过。系统通知点击、真实音效和大规模目录 UI 压测不在本轮成功声明内。
- Terra已核实本轮临时认证副本、验收进程和短链均无残留；旧dsh-stage5短链不属于本轮，未触碰。

本轮到达交回关卡，停止继续实验和产品修改。下面保留之前的完整记录，均有各轮边界，不能替代上面的本轮结果。

## 上一轮基线对照与补验

2026-11-16，`feat/multi-agent-dsh`。**本节保留上一轮完成结果；本轮续跑结果见文首。** 本轮在独立 `a06812f` worktree 先跑 Antigravity / Codex 基线，再修复 DSH 失败恢复与验收夹具。新包 DSH 正式全套 **7/7 首次通过、0 失败、0 跳过，未触发自动重试**；Antigravity 最终为 4 首次通过 / 1 基线同点失败；Codex **10/10 首次通过**。上一轮六个新增失败点本轮均未重现，结合原生现场与共享源码审计，未发现 DSH 共享代码回归；Antigravity 既有 Fork 定位器失败仍保留，不宣称该旅程已签收。

## 修复与原因

1. **失败后可以重试或新开 DSH。** 已明确停放为 provider 恢复失败的 DSH 卡片，释放外层终端挂载队列，由现有 DSH 失败横幅处理重试和新开；原会话绑定仍为 failed，身份仍未确认。没有把恢复失败改成恢复成功，也没有修改共享前端的阻挡规则。[可操作失败横幅](evidence/dsh-stage5/followup/screenshots/08-restore-failed.png)、[重试与新开实测](evidence/dsh-stage5/followup/screenshots/08-retry-new.png)。
2. **审批三轮使用独立请求。** 旧原生日志显示模型在反复拒绝同一命令后正常结束，未再申请，Matou 显示 idle 符合原生结果。本轮使用不同的 printf 命令，前两次原生 allow-once，最后拒绝并中断；三次审批、通知与返回空闲全部通过。默认模式与全局权限偏好未改。[第三轮审批](evidence/dsh-stage5/followup/screenshots/03-approval-3.png)。
3. **错误验证进入真正的原生请求路径。** 旧夹具选择不存在的模型，卡在原生初始化，未生成会话日志。本轮使用已知模型和仅本 run 的回环 HTTP 400 端点、专用假 key，真实 DSH 发出请求并记录三次 `turn/end reason=error`，Matou 显示出错并各通知一次；没有伪造事件，也不向外部端点发送这些测试请求。[原生错误与通知](evidence/dsh-stage5/followup/screenshots/03-native-error.png)。
4. **Codex Fork 检查成功身份。** 上轮失败子卡只有继承的 unknown / 未验证父 ID，没有原生子 rollout；Fork intent 为 failed，原生报 `workspace routing discovery timed out`。基线新 Worktree Fork 出现了相同的原生 account/read 路由发现超时，随后也读到了继承父 ID。测试现在先要求 intent=succeeded，再读取 available/resumed 且 validated 的绑定，随后仍检查独立 ID 与原生 `forked_from_id`；真实启动失败仍会报失败，不会被写成成功绑定父会话。 本轮当前目录与新 Worktree 两种 Fork 均已成功；主代理另外读取原生 `session_meta` / `forked_from_id` 和绑定表，确认两个子 ID 均独立、已验证并关联原父会话。[逐字段复核](evidence/dsh-stage5/followup/codex-fork-verification.json)、[当前目录](evidence/dsh-stage5/followup/screenshots/codex-09-fork-current.png)、[新 Worktree](evidence/dsh-stage5/followup/screenshots/codex-09-fork-new.png)。

[逐项原生现场与共享源码审计](evidence/dsh-stage5/followup/failure-source-audit.json)。共享文件中 DSH 分支的变动没有改变 Codex 原生启动、Antigravity 原生回复或标题读取路径；上一轮异常不能仅凭失败集合不同就归因于 DSH，也不能全部写成基线已知失败。

## 基线对照与最终回归

基线与本轮均使用真实 HOME、空 ZDOTDIR、临时 Git 仓库、独立 Matou / Electron 数据；Codex 的配置、认证与会话目录独立。三套串行，1 worker；基线 retries=0，最终各完整一次 retries=1。

| 套件 | `a06812f` 基线 | 上轮 DSH 分支 | 本轮最终包 |
| --- | --- | --- | --- |
| Antigravity | 4 通过 / 1 失败；`:876` 定位器同时命中历史与 Fork 按钮 | 2 通过 / 3 失败；`:497` 原生尚未回复、`:802` 原生标题为空、`:408` 信任提示未被测试确认 | **4 首次通过 / 1 失败**；两次尝试均为基线相同的 `:876` 选择器冲突；后续 Fork / 载入未到 |
| Codex | 9 通过 / 1 失败；新 Worktree Fork 原生退出后误读继承父 ID | 7 通过 / 3 失败；两处首轮身份等待及当前目录 Fork 误读继承父 ID | **10 首次通过 / 0 失败 / 0 跳过**；9 分 41 秒；两种 Fork 的独立身份、父子隔离、搜索、载入与恢复通过 |
| DSH | 不适用（尚未接入） | 4 通过 / 3 失败 | **7 首次通过 / 0 失败 / 0 跳过**；3 分 12 秒 |

基线中 Codex 的主流程、独立重启通过；上一轮主流程原生 bootstrap 报 account/read 路由发现超时，独立重启则尚在 MCP 启动、未到重启步骤。Antigravity 上一轮主流程原生 transcript 只有用户输入和系统消息，标题场景的原生 Title 为空；另一次停在原生信任目录提示。这些是原生/夹具的具体现场，不是 Matou 丢失了已经存在的回复、标题或模型字段。[完整基线结果与失败集合](evidence/dsh-stage5/followup/baseline-results.json)、[本轮逐用例与全部尝试](evidence/dsh-stage5/followup/ui-regression.json)。

## reference product 交互对照矩阵：本轮 DSH 最终包

原生 dsh-TUI 的可运行会话及现存代码为 provider 行为基线；Matou 卡片、HUD、载入与通知入口沿用现有 CLI 模块。范围只覆盖 CLI；模式只读与卡片不提供一键 Fork 沿用已裁决边界。下表来自本轮完整 7 条真实打包 App 旅程，原生和 Runtime 证据均保留。

| # | 原生基线与 Matou 行为 | 本轮结果与证据 | 结论 / 边界 |
| --- | --- | --- | --- |
| 1 新建 | 原生启动、manifest、Shell 升级与 mt 建卡；不存在命令提示 | 新建、标签、HUD 与缺 CLI 的可操作安装反馈通过 | 首次通过；缺 CLI 使用不存在命令夹具 |
| 2 工作状态与标签 | 原生 turn/status/approval → 主卡与 DAG | 工作、空闲、三次需要输入、三次原生错误均通过 | 首次通过；错误为本 run 回环 HTTP 400 触发的真实原生请求错误 |
| 3 通知 | 原生完成、审批和错误 → 共享通知中心；TUI 原生通知关闭 | 后台完成一次、审批三次、错误三次；原生 OSC 9/99/777 为 0 | 首次通过；未另行签收 macOS 系统通知点击和真实音效 |
| 4 HUD | 原生模型/档位、上下文、权限、工具、目录/Git | 已观察字段显示、未知指令文件隐藏；模型 deepseek-flash / max，工具与目录/Git 正常 | 首次通过；占用来自原生事实，不猜未知字段 |
| 5 执行模式 | 原生权限偏好会全局持久化；本期只读 | 原生模式徽标可见，Matou 模式切换菜单不存在 | 已裁决范围首次通过，不宣称可切换 |
| 6 自动标题 | 原生 session/title、手动锁定、恢复自动跟随 | 自动更新、手动锁定、恢复最新原生标题通过 | 首次通过；没有新增 Matou 标题生成请求 |
| 7 载入会话 | 原生 v4 历史与 cwd；目录、预览、搜索、精确 resume | 原生子会话预览、搜索和载入后回忆父暗号通过 | 首次通过；大规模目录真实 UI 压测不在本轮声明内 |
| 8 重启恢复 | 同 ID、已落盘的真实 ready 才确认恢复 | 正常重启静置与记忆；受控早退后点击重试接回原 ID 并找回暗号；再次失败后新开不同 ID | **本轮闭合，首次通过**；binding failed 与挂载队列 ready 分别代表 provider 失败和界面可操作 |
| 9 Fork | 原生 /fork 独立子 ID 与父关联；manifest fork=false | 原生 Fork 子会话可被搜索、预览、载入并保留继承记忆，卡片无一键入口 | 已裁决范围首次通过；未注入 /fork 或复制上游编排 |
| 10 mt 跨卡 | 独立 system section、原生 shellEnv、默认 workspace-write | DSH 真实执行 identify/list/read；独立 Shell 兄弟屏幕随机标记同时出现在工具结果与回复 | 首次通过；未宣称本轮逐项操作所有 mt 命令 |
| 11 不提供的入口 | manifest 与共享 HUD/menu | 账户、费用/额度、供应商、团队和不可靠指令入口均未出现 | 首次通过；不补造原生缺失接口 |

[本轮正式 7 条旅程与脱敏证据](evidence/dsh-stage5/followup/dsh-acceptance.json)。原始报告、原生日志、数据库与完整截图在 `.cache/dsh-next/final/dsh/`；修复后的三处定点检查另为 3/3（92 秒），[单独记录](evidence/dsh-stage5/followup/targeted-ui.json)，不与正式旅程重复计数。9 项受保护配置均无变化。

## 状态到实际 DAG 界面的性能

同一主机 Runtime domain event 到 MutationObserver，四类状态各 3 次、门槛 ≤1000ms。原生时间以真实日志为辅证，不把乐观 UI 早于原生事件的截零值当成有效测量。

| 状态 | Runtime → UI（ms） | 原生 → UI 辅证（ms） | 判定 |
| --- | --- | --- | --- |
| 工作中 | 1 / 2 / 2 | UI 早于原生 turn/start 9 / 3 / 4 | Runtime 口径三次达标；不声明原生口径三次有效 |
| 回到空闲 | 2 / 2 / 2 | 9 / 8 / 7 | 两口径三次达标 |
| 需要输入 | 2 / 3 / 3 | 107 / 107 / 105 | 两口径三次达标；第一轮时间由完成后落盘的对应 approval/asked 补齐，原始即时 proof 保留 |
| 出错 | 3 / 9 / 23 | 15 / 14 / 32 | 两口径三次达标；三次原生 error 结束与独立错误通知均已核对 |

## 自动验证、测试包与边界

- 恢复五场景定点通过；同一批全部 Runtime server **166/166** 通过（含这五场景，不重复加总）。工作树 `pnpm typecheck`、DSH 验收 spec 类型检查、命名门禁与 diff 检查通过。[检查记录](evidence/dsh-stage5/followup/source-checks.json)。没有重跑与本次变更无关的全量 Desktop / Runtime 主批；上轮结果保留在历史记录。
- Codex spec 额外严格类型检查有 3 条诊断（观察数组推断、HTTP 可选字段）；独立 `a06812f` 同样存在，已分别保存输出，不宣称该额外检查通过。本轮不扩展修改无关 helper 类型。
- 产品仅修改 DSH 已停放失败的队列处理，无新依赖、数据库或产品运行时 LLM 步骤。测试的回环错误请求与独立审批仅用于验收。
- 产品源码变更后只重打一次本轮包；604 个普通文件，清单 SHA-256 `278eace7af708014831f422a50a67540ce838db21343205b1d6edfec61c9a26a`，runtime SHA-256 `059178468241196488db94e94e47ffce3540e6e749001dd9d221e5a2c86d23ce`。[包记录](evidence/dsh-stage5/followup/package.json)、[清单](evidence/dsh-stage5/followup/bundle-files.sha256)。最终逐一复核 604 文件、清单及对应产品源码均无变化，[复核记录](evidence/dsh-stage5/followup/final-verification.json)。未签名、未安装到 Applications、未发布或推送。

## 我自己做的决定

1. 主代理：用 DSH 已有 provider 失败横幅承担恢复重试，释放外层挂载队列；保留失败绑定、身份门禁及其它 Agent 的行为。
2. 主代理：审批使用不同的新请求并覆盖允许一次与拒绝；未知模型改为回环 HTTP 错误，确保验收的是真实原生事件。
3. 主代理：Codex Fork 测试先检查操作成功及验证过的身份，不把 inherited unknown ID 当成功子会话；不修改原生启动路由。
4. 主代理：对上一轮与基线不一致处逐项读取原生现场和共享源码，不把全部失败归为旧问题，也不为了原生未产生的信号改 Matou 状态或补造字段。
5. Terra 界面代理：三套串行、隔离目录与相同包；只清理自己产生的认证副本、短链和测试进程，保留原始报告、截图、rollout 与数据库供复核。

## 仍未闭合与交回关卡

- Antigravity Fork 用例仍被基线同点的定位器冲突阻断，两次均停在 `:876`；当前目录、新 Worktree、运行中父会话 Fork 与后续载入断言未执行。本轮按要求保留既有问题，不扩大修复。
- Codex 原生路由发现超时已在无 DSH 的基线复现，本轮未重现；不宣称外部启动波动已被永久修复。Codex spec 的 3 条既有严格类型诊断另行保留。
- DSH 指定的审批后续轮次、原生错误状态/通知/性能、失败恢复重试与新开均已闭合。系统通知点击、真实音效及大规模目录 UI 压测不属于本轮成功声明。
- 三套正式验收已全部结束，允许一次自动重试，仅 Antigravity 失败用例使用了重试。至此停下交回；改动与证据保留在当前工作树，未提交、推送或发布，原有未跟踪 PRD 未改。

# DSH 接入对照矩阵：第五关收口

2026-11-16，`feat/multi-agent-dsh`。本次收口按用户要求停止新增实验与产品调试；先提交代码/说明和门禁，再核对最终包，完整运行 DSH（允许一次自动重试），随后执行指定回归。**DSH 正式验收 7 项：4 项首跑通过、3 项重试后仍失败、0 跳过。矩阵未闭合，继续处于实现阶段，尚未产品签收。**

## 当前产物、范围与改动

- 当前测试包对应产品源码 `9f82f8d`，其后仅测试说明与证据变更。收口时逐一验证包内 604 个普通文件，指纹无差异，本次没有重打包。整个第五关曾打包两次：首次包验收后出现产品源码修正，才生成当前包；[初始记录](evidence/dsh-stage5/package-initial.json)保留。
- 最终包：`apps/desktop/release/mac-arm64/码头.app`，未签名、未安装到 Applications、未发布、未推送。逐文件清单 SHA-256：`1207ec4457ab31ad111fcff53d42963ef4ec8dae6a4daa17e4e42903e08699f6`；runtime SHA-256：`fc8bbe7dee21eda293512b29ab619dae00e2838bddd427f692368f6b4529b48c`。[完整指纹](evidence/dsh-stage5/package.json)、[复用核对](evidence/dsh-stage5/closeout/package-reuse.json)。
- 已提交的产品调整：DSH 恢复早退沿用 error 状态并结束恢复等待；HUD 百分比在 DSH 数据边界取整。自动回归通过，但真实恢复按钮仍禁用，不能宣称恢复重试已修好。
- `runtime-server.test.ts` 新增早退后同 ID 重试检查；fixture 启动等待与原生恢复期限分开，前者有界 5 秒，未延长产品恢复期限。DSH 验收增加原生输入就绪等待、按同轮原生事件关联状态样本、拒批后原生中断后续解释。[收口代码与门禁](evidence/dsh-stage5/closeout/step1.json)。
- 启动器 0.2.2-rc.1 / TUI 0.19.0；未升级或改 profile。真实 HOME、空 ZDOTDIR、临时 Git 仓库、独立 Electron user-data / Matou 数据 / DSH 新会话目录；短 socket 别名仅指向本轮隔离数据。9 项受保护配置检查均无变化。

## reference product 交互对照矩阵：最终包

原生 dsh-TUI 可运行会话及其现存代码为 provider 行为基线；Matou 侧以现有 CLI 工作区、卡片、HUD、通知与载入/恢复管线为基线。只比较 CLI 模块；外部产品导航与品牌外壳不计入。第四关官方模式调用、第三关原生 projection/会话日志与第二关事件管线证据保留在下方；本表以真实最终包结果覆盖旧的“待 App 验证”状态。模式只读、卡片 Fork 不提供为已裁决边界，不按原 PRD 字面冒充已实现。

| # | 原生基线及 Matou 代码入口 | 最终 App 实际与证据 | 状态、差异与重试 |
| --- | --- | --- | --- |
| 1 新建 | 原生 `dsh dsh-tui`；manifest、provider launch、Shell 升级与 mt create | 综合旅程、mt 建卡及不存在命令的可操作提示通过；[HUD](evidence/dsh-stage5/closeout/screenshots/04-dsh-hud.png)、[未安装](evidence/dsh-stage5/closeout/screenshots/missing-cli.png) | 通过，均首跑；未安装为不存在命令 fixture |
| 2 工作状态与标签 | 原生 agent/status、turn/start/end、approval；共享 WorkStatus/DAG 投影 | DSH 主卡/DAG 标签、启动、工作、空闲通过；审批首跑第2轮、重试第3轮停留 idle；模型错误 fixture 两次首轮仍 idle，未完成原生错误诱发/映射闭环 | 部分通过；审批/错误整项各2次尝试均失败，不能宣布六态全部闭合 |
| 3 通知 | 原生审批/完成与 TUI OSC；provider events、共享通知中心 | 后台完成通知及无重复原生 OSC 通过；审批前轮通知/拒批执行过，[审批](evidence/dsh-stage5/closeout/screenshots/03-approval-2.png)；模型错误通知未到断言 | 部分通过；失败轮后续与最终通知 proof 未执行；没有真实音效/系统通知点击签收声明 |
| 4 HUD | 原生 model.current / sessionProjections / paired tool events；TerminalHud | 模型/档位、占用、只读原生模式、工具、目录/Git，以及不可靠指令路径隐藏均通过；[HUD](evidence/dsh-stage5/closeout/screenshots/04-dsh-hud.png) | 代表旅程首跑通过；未知字段隐藏，不猜测已读指令文件 |
| 5 执行模式 | 原生 commands.execute / permissionPresets.set；原生权限偏好会全局持久化 | 本期只读徽标，manifest 模式菜单关闭；真实 App 确认无模式按钮 | 已裁决范围通过；不宣称可切换。未来开放仍需裁决全局偏好写入 |
| 6 自动标题 | 原生 session/title；title policy、共享重命名/恢复菜单 | 自动跟随、手动锁定、原生改名后恢复最新标题通过 | 首跑通过；沿用原生标题，无新增标题生成请求 |
| 7 载入会话 | 原生 v4 历史与 cwd；DSH catalog、SessionLoaderDialog | 原生子会话预览、搜索和精确载入后答出父暗号；[预览/搜索](evidence/dsh-stage5/closeout/screenshots/07-child-preview-search.png) | 首跑通过；边界/损坏/分页由既有自动检查覆盖，不声明大规模目录真实 UI 压测 |
| 8 重启恢复 | 原生 --resume；ProviderResumeMonitor/恢复队列/横幅 | 正常重启静置和暗号记忆通过，[恢复记忆](evidence/dsh-stage5/closeout/screenshots/08-memory-restored.png)；受控早退后按钮仍显示“当前终端仍在恢复”，[失败横幅](evidence/dsh-stage5/closeout/screenshots/08-restore-failed.png) | 部分通过；失败场景两次均卡 :439。成功重试、重试后记忆与新开均未执行；未闭合 |
| 9 Fork | 原生 /fork 创建独立子会话；卡片 fork=false | 原生 Fork 的独立 ID/父关联、目录读取、搜索、载入与记忆通过；卡片菜单不提供一键 Fork | 已裁决阶段范围首跑通过；未注入 /fork，不复制上游编排；一键能力留后续正式上游调用口 |
| 10 mt 跨卡 | 原生独立 system section 与 shellEnv；现有 mt CLI/server | 默认 workspace-write 下建 DSH 卡，identify/list/read 读回独立 Shell 兄弟屏幕随机标记，原生工具结果与回复均确认；[跨卡](evidence/dsh-stage5/closeout/screenshots/10-mt-screen-read.png) | 代表读取旅程首跑通过；其它 mt 操作沿用已有自动边界，不宣称本轮逐项真实操作全部命令 |
| 11 不提供的入口 | DSH manifest 与共享 HUD/menu | 账户、费用/额度、供应商面板、团队、不可靠项目指令入口均未出现 | 首跑通过；没有补造原生缺失接口 |

[7 条旅程、10 次尝试与未到断言明细](evidence/dsh-stage5/closeout/dsh-acceptance.json)。原始日志 `closeout-dsh-acceptance.log`，完整 proof/每次失败现场分别保存在工作树 `.cache/dsh-stage5/closeout-evidence/`、`closeout-results/`；挑选的8张真实截图已随本矩阵提交。之前 retries=0 的一轮被宿主中断，保留现场但不作为这次完整签收依据。

## 性能：实际状态到 DAG DOM

同一主机的真实 Runtime domain event 与 MutationObserver 时间，门槛每类3次、≤1000ms。首轮旧采样曾错误关联更早的乐观状态，已作废；以下只使用正式收口轮。

| 状态 | Runtime → UI（ms） | 原生 → UI（ms） | 判定 |
| --- | --- | --- | --- |
| 工作中 | 2 / 3 / 1 | UI 早于原生 turn/start 4 / 3 / 2；原程序将负值截成0，该0不可当作有效原生延迟 | Runtime口径3次达标；不声明原生口径有效测量 |
| 回到空闲 | 2 / 2 / 3 | 131 / 7 / 8 | 两口径3次均达标 |
| 需要输入 | 2 / 2 / 2 | 105 / 104 / 105 | 三次分属首跑第1轮及重试第1/2轮；样本达标，审批旅程整体仍失败 |
| 出错 | 未取得 | 未取得 | 错误第一轮未进入预期状态；3次门槛未闭合 |

## 指定回归：收口轮

Runtime 主批 `1653 passed / 36 skipped`，server 独立 `--maxWorkers=1` 为 `166 passed`；合计1819通过、36跳过、0失败。RSS门禁本次通过，之前两次超限与基线副本通过仅保留为历史波动，不改写为稳定无波动。Desktop 功能1031项与性能3项全通过；[完整自动回归记录](evidence/dsh-stage5/closeout/automated-regression.json)。三组界面回归均完整执行一次、0重试，结果如下。断线前已完成的组未重跑；重连后只补尚未启动的Codex。不使用历史单条补测冒充本轮全套。

| UI 回归 | 通过 / 失败 / 跳过 | 与已知矩阵对照及未执行项 |
| --- | --- | --- |
| Claude 四规格 | 10 / 3 / 0（1.8分钟） | Fork :187 relations=[]、:276失败卡为0均历史已知；载入 :164搜索到2/2但虚拟预览无350，与历史同类失败一致，后续曾过，不宣称稳定修复。失败后的恢复/载入步骤未执行 |
| Antigravity | 2 / 3 / 0（8.9分钟） | :497等DAG idle 90秒、:802自动标题30秒为空、:408模型HUD 45秒未出现，均不同于最新矩阵已知:876选择器失败；是本轮新失败点，原因未知，后续标题/重启/Fork/载入部分未执行。mt及受控异常通过 |
| Codex | 7 / 3 / 0（8.8分钟） | :559和:805首次对话等待身份30秒为空，后者尚未执行到重启；:862当前目录Fork子UUID等于父UUID，后续隔离/新Worktree/载入未执行。三处为相对最新矩阵的新失败点，原因未知；模型错误通知、防升级、祖先历史、Default权限恢复、mt、审批中断、异常恢复及受控缺命令/未登录通过 |

[完整UI回归报告](evidence/dsh-stage5/closeout/ui-regression.md)、[结构化结果](evidence/dsh-stage5/closeout/ui-regression.json)。已知对照来自现有矩阵历史，本轮没有另开main实验。不能把六个新的失败点一概归为旧问题，也不能仅凭失败就确定由DSH改动造成；目前不能给出“其它Agent无新回归”的签收结论。

## 仍未闭合与本轮边界

DSH审批后续轮次、模型错误诱发到状态/通知/性能的闭环、恢复失败后的重试和新开未闭合。Claude三处失败、Antigravity三处新失败、Codex三处新失败及其被阻断的后续断言均保留；错误性能三次门槛也未闭合。没有放宽断言、删除权限检查、追加实验、继续修产品或推送发布。

本轮已到收口交回关卡，在此停止。打包App未签名、未安装到Applications、未发布；不将自动门禁全绿当作真实产品签收。原有未跟踪PRD保留。

## 我自己做的决定

1. 主代理：恢复早退用error并结束等待；HUD在DSH数据边界取整，避免扩展共享UI。恢复修改虽有单测，真实按钮仍失败，保持未闭合。
2. 主代理：fixture启动最多等5秒，与产品恢复身份期限分开，未改变产品10秒期限；补上早退后同ID重试的回归与说明。
3. 主代理：产品源码打包后未变，604文件指纹一致，复用最终包；仅测试和文档变更不重打包。
4. 主代理：running的原生时间晚于乐观UI，截零值不当有效原生延迟；审批样本注明分属两次尝试，不用样本达标替代整项通过。
5. terra界面代理：被中断的旧retries=0轮保留为非正式现场；按新要求独立执行retries=1全套。重连后确认Claude/Agy已有完整汇总，只补未启动的Codex；精确核对归属后结束旧测试残留，清理本轮临时认证/链接，不动用户真实App或认证。

步骤提交：产品修正`00d9940`，测试说明`1d736bf`，源码门禁`4fbd0bf`，包复用`86e5891`，DSH正式验收`dde6ca2`，完整回归`aaf25f1`。原始日志与完整临时现场保留；本矩阵及挑选截图/脱敏结果为提交证据。

以下保留前关已审记录，不能替代上面的最终 App 失败结论。

## 第四关结果与产品边界

| 场景 | 原生基线与 Matou 实际结果 | 结论及边界 |
| --- | --- | --- |
| 原生命令可用路径 | 新建真实 PTY 会话中通过官方 `commands.execute` 执行 `/plan`、`/plan off`；plan projection 即时 true/false，agent 始终 idle。`permissionPresets.set` 切到 read-only，再恢复 workspace-write，projection/current 一致 | 官方调用确实可用；没有伪造切换成功，也没有调用 danger-full-access |
| Matou 安全切换不可用路径 | TUI 的 `dsh-adapter/channel/mode-permission-actions.js` 在权限 preset 事件后调用 `persistPermissionPref`，`permissionPrefs.js` 写全局 `permission.json`；真实探针产生了该文件 | 与不改全局配置的约束冲突。按回答 #6，本期只读显示原生模式；manifest 为 `permissionModes: []`、`permissionSwitchMode: none`，菜单不出现。没有新增 Matou 写模式入口；将来要提供需重新裁决持久化取舍 |
| 探针副作用恢复 | 首次 `/plan on` 将 on 当附带消息并发起模型请求，样本作废。有效样本改为无参 `/plan`。新建 permission 文件的创建/修改时间、workspace-write 内容及项目内备份均再次核实 | 按回答 #6 仅删除该探针新文件；目录清单对比：仅移除 permission.json，其它增删改均为零。保留清理证据，不恢复共享运行索引 |
| 独立 mt 说明 | 本 run 观察插件向精确 agent.ctx 注册原生 `systemPrompt.section`，名称 `matou:host-control`，禁用插值；复用现有 mt 说明全文，仅改 DSH 开头 | 真实原生会话日志确认独立 system section。未改 AGENTS.md / CLAUDE.md、profile 或用户项目文件；说明约 6.1 KB，随已有请求发送，无额外运行时模型请求 |
| 默认沙箱首次失败 | DSH 原生 shell secret scrub 移除了继承的 MATOU_CONTROL_TOKEN，导致 mt 无身份。默认 workspace-write 的 IPC 路径可用 | 按回答 #7 用官方 `shellEnv.register` 显式转交本 run 的 DSH 专用候选；CLI 仅原变量缺失时兜底。没有放宽沙箱、增加 RPC/socket 或新存储 |
| 最终真实跨卡 | 两个独立 Matou 卡片，兄弟 Shell 屏幕打印随机标记；真实 DSH 在 workspace-write 中依次执行 `mt identify --json`、`mt list --json`、`mt read right --json` | 服务端确认 DSH 的 mt 读取目标为兄弟卡；返回 source=screen 且含随机标记，原生工具记录和模型回复都含标记，最后收到 completed。真实 Runtime/node-pty 探针通过 |
| 身份与隐私边界 | 前台顶层 agent 可取本 run 令牌；后台、子代理、相同 ID 的其它对象、旧 run、解绑后和退出后均拿不到；原 MATOU_CONTROL_TOKEN 优先，原值无效不改用兜底，撤销令牌失败 | helper、实际 bridge 归属与真实 CLI/server 回归通过；令牌未插入 system prompt，最终原生会话日志全量扫描无令牌值 |
| 配置核对 | mt 最终探针启动前备份 40 份文件，34 份哈希不变，6 份变化均为原生 workspace / 最近使用 / 会话索引 / 会话挂载 / usage 运行数据；无新偏好文件 | profile 和受保护偏好未变；没有整文件回滚共享索引 |

脱敏汇总：[evidence/dsh-stage4/summary.json](evidence/dsh-stage4/summary.json)。原始 PTY、会话、配置备份及失败样本位于工作树忽略目录 `.cache/dsh-stage4/`，探针位于 `output/dsh-stage4/`；不提交真实标识、提示词、账号或原始正文。先前探针误取 preserve-navigation 返回的当前卡而非 created.session，产生 journal 缺失；已修正夹具。该失败样本从启动文件读出的标记不计作跨卡证据。

第四关自动验证：Runtime 相关 13 文件共 215 条唯一测试（其中新加实际 bridge 隔离场景在最终定点复跑），DSH HUD 5 条、domain 12 条、资源依赖门禁 2 条通过；mt CLI 的最终修正后 114 条复跑通过。全工作树 typecheck 与最终 Runtime typecheck 通过；`pnpm check:identifiers` 与 `git diff --check` 通过。没有更改其它 Agent 的既有断言。真实模式和跨卡均由 PTY 验证；未执行全量测试或真实 Electron 界面验收。

## 第三关真实 PTY 结果（已审保留）

| 场景 | 原生基线与 Matou 实际结果 | 结论及边界 |
| --- | --- | --- |
| 新会话 HUD | Python `pty.fork()` 运行真实 `dsh dsh-tui --patch`；同一前台 agent 的 current model / reasoning effort 与公开 projection 提供模型、档位、原生模式、真实上下文占用 | 管线实测通过；没有从累计 token 或终端文案猜占用率 |
| 工具活动 | 仅要求新会话读取自建 Git 目录内 README；收到原生 read 开始与结束，HUD 计数 1、状态 completed | PTY 通过；桥接不发送正文、工具参数或结果 |
| 原生标题 | 原生自动标题先后更新；随后在该 PTY 执行 `/rename Matou DSH Stage3`，Matou hook 同步新标题 | PTY 通过；Matou 不额外调用模型生成标题 |
| 标题锁与恢复 | 手动命名后继续保存新的 provider title，卡片名称保持手动值；恢复自动标题后使用最新原生标题；空标题保持原名 | hook + repository 定点测试通过；真实 App 菜单点击留到下一关 |
| 目录预览与搜索 | 按该 PTY 的 cwd 编码定位、再核对 header.cwd；真实多帧历史读回 8 条可见内容，标记搜索命中 2 条，原生新标题可见 | 真实文件 + RPC list/detail/search/load 通过；UI 页签/分页/载入由组件回归覆盖 |
| 静置恢复 | 用真实 RuntimeServer/node-pty 恢复本关已结束的同一会话；之后关闭并重新打开同一 Runtime 数据库，自动恢复计划再次启动原 ID；两轮均零输入收到 persisted + ready | 真实 PTY / Runtime 数据库重开通过；分别约 4.9 / 3.1 秒，包含 1.2 秒额外观察，不作产品性能指标 |
| 恢复握手与失败 | 即使输出完整终端屏幕，DSH 恢复队列也不提前完成；同 ID 但未保存的 ready 不确认；不同 ID 或期限内无 ready 进入 failed | Runtime 定点测试通过；其它 Agent 的输出/期限判定保持原样 |
| 重试与新开 | failed 后重试仍带原 ID；新开不带 `--resume`；旧 run 的迟到 ready 不能覆盖失败或新 run | Runtime + 既有 transition 通过；真实 App 横幅按钮留到下一关 |
| 配置保护 | Python PTY 启动前备份 35 份配置，33 份受保护文件哈希一致，仅 workspace / last-used 原生运行索引变化；真实恢复阶段另备份并核对 35 份，全部一致 | 没有修改 profile、模型偏好、版本或回滚共享索引 |

脱敏汇总：[evidence/dsh-stage3/summary.json](evidence/dsh-stage3/summary.json)。原始 PTY、patch、自己创建的会话 ID、完整自建样本历史及配置备份只放工作树忽略目录 `.cache/dsh-stage3/`；探针驱动位于忽略目录 `output/dsh-stage3/`，不提交真实 ID 或私有正文。

## 数据与读取边界

- HUD 沿现有 hook → handler → registry → wire → TerminalHud 管线，只加 DSH 分支。获批的可选 `providerPermissionLabel` 是只读原生文字，不映射成 Claude 权限枚举或开放切换菜单。缺字段隐藏，真实测得的 0 可以显示，未知值不补 0。
- 模型/推理档位来自 `agent.model.current()`；上下文采用公开 `contextPressure.projectedTokens ?? pressureTokens` 除以真实 `contextWindow`；标题来自公开 title projection / `session/title`；目录来自 workingDirectory projection / 当前 session header。工具只统计已观察到的配对事件，不回放历史通知。
- Git 继续使用 Matou 现有环境探测。没有可靠的“实际已读项目指令文件路径”证据，按交代隐藏指令文件入口。费用、额度、账户、团队不显示。
- Catalog 只读 v4 JSONL 与串联独立 Zstd 帧；逐帧扫描、解压与校验，完整损坏或未知必要事件明确报错；末尾未写完帧不发布，文件增长后重新读取。可见内容按 surface append/replace 投影，压缩替换掉的历史不再出现在预览或搜索。
- 读取有界：单压缩/解压帧 32 MiB、单行 16 MiB、压缩文件/累计解压 512 MiB、单会话可见事件 10 万；索引/搜索缓存各最多 16 会话且总引用/命中 20 万，缓存不保存正文。响应沿用已有分页；大页按字节缩小，有单条/预览文字上限。超限只报错，不改写会话文件。
- 载入复用现有 RPC 与 provider binding，启动仍是 `--resume <完整ID>`。DSH 只在同 ID、已落盘、真实 ready 后结束恢复；沿用现有 10 秒恢复期限，期限内未确认走现有 failed / 重试 / 新开流程。没有新数据库、迁移、依赖或 Matou 运行时 LLM 请求。

## 第三关历史 reference product 交互对照矩阵

DSH 原生 TUI 为本 provider 的行为基线；Matou 卡片层沿用已批准 CLI 黑色工作区范围。组件验证不代表真实打包 App 的尺寸、焦点、滚动或交互性能已验收。

| # | 能力与原生基线 | 当前 Matou 结果 | 差异 / 未闭合项 |
| --- | --- | --- | --- |
| 1 | `dsh dsh-tui` 在指定 cwd 新建 | manifest、启动参数、精确 Shell 升级与真实 PTY 启动在第二关已审通过 | App 菜单、首帧、焦点和滚动未验 |
| 2 | 原生 agent / turn / 审批事件 | 工作、等待、完成、错误和前台隔离已审；本关 HUD/标题事件不改变原映射 | App 可见状态与终端负载响应未验 |
| 3 | 原生完成/审批通知 | 单次 patch 关闭原生完成通知，Matou 标题 DSH；重复、旧 run、后台隔离第二关已审 | macOS 通知与点击回卡未验 |
| 4 | 原生模型/档位/上下文/工具/目录/Git/指令 | 真实字段管线与缺失隐藏、只读模式、工具计数通过；Git 复用环境；指令路径未知则隐藏 | 打包 App HUD 外观/交互性能未验；没有可编辑指令入口的成功声明 |
| 5 | 原生权限与计划模式 | 官方切换已实测；因权限持久化与约束冲突，按已审裁决只读显示、manifest 关闭菜单 | 本期边界已闭合；将来提供切换需重新裁决全局偏好写入 |
| 6 | 原生 session/title | 自动跟随、手动锁定、恢复最新标题、空标题保留原名；菜单复用 manifest 动态名称 | 真实 App「恢复 DSH 自动标题」点击未验 |
| 7 | 原生会话目录与选择器 | DSH 页签、cwd 编码与 header 二次校验、多帧/损坏/缺失、有界缓存、分页/预览/搜索和精确载入通过 | 真实 App 大目录/加载反馈/焦点/滚动未验 |
| 8 | 显式 resume 与恢复 | autoRestore 开启；真实 Runtime 数据库重开后原 ID 零输入 ready 通过；失败、重试和新开定点测试通过 | 打包 App 退出重启、横幅按钮与可见「启动中→已恢复」留下一关 |
| 9 | 原生 `/fork` 子会话 | 按已审裁决 fork=false，不显示卡片一键入口；目录不依赖父会话猜测，原生子会话按自身 ID/cwd 读取 | 不注入 `/fork`、不改草稿；真实原生 Fork 子会话载入仍未专测；正式上游调用口可用后再评估一键入口 |
| 10 | 原生 system-prompt section / shell | 独立 literal section + 官方 shellEnv 前台定向转交；默认 workspace-write 下真实 identify/list/read 和兄弟屏幕读回通过 | PTY 与隔离回归通过；真实 App 跨卡入口和交互性能未验 |
| 11 | 账户/供应商/团队等 | DSH manifest 关闭；HUD 显式隐藏账户/额度/团队 | 符合范围；真实 App 入口显隐未验 |

## 第三关自动验证与未执行项（已审保留）

- 全工作树 `pnpm typecheck` 通过；最终 Runtime 文案/目录边界修改后，Runtime typecheck 再通过。
- Runtime 的 DSH / HUD / hook / repository / RPC / provider-mode 13 文件、165 测试通过。
- runtime-server 恢复/身份相关子集 57 测试通过，108 条因名称筛选未执行；其中新增 DSH 的匹配、不同 ID、超时重试、超时新开 4 场景。
- TerminalHud / SessionLoaderDialog / HierarchyShell 8 文件、196 测试通过；开启 manifest 能力后统一复跑。原有其它 Agent 行为断言未更改；总注册/页签列表仅增加 DSH。
- manifest 5 测试通过；`pnpm check:identifiers`、`git diff --check` 通过。
- 真实 Python PTY HUD/标题/目录探针 1 条、真实 Runtime/node-pty 数据库重开探针 1 条通过。

未执行全量测试、打包、真实 Electron 界面、通知中心和完整产品/性能签收。第二关已审证据保留于 [evidence/dsh-stage2/summary.json](evidence/dsh-stage2/summary.json) 与 Git 历史；第四关证据已在上方补齐，本关停下交回。
