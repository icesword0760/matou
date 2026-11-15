# Codex 打包版交互对照矩阵

2026-11-15，`feat/multi-agent-codex`。矩阵、截图和性能已获接受。本轮仅收口三处：**Bypass通过，Default审批策略恢复失败；正常静置重启通过；Antigravity同一失败已在main复现，属于既有测试选择器问题**。Default仍未闭合，第五关不宣称全项通过。最终包之后没有修改产品源码。

## 最终产物与改动原因

- 产品提交 `e62abde`；打包指纹提交 `96f4f40`；最终验收结果提交 `03555c8`；回归提交 `23563de`。旧包对应的 `60714be` 不作为最终验收依据。
- 重新执行一次 `CSC_IDENTITY_AUTO_DISCOVERY=false pnpm package:dir`，exit 0。产物 `apps/desktop/release/mac-arm64/码头.app`，未签名、未安装到 Applications、未发布。
- `app.asar` SHA-256：`69e4cc57bf526447b0e697a5df184f0364ff68d5b11418da9cfdc4761962d597`。
- runtime SHA-256：`50bd93fc1419318c7ef2b837503a65b0f90ab8655f1ccd1c7c284f190ee4180f`。完整 [最终指纹](evidence/codex-stage5/final/package-sha256.json)；旧包指纹单独保留为 [首次包指纹](evidence/codex-stage5/package-initial-sha256.json)。

`e62abde` 的用户可见改动：

| 改动 | 用户原因 | 验收结果 |
| --- | --- | --- |
| Codex 权限选择实际重启并传递对应参数 | 选择 Accept Edits / Plan 后，界面显示和实际可写范围必须一致，并保留原对话 | Accept Edits、Plan、Bypass真实通过；Default进程和记忆通过，但审批策略未恢复配置值 |
| Fork 沿用 Matou 已选目录 | 选了新 Worktree 后，不应又在被进度层遮住的 CLI 内等待重复选目录 | 当前 / 新 Worktree 均真实通过 |
| 缺命令提前检查 | 未安装 Codex 时能看到启动失败卡，避免卡在无效会话 | 受控不存在命令的打包 UI 通过 |
| 恢复失败保留 error 状态 | 恢复记录缺失时，卡片与 DAG 应一致显示出错，并允许重试或新开 | 真实隔离 rollout 暂移、还原重试、新开均通过 |
| Shell 升级后及时刷新 DAG | 用户先打开 DAG 再输入 codex，也应立即看到 Codex 标签 | 最终两次核心启动均通过，旧包 Shell 标签停留差异已闭合 |

## 验收口径

- [测试入口](../../tests/e2e/codex-acceptance.spec.ts)：最终打包 executable、真实 Codex **0.162.1**，模型 `gpt-6-astra` / medium；真实 HOME，空 ZDOTDIR，隔离 git 仓库、MATOU_DATA_DIR、Electron user-data 与 CODEX_HOME。认证仅临时复制，结束即清除。
- 启动 wrapper 显式使用 `--no-daemon --no-alt-screen` 并关闭更新提示；出现升级提示即 Escape 并关闭本次 App。没有覆盖默认 daemon 与所有 TUI 组合。
- 上一轮完整规格只跑一次，`--retries=1`。六个场景共 **7 次尝试**：核心失败后重试一次仍失败，其余五项不重试。单次上限5分钟；原始失败保留。本轮用户另授权定点收口，尝试次数见下。两次核心实际耗时约 108 / 100 秒，全批约 6.6 分钟。
- 真实回复须在相同新 turn 中同时匹配用户问题、assistant `final_answer` 和 `task_complete.last_agent_message`；不接受历史回显或问题包含的暗号。
- 普通探针显式 on-request/read-only，`mt` 探针隔离为 danger-full-access。审批通过不代表任意用户 default 配置都会触发审批。
- 本期 PRD 指定现有 Claude 卡片为同场景基线。以下同时列出可运行产品回归与代码入口；只比较 CLI 模块，外部品牌与产品外壳不计入。历史材料明确标为历史。

