# 登录改道验收记录

> 当前品牌已更名为 **Mantle**，安装路径为 `/Applications/Mantle.app/Contents/MacOS/Chromium`，下载页为 https://github.com/icesword0760/mantle-browser/releases/latest 。本关最新验证见 [Mantle 改名验收](mantle-rebrand.md)。以下原始命令、输出和截图文字保留为历史证据。

本关完成订阅登录和 Claude 账户添加/重新登录的浏览器改道。没有改 UI、版本号、依赖声明或 `apps/runtime`，没有发布或推送。目标分支为 `feat/login-browser-integration`。

## 用户可见结果

- OpenAI / Antigravity 订阅授权经原有地址白名单校验后，优先使用共享安全浏览器控制器的常驻资料；成功后不打开系统浏览器。未安装或启动失败时，仅回退一次到原系统浏览器入口，无弹窗、重试或 URL 日志。
- Claude 添加/重新登录每次操作都取得最新出口时区与同一份持久化种子；内核/环境有效时使用安全浏览器内核和原有一次性独立资料。任何必需变量缺失、内核不可执行、TZ 非法或 SEED 不是正整数时，Python 完整沿用既有 Chrome/Edge/Brave 选择与参数。
- renderer 在原有 egress 缓存通知处提取 `families` 中第一个非空 `exit.country`，经 `ipcRenderer.send` 上报。没有增加轮询、RPC、React state 或监听者通知次数。main 保留最近有效两位国家码；尚无国家或映射不支持的国家使用 Singapore。

## 已确认的边界与取舍

监督回答已明确批准以下做法：

1. 本 worktree 缺少 `.venv`，原始输出为 `ls: tooling/claude-accounts/.venv: No such file or directory`。随后使用已有 `pnpm accounts:setup` 建立项目环境，uv 缓存也放在当前工作区，未全局安装。
2. 安全浏览器路径直接把授权 URL 作为 Chromium 最后一个参数，不生成 `browser-start.html`。地址会在进程参数中可见；Matou 不新增 URL 日志或文件。
3. **Chrome/Edge/Brave 回退路径沿用既有临时 HTML 行为，这是已知边界，本关不改。** 原有窗口和临时资料清理规则保持原样：窗口仍开着时保留资料，已关闭时在 flow 结束删除；不宣称仍活跃窗口的资料会即时删除。
4. 账户引擎第二参数兼容 env / env getter，保留 shell PATH 缓存，每次 run 更新或移除三个安全浏览器变量。避免引擎启动早于出口报告导致时区永久停在 Singapore。
5. `/Applications` 下尚未正式安装安全浏览器。真实验收批准沿用 `MATOU_SAFE_BROWSER_PATH` 指向上一关构建内核，只读执行，不修改内核项目或安装目录。

## 改动文件清单

| 文件 | 改动 |
| --- | --- |
| `apps/desktop/src/main/index.ts` | 创建单例，注入订阅路径及账户环境，接收国家 IPC |
| `apps/desktop/src/main/runtime-host.ts` | 通过既有白名单后优先常驻浏览器，所有启动异常回退 |
| `apps/desktop/src/main/safe-browser.ts` | 最近国家/时区，安全账户 env，读取既有种子的方法 |
| `apps/desktop/src/main/claude-accounts/engine.ts` | env getter 与每次操作的三变量更新，保留 PATH 缓存 |
| `apps/desktop/src/shared/desktop-api.ts` | fire-and-forget 国家上报契约和 channel |
| `apps/desktop/src/preload/index.ts` | 使用 send 上报国家 |
| `apps/desktop/src/renderer/src/runtime/RuntimeClient.ts` | 在既有缓存通知处上报国家，无组件改动 |
| `tooling/claude-accounts/isolated_browser.py` | 优先内核和环境校验，附加参数，安全路径不写重定向 HTML |
| `tooling/claude-accounts/browser_smoke.py` | 适配浏览器选择返回的附加参数 |
| `tooling/claude-accounts/README.md` | 记录改道、回退及既有清理边界 |
| `apps/desktop/src/main/runtime-host.test.ts` | 两个提供商成功、缺失及失败回退、白名单与日志检查 |
| `apps/desktop/src/main/safe-browser.test.ts` | 默认/最近国家、共享种子、安装与缺失环境 |
| `apps/desktop/src/main/claude-accounts/browser-env.test.ts` | 每次操作刷新/删除三变量，shell PATH 只解析一次 |
| `apps/desktop/src/preload/index.test.ts` | IPC 使用 send，不调用 invoke |
| `apps/desktop/src/renderer/src/runtime/RuntimeClient.test.ts` | 初始读取、push、更新的国家报告及无额外请求/通知 |
| `tooling/claude-accounts/test_isolated_browser.py` | 内核优先、非法环境、精确参数、独立资料与不写 HTML |
| `tooling/validate-login-browser.mjs` | 可手动复核的完整开发版测试入口及进程证据采集 |
| `docs/acceptance/login-browser-integration.md` | 本记录 |

