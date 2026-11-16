# 任务：本次功能实现的终审（反重力）

## 目标

对 `feat/login-browser-integration` 分支从 `2658035` 到 HEAD 的全部改动做独立终审，判断本次功能实现能否通过。只审功能，不审许可证和边界情况。

## 输入材料

工作目录：`/Users/icesword/Documents/AIProjects/matou-login-integration`。

先读：

- 需求与出口检测 PRD：`docs/prd/PRD-Matou-网络出口检测.md`
- 出口检测验收：`docs/acceptance/network-egress-2026-11.md`
- 本轮任务书：`docs/brief-safe-browser-launcher.md`、`docs/brief-login-reroute.md`、`docs/brief-safe-browser-panel.md`、`docs/brief-antigravity-egress.md`、`docs/brief-subscription-quiet.md`、`docs/brief-prompt-quiet.md`、`docs/brief-antigravity-sanitizer.md`、`docs/brief-sub2api-gap.md`、`docs/brief-audit.md`
- 三轮 Codex 审计与修复记录：`docs/acceptance/audit-round-1-2026-11-15/`、`docs/acceptance/audit-round-2-code-protection-2026-11-15.md`、`docs/acceptance/antigravity-sanitizer-2026-11-15/`、`docs/acceptance/antigravity-sub2api-gap-2026-11-15.md`
- 路由器补丁：`tooling/model-router/patches/antigravity-client-identity.patch`
- Matou 集成：`apps/desktop/src/main/safe-browser.ts`、`safe-browser-ipc.ts`、`runtime-host.ts`、`apps/desktop/src/renderer/src/egress/`、`apps/runtime/src/egress/`、`tooling/claude-accounts/isolated_browser.py`

对标项目（只读）：

- `/tmp/router-research/repos/Antigravity-Manager`
- `/tmp/router-research/repos/antigravity-claude-proxy`
- `/tmp/router-research/repos/sub2api`
- `/tmp/router-research/repos/CLIProxyAPI`

## 终审要回答的问题

1. **目标是否达成**：计费行、身份句、竞品词、代码保护、扫描窗口；Codex 保持窄口径；安全浏览器启动、登录改道、面板卡片、客户端提醒、Antigravity 出口保护、订阅会话降噪。
2. **三轮审计的修复是否真的闭合**：D1、D3、D4、D6a、R2-1 是否解决，是否有回归。
3. **是否还有阻塞性缺口**：会导致请求失败、工具失败、用户内容被误改、登录流程不可用的问题。
4. **已知限制是否可接受**：D2（Claude 最终 `tool_choice` 固定 VALIDATED）、D5（只处理三句身份）、D6b（8 句/1500 字符窗口）、D7（文本与工具调用排序）。
5. **是否可以进入发版前的人工验证阶段**（真实登录、真实订阅推理）。

## 要求

- 独立判断，不照抄 Codex 审计结论；要自己读代码和证据。
- 每条结论带文件:行号或运行证据。
- 只读，不改代码、不提交。
- 许可证、边界情况、发版清单不在范围内。
- 输出结论只能是：**通过** / **有条件通过** / **不通过**。
  - 通过：可以进入人工验证和发版准备。
  - 有条件通过：列出必须先处理的条件。
  - 不通过：列出阻塞项和原因。

## 交付

回复中给出：

- 结论；
- 支撑结论的关键证据；
- 阻塞项或前置条件（如果有）；
- 你认为仍未被验证、需要人工确认的部分。

## 范围

- 工作目录：`/Users/icesword/Documents/AIProjects/matou-login-integration`
- 不能碰主目录、其他 worktree、`safe-browser` 项目、参考仓库。
- 禁止修改代码、配置或提交。
