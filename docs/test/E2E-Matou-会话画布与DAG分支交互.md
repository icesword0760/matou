# Matou Desktop 会话画布与 DAG 分支交互—端到端测试设计

## 1. 文档信息

| 字段 | 值 |
|---|---|
| 对应 PRD | `docs/prd/PRD-Matou-会话画布与DAG分支交互.md` v1.1 |
| 测试层级 | 真实 Electron App 端到端功能验收 |
| 设计角色 | 资深测试工程师 |
| 文档状态 | 已设计、待开发完成后逐条执行 |
| 用例总数 | 102（P0: 74，P1: 28） |
| 编制日期 | 2026-08-30 |

## 2. 测试目标与准入门槛

### 2.1 用户价值目标

1. 证明用户能在一个事项中完成“建立画布—创建会话—Fork 分支—并行工作—导航回溯—退出与恢复”的完整旅程。
2. 证明 Shell 与 Claude Code 是会话节点的当前运行形态，形态改变时父子、兄弟、画布归属和已有子节点保持原样。
3. 证明横向会话区、父会话徽章、二次右拉、DAG 浮层和独立窗口之间的导航结果一致。
4. 证明排序、状态、聚合徽章和通知只反映真实用户交互与真实会话状态。
5. 证明工作树、应用重启、创建失败和恢复失败均将影响限制在当前节点，并保留用户的本地修改和关系数据。

### 2.2 执行准入

- PRD 和 Spec 的已定稿版本一致；本文第 10 节的待决规格已得到产品决策。
- 测试版 App 可使用独立数据根目录和独立 Electron profile 启动。
- 真实 `zsh`/PTY、真实 Git/worktree、真实 Claude Code CLI 和专用测试身份已就绪。
- macOS 辅助功能、输入监控、系统通知权限已授予测试版 App。
- 测试机上可使用鼠标、触摸板、键盘和第二显示器；高对比和深色主题可切换。

### 2.3 禁止绕过的验证方式

- E2E 执行中使用真实 App 界面、真实 PTY、真实文件系统、真实 Git 命令和真实 Claude Code 会话。
- 不以静态 HTML Mockup、注入前端状态、篡改持久化数据、伪造 PTY 输出或替换 Claude CLI 为测试通过依据。
- 异常用例优先通过真实条件制造，例如移动目录、设置只读权限、创建同名 Git 分支、终止真实进程、使原 Claude 会话身份失效。
- 用例期望以 PRD/Spec 为准。发现与原始需求冲突时，先记录证据并提请产品决策，不以放宽断言或删减步骤消除失败。

## 3. 隔离数据与真实测试夹具

### 3.1 每次执行的唯一目录

```bash
export RUN_ID="$(date +%Y%m%d-%H%M%S)"
export TEST_ROOT="/tmp/matou-e2e-${RUN_ID}"
export MATOU_DATA_DIR="${TEST_ROOT}/matou-data"
export ELECTRON_USER_DATA_DIR="${TEST_ROOT}/electron-user-data"
export WORKSPACE_GIT="${TEST_ROOT}/workspace-git"
export WORKSPACE_NON_GIT="${TEST_ROOT}/workspace-non-git"
mkdir -p "$MATOU_DATA_DIR" "$ELECTRON_USER_DATA_DIR" "$WORKSPACE_GIT" "$WORKSPACE_NON_GIT"
git -C "$WORKSPACE_GIT" init -b main
git -C "$WORKSPACE_GIT" config user.name "Matou E2E"
git -C "$WORKSPACE_GIT" config user.email "matou-e2e@example.invalid"
printf 'baseline\n' > "$WORKSPACE_GIT/baseline.txt"
git -C "$WORKSPACE_GIT" add baseline.txt
git -C "$WORKSPACE_GIT" commit -m "e2e baseline"
printf 'uncommitted\n' > "$WORKSPACE_GIT/uncommitted.txt"
printf 'non-git\n' > "$WORKSPACE_NON_GIT/readme.txt"
```

App 启动时必须将 Runtime 数据指向 `$MATOU_DATA_DIR`，将 Electron profile 指向 `$ELECTRON_USER_DATA_DIR`。执行前后记录用户默认数据目录的文件清单与修改时间，期望没有新增或更新。

### 3.2 共用测试数据

| 代号 | 真实数据/操作 | 用途 |
|---|---|---|
| `WS-GIT` | `$WORKSPACE_GIT`，含一次提交和 `uncommitted.txt` | 共享/新工作树、未提交修改 |
| `WS-NON-GIT` | `$WORKSPACE_NON_GIT` | 新工作树选项限制 |
| `CMD-LONG` | `for i in {1..30}; do echo "line-$i"; sleep 0.2; done` | 运行中、实时摘要、多会话输出 |
| `CMD-FAIL` | `sh -c 'echo real-error >&2; exit 23'` | Shell 错误状态 |
| `CMD-WAIT` | `read -r -p 'enter value: ' value; echo "$value"` | 可明确识别的 Shell 待输入 |
| `CMD-MARK` | `printf 'E2E_MARK_%s\n' "$RUN_ID"` | 校验真实 PTY 输入/输出 |
| `CC-CONTEXT` | 在真实 Claude Code 对话中发送本轮唯一记号，再 Fork 并询问该记号 | 验证真实上下文继承 |
| `CC-WAIT` | 向真实 Claude Code 提交一个会请求文件写入授权的任务 | 待输入/授权状态 |

### 3.3 标准基线场景

- `BASE-EMPTY`：独立 profile 首次启动，一个指向 `WS-GIT` 的工作空间，一个新事项。
- `BASE-CANVAS`：同一事项含两张画布；画布 A 根层 2 个 Shell，画布 B 根层 1 个 Shell。
- `BASE-TREE`：一张画布内存在 `Claude-P`（父）、`Claude-C1`（Fork 子）、`Shell-C2`（普通子）、`Claude-G1`（C1 的 Fork 子），同级再增加 3 个 Shell，用于构造 5+ 会话。
- `BASE-STATES`：使用真实命令和真实 Claude Code 操作形成空闲、运行、待输入、错误、中断、退出六类节点。
- `BASE-LARGE`：通过真实界面操作累积 100 个节点；可使用自动化驱动 UI，每个节点仍由真实创建流程产生。

## 4. 执行规则与证据

1. 每条用例单独记录：App 版本、Git commit、macOS 版本、显示器、主题、输入设备、`RUN_ID`。
2. 每个步骤保留屏幕截图或录屏时间点；状态、顺序、关系和持久化用例同时保留测试 profile 内的诊断导出。
3. 测试结果仅使用 `PASS / FAIL / BLOCKED`；`FAIL` 需记录实际结果、影响用户旅程、证据路径和缺陷编号。
4. 修复后从失败用例的场景初始化开始重跑，再回归同类导航、排序、状态和持久化用例。
5. 用例全部通过后，删除 `$TEST_ROOT`，再次验证用户默认数据没有变化。

## 5. E2E 功能用例

> 用例中“点击/输入/滑动”均指针对真实 App 的用户操作。每条用例默认使用第 3 节的独立 profile。

### A. 首次进入、画布与焦点（8 条）

#### E2E-CAN-001 [P0] 首次进入默认事项
- **目标**：验证新用户立即获得可输入的默认 Shell。
- **前置**：`BASE-EMPTY`，App 从未在该 profile 启动。
- **步骤/期望**：1) 启动 App 并进入默认事项 → 显示一张默认画布和一个 Shell；2) 不点击终端，直接输入 `CMD-MARK` 并回车 → 真实 PTY 输出唯一记号；3) 查看工作目录 → 为 `WS-GIT`。

#### E2E-CAN-002 [P0] 新建画布直接产生 Shell
- **前置**：`E2E-CAN-001` 已完成。
- **步骤/期望**：1) 点击顶部 `+` → 新 Tab 出现并立即选中，无类型/工作树弹框；2) 查看新画布 → 只有一个使用 `WS-GIT` 的 Shell；3) 直接输入 `pwd` → 可提交且路径正确。

#### E2E-CAN-003 [P0] 多画布数据与观察状态隔离
- **前置**：`BASE-CANVAS`。
- **步骤/期望**：1) 在画布 A 选中第二会话并滑到中部 → 记录焦点和横向位置；2) 打开 DAG，平移并缩放到 80%，关闭 → 位置被保留；3) 切换画布 B 并改变其焦点/DAG 观察位置；4) 往返 A/B → 两组节点、焦点、滑动与 DAG 位置各自恢复，输出不串画布。

