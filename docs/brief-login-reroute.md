# 任务：Matou 登录改道（订阅登录 + Claude 账户添加）

## 目标

把 Matou 现有的两条登录路径改走安全浏览器：

1. 订阅登录（OpenAI / Antigravity）用安全浏览器的常驻资料打开授权页。
2. 添加 / 重新登录 Claude 账户用安全浏览器内核 + 一次性临时资料。

未安装安全浏览器时，两条路径都完全回退到现有行为，不报错、不阻断。这一关不改 UI。

## 实际场景

用户点 Matou 里的登录按钮，浏览器由 Matou 直接启动，不需要第二个 App 的界面。账户添加仍然要保持一次性隔离，不能把上一个账号的登录态带进来。

## 开发者意图

- 未安装时用户完全无感，还是原来的 Chrome / 系统浏览器。
- 授权地址不写日志、不落盘。
- 账户添加的临时资料用完即删。

## 项目意图

在 worktree `/Users/icesword/Documents/AIProjects/matou-login-integration`（分支 `feat/login-browser-integration`）。先读：

- 上一关：`apps/desktop/src/main/safe-browser.ts`、`safe-browser-pipe.ts`
- `apps/desktop/src/main/runtime-host.ts` 里 `isSubscriptionAuthorizationUrl` 之后调用 `openExternal` 的位置
- `apps/desktop/src/main/index.ts` 里 `new ClaudeSwapEngine(accountLaunch)` 的位置
- `tooling/claude-accounts/isolated_browser.py` 和它的测试

## 已经定下的决定（照做，不要改）

### 共享控制器与出口国家

- 在 desktop main 创建一个单例 `SafeBrowserController`，供 `runtime-host.ts`、IPC 和账户引擎共用。
- 新增 renderer → main 的 IPC（fire-and-forget）：把当前出口国家（`EgressStatusWire.families` 里第一个非空 `exit.country`）报告给 main。
- main 保存最近一次国家；`safeBrowserTimeZone()` = `browserTimeZone(最近国家)`，没有则 `Asia/Singapore`。
- 在现有 egress 状态订阅处上报，不要新增轮询。

### 订阅登录

- 在 `isSubscriptionAuthorizationUrl` 校验通过之后：
  - 调 `controller.openPersistent(request.url, safeBrowserTimeZone())`。
  - 任何启动错误（包括 `safe-browser-missing`）都回退到现有 `this.#openExternal(request.url)`。
  - 不重试、不弹窗、不记录 URL。
- 如果安全浏览器成功打开，不再调用系统浏览器。

### Claude 账户添加

- `new ClaudeSwapEngine(accountLaunch, env)` 的 env 增加三个变量（仅在安全浏览器已安装时设置）：
  - `MATOU_SAFE_BROWSER_BIN`：`/Applications/安全浏览器.app/Contents/MacOS/Chromium`
  - `MATOU_SAFE_BROWSER_TZ`：`safeBrowserTimeZone()`
  - `MATOU_SAFE_BROWSER_SEED`：控制器使用的固定种子
- 提供一个读取种子的方法，不要重复生成；种子文件路径与上一关一致。
- `isolated_browser.py`：
  - `find_browser()` 优先使用 `MATOU_SAFE_BROWSER_BIN`（必须可执行），并返回附加参数：
    `--timezone=<TZ>`、`--accept-lang=zh-CN,zh`、`--fingerprint=<SEED>`、`--fingerprint-platform=macos`
  - 变量缺失、内核不可执行、TZ 非法、SEED 非正整数：回退现有 Chrome/Edge/Brave，行为与现在完全一致。
  - 保留临时资料、`--incognito`、`--new-window`、`--disable-background-mode` 和进程监控。
  - 不把这三个变量或内核路径写入日志。

### 不改的部分

- 不改 UI、面板卡片、客户端提醒（下一关）。
- 不改 `apps/runtime` 的出口检测。
- 不改 Python 的登录流程、授权地址匹配、临时资料清理逻辑。

## 原因与方案

原因：两条登录路径目前分别走系统浏览器和普通 Chrome。方案：复用上一关的启动器，订阅登录用常驻资料，账户添加用一次性资料；未安装时完全回退。出口国家通过一个轻量 IPC 从 renderer 同步到 main，避免为时区再开 runtime RPC。

## 范围

- 工作目录：`/Users/icesword/Documents/AIProjects/matou-login-integration`
- 不能碰主目录、其他 worktree、`safe-browser` 项目、`/Applications` 下的 App。
- 禁止 stash / reset / rebase。
- 不改版本号、不发布、不推送。
- 验收不要触发真实的订阅授权（会改变账号状态）；用测试替身。
- 账户添加的 Python 测试用假参数，不触发真实 `claude auth login`。

## 必须先问的地方

- 共享控制器与现有 `will-quit` 清理流程冲突，先提问。
- IPC 上报出口国家会引入新的渲染层渲染或请求，先提问。
- `isolated_browser.py` 的测试环境缺少 `.venv`，报告原文，不要全局安装。

## 关卡与交付物

一关完成：

1. `pnpm typecheck`、`pnpm test`、`pnpm build` 通过；Python 账户工具测试通过。
2. 新增测试覆盖：订阅登录成功走安全浏览器、未安装回退系统浏览器、启动失败回退、出口国家 IPC、时区解析、Python 优先内核与回退、附加参数逐字、非法 env 回退。
3. 真实启动 Matou（开发版）：手动触发一次订阅登录的浏览器打开路径（可以用测试注入的授权 URL，不要真的完成授权），确认打开的是安全浏览器、资料目录正确。
4. 提交 commit，信息 `feat: route sign-in through the safe browser`。

回复中给出：改动文件清单、新增测试名、真实打开时的进程参数、回退路径的测试证据、`我自己做的决定` 逐条列出。

## 验收（我会亲自复核）

- 读代码确认未安装路径完全回退、URL 不写日志。
- 亲自跑 typecheck / test / build 和 Python 测试。
- 亲自用测试注入的 URL 触发一次，核对浏览器进程和资料目录。
- 确认没有改 UI 和 `apps/runtime`。
