# 网页链接统一走 Mantle

## 用户行为

网络检查的「IP 质量」「浏览器环境」、供应商预设密钥页和订阅授权页都交给同一个主进程入口。Mantle 成功打开时不弹框；未安装或启动失败时按任务指定文案、按钮顺序、默认按钮和取消按钮显示原生对话框，只有用户选择后才打开系统浏览器。

Mantle 安装页继续直接使用系统浏览器。已有控制器、时区、资料目录及外链白名单保持原样。

## 已裁决的白名单边界

原通用白名单不包含预设密钥页或授权页。收到任务监督答复确认后，采用以下规则：统一入口先检查原 `isAllowedExternalUrl`，未命中时仅接受现有 `PROVIDER_PRESETS.keyUrl` 精确匹配，或 `source: 'sign-in'` 且通过现有订阅授权白名单。运行时仍在调用前校验提供商和授权 URL。普通外链 IPC 仍先使用原通用白名单，不获得密钥页或授权页的新权限。

两个外链 IPC 沿用 Mantle 面板现有的 sender 校验模式：窗口必须属于 Matou 主窗口集合、未销毁，并来自主框架。

## 修改范围

- `apps/desktop/src/main/web-links.ts`：统一入口、原生弹框、外链 IPC 注册；复用 index 中原有控制器。
- `apps/desktop/src/main/index.ts`：注册统一入口，并注入 RuntimeHost。
- `apps/desktop/src/main/runtime-host.ts`：移除独立尝试 Mantle、静默回退系统浏览器的双路径。
- `apps/desktop/src/main/web-links.test.ts`：统一入口与 IPC 定点用例。
- `apps/desktop/src/main/runtime-host.test.ts`：订阅授权转发、白名单顺序及失败结果用例。

## 自动验证

- `pnpm typecheck`：通过。
- `pnpm build`：通过。
- `pnpm test`：通过。Vitest 合计 3105 项通过、37 项按环境条件跳过；另外命名策略与包资源检查共 6 项通过。
- `pnpm check:identifiers`：通过（包含最终验收证据文件）。

重点测试名：

- `opens allowed links in the shared persistent Mantle with the current timezone without a dialog`
- `shows the missing-Mantle modal and handles button %s`（三个按钮）
- `shows the launch-failure modal and handles button %s`（两个按钮）
- `waits for user choice before opening anything in the system browser`
- `rejects untrusted ordinary URL %s without browser or dialog`
- `accepts only preset key pages and revalidates controlled sign-in URLs`
- `keeps ordinary IPC restricted to the original external allowlist`
- `rejects unknown, foreign, destroyed and subframe senders for %s`（两个 IPC）
- `routes validated %s subscription sign-in through openWebLink`（两种提供商）
- `reports unified opener failure without retrying or logging private authorization URLs`
- `opens subscription authorization only from the current runtime and only on the fixed service allowlist`

跳过项包含需显式启用的真实 Mantle 内核测试及需外部运行条件的运行时集成测试；其中模型路由协议集成因未设置 `MATOU_MODEL_ROUTER_BINARY` 未执行。新增外链与授权转发测试均已执行通过。

## 真实界面

由 terra 子代理操作本 worktree 的真实 Electron 窗口，已验证已安装态打开 IP 质量、未安装三个按钮和启动失败两个按钮；主代理已读回两张原生对话框截图。详见 [界面验收证据](evidence/link-routing/ui-evidence.md)。

macOS 原生框不单独展示传入标题，失败框按系统规则呈现按钮排列；代码中的标题、按钮数组及 defaultId/cancelId 均保持交代指定值。其他平台、正式安装包、真实供应商授权以及失败框 Escape 键尚未验证。
