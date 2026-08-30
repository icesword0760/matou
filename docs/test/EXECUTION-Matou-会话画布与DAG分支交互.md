# Matou 会话画布与 DAG 分支交互—第四轮独立黑盒执行记录

## 1. 验收结论

- **102 条全部逐条执行：PASS 63 / FAIL 33 / BLOCKED 6。**
- **产品决策建议：暂不进入正式发布验收。** 当前有 21 个 P0 失败，集中在多层 Fork、DAG 历史操作、Claude 权限状态、画布恢复、独立窗口、启动失败闭环。
- 本轮确认修复/可用的重点：二段边界右拉、窄窗让位、Claude Stop 后 <1s 回空闲、10 会话按稳定 ID 并发、真实新工作树未提交文件保留、父历史节点聚合子运行状态。

## 2. 环境与隔离

| 项目 | 值 |
|---|---|
| App | `.worktrees/session-dag-canvas/apps/desktop/release/mac-arm64/Matou.app` |
| Commit | `a43d42c1934200de81c159b68668d9fc102aff51` |
| 平台 | macOS arm64，单活动显示器 |
| 隔离根 | `/tmp/matou-independent-qa-round4-20260830-122509` |
| 隔离策略 | 每组独立 MATOU_DATA_DIR、Electron profile、HOME 与真实 Git 工作目录 |
| 外部能力 | 真实 PTY、文件系统、Git/worktree、真实 OAuth Claude Code |
| 操作边界 | 仅打包 App UI、键鼠/触摸板等价真实操作；未改产品实现、未改数据库、未用 mock/fake provider |

## 3. 用户旅程结论

- **创建与基础导航：** 默认 Shell、横向创建、窄窗、二段右拉和 DAG 搜索跳转主链路可用。
- **Claude 与关系：** 真实 Fork/当前与新工作树可用；多层继续 Fork、共同父兄弟命名、主动退出后重新进入 Claude 仍有缺口。
- **状态与排序：** Shell 与 Claude Stop 后状态可见；Claude 权限待输入、组合聚合、历史统计与授权排序仍不稳定。
- **生命周期与恢复：** 父历史保留、子状态更新、真实未提交工作树保留可用；叶历史移除、整分支移除、已关闭画布恢复、完整现场恢复仍失败。
- **隔离与边界：** 10 会话并发及真实覆盖升级基础数据目录连续；自定义工作空间入口、独立窗口关闭、Shell 启动失败卡片仍未闭环。

## 4. 逐条执行结果

