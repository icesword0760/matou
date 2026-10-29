# 0.2.17 受管会话通行证：第 1 关

日期：2026-10-29。基线：main / `f69cce4bec33cf1f150fdd50b3443aa580f70b03`（0.2.16）。

## 用户可见结果与范围

新版本发给受管 Shell、Claude Code、Codex 运行的 `MATOU_CONTROL_TOKEN` 不再在 24 小时后失效。凭证仍受原权限范围约束，卡片移除、这次运行结束或 Runtime 重启后失效。没有续期接口、刷新命令、配置项或新增依赖。

只修改受管会话的凭证生命周期；`CapabilityTokenService` 和确认引用服务的实现、接口、短期到期规则均未修改。当前已运行的 0.2.16 及其已发出的凭证不会被这份源码热更新。

## Diff 摘要

| 文件 | 改动与原因 |
| --- | --- |
| `apps/runtime/src/runtime-server.ts` | 受管会话发证的到期值改为 `Number.POSITIVE_INFINITY`；绑定真实进程退出回调撤销该 run；发证后启动失败的 catch 回收该 run。 |
| `apps/runtime/src/session/pty-session.ts` | 只增加可选 `onProcessExit` 回调，并在真实 PTY 退出时、原有退出日志队列之前调用；只有受管会话传入该回调，原有日志、状态迁移和通知顺序保留。 |
| `apps/runtime/src/runtime-server.test.ts` | 新增启动失败回收和正常/ENOSPC 两种真实进程退出测试；既有三种 profile 的环境测试加入启动 7 天后的校验；移除测试使用无限期凭证。保留原有多处幂等撤销，移除测试不再限制 revoke 回调只能调用一次，仍检查实际失效。 |
| `apps/runtime/src/control/host-control-server.test.ts` | 新增 generation 重建拒绝旧凭证、短期与无限期凭证并存及权限隔离测试。 |
| `apps/desktop/package.json` | 版本改为 0.2.17；已检查上一版提交，版本同步范围只有此处。 |
| `docs/release/0.2.17.md` | 更新正文逐字采用用户指定内容；下载与安装四条沿用 0.2.16；验证部分如实列明通过与失败。 |

## 生命周期验证矩阵

| 场景 | 证据和实际结果 |
| --- | --- |
| 会话保持运行超过 24 小时 | 从实际 Shell、Claude Code、Codex 子进程读取注入的凭证，将校验时间推进 7 天，原有全部控制权限仍可用；修复前同一断言失败，修复后通过。 |
| 卡片/会话移除 | 移除实际会话后，无限期凭证失效，终端输入目标注销，HUD 和恢复任务清理。 |
| 正常进程退出 | 真实 Shell 正常退出，凭证失效，运行记录结束。 |
| 进程退出而日志队列被 ENOSPC 卡住 | 只对测试创建的 PTY 发出 SIGKILL，等待实际退出；在退出日志仍未持久化、原有退出通知和注册表状态尚未变化时，凭证已经失效。恢复写入后，原有退出状态继续正常完成。补回调前该断言失败。 |
| 已发证但创建 PTY 失败 | 用测试文件阻止该会话日志目录创建，真实创建路径报错；已发出的凭证失效，没有注册终端。补 catch 回收前该断言失败。 |
| Runtime 重启/generation 变化 | 新 generation 的凭证服务拒绝前一 generation 的无限期凭证；凭证记录只存在于内存。 |
| 其他短期凭证和权限 | 同一服务内短期凭证在原到期时间后失效，无限期凭证继续有效；未授予的权限仍被拒绝。未修改确认引用及其他发证调用方。 |
| CLI 和界面操作 | 11 项真实桌面 E2E 全部通过，覆盖识别、读取/发送、新建会话、跨窗口导航、分支操作和一次性确认引用过期；原有焦点与不移动 UI 的断言通过。 |

专项选跑共 7 项通过；它们是全套测试的子集，不重复计入下表总数。日志保留了“原发证规则失败 → 修复后通过”和“两处回收边界失败 → 修复后通过”。

## 自检结果

