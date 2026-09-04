# Matou AI 结构控制设计

**日期：** 2026-09-03  
**状态：** 实现完成，待产品验收<br>
**范围：** 在现有终端读取与输入能力之上，让 Matou 托管的 AI 会话通过自然语言创建、Fork、移除、关闭和导航产品层级

## 1. 目标

用户可以直接对 Matou 内的 Claude Code 或 Codex 描述结构操作，由 AI 识别当前上下文、补齐必要决策并完成操作。核心场景是：

> AI 给出三个实现方案后，用户说“根据这三个方案创建三个子节点”。AI 总结三个方案的标题，确认每个节点的分支与 Worktree 策略，在当前会话下创建三个子节点，并汇报逐项结果。

本期新增以下自然语言能力：

- 创建工作空间、事项、画布和会话；
- 创建子节点、同级节点以及批量子节点；
- Fork 会话并继承上下文；
- 移除工作空间、事项、会话节点或会话子树；
- 关闭画布；
- 聚焦会话；
- 切换工作空间、事项和画布；
- 跨窗口定位、切换和聚焦。

## 2. 产品原则

1. 自然语言操作和界面操作共用 Runtime 权威数据及业务服务，结果保持一致。
2. AI 负责理解用户意图、总结标题和发起必要询问；Runtime 负责校验目标、权限、结构版本和最终状态。
3. 创建、Fork、聚焦和切换在目标唯一且参数完整时直接执行。
4. 移除与关闭先展示影响范围，用户明确确认后执行。
5. Git 分支与 Worktree 由用户决定；AI 可以提出建议，但在用户确认前不创建。
6. 批量操作保留成功项、继续处理剩余项，并支持仅重试失败项。
7. 默认保持用户当前焦点和画布位置；用户明确要求进入目标时才切换。
8. 用户可见术语统一使用“移除”。画布使用“关闭画布”。
9. 项目目录和 Git 仓库文件不随层级移除而变化；Worktree 清理由独立动作处理。
10. 会话的模型与权限模式持久化，并在重启、恢复和 Fork 后继承。

## 3. 方案选择

### 3.1 采用方案：扩展现有 Host Control

扩展 Runtime 内现有 Host Control，在终端读取与输入能力旁增加一层高层产品动作。高层动作解析稳定目标引用，调用当前层级、会话画布、Fork、Git Worktree 和导航服务，再把权威结果返回给 AI。

调用链路：

```text
用户自然语言
  → Claude Code / Codex 会话级规则
  → mt 高层产品命令
  → Host Control 身份、能力和结构版本校验
  → 现有 Runtime 业务服务
  → 权威投影与界面状态更新
  → 结构化结果
  → AI 自然语言反馈
```

该方案保持 Runtime 为唯一写入权威，也让手动 `mt` 调用与自然语言调用共享同一行为。

### 3.2 其他路径

- 直接暴露底层 Runtime RPC：内部参数过多，AI 容易混淆窗口、画布、提交标识和 Worktree 状态，接口演进也会直接影响用户体验。
- 通过界面自动点击完成操作：依赖窗口前台状态，批量和跨窗口操作容易干扰用户，后台执行与结果确认也较弱。

## 4. 能力模型

### 4.1 新增能力域

Host Control 增加下列显式能力，继续沿用 run-bound capability token：

```text
structure.create.workspace
structure.create.task
structure.create.canvas
structure.create.session
structure.fork.child
structure.fork.sibling
structure.fork.children
structure.remove.preview
structure.remove.commit
structure.canvas-close.preview
structure.canvas-close.commit
navigation.focus.session
navigation.switch.workspace
navigation.switch.task
navigation.switch.canvas
```

每项能力独立授权。内部控制凭证随会话进程启动自动生成，进程结束后作废；进程重启时自动生成新凭证。该凭证与用户可见的会话权限模式分离。

### 4.2 用户权限与会话继承

