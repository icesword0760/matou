# macOS 更新后的文稿权限连续性修复

## 问题与来源

2026-11-16 15:17:56（Asia/Shanghai）的一次本地安装，将 `/Applications/码头.app` 移到 `~/.Trash/码头-20261116-151756.app`，然后复制本地构建覆盖安装。原包使用 Developer ID / Team `JAKX72KQ6X`；新包为 ad-hoc，指定代码要求绑定 `cdhash`。当时 `codesign --verify --deep --strict` 通过，但该检查只能说明包完整，不能证明签名身份连续。

系统 tccd 日志证实旧版遗留的 `agy` / `git` 等进程仍被归属到废纸篓旧包，同时新主进程使用 ad-hoc 包；同一 bundle ID 在两种代码要求之间交替触发 `Failed to match existing code requirement` 与 `AUTHREQ_PROMPTING`。因此不仅更新时会询问，两个身份同时访问时还会反复询问。相关旧进程 PID 12643、18006、28042、12626 在修复时已退出，没有强杀这些用户进程。

## 修改

- `apps/desktop/package.json` 固定 `mac.type=distribution`、既有 Developer ID 身份和 `forceCodeSigning=true`，防止正式安装包悄悄回退到临时或开发签名。
- 新增 `pnpm install:mac --source <绝对路径.app>`：检查完整性、Developer ID、Team、bundle ID、稳定指定代码要求，拒绝临时签名；已安装正式版本时验证新包满足旧包要求。
- 安装前及替换前检查相关进程，运行时停止安装；同卷暂存并检查，新旧包不在运行中移动。原包存为 ZIP，替换失败尝试恢复，保留备份路径。
- `AGENTS.md` 规定统一安装流程，禁止把旧安装 bundle 改名移动到带时间戳 `.app` 或用 TCC 重置 / 完整磁盘权限掩盖问题。

## 本机处理

当前应用内容从已安装版本复制后，使用现有 Developer ID 重新签名；业务源码与 app.asar 不变（app.asar SHA-256 `433d7330f65fd1db63192812d9292ec283a8c6667f848f03b09d74774d59a41b`）。新签名 DR 与旧正式包相同。

- 时间戳旧包已通过 Launch Services 单独注销并生成可恢复 ZIP：`~/.matou/backups/permission-repair-20261116/previous-developer-id.zip`，ZIP CRC 及关键文件内容校验通过。macOS 拒绝移除原废纸篓 bundle 的 Contents，未绕过系统权限；原目录仍保留，ZIP 可恢复。
- 原 ad-hoc 安装包备份：`~/.matou/backups/local-install-2026-11-16T07-43-24-273Z-gEYjid/previous.zip`。
- 未重置或修改 TCC 数据库，未扩大完整磁盘访问权限。正式签名迁移需要用户一次性授权，用户已完成。

## 验证

- 自动检查：4 项安装器测试通过（正式签名/拒签、运行中阻断及路径边界、ad-hoc 迁移、替换失败回滚）；electron-builder 实际配置 schema 校验通过；`pnpm check:identifiers` 通过。业务代码未变，未重复执行业务全量测试。
- 本机验签：新包通过深度严格验签，Developer ID / Team / bundle ID / DR 校验通过，安装器实际完成迁移。
- 真实 UI：正式版启动后恢复原 SceneCraft 工作现场与终端。首次迁入固定正式身份时用户允许文稿访问，tccd 于 15:44:19 记录响应，随后返回 Allowed (User Consent)。
- 更新测试：在暂存副本 Info.plist 添加仅用于本轮验证的自定义字段，重新签顶层包，CDHash 从 `1b1353ad6a316fee673ce0223f969a8df9b07ebb` 变为 `e6294c80fcb00029e7a432cbde3d14bcebb9f834`，DR 不变。正常退出、使用安装器替换并重新启动；15:46:23 tccd 对新 PID 2490 文稿访问直接返回 Allowed (User Consent)，该次启动未出现 AUTHREQ_PROMPTING；工作区与终端恢复。
- 最终版回装：已再次正常退出、经同一安装器替换回无验证字段的正式签名包；15:48:37 tccd 对最终 PID 12474 文稿访问直接返回 Allowed (User Consent)，无重新授权提示。最终包通过完整性及身份校验，自定义验证字段不存在，临时签名/测试 app 目录已移除。最终真实界面已恢复 SceneCraft 工作现场和两个已启动终端；其它未自动恢复会话仍按原有最近 16 条恢复策略显示。

此结果验证本机在固定开发者签名与正常安装路径下的权限连续性；未验证更换证书团队、另一个 macOS 用户、系统权限被用户撤销等不同条件，也不保证绕开安装器手工复制临时签名包后仍可保留授权。

原始运行证据位于 `docs/acceptance/evidence/macos-permission-continuity-2026-11-16/`。
