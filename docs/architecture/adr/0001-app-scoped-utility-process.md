# ADR-0001：V1 Runtime 使用应用生命周期内的 UtilityProcess

- 状态：Accepted
- 日期：2026-08-24

## 背景

终端输出是高频数据流。由 Electron Main 转发会增加一次消息处理并让窗口生命周期、桌面能力和 PTY 压力集中在同一进程。产品同时需要 Renderer reload 后会话继续运行，以及未来升级为长期智能体任务。

候选方案：

1. PTY 运行在 Main；
2. Node `child_process.fork()`；
3. Electron `utilityProcess.fork()`；
4. 独立系统 daemon，通过 Unix Socket/Named Pipe 通信。

## 决策

V1 采用 Electron `utilityProcess.fork()`：

- Runtime 随 Electron 应用启动和退出；
- Main 创建 `MessageChannelMain`，把两端移交给 Renderer 和 Runtime；
- 高频 Terminal Data 不经过 Main；
- 协议只接受当前应用内置的精确版本；
- Runtime 重启后从 SQLite、Checkpoint 和 Journal 重建状态。

## 原因

- UtilityProcess 原生支持 MessagePort，与 Renderer 建立直接通道；
- PTY 或 Runtime 崩溃不直接拖垮 Main；
- 进程仍由应用统一打包、升级和监督；
- V1 省去 daemon 安装、单实例、跨版本兼容、权限和残留进程治理。

## 后果

正面：

- Main 不进入 Terminal Data 热路径；
- Renderer reload 不要求重启 PTY；
- Runtime 与应用版本天然一致；
- node-pty 原生模块集中在 Runtime。

代价：

- 用户退出应用后，PTY 和 Agent 进程随之结束；
- 应用重启属于 revive/resume，不是连接旧 PTY；
- Runtime 崩溃需要 Journal 与 provider session 恢复。

## 排除方案

### PTY 运行在 Main

让高频输出、原生窗口生命周期和业务执行共享事件循环，故障域过大。

### child_process.fork

能隔离 PTY，但与 Electron Renderer 之间缺少 UtilityProcess 提供的直接 MessagePort 集成，需要 Main 转发或另建 socket。

### 独立 daemon

支持应用退出后继续运行，但 V1 会提前承担版本协商、安装升级、权限、孤儿进程和多实例仲裁。

## 迁移触发器

满足任一条件时新增 ADR：

- 产品承诺应用退出后任务继续；
- 多个桌面实例共享 Runtime；
- 引入远程 Runtime；
- Runtime 独立发布或跨版本滚动升级。

届时 transport 迁移到 Unix Domain Socket/Named Pipe，保留 Domain、Journal、Checkpoint 和消息语义。