- 默认权限、Auto 和开放所有权限等用户设置保存在会话配置中；
- App 重启、会话恢复和进程重启后继续使用原设置；
- Fork 子节点继承父会话的 provider、模型、权限模式和已存在的对话上下文；
- 用户随后在某个子节点修改模型或权限时，只影响该子节点；
- Host Control 凭证更新不会改变模型、权限模式或会话设置。

### 4.3 注入时机

- 所有 Matou 托管 Shell、Claude Code 和 Codex 会话获得 `mt` CLI 与本次运行的控制身份；
- Claude Code 和 Codex 额外加载自然语言规则；
- 新建、恢复、Fork、重试以及 App 重启后恢复的 provider 进程都加载当前版本规则；
- 规则只作用于 Matou 托管会话，不写入用户全局 provider 配置。

## 5. `mt` 高层命令

保留现有 `identify/list/read/history/commands/send/key`，新增以下命令族：

```text
mt create workspace --path PATH [--title TITLE]
mt create task --workspace TARGET [--title TITLE]
mt create canvas --task TARGET [--title TITLE]
mt create session --canvas TARGET [--profile shell|claude-code|codex] [--title TITLE]

mt fork child SOURCE --title TITLE --environment-json JSON [--prompt TEXT] [--start]
mt fork sibling SOURCE --title TITLE --environment-json JSON [--prompt TEXT] [--start]
mt fork children SOURCE --items-json JSON [--batch-key KEY]

mt remove preview TARGET --scope node|subtree
mt remove commit CONFIRMATION_REF
mt close canvas-preview TARGET
mt close canvas-commit CONFIRMATION_REF

mt focus TARGET
mt switch workspace TARGET
mt switch task TARGET
mt switch canvas TARGET
```

所有写操作支持 `--json` 结构化输出。批量参数也支持从标准输入读取 JSON，避免长内容在 Shell 引号处理中失真。

### 5.1 环境选择

每个 Fork 项必须带明确环境决策：

```ts
type ForkEnvironmentChoice =
  | { mode: 'current' }
  | { mode: 'existing-worktree'; branch: string; worktreeRef: string }
  | { mode: 'new-worktree'; branch: string }
```

- `current`：继续使用父节点当前执行环境，多个节点可以共用当前分支；
- `existing-worktree`：使用 Host Control 已列举且分支匹配的 Worktree；用户确认分支，AI 提交对应稳定引用；
- `new-worktree`：创建新分支和独立 Worktree；
- 普通目录只使用 `current`；
- AI 不根据猜测填写环境选择，用户明确决定后再提交；
- 用户说“全部使用 main”时，AI 解析承载 `main` 的现有执行环境，让全部子节点共享该环境；存在多个候选时先询问。

### 5.2 幂等与结构版本

- 每个写操作包含稳定 submission key；
- 批量操作包含 batch key，每个条目包含 item key；
- 相同 key 与相同输入重复提交时返回原结果；
- 相同 key 与不同输入返回冲突；
- 基于序号、相对关系或名称解析的写操作携带 projection revision；
- 目标关系变化后返回结构已更新，由 AI 重新列举和解析。

## 6. 目标解析

### 6.1 默认目标

AI 先运行 `mt identify --json`。未指定层级时：

- 创建事项使用当前工作空间；
- 创建画布使用当前事项；
- 创建会话使用当前画布；
- 创建子节点或同级节点使用当前会话；
- 聚焦和切换使用用户话语中的明确目标。

### 6.2 相对与跨窗口目标

- “左边、右边、同级第 N 个”只在当前画布、当前 DAG level 中解析；
- “父会话、第 N 个子会话”使用 DAG 关系解析；
- 明确给出工作空间、事项、画布、标题或路径时，可搜索本机 Matou 全部窗口；
- 唯一候选直接使用；
- 2–5 个候选展示人类可读信息供用户选择；
- 更多候选请用户补充一个筛选条件；
- 连续指代“它、那个、刚才那个”复用上一轮唯一且已确认的目标；
- “另一个、左边、右边、换成”触发重新解析。

