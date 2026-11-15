# Mantle 启动器验收记录（历史验证）

> 当前品牌已更名为 **Mantle**，安装路径为 `/Applications/Mantle.app/Contents/MacOS/Chromium`，下载页为 https://github.com/icesword0760/mantle-browser/releases/latest 。本关最新验证见 [Mantle 改名验收](mantle-rebrand.md)。以下原始命令、输出和截图文字保留为历史证据。

本关只交付 desktop main 模块和测试；未接入登录流程或 UI，未修改 runtime-host.ts、Python 工具、版本号、依赖声明或其他项目。

## 改动文件

- `apps/desktop/src/main/safe-browser.ts`：安装/版本、国家时区、指纹种子、资料目录、进程生命周期和退出钩子。
- `apps/desktop/src/main/safe-browser-pipe.ts`：移植参考实现私有 CDP pipe，发现页面、开页、最后一页关闭时请求 Browser.close。
- `apps/desktop/src/main/safe-browser.test.ts`：37 项自动测试。
- `apps/desktop/src/main/safe-browser.real.test.ts`：显式启用的真实内核验收。
- `docs/acceptance/safe-browser-launcher.md`：本记录。

## 调用与边界

默认 `new SafeBrowserController()` 使用 Electron 的 `app.getPath("userData")`；可传入现有或父目录已存在的独立 userData 根目录，不修改 Electron 全局路径。构造时监听 will-quit，有活动进程时等待关闭和临时资料清理再继续退出。

常驻进程存活时后续打开通过 pipe 新建并激活页面；时区沿用进程首次启动值。closeAll 可重复调用并在完成后再次打开；App 退出后拒绝新启动。未安装仅抛出 code=safe-browser-missing，不展示 UI。

种子字段为 fingerprintSeed。损坏种子报错，避免悄悄改变指纹；已有种子与资料目录修正为0600/0700。userData、safe-browser、profile、tmp及种子文件拒绝符号链接。临时资料只删除本次启动创建的目录；父进程退出后先确认同组辅助进程已结束，再删除并完成 done。

## 自动验证

- `pnpm typecheck`：通过。
- `pnpm test`：退出码0。domain 12、contracts 71、desktop 1057、desktop performance 3、runtime 1517 + 150 + 9 项通过；desktop 1项、runtime 36项跳过。普通测试显式跳过真实浏览器用例，真实用例另行启用并通过。全仓运行后关闭竞态和权限细节补丁已用最终37项定点测试及真实内核复验覆盖。
- `pnpm build`：通过。
- 最终定点 `pnpm --filter @matou/desktop exec vitest run src/main/safe-browser.test.ts`：37 passed。
- `git diff --check`：通过。
- 初次直接运行 desktop typecheck 时工作区 packages 尚未构建，报缺少本地包类型；根命令按既定流程先构建 packages 后通过。

全仓已有 model-router 集成因 MATOU_MODEL_ROUTER_BINARY 未设置而跳过；本关不更改该套件或其依赖。App 退出钩子以隔离 Electron mock 验证，未接入真实 Matou 登录/退出流程。

## 新增自动测试名

- `detects an executable and reads the bundle short version without starting it`
- `uses the fixed installed application path when there is no override`
- `reports missing, non-executable and directory paths as not installed`
- `keeps an executable usable when its plist is missing or invalid`
- `builds exactly the approved arguments and uses a private detached pipe`
- `defaults to app userData while allowing an isolated constructor root`
- `persists one seed across controller instances and repairs private permissions`
- `rejects invalid persisted seeds instead of changing the fingerprint`
- `serializes concurrent persistent opens into one process and opens extra pages by pipe`
- `opens a new persistent process after the previous one exits`
- `sends Browser.close only when the last page is destroyed`
- `waits two seconds then terminates a windowless process group`
- `keeps the two-second fallback alive when Chromium closes its pipe`
- `does not terminate a process if a new page appeared during the fallback`
- `escalates from SIGTERM to SIGKILL after a further two seconds`
- `keeps explicit close authoritative and idempotent across late target events`
- `gives every ephemeral launch a private directory and deletes it only after exit`
- `cleans surviving helpers before deleting an exited parent profile`
- `cleans a failed spawn and allows a subsequent attempt`
- `returns a stable missing error code without creating browser data`
- `closes all processes on app quit and preserves the persistent profile`
- `closes an in-flight launch without allowing another process to start`
- `handles fragmented messages and rejects pending commands when the pipe ends`
- `maps %s to %s`：10组国家码/缺失/未知/小写输入。
- `rejects a symlink at %s before spawning`：safe-browser、profile、tmp、seed四组。

真实测试：`real kernel: verifies page timezones, final-window exit, process groups and ephemeral deletion`。

## 真实内核验证

