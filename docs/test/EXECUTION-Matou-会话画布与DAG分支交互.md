# Matou 会话画布与 DAG 分支交互—独立黑盒执行记录

## 1. 结论

- **102 条均已有结果：PASS 26 / FAIL 49 / BLOCKED 27。**
- 当前版本能够完成默认 Shell、多画布、横向同级 Shell、真实 Claude 模式进入/主动退出/再次进入、独立 DAG、部分排序和完整退出不重跑。
- 核心用户旅程仍未闭环：真实 Fork 子节点出现 `provider session not found`；状态长期误报运行中；右拉分段、DAG跳转定位、运行画布关闭确认/历史恢复、观察位置持久化存在失败。
- 本轮只做 UI 与真实外部行为验收，没有读取/修改产品实现代码、没有改数据库、没有使用 mock/fake provider。

## 2. 测试对象与环境

| 项目 | 值 |
|---|---|
| App | `.worktrees/session-dag-canvas/apps/desktop/release/mac-arm64/Matou.app` |
| Git commit | `a07c09a60ca98653069fb4678b106b05b46726fc` |
| 系统 | macOS 15.7.4 (24G517), arm64 |
| 显示器 | 1920×1080 单显示器 |
| 隔离根 | `/tmp/matou-independent-qa-20260830/` |
| Claude | 真实 Claude Code v2.1.251；OAuth 凭据只读注入，HOME/会话/配置在 /tmp |
| 输入驱动 | Playwright Electron 真实 UI、真实键盘/鼠标、真实 PTY/Git/文件系统 |

### 2.1 隔离事件披露

- 初次使用系统级桌面自动化时，因三个 Matou bundle 共用 Bundle ID，系统误启动旧 worktree/default profile。发现后立即停止并废弃相关结果；记录：`/tmp/matou-independent-qa-20260830/evidence/INCIDENT-001.txt`。
- 此后所有有效结果均由 Playwright `_electron.launch` 指定上述包体绝对路径，并设置独立 `--user-data-dir`、`HOME`、`MATOU_DATA_DIR`。
- 因默认 profile 基线曾被事件污染，`E2E-CAN-008` 记为 BLOCKED，不把隔离工具问题算作产品通过。

## 3. 逐条执行结果