| 用例 ID | 优先级 | 时间（UTC+8） | 结果 | 实际观察与用户影响 | 证据 |
|---|---|---:|---|---|---|
| E2E-CAN-001 | P0 | 12:27:14 | **PASS** | 首次进入显示默认事项=true、Shell=true、终端焦点=true。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/SHELL-CORE-r3b/evidence/E2E-CAN-001.png` |
| E2E-CAN-002 | P0 | 12:27:16 | **PASS** | Tab 1->2，直接出现 Shell，焦点=true，未出现类型/工作树弹框。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/SHELL-CORE-r3b/evidence/E2E-CAN-002.png` |
| E2E-CAN-003 | P0 | 12:27:04 | **FAIL** | A/B 节点=3/2，输出隔离=false；DAG 缩放 A 80%->80%，B 110%->110%；A 横向 0->0。 用户影响：画布现场与连续工作会丢失或串扰。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/CN6-r3/evidence/E2E-CAN-003.png` |
| E2E-CAN-004 | P1 | 12:27:04 | **PASS** | 重启前名称=["画布 A","性能验证"]；重启后=["画布 A","性能验证"]。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/CN6-r3/evidence/E2E-CAN-004.png` |
| E2E-CAN-005 | P0 | 12:27:04 | **PASS** | 切换 B→A 后焦点=88ef7199-de80-4a52-9ab9-9d7eff7ce25f，重启前/后焦点=88ef7199-de80-4a52-9ab9-9d7eff7ce25f/88ef7199-de80-4a52-9ab9-9d7eff7ce25f，A 节点集合保持。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/CN6-r3/evidence/E2E-CAN-005.png` |
| E2E-CAN-006 | P0 | 12:27:17 | **PASS** | 新增 Shell 后自动焦点=true，可直接接收键盘。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/SHELL-CORE-r3b/evidence/E2E-REL-001.png` |
| E2E-CAN-007 | P0 | 12:30:47 | **PASS** | 真实回答耗时 2ms；回答完成后终端输入焦点=true。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/CF4-r3/evidence/E2E-CAN-007.png` |
| E2E-CAN-008 | P0 | 12:27:14 | **PASS** | 可见 cwd=/tmp/matou-independent-qa-round4-20260830-122509/groups/SHELL-CORE-r3b/workspace-git；profile 和数据均位于隔离组目录。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/SHELL-CORE-r3b/evidence/E2E-CAN-008.png` |
| E2E-REL-001 | P0 | 12:27:17 | **PASS** | 根层节点 1->6；新增位队尾=true；焦点在新增=true。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/SHELL-CORE-r3b/evidence/E2E-REL-001.png` |
| E2E-REL-002 | P0 | 12:30:57 | **PASS** | 非根子列表共 6，Shell=5，Claude=1。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/CF4-r3/evidence/E2E-REL-002.png` |
| E2E-REL-003 | P0 | 12:30:57 | **PASS** | 同一父下混排 profiles=["claude-code","shell","shell","shell","shell","shell"]。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/CF4-r3/evidence/E2E-REL-002.png` |
| E2E-REL-004 | P0 | 12:30:47 | **PASS** | 同一 Session 180df234-2841-450d-8c4e-4c991e122181 原地切换为 profile=claude-code，首轮后 Fork 可用。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/CF4-r3/evidence/E2E-REL-004.png` |
| E2E-REL-005 | P0 | 12:30:47 | **PASS** | 首轮前 Fork disabled=true、提示=完成首轮对话后可创建分支；真实首轮完成后 disabled=false。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/CF4-r3/evidence/E2E-REL-005.png` |
| E2E-REL-006 | P0 | 12:30:55 | **PASS** | 真实 Fork 子会话 ID=e28722b1-eae1-4127-a75a-93fa534027bb，回复中继承父 token=true。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/CF4-r3/evidence/E2E-REL-006.png` |
| E2E-REL-007 | P0 | 12:30:55 | **PASS** | 父 cwd=/private/tmp/matou-independent-qa-round4-20260830-122509/groups/CF4-r3/workspace-git，子 cwd=/private/tmp/matou-independent-qa-round4-20260830-122509/groups/CF4-r3/workspace-git，相同=true，子共享标记=true。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/CF4-r3/evidence/E2E-REL-007.png` |
| E2E-REL-008 | P0 | 12:32:25 | **PASS** | 新子节点 cwd=/private/tmp/matou-independent-qa-round4-20260830-122509/groups/CF4-r3/matou-data/worktrees/30c2c3d5-a138-4daf-91a7-f4c3bb2e95b8/635947f4-e991-48b3-b620-1807ed3c984d，父 cwd=/private/tmp/matou-independent-qa-round4-20260830-122509/groups/CF4-r3/workspace-git，独立=true；git worktree list 包含=true。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/CF4-r3/evidence/E2E-REL-008-wt.png` |
| E2E-REL-009 | P0 | 12:30:25 | **PASS** | 非 Git 工作区新工作树 radio disabled=true，说明包含 Git=true；当前工作树选项仍可用。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/NG3-r3/evidence/E2E-REL-009.png` |
| E2E-REL-010 | P1 | 12:32:31 | **PASS** | 首次同名节点后直接子=8；第二次 modal保留=true、冲突提示=true、输入保留=same-visible-name；徽章 查看 8 个子会话->查看 8 个子会话；safe refs=["branch refs/heads/matou/same-visible-name-4cbc2257"]。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/CF4-r3/evidence/E2E-REL-010-wt.png` |
| E2E-REL-011 | P0 | 12:45:28 | **FAIL** | 有效Claude Fork=true；真实只读失败=true、原因=true；重试=1、恢复权限后成功=false。 用户影响：分支/会话关系无法可靠建立或恢复。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/CSTAT-r4/evidence/E2E-REL-011.png` |
| E2E-REL-012 | P0 | 12:59:37 | **BLOCKED** | 执行窗口内真实provider session身份保持有效；在不改凭据、不注入内部状态的约束下，没有真实可控的创建期身份失效事件。 本轮不计为产品通过或产品缺陷。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/BLOCKERS-r4/evidence/external-preconditions.txt` |
| E2E-REL-013 | P0 | 12:51:42 | **FAIL** | DAG Fork边=2；显示名同父分支1/2节点=1；节点ID唯一=1。 用户影响：分支/会话关系无法可靠建立或恢复。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/GRAPH-r4/evidence/E2E-REL-013-correct.png` |
| E2E-REL-014 | P0 | 12:48:36 | **FAIL** | 真实连续Fork链节点=2，ID唯一=true。 用户影响：分支/会话关系无法可靠建立或恢复。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/CMODE-r4/evidence/E2E-REL-014.png` |
| E2E-REL-015 | P0 | 12:47:23 | **PASS** | 主动/exit回Shell=true；“已退出”文案=false。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/CMODE-r4/evidence/E2E-REL-015.png` |
| E2E-REL-016 | P0 | 12:47:24 | **PASS** | 父转Shell后子入口=true，进入后直接子=1。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/CMODE-r4/evidence/E2E-REL-016.png` |
| E2E-REL-017 | P0 | 12:47:24 | **FAIL** | 同Session重新进入Claude=true；首轮后Fork可用=false；子关系=true。 用户影响：分支/会话关系无法可靠建立或恢复。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/CMODE-r4/evidence/E2E-REL-017.png` |
| E2E-REL-018 | P1 | 12:27:17 | **FAIL** | 同目录 6 个 Shell，共享标记分布=[false,false,false,false,false,false]。 用户影响：分支/会话关系无法可靠建立或恢复。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/SHELL-CORE-r3b/evidence/E2E-REL-018.png` |
| E2E-REL-019 | P1 | 12:30:26 | **BLOCKED** | 真实认证账号未提供 Team 队友子会话，无法从实际队友节点验证入口边界。 本轮不计为产品通过或产品缺陷。 | `/tmp/matou-independent-qa-round4-20260830-122509/environment/blockers.md` |
| E2E-NAV-001 | P0 | 12:30:57 | **FAIL** | 子层标题/徽章="Claude 的子会话\n6 个会话\n← 返回父会话\n⑂ Fork\n＋"。 用户影响：用户在多会话间定位或返回层级会受阻。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/CF4-r3/evidence/E2E-NAV-001.png` |
| E2E-NAV-002 | P0 | 12:30:57 | **PASS** | 进入子会话列表后只显示直接子=6，父节点不混入=true。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/CF4-r3/evidence/E2E-NAV-001.png` |
| E2E-NAV-003 | P0 | 12:30:59 | **PASS** | 子会话=6，visibleColumns=4，可横向距离=985。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/CF4-r3/evidence/E2E-NAV-003.png` |
| E2E-NAV-004 | P1 | 12:27:20 | **FAIL** | 目标宽度 悬浮前/中/移出=286.0/286.0/286.0，点击聚焦后=633.6。 用户影响：用户在多会话间定位或返回层级会受阻。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/SHELL-CORE-r3b/evidence/E2E-NAV-004.png` |
| E2E-NAV-005 | P0 | 12:30:57 | **PASS** | visibleColumns=4，从 654 一次右滑到 0，仍在 6 子列表=true。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/CF4-r3/evidence/E2E-NAV-005.png` |
| E2E-NAV-006 | P0 | 12:30:58 | **PASS** | 最左独立小幅右拉后仍在子列表=true。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/CF4-r3/evidence/E2E-NAV-006.png` |
| E2E-NAV-007 | P0 | 12:30:58 | **PASS** | 大幅第二右拉返回父=true，父输入焦点=true。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/CF4-r3/evidence/E2E-NAV-007.png` |
| E2E-NAV-008 | P0 | 12:31:00 | **PASS** | 明确返回入口=true，键盘 Enter 返回父=true。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/CF4-r3/evidence/E2E-NAV-008.png` |
| E2E-NAV-009 | P1 | 12:27:21 | **PASS** | 700px 下卡片=6，视野标记=1，宽度=123/299/123/123/123/123，各卡保留可识别状态=true。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/SHELL-CORE-r3b/evidence/E2E-NAV-009.png` |
| E2E-NAV-010 | P1 | 12:51:24 | **PASS** | 持续输出中小幅边界右拉仍在列表=true；独立大幅手势返回父=true。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/GRAPH-r4/evidence/E2E-NAV-010.png` |
| E2E-DAG-001 | P0 | 12:31:06 | **PASS** | Option+Tab 长按前后窗口=1/2，独立 DAG=true。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/CF4-r3/evidence/E2E-DAG-001.png` |
| E2E-DAG-002 | P0 | 12:27:15 | **PASS** | 短按 Tab 前后窗口数=1/1，终端仍可见=true。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/SHELL-CORE-r3b/evidence/E2E-DAG-002.png` |
| E2E-DAG-003 | P0 | 12:48:37 | **FAIL** | 当前多层DAG节点=3，虚影/远层=1，边=2。 用户影响：用户无法可靠理解或跳转复杂会话关系。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/CMODE-r4/evidence/E2E-DAG-003.png` |
| E2E-DAG-004 | P0 | 12:27:49 | **PASS** | 缩放 100%->90%->100%，聚焦当前按钮可用。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/SHELL-CORE-r3b/evidence/E2E-DAG-004.png` |
| E2E-DAG-005 | P1 | 12:27:49 | **FAIL** | 节点信息包含类型=true、路径=false、子会话=true。 用户影响：用户无法可靠理解或跳转复杂会话关系。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/SHELL-CORE-r3b/evidence/E2E-DAG-005.png` |
| E2E-DAG-006 | P1 | 12:27:52 | **PASS** | 最近输出刷新到 LIVE_6=true，画布 transform 未被推移=true。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/SHELL-CORE-r3b/evidence/E2E-DAG-006.png` |
| E2E-DAG-007 | P0 | 12:31:04 | **PASS** | 目标初始 inViewport=false；真实搜索结果项=1；点击后 inViewport=true、focused=true。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/CF4-r3/evidence/E2E-DAG-007.png` |
| E2E-DAG-008 | P0 | 12:31:05 | **PASS** | 悬浮/窄窗/恢复后 inViewport=true/true/true，输入焦点=true。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/CF4-r3/evidence/E2E-DAG-008.png` |
| E2E-DAG-009 | P0 | 12:31:08 | **FAIL** | 父搜索结果=1，点击后父可见=false，当前可见节点=6。 用户影响：用户无法可靠理解或跳转复杂会话关系。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/CF4-r3/evidence/E2E-DAG-009.png` |
| E2E-DAG-010 | P1 | 12:30:08 | **FAIL** | 真实会话=100、DAG节点=4，首次打开=490ms；30次缩放/聚焦=true；远端搜索结果=1、跳转焦点/视野=true/true；10个真实长命令启动，运行时DAG重开=496ms、可缩放=true。 用户影响：用户无法可靠理解或跳转复杂会话关系。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/BIG-r3/evidence/E2E-DAG-010.png` |
| E2E-STA-001 | P0 | 12:27:25 | **PASS** | 提交后运行中=true，完成后空闲=true。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/SHELL-CORE-r3b/evidence/E2E-STA-001.png` |
| E2E-STA-002 | P0 | 12:27:31 | **PASS** | 退出码23后错误可见=true，同 Shell 后续输出恢复=true。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/SHELL-CORE-r3b/evidence/E2E-STA-002.png` |
| E2E-STA-003 | P0 | 12:27:33 | **PASS** | 明确 prompt 后 DAG 待输入=true，输入后完成=true。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/SHELL-CORE-r3b/evidence/E2E-STA-003.png` |
| E2E-STA-004 | P1 | 12:27:35 | **PASS** | 无 prompt stdin 等待：运行中=true、待输入=false。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/SHELL-CORE-r3b/evidence/E2E-STA-004.png` |
| E2E-STA-005 | P0 | 12:41:54 | **PASS** | 提交后运行中=true；真实 Stop/Fork恢复可用=true；Stop后空闲=true，可见延迟=801ms。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/CSTAT-r4/evidence/E2E-STA-005.png` |
| E2E-STA-006 | P0 | 12:44:28 | **FAIL** | 权限提示=true；DAG待输入=false；允许后文件=false、Stop=false、空闲=true。 用户影响：状态与真实进程不一致，可能误判是否需处理。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/CSTAT-r4/evidence/E2E-STA-006.png` |
| E2E-STA-007 | P0 | 12:59:37 | **BLOCKED** | 执行窗口内真实Claude服务持续可用，未出现自然provider异常；主动网络故障注入不满足真实外部行为约束。 本轮不计为产品通过或产品缺陷。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/BLOCKERS-r4/evidence/external-preconditions.txt` |
| E2E-STA-008 | P0 | 12:51:16 | **FAIL** | 父徽章=Claude 2 · Shell 3；运行中 3 · 待输入 0 · 空闲 1；错误 1 · 中断 0。 用户影响：状态与真实进程不一致，可能误判是否需处理。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/GRAPH-r4/evidence/E2E-STA-008.png` |
| E2E-STA-009 | P0 | 12:45:58 | **FAIL** | 父徽章 aria=查看 1 个子会话，title=Claude 1 · Shell 0；运行中 0 · 待输入 0 · 空闲 0；错误 1 · 中断 0；活动横向节点=1，历史差额=0；DAG status-exited=0，文本含历史=false；运行中=0。 用户影响：状态与真实进程不一致，可能误判是否需处理。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/REC2-r3/evidence/E2E-STA-009-final.png` |
| E2E-STA-010 | P0 | 12:27:36 | **FAIL** | Shell A 真实失败后 Shell B 独立输出 OTHER_ALIVE。 用户影响：状态与真实进程不一致，可能误判是否需处理。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/SHELL-CORE-r3b/evidence/E2E-STA-010.png` |
| E2E-STA-011 | P1 | 12:59:37 | **BLOCKED** | 执行窗口未发生自然摘要流中断；内部channel终止或接口注入属于被禁止的测试手段。 本轮不计为产品通过或产品缺陷。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/BLOCKERS-r4/evidence/external-preconditions.txt` |
| E2E-STA-012 | P1 | 12:27:27 | **PASS** | 真实命令到 DAG 运行中可见延迟=765ms。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/SHELL-CORE-r3b/evidence/E2E-STA-012.png` |
| E2E-SORT-001 | P0 | 12:27:37 | **PASS** | 草稿不排序=true；提交后移首=true；焦点=true。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/SHELL-CORE-r3b/evidence/E2E-SORT-001.png` |
| E2E-SORT-002 | P0 | 12:39:01 | **PASS** | 末尾 C Ctrl+C 后移首=true；等待 stdin 的 B 完成输入后移首=true；三个稳定节点仍唯一=true。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/SORT2-r3/evidence/E2E-SORT-002.png` |
| E2E-SORT-003 | P0 | 12:44:28 | **PASS** | Claude发送消息后移首=true。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/CSTAT-r4/evidence/E2E-SORT-003.png` |
| E2E-SORT-004 | P0 | 12:44:28 | **FAIL** | 其他Shell先移首后，真实权限允许将Claude移首=false。 用户影响：最近操作顺序不可靠，增加找回会话成本。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/CSTAT-r4/evidence/E2E-SORT-003.png` |
| E2E-SORT-005 | P0 | 12:27:41 | **PASS** | 横向滚动、打开关闭 DAG 前后顺序稳定=true。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/SHELL-CORE-r3b/evidence/E2E-SORT-005.png` |
| E2E-SORT-006 | P0 | 12:27:43 | **PASS** | 后台完成前后顺序不变=true。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/SHELL-CORE-r3b/evidence/E2E-SORT-006.png` |
| E2E-SORT-007 | P0 | 12:27:44 | **PASS** | 新节点位队尾=true，总数 6->7。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/SHELL-CORE-r3b/evidence/E2E-SORT-007.png` |
| E2E-SORT-008 | P0 | 12:27:45 | **FAIL** | 同秒连续提交后顺序前两位=702e7dd0-c0bf-45cb-9c63-0c7fd6792a93,ac908f48-fadd-481a-bda7-9d26f19d89cc，预期 B/A。 用户影响：最近操作顺序不可靠，增加找回会话成本。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/SHELL-CORE-r3b/evidence/E2E-SORT-008.png` |
| E2E-SORT-009 | P0 | 12:51:21 | **PASS** | 交互重排后关系边=5，路径唯一=true。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/GRAPH-r4/evidence/E2E-EDGE-012-light.png` |
| E2E-LIFE-001 | P0 | 12:52:31 | **PASS** | 父卡定位=1；确认提示5子=true；取消保留=true；确认后活动=false；历史节点=1、边=5。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/GRAPH-r4/evidence/E2E-LIFE-001.png` |
| E2E-LIFE-002 | P1 | 12:49:00 | **FAIL** | 活动叶搜索=1；结束后历史结果=1；单节点移除入口=true；确认无后代=false；取消保留=true；确认后搜索消失=false；工作目录保留=true。 用户影响：结束、恢复或清理工作现场存在丢失/残留风险。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/CMODE-r4/evidence/E2E-LIFE-002-leaf.png` |
| E2E-LIFE-003 | P1 | 12:52:31 | **PASS** | 父历史节点=1；子关系边=5。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/GRAPH-r4/evidence/E2E-LIFE-001.png` |
| E2E-LIFE-004 | P1 | 12:53:48 | **FAIL** | 历史父节点=1；真实点击位置命中=，窗口标题层遮挡节点，未能进入整分支移除确认。 用户影响：结束、恢复或清理工作现场存在丢失/残留风险。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/GRAPH-r4/evidence/E2E-LIFE-004.png` |
| E2E-LIFE-005 | P0 | 12:33:34 | **PASS** | 确认框=true，取消后运行输出继续=true，画布/三节点仍在=true。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/LIF3-r3/evidence/close-modal.png`<br>`/tmp/matou-independent-qa-round4-20260830-122509/groups/LIF3-r3/evidence/E2E-LIFE-005.png` |
| E2E-LIFE-006 | P1 | 12:33:37 | **FAIL** | 确认关闭后Tab消失=true；已关闭入口=true；恢复按钮=0；恢复Tab唯一=false；节点数=1。 用户影响：结束、恢复或清理工作现场存在丢失/残留风险。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/LIF3-r3/evidence/E2E-LIFE-006.png` |
| E2E-LIFE-007 | P0 | 12:33:54 | **PASS** | 关闭窗口后AX窗口数=0；计数 3→隐藏2秒8→重新打开10；原会话可见=true、焦点=true。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/OSL1-r3/evidence/E2E-LIFE-007.png` |
| E2E-LIFE-008 | P0 | 12:27:04 | **FAIL** | 完整重启后 Tab=2、当前画布索引=0、节点隔离=false、缩放 A/B=80%/110%。 用户影响：结束、恢复或清理工作现场存在丢失/残留风险。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/CN6-r3/evidence/E2E-LIFE-008.png` |
| E2E-LIFE-009 | P0 | 12:34:30 | **PASS** | 真实 Cmd+Q 进程退出=true；退出前3行，退出等待后3，重启继续等待3，手动重提后5；中断且未自动重跑提示=true。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/OSL4-r3/evidence/E2E-LIFE-009.png` |
| E2E-LIFE-010 | P0 | 12:33:12 | **PASS** | 移除真实 transcript 后 profile=shell、失败提示=true、重试入口=1；Shell可用=true；子关系=true；缺失时重试仍失败=true；恢复文件后Claude=true、关系入口=true。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/REC2-r3/evidence/E2E-LIFE-010.png` |
| E2E-ISO-001 | P0 | 12:54:46 | **FAIL** | 点击后未出现 macOS 目录选择窗口（System Events 返回窗口数0），侧栏仍仅默认工作空间，因此无法继续移动/重连闭环。 用户影响：故障或窗口隔离边界不闭环。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/CHOOSER2-r4/evidence/chooser.png` |
| E2E-ISO-002 | P0 | 12:59:15 | **PASS** | 有效新工作树入口=true、提交后立即真实Cmd+Q=true；重启重试/失败/准备提示=false；worktree增量=1。 操作在退出前已原子完成，未观察到准备中中断窗口。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/CF4-r3/evidence/E2E-ISO-002.png` |
| E2E-ISO-003 | P0 | 12:33:13 | **PASS** | 仅目标恢复失败且恢复；另一画布Tab数=2、真实Shell输入=true；目标子关系=true。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/REC2-r3/evidence/E2E-ISO-003.png` |
| E2E-ISO-004 | P1 | 12:37:14 | **FAIL** | 独立窗口创建页数=2、初始焦点=true；DAG搜索=1，点击后窗口前台=true/输入焦点=true；真实 Cmd+W 关闭=false；DAG历史结果=1、继续/重开入口=false。 用户影响：故障或窗口隔离边界不闭环。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/IND3-r3/evidence/E2E-ISO-004-front.png`<br>`/tmp/matou-independent-qa-round4-20260830-122509/groups/IND3-r3/evidence/E2E-ISO-004.png` |
| E2E-ISO-005 | P1 | 12:59:37 | **BLOCKED** | 当前主机仅检测到单一内建显示器，缺少第二物理显示器来验证浮层跟随当前屏幕。 本轮不计为产品通过或产品缺陷。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/BLOCKERS-r4/evidence/display-topology.txt` |
| E2E-ISO-006 | P0 | 12:33:34 | **FAIL** | 白/深主题切换成功且 Shell 焦点、运行、待输入可区分；当前真实场景中未同时呈现 Claude、Fork 边、历史及虚影节点，无法满足 BASE-STATES 全项视觉断言。 用户影响：故障或窗口隔离边界不闭环。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/LIF3-r3/evidence/theme-light.png`<br>`/tmp/matou-independent-qa-round4-20260830-122509/groups/LIF3-r3/evidence/theme-dark.png` |
| E2E-ISO-007 | P1 | 12:59:37 | **BLOCKED** | 所有真实DAG启动均成功；未发生自然DAG异常，禁止通过内部接口/实现篡改制造失败。 本轮不计为产品通过或产品缺陷。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/BLOCKERS-r4/evidence/external-preconditions.txt` |
| E2E-ISO-008 | P1 | 12:40:24 | **PASS** | 稳定ID数=10；按ID提交后全部唯一标记=[true,true,true,true,true,true,true,true,true,true]；DAG仍响应=true。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/ISO8-r4/evidence/E2E-ISO-008.png` |
| E2E-EDGE-001 | P0 | 12:39:31 | **FAIL** | 真实不可执行 SHELL 首画布失败提示=false、重试=0；恢复执行权限后成功=true。 用户影响：边界场景会破坏核心工作连续性。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/FAILSHELL1-r4/evidence/E2E-EDGE-001.png` |
| E2E-EDGE-002 | P0 | 12:39:36 | **FAIL** | 现有会话=1；真实 SHELL 在命令接受后失效，失败提示=false、重试=0；恢复权限后活动卡=2、成功=true。 用户影响：边界场景会破坏核心工作连续性。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/FAILSHELL2-r4/evidence/E2E-EDGE-002.png` |
| E2E-EDGE-003 | P1 | 12:27:15 | **PASS** | 单节点 DAG 节点数=1，子会话0=true，连线=2。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/SHELL-CORE-r3b/evidence/E2E-EDGE-003.png` |
| E2E-EDGE-004 | P1 | 12:48:37 | **FAIL** | 超过三层节点=3；平移后缩放可用=true。 用户影响：边界场景会破坏核心工作连续性。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/CMODE-r4/evidence/E2E-EDGE-004.png` |
| E2E-EDGE-005 | P0 | 12:55:21 | **PASS** | DAG定位新工作树节点=1；真实未提交=true；结束会话后文件=true、仍未提交=true、worktree保留=true。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/CF4-r3/evidence/E2E-EDGE-005.png` |
| E2E-EDGE-006 | P0 | 12:34:51 | **PASS** | 主动 /exit 回 Shell=true；正常退出重启后 profile=shell；无恢复/错误/Claude启动输出=true；Shell直接输入=true。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/CEX-r3/evidence/E2E-EDGE-006.png` |
| E2E-EDGE-007 | P1 | 12:33:34 | **FAIL** | 取消后草稿=false，顺序=true，焦点=true，真实进程继续=true。 用户影响：边界场景会破坏核心工作连续性。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/LIF3-r3/evidence/E2E-LIFE-005.png` |
| E2E-EDGE-008 | P1 | 12:40:51 | **PASS** | 完整title=284字符，侧栏/Tab截断=true/true；650px下节点=5、数量可读=true。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/LONG-r4/evidence/E2E-EDGE-008.png` |
| E2E-EDGE-009 | P0 | 12:36:26 | **FAIL** | 新画布焦点=true；新兄弟=true；Fork可用=0、子节点就绪/焦点=true/false；返回父焦点=true；DAG搜索结果=0、跳转焦点/视野=false/true。 用户影响：边界场景会破坏核心工作连续性。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/REC2-r3/evidence/E2E-EDGE-009.png` |
| E2E-EDGE-010 | P0 | 12:52:34 | **PASS** | 历史父存在=true；真实子Shell=ba01d7c7-466d-41a7-ad79-32c173712991; 子运行后历史父聚合运行中=true。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/GRAPH-r4/evidence/E2E-EDGE-010.png` |
| E2E-EDGE-011 | P1 | 12:28:34 | **PASS** | DAG节点=1；ANSI/alternate控制字符泄漏=false；中文/emoji/最终摘要可读=true；卡片宽=260px。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/E11-r3/evidence/E2E-EDGE-011.png` |
| E2E-EDGE-012 | P0 | 12:51:21 | **PASS** | Fork边=2，普通边=3；白/深主题均截图。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/GRAPH-r4/evidence/E2E-EDGE-012-light.png`<br>`/tmp/matou-independent-qa-round4-20260830-122509/groups/GRAPH-r4/evidence/E2E-EDGE-012-dark.png` |
| E2E-EDGE-013 | P1 | 12:33:40 | **FAIL** | 重复恢复后活动同名Tab数=0；再次关闭并重启后已关闭列表同名记录数=1。 用户影响：边界场景会破坏核心工作连续性。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/LIF3-r3/evidence/E2E-EDGE-013.png` |
| E2E-EDGE-014 | P1 | 12:33:34 | **FAIL** | 主题背景 rgb(247, 248, 250)→rgba(0, 0, 0, 0)→rgb(247, 248, 250)；草稿保留=false；顺序保持=true。 用户影响：边界场景会破坏核心工作连续性。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/LIF3-r3/evidence/theme-dark.png`<br>`/tmp/matou-independent-qa-round4-20260830-122509/groups/LIF3-r3/evidence/theme-light.png` |
| E2E-EDGE-015 | P0 | 12:51:21 | **PASS** | 五个直接子，DAG边=5，边唯一=true。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/GRAPH-r4/evidence/E2E-EDGE-012-light.png` |
| E2E-EDGE-016 | P0 | 13:01:46 | **FAIL** | 446195a→a43d42c：自定义主画布名=true；已关闭画布入口=true；DAG节点/边/历史=3/2/0；worktree数=2、归属清单不变=true；数据目录不变=true。 用户影响：边界场景会破坏核心工作连续性。 | `/tmp/matou-independent-qa-round4-20260830-122509/groups/UPGRADE2-r4/evidence/E2E-EDGE-016.png` |