## 自动验证

- `pnpm typecheck`：通过。
- `pnpm test`：退出码 0。domain 12、contracts 71、desktop 1068、desktop performance 3、runtime 1517 + 150 + 9 通过；desktop 1 项、runtime 36 项按既有配置跳过。
- `pnpm build`：通过。
- `tooling/claude-accounts/.venv/bin/python -m unittest discover -s tooling/claude-accounts -p 'test_*.py'`：32 项通过；只用假 CLI/参数和合成凭据，不触发真实登录。
- 定点 TypeScript：5 文件、103 项通过；随后全部纳入上面的全仓测试。
- `node --check tooling/validate-login-browser.mjs`、`git diff --check` 和最终 `pnpm check:identifiers`：通过。

初次类型检查暴露新增测试夹具字段不完整，已按真实 wire 契约修正；Python 首轮 NUL 环境变量用例无法通过 OS 写入，已移除这个无法实际传入的输入并复验。全仓原有 canvas `getContext()` 和 SQLite 实验特性警告没有导致失败。`MATOU_MODEL_ROUTER_BINARY` 未设置，既有真实路由内核集成套件未运行；本关用替身覆盖授权路径。

日志位于 `.cache/login-integration/{accounts-setup,focused,python,typecheck,test,build,visible}.log`，并同步到监督运行目录的 `logs/`。本次不提交生成日志与浏览器资料。

## 新增测试名

TypeScript：

- `opens %s subscription sign-in in the safe browser without opening the system browser`（codex / antigravity 两组）
- `silently falls back once to the system browser on %s`（safe-browser-missing / launch-failed 两组）
- `uses Singapore until a country arrives, retains the last valid country and maps unknown countries`
- `shares the persisted seed with account login and reads current country on each env call`
- `omits all safe-browser env when missing, including inherited values, without creating a seed`
- `refreshes safe-browser env for every operation, removes absent values and retains the shell PATH cache`
- `reports egress country through fire-and-forget IPC without invoking a request`
- `reports the first nonempty egress country on existing snapshot and push notifications without extra RPC or notifications`

Python：

- `test_safe_kernel_takes_priority_and_returns_exact_parameters`
- `test_missing_or_invalid_safe_env_falls_back_without_extra_parameters`
- `test_non_executable_kernel_uses_existing_browser_selection`
- `test_safe_launch_has_exact_arguments_unique_profiles_and_no_redirect_file`
- `test_login_forwards_safe_parameters_and_removes_closed_profile`

## 回退路径证据

| 场景 | 断言结果 |
| --- | --- |
| 安全浏览器抛出 `safe-browser-missing` | openPersistent 一次，原 openExternal 一次，runtime 返回成功，log/warn/error 均未调用 |
| 安全浏览器启动失败 | 同上，无重试 |
| 安全浏览器成功 | openPersistent 一次，openExternal 零次；两个提供商分别覆盖 |
| 非法授权地址 | 在浏览器调用之前拒绝，安全/系统浏览器均不启动 |
| main 检测未安装 | env 无三个变量，清除继承的旧值，不创建种子文件 |
| Python 缺失/非法 env、不可执行内核 | find_browser 返回原 Chrome 路径和空附加参数；已有临时 HTML、隔离参数、进程监控与清理测试继续通过 |
| 账户 env 第二次改变国家/卸载 | 后续进程分别收到新 TZ/无三个变量，shell PATH 缓存仍只读取一次 |

## 真实开发版验收

复核命令（先 `pnpm build`）：

