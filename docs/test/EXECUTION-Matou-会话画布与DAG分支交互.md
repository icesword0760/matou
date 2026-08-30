# Matou 会话画布与 DAG 分支交互—独立黑盒执行记录

## 1. 验收结论

- **102 条逐条完成记录：PASS 66 / FAIL 25 / BLOCKED 11。**
- 受测对象：`Matou.app`，分支 `feature/session-dag-canvas`，提交 `811aec29c923c73c1f1e415b4d751717dc2fc480`。
- 真实 Fork、当前/新工作树、上下文继承、Shell/Claude 混排、Shell 状态、主动 `/exit` 后重启等核心阻断已修复。
- 当前仍不满足发布门槛：DAG 跳转/观察持久化、100节点性能、历史移除、恢复重试、独立窗关闭、窗口重开焦点和若干布局交互仍影响完整旅程。

## 2. 环境与隔离

| 项目 | 值 |
|---|---|
| App | `.worktrees/session-dag-canvas/apps/desktop/release/mac-arm64/Matou.app` |
| Commit | `811aec29c923c73c1f1e415b4d751717dc2fc480` |
| macOS | 15.7.4 (24G517), arm64 |
| 显示 | 1920×1080 单显示器 |
| 隔离根 | `/tmp/matou-independent-qa-final-20260830-071752` |
| Claude | 真实 Claude Code v2.1.251；OAuth 只读注入，项目/会话配置均写入隔离 HOME |
| 操作 | Playwright Electron 真实 UI 键鼠、真实 PTY/Git/worktree/Claude；未读取实现代码、未改数据库、未使用 mock |

## 3. 逐条执行结果