## 7. 创建行为

### 7.1 工作空间

- “把当前目录创建为工作空间”使用调用方会话当前目录；
- 用户给出路径时先做路径校验；
- 只说“创建工作空间”时由 AI 询问目录；
- 标题缺省时使用目录名称；
- 创建结果沿用界面行为，包含默认事项、默认画布和初始 Shell 会话；
- 新建完成后默认保持调用方焦点，用户明确要求进入时再激活新工作空间。

### 7.2 事项

- 默认创建在当前工作空间；
- 可通过稳定目标指定其他工作空间；
- 创建时可直接写入用户或 AI 给出的标题，避免先出现默认标题再变化；
- 新事项包含默认画布和初始 Shell 会话；
- 默认保持调用方焦点。

### 7.3 画布

- 默认创建在当前事项；
- 支持在明确目标事项中创建；
- 创建时可直接写入标题；
- 新画布包含初始 Shell 会话；
- 默认保持调用方画布，用户明确要求进入时再切换。

### 7.4 普通会话

- 默认在当前画布创建同级会话；
- profile 缺省时创建 Shell；
- 明确指定 Claude Code 或 Codex 时创建对应类型；
- 用户或 AI 可以同时给出标题；
- 普通创建不继承另一会话的 provider 对话；需要继承时使用 Fork。

## 8. Fork 与批量子节点

### 8.1 单个 Fork

- `child` 创建当前节点的子节点；
- `sibling` 创建当前节点的同级节点；
- provider 会话继承对话上下文、模型和权限模式；
- Git 环境严格采用用户已确认的环境选择；
- `--prompt` 为新节点分配专属任务；
- `--start` 表示创建完成后发送任务并开始执行；
- 缺少 `--start` 时节点创建后保持待命。

### 8.2 三方案批量场景

用户说“根据刚才三个方案创建三个子节点”时，AI 按以下顺序处理：

1. 识别当前会话为父节点；
2. 从最近对话中提取三个方案；
3. 为每个方案生成简短、可区分的标题；
4. 精确重复的标题按出现顺序增加序号；
5. 展示三个标题并询问每个节点使用的分支和 Worktree；
6. 用户可以统一选择当前分支，也可以逐项指定现有 Worktree 或新分支；
7. 提交一个批量 Fork 请求；
8. 默认保持父节点和当前画布位置；
9. 汇总成功项、失败项、分支、Worktree 和运行状态。

示例确认内容：

```text
准备创建 3 个子节点，请确认环境：
1. 轻量适配方案 → 当前 main
2. 服务层重构方案 → 新分支 feature/service-refactor
3. 完整架构升级 → 新分支 feature/architecture-upgrade
```

### 8.3 创建与执行的区分

- “创建三个子节点”：完成 Fork、命名和环境设置，各节点保持待命；
- “创建并分别实现三个方案”：除创建外，还向每个节点发送对应方案任务并开始执行；
- 每个节点收到自己的方案说明，避免任务混淆；
- 自动发送任务失败时保留已创建节点，并将该项标记为“已创建，待启动”。

### 8.4 部分失败

- 一个条目失败后继续处理其他条目；
- 成功条目保持原状态；
- 返回每个条目的 `created`、`ready`、`started` 或 `failed` 状态；
- 重试请求只携带失败 item key；
- 重试不会复制已成功节点。

## 9. 聚焦与切换

### 9.1 当前窗口

- 切换工作空间时同时恢复其活动事项、画布和会话；
- 切换事项时同时恢复其活动画布和会话；
- 切换画布时展示其活动会话；
- 聚焦会话时展示其画布、滚动到卡片并把输入焦点交给终端。

### 9.2 跨窗口

跨窗口聚焦或切换采用完整路径导航：

