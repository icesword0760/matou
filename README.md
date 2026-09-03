# 码头 Matou — Claude Code / Codex 多智能体桌面终端

**Claude Code GUI · Codex Desktop Workspace · AI Coding Agent Session Manager · DAG Visualization · Git Worktree**

码头（Matou）是一款面向 AI 编程的桌面工作台：把 Claude Code、Codex、任务、分支和上下文放进同一个可恢复的工作现场。你可以同时推进多个编码智能体，又随时知道每个会话在做什么、需要什么、从哪里分出来。

[快速开始](#三分钟运行) · [核心场景](#从混乱的终端到可管理的-ai-工作流) · [DAG 使用方法](#用-dag-看懂会话从哪里来下一步去哪里) · [架构文档](#架构与质量)

![码头 Matou 的 Claude Code 多会话桌面工作台](assets/shots/workspace-demo.png)

> 上图为副屏中的完整 Matou App 窗口。所有会话均为 Claude Code 隔离演示会话；工作空间、任务名称、终端输出和通知均为一次性演示数据。

---

## 你需要管理的不是终端，而是正在推进的工作

同时开两个 AI 编程会话很轻松，开到十个以后，真正消耗注意力的通常不是代码：

- 这个窗口属于哪个项目、哪个需求、哪个分支？
- 哪个 Claude Code 已完成，哪个在等确认，哪个已经出错？
- 另一个会话刚得出的结论，是否又要复制粘贴一遍？
- 想验证第二种方案，如何保留原会话上下文，又不污染正在工作的目录？
- App 重启后，页签、分屏、目录、输出和 Agent 身份还能否回到原位？

码头把这些问题收进一个工作模型：**工作空间 → 事项 → 画布 → 会话卡片**。你看到的是目标、关系、状态和下一步，而不是一排难以辨认的终端窗口。

## 从混乱的终端到可管理的 AI 工作流

### 1. 用四级结构管理项目、任务和 Agent

| 层级 | 适合放什么 | 你会在什么时候用到 |
|---|---|---|
| **工作空间 Workspace** | 一个代码仓库、产品或客户环境 | 同时维护多个项目时，隔离目录、任务和通知 |
| **事项 Task** | 一项能交付的工作，例如“发布 Matou 0.1” | 按就绪、运行中、阻塞、完成推进工作，而不是寻找窗口 |
| **画布 Canvas** | 一个事项里的阶段或场景 | 把方案探索、实现、回归放到不同页签，减少视觉噪音 |
| **会话卡片 Session** | 一个独立 Claude Code / Codex Agent | 并行编码、审查、测试或调研；每张卡片保留自己的输入、输出和状态 |

事项支持新建、重命名、排序与看板流转；画布支持页签、水平分屏和垂直分屏；会话卡片可以独立运行、聚焦、脱出窗口或回到原画布。

### 2. 直接用自然语言获取其他卡片的信息

当结果在另一张卡片里时，你不必逐个切换、滚动、复制。可以直接对当前 Claude Code 或 Codex 说：

> “看看右边那张卡片的测试跑到哪了，给我结论。”

> “读取父会话最近的输出，对比方案 A 和方案 B 的风险。”

> “让左边的会话继续运行回归，完成后把结果发回来。”

Matou 会向受管 Agent 提供会话定位与控制能力，使它能够识别自己、列出关联卡片、读取实时屏幕或历史输出、查看可执行命令，并向父卡片、子卡片、左右相邻卡片或指定会话发送输入。这样，跨会话协作仍然发生在你的任务结构里。

<details>
<summary>当前 main 已提供的会话控制范围</summary>

受管会话可使用 `mt identify`、`mt list`、`mt read`、`mt history`、`mt commands`、`mt send` 和 `mt key`。目标可按 `self`、`left`、`right`、`parent`、`child:N`、`sibling:N` 或会话引用指定。

</details>

### 3. 用 DAG 看懂会话从哪里来、下一步去哪里

普通标签页只能告诉你“有哪些会话”，DAG 会话图还能告诉你“它们是什么关系”。当一个问题被拆成多条验证路线时，按 `Option + Tab` 打开独立 DAG：

1. **从当前节点看上下游**：快速定位父会话、当前会话和子会话。
2. **区分两种关系**：实线 Fork 表示继承对话上下文；虚线普通关联表示建立关系但不继承对话。
3. **不打开终端也能判断进展**：节点直接显示 Agent 类型、运行状态、目录、分支、最近输出与子会话数量。
4. **搜索与大图导航**：按名称、路径、分支或输出搜索，配合缩放、平移和自动聚合浏览大规模会话图。
5. **一键回到现场**：点击节点即可关闭 DAG 并聚焦对应会话；已停止节点仍保留在关系图中，方便回看决策链。

![Claude Code 会话分支 DAG 可视化](assets/shots/session-dag-demo.png)

> DAG 演示使用五个隔离 Claude Code 会话；画面展示当前层级的三个节点，更深层节点由图层聚合承载。

### 4. AI 通知：只在需要你时打断你

后台 Agent 完成任务、等待输入、请求帮助或发生错误时，Matou 会把信号送到对应会话卡片，并沿画布、事项和工作空间逐级提示。当前正在查看的卡片保持安静；点击通知即可回到产生事件的现场。

通知中心提供事件摘要、来源路径和声音开关，适合同时跑实现、测试、审查等多个 Claude Code 会话时使用。

### 5. AI HUD：不用再问“你现在做到哪了”

底部 HUD 把影响下一步判断的信息放在一个视线范围内：

- 当前模型、权限模式、上下文窗口与已用比例
- 会话持续时间、周期用量和重置时间
- 当前目录、Git 分支、脏状态与工作树环境
- 正在调用的工具、待办进度、运行回归与 MCP 异常

![Claude Code AI HUD 与分级通知中心](assets/shots/agent-hud-notifications-demo.png)

> 左侧是通知中心，右侧卡片收到新通知，底部为 Claude Code Agent HUD。截图只包含完整 App 内容。

### 6. Fork + Git Worktree：放心比较多种实现

从已有会话创建 Fork，可以保留父会话，让新会话继承对话后独立继续。需要代码隔离时，为分支绑定 Git Worktree：

- 父会话继续守住稳定实现；
- 子会话分别验证方案 A、方案 B；
- 每条路线拥有清楚的会话关系、目录和 Git 状态；
- 验证完成后，再决定合并、保留或停止。

这适合架构选型、疑难缺陷、多方案 UI、并行代码审查和高风险重构。

### 7. 用看板判断下一步，而不是靠记忆

工作空间看板把事项分为**就绪、运行中、阻塞、完成**。拖动卡片即可更新状态；每张事项卡同时显示会话数量，让你先处理真正需要关注的工作。

![Matou Workspace AI 任务看板](assets/shots/workspace-board-demo.png)

### 8. 重启恢复与多窗口：工作现场跟着任务走

Matou 持久化事项、页签、分屏、目录、焦点、终端输出和受管 Agent 身份。窗口隐藏、应用重启或异常退出后，按不同会话类型恢复现场。会话还可以脱出为独立窗口，再归还原画布；主窗口和独立窗口共享同一 Runtime 会话。

## 自然语言新建子卡片：产品设计已确认

自然语言结构操作已经进入产品设计，目标是让用户直接描述任务拆分，而不必连续点击菜单。例如：

> “根据这三个方案创建三个子卡片，分别验证性能、兼容性和回滚路径。”

规划范围包括新建工作空间、事项、画布和会话卡片；创建子卡片、兄弟卡片和批量子卡片；聚焦、切换、移除与关闭前预览确认。**当前 main 已支持自然语言读取与控制其他卡片；自然语言创建层级结构处于待实现阶段。**

## 适合谁

- 同时运行多个 Claude Code 或 Codex 会话的独立开发者
- 希望把 AI coding agent 从“聊天窗口”变成可管理工作流的团队
- 需要 DAG visualization 回溯方案分支、上下文来源和决策路径的复杂项目
- 经常使用 Git Worktree 并行开发、测试、审查和修复的工程师
- 关注会话恢复、通知分级和上下文用量的重度 AI 编程用户

## 三分钟运行

### 环境要求

- Node.js `>=22.16.0`
- pnpm `10.17.1`
- macOS、Linux 或 Windows

### 从源码启动

```bash
git clone https://github.com/icesword0760/matou.git
cd matou
corepack enable
pnpm install
pnpm dev
```

`pnpm install` 会校正 node-pty 在 macOS 预构建包中的 `spawn-helper` 可执行权限。使用 Claude Code 或 Codex 前，请先在本机完成对应 CLI 的安装与登录。

## 常用命令

```bash
pnpm test               # 单元与集成测试
pnpm typecheck          # 全工作区类型检查
pnpm build              # 生产构建
pnpm test:e2e           # Electron → Runtime → PTY → xterm 完整链路
pnpm check:identifiers  # 品牌与命名门禁
```

## 项目结构

```text
apps/
├── desktop/              Electron Main、Preload、React Renderer、xterm
└── runtime/              UtilityProcess、PTY、会话、Journal、SQLite

packages/
├── contracts/            跨进程协议与运行时校验
├── domain/               领域类型与不变量
└── ui/                   共享 UI 边界

docs/
├── architecture/         进程、领域、协议与 ADR
├── prd/                  产品需求与交互规格
├── acceptance/           验收记录与运行证据
└── parity/               产品行为对照矩阵

tests/e2e/                真实 Electron 用户旅程
```

## 架构与质量

- Electron Main 创建并监督 app-scoped Runtime UtilityProcess；终端数据通过 `MessageChannelMain` 直达 Renderer。
- Runtime 使用 node-pty 管理 PTY，以 credit window 和累计 ACK 控制输出流量。
- Renderer 只消费可重建投影；会话、层级、持久化和进程生命周期由 Runtime 维护。
- SQLite 保存结构元数据，分段 Journal 保存终端输出与恢复检查点。
- 跨进程协议使用精确版本握手与 Zod schema 校验。
- Renderer 保持 `nodeIntegration: false`、`contextIsolation: true`、`sandbox: true`。

进一步阅读：[进程模型](docs/architecture/process-model.md) · [领域模型](docs/architecture/domain-model.md) · [事件与流协议](docs/architecture/event-and-stream-protocol.md) · [ADR-0001](docs/architecture/adr/0001-app-scoped-utility-process.md)

## 反馈与交流

- 在 [Issues](https://github.com/icesword0760/matou/issues) 提交问题或建议。
- 涉及恢复、分支、通知或多窗口问题时，请附上复现步骤、系统版本和可公开的演示数据。

---

如果 Matou 让你少花一点时间寻找终端、确认上下文，欢迎点一个 ⭐。
