# Matou 会话画布与 DAG 分支交互—第二轮独立黑盒执行记录

## 1. 验收结论

- **102 条逐条记录完成：PASS 68 / FAIL 25 / BLOCKED 9。**
- 本轮确认修复：终端焦点下键盘返回父会话、Claude 权限等待状态、Claude 恢复失败后的真实重试恢复。
- 当前仍未达到发布门槛：每画布 DAG 观察位置、DAG 远节点定位、悬浮/窄窗布局、历史节点移除、目录移动恢复、窗口重开焦点等核心旅程仍存在缺口。

## 2. 环境与隔离

| 项目 | 值 |
|---|---|
| App | `.worktrees/session-dag-canvas/apps/desktop/release/mac-arm64/Matou.app` |
| Commit | `1d30ea7de64035562151a69a23678f0732d660ae` |
| macOS | `15.7.4 (24G517), arm64` |
| 显示 | `1920×1080 单显示器` |
| 隔离根 | `/tmp/matou-independent-qa-round2-20260830-084703` |
| Claude | 真实 Claude Code 2.1.251，真实 OAuth；项目、会话、配置均写入隔离 HOME |
| 操作 | Playwright Electron 真实 UI 键鼠、真实 PTY/Git/worktree/Claude；未改数据库、未改产品实现、未使用 mock |

## 3. 逐条执行结果

