# Matou 会话画布与 DAG 分支交互—第三轮独立黑盒执行记录

## 1. 验收结论

- **102 条全部逐条完成：PASS 74 / FAIL 20 / BLOCKED 8。
- **产品决策建议：暂不进入正式发布验收。** 当前仍有横向层级返回、DAG 远层表达、Claude 状态、目录移动恢复、独立窗口历史闭环等核心失败。
- 已确认可用的主链路包括：默认 Shell、新画布/兄弟自动聚焦、真实 Claude Fork、当前/新工作树、共同父兄弟、会话恢复重试、历史与整分支移除、关闭画布恢复、完整退出不中断重跑、100 会话 DAG 基础可用性。

## 2. 环境与隔离

| 项目 | 值 |
|---|---|
| App | `.worktrees/session-dag-canvas/apps/desktop/release/mac-arm64/Matou.app` |
| Commit | `446195a77d792032f6385ac2b553c732b01ad987` |
| 平台 | macOS arm64，单活动显示器 |
| 隔离根 | `/tmp/matou-independent-qa-round3-20260830-103649` |
| 隔离变量 | 每组独立 `MATOU_DATA_DIR`、`ELECTRON_USER_DATA_DIR`、`HOME`、`MATOU_DEFAULT_WORKSPACE` |
| 外部能力 | 真实 PTY、文件系统、Git/worktree、Claude Code 2.1.251 与真实 OAuth |
| 操作边界 | 真实打包 App UI、键鼠/触摸板等价 UI 操作；未改产品实现、未改数据库、未使用 mock/fake provider |

## 3. 产品旅程结论

- **会话创建与关系：** 根 Shell、Claude Fork、共同父兄弟、工作树隔离、关系恢复均可形成闭环。
- **横向工作区：** 普通滚动与小幅边界手势稳定，但清晰第二段大幅右拉仍可能不返回；窄窗不能同时保留至少两张可识别摘要卡。
- **DAG：** 快捷键、缩放平移、搜索跳转及 100 会话基础响应可用；远层虚影、完整路径信息和普通/Fork 边的可见说明仍不足。
- **状态与排序：** Shell 主状态、输入排序和大部分历史聚合可用；Claude 完成后的空闲、权限待输入、授权/拒绝/停止排序仍存在不一致。
- **恢复与边界：** Claude 恢复失败重试、主动退出、画布关闭恢复、完整退出 Shell 中断语义可用；工作区移动后原会话未自动恢复、独立窗口关闭后历史跳转未闭环。

## 4. 逐条执行结果