| 用例 ID | 时间（UTC+8） | 结果 | 实际观察 | 证据 |
|---|---|---|---|---|
| E2E-CAN-001 | 05:10:00 | **FAIL** | 首次页面与真实 PTY 可直接输入，但默认工作目录实际为 /Users/icesword，而不是用例夹具 WS-GIT。 | `/tmp/matou-independent-qa-20260830/groups/CAN-blackbox-r1/evidence/results.json` |
| E2E-CAN-002 | 05:10:01 | **PASS** | 点击顶部 + 后直接创建并选中新画布；无类型/工作树弹框，新 Shell 可直接接收真实 pwd。 | `/tmp/matou-independent-qa-20260830/groups/CAN-blackbox-r1/evidence/results.json` |
| E2E-CAN-003 | 05:10:02 | **FAIL** | A/B 节点集合和焦点隔离；A 横向位置 1904→476，DAG 80%→100%；B DAG 110%→100%，观察状态串为默认值。 | `/tmp/matou-independent-qa-20260830/groups/CAN003-r2/evidence/results.json` |
| E2E-CAN-004 | 05:10:03 | **FAIL** | 连续新建名称存在，但双击/可见操作均没有进入画布重命名，重启保名步骤没有入口。 | `/tmp/matou-independent-qa-20260830/groups/CAN-blackbox-r1/evidence/results-cont.json` |
| E2E-CAN-005 | 05:10:04 | **PASS** | 新增事项后切回原事项，原画布与会话现场恢复，真实输入仍进入原焦点会话。 | `/tmp/matou-independent-qa-20260830/groups/CAN-blackbox-r1/evidence/results-cont.json` |
| E2E-CAN-006 | 05:10:05 | **PASS** | 普通横向 Shell 追加队尾、自动焦点，未二次点击即输出唯一真实标记，旧会话未收到输入。 | `/tmp/matou-independent-qa-20260830/groups/CAN-blackbox-r1/evidence/results.json` |
| E2E-CAN-007 | 05:10:06 | **PASS** | 隔离 HOME 下真实 OAuth Claude 连续完成 CAN7_ONE/CAN7_TWO 两轮，回答完成后 textarea 均保持焦点。 | `/tmp/matou-independent-qa-20260830/groups/CLAUDE-FINAL-r3/evidence/results.json` |
| E2E-CAN-008 | 05:10:07 | **BLOCKED** | 精确包体后续运行均使用 /tmp profile；但前期系统自动化按同 Bundle ID 误启动默认 profile，隔离基线已被污染，本用例证据作废。 | `/tmp/matou-independent-qa-20260830/evidence/INCIDENT-001.txt` |
| E2E-REL-001 | 05:10:08 | **PASS** | 根层横向新增不弹框、追加队尾、自动焦点；DAG 中两个根节点之间无会话父子语义。 | `/tmp/matou-independent-qa-20260830/groups/SHELL-CORE-r2/evidence/results.json` |
| E2E-REL-002 | 05:10:09 | **PASS** | 在 Claude 父节点的子列表用普通 + 创建真实 Shell，2→3、队尾自动焦点并输出 NONROOT_SHELL_REAL；DAG 保持共同父节点。 | `/tmp/matou-independent-qa-20260830/groups/CLAUDE-FINAL-r3/evidence/NONROOT-results.json` |
| E2E-REL-003 | 05:10:10 | **FAIL** | 真实 Claude Fork 子节点均进入 provider session not found 并退化为 Shell，列表无法形成可工作的 Claude/Shell 混合兄弟。 | `/tmp/matou-independent-qa-20260830/groups/CLAUDE-FINAL-r3/evidence/BADGE-results.json` |
| E2E-REL-004 | 05:10:11 | **FAIL** | Shell 进入真实 Claude 后标题会变 Claude；但首轮前 Fork 入口完全隐藏，未按期望可见且置灰；首轮后才出现。 | `/tmp/matou-independent-qa-20260830/groups/CLAUDE-FINAL-r3/evidence/MODE-results.json` |
| E2E-REL-005 | 05:10:12 | **FAIL** | 有效首轮前不存在可悬浮的禁用 Fork 图标与启用条件提示；完成首轮后入口直接出现。 | `/tmp/matou-independent-qa-20260830/groups/CLAUDE-FINAL-r3/evidence/MODE-results.json` |
| E2E-REL-006 | 05:10:13 | **FAIL** | 真实父会话创建当前工作树 Fork 后出现子卡片，但子 Claude 启动/恢复报 provider session not found，无法验证上下文继承。 | `/tmp/matou-independent-qa-20260830/groups/CLAUDE-FINAL-r3/evidence/FORK-results.json` |
| E2E-REL-007 | 05:10:14 | **FAIL** | 当前工作树 Fork 能建立关系卡片，但子会话未就绪并转恢复失败，无法在子会话执行 pwd/cat 与双向文件验证。 | `/tmp/matou-independent-qa-20260830/groups/CLAUDE-FINAL-r3/evidence/fork-created.png` |
| E2E-REL-008 | 05:10:15 | **BLOCKED** | 当前允许的默认工作空间为非 Git；原生工作空间选择器的隔离路径选择未成功建立，无法进入真实新 worktree Fork。 | `/tmp/matou-independent-qa-20260830/groups/WORKSPACE-r1/evidence/results.json` |
| E2E-REL-009 | 05:10:16 | **FAIL** | 非 Git 工作空间中“新工作树”正确置灰并提示需要 Git；选择当前工作树后子会话 provider session not found，Fork 未完成。 | `/tmp/matou-independent-qa-20260830/groups/CLAUDE-FINAL-r3/evidence/fork-dialog.png` |
| E2E-REL-010 | 05:10:17 | **BLOCKED** | 未建立受 App 管理的隔离 Git 工作空间，无法通过 UI 构造同名分支冲突。 | `/tmp/matou-independent-qa-20260830/groups/WORKSPACE-r1/evidence/results.json` |
| E2E-REL-011 | 05:10:18 | **BLOCKED** | 未建立受 App 管理且可安全切只读的隔离 Git 工作空间，真实 worktree 失败前置缺失。 | `/tmp/matou-independent-qa-20260830/groups/WORKSPACE-r1/evidence/results.json` |
| E2E-REL-012 | 05:10:19 | **BLOCKED** | Fork 子节点在 provider session not found 前没有足够的可观察准备窗口，父身份创建期失效步骤未能执行。 | `/tmp/matou-independent-qa-20260830/groups/CLAUDE-FINAL-r3/evidence/FORK-results.json` |
| E2E-REL-013 | 05:10:20 | **FAIL** | “创建同级 Claude 分支”存在并建立兄弟卡片，但新兄弟同样 provider session not found，无法完成共同父上下文隔离。 | `/tmp/matou-independent-qa-20260830/groups/CLAUDE-FINAL-r3/evidence/BADGE-results.json` |
| E2E-REL-014 | 05:10:21 | **BLOCKED** | 首层真实 Fork 子节点即 provider session not found，后续 G1/H1 无法由 UI 创建。 | `/tmp/matou-independent-qa-20260830/groups/CLAUDE-FINAL-r3/evidence/BADGE-results.json` |
| E2E-REL-015 | 05:10:22 | **PASS** | 有 6 个子节点的 Claude 执行真实 /exit 后同一节点变 Shell，子数保留、Fork 隐藏、普通新增可用。 | `/tmp/matou-independent-qa-20260830/groups/CLAUDE-FINAL-r3/evidence/MODE-results.json` |
| E2E-REL-016 | 05:10:23 | **PASS** | Shell 父节点仍可用徽章进入 6 个原子节点并返回；返回后父节点保持 Shell。 | `/tmp/matou-independent-qa-20260830/groups/CLAUDE-FINAL-r3/evidence/MODE-results.json` |
| E2E-REL-017 | 05:10:24 | **PASS** | 同一 Shell 重新启动真实 Claude，首轮 REENTER_OK 完成后 Fork 再出现，6 个已有子节点保持。 | `/tmp/matou-independent-qa-20260830/groups/CLAUDE-FINAL-r3/evidence/MODE-results.json` |
| E2E-REL-018 | 05:10:25 | **FAIL** | 创建共享当前工作树 Fork 后父/子标题与卡片未显示共享工作树标记，且子会话恢复失败。 | `/tmp/matou-independent-qa-20260830/groups/CLAUDE-FINAL-r3/evidence/fork-created.png` |
| E2E-REL-019 | 05:10:26 | **BLOCKED** | 本次专用真实 Claude 身份未提供可用团队/队友会话，前置外部能力缺失。 | `/tmp/matou-independent-qa-20260830/groups/CLAUDE-FINAL-r3/evidence/results.json` |
| E2E-NAV-001 | 05:10:27 | **FAIL** | 父徽章数量为 6，但失败/空闲子节点全部被统计为 Shell 运行中，形态与聚合状态错误。 | `/tmp/matou-independent-qa-20260830/groups/CLAUDE-FINAL-r3/evidence/BADGE-results.json` |
| E2E-NAV-002 | 05:10:28 | **PASS** | 点击“查看 3 个子会话”后父单卡被 3 个直接子节点列表替换；列表不混入根节点。 | `/tmp/matou-independent-qa-20260830/groups/CLAUDE-FINAL-r3/evidence/BADGE-results.json` |
| E2E-NAV-003 | 05:10:29 | **PASS** | 6 个真实 Shell 布局 data-visible-columns=4，横向滚轮使 scrollLeft 45→476，可访问第五/第六节点。 | `/tmp/matou-independent-qa-20260830/groups/SHELL-CORE-r2/evidence/results.json` |
| E2E-NAV-004 | 05:10:30 | **FAIL** | 悬浮前/后卡片宽度均为 226px，无平滑扩展或相邻收缩。 | `/tmp/matou-independent-qa-20260830/groups/SHELL-CORE-r2/evidence/results.json` |
| E2E-NAV-005 | 05:10:31 | **FAIL** | 从中部一次大幅向右滚动时出现直接回父的情况，未稳定停留在最左子列表。 | `/tmp/matou-independent-qa-20260830/groups/CLAUDE-FINAL-r3/evidence/NAV-pull-results.json` |
| E2E-NAV-006 | 05:10:32 | **FAIL** | 左边界手势阈值/手势分段不稳定，第一次到边界可直接返回父视图，未观察到未达阈值的弹性回弹。 | `/tmp/matou-independent-qa-20260830/groups/CLAUDE-FINAL-r3/evidence/NAV-pull-results.json` |
| E2E-NAV-007 | 05:10:33 | **FAIL** | 重新进入列表后大幅第二次右拉仍停留子列表，未稳定触发超过阈值返回。 | `/tmp/matou-independent-qa-20260830/groups/CLAUDE-FINAL-r3/evidence/NAV-pull-results.json` |
| E2E-NAV-008 | 05:10:34 | **BLOCKED** | 显式“返回父会话”鼠标路径已通过；在真实终端占用焦点下未完成 Tab/方向键到达返回入口的完整证据。 | `/tmp/matou-independent-qa-20260830/groups/CLAUDE-FINAL-r3/evidence/MODE-results.json` |
| E2E-NAV-009 | 05:10:35 | **FAIL** | 700px 窗宽时四卡全部等宽压到 101px，当前会话没有优先保留，终端提示符被逐字换行；恢复宽度后回 226px。 | `/tmp/matou-independent-qa-20260830/groups/NAV9-r1/evidence/results.json` |
| E2E-NAV-010 | 05:10:36 | **FAIL** | 边缘手势分段本身不稳定；输出期间无法满足“只有清晰第二手势超过阈值才返回”。 | `/tmp/matou-independent-qa-20260830/groups/CLAUDE-FINAL-r3/evidence/NAV-pull-results.json` |
| E2E-DAG-001 | 05:10:37 | **PASS** | 真实长按 Option+Tab 约 650ms 打开独立 DAG BrowserWindow，松键后保留。 | `/tmp/matou-independent-qa-20260830/groups/DAG-CORE-r1/evidence/results.json` |
| E2E-DAG-002 | 05:10:38 | **PASS** | Shell 短 Tab 完成真实文件名且窗口数保持 1；真实 Claude 内连续短 Tab 未打开 DAG。 | `/tmp/matou-independent-qa-20260830/groups/STATUS-FOCUS-r2/evidence/results.json` |
| E2E-DAG-003 | 05:10:39 | **BLOCKED** | 首层 Fork 子节点恢复失败，无法形成上下各2+层的真实关系链。 | `/tmp/matou-independent-qa-20260830/groups/CLAUDE-FINAL-r3/evidence/BADGE-results.json` |
| E2E-DAG-004 | 05:10:40 | **PASS** | 独立 DAG 提供缩小/100%/放大/聚焦控件，真实拖拽与按钮缩放持续响应并可复位。 | `/tmp/matou-independent-qa-20260830/groups/DAG-CORE-r1/evidence/results.json` |
| E2E-DAG-005 | 05:10:41 | **FAIL** | 100% 节点显示名称、类型、路径、状态、最近四行和活动时间，但真实 Git 工作目录未显示分支信息。 | `/tmp/matou-independent-qa-20260830/groups/DAG-INFO-r1/evidence/results.json` |
| E2E-DAG-006 | 05:10:42 | **PASS** | 输出 LIVE_DAG_1..7 时摘要实时滚动保持最近四行，节点边界始终 260×154、坐标 350,262。 | `/tmp/matou-independent-qa-20260830/groups/DAG-LIVE-r2/evidence/results.json` |
| E2E-DAG-007 | 05:10:43 | **FAIL** | 点击视野外兄弟节点后目标卡 aria-current 为空且 data-in-viewport=false，未滚入视野/选中。 | `/tmp/matou-independent-qa-20260830/groups/DAG-CORE-r1/evidence/results.json` |
| E2E-DAG-008 | 05:10:44 | **FAIL** | DAG 跳转目标未保持在视野；悬浮/布局变化后仍存在不可见或未选中的目标。 | `/tmp/matou-independent-qa-20260830/groups/DAG-CORE-r1/evidence/results.json` |
| E2E-DAG-009 | 05:10:45 | **PASS** | 子列表打开 DAG 点击 Claude 父节点后回根单卡，父徽章恢复且 textarea 获得焦点。 | `/tmp/matou-independent-qa-20260830/groups/CLAUDE-FINAL-r3/evidence/DAG-parent-results.json` |
| E2E-DAG-010 | 05:10:46 | **FAIL** | 100 个真实 UI 节点均出现，但 DAG 首屏节点可操作耗时约 5130ms；不符合可感知即时反馈。 | `/tmp/matou-independent-qa-20260830/groups/DAG100-r2/evidence/results.json` |
| E2E-STA-001 | 05:10:47 | **FAIL** | 命令真实运行并完成，但 DAG 在提示符前、运行期、完成后始终显示“运行中”。 | `/tmp/matou-independent-qa-20260830/groups/STATUS-FOCUS-r2/evidence/results.json` |
| E2E-STA-002 | 05:10:48 | **FAIL** | 真实 stderr 与 exit 23 可见且可继续 echo recovered，但红色错误状态未出现，DAG仍显示运行中。 | `/tmp/matou-independent-qa-20260830/groups/STATUS-FOCUS-r2/evidence/results.json` |
| E2E-STA-003 | 05:10:49 | **BLOCKED** | 严格执行 CMD-WAIT 后 zsh 返回“read: -p: no coprocess”，程序没有进入等待输入，验收前提由夹具命令破坏。 | `/tmp/matou-independent-qa-20260830/groups/STATUS-FOCUS-r2/evidence/results.json` |
| E2E-STA-004 | 05:10:50 | **PASS** | 无标准提示的真实 stdin 等待程序保持运行中而未猜测为待输入，输入后可继续。 | `/tmp/matou-independent-qa-20260830/groups/SHELL-CORE-r2/evidence/results.json` |
| E2E-STA-005 | 05:10:51 | **FAIL** | 真实 Claude 可完成回答，但 DAG/聚合状态没有可靠空闲回落，节点常驻运行中。 | `/tmp/matou-independent-qa-20260830/groups/CLAUDE-FINAL-r3/evidence/DAG-parent-results.json` |
| E2E-STA-006 | 05:10:52 | **BLOCKED** | 真实 Claude身份可用，但未形成可控且可重复的权限请求；同时现有状态回落缺陷影响前置。 | `/tmp/matou-independent-qa-20260830/groups/CLAUDE-FINAL-r3/evidence/results.json` |
| E2E-STA-007 | 05:10:53 | **BLOCKED** | 未在不影响测试机其他流量的条件下提供可恢复网络/账户失败开关，外部失败前置缺失。 | `/tmp/matou-independent-qa-20260830/groups/CLAUDE-FINAL-r3/evidence/results.json` |
| E2E-STA-008 | 05:10:54 | **FAIL** | 恢复失败、空闲和已完成子节点在徽章中被统一计为“运行中”，状态优先级不成立。 | `/tmp/matou-independent-qa-20260830/groups/CLAUDE-FINAL-r3/evidence/BADGE-results.json` |
| E2E-STA-009 | 05:10:55 | **FAIL** | 恢复失败节点仍占活动横向列表并计入运行中，没有作为历史节点从日常列表收起。 | `/tmp/matou-independent-qa-20260830/groups/CLAUDE-FINAL-r3/evidence/BADGE-results.json` |
| E2E-STA-010 | 05:10:56 | **FAIL** | 单 Shell 错误没有串流，其他 Shell/真实 Claude仍可用；但父/画布聚合未显示异常且不能定位错误节点。 | `/tmp/matou-independent-qa-20260830/groups/SHELL-CORE-r2/evidence/results.json` |
| E2E-STA-011 | 05:10:57 | **BLOCKED** | 没有可从 App UI/真实外部边界单独中断并恢复摘要链路而不杀死整个测试 App 的夹具。 | `/tmp/matou-independent-qa-20260830/groups/DAG-LIVE-r2/evidence/results.json` |
| E2E-STA-012 | 05:10:58 | **FAIL** | 提交后约 91ms 出现运行中，但命令完成后超过 1s 仍未回空闲，DAG/徽章一致性失败。 | `/tmp/matou-independent-qa-20260830/groups/DAG-CORE-r1/evidence/results.json` |
| E2E-SORT-001 | 05:10:59 | **PASS** | 草稿不换序；Shell 回车提交后目标移首且焦点保持。 | `/tmp/matou-independent-qa-20260830/groups/SHELL-CORE-r2/evidence/results.json` |
| E2E-SORT-002 | 05:11:00 | **PASS** | Ctrl+C 与完成 stdin 输入均将真实操作会话移首，关系未变。 | `/tmp/matou-independent-qa-20260830/groups/SHELL-CORE-r2/evidence/results.json` |
| E2E-SORT-003 | 05:11:01 | **PASS** | 真实 Claude 草稿不换序；发送后目标从末位移首，回答完成顺序不再变化，焦点保持。 | `/tmp/matou-independent-qa-20260830/groups/CLAUDE-REAL-r2/evidence/SORT003-results.json` |
| E2E-SORT-004 | 05:11:02 | **BLOCKED** | 真实 Claude授权/拒绝/选项场景未形成可重复测试前置，未取得四种控制操作完整序列。 | `/tmp/matou-independent-qa-20260830/groups/CLAUDE-FINAL-r3/evidence/results.json` |
| E2E-SORT-005 | 05:11:03 | **PASS** | 点击/聚焦/选择/滚动及 DAG 查看不更新排序。 | `/tmp/matou-independent-qa-20260830/groups/SHELL-CORE-r2/evidence/results.json` |
| E2E-SORT-006 | 05:11:04 | **FAIL** | 后台 Shell 输出本身不换序，但 Claude 语义状态/通知缺失，无法满足内容与徽章同步要求。 | `/tmp/matou-independent-qa-20260830/groups/SHELL-CORE-r2/evidence/results.json` |
| E2E-SORT-007 | 05:11:05 | **FAIL** | 新 Shell 追加队尾；Fork 节点建立后立即恢复失败，历史恢复队尾流程不可完成。 | `/tmp/matou-independent-qa-20260830/groups/CLAUDE-FINAL-r3/evidence/fork-created.png` |
| E2E-SORT-008 | 05:11:06 | **PASS** | B/C 同一时钟粒度真实提交后顺序稳定；重启保持，后续交互目标明确移首。 | `/tmp/matou-independent-qa-20260830/groups/SORT8-r1/evidence/results.json` |
| E2E-SORT-009 | 05:11:07 | **BLOCKED** | 首层 Fork恢复失败，BASE-TREE关系链不完整，无法执行完整关系坐标与排序回归。 | `/tmp/matou-independent-qa-20260830/groups/CLAUDE-FINAL-r3/evidence/BADGE-results.json` |
| E2E-LIFE-001 | 05:11:08 | **FAIL** | 运行中的带 6 子节点父会话点击删除无确认（dialog=0），父节点直接消失且子徽章不再可见。 | `/tmp/matou-independent-qa-20260830/groups/CLAUDE-FINAL-r3/evidence/LIFE001.json` |
| E2E-LIFE-002 | 05:11:09 | **FAIL** | 活动叶子点击 × 后直接从活动区消失；DAG没有历史节点确认/移除流程。 | `/tmp/matou-independent-qa-20260830/groups/LIFE-CORE-r1/evidence/E2E-LIFE-002.png` |
| E2E-LIFE-003 | 05:11:10 | **FAIL** | 删除父节点后未保留可操作历史父节点，关系入口随父消失。 | `/tmp/matou-independent-qa-20260830/groups/CLAUDE-FINAL-r3/evidence/LIFE001.png` |
| E2E-LIFE-004 | 05:11:11 | **FAIL** | 父节点删除没有“移除整条分支”专用确认与后代数量，直接删除主节点。 | `/tmp/matou-independent-qa-20260830/groups/CLAUDE-FINAL-r3/evidence/LIFE001.json` |
| E2E-LIFE-005 | 05:11:12 | **FAIL** | 运行中非最后画布点击关闭后 dialog=0，Tab直接减少，用户没有取消机会。 | `/tmp/matou-independent-qa-20260830/groups/CLOSE-r3/evidence/after-close-click.png` |
| E2E-LIFE-006 | 05:11:13 | **FAIL** | 空闲画布关闭后事项菜单仅置顶/重命名/删除，不存在“已关闭画布”与恢复入口。 | `/tmp/matou-independent-qa-20260830/groups/HISTORY-r1/evidence/results.json` |
| E2E-LIFE-007 | 05:11:14 | **FAIL** | 窗口关闭后真实命令继续（counter 2→4），重新唤起成功但终端焦点未恢复。 | `/tmp/matou-independent-qa-20260830/groups/HIDE-r2/evidence/results.json` |
| E2E-LIFE-008 | 05:11:15 | **FAIL** | 同 profile 重启可恢复基本 Tab/历史，但每画布横向位置与 DAG 缩放均复位到默认。 | `/tmp/matou-independent-qa-20260830/groups/CAN003-r2/evidence/results.json` |
| E2E-LIFE-009 | 05:11:16 | **PASS** | 完整退出时 counter 保持 2→2；重启后仍为2且出现历史/中断提示，未自动重复执行。 | `/tmp/matou-independent-qa-20260830/groups/QUIT-r1/evidence/results.json` |
| E2E-LIFE-010 | 05:11:17 | **FAIL** | 真实 Fork 子会话重启后呈 Shell+“Claude Code 恢复失败/provider session not found”并保留重试；点击重试仍未恢复真实子会话。 | `/tmp/matou-independent-qa-20260830/groups/CLAUDE-FINAL-r3/evidence/BADGE-results.json` |
| E2E-ISO-001 | 05:11:18 | **BLOCKED** | 通过真实“新增工作空间”与 macOS选择器尝试选择隔离目录，自动化操作返回成功但工作空间未添加；目录改名旅程前置未建立。 | `/tmp/matou-independent-qa-20260830/groups/WORKSPACE-r1/evidence/results.json` |
| E2E-ISO-002 | 05:11:19 | **BLOCKED** | 新 worktree Fork入口因默认非 Git 且隔离 Git工作空间未建立，无法观察创建中异常退出。 | `/tmp/matou-independent-qa-20260830/groups/WORKSPACE-r1/evidence/results.json` |
| E2E-ISO-003 | 05:11:20 | **BLOCKED** | 已观察多个 Fork子节点均恢复失败，不满足“仅一个目标节点恢复条件失效”的隔离前置。 | `/tmp/matou-independent-qa-20260830/groups/CLAUDE-FINAL-r3/evidence/BADGE-results.json` |
| E2E-ISO-004 | 05:11:21 | **FAIL** | 独立窗口可真实脱出，DAG点击会唤起并聚焦；关闭独立窗口后 DAG仍把节点显示为运行中，没有历史/继续入口。 | `/tmp/matou-independent-qa-20260830/groups/INDEPENDENT-r2/evidence/results.json` |
| E2E-ISO-005 | 05:11:22 | **BLOCKED** | 测试机仅检测到 1 台 1920×1080 主显示器，双显示器前置不存在。 | `/tmp/matou-independent-qa-20260830/environment/display.txt` |
| E2E-ISO-006 | 05:11:23 | **FAIL** | 主题真实切换可用，但状态节点持续误报运行中、普通/Fork边语义不完整，无法完成关系可读性验收。 | `/tmp/matou-independent-qa-20260830/groups/THEME-r3/evidence/results.json` |
| E2E-ISO-007 | 05:11:24 | **BLOCKED** | 未发现可从真实系统外部稳定制造“仅 DAG BrowserWindow创建失败”且主窗口保持的环境条件。 | `/tmp/matou-independent-qa-20260830/groups/DAG-CORE-r1/evidence/results.json` |
| E2E-ISO-008 | 05:11:25 | **FAIL** | 10个真实终端并发输出未串流且UI响应；命令结束后节点仍显示运行中，未满足全量回空闲。 | `/tmp/matou-independent-qa-20260830/groups/SHELL-CORE-r2/evidence/results.json` |
| E2E-EDGE-001 | 05:11:26 | **BLOCKED** | 当前工作空间为有效 /Users/icesword，隔离失效 cwd 未能通过真实工作空间选择器建立。 | `/tmp/matou-independent-qa-20260830/groups/WORKSPACE-r1/evidence/results.json` |
| E2E-EDGE-002 | 05:11:27 | **BLOCKED** | 同上，未取得只影响新 PTY创建而不影响现有 PTY的真实资源/目录失效夹具。 | `/tmp/matou-independent-qa-20260830/groups/WORKSPACE-r1/evidence/results.json` |
| E2E-EDGE-003 | 05:11:28 | **PASS** | 单节点 DAG居中且缩放/平移/聚焦控件可用；点击当前节点返回终端。 | `/tmp/matou-independent-qa-20260830/groups/DAG-CORE-r1/evidence/results.json` |
| E2E-EDGE-004 | 05:11:29 | **BLOCKED** | 首层 Fork恢复失败，无法通过真实 UI建立7层节点链。 | `/tmp/matou-independent-qa-20260830/groups/CLAUDE-FINAL-r3/evidence/BADGE-results.json` |
| E2E-EDGE-005 | 05:11:30 | **BLOCKED** | 新 Git worktree创建前置未建立，无法生成原/新工作树两组未提交修改。 | `/tmp/matou-independent-qa-20260830/groups/WORKSPACE-r1/evidence/results.json` |
| E2E-EDGE-006 | 05:11:31 | **FAIL** | Claude主动 /exit 立即转 Shell，但完整重启后同节点错误尝试恢复并显示“Claude Code恢复失败/provider resume exited with code 0”。 | `/tmp/matou-independent-qa-20260830/groups/CLAUDE-FINAL-r3/evidence/EDGE006-final.json` |
| E2E-EDGE-007 | 05:11:32 | **FAIL** | 关闭运行画布没有确认框，因此无“取消关闭并保持草稿/焦点/进程”的路径。 | `/tmp/matou-independent-qa-20260830/groups/CLOSE-r3/evidence/after-close-click.png` |
| E2E-EDGE-008 | 05:11:33 | **FAIL** | 进入200+字符真实目录后 Tab仍显示 /Users/icesword；窄窗仅终端逐字换行，无完整路径悬浮/信息优先让位。 | `/tmp/matou-independent-qa-20260830/groups/LONGPATH-r2/evidence/results.json` |
| E2E-EDGE-009 | 05:11:34 | **BLOCKED** | 新画布/兄弟/显式返回焦点已分别验证；Fork子节点 provider session not found、DAG视野外跳转失败阻断完整五段旅程。 | `/tmp/matou-independent-qa-20260830/groups/CLAUDE-FINAL-r3/evidence/FORK-results.json` |
| E2E-EDGE-010 | 05:11:35 | **FAIL** | 运行中父节点删除后未以历史节点保留，子状态更新与回历史父详情路径消失。 | `/tmp/matou-independent-qa-20260830/groups/CLAUDE-FINAL-r3/evidence/LIFE001.json` |
| E2E-EDGE-011 | 05:11:36 | **PASS** | 6个真实会话分别输出空行、ANSI、超长行、中文、emoji、alternate-screen；DAG无实际ESC泄漏、Unicode正确、卡宽固定260px、节点数未增。 | `/tmp/matou-independent-qa-20260830/groups/EDGE11-r2/evidence/results.json` |
| E2E-EDGE-012 | 05:11:37 | **FAIL** | 普通 Shell 子边存在；Claude Fork 子节点恢复失败后显示为 Shell，DAG无法持续区分 Fork继承边与普通边。 | `/tmp/matou-independent-qa-20260830/groups/CLAUDE-FINAL-r3/evidence/NONROOT-results.json` |
| E2E-EDGE-013 | 05:11:38 | **FAIL** | 事项菜单没有已关闭画布入口，首次恢复即不存在，多次恢复/重新关闭无法进行。 | `/tmp/matou-independent-qa-20260830/groups/HISTORY-r1/evidence/results.json` |
| E2E-EDGE-014 | 05:11:39 | **PASS** | 真实运行命令与未提交草稿跨 Cmd+I 白→深→白切换均保留，焦点和会话顺序不变。 | `/tmp/matou-independent-qa-20260830/groups/THEME-STATE-r2/evidence/results.json` |
| E2E-EDGE-015 | 05:11:40 | **BLOCKED** | 已验证一个父下多子关系；未形成多个有效可作为父的Claude节点，无法执行P1/P2唯一父全序列。 | `/tmp/matou-independent-qa-20260830/groups/CLAUDE-FINAL-r3/evidence/NONROOT-results.json` |
| E2E-EDGE-016 | 05:11:41 | **BLOCKED** | 只提供当前单一打包版本，没有“后续覆盖安装测试版”工件。 | `/tmp/matou-independent-qa-20260830/environment/build.txt` |

