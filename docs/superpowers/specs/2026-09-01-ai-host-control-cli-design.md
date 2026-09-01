# Matou AI 宿主控制与 `mt` CLI 设计

**日期：** 2026-09-01
**状态：** 已批准，进入实施
**范围：** Matou 托管终端通过自然语言或 `mt` CLI 读取和输入其他 Matou 终端

## 1. 目标

让 Matou 内的 Claude Code 和 Codex 能理解用户对窗口、工作空间、事项、画布和会话卡片的自然语言指向，并读取或输入目标终端。所有 Matou 托管终端同时获得可手动使用的 `mt` CLI。

用户安装 Matou 后无需配置环境、权限或全局命令。AI 可以直接响应以下请求：

- “看看右边那个终端在干什么”；
- “它之前执行过哪些命令”；
- “把 `pnpm test` 发给隔壁并回车”；
- “给父会话按一下中断键”；
- “看看另一个窗口中 `matou_workspace` 的测试事项”。

## 2. 已确认的产品决策

1. 首期只开放查看、读取、发送文本和发送预置控制键。创建画布、创建会话、Fork、移出、关闭、聚焦和切换视图不在首期能力中。
2. “左边、右边、第 N 个”按当前画布、当前 DAG 层级的兄弟卡片列表解析；父子节点使用“父会话、子会话”表达。
3. 所有 Matou 托管 Shell、Claude Code、Codex 都获得宿主控制身份和 `mt` CLI；Claude Code 与 Codex 额外获得自然语言调用规则。
4. “当前屏幕”指终端最新运行画面，不随用户临时滚动到历史位置而变化；历史和历史命令使用独立读取能力。
5. 相对位置只在调用方当前窗口、当前画布、当前 DAG 层级内解析；明确给出工作空间、事项、画布或会话名称时，可搜索本机 Matou 的全部窗口。
6. AI 读取或输入其他会话时不切换焦点、不滚动画布、不拉起窗口、不闪动边框、不生成通知。终端实际变化和 AI 的结果回复构成用户反馈。
7. 控制命令使用短名称 `mt`。
8. 默认界面不新增宿主控制开关、权限标签或操作提示。
9. `agent-team-member` 是内部兼容与预留投影，不作为本期正式产品功能，也不新增入口。

## 3. Kooky 对照结论

### 3.1 复用的产品逻辑

- 托管终端启动时自动获得控制身份，用户零配置；
- 调用方先识别自己，再列举拓扑，随后解析目标并执行；
- 多候选时先反问，元信息不足时才短读少量候选；
- 连续追问复用上一次已确认目标；
- 输入直接写入目标 PTY，不改变窗口焦点或界面位置；
- 外部调用默认拒绝；
- AI 只向用户展示名称、序号、标题、工作目录等人类字段，不展示内部标识。

### 3.2 Matou 的结构适配

Kooky 的 `window/workspace/pane/surface` 不直接复制。Matou 使用自己的产品层级：

```text
窗口
└─ 工作空间
   └─ 事项
      └─ 画布
         └─ 会话卡片 + DAG 父子关系
```

Matou 的卡片横向顺序、DAG 层级和独立窗口状态必须由 Runtime 权威数据投影，不能从 Kooky 的 Pane 或分屏结构反推。

### 3.3 Kooky 参考实现

- 控制协议与服务：`/Users/icesword/Documents/AIProjects/kookey/electron/kc-control-server.js`
- 控制分发：`/Users/icesword/Documents/AIProjects/kookey/electron/kc-control-dispatcher.js`
- 认证边界：`/Users/icesword/Documents/AIProjects/kookey/electron/kc-control-auth.js`
- CLI：`/Users/icesword/Documents/AIProjects/kookey/electron/kc-control-cli.js`
- 托管终端身份注入：`/Users/icesword/Documents/AIProjects/kookey/electron/claude-code-manager.js`
- 自然语言 Skill：`/Users/icesword/Documents/AIProjects/kookey/electron/claude-skills/kc-terminal/SKILL.md`
- Skill 目标解析规则：`/Users/icesword/Documents/AIProjects/kookey/electron/claude-skills/kc-terminal/references/target-resolution.md`

## 4. 方案选择

### 4.1 采用方案：扩展 Matou 现有 Host Control

Runtime 继续作为唯一权威层，扩展已经存在的本地控制面，补齐拓扑投影、调用方身份、最新屏幕、CLI 和 Provider 规则。

该方案保留 Matou 现有数据和生命周期边界，不建立第二套 Renderer 权威，也避免把 Kooky 的结构模型硬套到 DAG 上。

### 4.2 未采用方案

- Claude Code 与 Codex 分别实现专属工具：会导致 Shell 缺少统一命令，并使两种 AI 的行为逐渐分叉。
- 整体移植 Kooky 控制面：会引入与 Matou 事项、画布和 DAG 不一致的第二套层级语义。

