# 码头 Matou

Agent-Native Desktop Terminal · 面向 AI 编程智能体的桌面任务工作台

支持 Claude Code · Codex · Shell · 工作空间 / 事项 / 画布 / 会话 · 分支 Fork · 自动恢复 · 多窗口

[快速开始](#三分钟运行) · [界面一览](#界面一览) · [架构文档](#架构与质量)

![码头多会话工作现场](assets/shots/workspace-demo.png)

> 截图来自副屏中运行的真实 Matou App；工作区、事项和终端输出均为一次性隔离演示数据，不含个人工作区内容。

---

## 如果你也经历过这些

你同时开着几个 AI 编程会话：一个在改功能，一个在跑测试，一个等你确认，另一个已经失败。终端窗口越开越多，任务、目录、分支和上下文散落在不同标签里；切回来时，你先花时间确认“它是谁、做到哪、还能不能接着做”。

码头为这个时刻而做：让每个 AI 会话拥有清楚的位置、独立的上下文和可恢复的现场，让你管理的是任务，而不是一堆终端窗口。

## 它替你守住的五件事

| | 你得到的 |
|---|---|
| 🧭 现场不乱 | 用「工作空间 → 事项 → 画布 → 会话」组织终端；Shell、Claude Code 与 Codex 可以并行存在，切换任务不重启后台会话 |
| 🌿 探索隔离 | 从已有 Claude 会话 Fork 新分支，源会话保持原样；父子会话各自输入、恢复，适合并行验证不同方案 |
| 💾 重启可续 | 事项、页签、分屏、目录、焦点与 AI 会话身份自动恢复；普通 Shell 以干净进程回到原目录 |
| 🔔 动静可见 | 后台会话完成、等待、求助或出错时，通知会沿会话、页签、事项和工作空间逐级提示，点击即可返回现场 |
| 📊 状态集中 | 底部 HUD 汇总目录、Git、模型、上下文、权限、用量、工具与待办；看板把事项分成就绪、运行中、阻塞和完成 |

## 界面一览

### 多会话工作现场

同一个事项里并排运行多个终端，会话各自保留进程、目录、输出和输入焦点；上方主图展示了完整工作现场。

### Workspace 看板

把并行事项拖到对应状态列，快速判断下一步该推进哪里。

![Workspace 看板](assets/shots/workspace-board-demo.png)

## 核心能力

- **多层任务组织**：工作空间、事项、画布和会话四级结构，支持新建、重命名、排序、页签和水平 / 垂直分屏。
- **会话画布与 DAG**：沿父子关系查看分支，返回父会话、创建子分支或兄弟分支，并保留每条分支的独立现场。
- **终端与 Agent 共存**：普通 Shell、Claude Code、Codex 可混合编排；每个面板拥有独立 HUD 和生命周期。
- **Git 就地操作**：从底栏搜索和切换分支，创建分支与 Worktree，提交并推送，无需离开当前任务。
- **可靠恢复**：SQLite 保存结构元数据，分段 Journal 保存终端输出；窗口隐藏、应用重启、异常退出后按各自规则恢复。
- **多窗口**：会话可脱出为独立窗口，也可归还原画布；主窗口与独立窗口共享同一 Runtime 会话。
- **终端协作控制**：内置 `mt` 命令支持识别、列出、读取和向指定会话发送输入，便于智能体之间协作。

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
- Runtime 使用 node-pty 管理 Shell，以 credit window 和累计 ACK 控制输出流量。
- Renderer 只消费可重建投影；会话、层级、持久化和进程生命周期由 Runtime 维护。
- 跨进程协议使用精确版本握手与 Zod schema 校验。
- Renderer 保持 `nodeIntegration: false`、`contextIsolation: true`、`sandbox: true`。

进一步阅读：[进程模型](docs/architecture/process-model.md) · [领域模型](docs/architecture/domain-model.md) · [事件与流协议](docs/architecture/event-and-stream-protocol.md) · [ADR-0001](docs/architecture/adr/0001-app-scoped-utility-process.md)

## 反馈与交流

- 在 [Issues](https://github.com/icesword0760/matou/issues) 提交问题或建议。
- 涉及恢复、分支或多窗口问题时，请附上复现步骤、系统版本和可公开的演示数据。

---

如果码头让你少花一点时间寻找终端、确认上下文，欢迎点一个 ⭐。