## 4. 失败清单（按用户影响聚合）

### D-01 Fork 子会话不可用（P0）
- **影响用例**：REL-003/006/007/009/013/018、SORT-007、LIFE-010、EDGE-012。
- **最短复现**：进入真实 Claude → 完成一轮 → 点击 Fork → 使用当前工作树创建。
- **期望**：子 Claude 就绪、继承父上下文并可继续工作。
- **实际**：关系卡片出现后子节点转为 Shell，并显示 `Claude Code 恢复失败 / provider session not found`；重试仍失败。
- **证据**：`/tmp/matou-independent-qa-20260830/groups/CLAUDE-FINAL-r3/evidence/fork-created.png`、`BADGE-results.json`。

### D-02 状态与聚合长期误报运行中（P0）
- **影响用例**：NAV-001、STA-001/002/005/008/009/010/012、ISO-006/008。
- **最短复现**：Shell 执行 `sleep 2; echo done` → 等待提示符 → 打开 DAG。
- **期望**：运行中后回空闲；exit 23 显示错误；恢复失败不计运行中。
- **实际**：提示符已恢复仍显示运行中；exit 23 无红色错误；恢复失败节点计为 Shell 运行中。
- **证据**：`/tmp/matou-independent-qa-20260830/groups/STATUS-FOCUS-r2/evidence/results.json`、`/tmp/matou-independent-qa-20260830/groups/CLAUDE-FINAL-r3/evidence/BADGE-results.json`。