## 5. 失败项最短真实复现

### E2E-CAN-003 [P0]
- **最短复现：** 两画布设置不同焦点、横向位置和 80%/110% DAG 缩放后完整重启。
- **实际：** A/B 节点=3/2，输出隔离=false；DAG 缩放 A 80%->80%，B 110%->110%；A 横向 0->0。
- **期望：** 按原用例与 PRD 验收标准完成闭环。
- **用户影响：** 画布现场与连续工作会丢失或串扰。
- **证据：** `/tmp/matou-independent-qa-round4-20260830-122509/groups/CN6-r3/evidence/E2E-CAN-003.png`

### E2E-REL-011 [P0]
- **最短复现：** 真实chmod worktree根为只读，创建新工作树Fork，恢复权限后重试。
- **实际：** 有效Claude Fork=true；真实只读失败=true、原因=true；重试=1、恢复权限后成功=false。
- **期望：** 按原用例与 PRD 验收标准完成闭环。
- **用户影响：** 分支/会话关系无法可靠建立或恢复。
- **证据：** `/tmp/matou-independent-qa-round4-20260830-122509/groups/CSTAT-r4/evidence/E2E-REL-011.png`

### E2E-REL-013 [P0]
- **最短复现：** 真实有效父Claude创建两个分支后，从DAG实际节点ID与Fork边核验兄弟关系。
- **实际：** DAG Fork边=2；显示名同父分支1/2节点=1；节点ID唯一=1。
- **期望：** 按原用例与 PRD 验收标准完成闭环。
- **用户影响：** 分支/会话关系无法可靠建立或恢复。
- **证据：** `/tmp/matou-independent-qa-round4-20260830-122509/groups/GRAPH-r4/evidence/E2E-REL-013-correct.png`