1. Runtime 校验目标仍存在；
2. Desktop Main 将目标窗口带到前台；
3. 目标 Renderer 激活工作空间、事项、画布和会话；
4. 卡片进入可见区域并获得终端焦点；
5. Renderer 返回导航完成确认；
6. AI 告知用户最终位置。

导航结果包含最终窗口、工作空间、事项、画布和会话信息。目标窗口关闭或确认超时时，结构数据保持原样，并返回可重试状态。

## 10. 移除与关闭

### 10.1 产品术语

- 工作空间：移除工作空间；
- 事项：移除事项；
- 会话：移除节点；
- 会话子树：移除当前节点及全部子节点；
- 画布：关闭画布；
- Worktree：移除 Worktree。

用户界面、自然语言规则、CLI 输出和异常反馈统一使用以上术语。

### 10.2 影响预览

执行前返回：

- 目标名称和完整层级路径；
- 受影响的事项、画布、会话与子节点数量；
- 仍在启动、运行或等待输入的会话数量；
- 将结束的终端进程；
- 操作范围是当前节点还是完整子树；
- 项目目录、Git 分支、仓库文件和 Worktree 的保留状态。

叶子节点使用“移除节点”。存在子节点时，AI 询问“仅移除当前节点”或“移除当前节点及全部子节点”。

### 10.3 一次性确认

预览成功后生成短时有效的一次性 confirmation ref，其服务端记录绑定：

- 调用方 SessionRun；
- 操作类型；
- 目标稳定引用；
- 操作范围；
- projection revision；
- 影响摘要哈希；
- 过期时间。

用户明确确认后，AI 使用 confirmation ref 提交操作。结构或影响范围变化时该引用失效，AI 展示最新预览。引用成功使用一次后立即失效。

### 10.4 结果

- 关闭画布结束并归档画布内会话，然后沿用现有界面规则选择相邻画布或处理最后一个画布；
- 移除事项和工作空间沿用当前界面的层级处理规则；
- 移除会话沿用当前节点与子树规则；
- 项目目录、仓库文件和 Git 分支保持原状；
- Worktree 清理由单独的“移除 Worktree”动作处理；
- 完成后返回实际受影响对象及新的活动层级路径。

## 11. Provider 自然语言规则

Claude Code Skill 与 Codex 会话指令共享以下规则：

1. 始终先 `identify`，再 `list/resolve`，最后执行动作；
2. 目标唯一且参数完整时直接执行创建或导航；
3. 分支和 Worktree 缺少用户决策时集中询问一次；
4. 移除与关闭始终先预览影响，再等待明确确认；
5. 批量创建先总结标题，再确认环境；
6. “创建”和“创建并执行”使用不同动作；
7. 保留成功批量条目，只重试失败条目；
8. 自然语言结果使用产品名称、标题和路径，省略内部 ID、确认引用和协议字段；
9. 连续指代只复用上一轮唯一且已确认的目标；
10. 用户已明确目标、环境和执行意图时，避免重复询问。

## 12. 错误与恢复

Host Control 使用结构化错误：

```text
TARGET_NOT_FOUND
AMBIGUOUS_TARGET
STALE_PROJECTION
TARGET_NOT_READY
CAPABILITY_DENIED
CONFIRMATION_REQUIRED
CONFIRMATION_EXPIRED
CONFIRMATION_STALE
PATH_CONFLICT
BRANCH_CONFLICT
WORKTREE_CONFLICT
PARTIAL_SUCCESS
NAVIGATION_TIMEOUT
STORAGE_READ_ONLY
```

对应用户反馈：

- 目标缺失或结构已变化：重新列举并解析；
- 多候选：展示候选项；
- 目标准备中：显示当前状态并稍后重试；
- 确认过期或影响变化：重新生成预览；
- 分支或 Worktree 冲突：展示具体节点与分支，重新询问该节点环境；
- 批量部分成功：保留成功项并生成失败项重试请求；
- 导航超时：说明目标窗口状态并允许重试；
- 数据库只读恢复：保留用户请求摘要，状态恢复后可重新执行。