### D-03 子列表二次右拉手势不稳定（P0）
- **影响用例**：NAV-005/006/007/010。
- **最短复现**：6 个子会话 → 中部一次向右大滑 → 在最左再次右拉。
- **期望**：第一手势只到最左；第二手势未过阈值回弹、过阈值返回。
- **实际**：出现第一手势直接回父；重新进入后大幅第二手势又不返回，阈值与手势边界不稳定。
- **证据**：`/tmp/matou-independent-qa-20260830/groups/CLAUDE-FINAL-r3/evidence/NAV-pull-results.json`。

### D-04 DAG 跳转与观察状态不可靠（P0）
- **影响用例**：CAN-003、DAG-007/008/010、LIFE-008。
- **最短复现**：6 个同级会话，将目标移出视野 → DAG点击目标；或设置DAG 80%后切换画布。
- **期望**：目标完整进入视野并聚焦；每画布缩放/平移独立恢复。
- **实际**：目标 `aria-current` 为空且不在视野；缩放回到100%；100节点首屏约5.13秒。
- **证据**：`/tmp/matou-independent-qa-20260830/groups/DAG-CORE-r1/evidence/results.json`、`/tmp/matou-independent-qa-20260830/groups/CAN003-r2/evidence/results.json`、`/tmp/matou-independent-qa-20260830/groups/DAG100-r2/evidence/results.json`。