### E2E-REL-014 [P0]
- **最短复现：** 从已有父子真实Claude沿直接子继续Fork至第4层。
- **实际：** 真实连续Fork链节点=2，ID唯一=true。
- **期望：** 按原用例与 PRD 验收标准完成闭环。
- **用户影响：** 分支/会话关系无法可靠建立或恢复。
- **证据：** `/tmp/matou-independent-qa-round4-20260830-122509/groups/CMODE-r4/evidence/E2E-REL-014.png`

### E2E-REL-017 [P0]
- **最短复现：** 同一Shell重新启动真实Claude并完成首轮。
- **实际：** 同Session重新进入Claude=true；首轮后Fork可用=false；子关系=true。
- **期望：** 按原用例与 PRD 验收标准完成闭环。
- **用户影响：** 分支/会话关系无法可靠建立或恢复。
- **证据：** `/tmp/matou-independent-qa-round4-20260830-122509/groups/CMODE-r4/evidence/E2E-REL-017.png`

### E2E-REL-018 [P1]
- **最短复现：** 从单节点横向新增同目录 Shell，观察全部节点标记。
- **实际：** 同目录 6 个 Shell，共享标记分布=[false,false,false,false,false,false]。
- **期望：** 按原用例与 PRD 验收标准完成闭环。
- **用户影响：** 分支/会话关系无法可靠建立或恢复。
- **证据：** `/tmp/matou-independent-qa-round4-20260830-122509/groups/SHELL-CORE-r3b/evidence/E2E-REL-018.png`

