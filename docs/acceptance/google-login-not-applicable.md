# Google 登录出口取消探测验收

本次仅修改 Google 登录出口采样与判定；不改前端布局、文案、版本号或其他项目。工作分支 `fix/google-exit-not-applicable`，基线 `3f99a49`。

## 用户可见结果

Google 组只保留「出口不在中国大陆」「出口符合出口规则」两项；不生成登录与 API 同一出口、无法确认或不适用占位行。底部「Google 线路使用通用出口检测，不能保证 Google 实际看到的出口。」保留。

Google 没有公开出口接口，只能用通用出口近似。全量采样和 fixture 均将登录主机标记为 `notApplicable: true`，不附请求错误，不读取 fixture 登录数据，也不请求 `accounts.google.com/cdn-cgi/trace`。轻量采样仍只请求通用 API。判定忽略 Google 登录数据，真实通用 API 失败仍为 unknown，大陆和出口规则异常仍正常生效。

## 改动文件

- `packages/contracts/src/egress.ts`：可选的不检测标记。
- `apps/runtime/src/egress/egress-sampler.ts`：取消 Google 登录请求，fixture 同步跳过。
- `apps/runtime/src/egress/egress-evaluator.ts`：Google 只生成两项检查且只用通用 API 判定。
- `apps/runtime/src/egress/egress-sampler.test.ts`：注入真实 probe 的 requester，断言完整请求列表；保留轻量检测、fixture 回归。
- `apps/runtime/src/egress/egress-evaluator.test.ts`：两项检查、缺失登录、通用 API 失败、大陆和规则、其他三类登录检查回归。
- `apps/desktop/src/renderer/src/egress/EgressChip.test.tsx`：面板两行及说明断言，未修改前端产品代码。
- `docs/prd/PRD-Matou-网络出口检测.md`：同步不检测登录出口与通用近似边界。
- 本记录及 `evidence/google-login-not-applicable/` 内两张截图和 `results.json`。

## 定点测试名

运行 `pnpm --filter @matou/runtime exec vitest run src/egress/egress-sampler.test.ts src/egress/egress-evaluator.test.ts`：43 项通过。

- `full sample never requests the Google login trace and retains other login requests`
- `light sample requests only the generic api trace through the agent proxy`
- `reads Google api but ignores fixture login data: %s`
- `keeps Google and overall status ok with an unmeasured login and only two checks: %s`
- `still reports unknown when the Google generic API probe fails`
- `marks Google CN api abnormal with mainland violation`
- `enforces saved Google %s rule`
- `ignores Google login data even when its country is %s`
- `preserves matching, mismatching, unknown and mainland login checks for %s`（Anthropic / OpenAI / ChatGPT 订阅）

运行 `pnpm --filter @matou/desktop exec vitest run src/renderer/src/egress/EgressChip.test.tsx`：25 项通过。其中 `renders the shared HUD exit label for a registered Google session and opens its panel` 验证两行和底部说明。

## 真实 Electron 面板

由指定 Terra 子代理操作，主代理回读截图与结果。使用工作区内隔离 fixture 和应用数据，验收后关闭本次启动的 App。

| 场景 | 实际结果 | 证据 |
| --- | --- | --- |
| 从底部网络检查入口打开面板 | Google 恰好两项，无登录检查、无法确认或不适用行 | [面板截图](evidence/google-login-not-applicable/google-panel.png) |
| Google 底部说明 | 原文保留 | 同上及 [文字记录](evidence/google-login-not-applicable/results.json) |
| 重新检查 | 点击到观察完成 35ms；按钮禁用到恢复约 5.8ms | [刷新后截图](evidence/google-login-not-applicable/google-panel-after-recheck.png) 与文字记录 |
| Anthropic / OpenAI 登录检查 | 两组仍有「登录与 API 同一出口」 | 面板截图与文字记录 |
| ChatGPT 订阅登录检查 | 自动采样和判定测试覆盖；本次 fixture 未显示该组，未做真实界面验证 | 上述定点测试 |

35ms 是 fixture 模式耗时，不代表真实网络总耗时。取消 Google 登录请求已由注入 requester 的测试验证；其他探测仍可能受各自网络状况影响。本次没有测量真实互联网刷新耗时。

## 我自己做的决定

- 用 `notApplicable: true` 表示主动不检测，不带 `error`，明确区别于请求失败。
- Google 判定完全忽略登录数据；沿用面板按检查项渲染的现有机制，不增加前端过滤、占位或布局修改。
- 依赖缓存与测试临时文件留在本工作区。初跑完整测试时，较长的 `.cache/tmp` 路径导致两个 macOS socket 创建失败，另两个无扩展临时脚本继承了项目 ESM 设置；缩短路径并隔离 CommonJS 后四项均定点复测通过；随后源码边界扫描撞上临时目录清理，最终统一采用该扫描已排除的短目录 `out/` 并写入临时 `{"type":"commonjs"}`，不修改无关源码。
- 来自 Terra 子代理：使用隔离 fixture 验证真实 Electron，明确耗时仅适用于 fixture，未将其作为真实网络性能结论。

## 完整自动验证

- `pnpm typecheck`：通过。
- `TMPDIR="$PWD/out" pnpm test`：退出码 0。Domain 12 项、Contracts 71 项、Desktop 1133 项、Desktop 性能 3 项、Runtime 主集 1699 项、RuntimeServer 171 项、日志范围读取 9 项全部通过；标识政策 4 项与打包资源检查 2 项通过。
- `pnpm build`：通过。
- `pnpm check:identifiers`：提交前再次运行，通过。
- `git diff --check`：通过。

仓库按既有条件跳过 Desktop 1 项、Runtime 36 项（真实安全浏览器、真实 Claude/订阅/固定路由引擎等），本任务未新增跳过。未验证真实互联网总耗时，也未验证 ChatGPT 订阅组的真实界面；其登录采样与判定已通过自动测试。

完整命令日志保存在本次监督运行目录 `Matou-Google检测-14/logs/`：`typecheck.log`、`build.log`、`test-accepted.log`、`identifiers.log`。首次失败和诊断日志亦保留。