## reference product 交互对照矩阵

下表原有截图与JSON默认相对于 [最终证据目录](evidence/codex-stage5/final)；本轮新增证据明确以closure路径链接。上一轮详细记录见 [最终UI报告](evidence/codex-stage5/final/ui-report.md)，本轮三项记录见下方收口节。

| 场景（PRD #） | Claude 同场景基线与代码入口 | Codex 最终实际 / 证据 | 结论 |
| --- | --- | --- | --- |
| 1 Shell 输入 / mt 新建 | 现有 Shell 升级与 `mt create session --profile`；`runtime-server.ts`、`provider-launch-plan.ts`、`mt-cli.ts` | Shell 输入出现真实 CLI；mt 创建真实 Codex 卡。`01-codex-starting.png`、`11-mt-cross-card.png`、`results/mt.json` | 通过 |
| 2 主卡与 DAG 标签 | 共享 `DagCanvas.tsx`、provider mode 投影；Claude 四规格回归 | 先打开 DAG 再升级，主卡和 DAG 均显示 Codex；两次核心均过断言 | 通过；旧包差异已闭合 |
| 2 启动 / 工作 / 空闲 / 等输入 / 出错 | `SessionWorkStatusService`、`ProviderModeService`；Claude 通知 / 持久化路径 | starting、running、idle、审批 needs-input、恢复失败 error 均走真实 CLI；`results/latency-round-*.json`、`02-approval-*.png`、`02-real-error-dag.png` | 五态通过；不把命令退出127当整轮模型 error |
| 3 完成与审批通知 | 共享 notification store、`agent-event-ingestion`；Claude 通知四项回归 | 另一卡聚焦时完成通知与审批通知可见；批准后继续，中断后新 turn 回复成功。`03-completion-notification.png`、`02-approval-notification.png`、`results/approval-interrupt.json` | 这两类真实通过；聚焦免打扰 / 声音 / 冷却由共享自动规则覆盖，未单独做真实 Codex 音频验收 |
| 3 错误通知 | 共享错误通知管线 | 未诱发真实模型级失败，未验证 | 按本轮指示保留，不强行造错 |
| 4 HUD 字段 | `TerminalHud.tsx`、HUD service；Claude HUD 回归 | 图中可见 gpt-6-astra (medium)、486K context、5%、1 Tools、~/repo、main；无5h / Weekly。`04-hud.png` | 代表场景通过；完整上下文百分比算法尚无独立模型预算真值对照 |
| 5 Accept Edits / Plan | 共享权限菜单；`provider-mode-service`、`provider-launch-plan.ts` | 两次均 PID 改变、UUID不变，真实新 turn 回答原暗号，rollout 分别 workspace-write/on-request 与 read-only/on-request。`results/modes-progress.json` | 通过；Codex 重启为 PRD 明确差异 |
| 5 Bypass / Default | 同一四项菜单及高权限确认；`TerminalHud.tsx` | 修为等待alertdialog并点具名确认。PID 1721→3381→4685，UUID不变，两个新turn均答出暗号。Bypass为danger-full-access/never；Default为read-only/never，未回到配置on-request。[实测记录](evidence/codex-stage5/closure/codex-ui-report.md)、[原始结果回读](evidence/codex-stage5/closure/results/modes-observed.json) | Bypass通过；Default进程/记忆通过，审批策略恢复失败 |
| 6 自动标题 / 手工锁定 / 恢复 | `provider-title-source.ts`、pane 菜单；Claude 同类规则 | 自动标题来自真实索引；重命名后新回复不覆盖，恢复原自动标题。`06-title.png`、`results/core-front.json` | 通过 |
| 7 列表 / 预览 / 搜索 / 载入 | `SessionLoaderDialog.tsx`；Claude load-existing 两项回归 | Codex 页签、父会话预览、全文搜索、载入父 UUID及新 turn 暗号通过。`07-loader.png`、`07-loaded-memory.png`、`results/fork-loader.json` | 父会话场景通过；history_base 祖先历史见边界 |
| 8 正常重启静置 / 首轮接回 | 共享恢复monitor / queue / provider binding；既有持久化入口 | 独立重启静置10003ms，DB starting、可见DAG节点status-starting，未显示已恢复；不含暗号的新问题，同UUID新turn答出暗号。[补测记录](evidence/codex-stage5/closure/codex-restart-recheck/results/summary.md)、[逐turn证据](evidence/codex-stage5/closure/codex-restart-recheck/results/closure-restart.json) | 通过；共享DAG文案仍为“运行中”，starting以节点状态类与DB核验，不另改文案 |
| 8 恢复失败 / 重试 / 新开 | 共享恢复横幅；`provider-mode-service`；Claude恢复同类入口 | 仅暂移隔离真实rollout，失败卡和DAG error一致；还原并重试取回暗号；再次暂移后新开得到不同UUID。`08-real-*.png`、`results/recovery-failure.json` | 通过；不替代正常静置重启 |
| 9 当前目录 / 新 Worktree Fork | `fork-workflow-service`；[既有真实 Claude Fork](evidence/prd-06/matou/real-claude-fork-validation.json) 为历史，另有本轮回归 | 两子UUID不同、forked_from父UUID匹配、cwd匹配、继承父暗号；父回复不包含子暗号。`09-fork-current.png`、`09-fork-new.png`、`09-fork-dag.png`、`results/turns-fork-load-M4nFbe.json` | 通过；外部同父并发未实测 |
| 10 AGENTS.md | `TerminalHud.tsx` 共用编辑器，Claude指令文件路径 | 界面保存后读真实文件核对成功，`10-agents-saved.png` | 通过；文件名差异为既定规格 |
| 11 mt 跨卡 | `mt-cli.ts` / host control / caller identity；一期同类路径 | 兄弟Shell随机令牌不写入用户问题；真实Codex执行mt read并正确回答，命令exit0与输出核对。`11-mt-cross-card.png`、`results/mt.json` | 通过 |
| 未安装 / 未登录 | 既有启动失败与登录提示入口 | 不存在命令显示失败卡；登录替身无真实身份绑定。`fixture-missing.png`、`fixture-unauthenticated.png`、对应JSON | 受控fixture通过；未撤销真实登录，不代表真实首次登录流程全测 |
| 交互性能 | 一期 Runtime domain-event→DAG DOM MutationObserver 同口径 | 六样本见下，均≤1000ms | 本机指定负载通过 |

