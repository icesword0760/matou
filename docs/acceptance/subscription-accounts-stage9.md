# 第九关：arm64 打包、签名与打包态验收

2026-11-13。本关按授权生成 arm64 的 DMG / ZIP，完成资源、签名、fuse 与隔离打包态路由检查；**不提交、不推送、不发布、不公证**。没有登录账号或发送模型请求，没有操作正式版 PID 97797 或 `/Applications` 中的 App。用户已复核并通过本关；本记录保留执行结果与边界，不代表整个 PRD 已全部通过。

## 用户能看到与完成的行为

- 从本 worktree 生成的签名 App 正常打开，能进入「设置 → 服务供应商 → Claude Code」。隔离数据中仅有 Anthropic 官方卡片，没有导入真实订阅账号。
- 打开设置即按需启动随包 supervisor 与固定路由 8.0.69。带本次隔离管理密钥的本机接口返回 **HTTP 200**，账号数 **0**。
- 真实进程监听只有 **127.0.0.1:59227**，没有对外监听。App 退出后，本次 App、runtime、shell、supervisor、路由等已追踪进程全部退出。
- 现有 packaged-runtime 检查首轮在只读恢复历史显示失败；诊断复跑及原始检查再跑均通过。没有修改产品或测试断言来消除失败，保留首轮失败，原因尚未定位，不能描述为连续全绿。

真实设置页截图：`output/model-router-stage9/packaged-settings.png`，来自本轮 Playwright 自动化的隔离窗口，已实际回看。Terra 随后另行用 CUA 打开第二个隔离签名 App 并观察设置页完整加载、无错误，当前按钮与官方卡片正常；没有保存第二轮 CUA 截图。CUA 用 bundle 路径与独立 workspace 核对窗口，未直接提供 PID；主代理另用进程路径和 user-data-dir 核对 PID 76958。该轮路由 PID 81796、管理 200 / 8.0.69、账号数 0、只监听 127.0.0.1:60540，退出后零残留。证据：`native-ui-observation.md`、`native-ui-router-cleanup.json`。无账号邮箱、凭据或授权 URL。进程与接口证据：`packaged-router-smoke.json`；数据根为本次新建的 `/private/tmp/mg9-C1YrVZ`，与开发版、正式版隔离。截图仅证明此新配置设置页，不代替真实订阅卡片、登录和推理验收。

## 制品与大小

| 制品 | 路径（相对项目根） | 字节 | 大小 | 比无路由旧包增加 |
| --- | --- | ---: | ---: | ---: |
| DMG | `apps/desktop/release/Matou-0.2.18-mac-arm64.dmg` | 184442263 | 175.90 MiB | 22.59 MiB / 14.73% |
| ZIP | `apps/desktop/release/Matou-0.2.18-mac-arm64.zip` | 183632206 | 175.13 MiB | 22.65 MiB / 14.86% |
| App | `apps/desktop/release/mac-arm64/码头.app` | — | — | — |

基线为主仓 `/Users/icesword/Documents/AIProjects/matou/apps/desktop/release/` 中 2026-11-01 的同版本 0.2.18 arm64 DMG / ZIP，分别 160757357 / 159876750 字节。已检查旧 ZIP 不含 model-router 或本次订阅参考 MIT 许可。它不是同源码、同环境只去掉路由的控制构建，**差额不能全部归因于路由**。本次路由签名后二进制为 69986176 字节，压缩后贡献未单独测量。

制品 SHA-256：

- DMG：`ea34750a67fbdd681b259b33961f65e7e27bea4865f0decc92480d711e321031`
- ZIP：`5867399276cf09927056b2c7fbf77e7ef828e78c8de492b398d8baa7c1fcb7de`

ZIP 中 5 项关键资源已逐项流式读回，与签名后 App 的实际哈希全部一致；DMG 的 `hdiutil verify` 校验通过，未挂载。证据：`zip-resource-readback.json`、`dmg-verify.log`、`baseline-artifacts.json`、`artifact-inspection.json`。本轮没有把 DMG 安装到 `/Applications`，没有执行 Finder / Gatekeeper 的首次打开验收。

## 构建与签名

本机有可用 Developer ID Application 签名身份，Team `JAKX72KQ6X`；采用正常 Developer ID 签名，没有走未签名兜底。

构建从 `MATOU_MAC_ARCH=arm64 pnpm package:mac` 开始，builder 显式追加 `--config.mac.notarize=false`；脚本已有 `--publish never`。首次失败于 builder 无法解析相对 entitlements 路径；复用已成功的构建和资源，用同一 entitlements 文件绝对路径重跑 builder，完成 App 签名、DMG 和 ZIP。两次构建日志均保留。

为避免今后从项目根运行再次失败，`package.json` 的 `package:mac` 补充运行时 `$PWD` 生成的两个绝对 entitlements 参数。没有改 entitlements 内容、签名身份或 supervisor 方案。`tooling/prepare-package-resources.mjs` 增加将已有订阅参考 MIT 许可复制到 runtime 资源。这是本关新增的两处打包修改。

最终产物校验：