| 用例 ID | 时间（UTC+8） | 结果 | 实际观察与用户影响 | 证据 |
|---|---:|---|---|---|
| E2E-CAN-001 | 08:48:22 | **PASS** | 隔离 profile 首次启动即显示默认画布和可直接输入的 Shell；真实 pwd 指向隔离 WS-GIT。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/SMOKE-r1/evidence/body.txt` |
| E2E-CAN-002 | 08:48:30 | **PASS** | 顶部 + 直接产生并选中新画布，无类型/工作树弹框；新 Shell 自动聚焦。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/CANVAS-RENAME-r1/evidence/results.json` |
| E2E-CAN-003 | 08:48:26 | **FAIL** | 两张画布各自的节点集合、横向位置和焦点可恢复；但 DAG 的 80%/110% 缩放在完整重启后均变为 100%，每画布观察位置未持久化。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/CAN003-r2/evidence/results.json` |
| E2E-CAN-004 | 08:48:30 | **PASS** | 连续名称为“新画布/新画布 2”；双击重命名为“性能验证”后完整重启仍保留。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/CANVAS-RENAME-r1/evidence/results.json` |
| E2E-CAN-005 | 08:48:26 | **PASS** | 事项/画布切换后会话集合、上次焦点与可输入现场保持；数据未串到其他画布。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/CAN003-r2/evidence/results.json` |
| E2E-CAN-006 | 08:51:32 | **PASS** | 横向 Shell 追加队尾并自动焦点，未二次点击即可输入；旧终端未收到内容。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/SHELL-CORE-r3/evidence/results.json` |
| E2E-CAN-007 | 09:01:23 | **PASS** | 真实 Claude 两轮回复后输入 textarea 持续聚焦，第二条无需鼠标即可提交。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/CLAUDE-FORK-r5/evidence/child-results.json` |
| E2E-CAN-008 | 09:26:55 | **PASS** | 所有 App、Electron、Claude、Git、worktree 与证据路径均位于唯一 /tmp 根；未使用默认 profile 启动。 | `/tmp/matou-independent-qa-round2-20260830-084703/environment/isolation.md` |
| E2E-REL-001 | 08:51:32 | **PASS** | 根层新增 Shell 无弹框、追加队尾、自动焦点，DAG 中无会话父子边。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/SHELL-CORE-r3/evidence/results.json` |
| E2E-REL-002 | 09:01:23 | **PASS** | Claude 父的子列表中新增普通 Shell 后与 Claude 子共享同一父，追加并聚焦。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/CLAUDE-FORK-r5/evidence/child-results.json` |
| E2E-REL-003 | 09:01:23 | **PASS** | 同一横向子列表真实混排 Claude 与 Shell；两类节点独立工作，DAG 保留共同父。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/CLAUDE-FORK-r5/evidence/child-results.json` |
| E2E-REL-004 | 09:01:04 | **PASS** | Shell 启动真实 Claude 后节点原地切型；首轮前 Fork 图标可见且禁用，完成后启用。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/CLAUDE-FORK-r5/evidence/results.json` |
| E2E-REL-005 | 09:01:04 | **PASS** | 首轮前 Fork 悬浮文案为“完成首轮对话后可创建分支”且按钮禁用；首轮完成后可打开弹框。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/CLAUDE-FORK-r5/evidence/results.json` |
| E2E-REL-006 | 09:01:23 | **PASS** | 真实 Fork 子 Claude 正常就绪，并准确回答父会话唯一 token；父子后续输入隔离。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/CLAUDE-FORK-r5/evidence/child-results.json` |
| E2E-REL-007 | 09:01:27 | **PASS** | 当前工作树 Fork 的父子路径一致、双方显示共享工作树，子 Shell 可读父目录 uncommitted.txt。 | `/tmp/matou-independent-qa-round2-20260830-084703/environment/qa-current-files-r5.log` |
| E2E-REL-008 | 09:02:27 | **PASS** | 新 worktree Fork 产生独立路径和生成分支；子目录无父未提交文件，原 main/uncommitted 保留且子写入不回原目录。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/CLAUDE-WT-r1/evidence/results.json` |
| E2E-REL-009 | 09:02:42 | **PASS** | 非 Git 目录新工作树项真实禁用并提示需要 Git；键盘 Space 不改变选择，当前目录 Fork 成功，普通 Shell 仍可直接新增。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/CLAUDE-NONGIT-r1/evidence/results.json` |
| E2E-REL-010 | 09:07:23 | **FAIL** | 同一父节点连续创建两个显示名为 same-visible-name 的真实新 worktree 子节点均成功，节点数 1→2→3；Runtime 生成了两个不同且安全的 Git ref，但显示名唯一性约束未生效。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/CLAUDE-WT-r1/evidence/REL010-results.json` |
| E2E-REL-011 | 09:26:55 | **BLOCKED** | 真实小仓库 worktree 准备快于可重复注入只读失败的用户可见窗口；未使用延时钩子或内部接口。 | `/tmp/matou-independent-qa-round2-20260830-084703/environment/blockers.md` |
| E2E-REL-012 | 09:26:55 | **BLOCKED** | 真实父 provider 身份无法在亚秒准备窗口内定点失效而不改会话存储/替换 provider。 | `/tmp/matou-independent-qa-round2-20260830-084703/environment/blockers.md` |
| E2E-REL-013 | 09:03:49 | **PASS** | 从同级入口创建第二 Claude；新兄弟继承共同父 NONGIT_READY，未继承 C1_SECRET，DAG 显示共同父。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/CLAUDE-NONGIT-r1/evidence/sibling-results.json` |
| E2E-REL-014 | 09:16:41 | **PASS** | 真实建立多级 P→C→G→H 链，直接子列表逐层只显示下一层，DAG 连线保留。 | `/tmp/matou-independent-qa-round2-20260830-084703/environment/qa-multilevel-rerun.log` |
| E2E-REL-015 | 09:01:04 | **PASS** | 真实 /exit 后同一节点转为 Shell，不显示“已退出”，子数量/导航/关系保留，Fork 隐藏。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/CLAUDE-FORK-r5/evidence/results.json` |
| E2E-REL-016 | 09:01:04 | **PASS** | 主动退出后的 Shell 仍可进入原子列表；完整重启后父继续是 Shell，没有自动恢复。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/CLAUDE-FORK-r5/evidence/results.json` |
| E2E-REL-017 | 09:15:40 | **PASS** | 同一稳定节点可再次进入 Claude；完成新一轮后 Fork 能力恢复，既有关系未重写。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/CLAUDE-NONGIT-r1/evidence/card-inspect.json` |
| E2E-REL-018 | 09:13:40 | **FAIL** | 同目录第二个 Shell 创建后，仅新节点显示“共享工作树”，既有节点未同步显示；移除新节点后剩余标记消退。共享占用的双向反馈不一致。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/TESTNEW-r1/evidence/shared-marker.json` |
| E2E-REL-019 | 09:26:55 | **BLOCKED** | 真实已认证账号没有 Team 队友子会话前置。 | `/tmp/matou-independent-qa-round2-20260830-084703/environment/blockers.md` |
| E2E-NAV-001 | 09:01:23 | **PASS** | 父徽章显示直接子总数、Claude/Shell 构成和运行/空闲/中断计数；错误优先级由状态引擎实时更新。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/CLAUDE-FORK-r5/evidence/child-results.json` |
| E2E-NAV-002 | 09:01:23 | **PASS** | 点击徽章以横向直接子列表替换父卡；孙节点不混入，焦点落在选中子节点。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/CLAUDE-FORK-r5/evidence/child-results.json` |
| E2E-NAV-003 | 09:01:33 | **PASS** | 同级 6 节点每屏不超过四卡；横向 wheel 可在 scrollLeft 952 与 0 间访问首尾。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/CLAUDE-FORK-r5/evidence/NAV-pull-results.json` |
| E2E-NAV-004 | 08:51:32 | **FAIL** | 四卡布局中目标卡悬浮前/中/后宽度均为 305.3px，相邻卡无收缩；点击聚焦后也没有保持扩展。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/SHELL-CORE-r3/evidence/results.json` |
| E2E-NAV-005 | 09:01:39 | **PASS** | 中部一次大幅右滑只到 scrollLeft=0，仍停在子列表且未出现父视图。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/CLAUDE-FORK-r5/evidence/NAV-boundary-results.json` |
| E2E-NAV-006 | 09:01:39 | **PASS** | 已在最左后的小幅独立右滑仍留在子列表并保持原焦点，未越级。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/CLAUDE-FORK-r5/evidence/NAV-boundary-results.json` |
| E2E-NAV-007 | 09:01:39 | **PASS** | 边界第二次大幅右滑返回父视图，父 textarea 自动恢复焦点。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/CLAUDE-FORK-r5/evidence/NAV-boundary-results.json` |
| E2E-NAV-008 | 09:05:45 | **PASS** | 终端聚焦后一次 Tab 即到达“返回父会话”，Enter 返回父节点且父终端自动聚焦；鼠标入口同样可用。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/CLAUDE-NONGIT-r1/evidence/shortcut-keyboard.json` |
| E2E-NAV-009 | 08:49:09 | **FAIL** | 700px 窄窗只保留一个 440px 完整卡并横向溢出，其他会话未收敛为可识别摘要，状态/数量的优先让位规则未实现。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/NAV9-r1/evidence/results.json` |
| E2E-NAV-010 | 09:01:39 | **PASS** | 最左边界在静态/持续输出场景不自动切层；小手势不返回，清晰第二大手势才返回父会话。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/CLAUDE-FORK-r5/evidence/NAV-boundary-results.json` |
| E2E-DAG-001 | 09:05:45 | **PASS** | 真实按住 Option+Tab 650ms 打开第二个独立 DAG BrowserWindow，松键后仍保留。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/CLAUDE-NONGIT-r1/evidence/shortcut-keyboard.json` |
| E2E-DAG-002 | 08:51:32 | **PASS** | Shell 短 Tab 完成 baseline.txt 且窗口数不变；未出现 DAG 闪现。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/SHELL-CORE-r3/evidence/results.json` |
| E2E-DAG-003 | 09:17:31 | **FAIL** | 在当前隔离数据上连续尝试从真实 Claude 子节点继续建立更深层关系时，恢复后的目标卡未出现可用 Fork 入口，未形成可验收的父/当前/子三层与远层虚影闭环。 | `/tmp/matou-independent-qa-round2-20260830-084703/environment/qa-multilevel-target.log` |
| E2E-DAG-004 | 08:51:32 | **PASS** | 独立画布缩放、100%复位、聚焦与平移均响应；节点布局保持。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/SHELL-CORE-r3/evidence/results.json` |
| E2E-DAG-005 | 08:51:32 | **PASS** | 100% 节点含名称/类型/分支/状态/最近四行/活动时间；缩放时保持核心信息和固定卡宽。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/SHELL-CORE-r3/evidence/results.json` |
| E2E-DAG-006 | 08:48:34 | **PASS** | LIVE_DAG_1..7 实时轮换为最近四行，节点位置与260×154边界不移动。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/DAG-LIVE-r2/evidence/results.json` |
| E2E-DAG-007 | 09:19:00 | **FAIL** | 6 个真实 Shell 中目标卡初始在视野外；DAG 137ms 打开，但用节点最新输出唯一标记搜索得到 0 个结果，无法从 DAG 选择目标，目标未获焦点。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/DAG-JUMP-r1/evidence/results.json` |
| E2E-DAG-008 | 09:19:00 | **FAIL** | 目标未能通过 DAG 被选中；主列表布局虽自行滚到边缘，但目标 textarea 焦点=false，后续悬浮/缩放持续可见旅程中断。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/DAG-JUMP-r1/evidence/results.json` |
| E2E-DAG-009 | 09:01:47 | **PASS** | 从子列表点击活动父节点可回到父层级，子徽章和终端焦点恢复。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/CLAUDE-FORK-r5/evidence/PARENT-LIFE-results.json` |
| E2E-DAG-010 | 09:19:00 | **FAIL** | 100 个真实 UI 节点已创建；当前三层首屏已虚拟化且打开反馈实测 137ms，但远节点搜索/选择仍失败，无法完成远节点定位与后续并发交互闭环。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/DAG-JUMP-r1/evidence/results.json` |
| E2E-STA-001 | 08:51:32 | **PASS** | Shell 从空闲→运行中→空闲真实切换，结束后提示符与 DAG 一致。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/SHELL-CORE-r3/evidence/results.json` |
| E2E-STA-002 | 08:51:32 | **PASS** | exit 23 在 DAG 显示异常；同一 Shell 随后可输出 recovered。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/SHELL-CORE-r3/evidence/results.json` |
| E2E-STA-003 | 08:51:32 | **FAIL** | 使用 zsh 真实命令 `printf; read -r` 出现明确 enter value prompt 后，DAG 仍显示运行中而非待输入；输入值后命令正常完成。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/SHELL-CORE-r3/evidence/results.json` |
| E2E-STA-004 | 08:51:32 | **PASS** | 无标准提示的 python input 只显示运行中而不误判待输入，提交后继续。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/SHELL-CORE-r3/evidence/results.json` |
| E2E-STA-005 | 09:05:14 | **PASS** | 真实 Claude 提交后 339ms 显示运行中，回复完成后回空闲，输入焦点保留。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/CLAUDE-NONGIT-r1/evidence/claude-status.json` |
| E2E-STA-006 | 09:05:25 | **PASS** | 真实 Claude Write 权限确认出现时，DAG 将目标节点标为“等待输入”；确认后继续执行并写入真实文件，焦点保留。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/CLAUDE-NONGIT-r1/evidence/permission2-results.json` |
| E2E-STA-007 | 09:26:55 | **BLOCKED** | 没有可逆且只影响一个隔离 Claude turn 的真实网络/账户故障条件。 | `/tmp/matou-independent-qa-round2-20260830-084703/environment/blockers.md` |
| E2E-STA-008 | 09:07:51 | **PASS** | 错误/中断/运行/空闲均能进入徽章和 DAG，错误优先；历史节点不覆盖活动子状态。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/CLAUDE-WT-r1/evidence/restore-results.json` |
| E2E-STA-009 | 09:01:47 | **PASS** | 结束父节点从活动列表收起，在 DAG 标为历史；活跃子数量/状态继续独立统计。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/CLAUDE-FORK-r5/evidence/PARENT-LIFE-results.json` |
| E2E-STA-010 | 08:51:32 | **PASS** | 一个 Shell 异常未串流，其他 Shell/Claude继续可输入，DAG可定位异常节点。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/SHELL-CORE-r3/evidence/results.json` |
| E2E-STA-011 | 09:26:55 | **BLOCKED** | 没有只中断语义摘要、同时保留同一真实 PTY/runtime 的 OS 外部条件。 | `/tmp/matou-independent-qa-round2-20260830-084703/environment/blockers.md` |
| E2E-STA-012 | 09:05:14 | **PASS** | Shell/Claude 运行状态均在1秒内可见；Claude实测339ms，结束后回空闲。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/CLAUDE-NONGIT-r1/evidence/claude-status.json` |
| E2E-SORT-001 | 08:51:32 | **PASS** | Shell 草稿不换序，回车提交后目标移首且焦点保持。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/SHELL-CORE-r3/evidence/results.json` |
| E2E-SORT-002 | 08:51:32 | **PASS** | Ctrl+C 与真实 stdin 控制操作更新交互序并将目标移首。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/SHELL-CORE-r3/evidence/results.json` |
| E2E-SORT-003 | 09:05:38 | **PASS** | Claude 草稿不换序；发送后目标移首，回答完成不再次换序且焦点保留。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/CLAUDE-NONGIT-r1/evidence/sort-results.json` |
| E2E-SORT-004 | 09:05:25 | **FAIL** | 真实允许授权路径已执行且状态识别恢复，但同一用例要求的允许、拒绝、选项、停止/继续四类排序闭环没有共同的可见排序反馈，仍不满足完整验收标准。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/CLAUDE-NONGIT-r1/evidence/permission2-results.json` |
| E2E-SORT-005 | 08:51:32 | **PASS** | 点击、悬浮、滚动和打开 DAG 不更新顺序。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/SHELL-CORE-r3/evidence/results.json` |
| E2E-SORT-006 | 09:05:38 | **PASS** | Claude 后台回答结束未把自己移首；用户随后操作的 Shell 始终保持首位。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/CLAUDE-NONGIT-r1/evidence/sort-results.json` |
| E2E-SORT-007 | 09:01:23 | **PASS** | 新 Shell 与新 Fork 都追加到当前层队尾并获得初始焦点；恢复不擅自按输出换序。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/CLAUDE-FORK-r5/evidence/child-results.json` |
| E2E-SORT-008 | 08:49:24 | **PASS** | B/C 快速提交得到稳定 C/B/A；重启保留，后续 B 交互变 B/C/A。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/SORT8-r1/evidence/results.json` |
| E2E-SORT-009 | 09:03:49 | **PASS** | 交互排序只改变横向投影，DAG 的父子/Fork 边和节点ID在排序与重启后保持。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/CLAUDE-NONGIT-r1/evidence/sibling-dag.png` |
| E2E-LIFE-001 | 09:01:47 | **PASS** | 结束有6个子会话的父节点出现影响确认；确认后父在DAG为历史，6子继续留在活动列表且连线保留。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/CLAUDE-FORK-r5/evidence/PARENT-LIFE-results.json` |
| E2E-LIFE-002 | 09:01:51 | **FAIL** | 历史叶子可在 DAG 中看到，但节点和主界面没有“移除历史叶子”确认入口。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/CLAUDE-FORK-r5/evidence/HISTORY-parent-inspect.json` |
| E2E-LIFE-003 | 09:01:51 | **FAIL** | 历史父保留 6 个子关系；选择历史父后仍没有受限说明或“移除整条分支”入口。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/CLAUDE-FORK-r5/evidence/HISTORY-parent-inspect.json` |
| E2E-LIFE-004 | 09:01:51 | **FAIL** | 没有可发现的整条分支移除确认页，无法核对后代数量、取消和确认后的文件保留。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/CLAUDE-FORK-r5/evidence/HISTORY-parent-inspect.json` |
| E2E-LIFE-005 | 08:51:53 | **PASS** | 运行画布关闭弹框明确1个运行会话；取消后Tab数不变、原textarea焦点保留且进程继续。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/CLOSE-FINAL-r1/evidence/results.json` |
| E2E-LIFE-006 | 08:51:57 | **PASS** | 确认关闭后Tab消失并进入“已关闭画布 1”；重新打开恢复Tab、历史输出和工作区。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/CLOSE-FINAL-r1/evidence/restore-results.json` |
| E2E-LIFE-007 | 08:49:15 | **FAIL** | 关闭最后主窗口后真实计数器继续增长且重新打开回原画布，但原终端输入焦点=false，用户需再点一次。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/HIDE-r2/evidence/results.json` |
| E2E-LIFE-008 | 08:48:26 | **FAIL** | Tab、节点、顺序、焦点与横向位置恢复；两张画布 DAG 缩放均重置为 100%，观察位置恢复不完整。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/CAN003-r2/evidence/results.json` |
| E2E-LIFE-009 | 08:49:44 | **PASS** | 完整退出后 counter 2→2，重启仍2；历史与中断提示存在，命令未重复运行。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/QUIT-r1/evidence/results.json` |
| E2E-LIFE-010 | 09:07:51 | **PASS** | 移走真实 provider transcript 后节点显示恢复失败、原因和重试；失败条件保持时可再次重试且 Shell/子列表可用；放回 transcript 再重试成功回到 Claude，节点 ID、3 个子关系与焦点保持。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/CLAUDE-WT-r1/evidence/restore-results.json` |
| E2E-ISO-001 | 09:14:18 | **FAIL** | App 完整退出后真实移动 workspace，重启仍把旧路径会话当作可操作状态，创建入口保持启用且没有“选择移动后目录”入口；外部把目录移回后才恢复。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/WINDOW-LIST-r1/evidence/workspace-move-results.json` |
| E2E-ISO-002 | 09:26:55 | **BLOCKED** | 真实隔离仓库的 worktree 准备在外部终止前完成，未稳定命中“准备中”状态。 | `/tmp/matou-independent-qa-round2-20260830-084703/environment/blockers.md` |
| E2E-ISO-003 | 09:07:51 | **PASS** | 只让一个 provider session 文件失效后，仅目标父节点报恢复失败；子节点真实可用，关系/徽章保留。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/CLAUDE-WT-r1/evidence/restore-results.json` |
| E2E-ISO-004 | 08:49:18 | **FAIL** | 节点可脱出、从 DAG 唤起独立窗并聚焦；关闭独立窗后 DAG 只显示历史卡，未提供可执行的继续/重新打开入口。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/INDEPENDENT-r2/evidence/results.json` |
| E2E-ISO-005 | 09:26:55 | **BLOCKED** | 测试主机仅一台 1920×1080 显示器。 | `/tmp/matou-independent-qa-round2-20260830-084703/environment/blockers.md` |
| E2E-ISO-006 | 09:01:21 | **PASS** | 白/深主题切换下状态、历史和普通/Fork连线语义保持；蓝色Fork边与普通边不只靠节点颜色区分。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/CLAUDE-FORK-r5/evidence/child-mixed-dag.png` |
| E2E-ISO-007 | 09:26:55 | **BLOCKED** | 没有只让 DAG BrowserWindow 创建失败且保持主窗口健康的真实系统条件。 | `/tmp/matou-independent-qa-round2-20260830-084703/environment/blockers.md` |
| E2E-ISO-008 | 08:51:32 | **PASS** | 10个真实PTY并发输出未串流，UI仍可操作；状态结束后回空闲。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/SHELL-CORE-r3/evidence/E2E-ISO-008.png` |
| E2E-EDGE-001 | 09:14:18 | **FAIL** | 真实 cwd 移走后重启没有显示 Shell 启动失败或重新创建操作，旧路径仍被展示为普通可用会话。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/WINDOW-LIST-r1/evidence/workspace-move-results.json` |
| E2E-EDGE-002 | 09:14:18 | **FAIL** | 真实 cwd 失效时横向创建仍保持启用，但没有在队尾形成包含失败原因、重试和移除的失败卡片。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/WINDOW-LIST-r1/evidence/workspace-move-results.json` |
| E2E-EDGE-003 | 08:49:07 | **PASS** | 单节点DAG居中，缩放/平移/聚焦可用；点节点回终端。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/DAG100-r2/evidence/dag100.png` |
| E2E-EDGE-004 | 09:17:31 | **FAIL** | 真实多级建立尝试未进入可观察的远层虚影/渐进加载阶段，用户从恢复后的子 Claude 无法继续 Fork，七层画布闭环未成立。 | `/tmp/matou-independent-qa-round2-20260830-084703/environment/qa-multilevel-target.log` |
| E2E-EDGE-005 | 09:02:27 | **PASS** | 父 main 未提交文件与新 worktree 独立写入在会话退出/App关闭后都保留，原目录未被回写。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/CLAUDE-WT-r1/evidence/results.json` |
| E2E-EDGE-006 | 09:01:04 | **PASS** | Claude真实/exit后完整重启仍为Shell，无恢复中/恢复失败；历史与子徽章保留并可直接输入。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/CLAUDE-FORK-r5/evidence/results.json` |
| E2E-EDGE-007 | 08:51:53 | **FAIL** | 取消关闭可保留真实运行 Shell 和焦点，但“运行 Claude + 待输入 Shell + 草稿 + 滚动”组合现场仍缺少完整保留证据和统一反馈。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/CLOSE-FINAL-r1/evidence/results.json` |
| E2E-EDGE-008 | 08:49:20 | **FAIL** | 200+ 字符真实路径能在 Tab title 中读取，但窄窗只剩单卡，未同时验证异常徽章、子数量与创建入口按规定优先让位。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/LONGPATH-r2/evidence/results.json` |
| E2E-EDGE-009 | 09:19:00 | **FAIL** | 新画布、兄弟、Fork、明确返回均能聚焦；DAG 对视野外唯一输出节点搜索为 0，目标 textarea 未聚焦，键盘旅程在最后一步中断。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/DAG-JUMP-r1/evidence/results.json` |
| E2E-EDGE-010 | 09:01:51 | **FAIL** | 历史父及活动子在 DAG 中保留；点击历史父回到子列表，而不是显示历史父详情，无法从子节点正确回看父历史。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/CLAUDE-FORK-r5/evidence/HISTORY-parent-inspect.json` |
| E2E-EDGE-011 | 08:52:40 | **PASS** | 空行/ANSI/超长行/中文/emoji/alternate-screen真实输出无控制字符泄漏、乱码或重复节点，卡宽固定。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/EDGE11-r2/evidence/results.json` |
| E2E-EDGE-012 | 09:01:21 | **PASS** | 同父普通Shell边与Claude Fork边同时存在；Fork边蓝色、普通边深色，节点类型和继承语义可区分。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/CLAUDE-FORK-r5/evidence/child-mixed-dag.png` |
| E2E-EDGE-013 | 08:51:57 | **PASS** | 已关闭画布恢复只生成一个Tab；历史入口随恢复消失，重新关闭后列表仍是一条同名记录。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/CLOSE-FINAL-r1/evidence/restore-results.json` |
| E2E-EDGE-014 | 09:13:25 | **PASS** | 白→深→白过程中运行命令、stdin等待、未提交草稿、顺序和焦点均保持。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/THEME-STATE-r2/evidence/results.json` |
| E2E-EDGE-015 | 09:03:49 | **PASS** | 多父、多级、混合Shell/Claude真实关系中每个节点始终只有一个父；切型、排序和重启未复制边。 | `/tmp/matou-independent-qa-round2-20260830-084703/groups/CLAUDE-NONGIT-r1/evidence/sibling-dag.png` |
| E2E-EDGE-016 | 09:26:55 | **BLOCKED** | 只提供 commit 1d30ea7 的一个包体，没有可执行真实覆盖安装的后续包。 | `/tmp/matou-independent-qa-round2-20260830-084703/environment/blockers.md` |

## 4. 失败清单与最短复现

### E2E-CAN-003
- **最短复现**：在画布 A 将 DAG 缩放到 80%，在画布 B 缩放到 110%；完整退出 App 后用同一隔离 profile 重启，分别重新打开两张画布的 DAG。
- **期望**：每张画布恢复各自的节点、焦点、横向位置、DAG 平移和缩放。
- **实际**：两张画布各自的节点集合、横向位置和焦点可恢复；但 DAG 的 80%/110% 缩放在完整重启后均变为 100%，每画布观察位置未持久化。
- **用户影响**：用户切换或重启后丢失每张画布的观察尺度，需要反复重新定位复杂关系图。
- **证据**：`/tmp/matou-independent-qa-round2-20260830-084703/groups/CAN003-r2/evidence/results.json`

### E2E-REL-010
- **最短复现**：在同一 Claude 父节点连续两次选择“从新工作树创建分支”，两次显示名称都输入 `same-visible-name` 并确认。
- **期望**：第二次在输入框就地提示同父活跃节点显示名冲突、保留文本且不新增节点；Runtime Git ref 仍应安全唯一。
- **实际**：同一父节点连续创建两个显示名为 same-visible-name 的真实新 worktree 子节点均成功，节点数 1→2→3；Runtime 生成了两个不同且安全的 Git ref，但显示名唯一性约束未生效。
- **用户影响**：同一父节点下出现两个同名分支，用户在列表和 DAG 中难以区分工作分支，容易进入错误会话。
- **证据**：`/tmp/matou-independent-qa-round2-20260830-084703/groups/CLAUDE-WT-r1/evidence/REL010-results.json`

### E2E-REL-018
- **最短复现**：从单个节点开始，横向新增同目录 Shell；观察两个节点的共享标记，再关闭新增节点。
- **期望**：目录被两个节点占用时两边均显示共享标记；恢复单节点占用后剩余节点标记消失。
- **实际**：同目录第二个 Shell 创建后，仅新节点显示“共享工作树”，既有节点未同步显示；移除新节点后剩余标记消退。共享占用的双向反馈不一致。
- **用户影响**：既有节点没有共享风险提示，用户可能误以为工作目录独占并产生互相覆盖。
- **证据**：`/tmp/matou-independent-qa-round2-20260830-084703/groups/TESTNEW-r1/evidence/shared-marker.json`

### E2E-NAV-004
- **最短复现**：在一屏四卡布局记录卡宽；悬浮第三卡后移出，再点击第二卡并把鼠标移开。
- **期望**：悬浮卡平滑扩展、邻卡适度收缩；移出恢复等宽；聚焦卡保持适度扩展。
- **实际**：四卡布局中目标卡悬浮前/中/后宽度均为 305.3px，相邻卡无收缩；点击聚焦后也没有保持扩展。
- **用户影响**：用户难以通过悬浮或聚焦获得更大的可读/输入空间，四会话并行效率下降。
- **证据**：`/tmp/matou-independent-qa-round2-20260830-084703/groups/SHELL-CORE-r3/evidence/results.json`

### E2E-NAV-009
- **最短复现**：建立四个同级会话，将主窗口逐步缩窄至约 700px，再恢复宽度。
- **期望**：当前卡保留完整交互区，其余卡收敛为可识别标题、状态和摘要；数量/异常优先于长路径。
- **实际**：700px 窄窗只保留一个 440px 完整卡并横向溢出，其他会话未收敛为可识别摘要，状态/数量的优先让位规则未实现。
- **用户影响**：窄屏只剩单张完整卡和横向溢出，用户看不到其余会话的状态与数量。
- **证据**：`/tmp/matou-independent-qa-round2-20260830-084703/groups/NAV9-r1/evidence/results.json`

### E2E-DAG-003
- **最短复现**：建立上下各两层以上的真实 Claude 关系，以中间节点为当前节点打开 DAG 并向祖先/后代方向平移。
- **期望**：默认完整显示父层、当前兄弟层、子层；远层以虚影提示并随平移变实。
- **实际**：在当前隔离数据上连续尝试从真实 Claude 子节点继续建立更深层关系时，恢复后的目标卡未出现可用 Fork 入口，未形成可验收的父/当前/子三层与远层虚影闭环。
- **用户影响**：复杂分支中用户看不到稳定的三层上下文与远层方向，关系导航无法闭环。
- **证据**：`/tmp/matou-independent-qa-round2-20260830-084703/environment/qa-multilevel-target.log`

### E2E-DAG-007
- **最短复现**：建立 6 个同级真实 Shell，使目标卡在主列表视野外；打开 DAG，以目标卡最新唯一输出定位并点击。
- **期望**：DAG 关闭，主列表滚动到目标完整可见并在空间允许时居中；目标获选中和输入焦点。
- **实际**：6 个真实 Shell 中目标卡初始在视野外；DAG 137ms 打开，但用节点最新输出唯一标记搜索得到 0 个结果，无法从 DAG 选择目标，目标未获焦点。
- **用户影响**：用户从 DAG 找不到视野外兄弟节点，无法用关系图快速跳转到目标工作会话。
- **证据**：`/tmp/matou-independent-qa-round2-20260830-084703/groups/DAG-JUMP-r1/evidence/results.json`

### E2E-DAG-008
- **最短复现**：按 DAG-007 从 DAG 选择列表边缘目标；悬浮扩展目标、缩窄/放宽窗口并直接键入。
- **期望**：每次布局变化后目标持续完整可见，键入只进入该目标会话。
- **实际**：目标未能通过 DAG 被选中；主列表布局虽自行滚到边缘，但目标 textarea 焦点=false，后续悬浮/缩放持续可见旅程中断。
- **用户影响**：DAG 跳转后目标未聚焦，后续布局变化与键盘输入旅程中断。
- **证据**：`/tmp/matou-independent-qa-round2-20260830-084703/groups/DAG-JUMP-r1/evidence/results.json`

### E2E-DAG-010
- **最短复现**：通过真实 UI 建立 100 个节点；打开 DAG，连续平移/缩放/聚焦各 10 次，选择远节点，并让 10 个 Shell 并行输出。
- **期望**：即时显示当前三层，画布持续流畅，远节点可定位并返回主列表，并发摘要更新不阻断操作。
- **实际**：100 个真实 UI 节点已创建；当前三层首屏已虚拟化且打开反馈实测 137ms，但远节点搜索/选择仍失败，无法完成远节点定位与后续并发交互闭环。
- **用户影响**：大画布虽能打开，但远节点选择失效，规模增大后 DAG 失去实际导航价值。
- **证据**：`/tmp/matou-independent-qa-round2-20260830-084703/groups/DAG-JUMP-r1/evidence/results.json`

### E2E-STA-003
- **最短复现**：在空闲 zsh Shell 输入 `printf 'enter value: '; read -r value; echo $value`；prompt 出现后查看卡片、父徽章和画布 Tab，再输入值回车。
- **期望**：明确 prompt 出现后卡片、父徽章和 Tab 均显示琥珀色待输入；提交值后短暂运行并回到空闲。
- **实际**：使用 zsh 真实命令 `printf; read -r` 出现明确 enter value prompt 后，DAG 仍显示运行中而非待输入；输入值后命令正常完成。
- **用户影响**：等待用户输入的 Shell 被误报为运行中，用户会遗漏需要人工响应的任务。
- **证据**：`/tmp/matou-independent-qa-round2-20260830-084703/groups/SHELL-CORE-r3/evidence/results.json`

### E2E-SORT-004
- **最短复现**：分别在后位 Claude 会话执行真实允许授权、拒绝授权、完成选项、停止/继续，并在每一步记录列表顺序。
- **期望**：每次明确用户交互后，对应会话立即移动到最前，其他会话相对顺序稳定。
- **实际**：真实允许授权路径已执行且状态识别恢复，但同一用例要求的允许、拒绝、选项、停止/继续四类排序闭环没有共同的可见排序反馈，仍不满足完整验收标准。
- **用户影响**：授权/选项/停止等关键交互后会话排序反馈不完整，用户难以找到刚处理的工作。
- **证据**：`/tmp/matou-independent-qa-round2-20260830-084703/groups/CLAUDE-NONGIT-r1/evidence/permission2-results.json`

### E2E-LIFE-002
- **最短复现**：在 DAG 选择没有子节点的已退出叶子，查找并触发移除；先取消，再确认，并检查关系和工作树。
- **期望**：出现无后代的确认；取消保留，确认只移除该叶子和连线，本地工作树继续存在。
- **实际**：历史叶子可在 DAG 中看到，但节点和主界面没有“移除历史叶子”确认入口。
- **用户影响**：历史叶子持续堆积且没有安全清理入口，画布会被无用节点占满。
- **证据**：`/tmp/matou-independent-qa-round2-20260830-084703/groups/CLAUDE-FORK-r5/evidence/HISTORY-parent-inspect.json`

### E2E-LIFE-003
- **最短复现**：在 DAG 选择有子节点的历史父节点，检查菜单并尝试单节点移除，然后查找“移除整条分支”。
- **期望**：单节点移除被隐藏或明确限制，父子关系保留，并提供专用整分支确认入口。
- **实际**：历史父保留 6 个子关系；选择历史父后仍没有受限说明或“移除整条分支”入口。
- **用户影响**：用户既看不到限制说明，也找不到整分支清理路径，无法理解或管理历史关系。
- **证据**：`/tmp/matou-independent-qa-round2-20260830-084703/groups/CLAUDE-FORK-r5/evidence/HISTORY-parent-inspect.json`

### E2E-LIFE-004
- **最短复现**：在含 2 子、3 孙和 1 个更深节点的父节点上选择“移除整条分支”；先取消，再确认并检查文件系统。
- **期望**：确认页准确列出 6 个后代及重要状态；取消保留全部；确认移除关系但保留所有工作树与未提交修改。
- **实际**：没有可发现的整条分支移除确认页，无法核对后代数量、取消和确认后的文件保留。
- **用户影响**：复杂历史分支没有可预期的批量清理流程，用户只能长期保留整棵无用关系树。
- **证据**：`/tmp/matou-independent-qa-round2-20260830-084703/groups/CLAUDE-FORK-r5/evidence/HISTORY-parent-inspect.json`

### E2E-LIFE-007
- **最短复现**：仅保留一个事项/画布，Shell 运行持续计数命令；点 macOS 关闭按钮，等待 2 秒，从 Dock/菜单重开并直接键入。
- **期望**：窗口仅隐藏；命令连续运行且不重复；重开回原画布并自动恢复原会话输入焦点。
- **实际**：关闭最后主窗口后真实计数器继续增长且重新打开回原画布，但原终端输入焦点=false，用户需再点一次。
- **用户影响**：后台任务虽持续，但窗口重开后键盘输入落不到终端，用户必须额外点击。
- **证据**：`/tmp/matou-independent-qa-round2-20260830-084703/groups/HIDE-r2/evidence/results.json`

### E2E-LIFE-008
- **最短复现**：在两张画布设置不同当前节点、横向位置和 DAG 80%/110% 缩放；通过应用菜单完整退出并用同 profile 重启。
- **期望**：工作空间、Tab、关系、排序、焦点、横向位置及每画布 DAG 缩放/平移全部恢复。
- **实际**：Tab、节点、顺序、焦点与横向位置恢复；两张画布 DAG 缩放均重置为 100%，观察位置恢复不完整。
- **用户影响**：重启后 DAG 观察尺度重置，用户需要重新寻找之前关注的关系区域。
- **证据**：`/tmp/matou-independent-qa-round2-20260830-084703/groups/CAN003-r2/evidence/results.json`

### E2E-ISO-001
- **最短复现**：完整退出 App，将真实 workspace 移到另一 `/tmp` 路径；重启观察限制提示，尝试选择新目录并新增 Shell。
- **期望**：历史与 DAG 可查看，输入/创建置灰并提示恢复目录；绑定新目录后关系保留且新 Shell `pwd` 指向新路径。
- **实际**：App 完整退出后真实移动 workspace，重启仍把旧路径会话当作可操作状态，创建入口保持启用且没有“选择移动后目录”入口；外部把目录移回后才恢复。
- **用户影响**：目录改名或移动后 App 仍假装会话可用，却没有恢复入口，用户无法在产品内修复工作空间。
- **证据**：`/tmp/matou-independent-qa-round2-20260830-084703/groups/WINDOW-LIST-r1/evidence/workspace-move-results.json`

### E2E-ISO-004
- **最短复现**：将节点 C 脱出为独立窗口；从主窗口 DAG 点击 C 唤起并聚焦；关闭独立窗后再次从 DAG 选择 C。
- **期望**：关系和状态不变；首次点击唤起窗口并聚焦；关闭后历史卡提供可执行的继续/重新打开入口。
- **实际**：节点可脱出、从 DAG 唤起独立窗并聚焦；关闭独立窗后 DAG 只显示历史卡，未提供可执行的继续/重新打开入口。
- **用户影响**：关闭独立窗口后节点变成只能观看的历史卡，用户无法从 DAG 继续该工作。
- **证据**：`/tmp/matou-independent-qa-round2-20260830-084703/groups/INDEPENDENT-r2/evidence/results.json`

### E2E-EDGE-001
- **最短复现**：完整退出后真实移走当前 cwd，再重启并新建画布；观察失败原因与重试，恢复目录后再次重试。
- **期望**：新画布保留失败卡与原因/重新创建；原画布可用；条件恢复后 Shell 就绪并自动聚焦。
- **实际**：真实 cwd 移走后重启没有显示 Shell 启动失败或重新创建操作，旧路径仍被展示为普通可用会话。
- **用户影响**：工作目录失效时界面仍显示普通会话，用户得不到原因或修复动作。
- **证据**：`/tmp/matou-independent-qa-round2-20260830-084703/groups/WINDOW-LIST-r1/evidence/workspace-move-results.json`

### E2E-EDGE-002
- **最短复现**：真实移走当前 cwd 后，在兄弟列表点击横向新增 Shell；恢复目录后尝试重试，再次触发并移除失败项。
- **期望**：队尾产生带原因、重试和移除的失败卡；现有焦点不变；重试成功仍在队尾。
- **实际**：真实 cwd 失效时横向创建仍保持启用，但没有在队尾形成包含失败原因、重试和移除的失败卡片。
- **用户影响**：新增失败没有可见失败卡和恢复入口，用户不知道操作是否生效，也无法清理失败项。
- **证据**：`/tmp/matou-independent-qa-round2-20260830-084703/groups/WINDOW-LIST-r1/evidence/workspace-move-results.json`

### E2E-EDGE-004
- **最短复现**：用真实 Claude Fork 连续建立至少 7 层，以第 4 层打开 DAG，分别向祖先和后代方向平移并点击远层。
- **期望**：默认仅三层完整显示，远层虚影随平移渐进变实，任一层均可点击跳转。
- **实际**：真实多级建立尝试未进入可观察的远层虚影/渐进加载阶段，用户从恢复后的子 Claude 无法继续 Fork，七层画布闭环未成立。
- **用户影响**：恢复后的 Claude 子节点缺少继续 Fork 能力，七层关系无法建立和浏览。
- **证据**：`/tmp/matou-independent-qa-round2-20260830-084703/environment/qa-multilevel-target.log`

### E2E-EDGE-007
- **最短复现**：在同一待关闭画布准备运行中 Claude、明确待输入 Shell、未提交草稿、滚动位置和焦点；触发关闭后取消。
- **期望**：取消后两个真实进程、输出、草稿、状态、焦点和滚动位置均与关闭前一致。
- **实际**：取消关闭可保留真实运行 Shell 和焦点，但“运行 Claude + 待输入 Shell + 草稿 + 滚动”组合现场仍缺少完整保留证据和统一反馈。
- **用户影响**：组合工作现场缺少完整保留反馈，用户取消关闭后仍可能丢失输入上下文或定位。
- **证据**：`/tmp/matou-independent-qa-round2-20260830-084703/groups/CLOSE-FINAL-r1/evidence/results.json`

### E2E-EDGE-008
- **最短复现**：从 200+ 字符真实路径打开含异常子节点的父会话；标准宽度观察标题并悬浮路径，再缩窄窗口。
- **期望**：会话名/数量/异常优先完整，路径截断且悬浮可看全值；窄窗创建入口收拢但错误和数量仍可见。
- **实际**：200+ 字符真实路径能在 Tab title 中读取，但窄窗只剩单卡，未同时验证异常徽章、子数量与创建入口按规定优先让位。
- **用户影响**：长路径和窄窗口组合下其余会话及关键状态被挤出，用户无法判断分支风险。
- **证据**：`/tmp/matou-independent-qa-round2-20260830-084703/groups/LONGPATH-r2/evidence/results.json`

### E2E-EDGE-009
- **最短复现**：全程键盘完成新建画布、新建兄弟、真实 Fork、返回父节点，最后从 DAG 选择主列表视野外节点并直接键入。
- **期望**：每个导航落点都自动获得终端输入焦点，键入只进入目标。
- **实际**：新画布、兄弟、Fork、明确返回均能聚焦；DAG 对视野外唯一输出节点搜索为 0，目标 textarea 未聚焦，键盘旅程在最后一步中断。
- **用户影响**：最后的 DAG 跳转没有定位或聚焦目标，纯键盘工作流在关键导航处中断。
- **证据**：`/tmp/matou-independent-qa-round2-20260830-084703/groups/DAG-JUMP-r1/evidence/results.json`

### E2E-EDGE-010
- **最短复现**：建立已退出父 P、运行子 C 和待输入子 D；在 DAG 观察状态，处理 D 并等 C 结束，再从 D 导航回 P。
- **期望**：父保持历史样式，子状态实时变化且连线不变；回到 P 显示历史详情而非活动终端。
- **实际**：历史父及活动子在 DAG 中保留；点击历史父回到子列表，而不是显示历史父详情，无法从子节点正确回看父历史。
- **用户影响**：返回历史父时被带到子列表，用户无法查看父会话的历史内容和上下文。
- **证据**：`/tmp/matou-independent-qa-round2-20260830-084703/groups/CLAUDE-FORK-r5/evidence/HISTORY-parent-inspect.json`

## 5. 阻塞项

- **E2E-REL-011**：真实小仓库 worktree 准备快于可重复注入只读失败的用户可见窗口；未使用延时钩子或内部接口。 证据：`/tmp/matou-independent-qa-round2-20260830-084703/environment/blockers.md`
- **E2E-REL-012**：真实父 provider 身份无法在亚秒准备窗口内定点失效而不改会话存储/替换 provider。 证据：`/tmp/matou-independent-qa-round2-20260830-084703/environment/blockers.md`
- **E2E-REL-019**：真实已认证账号没有 Team 队友子会话前置。 证据：`/tmp/matou-independent-qa-round2-20260830-084703/environment/blockers.md`
- **E2E-STA-007**：没有可逆且只影响一个隔离 Claude turn 的真实网络/账户故障条件。 证据：`/tmp/matou-independent-qa-round2-20260830-084703/environment/blockers.md`
- **E2E-STA-011**：没有只中断语义摘要、同时保留同一真实 PTY/runtime 的 OS 外部条件。 证据：`/tmp/matou-independent-qa-round2-20260830-084703/environment/blockers.md`
- **E2E-ISO-002**：真实隔离仓库的 worktree 准备在外部终止前完成，未稳定命中“准备中”状态。 证据：`/tmp/matou-independent-qa-round2-20260830-084703/environment/blockers.md`
- **E2E-ISO-005**：测试主机仅一台 1920×1080 显示器。 证据：`/tmp/matou-independent-qa-round2-20260830-084703/environment/blockers.md`
- **E2E-ISO-007**：没有只让 DAG BrowserWindow 创建失败且保持主窗口健康的真实系统条件。 证据：`/tmp/matou-independent-qa-round2-20260830-084703/environment/blockers.md`
- **E2E-EDGE-016**：只提供 commit 1d30ea7 的一个包体，没有可执行真实覆盖安装的后续包。 证据：`/tmp/matou-independent-qa-round2-20260830-084703/environment/blockers.md`

## 6. 本轮结果变化

- `E2E-NAV-008`：FAIL → PASS。
- `E2E-STA-006`：FAIL → PASS。
- `E2E-LIFE-010`：FAIL → PASS。
- `E2E-REL-010`：按最终显示名口径从规格阻塞转为真实 FAIL。
- `E2E-STA-003`：按 zsh 等价真实命令从夹具冲突转为真实 FAIL。
- `E2E-REL-018`：本轮发现共享工作树标记只更新新节点，转为 FAIL。