#### E2E-CAN-004 [P1] 新画布顺序名称与用户命名保留
- **前置**：已有一张画布。
- **步骤/期望**：1) 连续新建两张画布 → 显示“新画布”“新画布 2”等无重名顺序名；2) 将第二张命名为“性能验证”；3) 在 Shell 中改变目录并重启 App → 用户名称保持“性能验证”。

#### E2E-CAN-005 [P0] 进入已有事项恢复上次现场
- **前置**：`BASE-CANVAS`，事项 1 的画布 A 第二会话为上次焦点。
- **步骤/期望**：1) 切换到其他事项；2) 点击事项 1 → 回到画布 A、第二会话及之前横向位置；3) 直接输入 `echo focus-restored` → 内容进入第二会话。

#### E2E-CAN-006 [P0] 新 Shell 创建后自动焦点
- **前置**：任意可用画布。
- **步骤/期望**：1) 用普通横向入口创建 Shell → 新会话追加队尾且完整可见；2) 不点击新窗口直接输入 `CMD-MARK` → 内容只进入新 Shell；3) 验证旧会话没有收到输入。

#### E2E-CAN-007 [P0] Claude Code 回答完成后焦点连续
- **前置**：真实 Claude Code 会话空闲。
- **步骤/期望**：1) 发送一条会正常完成的消息；2) 等待状态回到空闲 → 输入区保持可用；3) 不点击窗口直接输入并提交第二条消息 → 正确发送到同一会话。

#### E2E-CAN-008 [P0] 默认数据零干扰
- **前置**：已记录用户默认 Matou 数据目录快照，测试 profile 存在。
- **步骤/期望**：1) 在测试 profile 创建两张画布、多个会话和工作树；2) 正常退出 App；3) 对比用户默认数据快照 → 文件、修改时间和数据库内容均保持不变；4) 测试数据只存在 `$TEST_ROOT`。

### B. 关系、Shell/Claude 形态与 Fork 工作树（19 条）

#### E2E-REL-001 [P0] 根级横向新建 Shell
- **前置**：默认画布只有一个根 Shell。
- **步骤/期望**：1) 点击标题行横向分栏图标 → 不出现弹框；2) 新 Shell 出现在队尾并自动焦点；3) 打开 DAG → 两节点通过画布起点汇聚，之间没有会话父子边。

#### E2E-REL-002 [P0] 非根层横向新建 Shell 兄弟
- **前置**：`BASE-TREE`，进入 `Claude-P` 的直接子会话列表。
- **步骤/期望**：1) 聚焦 `Claude-C1` 并点击普通横向新增；2) 新 Shell 追加当前列表队尾并获得焦点；3) 打开 DAG → 新 Shell 与 C1/C2 共享 `Claude-P` 为父，普通边没有 Fork 标记。

#### E2E-REL-003 [P0] 混合 Shell/Claude 兄弟列表
- **前置**：`BASE-TREE`。
- **步骤/期望**：1) 点击 `Claude-P` 的子会话徽章；2) 在同一横向列表确认 `Claude-C1` 和 `Shell-C2` 均可见；3) 分别输入真实内容 → 两者独立运行；4) 打开 DAG → 两者拥有同一父节点，节点形态标签正确。

#### E2E-REL-004 [P0] Shell 启动 Claude Code 后形态切换
- **前置**：普通 Shell 空闲，真实 Claude CLI 可用。
- **步骤/期望**：1) 在 Shell 输入 `claude` 并回车 → 标题逐步显示 Claude Code 形态；2) 首轮对话完成前 → Fork 图标可见但置灰，悬浮说明启用条件；3) 完成真实首轮对话 → Fork 入口变为可用；4) DAG 中节点 ID 和关系保持，类型显示更新。

#### E2E-REL-005 [P0] 无有效对话时 Fork 入口约束
- **前置**：Claude Code 刚启动，尚未完成一轮有效对话。
- **步骤/期望**：1) 悬浮分支图标 → 提示“完成首轮对话后可创建分支”；2) 点击图标/用键盘触发 → 创建弹框不出现、节点数不变；3) 完成对话后再点击 → 正常进入 Fork 流程。

#### E2E-REL-006 [P0] Claude Code Fork 子分支继承实际上下文
- **前置**：有效 Claude 父会话，已发送本轮唯一记号和一项约束。
- **步骤/期望**：1) 点击标题行 Fork 图标，选择当前工作树并填写唯一分支名；2) 确定 → 父会话下出现准备中子节点，就绪后自动焦点；3) 在子会话询问唯一记号和约束 → 回答与父会话一致；4) 分别向父/子发送不同后续信息 → 对方不自动获得。

#### E2E-REL-007 [P0] Fork 使用当前工作树
- **前置**：`WS-GIT` 有 `uncommitted.txt`，有效 Claude 父会话使用该目录。
- **步骤/期望**：1) Fork 选择“在当前工作树中创建”；2) 子会话就绪后运行 `pwd` 和 `cat uncommitted.txt` → 路径与父会话相同且可读未提交内容；3) 父子节点都显示“共享工作树”；4) 子会话写入新文件 → 父会话立即可见。

#### E2E-REL-008 [P0] Fork 使用新工作树
- **前置**：`WS-GIT` 有未提交文件，有效 Claude 父会话。
- **步骤/期望**：1) Fork 选择“在新工作树中创建” → 创建前明确告知从最近提交开始；2) 输入唯一分支名并确定 → 节点依次显示准备、就绪；3) 在子会话运行 `pwd`/`git branch --show-current` → 为新目录和新分支；4) `uncommitted.txt` 不在新工作树，原目录中仍存在；5) 父子各自修改文件 → 内容互相隔离。

#### E2E-REL-009 [P0] 非 Git 目录的 Fork 选项
- **前置**：有效 Claude 会话使用 `WS-NON-GIT`。
- **步骤/期望**：1) 打开 Fork 弹框 → “在新工作树中创建”置灰并标注“需要 Git 仓库”；2) 尝试键盘选中该项 → 选择保持不变；3) 选择当前工作树 → Fork 成功；4) 普通新建 Shell 仍然直接完成。

#### E2E-REL-010 [P1] 分支名称冲突
- **前置**：`WS-GIT` 已存在分支 `e2e-duplicate`。
- **步骤/期望**：1) Fork 选择新工作树并输入 `e2e-duplicate`；2) 确定 → 输入框就地显示同名冲突，文本保留，画布节点数不增加；3) 修改为唯一名称并确定 → 创建成功且只有一个新节点。

#### E2E-REL-011 [P0] 新工作树真实创建失败隔离
- **前置**：将测试工作树父目录设为只读；其他兄弟会话正在运行 `CMD-LONG`。
- **步骤/期望**：1) 选择新工作树并确定 → 新节点显示真实失败阶段和原因；2) 确认当前节点和正在输出的兄弟仍可交互；3) 恢复目录权限，点击重试 → 原失败卡片进入准备并成功；4) 另一次失败后点击移除 → 只移除失败节点。

#### E2E-REL-012 [P0] 父 Claude 会话身份创建期失效
- **前置**：有效 Claude 父会话；通过真实会话终止/身份失效条件使 Fork 准备期父身份失效。
- **步骤/期望**：1) 启动 Fork，在创建尚未完成时使原对话身份失效；2) 新节点显示“父会话已失效”与可执行的后续入口；3) 父节点已有输出、已有子节点和其他画布保持；4) 移除失败节点后关系图恢复到创建前。

#### E2E-REL-013 [P0] Fork 兄弟来自共同父会话
- **前置**：`Claude-P` 有子节点 C1，共同父会话身份有效。
- **步骤/期望**：1) 在 C1 使用“从共同父会话 Fork”入口；2) 选择工作树并建立 C2 → C2 追加在 C1 所在列表；3) 在 C1 写入的后续唯一信息不出现在 C2；4) P 中 Fork 前的信息在 C1/C2 均可识别；5) DAG 显示 C1/C2 共享 P 为父。

#### E2E-REL-014 [P0] 多级子分支
- **前置**：有效 Claude 节点 P。
- **步骤/期望**：1) 从 P Fork C1；2) 在 C1 完成新一轮后 Fork G1；3) 在 G1 再 Fork H1；4) 打开 DAG → P→C1→G1→H1 方向和 Fork 标记正确；5) 从各层徽章进入直接子会话 → 只出现直接下一层。

