# Matou

Matou 是面向智能体任务的桌面终端。当前仓库是 Electron + React + xterm.js 的绿地实现，旧 reference product 工程仅作为产品能力与 PRD 参考。

## 当前基础能力

- Electron Main 创建并监督 app-scoped Runtime UtilityProcess；
- Main 使用 `MessageChannelMain` 移交端口，不转发 Terminal Data；
- Runtime 使用 node-pty 启动 Shell；
- Renderer 使用 React 和 xterm.js 显示终端；
- xterm write callback 驱动累计 ACK；
- Runtime 按 Session 维护 credit window；
- PTY 输出先写入分段 Journal，再投递给 Renderer；
- 协议使用精确版本握手和 Zod schema 校验；
- Playwright E2E 覆盖 Renderer → UtilityProcess → node-pty → xterm 完整链路。

Checkpoint、SQLite 元数据、Agent Adapter、Worktree 生命周期、语义事件、Scene Projection 和 Journal rotation 已在架构文档中定义，按独立产品阶段实现。

## 目录

```text
apps/
├── desktop/              Electron Main、Preload、React Renderer、xterm
└── runtime/              UtilityProcess、PTY、flow control、Journal

packages/
├── contracts/            跨进程协议与运行时校验
├── domain/               纯领域类型
└── ui/                   共享设计系统边界

docs/
├── architecture/         进程、领域、协议和 ADR
└── superpowers/plans/    实施计划

tests/e2e/                Electron 完整链路测试
```

## 环境

- Node.js `>=22.16.0`
- pnpm `10.17.1`
- macOS、Linux 或 Windows

## 命令

```bash
pnpm install
pnpm dev
pnpm test
pnpm typecheck
pnpm build
pnpm test:e2e
```

`pnpm install` 会校正 node-pty 在 macOS 预构建包中的 `spawn-helper` 可执行权限。

## 架构文档

- [进程模型](docs/architecture/process-model.md)
- [领域模型](docs/architecture/domain-model.md)
- [事件与流协议](docs/architecture/event-and-stream-protocol.md)
- [ADR-0001：app-scoped UtilityProcess](docs/architecture/adr/0001-app-scoped-utility-process.md)

## 安全约束

- Renderer：`nodeIntegration: false`
- Renderer：`contextIsolation: true`
- Renderer：`sandbox: true`
- Preload 只移交具名 MessagePort，不暴露任意 Electron IPC
- Runtime 自行校验所有 Renderer 消息
- Renderer 不提供 shell path、任意 argv 或任意 cwd