### E2E-NAV-001 [P0]
- **最短复现：** 父节点拥有 1 Claude + 5 Shell 后检查聚合徽章。
- **实际：** 子层标题/徽章="Claude 的子会话\n6 个会话\n← 返回父会话\n⑂ Fork\n＋"。
- **期望：** 按原用例与 PRD 验收标准完成闭环。
- **用户影响：** 用户在多会话间定位或返回层级会受阻。
- **证据：** `/tmp/matou-independent-qa-round4-20260830-122509/groups/CF4-r3/evidence/E2E-NAV-001.png`

### E2E-NAV-004 [P1]
- **最短复现：** 四卡以上布局悬浮第三卡、移出、点击第二卡。
- **实际：** 目标宽度 悬浮前/中/移出=286.0/286.0/286.0，点击聚焦后=633.6。
- **期望：** 按原用例与 PRD 验收标准完成闭环。
- **用户影响：** 用户在多会话间定位或返回层级会受阻。
- **证据：** `/tmp/matou-independent-qa-round4-20260830-122509/groups/SHELL-CORE-r3b/evidence/E2E-NAV-004.png`

### E2E-DAG-003 [P0]
- **最短复现：** 当前位于4+层节点，打开DAG检查默认三层与远层虚影。
- **实际：** 当前多层DAG节点=3，虚影/远层=1，边=2。
- **期望：** 按原用例与 PRD 验收标准完成闭环。
- **用户影响：** 用户无法可靠理解或跳转复杂会话关系。
- **证据：** `/tmp/matou-independent-qa-round4-20260830-122509/groups/CMODE-r4/evidence/E2E-DAG-003.png`

