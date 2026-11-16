# Mantle 品牌改名验收

工作区：`matou-login-integration`；分支：`feat/login-browser-integration`。本关仅改 Matou 侧品牌、固定安装路径和下载仓库，不改版本，不发布或推送。

## 当前用户体验

- 中文与英文卡片标题、无障碍名称均为 `Mantle`。
- 默认检测 `/Applications/Mantle.app/Contents/MacOS/Chromium`，版本从其 Info.plist 读取。本机安装版与构建版版本均为 `148.0.7778.215`。
- 已安装：`已安装 · 148.0.7778.215`，按钮仍为「检查更新」「打开」，提示仍为「登录 Claude / ChatGPT / Antigravity 时使用」。
- 未安装：标题仍为 `Mantle`，状态「未安装」，按钮「获取安装包」。
- 固定下载页：`https://github.com/icesword0760/mantle-browser/releases/latest`；更新检查访问同仓库 GitHub API。
- 启动参数、时区、种子、`safe-browser` 资料目录、IPC、错误码、登录改道和回退逻辑不变。`runningDesktopClients` 仅检测 Claude / ChatGPT，原来就不含本浏览器品牌，无需改动。

## 改动文件

- `apps/desktop/src/main/safe-browser.ts`：固定安装路径与错误文案。
- `apps/desktop/src/main/external-url-policy.ts`：下载和更新仓库。
- `apps/desktop/src/renderer/src/egress/EgressPanel.tsx`：中英文标题与无障碍名称。
- `apps/desktop/src/main/{safe-browser,external-url-policy,safe-browser-panel,safe-browser.real}.test.ts`：品牌、路径、仓库断言与本地验收页面标题。
- `apps/desktop/src/renderer/src/egress/EgressPanel.safe-browser.test.tsx`：复用现有用例覆盖中英文已安装卡片及未安装卡片品牌。
- `tooling/validate-login-browser.mjs`：本地验证页文案；`tooling/claude-accounts/README.md`：当前品牌说明。
- `docs/mockups/{login-browser-integration,matou-login-interaction}.html`：仅替换品牌字样。
- 旧启动器和登录验收文档新增当前品牌及本记录链接，原始命令/输出保留为历史证据；历史 brief 不改。

## 自动验证

- `env -u TMPDIR CI=1 pnpm typecheck`：退出 0。
- `env -u TMPDIR CI=1 pnpm test`：退出 0；2880 项 Vitest 通过、37 项按既有配置跳过，6 项 Node 工程门禁通过。未改重试、超时或性能预算。真实路由协议用例因未设置 `MATOU_MODEL_ROUTER_BINARY` 沿用既有跳过条件，不计为已验证。
- `env -u TMPDIR CI=1 pnpm build`：退出 0。
- 首轮定点 5 文件 73 项通过；首轮全量类型检查发现新增英文测试误用 `en-US`，修正为项目已有 `en` 后，上述最终检查全部通过。
- 全量测试沿用既有系统默认临时目录口径；真实浏览器资料及界面验证临时文件使用工作区 `.cache`。
- 完整命令输出：监督运行目录 `Matou-Mantle改名-11/logs/{typecheck-final,test-final,build-final,real-launch,card-ui-final}.log`。

关键用例：

- `uses the fixed installed application path when there is no override`
- `allows only the exact Mantle repository release path on GitHub`（同时拒绝旧仓库 URL）
- `shows Mantle with the installed version, both actions and a URL-free open before reference checks in %s`（`zh-CN` / `en`）
- `shows an uninstalled card with only the fixed installer action`
- `real kernel: verifies page timezones, final-window exit, process groups and ephemeral deletion`

## 真实界面与启动

真实内核用例通过（`2026-11-15T14:31:54.195Z`）：

```sh
env -u TMPDIR CI=1 MATOU_SAFE_BROWSER_REAL_TEST=1 \
  MATOU_SAFE_BROWSER_PATH='/Users/icesword/Documents/AIProjects/mantle/dist/Mantle.app/Contents/MacOS/Chromium' \
  pnpm --filter @matou/desktop exec vitest run src/main/safe-browser.real.test.ts
```

- 使用真实 Mantle 内核，本地页面自检常驻窗口为 `Asia/Tokyo`、一次性窗口为 `America/Los_Angeles`。
- 两次启动沿用相同种子，九项启动参数结构未变。
- 关闭最后一个页面后，两组进程均退出；常驻资料保留，一次性资料删除。
- 完整参数、进程和资料证据：[real-launch.json](evidence/mantle-rebrand/real-launch.json)。
- 已安装应用的真实面板验收通过（`2026-11-15T14:38:40.647Z`）。未设置 `MATOU_SAFE_BROWSER_PATH`，通过真实 `safeBrowserStatus` IPC 读取默认安装路径，返回 `installed: true`、`version: 148.0.7778.215`。
- 指定 `gpt-6.1-terra / high` 子代理在独立 Electron 测试壳中确认窗口 URL 属于 `.cache/m3/workspace`，用既有 E2E 本地 fake Claude 进入受保护终端卡片，再点击出口状态打开网络检查。没有请求真实授权、更新或第三方检查网站。
- 子代理目视核对后，主代理读回真实卡片文字、状态 IPC 结果及截图，确认标题、版本、提示、按钮完整且正确。
- 验收结束后 terra 关闭面板并退出本次测试应用，进程 `65948` 已退出。子代理交回的自行决定为「无」。
- 截图：[card.png](evidence/mantle-rebrand/card.png)；文字和状态：[card.json](evidence/mantle-rebrand/card.json)。

| 场景 | 本关要求 / 原有行为 | Matou 结果 | 证据 |
| --- | --- | --- | --- |
| 已安装卡片 | 名称 Mantle、真实版本；原布局和按钮不变 | Mantle；已安装 · 148.0.7778.215；检查更新 / 打开 | card.png、card.json、中文/英文组件用例 |
| 未安装卡片 | 名称 Mantle；未安装和固定下载入口 | Mantle；未安装；获取安装包 | 组件用例、外链白名单用例 |
| 构建内核真实启动 | 指定 mantle/dist/Mantle.app；时区、种子、清理不变 | 两组真实启动及页面时区、进程退出和资料清理通过 | real-launch.json、真实内核用例 |

## 本关决定与边界

1. 沿用指定工作区当前分支提交；原有未跟踪 `docs/brief-rename-mantle.md` 不纳入提交。
2. 固定品牌在中英文共用同一个 `Mantle` 字面量；不改卡片的状态、提示、布局或交互。
3. 旧验收中的命令、路径和截图文字保留历史真实性；本关新证据单独记录。
4. 桌面工具无法按进程号区分多个同名 Electron；仅在本工作区 `.cache` 复制带唯一 bundle ID 的 Electron 测试壳，加载同一产品构建。由 terra 操作本地假 Claude 的真实入口，避免真实登录和外部请求。
5. 无新依赖、存储或产品抽象。旧应用未删除，未修改 Mantle 项目或 `/Applications` 下的应用。