## 5. 架构

### 5.1 组件边界

#### Host Control Server

沿用 Runtime 私有 Unix Domain Socket；Windows 使用 Named Pipe。负责协议版本、请求截止时间、能力令牌、结构化错误、请求大小边界和分发。

#### Host Topology Projector

从 Runtime 权威数据库生成：

- 窗口及当前导航上下文；
- 工作空间、事项、画布的名称和界面顺序；
- 当前 DAG 层级的会话顺序；
- 会话类型、标题、工作目录、运行状态、独立窗口状态；
- 父会话、子会话和兄弟会话关系；
- 调用方自身所在的完整层级。

投影同时返回 `projectionRevision`。按序号调用时必须携带对应 revision；界面顺序变化后，旧序号返回冲突，AI 重新列举后再执行。

#### Terminal Screen Projector

Runtime 在接收 PTY 输出和 resize 时同步更新终端屏幕模型，形成与最新运行状态一致的屏幕快照。`read` 返回最新屏幕，`history` 返回 Journal 中的滚动历史，`commands` 返回 Shell Integration 记录的真实命令边界。

屏幕模型属于 Runtime，不依赖某张卡片当前是否在 Renderer 中可见，也不读取用户临时滚动位置。

#### `mt` CLI

`mt` 是安装包内置的轻量客户端，通过环境中的私有 endpoint 和令牌访问 Host Control Server。首期命令：

```text
mt identify
mt list
mt read <target>
mt history <target>
mt commands <target>
mt send <target> <text>
mt key <target> <key>
```

CLI 支持结构化 JSON 输出供 AI 使用，并提供简短的人类帮助。`send` 支持只发送文本或附带回车；`key` 只接受系统预置控制键。

#### Provider Integration

Claude Code 与 Codex 使用同一份产品行为规范，并通过各自支持的会话级启动参数或受管配置层加载。规则包含：

- `identify → list → resolve → act`；
- 名称、工作目录、类型、序号和 DAG 关系匹配；
- 多候选先反问；
- 元信息不足时才短读屏；
- 连续指代复用上次目标；
- 错误转成中文用户反馈；
- 能力边界和本期非目标。

Provider 集成只在当前会话生效，不写入用户的全局 Claude Code 或 Codex 配置。

## 6. 安装、注入与升级

### 6.1 安装包内容

安装包随 App 资源一起携带：

- `mt` CLI 及 macOS/Linux/Windows wrapper；
- Claude Code 与 Codex 的宿主控制规则资产；
- Runtime 控制服务代码。

新安装不依赖用户已安装 Node 或 npm，不修改系统 PATH，也不写入 `~/.claude` 或 `~/.codex`。

### 6.2 注入时机

每次创建 `SessionRun` 时完成注入，包括：

- 新建 Shell、Claude Code、Codex；
- 恢复和载入会话；
- Fork 后的首次启动；
- App 重启后的会话重建；
- 启动失败后的重试。

所有托管会话注入：

- App 私有 `mt` 目录到该 PTY 的 PATH；
- 控制 endpoint；
- 本次运行专属 capability token；
- 调用方窗口、工作空间、事项、画布、会话身份。

Claude Code 与 Codex 再加载会话级自然语言规则。Shell 只获得 CLI 和控制身份。

### 6.3 生命周期

- App 启动时 Runtime 创建私有控制 endpoint；
- SessionRun 启动时生成新令牌；
- SessionRun 结束时撤销令牌；
- Runtime 重启时旧 generation 的全部令牌失效；
- App 升级并重启后自动使用新版 CLI 与规则。

## 7. 目标解析语义

### 7.1 默认上下文

AI 首先通过 `mt identify` 获得调用方当前窗口、工作空间、事项、画布、会话和 DAG 层级。

### 7.2 相对位置

- “左边、右边、隔壁、第 N 个”只在当前画布、当前 DAG 层级的兄弟卡片中解析；
- 顺序包含暂时位于屏幕外但属于同一横向列表的卡片；
- 父节点不混入当前层级序号，使用“父会话”指向；
- 子节点使用“第一个子会话”等关系指向；
- 已移出的节点不参与画布序号；
- 独立窗口中的会话保留原 DAG 身份，并标记为独立窗口。

### 7.3 搜索范围

- 相对位置默认限制在调用方当前窗口和当前画布；
- 明确给出工作空间、事项、画布或会话名称时，可搜索本机 Matou 全部窗口；
- 多窗口同名目标按多候选处理；
- AI 操作其他窗口时不将其拉到前台。

### 7.4 歧义规则