### E2E-DAG-005 [P1]
- **最短复现：** 检查 DAG 节点信息与缩放让位。
- **实际：** 节点信息包含类型=true、路径=false、子会话=true。
- **期望：** 按原用例与 PRD 验收标准完成闭环。
- **用户影响：** 用户无法可靠理解或跳转复杂会话关系。
- **证据：** `/tmp/matou-independent-qa-round4-20260830-122509/groups/SHELL-CORE-r3b/evidence/E2E-DAG-005.png`

### E2E-DAG-009 [P0]
- **最短复现：** 从子列表 DAG 点击父节点。
- **实际：** 父搜索结果=1，点击后父可见=false，当前可见节点=6。
- **期望：** 按原用例与 PRD 验收标准完成闭环。
- **用户影响：** 用户无法可靠理解或跳转复杂会话关系。
- **证据：** `/tmp/matou-independent-qa-round4-20260830-122509/groups/CF4-r3/evidence/E2E-DAG-009.png`

### E2E-DAG-010 [P1]
- **最短复现：** 真实创建 100 个 Shell，DAG 平移/缩放/聚焦各10次，远端跳转，10会话并行输出。
- **实际：** 真实会话=100、DAG节点=4，首次打开=490ms；30次缩放/聚焦=true；远端搜索结果=1、跳转焦点/视野=true/true；10个真实长命令启动，运行时DAG重开=496ms、可缩放=true。
- **期望：** 按原用例与 PRD 验收标准完成闭环。
- **用户影响：** 用户无法可靠理解或跳转复杂会话关系。
- **证据：** `/tmp/matou-independent-qa-round4-20260830-122509/groups/BIG-r3/evidence/E2E-DAG-010.png`

### E2E-STA-006 [P0]
- **最短复现：** 真实 Claude Write 权限请求、允许并等待Stop。
- **实际：** 权限提示=true；DAG待输入=false；允许后文件=false、Stop=false、空闲=true。
- **期望：** 按原用例与 PRD 验收标准完成闭环。
- **用户影响：** 状态与真实进程不一致，可能误判是否需处理。
- **证据：** `/tmp/matou-independent-qa-round4-20260830-122509/groups/CSTAT-r4/evidence/E2E-STA-006.png`

### E2E-STA-008 [P0]
- **最短复现：** 2 Claude+3 Shell，分别制造运行中与错误并观察聚合优先级。
- **实际：** 父徽章=Claude 2 · Shell 3；运行中 3 · 待输入 0 · 空闲 1；错误 1 · 中断 0。
- **期望：** 按原用例与 PRD 验收标准完成闭环。
- **用户影响：** 状态与真实进程不一致，可能误判是否需处理。
- **证据：** `/tmp/matou-independent-qa-round4-20260830-122509/groups/GRAPH-r4/evidence/E2E-STA-008.png`

