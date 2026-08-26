# PRD 06 会话 Fork 验收

状态：实现完成，待产品验收

## 1. 用户可获得的结果

- 用户在已经形成恢复身份的 Claude 面板可操作区域（包含黑色终端内容区）右键，会同时看到 `⑂ Fork 会话` 与 `↗ 独立窗口`；Shell、刚启动且尚未形成首轮对话的 Claude、团队队友和脱出窗口均隐藏 Fork。高频状态栏只负责模型、上下文等展示，不再把“刚拿到临时 sessionId”误判成可 Fork；首轮真实对话形成后才开放入口。Shell 通过 `claude` 或用户已配置为 Claude 的 `cc` 别名启动后，面板标题先同步为 `Claude`，避免用户把入口误认为 Shell 能力；Claude 退出后恢复为 `Shell`。
- 点击 Fork 后，新 Claude 面板立即出现在源面板正右侧并自动聚焦；源面板内容、进程、滚动与输入现场继续保留。
- 新面板以源会话身份执行一次 `--resume SOURCE --fork-session`，继承点击时的完整上下文；获得自己的身份后，新旧面板各自收发、各自恢复。
- 用户可以在派生面板上继续 Fork；每次都创建新的右侧面板和独立会话，不复用此前结果。
- 分叉失败时，右侧面板仍保留并显示 `[Fork 未完成，请检查上方原因后重试]` 与供应方错误；该面板不会落到 Shell，也不会拿源会话继续运行。
- 应用重启后，每个成功面板只按自己的身份 `--resume`，历史 Fork 动作不会再次执行。
- 为后续 DAG 会话关系保留内部 `forked-from` 父边；本期界面不增加徽章、连线或分支树。

## 2. 已确认的产品基线

2026-08-25 产品确认采用以下处理：

1. 当前可运行 Kooky 在 `terminalContextMenu.mjs` 中临时返回空菜单；同文件保留完整 Fork / Detach 逻辑，`ClaudeCodeView.vue`、`forkSession.js` 与启动层也保留完整实现。Matou 激活这套已存在的 Kooky 交互，而不是复制当前隐藏状态。
2. PRD 06 写明不保存派生关系；项目已定的会话 DAG 演进要求保存父子来源。Matou 内部保存 `forked-from`，本期用户视觉仍与 PRD 一致，不出现关系装饰。
3. 入口可用性采用 PRD F2 的最终细则：身份尚未形成时直接隐藏，而不是显示灰色项。

详细双基线矩阵：`docs/parity/prd-06-kooky-parity.md`。

## 3. 16 项用户行为验收台账

| # | 用户场景 | 当前用户结果 | 权威证据 | 状态 |
|---:|---|---|---|---|
| 1 | 可恢复 Claude 面板内容区右键 | 在终端内容区直接打开菜单，同时显示 Fork 与独立窗口，顺序和文案与 Kooky dormant UI 一致 | Electron 内容区右键回归 + 真实系统鼠标操作 | 通过 |
| 2 | Shell 面板右键 | 标题显示 `Shell`，Fork 隐藏；通过 `claude` 或已配置的 `cc` 进入 Claude 后标题切为 `Claude` | Runtime title transition + Electron PRD 02 / 06 | 通过 |
| 3 | Claude 尚未形成首轮对话 | 状态栏即使已报告临时身份，Fork 仍隐藏；首个真实对话事件确认后立即出现 | Provider hook / HUD authority tests + 真实 Claude 成功场景 | 通过 |
| 4 | 团队队友面板右键 | Fork 隐藏 | component + application-service eligibility tests | 通过 |
| 5 | 脱出窗口右键 | Fork 隐藏 | Electron 成功场景 detached assertion | 通过 |
| 6 | 点击 Fork | 新面板嵌在源面板正右侧，菜单关闭，新面板输入光标聚焦 | Electron 成功场景 bounding-box / focus assertions | 通过 |
| 7 | 上下文继承 | 仅首次启动带 `--resume SOURCE --fork-session` | Runtime exact-args test + Electron invocation log | 通过 |
| 8 | 独立身份 | Hook 产生新 provider identity；新旧输入和回显互不串台 | Electron 三会话输入隔离 | 通过 |
| 9 | 目录与归属 | 三个会话 cwd 完全相同，task / scene 沿用当前层级 | application-service + Electron SQLite assertions | 通过 |
| 10 | 源面板现场 | Fork 前后的源 Session ID 与进程保持，内容继续可操作 | Electron success flow | 通过 |
| 11 | 派生面板再次 Fork | 第二次派生位于第一派生右侧，身份独立 | Electron continuous-fork flow | 通过 |
| 12 | 应用重启 | 三个面板按各自身份恢复；Fork 参数总次数保持为 2；安静启动的真实 Claude 不再被 10 秒输出量阈值误杀 | Electron restart invocation assertions + 真实 Claude 重启验证 | 通过 |
| 13 | Fork 失败 | 右侧面板显示供应方错误与 Kooky 警示行 | Electron failure flow + `fork-failure.png` | 通过 |
| 14 | 失败面板输入 | 键盘输入没有到达供应方，也没有启动 Shell | Electron input log + Runtime inert-panel tests | 通过 |
| 15 | 关闭一方 | 关闭派生面板后源面板 PID 与交互继续保持 | Electron success flow | 通过 |
| 16 | Fork 与 Detach 共存 | 同一菜单并列；Fork 后源会话仍可脱出并复用同一进程 | Electron success flow | 通过 |