- 唯一候选直接执行；
- 多个候选先列出最多 5 个并反问；
- 候选超过 5 个时要求用户补充条件；
- 元信息不足时才短读少量候选；
- 上一轮已经锁定目标时，“它、那个、刚才那个”沿用该目标；
- 用户明确说“另一个、左边那个、换成”时重新解析。

## 8. 读取和输入

### 8.1 读取

- `read`：终端最新运行画面；
- `history`：有界滚动历史；
- `commands`：提示符边界后的真实用户输入，保留拼写错误；
- 初始化脚本和提示符预热不计为用户命令；
- 历史不足时明确返回边界信息，AI据此说明可见范围。

所有读取都有最大行数和最大字节数，避免一个终端占用过多上下文或内存。

### 8.2 输入

- 文本和可选回车作为一个完整动作串行写入目标 PTY；
- 同一会话上的并发写入按完整动作排队，字符不会相互穿插；
- 控制键使用固定 allowlist，覆盖 Kooky PRD 定义的回车、Tab、Esc、退格、删除、方向键、Home/End、翻页键及常用 Ctrl 组合键；
- 未知按键返回结构化 `UNSUPPORTED`；
- 输入不触发焦点切换、滚动、窗口激活、边框动画或通知。

## 9. 权限与边界

- 控制 endpoint 仅监听本机私有 socket 或 Named Pipe；
- 每个 SessionRun 获得独立令牌，绑定 runId、scope、runtime generation 和生命周期；
- 所有 Matou 托管会话均获得首期 read/send scopes；
- 子进程继承当前托管会话身份，与 Kooky 的托管进程模型一致；
- Matou 外部进程缺少有效令牌，默认返回 `CAPABILITY_DENIED`；
- 首期令牌不包含创建、Fork、移出、关闭、聚焦和切换视图能力；
- 控制服务异常不影响 PTY 和终端日常输入。

## 10. 错误处理

能力层统一返回：

- `INVALID_REQUEST`：参数或帧格式错误；
- `TARGET_NOT_FOUND`：目标已经消失；
- `TARGET_NOT_READY`：目标暂时未进入可输入状态；
- `AMBIGUOUS_TARGET`：能力调用仍包含多个目标；
- `TIMEOUT`：超过请求截止时间；
- `CAPABILITY_DENIED`：令牌缺失、过期或 scope 不匹配；
- `CONFLICT`：序号 projection 已变化或同一对象动作冲突；
- `UNSUPPORTED`：对象或按键超出首期能力；
- `INTERNAL_ERROR`：服务内部错误，控制服务继续接受后续请求。

Claude Code 与 Codex 将错误转换成面向用户的中文，并说明重新列举、稍后重试、补充目标或选择其他会话等下一步。

## 11. 用户可见行为

- 默认界面没有新增开关、标签或提示；
- AI 读取其他会话时当前界面不变化；
- AI 输入其他会话时目标终端自然显示输入及其输出；
- AI 在自己的回复中确认目标位置和操作结果；
- 外部调用被拒只反馈给外部调用方，不打扰 Matou 内用户。

## 12. 测试与验收

### 12.1 单元测试

- 五级拓扑、DAG 层级、独立窗口和稳定序号；
- projection revision 与过期序号；
- 调用方身份注入和目标解析所需元信息；
- capability token 签发、撤销、generation 和 scope；
- 屏幕快照对 ANSI、光标移动、清屏、resize 和全屏程序的处理；
- Journal 历史边界和真实命令记录；
- send 动作原子排队和完整按键 allowlist；
- 控制服务错误隔离。

### 12.2 集成测试

- Shell、Claude Code、Codex 新建、恢复、载入、Fork、重试均获得 `mt`；
- `mt identify/list/read/history/commands/send/key` 完整链路；
- Claude Code 与 Codex 只加载会话级规则；
- AI 多候选反问和连续指代；
- 后台卡片、屏幕外卡片和独立窗口不发生焦点或导航变化；
- Matou 外部调用默认拒绝。

### 12.3 打包验收

使用全新用户目录和接近 Finder 启动的精简环境验证打包后的 macOS App：

1. 系统无需开发版 Node/npm；
2. 新建三种会话后 `mt identify` 均成功；
3. Shell 可手动读取和输入另一个终端；
4. Claude Code、Codex 可通过自然语言完成典型场景；
5. 重启 App 后旧令牌失效，新会话继续可用；
6. 操作其他卡片和窗口时没有焦点、滚动或通知副作用。

## 13. 非目标

- 操作 Matou 外部应用或远程机器；
- 新建、Fork、移出、关闭或聚焦会话；
- 创建事项或画布；
- 控制浏览器、编辑器、文件预览等非终端对象；
- Agent Team 的产品入口与交互；
- 新增宿主控制设置页或权限提示 UI；
- 多 Agent 高层任务协调和冲突仲裁。