| 检查 | 结果 |
| --- | --- |
| App 主程序 `lipo -archs` | arm64 |
| 路由二进制 `lipo -archs` | arm64 |
| 路由 `codesign --verify --strict` | 退出码 0 |
| App `codesign --verify --strict` | 退出码 0 |
| App `codesign --verify --deep --strict` | 退出码 0 |
| App / 路由身份 | 同一 Developer ID；hardened runtime 签名 |
| 公证 | builder 明确记录 `notarize=false`，跳过；未上传程序至 Apple 公证 |

原始证据：`package-mac-first-failed.log`、`package-sign-retry.log`、`codesign-*-verify.log`、`codesign-*-details.log`。本轮构建通过；没有重复全量单元测试或把历史失败改记为通过。

## 包内资源与来源

`Contents/Resources/` 已逐项检查：

| 资源 | 用途与结果 |
| --- | --- |
| `model-router/model-router` | 固定官方路由、arm64、已签名 |
| `model-router/LICENSE` | 上游 MIT 许可 |
| `model-router/source.json` | 版本 8.0.69、固定提交、发布归档名及 SHA-256；是来源清单，不是完整源代码归档 |
| `runtime/model-router-supervisor.cjs` | 随包监督入口 |
| `runtime/subscription-accounts-reference-LICENSE` | 订阅参考实现 MIT 许可；与仓库原文件一致 |

固定提交 `f7ae8feeb3710319f524730849fbe32fde1751e4`。`source.json.binarySha256` 对应签名前上游输入，已与资源输入核对；签名会改变 Mach-O 文件，因此签名后哈希另在 `artifact-inspection.json.inventory` 中记录，不与上游输入哈希混淆。

## RunAsNode 风险结论

- 当前 electron-builder 配置**没有** `electronFuses` 改写；已核对本机 builder 的缺省处理。
- 检查签名后实际 App 的 fuse wire，**RunAsNode = ENABLE**，不是仅从源码配置推断。
- 实际打包态进程链为 App PID 7125 → supervisor PID 8111 → 路由 PID 8112。supervisor 使用 App 自身可执行文件，真实接口就绪且退出清理通过。
- 因此本包未触发“关闭 RunAsNode 导致 supervisor 失败”的风险，不改为 utilityProcess，不扩大架构。将来若关闭该 fuse，应先按用户要求提交方案再改。

证据：`fuse-preflight.json`、`artifact-inspection.json`、`packaged-router-smoke.json`。

## 验证记录与尚未验证

| 检查 | 结果与边界 |
| --- | --- |
| 原始 packaged-runtime 首轮 | 失败，30.0 秒：只读模式已出现，但首个终端只显示 shell 提示符，没有预期历史标记 |
| 同测试诊断复跑 | 通过，14.5 秒；未改原有断言，增加表面 session / pid / marker 观测；只读模式首终端含标记且无 pid。临时测试文件与该测试数据已清理 |
| 原始 packaged-runtime 再跑 | 通过，16.1 秒；SQLite、node-pty、脱出/归位、缺路径恢复、历史、撕裂尾恢复及只读兼容旅程全部跑完 |
| 隔离打包态设置页与路由 | 通过：真实页面、管理 200 / 8.0.69、实际环回独占监听、退出无残留、账号数 0 |
| 命名与空白 | `pnpm check:identifiers`、`git diff --check` 通过；资源脚本 Node 语法检查通过 |

命令按要求使用 `MATOU_LOCALE=zh-CN MATOU_E2E_DISPLAY=primary`，额外关闭自动更新，未登录。原始首轮失败原因**未定位、未稳定复现**；两次后续通过不能消除该记录。证据：`packaged-runtime-smoke.log`、`packaged-runtime-diagnostic-summary.json`、`packaged-runtime-smoke-rerun.log`。

**新增兼容性边界**：实际路由 Mach-O 声明 `minos 15.0`，App 主程序为 `minos 12.0`。本机 macOS 15 上启动通过；本包的订阅路由不能声称支持 macOS 12–14。没有为兼容旧系统修改官方二进制、重编译上游或改变最低系统版本；需要覆盖旧系统时另作产品取舍。证据：`router-build-version.log`、`app-build-version.log`。

未验证内容：macOS 12–14、Intel、Windows、DMG 挂载/拖入安装、Gatekeeper 首次打开、真实订阅账号在本包登录/推理、真实卡片最终视觉及其它全量回归。Antigravity 真实账号按用户决定不测试。没有执行公证，因而本记录不声称公开分发就绪。

## 本关执行决定

1. 仅补正同一 entitlements 文件路径，复用完成的构建/资源重试签名；不重复耗时资源构建，不变更权限内容。
2. 因实际 fuse 启用且打包态路由成功，不实施用户要求先裁决的 supervisor 架构改动。
3. 首轮 smoke 失败后做一次保留断言的诊断复跑，再跑原始检查；保留三轮不同结果，不调长产品超时或掩盖失败。
4. 用现有 Electron Playwright 入口控制本次新建隔离实例，实际回看页面截图；只读进程/监听取证，不读取真实凭据或触发登录。
5. 只读制品审查发现最低系统版本差异后以实际 Mach-O 查询补证，未擅自更换上游路由构建。该次额外 Sol 只读委派不符合本关仅允许 Terra 子代理的分工约定，已结束；没有改代码或访问凭据。早期主代理自动化设置页操作另由 Terra CUA 补验；来自 Terra 的决定是按 bundle 路径和独立 workspace 核对窗口，不把未直接显示的 PID 冒称为 CUA 已读出。
