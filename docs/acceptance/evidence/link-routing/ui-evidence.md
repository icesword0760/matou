# 外链路由真实界面验收

2026-11-16，本机 macOS，构建后的本 worktree Electron 开发实例。真实点击使用原生桌面操作；网络检查面板出口数据使用固定 fixture，不代表当前网络检测结果。三次启动均设置 `MATOU_E2E=1`、`MATOU_E2E_DISPLAY=primary`，并将 `MATOU_DATA_DIR`、`MATOU_DEFAULT_WORKSPACE`、`ELECTRON_USER_DATA_DIR` 指向本工作区 `.cache/link-routing-ui` 下的隔离目录；未使用真实用户 Matou 数据。

## 已安装

未设置 `MATOU_SAFE_BROWSER_PATH`，使用本机 `/Applications/Mantle.app/Contents/MacOS/Chromium`。在测试窗口点击「网络检查」→「IP 质量」；实际启动 Mantle，原生窗口地址栏读回 `https://ip.net.coffee`，网页标题为「IP 查询 - IP分流测试 - Net.Coffee」。进程命令行显示使用隔离的 `electron-user-data/safe-browser/profile`。验证后关闭测试实例及其启动的 Mantle。

## 未安装

完全关闭前一测试 Electron 实例及其启动的 Mantle 后重新启动测试实例，设置 `MATOU_SAFE_BROWSER_PATH` 为工作区内不存在的 `.cache/link-routing-ui/does-not-exist`。点击「网络检查」→「IP 质量」，出现原生模态提示，正文为「从 Matou 打开的网页默认使用 Mantle 浏览器。」「安装 Mantle 后，登录和网页都会在受控环境里打开。」截图见 [missing-dialog.png](missing-dialog.png)。macOS 原生提示未在画面中显示传入的窗口标题；正文和三个按钮可见。

- 点击「安装 Mantle」：系统默认浏览器 Chrome 打开 `github.com/icesword0760/mantle-browser/releases/latest`，随后显示对应版本发布页。
- 点击「用系统浏览器打开」：Chrome 新标签实际显示 `ip.net.coffee/`，网页标题为「IP 查询 - IP分流测试 - Net.Coffee」。
- 点击「取消」：提示关闭并回到网络检查面板；另一次操作前后读取 Chrome 标签，均为 6 个，选中发布页不变，未观察到新标签。

## 启动失败

完全关闭缺失态测试 Electron 实例后重新启动测试实例，`MATOU_SAFE_BROWSER_PATH` 指向工作区内可执行脚本 `.cache/link-routing-ui/failing-browser`，脚本立即以状态 1 退出。点击「网络检查」→「IP 质量」出现真实原生失败提示「这次没能用 Mantle 打开网页。」及「用系统浏览器打开」「取消」；截图见 [start-failed-dialog.png](start-failed-dialog.png)。点击「取消」后提示关闭并回到面板；再次触发后点击「用系统浏览器打开」，Chrome 新标签实际显示 `ip.net.coffee/`。macOS 原生提示未在画面中显示传入的窗口标题；截图中原生按钮视觉排列为上方「取消」、下方「用系统浏览器打开」。

## 限制

这是本机开发构建的真实窗口验收，不是已安装 Matou 正式发行包验收；未验证其他平台、供应商密钥页和订阅登录真实授权流程，未测试失败提示的 Escape 键。测试启动的 Electron 与 Mantle 进程已关闭；未关闭用户原有浏览器或其标签。

## 我自己做的决定

使用项目已有固定出口 fixture 保持网络检查入口稳定；用工作区内立即退出的脚本模拟 Mantle 启动失败；保留系统浏览器打开的测试标签，避免误关用户原有标签。