#### E2E-REL-015 [P0] Claude 主动退出回到 Shell
- **前置**：Claude 节点 P 有子节点和分支徽章。
- **步骤/期望**：1) 在 Claude Code 内执行其真实退出操作 → 同一终端回到 Shell，标题更新为 Shell；2) 标题行不出现“Claude Code 已退出”或错误徽章；3) 子节点数量、聚合状态、导航入口和 DAG 连线保持；4) 新 Fork 入口隐藏，横向新建 Shell 可用。

#### E2E-REL-016 [P0] 回到 Shell 后查看已有子节点
- **前置**：`E2E-REL-015` 完成。
- **步骤/期望**：1) 点击仍存在的子会话数量徽章 → 打开原直接子会话列表；2) 选择一个子会话并输入 → 子会话继续工作；3) 返回父会话 → 父节点仍是 Shell，不发生自动 Claude 恢复。

#### E2E-REL-017 [P0] Shell 重新进入有效 Claude 形态
- **前置**：由 Claude 主动退出得到的 Shell 节点，已有子节点。
- **步骤/期望**：1) 在 Shell 再次启动真实 Claude Code；2) 完成一轮有效对话 → Fork 入口再次可用；3) 节点名称、已有子节点、父子边和横向归属保持；4) 新建 Fork 子节点 → 在原子列表队尾出现。

#### E2E-REL-018 [P1] 共享工作树标记的增减
- **前置**：一个会话独占 `WS-GIT`。
- **步骤/期望**：1) 确认单会话时标记不显示；2) 创建共享当前工作树的 Fork → 两个节点均显示共享标记；3) 结束并移除共享节点 → 剩余节点的共享标记按实际占用关系更新；4) 本地目录和修改保留。

#### E2E-REL-019 [P1] 团队队友会话的 Fork 入口边界
- **前置**：在真实 Claude Code 团队会话中出现队友子会话。
- **步骤/期望**：1) 进入队友会话卡片 → 可查看实时状态和摘要；2) 查看标题和操作菜单 → 独立 Fork 子分支入口隐藏；3) 回到团队主会话 → 整体节点仍可按其真实能力创建分支。

### C. 子会话徽章、横向浏览与右拉（10 条）

#### E2E-NAV-001 [P0] 子会话数量、形态构成与聚合状态
- **前置**：父节点含 2 个 Claude、2 个 Shell 直接子节点，状态包含运行和空闲。
- **步骤/期望**：1) 返回父会话 → 标题行数量为 4，Claude/Shell 构成为 2/2；2) 聚合状态显示运行中；3) 使一个子节点进入错误 → 聚合徽章在 1 秒内切换为异常；4) 悬浮 → 显示全部状态数量。

#### E2E-NAV-002 [P0] 点击徽章进入全部直接子会话
- **前置**：父会话有 5 个活动直接子节点。
- **步骤/期望**：1) 点击数量徽章 → 父单会话视图被子会话横向列表替换；2) 校验列表 → 只包含该父的直接子节点，不包含孙节点或其他画布节点；3) 当前选中节点有明确边框和焦点标记。

#### E2E-NAV-003 [P0] 一屏最多四个与第五个横向访问
- **前置**：同级有 5 个活动会话，窗口为标准宽度。
- **步骤/期望**：1) 进入列表 → 最多四个卡片完整显示且内容可读；2) 触摸板向左滑动 → 列表平滑左移并显示第五个；3) 使用鼠标滚轮/底部提示访问首尾 → 结果一致。

#### E2E-NAV-004 [P1] 悬浮扩展与离开恢复
- **前置**：一屏四个会话。
- **步骤/期望**：1) 记录四个卡片顺序和宽度；2) 鼠标悬浮第三个 → 其平滑变宽，相邻卡片适度收缩，无覆盖、无换序；3) 移出列表 → 等宽布局恢复；4) 点击第二个获得焦点 → 适度扩展在移开鼠标后保持。

#### E2E-NAV-005 [P0] 从列表中部一次快速滑到最左
- **前置**：5+ 会话，横向位置在中部。
- **步骤/期望**：1) 用一次大幅度触摸板手势向右滑至最左；2) 松手 → 列表停在最左，父投影和返回文案均不出现；3) 当前仍可与第一个子会话交互。

#### E2E-NAV-006 [P0] 第二次右拉未达阈值回弹
- **前置**：列表已经停在最左，上一次手势已结束。
- **步骤/期望**：1) 开始新的向右拖拽 → 左侧父会话投影随距离增强；2) 在提示“松手返回”出现前松手 → 列表带弹性回到原位；3) 视图仍是子会话列表，焦点不变。

#### E2E-NAV-007 [P0] 第二次右拉超过阈值返回父会话
- **前置**：列表已停在最左。
- **步骤/期望**：1) 新手势向右拉至显示“松手返回父会话”；2) 松手 → 页面切回父会话；3) 直接键入 `CMD-MARK` → 父会话获得输入，证明焦点恢复。

#### E2E-NAV-008 [P0] 明确返回入口与键盘操作
- **前置**：处于子会话列表中部，没有触摸板操作。
- **步骤/期望**：1) 点击标题区“返回父会话” → 立即回到父会话；2) 再进入列表，用 Tab/方向键定位返回入口并回车 → 结果相同；3) 父会话自动聚焦。

#### E2E-NAV-009 [P1] 窄窗口的卡片让位
- **前置**：同级 4 个会话。
- **步骤/期望**：1) 逐步缩窄主窗口 → 当前会话的标题、状态、输入区保留；2) 其他会话收敛为标题、状态和最新摘要；3) 分支数量与错误/待输入信息优先于长工作树路径；4) 窗口恢复后布局恢复。

#### E2E-NAV-010 [P1] 列表边缘持续输出时不误触层级切换
- **前置**：列表在最左，第一个和第五个会话同时运行 `CMD-LONG`。
- **步骤/期望**：1) 观察 3 秒 → 列表不随输出移动；2) 使用细小的横向回弹手势 → 不进入父投影；3) 完成一次清晰的新手势且超过阈值 → 才返回父会话。

### D. DAG 浮层、画布操作与跳转（10 条）

#### E2E-DAG-001 [P0] 长按 Option + Tab 打开系统浮层
- **前置**：App 前台，`BASE-TREE`。
- **步骤/期望**：1) 按住 `Option + Tab` 500ms 左右 → DAG 浮层出现；2) 拖动 Matou 主窗口边缘对比 → 浮层不受主窗口边界裁切；3) 松开按键 → 浮层继续保留；4) `Esc` 关闭。

#### E2E-DAG-002 [P0] 短按 Tab 保持终端原有响应
- **前置**：Shell 光标已聚焦，存在可 Tab 补全的文件名；Claude Code 可执行其 Tab 切换。
- **步骤/期望**：1) Shell 中输入部分文件名并短按 Tab → 原生补全即时响应，DAG 不出现；2) Claude Code 中短按 Tab → Claude 内部行为正常；3) 连续短按 不导致浮层闪现。

#### E2E-DAG-003 [P0] DAG 默认三层与超出层虚影
- **前置**：当前节点上下各有 2+ 层关系。
- **步骤/期望**：1) 以中间节点为当前节点打开 DAG → 父层、当前兄弟层、子层完整显示；2) 更远祖先/后代以虚影和方向提示出现；3) 平移向远层 → 虚影逐渐变实并显示摘要；4) 已经过的近层按视野规则收敛。

#### E2E-DAG-004 [P0] 平移、缩放、边界阻尼与复位
- **前置**：DAG 已打开。
- **步骤/期望**：1) 鼠标拖拽与触摸板任意方向平移 → 连续平滑，节点相对位置和连线不变；2) 用捏合/组合滚轮/按钮缩放至 40% 和 200% → 边界有阻尼且不超出；3) 点击 `100%` → 恢复精确比例；4) 点击“聚焦当前节点” → 当前节点回到视野中心。

#### E2E-DAG-005 [P1] 节点信息完整与缩放让位
- **前置**：节点名、形态、工作树、分支、状态、最近输出和活动时间均已产生。
- **步骤/期望**：1) 100% 查看节点 → 显示名称、Shell/Claude、工作树/分支、状态、最近四行、活动时间；2) 缩小至 40% → 名称、状态和连线优先保留，摘要与时间收起；3) 悬浮节点 → 可查看完整路径。