时间：2026-11-15T06:22:30.779Z。Info.plist 版本：`148.0.7778.215`。所有资料位于本工作区 `.cache/safe-browser/`，保留常驻资料供复核，未使用用户真实资料。

复跑命令：

```sh
MATOU_SAFE_BROWSER_REAL_TEST=1 MATOU_SAFE_BROWSER_PATH='/Users/icesword/Documents/AIProjects/safe-browser/dist/安全浏览器.app/Contents/MacOS/Chromium' pnpm --filter @matou/desktop exec vitest run src/main/safe-browser.real.test.ts
```

页面自检在本机127.0.0.1页面运行 `Intl.DateTimeFormat().resolvedOptions().timeZone` 并回报结果。随后通过 CDP 关闭最后一个 page target，未调用 closeAll 协助完成断言；验证进程组不存在后才执行 finally 清理。

### persistent

真实启动参数（按原顺序）：

```text
/Users/icesword/Documents/AIProjects/safe-browser/dist/安全浏览器.app/Contents/MacOS/Chromium
--user-data-dir=/Users/icesword/Documents/AIProjects/matou-login-integration/.cache/safe-browser/real-userData-HadAtJ/safe-browser/profile
--timezone=Asia/Tokyo
--accept-lang=zh-CN,zh
--fingerprint=1956414013
--fingerprint-platform=macos
--no-first-run
--no-default-browser-check
--remote-debugging-pipe
http://127.0.0.1:59776/?kind=persistent
```

- PID / PGID：`59250`；页面时区：`Asia/Tokyo`。
- 关闭前进程组：6 个进程；关闭后：0 个。
- 资料目录：`/Users/icesword/Documents/AIProjects/matou-login-integration/.cache/safe-browser/real-userData-HadAtJ/safe-browser/profile`。
- 运行中存在：true；退出后存在：true。

### ephemeral

真实启动参数（按原顺序）：

```text
/Users/icesword/Documents/AIProjects/safe-browser/dist/安全浏览器.app/Contents/MacOS/Chromium
--user-data-dir=/Users/icesword/Documents/AIProjects/matou-login-integration/.cache/safe-browser/real-userData-HadAtJ/safe-browser/tmp/profile-TISp3j
--timezone=America/Los_Angeles
--accept-lang=zh-CN,zh
--fingerprint=1956414013
--fingerprint-platform=macos
--no-first-run
--no-default-browser-check
--remote-debugging-pipe
http://127.0.0.1:59776/?kind=ephemeral
```

- PID / PGID：`59624`；页面时区：`America/Los_Angeles`。
- 关闭前进程组：6 个进程；关闭后：0 个。
- 资料目录：`/Users/icesword/Documents/AIProjects/matou-login-integration/.cache/safe-browser/real-userData-HadAtJ/safe-browser/tmp/profile-TISp3j`。
- 运行中存在：true；退出后存在：false。

完整机器证据：`.cache/safe-browser/real-launch.json`，含主进程完整参数及关闭前后 ps 记录。额外检查内核路径及本次资料路径，在 ps 中均无残留进程。内核自身派生 helper 的参数由内核决定；本模块传给主进程的参数严格为上述九项。

## 参考实现对照

| 场景 | 参考实现 | Matou 模块 | 证据 |
| --- | --- | --- | --- |
| 私有控制 | remote-debugging-pipe | 同样使用fd3/fd4，不开调试端口 | 参数断言及真实启动 |
| 再次打开 | Target.createTarget + activate | 同逻辑，额外串行化并发常驻打开 | 并发/复用测试 |
| 最后一页关闭 | Browser.close | 原逻辑移植 | 自动测试及真实CDP关页退出 |
| macOS后台常驻 | 2秒后检查页面再清进程组 | 同逻辑，pipe断开不撤销兜底 | fake timer测试 |
| 强制结束 | SIGTERM，2秒后SIGKILL | 同逻辑 | fake timer测试 |
| 临时资料 | 本关新增要求 | 对应父进程及辅助进程退出后删除 | 自动测试及真实目录消失 |

## 我自己做的决定

1. 使用可选 userData 构造参数，默认读取 app 路径；不改全局路径。
2. 退出钩子放在控制器内部，保留本关独立交付边界。
3. 种子JSON字段采用 fingerprintSeed；损坏时抛错，已有权限修正而不重建资料。
4. 常驻进程复用时不重启，沿用首次启动时区；符合复用同一进程要求。
5. 等同组辅助进程清零再删除临时资料，done 也等待这一步。
6. 主动关闭保持幂等，晚到页面事件不得取消主动关闭。
7. 真实测试显式环境变量启用，工作区内保留独立常驻验收资料和JSON证据。

## 未验证范围

真实 Matou App 退出事件与现有 runtime 退出钩子协作尚未在完整 App 中验证；已用 mock 覆盖控制器钩子。本关按要求不接 UI 和登录调用。