## 性能数值与重试说明

| 轮次 | 工作中 running | 空闲 idle |
| --- | --- | --- |
| 1 | 2 ms | 2 ms |
| 2 | 2 ms | 2 ms |
| 3 | 1 ms | 12 ms |

采样来自核心最终 retry，Runtime 事件发生时间到同主机 DAG DOM 状态变化，目标 session 与 workStatus 一致，工作 / 空闲各三次，最大12ms。原始 [六样本](evidence/codex-stage5/final/results/latency.json) 与 `latency-round-1/2/3.json` 保留；不是从按键到模型首字，也不代表跨平台或任意终端负载。旧脚本将 SessionStart idle 配给 Stop idle 的2605ms不用于最终结果。

`attempt0-latency.json` 实际是在 retry 进行中复制的四样本，命名不准确，**不能作为首跑完整性能数据**。首跑模式/回复保留原 sid，逐尝试耗时与失败见 [Playwright JSON](evidence/codex-stage5/final/results/codex-playwright.json)。没有通过重跑覆盖首次失败。

## 最终源码回归

上一轮全量回归对应产品源码 `e62abde`，该批未改产品或断言；本轮仅修改定点验收脚本，复用既有全量结果。所有首次失败保留，日志见 [最终结果目录](evidence/codex-stage5/final/results)。