| 用例 ID | 时间（UTC+8） | 结果 | 实际观察与用户影响 | 证据 |
|---|---:|---|---|---|
| E2E-CAN-001 | 07:18:35 | **PASS** | 隔离 profile 首次启动即显示默认画布和可直接输入的 Shell；真实 pwd 指向隔离 WS-GIT。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/SMOKE-r1/evidence/body.txt` |
| E2E-CAN-002 | 07:22:40 | **PASS** | 顶部 + 直接产生并选中新画布，无类型/工作树弹框；新 Shell 自动聚焦。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/CANVAS-RENAME-r1/evidence/results.json` |
| E2E-CAN-003 | 07:19:06 | **FAIL** | A/B 节点与焦点隔离、横向位置可恢复；但两张画布 DAG 80%/110% 均重启为 100%，观察位置未按画布保存。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/CAN003-r2/evidence/results.json` |
| E2E-CAN-004 | 07:22:40 | **PASS** | 连续名称为“新画布/新画布 2”；双击重命名为“性能验证”后完整重启仍保留。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/CANVAS-RENAME-r1/evidence/results.json` |
| E2E-CAN-005 | 07:19:06 | **PASS** | 事项/画布切换后会话集合、上次焦点与可输入现场保持；数据未串到其他画布。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/CAN003-r2/evidence/results.json` |
| E2E-CAN-006 | 07:32:33 | **PASS** | 横向 Shell 追加队尾并自动焦点，未二次点击即可输入；旧终端未收到内容。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/SHELL-CORE-r2/evidence/results.json` |
| E2E-CAN-007 | 07:28:57 | **PASS** | 真实 Claude 两轮回复后输入 textarea 持续聚焦，第二条无需鼠标即可提交。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/CLAUDE-FORK-r1/evidence/child-results.json` |
| E2E-CAN-008 | 07:54:34 | **PASS** | 所有 App、Electron、Claude、Git、worktree 与证据路径均位于唯一 /tmp 根；未使用默认 profile 启动。 | `/tmp/matou-independent-qa-final-20260830-071752/environment/isolation.md` |
| E2E-REL-001 | 07:32:33 | **PASS** | 根层新增 Shell 无弹框、追加队尾、自动焦点，DAG 中无会话父子边。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/SHELL-CORE-r2/evidence/results.json` |
| E2E-REL-002 | 07:28:57 | **PASS** | Claude 父的子列表中新增普通 Shell 后与 Claude 子共享同一父，追加并聚焦。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/CLAUDE-FORK-r1/evidence/child-results.json` |
| E2E-REL-003 | 07:28:57 | **PASS** | 同一横向子列表真实混排 Claude 与 Shell；两类节点独立工作，DAG 保留共同父。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/CLAUDE-FORK-r1/evidence/child-results.json` |
| E2E-REL-004 | 07:28:12 | **PASS** | Shell 启动真实 Claude 后节点原地切型；首轮前 Fork 图标可见且禁用，完成后启用。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/CLAUDE-FORK-r1/evidence/results.json` |
| E2E-REL-005 | 07:28:12 | **PASS** | 首轮前 Fork 悬浮文案为“完成首轮对话后可创建分支”且按钮禁用；首轮完成后可打开弹框。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/CLAUDE-FORK-r1/evidence/results.json` |
| E2E-REL-006 | 07:28:57 | **PASS** | 真实 Fork 子 Claude 正常就绪，并准确回答父会话唯一 token；父子后续输入隔离。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/CLAUDE-FORK-r1/evidence/child-results.json` |
| E2E-REL-007 | 08:00:05 | **PASS** | 当前工作树 Fork 的父子路径一致、双方显示共享工作树，子 Shell 可读父目录 uncommitted.txt。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/CLAUDE-FORK-r1/evidence/current-inspect.txt` |
| E2E-REL-008 | 07:30:03 | **PASS** | 新 worktree Fork 产生独立路径和生成分支；子目录无父未提交文件，原 main/uncommitted 保留且子写入不回原目录。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/CLAUDE-WT-r1/evidence/results.json` |
| E2E-REL-009 | 07:42:29 | **PASS** | 非 Git 目录新工作树项真实禁用并提示需要 Git；键盘 Space 不改变选择，当前目录 Fork 成功，普通 Shell 仍可直接新增。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/CLAUDE-NONGIT-r1/evidence/results.json` |
| E2E-REL-010 | 07:54:34 | **BLOCKED** | 规格冲突：用例以已有原始 Git ref 作为冲突，但最终规则按用户显示名去重、Runtime 生成安全 Git ref；实际生成 matou/e2e-duplicate-<id>。 | `/tmp/matou-independent-qa-final-20260830-071752/environment/spec-conflicts.md` |
| E2E-REL-011 | 07:54:34 | **BLOCKED** | 隔离小仓库的真实 worktree 准备过快，未形成可重复的只读失败阶段；未改实现或伪造失败。 | `/tmp/matou-independent-qa-final-20260830-071752/environment/blockers.md` |
| E2E-REL-012 | 07:54:34 | **BLOCKED** | 真实 Fork 准备窗口不足以在创建期失效父 provider 身份；未通过内部接口延长或伪造阶段。 | `/tmp/matou-independent-qa-final-20260830-071752/environment/blockers.md` |
| E2E-REL-013 | 07:44:28 | **PASS** | 从同级入口创建第二 Claude；新兄弟继承共同父 NONGIT_READY，未继承 C1_SECRET，DAG 显示共同父。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/CLAUDE-NONGIT-r1/evidence/sibling-list.png` |
| E2E-REL-014 | 07:46:51 | **PASS** | 真实建立多级 P→C→G→H 链，直接子列表逐层只显示下一层，DAG 连线保留。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/CLAUDE-NONGIT-r1/evidence/multilevel2-results.json` |
| E2E-REL-015 | 07:28:12 | **PASS** | 真实 /exit 后同一节点转为 Shell，不显示“已退出”，子数量/导航/关系保留，Fork 隐藏。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/CLAUDE-FORK-r1/evidence/results.json` |
| E2E-REL-016 | 07:28:12 | **PASS** | 主动退出后的 Shell 仍可进入原子列表；完整重启后父继续是 Shell，没有自动恢复。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/CLAUDE-FORK-r1/evidence/results.json` |
| E2E-REL-017 | 07:46:51 | **PASS** | 同一稳定节点可再次进入 Claude；完成新一轮后 Fork 能力恢复，既有关系未重写。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/CLAUDE-NONGIT-r1/evidence/multilevel2-results.json` |
| E2E-REL-018 | 07:52:23 | **PASS** | 单会话无共享标记；第二会话加入后双方出现共享工作树；移除后剩余节点标记消退，目录保留。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/TESTNEW-r1/evidence/shared-marker.json` |
| E2E-REL-019 | 07:54:34 | **BLOCKED** | 真实账号没有可用 Team teammate session，无法建立队友会话前置。 | `/tmp/matou-independent-qa-final-20260830-071752/environment/blockers.md` |
| E2E-NAV-001 | 07:28:57 | **PASS** | 父徽章显示直接子总数、Claude/Shell 构成和运行/空闲/中断计数；错误优先级由状态引擎实时更新。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/CLAUDE-FORK-r1/evidence/child-results.json` |
| E2E-NAV-002 | 07:28:57 | **PASS** | 点击徽章以横向直接子列表替换父卡；孙节点不混入，焦点落在选中子节点。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/CLAUDE-FORK-r1/evidence/child-results.json` |
| E2E-NAV-003 | 07:32:48 | **PASS** | 同级 6 节点每屏不超过四卡；横向 wheel 可在 scrollLeft 952 与 0 间访问首尾。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/CLAUDE-FORK-r1/evidence/NAV-pull-results.json` |
| E2E-NAV-004 | 07:32:33 | **FAIL** | 悬浮第三卡前/中/后宽度均 305.3px，相邻卡没有收缩，点击后也无保持扩展。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/SHELL-CORE-r2/evidence/results.json` |
| E2E-NAV-005 | 07:33:12 | **PASS** | 中部一次大幅右滑只到 scrollLeft=0，仍停在子列表且未出现父视图。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/CLAUDE-FORK-r1/evidence/NAV-boundary-results.json` |
| E2E-NAV-006 | 07:33:12 | **PASS** | 已在最左后的小幅独立右滑仍留在子列表并保持原焦点，未越级。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/CLAUDE-FORK-r1/evidence/NAV-boundary-results.json` |
| E2E-NAV-007 | 07:33:12 | **PASS** | 边界第二次大幅右滑返回父视图，父 textarea 自动恢复焦点。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/CLAUDE-FORK-r1/evidence/NAV-boundary-results.json` |
| E2E-NAV-008 | 07:53:50 | **FAIL** | 鼠标明确返回可用；但终端聚焦后连续 120 次 Tab 均由终端消费，键盘到达不了“返回父会话”。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/CLAUDE-NONGIT-r1/evidence/shortcut-keyboard.json` |
| E2E-NAV-009 | 07:19:20 | **FAIL** | 700px 窄窗仍把所有卡保持 440px 完整终端并横向溢出；没有“当前完整、其他摘要”的优先让位。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/NAV9-r1/evidence/results.json` |
| E2E-NAV-010 | 07:33:12 | **PASS** | 最左边界在静态/持续输出场景不自动切层；小手势不返回，清晰第二大手势才返回父会话。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/CLAUDE-FORK-r1/evidence/NAV-boundary-results.json` |
| E2E-DAG-001 | 07:53:50 | **PASS** | 真实按住 Option+Tab 650ms 打开第二个独立 DAG BrowserWindow，松键后仍保留。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/CLAUDE-NONGIT-r1/evidence/shortcut-keyboard.json` |
| E2E-DAG-002 | 07:32:33 | **PASS** | Shell 短 Tab 完成 baseline.txt 且窗口数不变；未出现 DAG 闪现。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/SHELL-CORE-r2/evidence/results.json` |
| E2E-DAG-003 | 07:46:51 | **FAIL** | 四层真实关系打开 DAG 时直接铺开跨层节点，未见只保留父/当前/子三层及超出层虚影。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/CLAUDE-NONGIT-r1/evidence/multilevel2-dag.png` |
| E2E-DAG-004 | 07:32:33 | **PASS** | 独立画布缩放、100%复位、聚焦与平移均响应；节点布局保持。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/SHELL-CORE-r2/evidence/results.json` |
| E2E-DAG-005 | 07:19:18 | **PASS** | 100% 节点含名称/类型/分支/状态/最近四行/活动时间；缩放时保持核心信息和固定卡宽。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/DAG-LIVE-r2/evidence/results.json` |
| E2E-DAG-006 | 07:19:18 | **PASS** | LIVE_DAG_1..7 实时轮换为最近四行，节点位置与260×154边界不移动。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/DAG-LIVE-r2/evidence/results.json` |
| E2E-DAG-007 | 07:21:51 | **FAIL** | DAG 点击视野外末节点后主列表目标未获得焦点，data-in-viewport 仍为 false。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/DAG100-r2/evidence/results.json` |
| E2E-DAG-008 | 07:21:51 | **FAIL** | 目标点击后未滚入视野，后续布局/悬浮无法保持其可见；100节点同样复现。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/DAG100-r2/evidence/results.json` |
| E2E-DAG-009 | 07:34:08 | **PASS** | 从子列表点击活动父节点可回到父层级，子徽章和终端焦点恢复。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/CLAUDE-FORK-r1/evidence/PARENT-LIFE-results.json` |
| E2E-DAG-010 | 07:21:51 | **FAIL** | 100 个真实 UI 节点均创建，但 DAG 首个节点可操作耗时 5128ms；点远端节点后仍不在视野且无焦点。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/DAG100-r2/evidence/results.json` |
| E2E-STA-001 | 07:32:33 | **PASS** | Shell 从空闲→运行中→空闲真实切换，结束后提示符与 DAG 一致。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/SHELL-CORE-r2/evidence/results.json` |
| E2E-STA-002 | 07:32:33 | **PASS** | exit 23 在 DAG 显示异常；同一 Shell 随后可输出 recovered。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/SHELL-CORE-r2/evidence/results.json` |
| E2E-STA-003 | 07:54:34 | **BLOCKED** | 用例硬编码 Bash read -p，在默认 zsh 返回 no coprocess，未进入等待；与 PRD 的抽象待输入要求冲突。 | `/tmp/matou-independent-qa-final-20260830-071752/environment/spec-conflicts.md` |
| E2E-STA-004 | 07:32:33 | **PASS** | 无标准提示的 python input 只显示运行中而不误判待输入，提交后继续。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/SHELL-CORE-r2/evidence/results.json` |
| E2E-STA-005 | 07:47:54 | **PASS** | 真实 Claude 提交后 339ms 显示运行中，回复完成后回空闲，输入焦点保留。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/CLAUDE-NONGIT-r1/evidence/claude-status.json` |
| E2E-STA-006 | 07:48:55 | **FAIL** | 真实 Write 权限确认已出现，但 DAG 将明确的 Yes/No 等待显示为“运行中”，未显示“待输入”。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/CLAUDE-NONGIT-r1/evidence/permission2-results.json` |
| E2E-STA-007 | 07:54:34 | **BLOCKED** | 无仅影响隔离 Claude 会话且可逆的真实网络/账户故障边界；未替换 provider。 | `/tmp/matou-independent-qa-final-20260830-071752/environment/blockers.md` |
| E2E-STA-008 | 07:36:51 | **PASS** | 错误/中断/运行/空闲均能进入徽章和 DAG，错误优先；历史节点不覆盖活动子状态。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/CLAUDE-WT-r1/evidence/restore-results.json` |
| E2E-STA-009 | 07:34:08 | **PASS** | 结束父节点从活动列表收起，在 DAG 标为历史；活跃子数量/状态继续独立统计。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/CLAUDE-FORK-r1/evidence/PARENT-LIFE-results.json` |
| E2E-STA-010 | 07:32:33 | **PASS** | 一个 Shell 异常未串流，其他 Shell/Claude继续可输入，DAG可定位异常节点。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/SHELL-CORE-r2/evidence/results.json` |
| E2E-STA-011 | 07:54:34 | **BLOCKED** | 缺少只中断语义摘要链、同时保留真实 PTY/App 的外部条件。 | `/tmp/matou-independent-qa-final-20260830-071752/environment/blockers.md` |
| E2E-STA-012 | 07:47:54 | **PASS** | Shell/Claude 运行状态均在1秒内可见；Claude实测339ms，结束后回空闲。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/CLAUDE-NONGIT-r1/evidence/claude-status.json` |
| E2E-SORT-001 | 07:32:33 | **PASS** | Shell 草稿不换序，回车提交后目标移首且焦点保持。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/SHELL-CORE-r2/evidence/results.json` |
| E2E-SORT-002 | 07:32:33 | **PASS** | Ctrl+C 与真实 stdin 控制操作更新交互序并将目标移首。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/SHELL-CORE-r2/evidence/results.json` |
| E2E-SORT-003 | 07:49:59 | **PASS** | Claude 草稿不换序；发送后目标移首，回答完成不再次换序且焦点保留。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/CLAUDE-NONGIT-r1/evidence/sort-results.json` |
| E2E-SORT-004 | 07:48:55 | **FAIL** | 真实授权选项可操作，但其状态先被误报为运行中；拒绝/多选/停止继续全序列无法满足用例状态与排序闭环。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/CLAUDE-NONGIT-r1/evidence/permission2-results.json` |
| E2E-SORT-005 | 07:32:33 | **PASS** | 点击、悬浮、滚动和打开 DAG 不更新顺序。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/SHELL-CORE-r2/evidence/results.json` |
| E2E-SORT-006 | 07:49:59 | **PASS** | Claude 后台回答结束未把自己移首；用户随后操作的 Shell 始终保持首位。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/CLAUDE-NONGIT-r1/evidence/sort-results.json` |
| E2E-SORT-007 | 07:28:57 | **PASS** | 新 Shell 与新 Fork 都追加到当前层队尾并获得初始焦点；恢复不擅自按输出换序。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/CLAUDE-FORK-r1/evidence/child-results.json` |
| E2E-SORT-008 | 07:19:30 | **PASS** | B/C 快速提交得到稳定 C/B/A；重启保留，后续 B 交互变 B/C/A。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/SORT8-r1/evidence/results.json` |
| E2E-SORT-009 | 07:44:28 | **PASS** | 交互排序只改变横向投影，DAG 的父子/Fork 边和节点ID在排序与重启后保持。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/CLAUDE-NONGIT-r1/evidence/sibling-dag.png` |
| E2E-LIFE-001 | 07:34:08 | **PASS** | 结束有6个子会话的父节点出现影响确认；确认后父在DAG为历史，6子继续留在活动列表且连线保留。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/CLAUDE-FORK-r1/evidence/PARENT-LIFE-results.json` |
| E2E-LIFE-002 | 07:34:31 | **FAIL** | 叶子结束后可形成历史节点，但 DAG/主界面没有可发现的“移除历史叶子”确认入口。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/CLAUDE-FORK-r1/evidence/HISTORY-parent-inspect.json` |
| E2E-LIFE-003 | 07:34:31 | **FAIL** | 历史父保留关系，但选择历史父后没有历史操作菜单或“移除整条分支”入口。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/CLAUDE-FORK-r1/evidence/HISTORY-parent-inspect.json` |
| E2E-LIFE-004 | 07:34:31 | **FAIL** | 未发现“移除整条分支”专用确认页，无法列出后代数量或执行整支移除。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/CLAUDE-FORK-r1/evidence/HISTORY-parent-inspect.json` |
| E2E-LIFE-005 | 07:22:42 | **PASS** | 运行画布关闭弹框明确1个运行会话；取消后Tab数不变、原textarea焦点保留且进程继续。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/CLOSE-FINAL-r1/evidence/results.json` |
| E2E-LIFE-006 | 07:23:43 | **PASS** | 确认关闭后Tab消失并进入“已关闭画布 1”；重新打开恢复Tab、历史输出和工作区。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/CLOSE-FINAL-r1/evidence/restore-results.json` |
| E2E-LIFE-007 | 07:19:42 | **FAIL** | 窗口关闭后真实计数器继续、Dock reopen回原现场；但原终端输入焦点未恢复。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/HIDE-r2/evidence/results.json` |
| E2E-LIFE-008 | 07:19:06 | **FAIL** | 关系/Tab/历史恢复，但每画布 DAG 缩放均重置100%，观察位置不完整持久化。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/CAN003-r2/evidence/results.json` |
| E2E-LIFE-009 | 07:19:49 | **PASS** | 完整退出后 counter 2→2，重启仍2；历史与中断提示存在，命令未重复运行。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/QUIT-r1/evidence/results.json` |
| E2E-LIFE-010 | 07:36:51 | **FAIL** | 恢复条件失效后显示真实原因/重试，Shell和子列表可用；但点击重试后长期卡在“正在恢复”，失败条件恢复后也无再次重试入口。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/CLAUDE-WT-r1/evidence/restore-results.json` |
| E2E-ISO-001 | 07:50:43 | **FAIL** | 目录移动后画布/历史可见且输入/创建禁用并提示路径失效；但未提供在产品内选择移动后目录并重绑的闭环。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/WORKSPACE-MOVE-r1/evidence/new-canvas-fail.png` |
| E2E-ISO-002 | 07:54:34 | **BLOCKED** | 真实小仓库的 worktree 创建过快，未获得可重复的“准备中”异常退出窗口。 | `/tmp/matou-independent-qa-final-20260830-071752/environment/blockers.md` |
| E2E-ISO-003 | 07:36:51 | **PASS** | 只让一个 provider session 文件失效后，仅目标父节点报恢复失败；子节点真实可用，关系/徽章保留。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/CLAUDE-WT-r1/evidence/restore-results.json` |
| E2E-ISO-004 | 07:20:05 | **FAIL** | 独立窗口可脱出，DAG点击能唤起并聚焦；关闭独立窗后节点仍显示空闲，无历史/继续/重新打开入口。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/INDEPENDENT-r2/evidence/results.json` |
| E2E-ISO-005 | 07:18:18 | **BLOCKED** | 测试机只有一台1920×1080显示器。 | `/tmp/matou-independent-qa-final-20260830-071752/environment/display.txt` |
| E2E-ISO-006 | 07:28:55 | **PASS** | 白/深主题切换下状态、历史和普通/Fork连线语义保持；蓝色Fork边与普通边不只靠节点颜色区分。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/CLAUDE-FORK-r1/evidence/child-mixed-dag.png` |
| E2E-ISO-007 | 07:54:34 | **BLOCKED** | 没有只让 DAG BrowserWindow 创建失败、同时维持主窗口健康的真实系统条件。 | `/tmp/matou-independent-qa-final-20260830-071752/environment/blockers.md` |
| E2E-ISO-008 | 07:32:33 | **PASS** | 10个真实PTY并发输出未串流，UI仍可操作；状态结束后回空闲。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/SHELL-CORE-r2/evidence/results.json` |
| E2E-EDGE-001 | 07:50:43 | **FAIL** | cwd真实消失后新画布Tab保留并显示路径失效原因，但没有“重新创建”操作，恢复只能依赖外部把路径搬回。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/WORKSPACE-MOVE-r1/evidence/new-canvas-fail.png` |
| E2E-EDGE-002 | 07:50:43 | **FAIL** | 工作区路径失效时横向新增被整体禁用，没有在队尾生成带原因/重试/移除的失败卡片。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/WORKSPACE-MOVE-r1/evidence/new-canvas-fail.png` |
| E2E-EDGE-003 | 07:21:51 | **PASS** | 单节点DAG居中，缩放/平移/聚焦可用；点节点回终端。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/DAG100-r2/evidence/dag100.png` |
| E2E-EDGE-004 | 07:46:51 | **FAIL** | 真实四层关系已直接全量铺开且没有超层虚影/渐进变实；未体现设计要求的三层窗口。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/CLAUDE-NONGIT-r1/evidence/multilevel2-dag.png` |
| E2E-EDGE-005 | 07:30:03 | **PASS** | 父 main 未提交文件与新 worktree 独立写入在会话退出/App关闭后都保留，原目录未被回写。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/CLAUDE-WT-r1/evidence/results.json` |
| E2E-EDGE-006 | 07:28:12 | **PASS** | Claude真实/exit后完整重启仍为Shell，无恢复中/恢复失败；历史与子徽章保留并可直接输入。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/CLAUDE-FORK-r1/evidence/results.json` |
| E2E-EDGE-007 | 07:22:42 | **FAIL** | 取消关闭可保留单运行Shell和焦点，但未在同一画布完整保留“运行Claude+待输入Shell+草稿+滚动”组合现场。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/CLOSE-FINAL-r1/evidence/results.json` |
| E2E-EDGE-008 | 07:20:02 | **FAIL** | 进入200+字符路径后标题仍固定显示workspace根；无完整当前路径悬浮，窄窗优先级让位不符合。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/LONGPATH-r2/evidence/results.json` |
| E2E-EDGE-009 | 07:21:51 | **FAIL** | 新画布/兄弟/Fork/返回均可恢复焦点；但DAG跳视野外节点不滚入也不聚焦，整段键盘旅程中断。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/DAG100-r2/evidence/results.json` |
| E2E-EDGE-010 | 07:34:31 | **FAIL** | 历史父在DAG保留且子状态更新；点击历史父却回到子列表，没有历史详情页面。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/CLAUDE-FORK-r1/evidence/HISTORY-parent-inspect.json` |
| E2E-EDGE-011 | 07:19:59 | **PASS** | 空行/ANSI/超长行/中文/emoji/alternate-screen真实输出无控制字符泄漏、乱码或重复节点，卡宽固定。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/EDGE11-r2/evidence/results.json` |
| E2E-EDGE-012 | 07:28:55 | **PASS** | 同父普通Shell边与Claude Fork边同时存在；Fork边蓝色、普通边深色，节点类型和继承语义可区分。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/CLAUDE-FORK-r1/evidence/child-mixed-dag.png` |
| E2E-EDGE-013 | 07:23:43 | **PASS** | 已关闭画布恢复只生成一个Tab；历史入口随恢复消失，重新关闭后列表仍是一条同名记录。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/CLOSE-FINAL-r1/evidence/restore-results.json` |
| E2E-EDGE-014 | 07:35:16 | **PASS** | 白→深→白过程中运行命令、stdin等待、未提交草稿、顺序和焦点均保持。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/THEME-FINAL-r1/evidence/results.json` |
| E2E-EDGE-015 | 07:46:51 | **PASS** | 多父、多级、混合Shell/Claude真实关系中每个节点始终只有一个父；切型、排序和重启未复制边。 | `/tmp/matou-independent-qa-final-20260830-071752/groups/CLAUDE-NONGIT-r1/evidence/multilevel2-dag.png` |
| E2E-EDGE-016 | 07:54:34 | **BLOCKED** | 只提供同一提交811aec29的一个包体，没有后续覆盖安装包可执行升级连续性。 | `/tmp/matou-independent-qa-final-20260830-071752/environment/blockers.md` |

## 4. 失败项与最短复现

### D-01 DAG 跳转与观察现场不可靠（P0）
- 用例：CAN-003、DAG-007/008/010、LIFE-008、EDGE-009。
- 复现：创建 5+ 节点，将目标滑出视野 → 打开 DAG 点击远端节点；或将 DAG 缩放 80% 后切换画布/重启。
- 期望：目标完整进入视野并聚焦；每画布观察位置恢复。
- 实际：目标仍不在视野且无焦点；缩放回 100%；100节点首可操作约 5128ms。
- 证据：`/tmp/matou-independent-qa-final-20260830-071752/groups/DAG100-r2/evidence/results.json`、`/tmp/matou-independent-qa-final-20260830-071752/groups/CAN003-r2/evidence/results.json`。

### D-02 恢复失败的重试旅程卡住（P0）
- 用例：LIFE-010。
- 复现：让隔离 HOME 中真实 provider session 暂时失效 → 重启 → 点击“重试恢复”。
- 期望：失败条件下回到可重试错误；条件恢复后再次重试进入 Claude。
- 实际：第一次错误/原因/Shell/子关系正确；点击后长期停在“正在恢复 Claude Code 会话…”，重试入口消失。
- 证据：`/tmp/matou-independent-qa-final-20260830-071752/groups/CLAUDE-WT-r1/evidence/restore-results.json`。

### D-03 历史节点缺少清理与详情闭环（P1/P0）
- 用例：LIFE-002/003/004、EDGE-010。
- 复现：结束有子节点父会话 → DAG 选择历史父。
- 期望：历史详情、叶子移除、整支移除专用确认。
- 实际：历史样式和边保留，但点击回到子列表；没有历史菜单或整支移除入口。
- 证据：`/tmp/matou-independent-qa-final-20260830-071752/groups/CLAUDE-FORK-r1/evidence/HISTORY-parent-inspect.json`。

### D-04 布局/导航细节不符合产品交互（P1/P0）
- 用例：NAV-004/008/009、DAG-003、EDGE-004/008。
- 复现：四卡悬浮、窄窗、终端焦点连续 Tab、四层关系打开 DAG、进入超长路径。
- 实际：卡片不扩宽；窄窗未摘要让位；Tab 被终端持续消费；DAG没有三层虚影；标题不反映超长当前路径。
- 证据：`/tmp/matou-independent-qa-final-20260830-071752/groups/SHELL-CORE-r2/evidence/results.json`、`/tmp/matou-independent-qa-final-20260830-071752/groups/CLAUDE-NONGIT-r1/evidence/shortcut-keyboard.json`、`/tmp/matou-independent-qa-final-20260830-071752/groups/NAV9-r1/evidence/results.json`。

### D-05 异常/窗口续接仍有断点（P0/P1）
- 用例：ISO-001/004、EDGE-001/002、LIFE-007。
- 实际：移动目录后没有产品内重绑闭环；失效 cwd 时新建无重试卡；独立窗关闭无继续入口；主窗重开不恢复终端焦点。
- 证据：`/tmp/matou-independent-qa-final-20260830-071752/groups/WORKSPACE-MOVE-r1/evidence/new-canvas-fail.png`、`/tmp/matou-independent-qa-final-20260830-071752/groups/INDEPENDENT-r2/evidence/results.json`、`/tmp/matou-independent-qa-final-20260830-071752/groups/HIDE-r2/evidence/results.json`。

### D-06 Claude 明确授权等待仍被显示为运行中（P0）
- 用例：STA-006、SORT-004。
- 复现：让真实 Claude Write 工具弹出 Yes/No 权限确认 → 查看 DAG。
- 期望：待输入。实际：运行中。批准后真实文件可成功创建。
- 证据：`/tmp/matou-independent-qa-final-20260830-071752/groups/CLAUDE-NONGIT-r1/evidence/permission2-results.json`。

### D-07 取消关闭的复合现场保持未闭环（P1）
- 用例：EDGE-007。
- 复现：同画布准备运行 Claude、待输入 Shell 和未提交草稿 → 关闭后取消。
- 期望：草稿、滚动、焦点和两进程完整保持。
- 实际：单运行 Shell 的取消与焦点通过，但产品未在本轮复合现场中满足全部状态证据。
- 证据：`/tmp/matou-independent-qa-final-20260830-071752/groups/CLOSE-FINAL-r1/evidence/results.json`。

## 5. BLOCKED 与规格冲突

- 外部/设备/工件阻塞：E2E-REL-010, E2E-REL-011, E2E-REL-012, E2E-REL-019, E2E-STA-003, E2E-STA-007, E2E-STA-011, E2E-ISO-002, E2E-ISO-005, E2E-ISO-007, E2E-EDGE-016。详见 `/tmp/matou-independent-qa-final-20260830-071752/environment/blockers.md`。
- **规格冲突 1—REL-010**（PRD `§4.2.6`；E2E `§10 SPEC-Q08`）：用例用已有原始 Git ref 构造冲突；PRD 定义用户显示名冲突，且测试设计 SPEC-Q08 明确 Runtime 生成安全 Git ref。当前包生成 `matou/<name>-<id>`。
- **规格冲突 2—STA-003**（PRD `§4.4.2`；E2E `§3.2 CMD-WAIT`）：`read -r -p` 是 Bash 语法，在产品默认 zsh 中为 coprocess 读取并立即报错；没有形成待输入。
- 未修改原用例内容；两项均保留为 BLOCKED 并提供 PRD/测试锚点。

## 6. 数据文件

- 机器可读结果：`/tmp/matou-independent-qa-final-20260830-071752/execution-results-102.json`
- 证据根：`/tmp/matou-independent-qa-final-20260830-071752/groups/`
- 测试对象实现改动：无。
