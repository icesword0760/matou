# 任务：Matou 内 Antigravity agent 的出口保护

## 目标

让 Matou 里由 `antigravity` profile 启动的 agent 会话也纳入出口检测：显示出口状态，出口在中国大陆或违反已保存规则时暂停会话，恢复后自动继续；同时沿用现有的时区处理。

## 实际场景

用户已经在 Matou 里用 Antigravity 作为 agent。现在出口检测只覆盖 Claude Code 和 Codex，Antigravity 卡片启动时没有登记检测，异常出口时不会暂停，面板也不显示状态。

## 开发者意图

- 和 Claude Code / Codex 的体验一致：HUD 有出口标签，异常时暂停，恢复后自动继续。
- 诚实标注限制：Google 没有 Cloudflare 那样的 trace 接口，只能用通用出口做近似。

## 项目意图

先读：

- `packages/contracts/src/egress.ts`
- `apps/runtime/src/egress/egress-evaluator.ts`、`egress-sampler.ts`、`egress-monitor.ts`
- `apps/desktop/src/renderer/src/egress/egress-copy.ts`、`EgressPanel.tsx`
- `apps/runtime/src/runtime-server.ts` 里 `egressPlanForLaunch` 的使用位置

## 已经定下的决定（照做，不要改）

### 新类别

- `EgressFamily` 增加 `'google'`，加进 `EGRESS_FAMILIES` 和 `isEgressFamily`。
- `EGRESS_HOSTS.google`：
  - `api`: `www.cloudflare.com`（通用 trace）
  - `login`: `accounts.google.com`
- 其他类别不变。

### 判定

- 复用现有 evaluator：`api` 采样国家为 `CN` → abnormal / mainland；违反已保存规则 → abnormal / rule；`login` 与 `api` 不一致只 warn；`login` unknown 不拉低整体状态。
- 不改判定权重和阈值。

### 启动登记

- `egressFamilyForLaunch`：`profile === 'antigravity'` 返回 `'google'`。
- `egressPlanForLaunch`：`profile === 'antigravity'` 返回 `{ family: 'google', exitFamily: 'google', hideTimeZone: true }`。
- 现有 claude-code / codex / 订阅分支保持不变。

### 采样

- 全量采样包含 google 的 api 和 login。
- 轻量采样对活跃的 google 会话只请求 `www.cloudflare.com/cdn-cgi/trace`。
- 不新增额外的网络请求频率。

### 界面文案（Mockup 已由用户确认，2026-11-15）

设计稿：`docs/mockups/login-browser-integration.html`。Google 组的位置、标题和“无法确认”的说明都按稿子实现。

- 类别标题：中文 `Google（通用出口检测）`，英文 `Google (generic exit)`
- 检查详情里保留现有通用文案，不新增术语。
- HUD 标签、冻结横幅、恢复横幅、面板逻辑全部复用现有组件。
- 在面板底部采样说明里补一句：`Google 线路使用通用出口检测，不能保证 Google 实际看到的出口。`

### PRD 与验收

- 在 `docs/prd/PRD-Matou-网络出口检测.md` 增加一节：Google 类别的依据、近似方式、不能保证的部分。
- 在 `docs/acceptance/network-egress-2026-11.md` 增加 Google 类别场景。

## 原因与方案

原因：现有 `egressFamilyForLaunch` 只处理 claude-code 和 codex，antigravity 返回 undefined，因此不登记、不采样、不冻结。

方案：新增 google 类别，api 采样用 Cloudflare trace 作为通用出口；其余复用现有 evaluator、monitor、冻结与恢复。Google 域名本身没有 trace，所以不做"Google 眼中的出口"这种精确承诺。

## 范围

- 工作目录：`/Users/icesword/Documents/AIProjects/matou-login-integration`（worktree，分支 `feat/login-browser-integration`，与登录改道同一分支顺序进行）
- 不能碰主目录、其他 worktree、安全登录浏览器项目。
- 禁止 stash / reset / rebase。
- 不改版本号、不发布、不推送。

## 必须先问的地方

- 发现 `EGRESS_HOSTS` 的 `api`/`login` 结构放不下 google 的通用探测方式，先提问。
- 冻结 Antigravity 会话需要改动 PTY 或进程组逻辑（不应该），先提问。

## 关卡与交付物

一关完成：

1. `pnpm typecheck`、`pnpm test`、`pnpm build` 通过。
2. 新增测试：google 类别注册、`egressPlanForLaunch('antigravity')`、CN 判定、login unknown 不拉低、轻量采样只请求 api、HUD/面板标题文案。
3. 提交 commit，信息 `feat(egress): protect Antigravity sessions`。

回复中给出：改动文件清单、新增测试名、`egressPlanForLaunch('antigravity')` 的实际返回、面板/HUD 的截图或文字证据、`我自己做的决定` 逐条列出。

## 验收（我会亲自复核）

- 读代码确认没有改动 claude-code / codex 的行为。
- 亲自跑三条命令。
- 亲自启动一个 antigravity 会话（可用现有开发版和假数据），确认 HUD 出现 Google 标签。
- 确认面板标题和限制说明文案正确。