### E2E-STA-009 [P0]
- **最短复现：** 真实结束一个子节点后检查总量、运行统计、活动列表与 DAG 历史样式。
- **实际：** 父徽章 aria=查看 1 个子会话，title=Claude 1 · Shell 0；运行中 0 · 待输入 0 · 空闲 0；错误 1 · 中断 0；活动横向节点=1，历史差额=0；DAG status-exited=0，文本含历史=false；运行中=0。
- **期望：** 按原用例与 PRD 验收标准完成闭环。
- **用户影响：** 状态与真实进程不一致，可能误判是否需处理。
- **证据：** `/tmp/matou-independent-qa-round4-20260830-122509/groups/REC2-r3/evidence/E2E-STA-009-final.png`

### E2E-STA-010 [P0]
- **最短复现：** 一个会话失败后在另一会话继续命令。
- **实际：** Shell A 真实失败后 Shell B 独立输出 OTHER_ALIVE。
- **期望：** 按原用例与 PRD 验收标准完成闭环。
- **用户影响：** 状态与真实进程不一致，可能误判是否需处理。
- **证据：** `/tmp/matou-independent-qa-round4-20260830-122509/groups/SHELL-CORE-r3b/evidence/E2E-STA-010.png`

### E2E-SORT-004 [P0]
- **最短复现：** 真实权限允许后检查交互排序。
- **实际：** 其他Shell先移首后，真实权限允许将Claude移首=false。
- **期望：** 按原用例与 PRD 验收标准完成闭环。
- **用户影响：** 最近操作顺序不可靠，增加找回会话成本。
- **证据：** `/tmp/matou-independent-qa-round4-20260830-122509/groups/CSTAT-r4/evidence/E2E-SORT-003.png`

### E2E-SORT-008 [P0]
- **最短复现：** 同一秒内按 A 后 B 连续提交。
- **实际：** 同秒连续提交后顺序前两位=702e7dd0-c0bf-45cb-9c63-0c7fd6792a93,ac908f48-fadd-481a-bda7-9d26f19d89cc，预期 B/A。
- **期望：** 按原用例与 PRD 验收标准完成闭环。
- **用户影响：** 最近操作顺序不可靠，增加找回会话成本。
- **证据：** `/tmp/matou-independent-qa-round4-20260830-122509/groups/SHELL-CORE-r3b/evidence/E2E-SORT-008.png`

### E2E-LIFE-002 [P1]
- **最短复现：** 结束真实无子叶节点，移除流程先取消再确认。
- **实际：** 活动叶搜索=1；结束后历史结果=1；单节点移除入口=true；确认无后代=false；取消保留=true；确认后搜索消失=false；工作目录保留=true。
- **期望：** 按原用例与 PRD 验收标准完成闭环。
- **用户影响：** 结束、恢复或清理工作现场存在丢失/残留风险。
- **证据：** `/tmp/matou-independent-qa-round4-20260830-122509/groups/CMODE-r4/evidence/E2E-LIFE-002-leaf.png`

### E2E-LIFE-004 [P1]
- **最短复现：** 历史父节点执行真实鼠标点击；目标被DAG窗口标题层截获。
- **实际：** 历史父节点=1；真实点击位置命中=，窗口标题层遮挡节点，未能进入整分支移除确认。
- **期望：** 按原用例与 PRD 验收标准完成闭环。
- **用户影响：** 结束、恢复或清理工作现场存在丢失/残留风险。
- **证据：** `/tmp/matou-independent-qa-round4-20260830-122509/groups/GRAPH-r4/evidence/E2E-LIFE-004.png`

### E2E-LIFE-006 [P1]
- **最短复现：** 确认关闭非最后画布，从事项已关闭列表恢复。
- **实际：** 确认关闭后Tab消失=true；已关闭入口=true；恢复按钮=0；恢复Tab唯一=false；节点数=1。
- **期望：** 按原用例与 PRD 验收标准完成闭环。
- **用户影响：** 结束、恢复或清理工作现场存在丢失/残留风险。
- **证据：** `/tmp/matou-independent-qa-round4-20260830-122509/groups/LIF3-r3/evidence/E2E-LIFE-006.png`

### E2E-LIFE-008 [P0]
- **最短复现：** 多画布现场通过应用菜单完整退出并重启。
- **实际：** 完整重启后 Tab=2、当前画布索引=0、节点隔离=false、缩放 A/B=80%/110%。
- **期望：** 按原用例与 PRD 验收标准完成闭环。
- **用户影响：** 结束、恢复或清理工作现场存在丢失/残留风险。
- **证据：** `/tmp/matou-independent-qa-round4-20260830-122509/groups/CN6-r3/evidence/E2E-LIFE-008.png`

### E2E-ISO-001 [P0]
- **最短复现：** 在全新隔离 profile 通过真实“新增工作空间”按钮发起自定义工作空间选择。
- **实际：** 点击后未出现 macOS 目录选择窗口（System Events 返回窗口数0），侧栏仍仅默认工作空间，因此无法继续移动/重连闭环。
- **期望：** 按原用例与 PRD 验收标准完成闭环。
- **用户影响：** 故障或窗口隔离边界不闭环。
- **证据：** `/tmp/matou-independent-qa-round4-20260830-122509/groups/CHOOSER2-r4/evidence/chooser.png`

### E2E-ISO-004 [P1]
- **最短复现：** 通过卡片真实右键独立窗口，主窗口 DAG 搜索跳转，关闭独立窗口后再次 DAG 跳转。
- **实际：** 独立窗口创建页数=2、初始焦点=true；DAG搜索=1，点击后窗口前台=true/输入焦点=true；真实 Cmd+W 关闭=false；DAG历史结果=1、继续/重开入口=false。
- **期望：** 按原用例与 PRD 验收标准完成闭环。
- **用户影响：** 故障或窗口隔离边界不闭环。
- **证据：** `/tmp/matou-independent-qa-round4-20260830-122509/groups/IND3-r3/evidence/E2E-ISO-004-front.png`、`/tmp/matou-independent-qa-round4-20260830-122509/groups/IND3-r3/evidence/E2E-ISO-004.png`

### E2E-ISO-006 [P0]
- **最短复现：** 在实际状态画布切换白色/深色并截图逐项检查。
- **实际：** 白/深主题切换成功且 Shell 焦点、运行、待输入可区分；当前真实场景中未同时呈现 Claude、Fork 边、历史及虚影节点，无法满足 BASE-STATES 全项视觉断言。
- **期望：** 按原用例与 PRD 验收标准完成闭环。
- **用户影响：** 故障或窗口隔离边界不闭环。
- **证据：** `/tmp/matou-independent-qa-round4-20260830-122509/groups/LIF3-r3/evidence/theme-light.png`、`/tmp/matou-independent-qa-round4-20260830-122509/groups/LIF3-r3/evidence/theme-dark.png`