| 检查 | 本轮结果 | 范围与解释 |
| --- | --- | --- |
| workspace typecheck / identifiers | 步骤1通过 | `stage5-fixes-typecheck.log`；之后产品源码未变，提交前另跑标识门禁 |
| runtime 全量 | **最终1727个不同用例通过 / 36跳过** | 主批1495通过、62失败、36跳过；runtime-server单独 `--maxWorkers=1` 为161/161，journal为9/9；受影响6文件串行217/218，剩余1条定点通过。不能把初跑写成全绿 |
| desktop 全量 | **1025/1025** | 111文件，初跑通过 |
| desktop 性能 | **3/3** | 独立performance配置，初跑通过 |
| Claude四规格 | **11 passed / 2 failed** | 本轮开发态Electron fixture，非最终包；四规格批次一次、retries=0，具体见下 |
| Antigravity最终包 | **4 passed / 1 failed** | 批次一次、retries=0；生命周期、标题/重启/性能、mt、受控异常通过；Fork/载入被选择器阻断 |
| diff / 最终标识门禁 | 通过 | 三张PNG曾出现压缩字节偶然误报，处理后通过；见下方说明 |

runtime主批的62个首次失败中：59项为临时Unix socket端点路径过长，2项为临时CommonJS替身继承项目 `type=module` 后 `require` 不可用，1项为并发恢复测试报数据库已被活跃Runtime占用。改用已核实的项目短路径、仅在临时fixture目录标明CommonJS，并串行复核6个受影响文件后剩1项socket；其端点仍109字节，再仅把该单项TMPDIR设为授权项目根（103字节）即通过。未改测试或产品。该6个测试文件与当时观察到的main `7315400` 无源码差异；没有在main跑runtime全量，不能据此宣称main的runtime实测通过。最后定点命令中108个未选中用例不重复计入36项跳过。

Claude两项失败仍是 `prd-06-session-fork.spec.ts` 第187行 `relations=[]`、第276行 `.fork-failure-card=0`，与 [一期矩阵](antigravity-reference-matrix.md) 中main **6132501** 历史实跑同点。没有在main重跑这四个Claude规格，不能把旧main结果称为本轮新鲜Claude对照。通知4/4、HUD3/3、载入2/2、Fork2/4。

Antigravity仅失败于第876行的Fork选择器同时匹配历史按钮和创建子分支按钮，触发严格模式错误；该项后续Fork/载入未执行。其余四项真实最终包通过，状态延迟running 2/1/2ms、idle 2/2/3ms。此前旧包2/5的idle超时和受控异常失败本轮未再出现；不能将本轮脚本失败项判通过或据此宣称全部无回归。完整逐项耗时、trace和进程清理见 [最终UI回归报告](evidence/codex-stage5/final/regression-ui-report.md)。本轮已用main `7315400`独立包复现同一第876行、同两个按钮的严格匹配失败，按指定标准判为既有测试选择器问题；不是此次共享产品代码改动造成的该失败。后续Fork/载入仍未运行。

## 本轮三项收口与main对照

本轮没有修改产品代码或重打分支最终包，只修测试和补证据；accepted性能数值不重采样。原始完整规格仍为5通过/1失败的历史记录，不能把定点结果冒充整套重跑全绿。

| 收口项 | 尝试 / 结果 | 事实与边界 |
| --- | --- | --- |
| #5 Bypass / Default | 独立场景一次，33.122秒，整体失败；无重试 | 两次确认均真正按下，PID均变，UUID不变，两个不含暗号的新问题均答对。Bypass通过；Default虽回到read-only，审批仍为never，与隔离配置on-request不同。保留该失败，不私自覆盖Default用户配置语义。 |
| #8 正常静置重启 | 首次43.18秒因主代理误加“恢复横幅必须可见”断言失败；修正测试后仅补跑一次，60.0秒通过 | 用户要求是starting且不宣称已恢复，现有UI不保证该横幅。补测静置10003ms，DAG状态与DB均starting，之后同UUID新turn答出暗号；问题本身无暗号。 |
| Antigravity main对照 | main目标用例仅一次，22.2秒，退出1；无重试 | 与分支同第876行，同“会话历史”与“创建子分支”双匹配。判为既有测试选择器问题；不修本轮范围外的既有测试，不把后续未运行步骤判通过。 |