#### E2E-DAG-006 [P1] 最近四行实时刷新不推移画布
- **前置**：一个 Shell 运行 `CMD-LONG`，DAG 已平移到非默认位置。
- **步骤/期望**：1) 记录缩放、画布坐标和节点边界；2) 观察输出 → 摘要持续更新且始终只显示最近四行；3) 比对位置 → 画布坐标、卡片尺寸和连线稳定；4) 悬浮摘要时输出继续产生，阅读位置不滚动。

#### E2E-DAG-007 [P0] 点击兄弟节点后完整进入视野
- **前置**：同级有 6 个会话，目标节点在横向列表视野外。
- **步骤/期望**：1) DAG 中点击目标兄弟 → 浮层关闭；2) 主界面显示该父节点的兄弟列表；3) 自动滚动使目标卡片完整可见，空间允许时居中；4) 目标获得选中和键盘焦点，不更新最近交互排序。

#### E2E-DAG-008 [P0] 目标节点在布局变化后持续可见
- **前置**：通过 `E2E-DAG-007` 定位到列表边缘的节点。
- **步骤/期望**：1) 鼠标悬浮目标使其扩展 → 横向位置自动微调，卡片仍完整可见；2) 缩窄和放宽主窗口 → 每次布局后目标都保持在视野；3) 直接键入 → 内容进入目标会话。

#### E2E-DAG-009 [P0] 点击父节点恢复单节点视图
- **前置**：当前处于子会话列表。
- **步骤/期望**：1) 打开 DAG 并点击父节点 → 浮层关闭；2) 主界面显示父会话单节点视图；3) 父的标题、子会话徽章、工作树和滚动位置恢复；4) 输入焦点在父会话。

#### E2E-DAG-010 [P1] 100 节点画布可用性
- **前置**：`BASE-LARGE`。
- **步骤/期望**：1) 打开 DAG → 当前三层在可感知的即时反馈内出现；2) 连续平移、缩放、聚焦当前节点各 10 次 → 操作连续且无卡死；3) 选择远处节点 → 弹层关闭并正确定位；4) 同时运行 10 个 `CMD-LONG` → 摘要更新时平移和缩放仍可用。

### E. 会话状态、聚合与用户反馈（12 条）

#### E2E-STA-001 [P0] Shell 空闲—运行—空闲
- **前置**：Shell 已出现提示符。
- **步骤/期望**：1) 确认标题和 DAG 为灰色空闲；2) 提交 `CMD-LONG` → 1 秒内转为绿色运行中且输出持续增加；3) 命令结束并回到提示符 → 回到空闲，光标可直接输入。

#### E2E-STA-002 [P0] Shell 错误后可继续工作
- **前置**：Shell 空闲。
- **步骤/期望**：1) 提交 `CMD-FAIL` → 显示真实 stderr 与红色错误状态；2) 直接提交 `echo recovered` → 新一轮运行状态替换当前错误；3) 命令成功后回到空闲，旧错误输出仍保留。

#### E2E-STA-003 [P0] Shell 明确待输入
- **前置**：Shell 空闲。
- **步骤/期望**：1) 提交 `CMD-WAIT` → 出现明确 prompt 后标题显示琲珀色待输入；2) 父徽章和画布 Tab 同步显示待输入；3) 输入值并回车 → 短暂运行后回到空闲。

#### E2E-STA-004 [P1] Shell 难以识别的交互等待
- **前置**：运行一个不产生标准交互提示、但仍在等待 stdin 的真实程序。
- **步骤/期望**：1) 程序进入等待 → 界面保持运行中，不猜测为待输入；2) 输入值后程序继续；3) 回到 prompt 后转为空闲。

#### E2E-STA-005 [P0] Claude 空闲—运行—空闲
- **前置**：真实 Claude 对话就绪。
- **步骤/期望**：1) 就绪后显示空闲；2) 发送需要读取多个文件的真实任务 → 思考/工具使用期显示运行中；3) 正常完成 → 转为空闲而非待输入；4) 输入焦点保持。

#### E2E-STA-006 [P0] Claude 授权/选择待输入
- **前置**：真实 Claude 对话和会触发权限请求的任务。
- **步骤/期望**：1) 提交 `CC-WAIT` → 工作期为运行中；2) 出现授权、选项或明确提问 → 转为琲珀色待输入，聚合与通知指向当前节点；3) 完成选择 → 转为运行中；4) 任务完成后转空闲。

#### E2E-STA-007 [P0] Claude 异常与真实重试
- **前置**：使用真实网络/账户/工具失败条件使当前一轮异常结束。
- **步骤/期望**：1) 观察异常 → 节点显示红色错误、简短原因和重试入口；2) 恢复真实外部条件，点击重试 → 转为运行中；3) 成功完成后转空闲，其他节点全程可用。

#### E2E-STA-008 [P0] 聚合状态全优先级
- **前置**：`BASE-STATES`，父节点直接子节点覆盖六类状态。
- **步骤/期望**：1) 同时存在错误/待输入/运行 → 聚合显示异常；2) 消除错误 → 显示待输入；3) 处理待输入 → 显示运行中；4) 停止所有运行 → 依次验证已中断、空闲、全部退出的优先级；5) 悬浮计数与实际一致。

#### E2E-STA-009 [P0] 已停止节点的数量与运行统计
- **前置**：父节点有 3 个子节点，其中 1 个会话已停止。
- **步骤/期望**：1) 查看父徽章 → 直接子节点总数为 3；2) 运行统计不将已停止节点计为运行；3) DAG 中它以深灰空心样式存在；4) 同一个已停止节点仍在日常横向列表中，可原位重新启动。

#### E2E-STA-010 [P0] 单会话错误隔离
- **前置**：两个 Shell 和一个 Claude 同时活动。
- **步骤/期望**：1) 在 Shell A 提交 `CMD-FAIL` → 只 A 显示错误；2) Shell B 提交 `echo still-alive` → 正常输出；3) Claude 发送消息 → 正常回答；4) 父/画布聚合显示异常，点击后定位到 A。

#### E2E-STA-011 [P1] 节点摘要暂时断流
- **前置**：DAG 节点已显示最后确认摘要，真实 PTY/runtime 链路被短暂中断。
- **步骤/期望**：1) 中断链路 → 卡片保留最后摘要并显示“信息稍后更新”；2) 节点名、关系、工作树与最后确认状态保留；3) 恢复真实链路 → 新输出自动刷新，画布不跳动。

#### E2E-STA-012 [P1] 状态更新延迟
- **前置**：高分辨率录屏可用。
- **步骤/期望**：1) 在录屏中同时记录命令/消息提交时刻与状态标记变化时刻；2) 分别触发 Shell 运行、Claude 运行、待输入和错误；3) 每个状态的用户可见反馈在 1 秒内出现；4) 父徽章、DAG 和 Tab 在同一刷新窗口内一致。

### F. 最近真实交互排序（9 条）

#### E2E-SORT-001 [P0] Shell 激活期间提交保持原位
- **前置**：同级会话顺序 A/B/C，C 为 Shell。
- **步骤/期望**：1) 在 C 输入但不回车 → 顺序 A/B/C；2) 回车提交 → 顺序仍为 A/B/C；3) 连续输入下一条命令 → 焦点、光标、字符和视野全部保持在 C；4) 激活 B 后，C 再按最近交互一次性更新排序；5) DAG 位置不变。

#### E2E-SORT-002 [P0] Shell 控制操作更新排序
- **前置**：Shell C 在列表末尾运行 `CMD-LONG`。
- **步骤/期望**：1) 向激活的 C 发送真实中断控制操作 → C 保持原位；2) 激活其他卡片 → C 再更新排序；3) 在一个等待 stdin 的 Shell B 完成输入，B 同样在失去激活后更新排序；4) 父子关系保持原样。

#### E2E-SORT-003 [P0] Claude 发送消息时保持原位
- **前置**：Claude C 在列表末尾且空闲。
- **步骤/期望**：1) 在 C 输入草稿 → 顺序不变；2) 发送消息 → C 仍位于原位；3) 等待回答完成 → 顺序不变；4) 用户可继续输入；5) 激活其他卡片后，C 再更新排序。

#### E2E-SORT-004 [P0] Claude 授权、拒绝、选项与停止/继续排序
- **前置**：列表中后位 Claude 分别触发真实授权、选项和运行任务。
- **步骤/期望**：对授权、拒绝、选项、停止和继续分别验证：1) 操作发生时激活卡片保持原位；2) 激活其他卡片后，上一张卡片再按最近交互更新排序；3) 每次只发生一次换位。