| 检查 | 命令 | 结果 |
| --- | --- | --- |
| 类型 | `pnpm typecheck` | 最终生产改动后通过。 |
| 命名门禁 | `pnpm check:identifiers` | 初轮及新增文档后的检查均通过。 |
| 默认全套 | `pnpm test` | 未通过：命名检查、门禁测试 4 项、Contracts 68 项、Domain 7 项、桌面 861 项及性能 3 项通过；Runtime 常规部分 1149 通过、1 跳过、1 失败，后续因 `&&` 未运行。 |
| Runtime 原命令重跑 | `pnpm --filter @matou/runtime test` | 同一既有日志用例再次 15 秒超时，1149 通过、1 跳过、1 失败；未改测试或上限。 |
| RuntimeServer 完整文件补跑 | `pnpm --filter @matou/runtime exec vitest run --testTimeout=15000 src/runtime-server.test.ts` | 136 通过、1 失败；失败项见“待查”。新增和调整的生命周期断言通过。 |
| 历史范围完整文件补跑 | `pnpm --filter @matou/runtime exec vitest run --testTimeout=120000 src/journal/journal-range-reader.test.ts` | 首次 8 通过、1 失败；单独完整重跑 7 通过、2 失败，均为性能/RSS 阈值。 |
| 日志完整文件单跑 | `pnpm --filter @matou/runtime exec vitest run --testTimeout=15000 src/journal/segment-journal.test.ts` | 20 项全部通过，测试耗时 6.15 秒，含编译总耗时 6.67 秒。 |
| 开发构建 | `pnpm build` | 通过；供隔离开发实例的 E2E 使用，并非安装包。 |
| Host control E2E | `MATOU_E2E_DISPLAY=primary pnpm exec playwright test tests/e2e/ai-host-control-cli.spec.ts tests/e2e/ai-host-structure-control.spec.ts tests/e2e/ai-host-navigation.spec.ts --workers=1 --reporter=list` | 11 项全部通过（2.2 分钟）。 |

## 待查

这三组既有失败已逐项提交确认，获准列入本次已知限制；本次没有修这些问题，也没有修改对应测试、时间上限或性能阈值。

1. **日志尾部边界/侧车重建超时**：`SegmentJournal > persists the 10,000-line tail boundary and rebuilds a corrupt sidecar`。两次全套均在 15 秒上限超时；该用例单独运行测试耗时 2.17 秒并通过，完整文件单跑 20/20 通过。完整文件运行前负载为 27.81/36.14/31.51，运行后为 31.67/36.74/31.78（均为 1/5/15 分钟）。后续排查并行运行时的资源竞争；目前只有现象证据，不将负载视为已经证明的唯一根因。
2. **Claude 失败重启**：`reports a failed Bypass respawn instead of presenting the Shell fallback as success`。在等待错误 RPC 的 2 秒断言处失败，完整文件和单跑均复现。将本次两个生产文件临时替换为 HEAD 原文后，同一用例仍在同一位置失败（测试耗时 2.293 秒）；基线验证后自动恢复本次源码。该用例不启用 control，本次未修改其失败重启逻辑或用例。它对应 0.2.16 的“Claude 重启失败时可能提前显示成功”修复，看起来像那次修复的回归，仍需后续查明。
3. **历史范围性能和内存阈值**：`recovers only the checkpoint suffix of a 32 MiB Journal within a 16 MiB RSS budget` 首次 p95 为 173.15ms，上限 100ms，单独重跑为 182.16ms；同组 `recovers only the checkpoint suffix of a 256 MiB Journal within a 16 MiB RSS budget` 重跑 RSS 增量为 17,088,512 字节，上限 16,777,216 字节。首次失败未在同一日志中采样负载，最近的相邻采样（13:39:22）为 27.81/36.14/31.51；重跑前后采样为 26.51/34.44/31.27 和 27.44/34.15/31.23。上述是相邻采样，不代表精确到断言时刻的机器负载。相关生产代码和测试完全未改。

## 我自己做的决定

- 用 `Number.POSITIVE_INFINITY` 表达受管会话不按时间过期，保持凭证服务接口和其他调用方不变；该值只在内存记录中使用，不涉及持久化格式变更。
- 保留原有退出/移除/关闭的撤销调用，让即时退出回调与现有清理幂等叠加；两处回收边界的扩充由用户明确选择 A 后实施。
- 使用独立数据目录的开发实例做 E2E；不安装或启动 `/Applications/码头.app`。

## 证据路径与关卡边界

- 仓库内原始日志：`output/control-token-0.2.17/logs/`；同名长命令日志同时写入任务目录 `logs/`。
- 完整审阅补丁：`output/control-token-0.2.17/review.patch`（含新文档，不含他人的未跟踪文件）。
- 关键日志：`regression-before.log`、`cleanup-before.log`、`regression-after.log`、`typecheck.log`、`full-test.log`、`runtime-final.log`、`runtime-server-final.log`、`journal-recheck.log`、`journal-file-final.log`、`bypass-recheck.log`、`bypass-baseline.log`、`journal-range-final.log`、`journal-range-recheck.log`、`build.log`、`host-control-e2e.log`。
- 用户已安装 app 主进程 PID 12500、Runtime PID 12505 在测试前后均保持运行；未替换、退出或重启它们。
- 未暂存或提交任何文件；他人的 `.impeccable/` 和 `docs/prd/mockups/mobile-2026-10-24/` 保留。
- 第 1 关结束时，第 2 关未执行：未打 arm64 安装包、未做安装包签名/冒烟、未生成发行 SHA256、未 commit/push/tag/Release，也未部署应用内更新服务。本关完成后停下等待审阅。

