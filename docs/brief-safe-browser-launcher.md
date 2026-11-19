# 任务：Matou 安全浏览器启动器（主进程模块）

## 目标

在 Matou 桌面主进程里增加一个启动器模块：检测「安全浏览器」是否安装、读取版本、管理常驻和临时资料、直接 spawn 内核、带时区和指纹参数、关窗后清理进程。这一关只做模块和测试，不改 UI、不改登录流程。

## 实际场景

Matou 将成为唯一入口。用户点登录时，Matou 直接用安全浏览器内核打开页面；添加 Claude 账户时，Matou 用同一内核开一次性资料。浏览器本身没有界面和启动器。

## 开发者意图

- 不依赖第二个 App 的 UI 或深链。
- 未安装时不影响现有登录，由调用方回退。
- 关闭浏览器窗口后不留后台进程。

## 项目意图

在 worktree `/Users/icesword/Documents/AIProjects/matou-login-integration`（分支 `feat/login-browser-integration`）。先读：

- `apps/desktop/src/main/` 现有主进程结构、`runtime-host.ts`
- 参考实现：`/Users/icesword/Documents/AIProjects/matou-egress-browser/src/main/browser.ts`（BrowserPipe 和关窗兜底已经过验收，可以移植，但不要把 Electron 面板代码带进来）

## 已经定下的决定（照做，不要改）

### 路径与版本

- App：`/Applications/安全浏览器.app`
- 可执行文件：`Contents/MacOS/Chromium`
- 版本：读 `Contents/Info.plist` 的 `CFBundleShortVersionString`
- 覆盖：`MATOU_SAFE_BROWSER_PATH` 指向任意 Chromium 可执行文件，仅用于测试。

### 参数

逐字使用：

```
--user-data-dir=<资料目录>
--timezone=<调用方传入的 IANA 时区>
--accept-lang=zh-CN,zh
--fingerprint=<固定种子>
--fingerprint-platform=macos
--no-first-run
--no-default-browser-check
--remote-debugging-pipe
<要打开的地址>
```

- 不传 `--lang`、`--proxy-server`、`--headless`、`--enable-automation`。
- 种子在 `app.getPath('userData')/safe-browser.json` 里生成一次（1 ~ 2147483647），之后固定，权限 0600。
- 常驻资料：`app.getPath('userData')/safe-browser/profile`，权限 0700，拒绝符号链接。
- 临时资料：`app.getPath('userData')/safe-browser/tmp/<随机>`，权限 0700，进程退出后删除。

### 时区解析

提供一个纯函数 `browserTimeZone(country?: string): string`，映射：

| 国家码 | 时区 |
|---|---|
| US | America/Los_Angeles |
| JP | Asia/Tokyo |
| SG | Asia/Singapore |
| TW | Asia/Taipei |
| HK | Asia/Hong_Kong |
| GB | Europe/London |
| DE | Europe/Berlin |
| 其他 / 缺失 | Asia/Singapore |

### 关闭

- 移植 `BrowserPipe` 的最后一页关闭 → `Browser.close`。
- 移植 2 秒兜底：请求关闭后主进程仍在，且没有新页面，则清理进程组（SIGTERM，2 秒不退再 SIGKILL）。
- App 退出时清理所有启动的浏览器和临时资料。
- 临时资料只在对应进程退出后删除；不要删除正在使用的目录。

### 接口

模块导出：

```ts
interface SafeBrowserStatus { installed: boolean; version: string | null }
function safeBrowserStatus(): SafeBrowserStatus
function browserTimeZone(country?: string): string
class SafeBrowserController {
  openPersistent(url: string, timezone: string): Promise<void>
  openEphemeral(url: string, timezone: string): Promise<{ done: Promise<void>; close(): Promise<void> }>
  closeAll(): Promise<void>
}
```

- `openPersistent` 复用同一个进程；已运行时用 pipe 新开页面。
- `openEphemeral` 每次新进程、新资料；`done` 在浏览器退出后 resolve；`close()` 主动关闭并清理。
- 未安装时抛出一个带稳定 code 的错误（例如 `safe-browser-missing`），由调用方回退；不要在这里弹窗或写 UI。

## 原因与方案

新功能。核心风险是进程生命周期：macOS 关窗后 Chromium 可能留在后台。参考实现里已经用私有 pipe 和 2 秒兜底解决，移植并补测试即可。

## 范围

- 工作目录：`/Users/icesword/Documents/AIProjects/matou-login-integration`
- 不能碰主目录、其他 worktree、`safe-browser` 项目、`/Applications` 下已安装的 App。
- 禁止 stash / reset / rebase。
- 不改 UI、不改 `runtime-host.ts`、不改 `isolated_browser.py`（这些是下一关）。
- 不改版本号、不发布、不推送。
- 测试可以用临时资料和临时 userData，不要动用户真实资料。

## 必须先问的地方

- 主进程没有合适的 userData 注入点，或测试需要改动全局路径，先提问。
- 移植 pipe 需要改 `apps/runtime` 或共享协议，先提问（预期不需要，放在 desktop main 内）。

## 关卡与交付物

一关完成：

1. `pnpm typecheck`、`pnpm test`、`pnpm build` 通过。
2. 新增测试覆盖：参数拼装、种子持久化、资料目录权限与符号链接拒绝、时区映射、安装检测、最后一页关闭触发 `Browser.close`、2 秒兜底、临时资料清理、未安装错误码。
3. 用 `MATOU_SAFE_BROWSER_PATH` 指向 `safe-browser/dist/安全浏览器.app/Contents/MacOS/Chromium`，真实启动一次常驻资料，通过 CDP 或页面自检确认时区正确；关闭后进程清零。
4. 真实启动一次临时资料，确认目录在退出后删除。
5. 提交 commit，信息 `feat: add the safe browser launcher`。

回复中给出：改动文件清单、新增测试名、真实启动的进程参数与关闭结果、临时资料清理证据、`我自己做的决定` 逐条列出。

## 验收（我会亲自复核）

- 读代码确认参数逐字一致、未安装路径只抛错不阻断、临时资料只在进程退出后删除。
- 亲自跑三条命令。
- 亲自用真实内核启动一次，核对时区和进程。
- 确认没有改 UI、`runtime-host.ts` 或 Python 工具。