#### E2E-SORT-005 [P0] 查看类操作保持顺序
- **前置**：固定顺序 A/B/C。
- **步骤/期望**：1) 依次点击和聚焦 C/B；2) 在 C 选择文本、复制、滚动；3) 打开 DAG，平移、缩放、点击 C 定位；4) 回到列表 → 顺序始终 A/B/C。

#### E2E-SORT-006 [P0] 后台输出、Claude 回答和通知保持顺序
- **前置**：A/B/C 中 B 运行 `CMD-LONG`，C 正在回答，顺序 A/B/C。
- **步骤/期望**：1) 不进行用户提交，观察 B 持续输出 → 顺序不变；2) C 回答完成并转空闲 → 顺序不变；3) B/C 产生状态或通知 → 顺序不变；4) 节点内容和徽章正常更新。

#### E2E-SORT-007 [P0] 新建与恢复会话初始追加队尾
- **前置**：固定顺序 A/B/C。
- **步骤/期望**：1) 横向创建 Shell D → 顺序 A/B/C/D，D 聚焦；2) 创建 Fork E → 当前层 E 在队尾；3) 停止并重新启动一个会话 → 节点身份和列表位置不变；4) 向 D 提交命令 → D 保持队尾；5) 激活其他卡片后，D 首次更新排序。

#### E2E-SORT-008 [P0] 同时间交互的稳定顺序
- **前置**：使用 UI 自动化在同一个应用时钟粒度内向 B/C 快速提交真实命令。
- **步骤/期望**：1) 记录提交先后，激活卡片保持原位；2) 重启 App → 顺序不抖动，pending 排序意图保留；3) 切换卡片后提交上一张卡片的排序，同时间节点维持原有相对顺序。

#### E2E-SORT-009 [P0] 排序与关系/DAG 稳定性
- **前置**：`BASE-TREE`，记录 DAG 节点坐标和连线。
- **步骤/期望**：1) 按 C3、C1、C4 顺序完成真实用户交互，并在每次交互后切换到下一张卡片 → 上一张卡片按最新交互更新排序；2) 打开 DAG → 父子连线、兄弟归属和稳定坐标与之前一致；3) 点击 C1 定位 → 不触发新的排序；4) 重启后已提交顺序和 pending 排序意图均正确恢复。

### G. 停止、移除、关闭、重启与恢复（10 条）

#### E2E-LIFE-001 [P0] 停止带子节点的父会话
- **前置**：父 P 有正在运行的子 C 和空闲子 D。
- **步骤/期望**：1) 从 P 卡片右上角更多菜单选择停止运行；2) P 在横向列表与 DAG 中均转为已停止，同一节点不收起；3) C 继续真实输出，D 仍可输入；4) P→C/P→D 连线保留，P 可原位重新启动。

#### E2E-LIFE-002 [P1] 移除叶子节点
- **前置**：叶子 L 没有子节点。
- **步骤/期望**：1) DAG 中选择 L 并点击移除 → 出现确认且不列出后代；2) 取消 → L 保留；3) 再次确认 → L 和其连线消失，其他关系不变；4) L 的本地工作树仍存在。

#### E2E-LIFE-003 [P1] 有子节点时只允许整支移除
- **前置**：父 P 有子 C。
- **步骤/期望**：1) 从 P 卡片右上角更多菜单选择移除节点；2) 确认框只提供“移除整个分支”或取消，不提供子节点改挂；3) 取消后 P/C 关系与运行状态完全保留。

#### E2E-LIFE-004 [P1] 移除整条分支
- **前置**：P 下有 2 个子、3 个孙与 1 个更深节点，相关工作树存在。
- **步骤/期望**：1) 在 P 选择“移除整条分支” → 确认页列出准确后代数量与重要状态；2) 取消 → 所有节点保留；3) 再次操作并确认 → P 与全部后代/连线消失；4) 检查文件系统 → 每个工作树和未提交修改保留。

#### E2E-LIFE-005 [P0] 关闭含运行/待输入会话的画布并取消
- **前置**：非最后一张画布内分别有运行中和待输入会话。
- **步骤/期望**：1) 关闭画布 → 确认框分别给出受影响总数/关键状态；2) 点击取消 → 画布 Tab、会话输出、焦点和所有状态保持；3) 运行命令继续输出。

#### E2E-LIFE-006 [P1] 确认关闭并恢复已关闭画布
- **前置**：非最后画布含节点关系、自定义名称和工作树。
- **步骤/期望**：1) 关闭并确认 → 活动 Tab 消失，画布进入事项的“已关闭画布”；2) 打开该列表并恢复 → Tab、名称、关系、节点摘要和工作树归属恢复；3) 已停止会话保持已停止样式，本地工作树保留。

#### E2E-LIFE-007 [P0] 最后事项/最后画布关闭窗口只隐藏应用
- **前置**：App 只有一个事项和一张画布，Shell 正运行 `CMD-LONG`。
- **步骤/期望**：1) 点击 macOS 窗口关闭 → 窗口隐藏；2) 等待 2 秒并从 Dock/菜单重新打开 → 回到原画布；3) 命令已继续运行且输出连续，不出现重复执行；4) 焦点返回原会话。

#### E2E-LIFE-008 [P0] 应用完整退出后关系和观察位置恢复
- **前置**：多画布、多层关系，已设置特定当前画布、节点、横向位置和 DAG 缩放/平移。
- **步骤/期望**：1) 记录现场并通过应用菜单完整退出；2) 用同一测试 profile 重启 → 恢复工作空间、事项、Tab、名称、关系、节点和顺序；3) 当前画布/焦点/横向位置与退出前一致；4) 打开 DAG → 缩放与观察位置一致。

#### E2E-LIFE-009 [P0] 完整退出时 Shell 命令中断且不重复执行
- **前置**：Shell 运行一个会每秒向 `$TEST_ROOT/counter.log` 追加唯一行的真实长命令。
- **步骤/期望**：1) 记录已追加行数并完整退出 App；2) 等待 2 秒后记录行数；3) 重启 App → Shell 已有输出和中断标记出现，命令不自动重新运行；4) 继续等待 → `counter.log` 行数不再增长；5) 用户手动重新提交后才继续追加。

#### E2E-LIFE-010 [P0] Claude 恢复失败、重试与关系保留
- **前置**：Claude 节点 P 有子节点；记录节点名、父子边、兄弟位置；使原 Claude 会话以真实方式失去恢复条件。
- **步骤/期望**：1) 完整退出并重启 → P 当前呈现 Shell，标题显示“Claude Code 恢复失败”、真实原因和“重试恢复”；2) 在 P 中运行 `echo shell-still-usable` → Shell 可用；3) 通过徽章进入子节点 → 关系与子会话可用；4) 保持失败条件点击重试 → 显示恢复中后继续显示原因与重试；5) 恢复真实对话条件再点击 → 回到 Claude Code，节点身份、关系和横向位置不变。

### H. 目录、异常退出、独立窗口、主题与大规模（8 条）

#### E2E-ISO-001 [P0] 工作空间目录改名/移动后受限与恢复
- **前置**：已有指向 `WS-GIT` 的多画布关系。
- **步骤/期望**：1) 退出 App，在 Finder/终端将 `WS-GIT` 移动到另一测试路径；2) 重启 → 画布、已有输出和 DAG 可查看，输入与创建入口置灰并提示恢复目录；3) 选择移动后的真实目录 → 各节点恢复可输入，关系和工作树归属不变；4) 新建 Shell 的 `pwd` 为新路径。

#### E2E-ISO-002 [P0] 应用在子分支/工作树创建中异常退出
- **前置**：通过真实大仓库或受控 I/O 条件使工作树准备阶段可观察。
- **步骤/期望**：1) 创建新工作树 Fork，节点显示正在准备；2) 直接终止 App 进程；3) 重启同一 profile → 该节点显示“创建已中断”与重试/移除；4) 重试 → 继续真实创建且不产生重复节点；5) 重复场景选择移除 → 清理半完成关系卡片，原工作树与用户修改保留。

#### E2E-ISO-003 [P0] 单节点恢复失败不扩散
- **前置**：两张画布各有多节点，仅使其中一个会话的真实恢复条件失效。
- **步骤/期望**：1) 重启 App → 只目标节点显示恢复错误；2) 其兄弟提交真实 Shell/Claude 输入 → 正常；3) 切换另一画布并操作 → 正常；4) 失败节点的关系、已有输出和重试入口保留。