### D-05 关闭/删除缺少保护与历史恢复（P0）
- **影响用例**：LIFE-001/002/003/004/005/006、EDGE-007/010/013。
- **最短复现**：运行命令的画布点关闭，或运行中的带子节点父会话点 ×。
- **期望**：影响确认、允许取消；关闭后进入已关闭画布；父历史及关系保留。
- **实际**：dialog=0，Tab/父节点直接消失；事项菜单无已关闭画布入口。
- **证据**：`/tmp/matou-independent-qa-20260830/groups/CLOSE-r3/evidence/after-close-click.png`、`/tmp/matou-independent-qa-20260830/groups/HISTORY-r1/evidence/results.json`、`/tmp/matou-independent-qa-20260830/groups/CLAUDE-FINAL-r3/evidence/LIFE001.json`。

### D-06 Claude 主动退出在重启后被错误恢复（P0）
- **影响用例**：EDGE-006。
- **最短复现**：有子节点的 Claude 执行 `/exit` → 看到 Shell → 完整退出 App → 重启。
- **期望**：继续是 Shell，不显示恢复失败。
- **实际**：重启显示 `Claude Code 恢复失败 / provider resume exited with code 0`。
- **证据**：`/tmp/matou-independent-qa-20260830/groups/CLAUDE-FINAL-r3/evidence/EDGE006-final.json`。