## 13. 数据与组件边界

### 13.1 Host Action Facade

新增单一高层 facade，负责：

- 将 Host Control 目标解析成稳定实体；
- 把自然语言需要的一次动作映射为现有业务服务调用；
- 为复合创建提供一个权威事务边界或显式阶段结果；
- 生成影响预览与确认 token；
- 统一返回人类可读结果和稳定 ref。

facade 不复制层级规则、Fork 工作流或 Git Worktree 实现。

### 13.2 Batch Fork Coordinator

批量协调器负责：

- 校验标题、环境决策和 item key；
- 按条目启动现有 Fork 工作流；
- 记录逐项状态；
- 汇总成功与失败；
- 只重试失败项；
- 在节点 ready 后按需发送专属任务。

### 13.3 Confirmation Service

确认服务只在 Runtime 内存中保存短期记录，不写入持久化数据库。CLI 获得绑定当前调用方的一次性 confirmation ref；AI 的自然语言反馈省略该引用。Runtime 重启后旧引用全部失效，用户重新获取最新预览。

### 13.4 Navigation Bridge

导航桥接 Runtime、目标 Renderer 和 Desktop Main，只承载导航意图与完成确认。层级激活仍由 Runtime 权威服务完成，窗口前台与终端 DOM 聚焦由 Desktop 完成。

## 14. 验收标准

### 14.1 自动化测试

1. 每个新增 Host Control scope 独立鉴权；
2. CLI 参数、JSON 输入、结构化输出和错误码；
3. 相对目标、父子目标、跨窗口目标和 stale revision；
4. 工作空间、事项、画布和三类会话创建；
5. 创建时直接写入标题，投影中不出现中间默认标题；
6. 单个 child/sibling Fork 的上下文、模型和权限继承；
7. 当前环境、现有 Worktree 和新 Worktree 三种决策；
8. 三项批量全部成功；
9. 中间条目失败后其余条目继续；
10. 仅重试失败项且无重复节点；
11. 创建与创建并执行的状态差异；
12. 影响预览数量、运行状态和范围准确；
13. confirmation ref 的调用方绑定、一次性、过期和结构变更失效；
14. 当前窗口与跨窗口导航完成确认；
15. 会话重启、恢复和 Fork 后模型与权限保持；
16. Runtime 重启后控制凭证更新且旧凭证失效；
17. 存储只读、路径冲突、分支冲突和导航超时恢复；
18. 产品文案统一使用“移除”和“关闭画布”。

### 14.2 真实 App 验收

1. Claude Code 总结三个方案并创建三个待命子节点；
2. 三个节点共享当前 `main` 环境；
3. 三个节点分别创建新分支和 Worktree；
4. 三个节点使用混合环境决策；
5. 创建并分别启动三个方案；
6. 单项失败后只重试失败节点；
7. 自然语言创建工作空间、事项、画布、Shell、Claude Code 和 Codex 会话；
8. 当前窗口切换与聚焦；
9. 跨窗口切换、置前、滚动可见和终端聚焦；
10. 移除会话、会话子树、事项和工作空间前展示准确影响；
11. 关闭画布前展示准确影响并执行现有最后画布规则；
12. App 重启后原会话模型与权限保持；
13. Shell 中手动使用新增 `mt` 命令；
14. AI 的自然语言反馈只展示产品名称、标题、路径和结果，不展示内部 ref、控制凭证和协议字段。

## 15. 交付边界

本期只扩展本机 Matou 的产品层级控制。以下能力留在后续范围：

- 多设备或远程 Matou 实例控制；
- 定时或无人值守结构操作；
- 基于自然语言自动决定 Git 分支策略；
- 自动清理分支或 Worktree；
- 绕过用户确认的移除与关闭；
- 将 Matou 控制配置写入用户全局 provider 环境。
