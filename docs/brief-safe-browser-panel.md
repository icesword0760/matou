# 任务：Matou 网络检查面板增加安全浏览器卡片与客户端提醒

## 目标

按已确认的设计稿，在 Matou 的网络检查面板增加：

1. 「安全浏览器」卡片：安装状态、版本、「检查更新」「打开」/「获取安装包」。
2. 正在运行的 Claude Desktop / ChatGPT 桌面版提醒。

## 实际场景

用户要知道安全浏览器有没有装、是什么版本，并能从这里打开它或检查更新。桌面客户端无法被 Matou 控制，但用户至少要知道它没被保护。

## 开发者意图

- 卡片是状态和安装入口，不重复放登录按钮。
- 提醒只在检测到客户端时出现，不弹窗、不阻断。
- 未安装时登录仍可用，卡片只是提示。

## 项目意图

在 worktree `/Users/icesword/Documents/AIProjects/matou-login-integration`（分支 `feat/login-browser-integration`）。先读：

- `docs/mockups/matou-login-browser-integration.html`（第一份稿子）和 `docs/mockups/matou-login-interaction.html`（第三部分）
- `apps/desktop/src/renderer/src/egress/EgressPanel.tsx`、`egress.css`、`use-egress-status.ts`
- `apps/desktop/src/main/safe-browser.ts`（上一关）
- `apps/desktop/src/main/external-url-policy.ts`
- `apps/desktop/src/shared/desktop-api.ts`、preload

## 已经定下的决定（照做，不要改）

### 卡片

位置：网络检查面板底部、参考检查之前。文案逐字：

- 标题：`安全浏览器`
- 已安装：`已安装 · <版本>`
- 未安装：`未安装`
- 小字：`登录 Claude / ChatGPT / Antigravity 时使用`
- 已安装按钮：`检查更新`、`打开`（两个都要）
- 未安装按钮：`获取安装包`

行为：

- `打开`：用常驻资料打开 `about:blank`，时区用 `safeBrowserTimeZone()`。
- `检查更新`：
  - 用 Electron `net.fetch` 请求
    `https://api.github.com/repos/icesword0760/safe-browser/releases/latest`
  - 比较 `tag_name` 与已安装版本。
  - 结果在卡片小字位置临时显示 3 秒：
    `已是最新` / `有新版本 · v<版本>` / `检查失败`。
  - 不自动下载、不自动替换。
  - 仓库地址集中定义在一个常量里，后续可改。
- `获取安装包`：打开 `https://github.com/icesword0760/safe-browser/releases/latest`，需要加进外链白名单（严格 host + 路径前缀）。

### 客户端提醒

- 检测方式与文案见上一版决定：`pgrep -f` 检测 Claude Desktop 和 ChatGPT 桌面版。
- 位置：面板「本机」检查之后、控制项之前。
- 文案逐字：
  - `检测到 Claude Desktop 正在运行，它的出口不在保护范围`
  - `检测到 ChatGPT 桌面版正在运行，它的出口不在保护范围`
- 样式沿用 `egress-check.is-warn` 或同等的黄色提示，不新增颜色。
- 面板打开时检测一次，之后每 5 秒刷新；面板关闭时停止。失败返回空数组。

### 接口

新增 IPC 与 preload：

- `safeBrowserStatus(): Promise<{ installed: boolean; version: string | null }>`
- `safeBrowserOpen(): Promise<void>`
- `safeBrowserCheckUpdate(): Promise<{ latest: string | null; failed: boolean }>`
- `openSafeBrowserReleases(): Promise<void>`
- `runningDesktopClients(): Promise<Array<'claude' | 'chatgpt'>>`

IPC sender 校验沿用现有模式；不接受渲染层传路径或 URL。

### 文案与状态

- 版本号直接显示，不加 `v` 前缀（`已安装 · 148.0.7778.215`）。
- 更新结果里的新版本带 `v`（`有新版本 · v148.1.0.0`）。
- 「检查更新」进行中按钮禁用，文字不变。
- 卡片不显示「已从 Matou 打开」之类的额外提示。

## 原因与方案

新 UI。方案按稿子实现；更新检查走 GitHub Releases API，失败不阻断。客户端提醒用 `pgrep`，只在 macOS 上做，失败静默。

## 范围

- 工作目录：`/Users/icesword/Documents/AIProjects/matou-login-integration`
- 不能碰主目录、其他 worktree、`safe-browser` 项目。
- 禁止 stash / reset / rebase。
- 不改版本号、不发布、不推送。
- 不改订阅登录、账户添加、`apps/runtime` 的出口逻辑。

## 必须先问的地方

- `EgressPanel` 的现有布局放不下卡片或提醒（需要改面板高度/滚动结构），先提问。
- GitHub API 请求在 `net.fetch` 下被 CSP 或网络策略拦截，先提问，不要改用 Node `https`。

## 关卡与交付物

一关完成：

1. `pnpm typecheck`、`pnpm test`、`pnpm build` 通过。
2. 新增测试覆盖：卡片已安装/未安装、检查更新三种结果、客户端两种提醒、外链白名单、IPC sender 校验。
3. 真实启动 Matou：面板显示「已安装 · 148.0.7778.215」；点「打开」能打开安全浏览器；点「检查更新」有结果（仓库未发布时显示检查失败，允许）。
4. 启动 Claude.app 或 ChatGPT.app（如果可用）验证提醒；不可用则用测试替身。
5. 提交 commit，信息 `feat(egress): show safe browser status and desktop client notices`。

回复中给出：改动文件清单、新增测试名、卡片和提醒的截图或文字证据、检查更新的实际结果、`我自己做的决定` 逐条列出。

## 验收（我会亲自复核）

- 读代码确认没有接受渲染层传路径/URL，外链白名单严格。
- 亲自跑三条命令。
- 亲自打开面板看卡片和提醒。
- 亲自点「打开」和「检查更新」。