## 4. 产品数据与恢复规则

- **一次性意图**：创建面板、布局、Fork 启动意图和关系边在同一个权威事务内提交。首次运行认领意图，获得派生身份后原子结算为 `succeeded`。
- **恢复身份隔离**：源会话保存源 ID，派生会话保存派生 ID；重启时每个面板只读取自己的 ID。
- **失败保持可见**：失败状态与错误 Journal 都落盘；重新打开失败面板仍显示同一结果，警示行只写一次。
- **关系演进**：`forked-from` 同时进入 append-only relation event 与 current projection，为后续 DAG 投影保留事实来源；当前 CLI 界面保持普通 Claude 面板外观。

## 5. 自动化与运行证据

- PRD 06 Electron 场景覆盖：入口显隐、菜单样式、右侧布局、自动聚焦、三会话连续派生、输入隔离、cwd / 归属、内部父边、重启不重放、关闭隔离、Detach 共存、失败可见与输入无效。
- Runtime 覆盖：Kooky 精确启动参数、一次消费、派生身份结算、恢复只 resume、自身身份丢失时失败、进程快速退出、Journal 单次警示、HUD 恢复可派生；另覆盖状态栏临时身份不提前开放 Fork、Fork / Resume 的状态栏确认解除启动超时、SessionEnd 在 PTY 退出后的短暂收尾窗口。
- 完整工作区自动化共 376 项通过：Contracts 16 项、Domain 3 项、Desktop 103 项、Runtime 254 项。
- 全量 Electron 回归 34 个用户场景通过，其中 PRD 06 新增 2 个端到端场景；类型检查与生产构建通过。
- 真实 Claude Code 端到端验证：验收全程使用本机 Claude Code、系统级鼠标右键与系统级键盘输入；新会话首轮前隐藏 Fork；完成首轮后在终端内容区右键并 Fork；派生面板保持运行超过启动截止时间并继承源历史；两侧分别输入且互不串台；应用重启后两侧都保持 Claude、各按自己的身份恢复；派生侧重启后继续输入成功；全程未出现 Fork 失败横幅与 Hook 404。模拟供应方仅用于低成本回归，不计入产品端到端验收证据。
- 运行证据：
  - `docs/acceptance/evidence/prd-06/kooky/runnable-menu-hidden.png`
  - `docs/acceptance/evidence/prd-06/kooky/fork-source-baseline.json`
  - `docs/acceptance/evidence/prd-06/matou/fork-menu.png`
  - `docs/acceptance/evidence/prd-06/matou/fork-menu.json`
  - `docs/acceptance/evidence/prd-06/matou/forked-conversations.png`
  - `docs/acceptance/evidence/prd-06/matou/fork-failure.png`
  - `docs/acceptance/evidence/prd-06/matou/real-claude-fork-validation.png`
  - `docs/acceptance/evidence/prd-06/matou/real-claude-fork-validation.json`
  - `docs/acceptance/evidence/prd-06/matou/real-system-fork-menu.png`
  - `docs/acceptance/evidence/prd-06/matou/real-system-fork-validation.png`
  - `docs/acceptance/evidence/prd-06/matou/real-system-fork-validation.json`

## 6. 产品验收建议

重点体验四条路径：

1. **入口出现时机**：刚进入 Claude 时右键，再完成首轮后右键，确认 Fork 从隐藏变为可点击。
2. **主线与支线**：从已有对话 Fork，在两侧分别继续输入，确认上下文继承且后续互不串台。
3. **连续探索与恢复**：在派生面板再 Fork，重启应用后确认三个面板分别回到各自会话，且没有多出新分支。
4. **失败与窗口协作**：观察失败面板中的明确提示；在成功 Fork 后把源面板移到独立窗口，确认两种能力各自成立。

PRD 06 的需求台账、Kooky 双基线与真实 Electron 行为已经闭合，等待产品验收。