#### E2E-ISO-004 [P1] 独立窗口节点的 DAG 关系与唤起
- **前置**：用真实 UI 将节点 C 移到独立窗口，窗口在其他窗口后方。
- **步骤/期望**：1) 主窗口打开 DAG → C 保持原画布的父子与兄弟位置，状态实时；2) 点击 C → DAG 关闭，C 的独立窗口到前台并获得输入焦点；3) 关闭 C 独立窗口后再在 DAG 选择 → 显示同一个已停止节点与可执行的重新启动入口。

#### E2E-ISO-005 [P1] 多显示器上 DAG 的位置
- **前置**：两个显示器，Matou 主窗口在显示器 2。
- **步骤/期望**：1) 长按 Option + Tab → DAG 出现在显示器 2；2) 再次尝试打开 → 同一时间只有一个浮层；3) 关闭后将主窗口移到显示器 1 再打开 → 浮层跟随新主窗口所在屏幕。

#### E2E-ISO-006 [P0] 白色与深色主题的关系可读性
- **前置**：`BASE-STATES`，同时含选中、悬浮、普通/Fork 连线、已停止、虚影节点。
- **步骤/期望**：1) 白色主题逐项查看 Shell/Claude、各状态、焦点、悬浮、已停止节点和连线 → 文字、边界与层级清晰；2) 切换深色主题 → 信息语义和数量不变，视觉仍可区分；3) 对两个主题截图检查状态颜色与 PRD 约定一致。

#### E2E-ISO-007 [P1] DAG 暂时异常时的主路径保留
- **前置**：通过真实 DAG 窗口创建/系统窗口失败条件使浮层未成功打开。
- **步骤/期望**：1) 长按快捷键 → 用户收到清晰的 DAG 打开异常反馈；2) 点击父徽章 → 子会话列表可用；3) 横向滑动、明确返回按钮和分支徽章继续可用；4) 恢复系统窗口条件后 DAG 可正常打开。

#### E2E-ISO-008 [P1] 10+ 会话同时真实输出
- **前置**：同一事项有两张画布，合计至少 10 个 Shell。
- **步骤/期望**：1) 逐个提交带不同前缀的 `CMD-LONG`；2) 在两张画布之间往返、横向滑动、打开 DAG → 主界面持续响应；3) 每个会话输出只含自己前缀，无串流；4) 焦点、横向位置和排序不被后台输出改变；5) 命令结束后所有状态回到空闲。

## 6. 补充异常与边界用例（16 条）

> 以下用例补足 PRD 第 7 节、状态机和质量门槛中未被上述主旅程完整表达的分支。

#### E2E-EDGE-001 [P0] 新画布 Shell 启动失败
- **前置**：通过真实无执行权限 Shell/不可用 cwd 条件触发启动失败。
- **步骤/期望**：1) 新建画布 → Tab 保留，显示启动失败原因与重新创建；2) 切换原画布 → 原会话可用；3) 恢复真实条件后重试 → Shell 成功就绪并自动焦点。

#### E2E-EDGE-002 [P0] 横向 Shell 创建失败卡片
- **前置**：使用真实 PTY 资源/目录失效条件。
- **步骤/期望**：1) 横向新增 Shell → 队尾显示失败卡片与原因；2) 当前会话焦点和其他兄弟保持；3) 重试成功后仍位于队尾；4) 重现并移除 → 只移除失败卡片。

#### E2E-EDGE-003 [P1] 只有一个节点的 DAG
- **前置**：画布只有默认 Shell。
- **步骤/期望**：1) 打开 DAG → 当前节点居中，父层和子层为空，画布起点的组织语义清楚；2) 缩放/平移/聚焦按钮均可用；3) 点击当前节点 → 回到原会话并获得焦点。

#### E2E-EDGE-004 [P1] 超过三层关系的逐渐加载
- **前置**：至少 7 层真实节点链。
- **步骤/期望**：1) 在第 4 层打开 DAG → 默认只完整显示父/当前/子三层；2) 向祖先方向平移 → 虚影渐进变实；3) 再向后代方向平移 → 另一端按相同规则加载；4) 任一层可点击跳转。

#### E2E-EDGE-005 [P0] 工作树与未提交修改在结束/关闭后保留
- **前置**：原工作树与新工作树均有未提交唯一文件。
- **步骤/期望**：1) 结束使用新工作树的会话；2) 关闭其所在画布；3) 移除符合条件的节点；4) 通过真实文件系统和 `git status` 检查 → 两个工作树、分支和未提交内容均保留。

#### E2E-EDGE-006 [P0] Claude 主动退出后重启不自动恢复
- **前置**：真实 Claude 会话已主动退出回到 Shell，标题不显示错误。
- **步骤/期望**：1) 完整退出 App 并重启；2) 节点以 Shell 恢复，没有 Claude 自动启动输出、恢复中或恢复失败徽章；3) 旧会话输出和子节点可查看；4) Shell 可直接输入。

#### E2E-EDGE-007 [P1] 关闭过程中取消并保持工作现场
- **前置**：运行中 Claude 和待输入 Shell 同属待关闭画布。
- **步骤/期望**：1) 记录两节点输出、草稿、滚动与焦点；2) 触发关闭后点击取消；3) 确认所有内容、草稿、状态、焦点、滚动位置与之前一致；4) 两个真实进程继续存活。

#### E2E-EDGE-008 [P1] 长路径与标题空间让位
- **前置**：工作树位于 200+ 字符路径，父节点有异常子会话和多个操作入口。
- **步骤/期望**：1) 标准宽度观察标题 → 会话名、数量、异常优先完整，路径截断；2) 悬浮路径 → 显示完整值；3) 缩窄窗口 → 创建入口收拢到操作菜单，数量和错误仍可见。

#### E2E-EDGE-009 [P0] 键盘焦点跨全部关键导航
- **前置**：键盘操作，存在父、5 个子节点和多张画布。
- **步骤/期望**：1) 新建画布 → 新 Shell 可直接输入；2) 新建兄弟 → 新节点可直接输入；3) 创建 Fork → 新 Claude 可直接输入；4) 用明确返回入口返回父节点 → 父可直接输入；5) 通过 DAG 跳转视野外节点 → 目标可直接输入。

#### E2E-EDGE-010 [P0] 父节点已有输出节点下子会话状态更新
- **前置**：已退出父 P 下有运行子 C 和待输入子 D。
- **步骤/期望**：1) DAG 观察 P/C/D → P 为已停止样式，C/D 为真实当前状态；2) 处理 D 并等待 C 结束 → 两子节点状态实时更新；3) P 仍为已停止，连线不变；4) 从 D 导航回 P → 显示已停止节点详情而非活动终端。

#### E2E-EDGE-011 [P1] DAG 节点摘要输出边界
- **前置**：会话分别输出空行、ANSI 颜色、超长单行、中文、emoji 和 alternate-screen 内容。
- **步骤/期望**：1) 打开 DAG → 最近四行文字可读，无 ANSI 控制字符泄漏；2) 超长行按视觉规则截断，不扩大卡片；3) 中文/emoji 不乱码；4) alternate-screen 更新不制造重复节点或改变关系。

#### E2E-EDGE-012 [P0] 普通边与 Fork 边视觉/语义区分
- **前置**：同一父节点下同时有普通 Shell 子节点和 Claude Fork 子节点。
- **步骤/期望**：1) 查看父标题徽章 → 数量和 Shell/Claude 构成正确；2) 打开 DAG → 两条边均表达父子归属，只 Fork 边带上下文继承标记；3) 切换白/深主题 → 不依赖单一颜色仍可区分；4) 悬浮边或节点查看说明 → 普通关系不声称继承对话。

#### E2E-EDGE-013 [P1] 已关闭画布的多次恢复与重新关闭
- **前置**：一张自定义名称的已关闭画布。
- **步骤/期望**：1) 恢复画布 → 只产生一个 Tab；2) 再次从已关闭画布入口触发恢复 → 不产生重复 Tab/节点；3) 再次关闭并重启 App；4) 已关闭列表中仍只有一条，名称和关系不变。

#### E2E-EDGE-014 [P1] 主题切换时工作状态保持原样
- **前置**：一个会话运行，一个待输入，一个输入框有未提交草稿。
- **步骤/期望**：1) 白色切换深色 → 进程不中断，待输入仍定位原节点，草稿完整；2) 深色切回白色 → 横向位置、DAG 位置和焦点不变；3) 草稿仍未提交，列表顺序不变。