| 用例 ID | 优先级 | 时间（UTC+8） | 结果 | 实际观察与用户影响 | 证据 |
|---|---|---:|---|---|---|
| E2E-CAN-001 | P0 | 10:42:20 | **PASS** | 首次进入显示默认事项=true、Shell=true、终端焦点=true。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/SHELL-CORE-r3b/evidence/E2E-CAN-001.png` |
| E2E-CAN-002 | P0 | 10:42:22 | **PASS** | Tab 1->2，直接出现 Shell，焦点=true，未出现类型/工作树弹框。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/SHELL-CORE-r3b/evidence/E2E-CAN-002.png` |
| E2E-CAN-003 | P0 | 10:50:14 | **FAIL** | 画布 A/B 节点 3/2、焦点与 DAG 缩放 80%/110% 分别恢复；画布 A 横向位置从退出前 108 重置为 0，导致返回画布时视野丢失。 用户影响：用户返回画布或重启后需要重新找回工作位置，破坏连续工作。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/CN6-r3/evidence/E2E-CAN-003.png` |
| E2E-CAN-004 | P1 | 10:50:14 | **PASS** | 重启前名称=["画布 A","性能验证"]；重启后=["画布 A","性能验证"]。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/CN6-r3/evidence/E2E-CAN-004.png` |
| E2E-CAN-005 | P0 | 10:50:14 | **PASS** | 切换 B→A 后焦点=f7a46cfa-38b5-4f7b-8b98-5fa2ae098dca，重启前/后焦点=f7a46cfa-38b5-4f7b-8b98-5fa2ae098dca/f7a46cfa-38b5-4f7b-8b98-5fa2ae098dca，A 节点集合保持。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/CN6-r3/evidence/E2E-CAN-005.png` |
| E2E-CAN-006 | P0 | 10:42:24 | **PASS** | 新增 Shell 后自动焦点=true，可直接接收键盘。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/SHELL-CORE-r3b/evidence/E2E-REL-001.png` |
| E2E-CAN-007 | P0 | 10:55:40 | **PASS** | 真实回答耗时 2ms；回答完成后终端输入焦点=true。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/CF4-r3/evidence/E2E-CAN-007.png` |
| E2E-CAN-008 | P0 | 10:42:20 | **PASS** | 可见 cwd=/tmp/matou-independent-qa-round3-20260830-103649/groups/SHELL-CORE-r3b/workspace-git；profile 和数据均位于隔离组目录。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/SHELL-CORE-r3b/evidence/E2E-CAN-008.png` |
| E2E-REL-001 | P0 | 10:42:24 | **PASS** | 根层节点 1->6；新增位队尾=true；焦点在新增=true。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/SHELL-CORE-r3b/evidence/E2E-REL-001.png` |
| E2E-REL-002 | P0 | 10:55:49 | **PASS** | 非根子列表共 6，Shell=5，Claude=1。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/CF4-r3/evidence/E2E-REL-002.png` |
| E2E-REL-003 | P0 | 10:55:49 | **PASS** | 同一父下混排 profiles=["claude-code","shell","shell","shell","shell","shell"]。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/CF4-r3/evidence/E2E-REL-002.png` |
| E2E-REL-004 | P0 | 10:55:40 | **PASS** | 同一 Session 2876d2bb-af04-48ff-afe9-53be4fca9368 原地切换为 profile=claude-code，首轮后 Fork 可用。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/CF4-r3/evidence/E2E-REL-004.png` |
| E2E-REL-005 | P0 | 10:55:40 | **PASS** | 首轮前 Fork disabled=true、提示=完成首轮对话后可创建分支；真实首轮完成后 disabled=false。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/CF4-r3/evidence/E2E-REL-005.png` |
| E2E-REL-006 | P0 | 10:55:47 | **PASS** | 真实 Fork 子会话 ID=6b0511a1-9a30-4fae-a1e7-92c1e57e4c71，回复中继承父 token=true。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/CF4-r3/evidence/E2E-REL-006.png` |
| E2E-REL-007 | P0 | 10:55:47 | **PASS** | 父 cwd=/private/tmp/matou-independent-qa-round3-20260830-103649/groups/CF4-r3/workspace-git，子 cwd=/private/tmp/matou-independent-qa-round3-20260830-103649/groups/CF4-r3/workspace-git，相同=true，子共享标记=true。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/CF4-r3/evidence/E2E-REL-007.png` |
| E2E-REL-008 | P0 | 10:58:57 | **PASS** | 新子节点 cwd=/private/tmp/matou-independent-qa-round3-20260830-103649/groups/CF4-r3/matou-data/worktrees/1f4a4948-29f7-4227-8cb0-caf2b8bb3ee1/db2bdb5b-87f5-4904-badb-b3fcc87955e2，父 cwd=/private/tmp/matou-independent-qa-round3-20260830-103649/groups/CF4-r3/workspace-git，独立=true；git worktree list 包含=true。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/CF4-r3/evidence/E2E-REL-008-wt.png` |
| E2E-REL-009 | P0 | 10:59:37 | **PASS** | 非 Git 工作区新工作树 radio disabled=true，说明包含 Git=true；当前工作树选项仍可用。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/NG3-r3/evidence/E2E-REL-009.png` |
| E2E-REL-010 | P1 | 10:59:02 | **PASS** | 首次同名节点后直接子=8；第二次 modal保留=true、冲突提示=true、输入保留=same-visible-name；徽章 查看 8 个子会话->查看 8 个子会话；safe refs=["branch refs/heads/matou/same-visible-name-f170503a"]。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/CF4-r3/evidence/E2E-REL-010-wt.png` |
| E2E-REL-011 | P0 | 11:50:37 | **PASS** | 真实 chmod 只读后失败=true、原因可见=true；兄弟输出未受影响=true；恢复权限后重试入口=1、成功=true。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/REC2-r3/evidence/E2E-REL-011.png` |
| E2E-REL-012 | P0 | 11:51:22 | **BLOCKED** | 真实 Fork 准备期短于稳定人工失效窗口；移动 transcript 不会使存活父进程身份失效，撤销 Keychain OAuth 会影响用户真实账户。 本轮不将其计为产品通过或产品缺陷。 | `/tmp/matou-independent-qa-round3-20260830-103649/environment/blockers.md` |
| E2E-REL-013 | P0 | 11:45:22 | **PASS** | C1 Claude=true；共同父入口=1 enabled=true；列表 5→6；C2无C1后续=true、含父信息=true；Fork边=3、节点=8。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/REC2-r3/evidence/E2E-REL-013.png` |
| E2E-REL-014 | P0 | 11:06:56 | **PASS** | 真实链深度节点数=8，ID 唯一=true，当前直接子列表=1。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/CF4-r3/evidence/E2E-REL-014-multi.png` |
| E2E-REL-015 | P0 | 10:57:44 | **PASS** | /exit 后 profile=shell，已退出文案=false。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/CF4-r3/evidence/E2E-REL-015-continue.png` |
| E2E-REL-016 | P0 | 10:57:44 | **PASS** | 转 Shell 后子会话入口保留=true。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/CF4-r3/evidence/E2E-REL-016-continue.png` |
| E2E-REL-017 | P0 | 10:57:52 | **PASS** | 重新进入 Claude 完成首轮后 Fork disabled=false，子关系仍可进入=true。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/CF4-r3/evidence/E2E-REL-017-continue.png` |
| E2E-REL-018 | P1 | 11:54:15 | **PASS** | 前序真实执行已确认单会话无标记、Fork 后父子均显示共享标记；结束并移除子节点后重启，剩余节点=1、共享标记=false、目录保留=true。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/SHARE-r3/evidence/E2E-REL-018.png` |
| E2E-REL-019 | P1 | 10:59:37 | **BLOCKED** | 真实认证账号未提供 Team 队友子会话，无法从实际队友节点验证入口边界。 本轮不将其计为产品通过或产品缺陷。 | `/tmp/matou-independent-qa-round3-20260830-103649/environment/blockers.md` |
| E2E-NAV-001 | P0 | 11:55:58 | **PASS** | 父徽章总数4；悬浮显示 Claude 2 · Shell 2；真实状态聚合为运行中3、错误1，异常在1秒观察窗口内可见。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/SHARE-r3/evidence/E2E-NAV-001-exact.png` |
| E2E-NAV-002 | P0 | 10:55:49 | **PASS** | 进入子会话列表后只显示直接子=6，父节点不混入=true。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/CF4-r3/evidence/E2E-NAV-001.png` |
| E2E-NAV-003 | P0 | 10:42:24 | **PASS** | 同级=6，visibleColumns=4，横向滚动 129->624 / max 624。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/SHELL-CORE-r3b/evidence/E2E-NAV-003.png` |
| E2E-NAV-004 | P1 | 10:42:26 | **PASS** | 目标宽度 悬浮前/中/移出=286.0/362.5/286.0，点击聚焦后=633.6。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/SHELL-CORE-r3b/evidence/E2E-NAV-004.png` |
| E2E-NAV-005 | P0 | 10:55:49 | **PASS** | visibleColumns=4，从 654 一次右滑到 0，仍在 6 子列表=true。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/CF4-r3/evidence/E2E-NAV-005.png` |
| E2E-NAV-006 | P0 | 10:55:50 | **PASS** | 最左独立小幅右拉后仍在子列表=true。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/CF4-r3/evidence/E2E-NAV-006.png` |
| E2E-NAV-007 | P0 | 10:57:33 | **FAIL** | 最左独立连续大幅右拉后返回父=false。 用户影响：用户在多会话间定位或返回层级时会卡住、误解或增加额外操作。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/CF4-r3/evidence/E2E-NAV-007-continue.png` |
| E2E-NAV-008 | P0 | 11:36:44 | **PASS** | 初始根层=true；进入子列表=true；点击返回根层=true/焦点=true；键盘 Enter 返回根层=true/焦点=true。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/REC2-r3/evidence/E2E-NAV-008-retest2.png` |
| E2E-NAV-009 | P1 | 10:42:27 | **FAIL** | 700px 下卡片=1，视野标记=0，宽度=299，各卡保留可识别状态=true。 用户影响：用户在多会话间定位或返回层级时会卡住、误解或增加额外操作。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/SHELL-CORE-r3b/evidence/E2E-NAV-009.png` |
| E2E-NAV-010 | P1 | 11:35:17 | **FAIL** | 双节点持续输出3秒列表 scrollLeft=0→0、仍在列表=true；小幅独立回弹不返回=true；后续清晰大幅手势返回=false。 用户影响：用户在多会话间定位或返回层级时会卡住、误解或增加额外操作。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/REC2-r3/evidence/E2E-NAV-010.png` |
| E2E-DAG-001 | P0 | 10:57:41 | **PASS** | 长按前后窗口 1->2，独立 DAG=true。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/CF4-r3/evidence/E2E-DAG-001-continue.png` |
| E2E-DAG-002 | P0 | 10:42:22 | **PASS** | 短按 Tab 前后窗口数=1/1，终端仍可见=true。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/SHELL-CORE-r3b/evidence/E2E-DAG-002.png` |
| E2E-DAG-003 | P0 | 11:06:58 | **FAIL** | 跳转第4层=true；DAG 节点=3，完整=3，虚影/远层=0。 用户影响：用户无法可靠理解、浏览或跳转复杂会话关系。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/CF4-r3/evidence/E2E-DAG-003-multi.png` |
| E2E-DAG-004 | P0 | 10:42:53 | **PASS** | 缩放 100%->90%->100%，聚焦当前按钮可用。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/SHELL-CORE-r3b/evidence/E2E-DAG-004.png` |
| E2E-DAG-005 | P1 | 10:42:53 | **FAIL** | 节点信息包含类型=true、路径=false、子会话=true。 用户影响：用户无法可靠理解、浏览或跳转复杂会话关系。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/SHELL-CORE-r3b/evidence/E2E-DAG-005.png` |
| E2E-DAG-006 | P1 | 10:42:56 | **PASS** | 最近输出刷新到 LIVE_6=true，画布 transform 未被推移=true。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/SHELL-CORE-r3b/evidence/E2E-DAG-006.png` |
| E2E-DAG-007 | P0 | 10:57:38 | **PASS** | 目标初始 inViewport=false；DAG 可见搜索结果项=1；点击后 inViewport=true、focused=true。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/CF4-r3/evidence/E2E-DAG-007-continue.png` |
| E2E-DAG-008 | P0 | 10:57:40 | **PASS** | 目标悬浮/窄窗/恢复后 inViewport=true/true/true，焦点=true。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/CF4-r3/evidence/E2E-DAG-008-continue.png` |
| E2E-DAG-009 | P0 | 11:54:33 | **PASS** | DAG父节点可见=1；点击后根层=true、单节点=1、子徽章=1、输入焦点=true。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/REC2-r3/evidence/E2E-DAG-009-final.png` |
| E2E-DAG-010 | P1 | 11:47:11 | **PASS** | 真实创建 100 个 Shell；DAG 在 416ms 内显示当前邻域 4 个节点（按三层/视区投影），30 次平移/缩放/聚焦连续；远端搜索 1 个结果并正确跳转视野和焦点；10 个真实长命令并行时 DAG 416ms 重开且继续缩放。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/BIG-r3/evidence/E2E-DAG-010.png` |
| E2E-STA-001 | P0 | 10:42:31 | **PASS** | 提交后运行中=true，完成后空闲=true。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/SHELL-CORE-r3b/evidence/E2E-STA-001.png` |
| E2E-STA-002 | P0 | 10:42:36 | **PASS** | 退出码23后错误可见=true，同 Shell 后续输出恢复=true。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/SHELL-CORE-r3b/evidence/E2E-STA-002.png` |
| E2E-STA-003 | P0 | 10:42:39 | **PASS** | 明确 prompt 后 DAG 待输入=true，输入后完成=true。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/SHELL-CORE-r3b/evidence/E2E-STA-003.png` |
| E2E-STA-004 | P1 | 10:42:40 | **PASS** | 无 prompt stdin 等待：运行中=true、待输入=false。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/SHELL-CORE-r3b/evidence/E2E-STA-004.png` |
| E2E-STA-005 | P0 | 10:55:43 | **FAIL** | Claude 提交后运行中=true，回答后空闲=false。 用户影响：状态与真实进程不同步，用户可能误判是否需要处理或定位错误。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/CF4-r3/evidence/E2E-STA-005.png` |
| E2E-STA-006 | P0 | 11:08:59 | **FAIL** | 真实工具权限提示=true；DAG 待输入=false；允许后文件=true、空闲=false。 用户影响：状态与真实进程不同步，用户可能误判是否需要处理或定位错误。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/CF4-r3/evidence/E2E-STA-006-perm.png` |
| E2E-STA-007 | P0 | 11:51:22 | **BLOCKED** | 无仅作用于目标一轮且可撤销的真实账户/网络失败条件；未用伪造网络或 provider。 本轮不将其计为产品通过或产品缺陷。 | `/tmp/matou-independent-qa-round3-20260830-103649/environment/blockers.md` |
| E2E-STA-008 | P0 | 11:09:19 | **PASS** | 父徽章聚合=Claude 5 · Shell 5；运行中 5 · 待输入 0 · 空闲 5；错误 0 · 中断 0。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/CF4-r3/evidence/E2E-STA-008-perm.png` |
| E2E-STA-009 | P0 | 11:45:45 | **PASS** | 父徽章总量 7，活动横向节点 6，差额 1；DAG 仅 1 个 status-exited 历史节点并显示“历史”；运行中 2 均为活动 Claude，已结束节点未计入运行。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/REC2-r3/evidence/E2E-STA-009-final.png` |
| E2E-STA-010 | P0 | 11:58:28 | **FAIL** | Shell B 输出=true；Claude真实回答=true；父徽章=Claude 4 · Shell 5；运行中 1 · 待输入 0 · 空闲 5；错误 3 · 中断 0；+1 历史、异常聚合=true；点击后错误 Shell A 首位=false。 用户影响：状态与真实进程不同步，用户可能误判是否需要处理或定位错误。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/REC2-r3/evidence/E2E-STA-010-final.png` |
| E2E-STA-011 | P1 | 11:51:22 | **BLOCKED** | 真实 Runtime 中断会同时停止 PTY，不满足仅摘要断流前置；未注入内部断流。 本轮不将其计为产品通过或产品缺陷。 | `/tmp/matou-independent-qa-round3-20260830-103649/environment/blockers.md` |
| E2E-STA-012 | P1 | 10:42:32 | **PASS** | 真实命令到 DAG 运行中可见延迟=748ms。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/SHELL-CORE-r3b/evidence/E2E-STA-012.png` |
| E2E-SORT-001 | P0 | 10:42:44 | **PASS** | 草稿不排序=true；提交后移首=true；焦点=true。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/SHELL-CORE-r3b/evidence/E2E-SORT-001.png` |
| E2E-SORT-002 | P0 | 11:57:39 | **PASS** | 末尾 C Ctrl+C 后移首=true；等待 stdin 的 B 完成输入后移首=true；三个稳定节点仍唯一=true。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/SORT2-r3/evidence/E2E-SORT-002.png` |
| E2E-SORT-003 | P0 | 11:08:59 | **PASS** | 后位 Claude 发送消息后移首=true，发送前同级=9。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/CF4-r3/evidence/E2E-SORT-003-perm.png` |
| E2E-SORT-004 | P0 | 11:09:19 | **FAIL** | 允许前他会话移首=true/允许后 Claude 移首=true；拒绝提示=true、拒绝后移首=false、文件未创建=true；选项提示=true/完成后移首=true；停止/继续移首=false/false。 用户影响：最近操作排序不稳定，用户难以快速回到刚处理的会话。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/CF4-r3/evidence/E2E-SORT-004-perm.png` |
| E2E-SORT-005 | P0 | 10:42:46 | **PASS** | 横向滚动、打开关闭 DAG 前后顺序稳定=true。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/SHELL-CORE-r3b/evidence/E2E-SORT-005.png` |
| E2E-SORT-006 | P0 | 10:42:48 | **PASS** | 后台完成前后顺序不变=true。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/SHELL-CORE-r3b/evidence/E2E-SORT-006.png` |
| E2E-SORT-007 | P0 | 10:42:48 | **PASS** | 新节点位队尾=true，总数 6->7。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/SHELL-CORE-r3b/evidence/E2E-SORT-007.png` |
| E2E-SORT-008 | P0 | 10:50:14 | **PASS** | 重启前后顺序稳定=true；再交互尾节点后移首=true。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/CN6-r3/evidence/E2E-SORT-008.png` |
| E2E-SORT-009 | P0 | 11:09:20 | **PASS** | 多次排序后 DAG 节点=2、边元素=4，父徽章关系仍在。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/CF4-r3/evidence/E2E-SORT-009-perm.png` |
| E2E-LIFE-001 | P0 | 11:13:14 | **PASS** | 父会话已结束，DAG 搜索仍有历史父=1，历史详情保留并提供查看子会话。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/CF4-r3/evidence/E2E-LIFE-001-branch.png` |
| E2E-LIFE-002 | P1 | 11:12:35 | **PASS** | 活动叶搜索=1；结束后历史结果=1；单节点移除入口=true；确认无后代=true；取消保留=true；确认后搜索消失=true；工作目录保留=true。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/CF4-r3/evidence/E2E-LIFE-002-leaf.png` |
| E2E-LIFE-003 | P1 | 11:13:14 | **PASS** | 历史父详情=true；单节点移除入口=false；整分支入口=true；Delete 后关系保留=true。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/CF4-r3/evidence/E2E-LIFE-003-branch.png` |
| E2E-LIFE-004 | P1 | 11:14:24 | **PASS** | 首确认准确列16个后代=16；二次确认说明保留本地=true；确认后父搜索消失=true；worktree清单不变=true。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/CF4-r3/evidence/E2E-LIFE-004-final.png` |
| E2E-LIFE-005 | P0 | 11:24:02 | **PASS** | 确认框=true，取消后运行输出继续=true，画布/三节点仍在=true。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/LIF3-r3/evidence/close-modal.png`<br>`/tmp/matou-independent-qa-round3-20260830-103649/groups/LIF3-r3/evidence/E2E-LIFE-005.png` |
| E2E-LIFE-006 | P1 | 11:24:55 | **PASS** | 已关闭画布列表显示一条，真实恢复控件=1；恢复后Tab=2、节点=3、终端历史草稿可见=true。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/LIF3-r3/evidence/E2E-LIFE-006-retest.png` |
| E2E-LIFE-007 | P0 | 11:25:35 | **PASS** | 关闭窗口后AX窗口数=0；计数 3→隐藏2秒8→重新打开10；原会话可见=true、焦点=true。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/OSL1-r3/evidence/E2E-LIFE-007.png` |
| E2E-LIFE-008 | P0 | 10:50:14 | **FAIL** | 完整退出重启后两张 Tab、当前画布、节点与 DAG 缩放 80%/110% 均恢复；横向位置未恢复（108→0），未满足观察位置完整恢复。 用户影响：退出/恢复后的工作现场不完整，增加重找上下文成本。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/CN6-r3/evidence/E2E-LIFE-008.png` |
| E2E-LIFE-009 | P0 | 11:27:01 | **PASS** | 真实 Cmd+Q 进程退出=true；退出前3行，退出等待后3，重启继续等待3，手动重提后5；中断且未自动重跑提示=true。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/OSL4-r3/evidence/E2E-LIFE-009.png` |
| E2E-LIFE-010 | P0 | 11:29:54 | **PASS** | 移除真实 transcript 后 profile=shell、失败提示=true、重试入口=1；Shell可用=true；子关系=true；缺失时重试仍失败=true；恢复文件后Claude=true、关系入口=true。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/REC2-r3/evidence/E2E-LIFE-010.png` |
| E2E-ISO-001 | P0 | 11:21:10 | **FAIL** | 真实 UI 新建自定义工作区成功；目录移动后路径失效、输入/创建受限与“恢复目录”入口正确。选择移动后的真实目录后 toast 和标题路径更新，但原会话仍停留“会话启动失败”，没有恢复为可输入状态，无法直接验证新 Shell pwd。 用户影响：跨目录、窗口、主题或并发场景的完整旅程中断。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/WM6-r3/evidence/E2E-ISO-001.png` |
| E2E-ISO-002 | P0 | 11:51:22 | **BLOCKED** | 真实小仓库 worktree 准备阶段在人工终止前完成；未使用 I/O mock 延长。 本轮不将其计为产品通过或产品缺陷。 | `/tmp/matou-independent-qa-round3-20260830-103649/environment/blockers.md` |
| E2E-ISO-003 | P0 | 11:29:55 | **PASS** | 仅目标恢复失败且恢复；另一画布Tab数=2、真实Shell输入=true；目标子关系=true。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/REC2-r3/evidence/E2E-ISO-003.png` |
| E2E-ISO-004 | P1 | 11:48:51 | **FAIL** | 独立窗口创建页数=2、初始焦点=true；DAG搜索=0，点击后窗口前台=true/输入焦点=true；真实 Cmd+W 关闭=false；DAG历史结果=0、继续/重开入口=false。 用户影响：跨目录、窗口、主题或并发场景的完整旅程中断。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/IND3-r3/evidence/E2E-ISO-004-front.png`<br>`/tmp/matou-independent-qa-round3-20260830-103649/groups/IND3-r3/evidence/E2E-ISO-004.png` |
| E2E-ISO-005 | P1 | 11:51:22 | **BLOCKED** | 系统仅有一个活动显示器，缺少第二显示器前置。 本轮不将其计为产品通过或产品缺陷。 | `/tmp/matou-independent-qa-round3-20260830-103649/environment/blockers.md` |
| E2E-ISO-006 | P0 | 11:24:03 | **FAIL** | 白/深主题切换成功且 Shell 焦点、运行、待输入可区分；当前真实场景中未同时呈现 Claude、Fork 边、历史及虚影节点，无法满足 BASE-STATES 全项视觉断言。 用户影响：跨目录、窗口、主题或并发场景的完整旅程中断。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/LIF3-r3/evidence/theme-light.png`<br>`/tmp/matou-independent-qa-round3-20260830-103649/groups/LIF3-r3/evidence/theme-dark.png` |
| E2E-ISO-007 | P1 | 11:51:22 | **BLOCKED** | 无可逆且仅让 DAG BrowserWindow 创建失败的真实 OS 条件。 本轮不将其计为产品通过或产品缺陷。 | `/tmp/matou-independent-qa-round3-20260830-103649/environment/blockers.md` |
| E2E-ISO-008 | P1 | 10:43:14 | **FAIL** | 10 个真实 Shell 并发完成标记=[false,false,false,false,true,false,false,false,false,false]，主界面仍响应。 用户影响：跨目录、窗口、主题或并发场景的完整旅程中断。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/SHELL-CORE-r3b/evidence/E2E-ISO-008.png` |
| E2E-EDGE-001 | P0 | 11:37:27 | **FAIL** | 真实目录移动后新建Tab disabled=true；Tab 1→1；启动失败卡片=false。实际入口被直接禁用，用户看不到可重试的新画布失败卡片。 恢复原目录后入口恢复=true。 用户影响：异常或边界场景缺少可恢复闭环，可能造成工作现场不可用或信息误读。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/EPF-r3/evidence/E2E-EDGE-001.png` |
| E2E-EDGE-002 | P0 | 11:37:27 | **FAIL** | 真实目录移动后横向新增 disabled=true；活动卡 1→1；失败卡片=false。实际入口被直接禁用。 恢复原目录后入口恢复=true。 用户影响：异常或边界场景缺少可恢复闭环，可能造成工作现场不可用或信息误读。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/EPF-r3/evidence/E2E-EDGE-002.png` |
| E2E-EDGE-003 | P1 | 10:42:21 | **PASS** | 单节点 DAG 节点数=1，子会话0=true，连线=2。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/SHELL-CORE-r3b/evidence/E2E-EDGE-003.png` |
| E2E-EDGE-004 | P1 | 11:07:00 | **PASS** | 七层关系 DAG 拖拽后仍响应=true；远层搜索结果=1；点击第7层跳转=true。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/CF4-r3/evidence/E2E-EDGE-004-multi.png` |
| E2E-EDGE-005 | P0 | 11:56:29 | **PASS** | 真实原工作树与 Runtime 创建的新工作树均写入唯一未提交文件；结束并移除无关系根 Shell（该节点直接移除，无独立历史入口）、关闭画布后，两工作树、分支、文件与 git status 均保留。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/E6-r3/evidence/E2E-EDGE-005.png` |
| E2E-EDGE-006 | P0 | 11:27:38 | **PASS** | 主动 /exit 回 Shell=true；正常退出重启后 profile=shell；无恢复/错误/Claude启动输出=true；Shell直接输入=true。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/CEX-r3/evidence/E2E-EDGE-006.png` |
| E2E-EDGE-007 | P1 | 11:25:00 | **PASS** | 关闭确认取消后，未提交草稿“UNSENT_DRAFT_446195A”仍在原 Shell 输入行；顺序和焦点保持，运行输出从 LIFE_RUN_4 继续增长。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/LIF3-r3/evidence/E2E-LIFE-005.png` |
| E2E-EDGE-008 | P1 | 11:38:36 | **FAIL** | 工作区完整 title 长度=284；侧栏/Tab CSS 溢出截断=true/true；标准/窄窗横向新增按钮=1/1；数量仍可见=true。 用户影响：异常或边界场景缺少可恢复闭环，可能造成工作现场不可用或信息误读。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/LONG-r3/evidence/E2E-EDGE-008-wide.png`<br>`/tmp/matou-independent-qa-round3-20260830-103649/groups/LONG-r3/evidence/E2E-EDGE-008.png` |
| E2E-EDGE-009 | P0 | 11:39:19 | **PASS** | 新画布焦点=true；新兄弟=true；Fork可用=true、子节点就绪/焦点=true/true；返回父焦点=true；DAG搜索结果=4、跳转焦点/视野=true/true。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/REC2-r3/evidence/E2E-EDGE-009.png` |
| E2E-EDGE-010 | P0 | 11:13:15 | **PASS** | 从 DAG 历史父搜索结果进入后显示历史详情=true，没有活动终端输入=true。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/CF4-r3/evidence/E2E-EDGE-010-branch.png` |
| E2E-EDGE-011 | P1 | 11:57:55 | **FAIL** | DAG节点=1；ANSI/alternate控制字符泄漏=false；中文/emoji/最终摘要可读=false；卡片宽=260px。 用户影响：异常或边界场景缺少可恢复闭环，可能造成工作现场不可用或信息误读。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/E11-r3/evidence/E2E-EDGE-011.png` |
| E2E-EDGE-012 | P0 | 11:35:20 | **FAIL** | 父徽章="Claude 的子会话\n5 个会话\n← 返回父会话\n⑂ Fork\n＋"；DAG边DOM=[{"cls":"dag-edge relation-forked-from","title":null,"text":""},{"cls":"dag-edge relation-derived-from","title":null,"text":""},{"cls":"dag-edge relation-derived-from","title":null,"text":""},{"cls":"dag-edge relation-derived-from","title":null,"text":""},{"cls":"dag-edge relation-derived-from","title":null,"text":""}]；Fork语义提示=false。 用户影响：异常或边界场景缺少可恢复闭环，可能造成工作现场不可用或信息误读。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/REC2-r3/evidence/E2E-EDGE-012-light.png`<br>`/tmp/matou-independent-qa-round3-20260830-103649/groups/REC2-r3/evidence/E2E-EDGE-012-dark.png` |
| E2E-EDGE-013 | P1 | 11:25:00 | **PASS** | 活动画布再次查看已关闭列表时同一条恢复入口=0；活动Tab数=2；再次关闭并重启后恢复入口=1。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/LIF3-r3/evidence/E2E-EDGE-013-retest.png` |
| E2E-EDGE-014 | P1 | 11:25:00 | **PASS** | 真实快捷键切换白→深→白；深色截图中运行输出、WAITING_INPUT 与未提交草稿仍在原节点，顺序和焦点边框未变化。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/LIF3-r3/evidence/theme-dark.png`<br>`/tmp/matou-independent-qa-round3-20260830-103649/groups/LIF3-r3/evidence/theme-light.png` |
| E2E-EDGE-015 | P0 | 11:55:15 | **PASS** | 重启且 Shell/Claude 模式切换后的实际 DAG 边=10；所有直接子边起点唯一父=true；终点无重复=true。 | `/tmp/matou-independent-qa-round3-20260830-103649/groups/REC2-r3/evidence/E2E-EDGE-015-final.png` |
| E2E-EDGE-016 | P0 | 11:51:22 | **BLOCKED** | 仅有 commit 446195a 的一个打包版本，缺少同 bundle id 后续真实版本。 本轮不将其计为产品通过或产品缺陷。 | `/tmp/matou-independent-qa-round3-20260830-103649/environment/blockers.md` |

## 5. 失败项详细复现

> 以下均为真实 App 现场。修复后应优先复测本项及其同组关联回归项。

### E2E-CAN-003 [P0] 多画布数据与观察状态隔离

- **最短真实复现**：两画布设置不同焦点、横向位置和 80%/110% DAG 缩放后完整重启。
- **期望**：1) 在画布 A 选中第二会话并滑到中部 → 记录焦点和横向位置；2) 打开 DAG，平移并缩放到 80%，关闭 → 位置被保留；3) 切换画布 B 并改变其焦点/DAG 观察位置；4) 往返 A/B → 两组节点、焦点、滑动与 DAG 位置各自恢复，输出不串画布。
- **实际**：画布 A/B 节点 3/2、焦点与 DAG 缩放 80%/110% 分别恢复；画布 A 横向位置从退出前 108 重置为 0，导致返回画布时视野丢失。
- **用户影响**：用户返回画布或重启后需要重新找回工作位置，破坏连续工作。
- **证据**：`/tmp/matou-independent-qa-round3-20260830-103649/groups/CN6-r3/evidence/E2E-CAN-003.png`

### E2E-NAV-007 [P0] 第二次右拉超过阈值返回父会话

- **最短真实复现**：最左边界用四段真实横向滚轮持续右拉。
- **期望**：1) 新手势向右拉至显示“松手返回父会话”；2) 松手 → 页面切回父会话；3) 直接键入 `CMD-MARK` → 父会话获得输入，证明焦点恢复。
- **实际**：最左独立连续大幅右拉后返回父=false。
- **用户影响**：用户在多会话间定位或返回层级时会卡住、误解或增加额外操作。
- **证据**：`/tmp/matou-independent-qa-round3-20260830-103649/groups/CF4-r3/evidence/E2E-NAV-007-continue.png`

### E2E-NAV-009 [P1] 窄窗口的卡片让位

- **最短真实复现**：逐步缩窄到 700px 观察当前卡与其他摘要。
- **期望**：1) 逐步缩窄主窗口 → 当前会话的标题、状态、输入区保留；2) 其他会话收敛为标题、状态和最新摘要；3) 分支数量与错误/待输入信息优先于长工作树路径；4) 窗口恢复后布局恢复。
- **实际**：700px 下卡片=1，视野标记=0，宽度=299，各卡保留可识别状态=true。
- **用户影响**：用户在多会话间定位或返回层级时会卡住、误解或增加额外操作。
- **证据**：`/tmp/matou-independent-qa-round3-20260830-103649/groups/SHELL-CORE-r3b/evidence/E2E-NAV-009.png`

### E2E-NAV-010 [P1] 列表边缘持续输出时不误触层级切换

- **最短真实复现**：第一与第五子节点真实持续输出，最左分两次独立右拉。
- **期望**：1) 观察 3 秒 → 列表不随输出移动；2) 使用细小的横向回弹手势 → 不进入父投影；3) 完成一次清晰的新手势且超过阈值 → 才返回父会话。
- **实际**：双节点持续输出3秒列表 scrollLeft=0→0、仍在列表=true；小幅独立回弹不返回=true；后续清晰大幅手势返回=false。
- **用户影响**：用户在多会话间定位或返回层级时会卡住、误解或增加额外操作。
- **证据**：`/tmp/matou-independent-qa-round3-20260830-103649/groups/REC2-r3/evidence/E2E-NAV-010.png`

### E2E-DAG-003 [P0] DAG 默认三层与超出层虚影

- **最短真实复现**：以第4层为当前节点打开 DAG，检查三层和远层虚影。
- **期望**：1) 以中间节点为当前节点打开 DAG → 父层、当前兄弟层、子层完整显示；2) 更远祖先/后代以虚影和方向提示出现；3) 平移向远层 → 虚影逐渐变实并显示摘要；4) 已经过的近层按视野规则收敛。
- **实际**：跳转第4层=true；DAG 节点=3，完整=3，虚影/远层=0。
- **用户影响**：用户无法可靠理解、浏览或跳转复杂会话关系。
- **证据**：`/tmp/matou-independent-qa-round3-20260830-103649/groups/CF4-r3/evidence/E2E-DAG-003-multi.png`

### E2E-DAG-005 [P1] 节点信息完整与缩放让位

- **最短真实复现**：检查 DAG 节点信息与缩放让位。
- **期望**：1) 100% 查看节点 → 显示名称、Shell/Claude、工作树/分支、状态、最近四行、活动时间；2) 缩小至 40% → 名称、状态和连线优先保留，摘要与时间收起；3) 悬浮节点 → 可查看完整路径。
- **实际**：节点信息包含类型=true、路径=false、子会话=true。
- **用户影响**：用户无法可靠理解、浏览或跳转复杂会话关系。
- **证据**：`/tmp/matou-independent-qa-round3-20260830-103649/groups/SHELL-CORE-r3b/evidence/E2E-DAG-005.png`

### E2E-STA-005 [P0] Claude 空闲—运行—空闲

- **最短真实复现**：真实 Claude 第二轮前中后观察 DAG 状态。
- **期望**：1) 就绪后显示空闲；2) 发送需要读取多个文件的真实任务 → 思考/工具使用期显示运行中；3) 正常完成 → 转为空闲而非待输入；4) 输入焦点保持。
- **实际**：Claude 提交后运行中=true，回答后空闲=false。
- **用户影响**：状态与真实进程不同步，用户可能误判是否需要处理或定位错误。
- **证据**：`/tmp/matou-independent-qa-round3-20260830-103649/groups/CF4-r3/evidence/E2E-STA-005.png`

### E2E-STA-006 [P0] Claude 授权/选择待输入

- **最短真实复现**：Claude Write 工具真实请求权限，允许后完成。
- **期望**：1) 提交 `CC-WAIT` → 工作期为运行中；2) 出现授权、选项或明确提问 → 转为琲珀色待输入，聚合与通知指向当前节点；3) 完成选择 → 转为运行中；4) 任务完成后转空闲。
- **实际**：真实工具权限提示=true；DAG 待输入=false；允许后文件=true、空闲=false。
- **用户影响**：状态与真实进程不同步，用户可能误判是否需要处理或定位错误。
- **证据**：`/tmp/matou-independent-qa-round3-20260830-103649/groups/CF4-r3/evidence/E2E-STA-006-perm.png`

### E2E-STA-010 [P0] 单会话错误隔离

- **最短真实复现**：同父两 Shell+Claude：A false、B echo、Claude真实消息，返回父检查聚合并点击。
- **期望**：1) 在 Shell A 提交 `CMD-FAIL` → 只 A 显示错误；2) Shell B 提交 `echo still-alive` → 正常输出；3) Claude 发送消息 → 正常回答；4) 父/画布聚合显示异常，点击后定位到 A。
- **实际**：Shell B 输出=true；Claude真实回答=true；父徽章=Claude 4 · Shell 5；运行中 1 · 待输入 0 · 空闲 5；错误 3 · 中断 0；+1 历史、异常聚合=true；点击后错误 Shell A 首位=false。
- **用户影响**：状态与真实进程不同步，用户可能误判是否需要处理或定位错误。
- **证据**：`/tmp/matou-independent-qa-round3-20260830-103649/groups/REC2-r3/evidence/E2E-STA-010-final.png`

### E2E-SORT-004 [P0] Claude 授权、拒绝、选项与停止/继续排序

- **最短真实复现**：真实允许、拒绝、选项、停止和继续后逐次检查排序。
- **期望**：1) 在会话 C 允许授权 → C 移到最前；2) 重置场景，在 B 拒绝授权 → B 移到最前；3) 在 A 完成选项 → A 移到最前；4) 在后位会话执行停止或继续 → 该会话移到最前。
- **实际**：允许前他会话移首=true/允许后 Claude 移首=true；拒绝提示=true、拒绝后移首=false、文件未创建=true；选项提示=true/完成后移首=true；停止/继续移首=false/false。
- **用户影响**：最近操作排序不稳定，用户难以快速回到刚处理的会话。
- **证据**：`/tmp/matou-independent-qa-round3-20260830-103649/groups/CF4-r3/evidence/E2E-SORT-004-perm.png`

### E2E-LIFE-008 [P0] 应用完整退出后关系和观察位置恢复

- **最短真实复现**：多画布现场通过应用菜单完整退出并重启。
- **期望**：1) 记录现场并通过应用菜单完整退出；2) 用同一测试 profile 重启 → 恢复工作空间、事项、Tab、名称、关系、节点和顺序；3) 当前画布/焦点/横向位置与退出前一致；4) 打开 DAG → 缩放与观察位置一致。
- **实际**：完整退出重启后两张 Tab、当前画布、节点与 DAG 缩放 80%/110% 均恢复；横向位置未恢复（108→0），未满足观察位置完整恢复。
- **用户影响**：退出/恢复后的工作现场不完整，增加重找上下文成本。
- **证据**：`/tmp/matou-independent-qa-round3-20260830-103649/groups/CN6-r3/evidence/E2E-LIFE-008.png`

### E2E-ISO-001 [P0] 工作空间目录改名/移动后受限与恢复

- **最短真实复现**：通过 UI 新建自定义工作区，正常退出后真实移动目录，重启并用 UI 重连。
- **期望**：1) 退出 App，在 Finder/终端将 `WS-GIT` 移动到另一测试路径；2) 重启 → 画布、历史和 DAG 可查看，输入与创建入口置灰并提示恢复目录；3) 选择移动后的真实目录 → 各节点恢复可输入，关系和工作树归属不变；4) 新建 Shell 的 `pwd` 为新路径。
- **实际**：真实 UI 新建自定义工作区成功；目录移动后路径失效、输入/创建受限与“恢复目录”入口正确。选择移动后的真实目录后 toast 和标题路径更新，但原会话仍停留“会话启动失败”，没有恢复为可输入状态，无法直接验证新 Shell pwd。
- **用户影响**：跨目录、窗口、主题或并发场景的完整旅程中断。
- **证据**：`/tmp/matou-independent-qa-round3-20260830-103649/groups/WM6-r3/evidence/E2E-ISO-001.png`

### E2E-ISO-004 [P1] 独立窗口节点的 DAG 关系与唤起

- **最短真实复现**：通过卡片真实右键独立窗口，主窗口 DAG 搜索跳转，关闭独立窗口后再次 DAG 跳转。
- **期望**：1) 主窗口打开 DAG → C 保持原画布的父子与兄弟位置，状态实时；2) 点击 C → DAG 关闭，C 的独立窗口到前台并获得输入焦点；3) 关闭 C 独立窗口后再在 DAG 选择 → 显示历史节点与可执行的继续/重新打开入口。
- **实际**：独立窗口创建页数=2、初始焦点=true；DAG搜索=0，点击后窗口前台=true/输入焦点=true；真实 Cmd+W 关闭=false；DAG历史结果=0、继续/重开入口=false。
- **用户影响**：跨目录、窗口、主题或并发场景的完整旅程中断。
- **证据**：`/tmp/matou-independent-qa-round3-20260830-103649/groups/IND3-r3/evidence/E2E-ISO-004-front.png`；`/tmp/matou-independent-qa-round3-20260830-103649/groups/IND3-r3/evidence/E2E-ISO-004.png`

### E2E-ISO-006 [P0] 白色与深色主题的关系可读性

- **最短真实复现**：在实际状态画布切换白色/深色并截图逐项检查。
- **期望**：1) 白色主题逐项查看 Shell/Claude、各状态、焦点、悬浮、历史和连线 → 文字、边界与层级清晰；2) 切换深色主题 → 信息语义和数量不变，视觉仍可区分；3) 对两个主题截图检查状态颜色与 PRD 约定一致。
- **实际**：白/深主题切换成功且 Shell 焦点、运行、待输入可区分；当前真实场景中未同时呈现 Claude、Fork 边、历史及虚影节点，无法满足 BASE-STATES 全项视觉断言。
- **用户影响**：跨目录、窗口、主题或并发场景的完整旅程中断。
- **证据**：`/tmp/matou-independent-qa-round3-20260830-103649/groups/LIF3-r3/evidence/theme-light.png`；`/tmp/matou-independent-qa-round3-20260830-103649/groups/LIF3-r3/evidence/theme-dark.png`

### E2E-ISO-008 [P1] 10+ 会话同时真实输出

- **最短真实复现**：10 个会话同时真实输出。
- **期望**：1) 逐个提交带不同前缀的 `CMD-LONG`；2) 在两张画布之间往返、横向滑动、打开 DAG → 主界面持续响应；3) 每个会话输出只含自己前缀，无串流；4) 焦点、横向位置和排序不被后台输出改变；5) 命令结束后所有状态回到空闲。
- **实际**：10 个真实 Shell 并发完成标记=[false,false,false,false,true,false,false,false,false,false]，主界面仍响应。
- **用户影响**：跨目录、窗口、主题或并发场景的完整旅程中断。
- **证据**：`/tmp/matou-independent-qa-round3-20260830-103649/groups/SHELL-CORE-r3b/evidence/E2E-ISO-008.png`

### E2E-EDGE-001 [P0] 新画布 Shell 启动失败

- **最短真实复现**：通过真实 UI 选择自定义工作区，运行中真实移动目录后点击新画布。
- **期望**：1) 新建画布 → Tab 保留，显示启动失败原因与重新创建；2) 切换原画布 → 原会话可用；3) 恢复真实条件后重试 → Shell 成功就绪并自动焦点。
- **实际**：真实目录移动后新建Tab disabled=true；Tab 1→1；启动失败卡片=false。实际入口被直接禁用，用户看不到可重试的新画布失败卡片。 恢复原目录后入口恢复=true。
- **用户影响**：异常或边界场景缺少可恢复闭环，可能造成工作现场不可用或信息误读。
- **证据**：`/tmp/matou-independent-qa-round3-20260830-103649/groups/EPF-r3/evidence/E2E-EDGE-001.png`

### E2E-EDGE-002 [P0] 横向 Shell 创建失败卡片

- **最短真实复现**：同一失效工作区点击横向新增 Shell。
- **期望**：1) 横向新增 Shell → 队尾显示失败卡片与原因；2) 当前会话焦点和其他兄弟保持；3) 重试成功后仍位于队尾；4) 重现并移除 → 只移除失败卡片。
- **实际**：真实目录移动后横向新增 disabled=true；活动卡 1→1；失败卡片=false。实际入口被直接禁用。 恢复原目录后入口恢复=true。
- **用户影响**：异常或边界场景缺少可恢复闭环，可能造成工作现场不可用或信息误读。
- **证据**：`/tmp/matou-independent-qa-round3-20260830-103649/groups/EPF-r3/evidence/E2E-EDGE-002.png`

### E2E-EDGE-008 [P1] 长路径与标题空间让位

- **最短真实复现**：真实 200+字符路径，悬浮可取完整 title，缩窄窗口检查操作收拢与异常/数量优先。
- **期望**：1) 标准宽度观察标题 → 会话名、数量、异常优先完整，路径截断；2) 悬浮路径 → 显示完整值；3) 缩窄窗口 → 创建入口收拢到操作菜单，数量和错误仍可见。
- **实际**：工作区完整 title 长度=284；侧栏/Tab CSS 溢出截断=true/true；标准/窄窗横向新增按钮=1/1；数量仍可见=true。
- **用户影响**：异常或边界场景缺少可恢复闭环，可能造成工作现场不可用或信息误读。
- **证据**：`/tmp/matou-independent-qa-round3-20260830-103649/groups/LONG-r3/evidence/E2E-EDGE-008-wide.png`；`/tmp/matou-independent-qa-round3-20260830-103649/groups/LONG-r3/evidence/E2E-EDGE-008.png`

### E2E-EDGE-011 [P1] DAG 节点摘要输出边界

- **最短真实复现**：单真实 Shell 依次输出空行、ANSI、超长行、中文、emoji、alternate-screen、最终行。
- **期望**：1) 打开 DAG → 最近四行文字可读，无 ANSI 控制字符泄漏；2) 超长行按视觉规则截断，不扩大卡片；3) 中文/emoji 不乱码；4) alternate-screen 更新不制造重复节点或改变关系。
- **实际**：DAG节点=1；ANSI/alternate控制字符泄漏=false；中文/emoji/最终摘要可读=false；卡片宽=260px。
- **用户影响**：异常或边界场景缺少可恢复闭环，可能造成工作现场不可用或信息误读。
- **证据**：`/tmp/matou-independent-qa-round3-20260830-103649/groups/E11-r3/evidence/E2E-EDGE-011.png`

### E2E-EDGE-012 [P0] 普通边与 Fork 边视觉/语义区分

- **最短真实复现**：同父真实 Claude Fork + 普通 Shell 混排，白/深主题检查边语义。
- **期望**：1) 查看父标题徽章 → 数量和 Shell/Claude 构成正确；2) 打开 DAG → 两条边均表达父子归属，只 Fork 边带上下文继承标记；3) 切换白/深主题 → 不依赖单一颜色仍可区分；4) 悬浮边或节点查看说明 → 普通关系不声称继承对话。
- **实际**：父徽章="Claude 的子会话\n5 个会话\n← 返回父会话\n⑂ Fork\n＋"；DAG边DOM=[{"cls":"dag-edge relation-forked-from","title":null,"text":""},{"cls":"dag-edge relation-derived-from","title":null,"text":""},{"cls":"dag-edge relation-derived-from","title":null,"text":""},{"cls":"dag-edge relation-derived-from","title":null,"text":""},{"cls":"dag-edge relation-derived-from","title":null,"text":""}]；Fork语义提示=false。
- **用户影响**：异常或边界场景缺少可恢复闭环，可能造成工作现场不可用或信息误读。
- **证据**：`/tmp/matou-independent-qa-round3-20260830-103649/groups/REC2-r3/evidence/E2E-EDGE-012-light.png`；`/tmp/matou-independent-qa-round3-20260830-103649/groups/REC2-r3/evidence/E2E-EDGE-012-dark.png`

## 6. 阻塞项

这些用例缺少本轮可安全、可逆且符合真实前置的外部条件；没有用内部注入或伪造结果替代。

- **E2E-REL-012 [P0] 父 Claude 会话身份创建期失效**：真实 Fork 准备期短于稳定人工失效窗口；移动 transcript 不会使存活父进程身份失效，撤销 Keychain OAuth 会影响用户真实账户。 证据：`/tmp/matou-independent-qa-round3-20260830-103649/environment/blockers.md`
- **E2E-REL-019 [P1] 团队队友会话的 Fork 入口边界**：真实认证账号未提供 Team 队友子会话，无法从实际队友节点验证入口边界。 证据：`/tmp/matou-independent-qa-round3-20260830-103649/environment/blockers.md`
- **E2E-STA-007 [P0] Claude 异常与真实重试**：无仅作用于目标一轮且可撤销的真实账户/网络失败条件；未用伪造网络或 provider。 证据：`/tmp/matou-independent-qa-round3-20260830-103649/environment/blockers.md`
- **E2E-STA-011 [P1] 节点摘要暂时断流**：真实 Runtime 中断会同时停止 PTY，不满足仅摘要断流前置；未注入内部断流。 证据：`/tmp/matou-independent-qa-round3-20260830-103649/environment/blockers.md`
- **E2E-ISO-002 [P0] 应用在子分支/工作树创建中异常退出**：真实小仓库 worktree 准备阶段在人工终止前完成；未使用 I/O mock 延长。 证据：`/tmp/matou-independent-qa-round3-20260830-103649/environment/blockers.md`
- **E2E-ISO-005 [P1] 多显示器上 DAG 的位置**：系统仅有一个活动显示器，缺少第二显示器前置。 证据：`/tmp/matou-independent-qa-round3-20260830-103649/environment/blockers.md`
- **E2E-ISO-007 [P1] DAG 暂时异常时的主路径保留**：无可逆且仅让 DAG BrowserWindow 创建失败的真实 OS 条件。 证据：`/tmp/matou-independent-qa-round3-20260830-103649/environment/blockers.md`
- **E2E-EDGE-016 [P0] 覆盖安装后测试 profile 数据连续**：仅有 commit 446195a 的一个打包版本，缺少同 bundle id 后续真实版本。 证据：`/tmp/matou-independent-qa-round3-20260830-103649/environment/blockers.md`

## 7. 证据与完整性

- 102 条结果的机器可读汇总：`/tmp/matou-independent-qa-round3-20260830-103649/execution-results-102.json`。
- 外部条件审计：`/tmp/matou-independent-qa-round3-20260830-103649/environment/blockers.md`。
- 所有结果证据均位于本轮唯一隔离根；文档生成时已逐路径验证存在。
- 用例正文未改动，仅更新原用例文档第 11 节结果索引。