## 第 2 关：安装包与发布证据

第 1 关已获用户审阅通过，以下补充本次安装包的实际验收；前述旧测试失败及“待查”保持不变。

- 成功构建从 `apps/desktop` 目录运行：`MATOU_UPDATE_BASE_URL=https://updates.baize-ailabs.com/stable CSC_IDENTITY_AUTO_DISCOVERY=false pnpm exec electron-builder --mac dmg zip --arm64 --publish never --config.mac.identity=-`。更新地址取自现有 `package:mac` 脚本默认值，编译资源由现有构建流程准备，未修改源码或配置。
- 以 SHA256 与 0.2.16 Release 一致的正式 ZIP 为对照，0.2.16 和 0.2.17 的 Identifier 均为 `com.matou.desktop`、`Signature=adhoc`，均有 `runtime` 标记和有效 Sealed Resources；未公证。
- 构建目录、0.2.17 ZIP 解压后的应用、0.2.17 DMG 只读挂载后的应用均通过 `codesign --verify --deep --strict`。三者的 CDHash、签名资源清单和关键 Runtime 文件一致；DMG 镜像校验通过，检查后已卸载镜像。
- `app-update.yml` 与 0.2.16 正式包逐字相同：地址为 `https://updates.baize-ailabs.com/stable`、渠道为 `stable`；`stable-mac.yml` 的版本为 0.2.17，DMG/ZIP 大小及 SHA512 与实际文件一致。
- 原有 `packaged-runtime.spec.ts` 完整冒烟通过：1 项用例、4 次隔离启动，覆盖 SQLite、真实 PTY、`mt identify`、历史回放、截断日志恢复及新版本数据库只读兼容和恢复正常；测试耗时 8.7 秒，总耗时 9.2 秒。未修改测试、超时或阈值。
- 所有测试只运行仓库构建输出中的应用；没有安装、启动或重启 `/Applications/码头.app`。没有部署应用内更新服务。
- 初轮关闭证书查找时跳过签名、随后根目录运行找不到相对 entitlements，两次失败均按要求先停下提问并获准修正；失败产物清理后，从正确工作目录进行同一次签名构建，重新生成全部发行附件。首次冒烟额外指定的长 TMPDIR 导致控制 socket 路径过长，移除该额外设置后，按 0.2.16 原环境通过；所有失败日志保留，未将失败产物用于发布。

发行目录：`apps/desktop/release-0.2.17/`。此目录只有以下 6 个附件，名称与 0.2.16 Release 逐个替换版本号后完全一致。`SHA256SUMS.txt` 沿用上一版格式，包含其他 5 个附件；清单自身的摘要另列于下表及回读记录。

| 附件 | 字节数 | SHA256 |
| --- | ---: | --- |
| `Matou-0.2.17-mac-arm64.dmg` | 159969029 | `99e3a8dee9a238fb4c41cb0a5cf8e02383536eb69709f672f88fcc0581507052` |
| `Matou-0.2.17-mac-arm64.dmg.blockmap` | 166583 | `36919a7102b247e8e35bf89ba3200551df296298888d49563d9c11f65cc23ad1` |
| `Matou-0.2.17-mac-arm64.zip` | 159772061 | `82de31e705378215206493540b6173a77b8ada62cb1e6262b0dcd942fcc75026` |
| `Matou-0.2.17-mac-arm64.zip.blockmap` | 166577 | `4a530e6d46abc7d48771b5fe329a0f4d44c244a32f9e11cad362fb4eb7ecb703` |
| `stable-mac.yml` | 507 | `9eddc58889716006d52ba8af66414bf0116e3ed1e7fcdc9df19342c2e5bc420e` |
| `SHA256SUMS.txt` | 471 | `b96decb9dcf355e93ee6c33942cf2ab69c23544ea2338453ae36d7316166ae22` |

签名与配置证据：`output/control-token-0.2.17/package-verification.json`、`logs/package-verification.log`；成功构建日志：`logs/package-arm64-desktop-cwd.log`；最终冒烟日志：`logs/packaged-smoke-final.log`。上述日志均位于 `output/control-token-0.2.17/` 下，并同步保存在任务目录 `logs/`。

本次仅提交指定的 5 个改动文件和 2 份文档。GitHub 发布使用本文件对应版本的源码、`docs/release/0.2.17.md` 全文及上述 6 个附件；远端 main、tag、Release ID、Latest、附件下载后的大小/摘要及正文逐字校验记录交付在 `output/control-token-0.2.17/release-readback.json` 和 `release-readback.md`，不会把构建输出或他人的未跟踪文件提交到仓库。