#### E2E-EDGE-015 [P0] 关系唯一父约束
- **前置**：画布内有多个可作为父节点的 Claude/Shell 节点。
- **步骤/期望**：1) 从 P1 创建普通子节点 C；2) 从 P2 执行多次创建/导航操作；3) DAG 检查 C → 始终只有一个直接父 P1；4) Shell/Claude 形态切换、排序和重启后再检查 → 父关系不重写、不复制。

#### E2E-EDGE-016 [P0] 覆盖安装后测试 profile 数据连续
- **前置**：使用已打包测试版和独立 profile，存在多画布、多层关系、自定义名、已关闭画布、已停止节点和工作树。
- **步骤/期望**：1) 记录完整现场并退出；2) 用同一应用标识覆盖安装后续测试版；3) 使用原测试 profile 启动 → 所有事项、画布、节点、关系、名称、顺序、已有输出和工作树归属保留；4) 用户默认数据目录仍没有变化。

## 7. 非功能与可用性记录项

- **性能**：DAG 跳转从点击到明确焦点反馈目标 `<300ms`；状态可见延迟 `<1s`。100 节点和 10 并发输出场景记录 p50/p95 响应、CPU、内存与 Main/Renderer 卡顿。
- **稳定性**：每条关键旅程连续重复 3 次；排序、焦点、手势与恢复不出现偶发偏差。
- **可访问性**：操作入口可通过键盘到达；焦点顺序与画面一致；状态不只依赖颜色；缩放文字后信息不遮挡。
- **视觉一致性**：白/深主题的同一会话、状态、连线和选中态必须语义一致；重点检查白色主题下列表层级区分。
- **数据安全**：任一结束、移除、关闭、创建失败和升级路径后，对比工作树文件 hash 与 `git status`，记录丢失/篡改数必须为 0。

## 8. 逐条执行结果模板

> 第三步真实验收时，复制下表为每条用例生成执行记录，原用例本文保持。

| 字段 | 记录内容 |
|---|---|
| 用例 ID | `E2E-...` |
| 执行结果 | `PASS / FAIL / BLOCKED` |
| 实际结果 | 用户实际看到和完成的结果 |
| 证据 | 截图/录屏/诊断导出/文件系统检查路径 |
| 缺陷 | 缺陷 ID、用户影响、复现概率 |
| 重试记录 | 修复 commit、重跑次数、最终结果 |

## 9. 退出门槛

1. 102 条 E2E 用例全部 `PASS`，没有 P0/P1 已知缺陷。
2. PRD 的 F1–F19、验收场景 1–50、用户可感知行为 1–74、边界/异常和质量门槛都有已通过的证据。
3. 同一受测 commit 完成 `pnpm test`、`pnpm typecheck`、`pnpm build`、新增 E2E 自动化和已打包 App 验收。
4. 默认用户数据目录在执行前后一致；所有测试产物都在 `$TEST_ROOT` 和仓库的测试证据目录。
5. 使用静态 Mockup、前端状态注入或伪造 PTY/Claude 输出得到的结果不计入通过数。

## 10. 测试设计发现并由 Spec 收敛的规格问题

| ID | 收敛后的产品规则 | 影响用例 |
|---|---|---|
| SPEC-Q01 | 徽章统一称“子会话”，总数包含运行与已停止的全部直接子节点；状态分布单独统计。 | NAV-001/002, EDGE-012 |
| SPEC-Q02 | 根层隐藏 Fork 兄弟入口；根层 Claude 仍可创建自己的 Fork 子会话。 | REL-001/013 |
| SPEC-Q03 | 横向列表与 DAG 投影同一节点集合；已停止节点在两侧同时显示，不提供历史会话开关或单独历史列表。 | NAV-001/002, STA-009 |
| SPEC-Q04 | 短按向终端转发一次 Tab；长按默认 450ms，设置范围 350–800ms。 | DAG-001/002 |
| SPEC-Q05 | Scene Tab 尾部提供 `会话关系 (⌥Tab)` 图形按钮并支持键盘访问。 | DAG-001, NAV-008 |
| SPEC-Q06 | DAG 搜索进入首版，匹配名称、目录、worktree 分支和最近摘要，并可居中/进入节点。 | DAG-010 |
| SPEC-Q07 | 复用 PRD 05 的 `↗ 独立窗口`；关闭后同一 live Session 返回，DAG 点击时激活独立窗口。 | ISO-004 |
| SPEC-Q08 | 显示名去除首尾空白后为 1–64 个 Unicode 字符，同父活跃节点内区分大小写唯一；Git 分支名由 Runtime 安全生成。 | REL-008/010/011 |
| SPEC-Q09 | 首条真实用户消息、持久 provider 身份和首轮正常完成共同启用 Fork。 | REL-004/005/006 |
| SPEC-Q10 | 恢复失败作为节点附加状态，聚合优先级等同错误，并进入现有错误通知链。 | LIFE-010, STA-008 |
| SPEC-Q11 | 关闭最后主窗口只隐藏；`Cmd+Q`/菜单/Dock Quit 完整退出；强制结束使用同一 stale-run 恢复语义。 | LIFE-007/008/009, ISO-002 |
| SPEC-Q12 | App 整体恢复保留原顺序；已停止节点原位重新启动，不创建新节点、不改变列表位置。 | SORT-007/009, LIFE-008 |
| SPEC-Q13 | Runtime 事务内生成持久单调交互序号；相同序号按创建序号和 Session ID 稳定排序。 | SORT-008/009 |
| SPEC-Q14 | 异常通过临时 Git/文件权限、真实 setup script、真实 provider 身份失效和真实 Runtime 终止构造；测试配置只选择隔离根目录。 | REL-011/012, ISO-002/007 |
| SPEC-Q15 | 复用 PRD 01 通知视觉和定位；恢复失败、待输入、错误都可从通知跳转到目标节点。 | STA-006/008/010 |
| SPEC-Q16 | 纯 Shell 没有子列表时仅提供同层新增；节点已有子列表时，列表横向 `+` 可添加共享该父节点的普通 Shell 子会话。 | REL-002/014/015, EDGE-015 |

## 11. 最终独立黑盒执行结果索引

> 完整逐项记录见 [`EXECUTION-Matou-会话画布与DAG分支交互.md`](./EXECUTION-Matou-会话画布与DAG分支交互.md)。最终合计：**PASS 101 / FAIL 0 / HARDWARE BLOCKED 1 / 未执行 0**。

