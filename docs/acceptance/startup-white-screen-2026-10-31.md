# 2026-10-31 启动白屏修复

## 用户影响与原因

已安装的 0.2.15 在启动时停留在空白窗口。现场页面加载成功，根节点为 `hierarchy-loading`，Runtime 阶段为 `opening-database`。进程采样确认同步 SQLite 查询占用启动路径，代码对应 `PRAGMA quick_check`。

原数据库为 82,513,330,176 字节（约 76.85 GiB），事件序号已达到 12,825,702。抽样显示连续的 `provider-binding.recorded` / `session.mode-changed`，主要来自恢复后每次状态栏刷新都重新记录身份、完整状态快照与命令返回值。

## 产品行为修复

- 每次运行首次确认身份仍正常持久化；同一会话的连续状态栏刷新不再重复记录身份和模式变更。
- 用量、上下文等 HUD 数据仍逐次更新；真实生命周期事件、模型、权限、目录或会话身份变化继续正常处理。
- 正常快速启动保持原样；等待超过 1.2 秒后显示打开工作区及恢复状态的文字，避免无反馈白屏。
- 启动完整性检查与数据库损坏恢复逻辑保持有效。

## 用户授权的数据归档

用户明确选择归档内部流水并优先恢复启动。完整原数据库与 WAL 保存在 `/Users/icesword/.matou/archives/startup-20261031/`，原安装版也保留为 `previous-0.2.15.app`。

新数据库保留所有其它表，并逐表核对行数与内容摘要。仅归档最近 10,000 条事件之前的 provider hook 身份/模式流水及对应旧命令记录，以及旧目录检查命令记录；保留原事件序号上限，避免重用序号。聊天文件与终端 journal/checkpoint 不作改动。

## 验证

- Provider hook 与模式转换测试：38 项通过。
- 页面启动状态测试：12 项通过。
- 存储完整性与数据库启动测试：35 项通过。
- 真实 Electron 数据库头部/中段损坏恢复：2 项通过。
- 本次实际候选安装包启动、PTY、历史回放、断尾恢复、前向 schema 兼容：1 项通过（含多次重启）。
- 桌面与 Runtime 类型检查、构建、命名门禁、diff 空白检查通过。
- 当前只有主显示器；Electron 测试使用现有 `MATOU_E2E_DISPLAY=primary` 选项完成。
- 本机候选由当前 0.2.17 源码加本次修复构建，采用本机 ad-hoc 签名，深度签名校验通过；未发布线上更新。

## 本机恢复验收

- 运行数据库从 82,513,330,176 字节缩至 159,780,864 字节（152.38 MiB）；完整原库保留在上述归档目录，未宣称释放对应磁盘空间。
- 其它表逐表行数和内容摘要一致；所有保留的事件、命令逐行与原库主键记录核对一致。schema、索引及自增序号一致，外键检查零错误。新数据库独立完整性检查通过（约 0.051 秒）。核验报告：归档目录内 `compaction-report.json`。
- `/Applications/码头.app` 已替换并实际启动，工作区侧栏、原选中的“预演”事项、两个 Claude 终端及原历史内容恢复可见。
- 实际大数据现场仍需要恢复终端历史，首次观察约 35–40 秒后进入主界面；不能据此宣称秒开。超过 1.2 秒的等待已有明确反馈。
- 启动约 10 分钟后，新增 provider 身份/模式事件各 67 条，与恢复时首次确认相符；连续观察未继续随状态栏刷新增长。
- 用户随后报告 CPU 负载高。现场发现 68 个 Claude 和 2 个 Shell 已自动恢复，码头进程树约 267 个进程，RSS 合计约 32 GiB（含共享部分），进程 CPU 合计采样约 55%；码头自身约 12%，系统文件服务 fileproviderd 约 117%–140%。这些 CPU 数值按单核心 100% 计；整机当时忙碌约 75%–84%。后台全量恢复带来的资源负担仍需另行处理，未擅自结束用户会话。
- 本次改动留在本地工作区，未提交、推送或发布。

## 系统文件服务高 CPU 的进一步定位

- 2026-10-31 17:31:17，系统日志对 CloudMounter 的 nas1、pic 两个域明确记录：`reset stream: FSEvent stream got reason: device UUID changed`；同一启动阶段 iCloud 域也出现该记录。17:31:25 系统启动 stream-reset 后台任务。该证据说明文件变更事件流标识不匹配触发重新核对，不能据此推断硬盘损坏或确定是谁改变了标识。
- 用户授权断开后，CloudMounter 两个连接均显示可重新挂载；系统诊断也确认 `temporarily disconnected: 在 CloudMounter 中卸载磁盘`。但两个域的同步调度仍为 `running`，nas1 的 `stream reset` 仍为 `yes`。远程断开没有终止已经在运行的本地状态核对。
- 两个域的 reconciliation 状态树分别为 366,114 和 368,488 条记录（合计 734,602）。这是状态树规模，不代表全部都是待处理队列。系统仍在读取 NAS 照片、手机备份及 `@eaDir` 缩略图目录的本地文件元数据。
- 进程采样中两个 CloudMounter 域的数据库队列和 `com.apple.fssync.fstree` 为主要工作栈，涉及 SQLite 状态写入、扩展属性读取、路径查找；Spotlight indexer 显示 disabled / not indexing。当前负载与这两套文件状态核对相关，不是 Spotlight 正在建立搜索索引。
- 断开后最新采样 fileproviderd 为 171.4%（约 1.7 个逻辑核心），码头 Runtime 为 9.5%、Renderer 为 0.2%；不能把 fileproviderd 的单进程百分比解释成整机占用率。未删除系统 FileProvider 缓存、连接配置或 NAS 文件。
- 现场只读证据暂存：`/tmp/fileprovider-startup-log.txt`、`/tmp/fileprovider-state.txt`、`/tmp/fileprovider-investigation.sample.txt`。具体标识变化的更上游原因仍未确定，也没有足够证据断言该扫描是永久死循环。

## 用户授权的临时降载

- 用户明确选择「先暂停同步，优先恢复电脑流畅」，已说明 iCloud 上传下载也会暂停。
- 正常重启 FileProvider 服务后任务继续；单独将 CloudMounter 扩展设为 ignore 也未停止系统已接管的核对。该扩展选择已恢复 default，未保留无效的禁用设置。
- 将服务置于后台优先级后 CPU 有所下降，但仍繁忙。系统不允许通过 launchctl bootout 卸载受 SIP 保护的服务；未修改 SIP 或其它保护设置。
- 最终对当前 fileproviderd PID 28484 发送 SIGSTOP，确认状态 T，连续三次 5 秒采样 CPU 均为 0.0%，累计 CPU 时间稳定在 1:47.62。属于本次运行的临时暂停，不是同步状态数据库的永久修复；恢复服务后可能继续核对。
- 恢复脚本：`/Users/icesword/.matou/archives/startup-20261031/resume-system-file-sync.command`，向 fileproviderd 发送 SIGCONT 并取消本次后台优先级；脚本语法检查通过。重启电脑也会结束这次进程暂停。
- 暂停后整机一次采样仍约 69% 忙碌，47 GiB 内存已使用，15 GiB 压缩内存。系统文件服务的 CPU 已停止，但不能宣称整机所有资源压力已经解决；此前恢复的终端/代理会话仍保留。