```sh
MATOU_SAFE_BROWSER_PATH='/Users/icesword/Documents/AIProjects/safe-browser/dist/安全浏览器.app/Contents/MacOS/Chromium' node tooling/validate-login-browser.mjs
```

脚本启动完整开发版，真实 runtime 使用已有出口测试夹具，经过实际 renderer 缓存和 IPC 得到 Japan。只在生成的测试入口绑定 `Control+Shift+L`：注入模拟订阅请求，经过原始 runtime-host 白名单，再在控制器调用边界把模拟 URL 替换成本机网页。没有向授权服务发出请求，没有把测试热键、URL 替换或全局验收方法放入生产源码；临时入口在脚本结束时删除。

看到 `ready-for-manual-key` 后聚焦本次 Matou 并按热键。浏览器打开后，脚本保存进程证据并打印 `browser-open-quit-matou-after-visual-check`；检查网页后回到同一次 Matou 按 Cmd+Q。通过时打印 `stage: passed`。

本轮由指定 `gpt-6.1-terra / high` 子代理完成界面操作。它先用窗口 URL 中的 workspace 确认实例，再按热键；实际安全浏览器页面标题“Matou 登录改道验证”，显示“安全浏览器已打开”“本页是本地测试替身，不连接授权服务。”“浏览器时区：Asia/Tokyo”。之后重新确认 Matou 实例并 Cmd+Q，CUA 返回 App quit。主代理读回本地页面自检、真实 ps 和退出结果。CUA 截图只在工具输出中显示，未保存为文件，未把截图文件作为已交付证据。

首轮测试根目录过长使 macOS 控制 socket 路径达到 126 字节，页面停在加载；改用工作区内较短的 `.cache/l-*` 测试根目录后通过。此为测试环境修正，未改 runtime。

验收时间：`2026-11-15T06:45:56.070Z`。Matou PID `4587`；浏览器 PID / PGID `6982`。实际主进程参数：

```text
/Users/icesword/Documents/AIProjects/safe-browser/dist/安全浏览器.app/Contents/MacOS/Chromium
--user-data-dir=/Users/icesword/Documents/AIProjects/matou-login-integration/.cache/l-d2hllE/userData/safe-browser/profile
--timezone=Asia/Tokyo
--accept-lang=zh-CN,zh
--fingerprint=1593893757
--fingerprint-platform=macos
--no-first-run
--no-default-browser-check
--remote-debugging-pipe
http://127.0.0.1:50125/
```

- 实际资料目录：`/Users/icesword/Documents/AIProjects/matou-login-integration/.cache/l-d2hllE/userData/safe-browser/profile`。
- 页面实际时区：`Asia/Tokyo`。
- 退出前进程组：6；退出后：0。
- 运行中资料存在：`true`；退出后常驻资料仍存在：`true`。
- 完整机器证据：`.cache/login-integration/visible-result.json`（含关闭前全部 ps 行和关闭后空列表）。原有控制器和 runtime 退出钩子在完整 App 中协作通过，无需调整退出流程。

## 我自己做的决定

1. 空/非法国家报告不覆盖最近有效国家；合法但未知国家沿用既有 Singapore 映射。理由是保持“最近国家”语义，不为短暂空状态切换时区。
2. 保留现有两套退出钩子和完成标志；实际退出验证通过，不重做退出结构。
3. 账户内核路径沿用启动器已有 `MATOU_SAFE_BROWSER_PATH` 覆盖；默认仍为规定的 `/Applications` 路径，使共享控制器和账户引擎使用同一个内核。
4. 真实验收采用生成的测试入口、手动热键与本地替身，不在生产源码加入测试开关；缩短测试根目录满足 macOS socket 限制。理由是同时验证真实打开/退出并避免授权服务收到请求。
5. 界面子代理 `visible_acceptance` 自己做的决定：无。

## 未验证内容

- 不执行真实订阅授权、真实 `claude auth login` 或真实账户添加/重新登录；这是本关明确限制。未安装/启动失败的回退由替身验证。
- 正式 `/Applications` 安装尚未执行；真实内核验证使用已获批准的只读覆盖路径。
- Python 仍开着的临时浏览器资料保留既有行为；没有承诺关闭后异步即时回收。此边界已获得明确确认。
- 当前无实现阻塞，待用户亲自复核。