| 用例 ID | 结果 | 执行记录 | 主要证据 |
|---|---|---|---|
| E2E-CAN-001 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/SHELL-CORE-r3b/evidence/E2E-CAN-001.png` |
| E2E-CAN-002 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/SHELL-CORE-r3b/evidence/E2E-CAN-002.png` |
| E2E-CAN-003 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/CN6-r3/evidence/suite-results.json` |
| E2E-CAN-004 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/CN6-r3/evidence/E2E-CAN-004.png` |
| E2E-CAN-005 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/CN6-r3/evidence/E2E-CAN-005.png` |
| E2E-CAN-006 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/SHELL-CORE-r3b/evidence/E2E-REL-001.png` |
| E2E-CAN-007 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/CF4-r3/evidence/E2E-CAN-007.png` |
| E2E-CAN-008 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/SHELL-CORE-r3b/evidence/E2E-CAN-001.png` |
| E2E-REL-001 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/SHELL-CORE-r3b/evidence/E2E-REL-001.png` |
| E2E-REL-002 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/CF4-r3/evidence/E2E-REL-002.png` |
| E2E-REL-003 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/CF4-r3/evidence/E2E-REL-002.png` |
| E2E-REL-004 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/CF4-r3/evidence/E2E-REL-004.png` |
| E2E-REL-005 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/CF4-r3/evidence/E2E-REL-005.png` |
| E2E-REL-006 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/CF4-r3/evidence/E2E-REL-006.png` |
| E2E-REL-007 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/CF4-r3/evidence/E2E-REL-006.png` |
| E2E-REL-008 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/REL008-A7E/evidence/E2E-REL-008.png` |
| E2E-REL-009 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/NG3-r3/evidence/E2E-REL-009.png` |
| E2E-REL-010 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/CF4-r3/evidence/E2E-REL-010-wt.png` |
| E2E-REL-011 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/REL011-removefresh/evidence/E2E-REL-011-remove-before.png` |
| E2E-REL-012 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/REL012-R5B/evidence/E2E-REL-012-before-remove.png` |
| E2E-REL-013 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/R13/evidence/E2E-REL-013.png` |
| E2E-REL-014 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/R14B/evidence/E2E-REL-014-final2.png` |
| E2E-REL-015 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/R1516-current/evidence/E2E-REL-015.png` |
| E2E-REL-016 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/R1516-current/evidence/E2E-REL-016.png` |
| E2E-REL-017 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/R17/evidence/E2E-REL-017.png` |
| E2E-REL-018 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/SHARE-r3/evidence/E2E-REL-018.png` |
| E2E-REL-019 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/REL019-A7/evidence/E2E-REL-019-final.png` |
| E2E-NAV-001 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/SHARE-r3/evidence/E2E-NAV-001-exact.png` |
| E2E-NAV-002 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/CF4-r3/evidence/E2E-NAV-001.png` |
| E2E-NAV-003 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/CF4-r3/evidence/E2E-NAV-003.png` |
| E2E-NAV-004 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/SHELL-CORE-r3b/evidence/E2E-NAV-004.png` |
| E2E-NAV-005 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/CF4-r3/evidence/E2E-NAV-005.png` |
| E2E-NAV-006 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/CF4-r3/evidence/E2E-NAV-006.png` |
| E2E-NAV-007 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/CF4-r3/evidence/E2E-NAV-007.png` |
| E2E-NAV-008 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/CF4-r3/evidence/E2E-NAV-008.png` |
| E2E-NAV-009 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/SHELL-CORE-r3b/evidence/E2E-NAV-009.png` |
| E2E-NAV-010 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/GRAPH-r4/evidence/E2E-NAV-010.png` |
| E2E-DAG-001 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/CF4-r3/evidence/E2E-DAG-001.png` |
| E2E-DAG-002 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/SHELL-CORE-r3b/evidence/E2E-DAG-002.png` |
| E2E-DAG-003 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/R14B/evidence/E2E-DAG-003-before.png` |
| E2E-DAG-004 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/SHELL-CORE-r3b/evidence/E2E-DAG-004.png` |
| E2E-DAG-005 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/R14B/evidence/E2E-DAG-005-100-final.png` |
| E2E-DAG-006 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/SHELL-CORE-r3b/evidence/E2E-DAG-006.png` |
| E2E-DAG-007 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/CF4-r3/evidence/E2E-DAG-007.png` |
| E2E-DAG-008 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/CF4-r3/evidence/E2E-DAG-008.png` |
| E2E-DAG-009 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/R14B/evidence/E2E-DAG-009.png` |
| E2E-DAG-010 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/BIG-A7/evidence/E2E-DAG-010.png` |
| E2E-STA-001 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/SHELL-CORE-r3b/evidence/E2E-STA-001.png` |
| E2E-STA-002 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/SHELL-CORE-r3b/evidence/E2E-STA-002.png` |
| E2E-STA-003 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/SHELL-CORE-r3b/evidence/E2E-STA-003.png` |
| E2E-STA-004 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/SHELL-CORE-r3b/evidence/E2E-STA-004.png` |
| E2E-STA-005 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/STA005-A7/evidence/E2E-STA-005.png` |
| E2E-STA-006 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/P6D/evidence/E2E-STA-006.png` |
| E2E-STA-007 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/STA007-R5E/evidence/E2E-STA-007-error.png` |
| E2E-STA-008 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/CF4-r3/evidence/E2E-STA-008-perm.png` |
| E2E-STA-009 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/STA009-R5B/evidence/E2E-STA-009.png` |
| E2E-STA-010 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/P10D/evidence/E2E-STA-010-dag.png` |
| E2E-STA-011 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/RUNTIMEBREAK-R5B/evidence/E2E-STA-011-break.png` |
| E2E-STA-012 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/SHELL-CORE-r3b/evidence/E2E-STA-012.png` |
| E2E-SORT-001 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/SHELL-CORE-r3b/evidence/E2E-SORT-001.png` |
| E2E-SORT-002 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/SORT2-r3/evidence/E2E-SORT-002.png` |
| E2E-SORT-003 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/STA005-A7/evidence/E2E-SORT-003.png` |
| E2E-SORT-004 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/P6D/evidence/E2E-SORT-004.png` |
| E2E-SORT-005 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/SHELL-CORE-r3b/evidence/E2E-SORT-005.png` |
| E2E-SORT-006 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/SHELL-CORE-r3b/evidence/E2E-SORT-006.png` |
| E2E-SORT-007 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/SHELL-CORE-r3b/evidence/E2E-SORT-007.png` |
| E2E-SORT-008 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/CN6-r3/evidence/E2E-SORT-008.png` |
| E2E-SORT-009 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/GRAPH-r4/evidence/E2E-EDGE-012-light.png` |
| E2E-LIFE-001 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/R13/evidence/E2E-LIFE-001-current.png` |
| E2E-LIFE-002 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/R14B/evidence/E2E-LIFE-002-r5.png` |
| E2E-LIFE-003 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/R13/evidence/E2E-LIFE-003-current.png` |
| E2E-LIFE-004 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/LIFE004-R5D/evidence/E2E-LIFE-004-cancel.png` |
| E2E-LIFE-005 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/LIF3-A7/evidence/close-modal.png` |
| E2E-LIFE-006 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/LIF3-A7/evidence/E2E-LIFE-006-retest.png` |
| E2E-LIFE-007 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/OSL1-r3/evidence/E2E-LIFE-007.png` |
| E2E-LIFE-008 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/CN6-r3/evidence/suite-results.json` |
| E2E-LIFE-009 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/OSL4-r3/evidence/E2E-LIFE-009.png` |
| E2E-LIFE-010 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/REC2-r3/evidence/E2E-LIFE-010.png` |
| E2E-ISO-001 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/ISO001-R5/evidence/E2E-ISO-001.png` |
| E2E-ISO-002 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/CF4-r3/evidence/E2E-ISO-002-current.png` |
| E2E-ISO-003 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/REC2-r3/evidence/E2E-ISO-003.png` |
| E2E-ISO-004 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/ISO004-R5B/evidence/E2E-ISO-004-front.png` |
| E2E-ISO-005 | **BLOCKED** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/ISO005-HARDWARE/evidence/display-topology.txt` |
| E2E-ISO-006 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/ISO006-R5F/evidence/E2E-ISO-006-light.png` |
| E2E-ISO-007 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/RUNTIMEBREAK-R5B/evidence/E2E-STA-011-break.png` |
| E2E-ISO-008 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/ISO8-r4/evidence/E2E-ISO-008.png` |
| E2E-EDGE-001 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/FAILSHELL1-R5B/evidence/E2E-EDGE-001.png` |
| E2E-EDGE-002 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/FAILSHELL2-R5B/evidence/E2E-EDGE-002.png` |
| E2E-EDGE-003 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/SHELL-CORE-r3b/evidence/E2E-EDGE-003.png` |
| E2E-EDGE-004 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/R14B/evidence/E2E-EDGE-004-before-b.png` |
| E2E-EDGE-005 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/REL008-A7E/evidence/E2E-EDGE-005-final.png` |
| E2E-EDGE-006 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/CEX-r3/evidence/E2E-EDGE-006.png` |
| E2E-EDGE-007 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/LIF3-A7/evidence/E2E-LIFE-005.png` |
| E2E-EDGE-008 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/LONG-r4/evidence/E2E-EDGE-008.png` |
| E2E-EDGE-009 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/R14B/evidence/E2E-EDGE-009-final.png` |
| E2E-EDGE-010 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/EDGE010B-current/evidence/E2E-EDGE-010-before.png` |
| E2E-EDGE-011 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/E11-r3/evidence/E2E-EDGE-011.png` |
| E2E-EDGE-012 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/GRAPH-r4/evidence/E2E-EDGE-012-light.png` |
| E2E-EDGE-013 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/LIF3-A7/evidence/E2E-EDGE-013-retest.png` |
| E2E-EDGE-014 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/LIF3-A7/evidence/theme-dark.png` |
| E2E-EDGE-015 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/GRAPH-r4/evidence/E2E-EDGE-012-light.png` |
| E2E-EDGE-016 | **PASS** | [查看](./EXECUTION-Matou-会话画布与DAG分支交互.md) | `/tmp/mqa5d-181108/groups/UPGRADE-A7/evidence/E2E-EDGE-016.png` |