### E2E-EDGE-001 [P0]
- **最短复现：** 以真实不可执行 SHELL 启动全新隔离 App，恢复权限后点击重试。
- **实际：** 真实不可执行 SHELL 首画布失败提示=false、重试=0；恢复执行权限后成功=true。
- **期望：** 按原用例与 PRD 验收标准完成闭环。
- **用户影响：** 边界场景会破坏核心工作连续性。
- **证据：** `/tmp/matou-independent-qa-round4-20260830-122509/groups/FAILSHELL1-r4/evidence/E2E-EDGE-001.png`

### E2E-EDGE-002 [P0]
- **最短复现：** 先用真实可执行 wrapper 启动，随后 chmod 000 并横向新增，再恢复权限重试。
- **实际：** 现有会话=1；真实 SHELL 在命令接受后失效，失败提示=false、重试=0；恢复权限后活动卡=2、成功=true。
- **期望：** 按原用例与 PRD 验收标准完成闭环。
- **用户影响：** 边界场景会破坏核心工作连续性。
- **证据：** `/tmp/matou-independent-qa-round4-20260830-122509/groups/FAILSHELL2-r4/evidence/E2E-EDGE-002.png`

### E2E-EDGE-004 [P1]
- **最短复现：** 真实四层以上关系DAG平移和缩放。
- **实际：** 超过三层节点=3；平移后缩放可用=true。
- **期望：** 按原用例与 PRD 验收标准完成闭环。
- **用户影响：** 边界场景会破坏核心工作连续性。
- **证据：** `/tmp/matou-independent-qa-round4-20260830-122509/groups/CMODE-r4/evidence/E2E-EDGE-004.png`

### E2E-EDGE-007 [P1]
- **最短复现：** 记录运行输出、待输入、未提交草稿、焦点后取消关闭。
- **实际：** 取消后草稿=false，顺序=true，焦点=true，真实进程继续=true。
- **期望：** 按原用例与 PRD 验收标准完成闭环。
- **用户影响：** 边界场景会破坏核心工作连续性。
- **证据：** `/tmp/matou-independent-qa-round4-20260830-122509/groups/LIF3-r3/evidence/E2E-LIFE-005.png`

### E2E-EDGE-009 [P0]
- **最短复现：** 全程仅键盘与可聚焦按钮完成新画布、兄弟、Fork、返回父、DAG跳转。
- **实际：** 新画布焦点=true；新兄弟=true；Fork可用=0、子节点就绪/焦点=true/false；返回父焦点=true；DAG搜索结果=0、跳转焦点/视野=false/true。
- **期望：** 按原用例与 PRD 验收标准完成闭环。
- **用户影响：** 边界场景会破坏核心工作连续性。
- **证据：** `/tmp/matou-independent-qa-round4-20260830-122509/groups/REC2-r3/evidence/E2E-EDGE-009.png`

### E2E-EDGE-013 [P1]
- **最短复现：** 恢复、重复恢复、再次关闭、正常退出重启。
- **实际：** 重复恢复后活动同名Tab数=0；再次关闭并重启后已关闭列表同名记录数=1。
- **期望：** 按原用例与 PRD 验收标准完成闭环。
- **用户影响：** 边界场景会破坏核心工作连续性。
- **证据：** `/tmp/matou-independent-qa-round4-20260830-122509/groups/LIF3-r3/evidence/E2E-EDGE-013.png`

### E2E-EDGE-014 [P1]
- **最短复现：** 白色/深色/白色真实快捷键切换，期间运行、待输入、草稿保持。
- **实际：** 主题背景 rgb(247, 248, 250)→rgba(0, 0, 0, 0)→rgb(247, 248, 250)；草稿保留=false；顺序保持=true。
- **期望：** 按原用例与 PRD 验收标准完成闭环。
- **用户影响：** 边界场景会破坏核心工作连续性。
- **证据：** `/tmp/matou-independent-qa-round4-20260830-122509/groups/LIF3-r3/evidence/theme-dark.png`、`/tmp/matou-independent-qa-round4-20260830-122509/groups/LIF3-r3/evidence/theme-light.png`

### E2E-EDGE-016 [P0]
- **最短复现：** 真实构建446195a包创建自定义名、关闭画布、父子关系、历史节点和新工作树后Cmd+Q，再以a43d42c同profile启动。
- **实际：** 446195a→a43d42c：自定义主画布名=true；已关闭画布入口=true；DAG节点/边/历史=3/2/0；worktree数=2、归属清单不变=true；数据目录不变=true。
- **期望：** 按原用例与 PRD 验收标准完成闭环。
- **用户影响：** 边界场景会破坏核心工作连续性。
- **证据：** `/tmp/matou-independent-qa-round4-20260830-122509/groups/UPGRADE2-r4/evidence/E2E-EDGE-016.png`

## 6. 阻塞项

- **E2E-REL-012 [P0]**：执行窗口内真实provider session身份保持有效；在不改凭据、不注入内部状态的约束下，没有真实可控的创建期身份失效事件。 证据：`/tmp/matou-independent-qa-round4-20260830-122509/groups/BLOCKERS-r4/evidence/external-preconditions.txt`
- **E2E-REL-019 [P1]**：真实认证账号未提供 Team 队友子会话，无法从实际队友节点验证入口边界。 证据：`/tmp/matou-independent-qa-round4-20260830-122509/environment/blockers.md`
- **E2E-STA-007 [P0]**：执行窗口内真实Claude服务持续可用，未出现自然provider异常；主动网络故障注入不满足真实外部行为约束。 证据：`/tmp/matou-independent-qa-round4-20260830-122509/groups/BLOCKERS-r4/evidence/external-preconditions.txt`
- **E2E-STA-011 [P1]**：执行窗口未发生自然摘要流中断；内部channel终止或接口注入属于被禁止的测试手段。 证据：`/tmp/matou-independent-qa-round4-20260830-122509/groups/BLOCKERS-r4/evidence/external-preconditions.txt`
- **E2E-ISO-005 [P1]**：当前主机仅检测到单一内建显示器，缺少第二物理显示器来验证浮层跟随当前屏幕。 证据：`/tmp/matou-independent-qa-round4-20260830-122509/groups/BLOCKERS-r4/evidence/display-topology.txt`
- **E2E-ISO-007 [P1]**：所有真实DAG启动均成功；未发生自然DAG异常，禁止通过内部接口/实现篡改制造失败。 证据：`/tmp/matou-independent-qa-round4-20260830-122509/groups/BLOCKERS-r4/evidence/external-preconditions.txt`

## 7. 可复核材料

- 机器可读结果：`/tmp/matou-independent-qa-round4-20260830-122509/execution-results-102.json`
- 分组证据：`/tmp/matou-independent-qa-round4-20260830-122509/groups/`
- 构建与环境：`/tmp/matou-independent-qa-round4-20260830-122509/environment/`