#5失败的具体可见影响：用户从Bypass切回Default后，文件沙箱回到只读，但需要审批时的策略仍为never，不能按原配置on-request申请审批。已定设计要求Default不主动覆盖用户配置，并明确resume继承方式不符时先报告（[设计方案](../superpowers/specs/2026-11-15-codex-adapter-design.md)第161–166行）。本轮只报告实测，没有为让测试变绿硬编码审批值。

main基线固定为本轮开始捕获的 **73154003bc16e8612a23d7fca8c264b6fb6303c9**，提交时间早于本轮前HEAD `89fea16`。临时detached worktree位于项目内 `.cache/c8/main`；未改正在使用的main工作区或移动main分支。main packages/runtime/desktop均独立从该提交构建，仅复用源码与锁文件无差异的native资源。首次资源准备cwd错误导致的缺资源包已作废，从未用于测试；修正后包才作为对照。

执行的Agy spec两侧文件相同；与main原文件差异仅为已有的三处隔离路径配置，目标用例正文、选择器和断言完全相同，哈希为 `1805c9db12e4791cd299fd2cd6bdd0c73768b4461c4995da8f437c7fc656c7db`。相关 `TerminalPane.tsx` 在两侧无差异，历史与Fork按钮原本都含pane-fork，测试没有排除pane-history。

- main `app.asar` SHA-256：`97f1c60e17cf99253c9feb1d907addf86e6b91d35bb54ad7b84ccd90523ee37d`。
- main runtime SHA-256：`fafcd16e19101d0ac4399f7389db263f975a25c9576b71489f14244bbda0694c`。
- [基线、测试正文和打包来源记录](evidence/codex-stage5/closure/results/main-baseline.json)、[main单项现场](evidence/codex-stage5/closure/antigravity-main/results/summary.md)、[原始Codex失败现场](evidence/codex-stage5/closure/codex-ui-report.md)。

本轮Codex两条首跑均失败，#8定点补跑一次通过；#5不再重跑。main Agy单条一次，无自动重试。没有无限调试；错误通知按用户指示只写“未诱发真实模型级失败，未验证”。

## 已接受边界与未闭合项

- **history_base**：按第四关裁决本期不修。catalog 不展开祖先引用；Fork子会话预览/搜索可能只有当前rollout。父会话载入通过不代表祖先历史完整性通过。
- **外部同父 Fork 并发**：唯一候选须同时满足创建时间、父UUID、canonical cwd；多个候选继续等真实钩子或超时，无外部锁。如果外部Fork恰为唯一候选，可能在真实钩子前暂时绑定错误候选；首次真实钩子不符则撤销并失败。这是已知风险，自动测试覆盖候选/冲突处理，未做外部并发真实验收。
- **本轮未闭合**：Default审批策略未恢复配置on-request，实测仍为never；Bypass、正常静置重启与新问题接回已补测通过。
- **错误通知**：未诱发真实模型级失败，未验证；不强行造错。其余既有未覆盖项仍为声音、结构化update_plan代表样本、上下文百分比与CLI TUI算法的独立真值对照。
- 未测试 Windows、默认 daemon / 备用TUI、签名分发、安装升级；没有发布、push或安装到Applications。
- 最终包后的产品源码未变；[最终完整性回读](evidence/codex-stage5/final/results/final-integrity.json)确认包内所有组件指纹一致、认证副本为0。仅收集测试、文档和截图。三张PNG仅无损重压IDAT以消除压缩字节偶然命中禁用短标识，解压字节逐一相同，原图留在忽略的缓存；没有改像素或放宽门禁。

旧包首次失败 / 逐轮调试材料继续保留在 [历史 UI 现场记录](evidence/codex-stage5/ui-report.md)，不可与本页最终包状态混用。