### D-07 其他可见问题（P1）
- 画布没有可发现的重命名入口（CAN-004）。
- 四列悬浮宽度不变化（NAV-004）。
- 窄窗全部等宽压缩，当前会话没有优先让位（NAV-009）。
- DAG Git节点不显示分支（DAG-005）。
- 200+字符工作目录后 Tab仍显示 `/Users/icesword`（EDGE-008）。
- 独立窗口关闭后 DAG仍显示运行中，无继续/重新打开入口（ISO-004）。

## 5. BLOCKED 与规格问题

- **外部环境阻塞**：团队 Claude 会话（REL-019）、第二显示器（ISO-005）、后续覆盖安装包（EDGE-016）、仅摘要链路断流夹具（STA-011）。
- **上游缺陷阻塞**：多层 Fork/7层链、真实新 worktree、单点恢复失败等用例受 D-01 或隔离 Git工作空间选择未建立影响。
- **测试夹具问题**：`CMD-WAIT = read -r -p ...` 在产品默认 zsh 中报 `read: -p: no coprocess`，E2E-STA-003 记为 BLOCKED；PRD只要求“明确待输入”，未要求该 Bash 语法。
- **PRD冲突**：未发现需要修改最终 PRD 验收口径的直接冲突；上述为执行环境/夹具或产品行为问题。

## 6. 数据文件

- 机器可读 102 条结果：`/tmp/matou-independent-qa-20260830/execution-results-102.json`
- 证据根：`/tmp/matou-independent-qa-20260830/groups/`
- 产品实现改动：无。
