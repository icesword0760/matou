# 最终构建与隔离完整应用验收

2026-11-17。原定只做验收；03:34收到独立复审两项具体缺陷后，监督者明确授权先修复对应源码/测试，再构建与验收；没有扩大为其它功能。不安装、发布或提交，不操作生产码头、真实账号或钥匙串。

## 本轮新执行的验证

| 验证 | 结果 | 当前日志 |
| --- | --- | --- |
| 相关runtime测试整轮，含runtime-server全175项 | **332通过、1失败、17跳过**，不能写整轮全绿 | `.cache/final-app-runtime-tests.log` |
| 唯一失败场景定点复验 | 1通过，174筛选排除；保留整轮失败 | `.cache/final-app-bypass-failure-recheck.log` |
| 桌面三设置组件、egress、RuntimeClient | 10文件134通过 | `.cache/final-app-desktop-tests.log` |
| contracts全套 | 9文件105通过 | `.cache/final-app-contracts-tests.log` |
| 修复后OpenCLI组件/runtime E2E | 1通过，17.6秒 | `.cache/final-app-e2e.log` |
| `pnpm build` | packages、runtime、desktop/main/preload/renderer全部构建成功，退出0 | `.cache/final-app-build.log` |
| runtime/contracts/desktop typecheck | 三处通过 | `.cache/final-app-types-*.log` |
| `pnpm check:identifiers` / `git diff --check` | 通过 | `.cache/final-app-identifiers.log` / `final-app-diff-check.log` |

runtime唯一失败为既有 `reports a failed Bypass respawn instead of presenting the Shell fallback as success`：等待错误RPC的2秒条件超时，随后有database-closed清理警告。它未配置角色分配，未进入本批新角色快照路径。单独复验211ms通过，尚不能由此确认具体根因或证明稳定；没有放宽超时、改源码或无限重跑。另一个初次定点命令筛选到了一个旧恢复场景，1通过、174筛选排除，日志`final-app-recovery-probe.log`，不重复累计。17跳过为既有固定router集成缺外部开关，不是通过。

本轮unit/集成整轮合计571通过、1失败、17跳过；组件E2E另1通过。没有把旧批日志拼作本轮全通过。

## 修复后组件E2E证据

[UI记录](final-ui-report.md)。`evidence/default.png`、`editing.png`、`saved.png`、`error-model.png`和`narrow-560.png`已由Terra实际查看。`evidence/requests.json`含7条loopback假上游请求：三档messages/count各命中对应目标，压缩命中Sonnet；非法role返回502，没有第8条上游请求。

这里是正式父设置组件+真实RPC/runtime/PTY+假CLI、假上游的浏览器测试，**不是完整Electron、真实CLI或真实订阅验收**。保存重开、恢复默认另存、失效模型、焦点与滚动、560px错误截图已覆盖；键盘激活和完整窄屏交互仍不作通过结论。

## 完整App隔离启动准备

已读AGENTS.md、package scripts、README开发启动说明与OpenCLI技能。指定`/private/tmp/claude-501/bundled-skills/2.1.325/7c661924e9f86c15e869725a76140e43/run/SKILL.md`不存在；采用用户指定的真实App基准。

产品已有隔离入口：`MATOU_DATA_DIR`、`ELECTRON_USER_DATA_DIR`、`MATOU_DEFAULT_WORKSPACE`和开发单实例策略。完整构建位于`apps/desktop/out/`及`apps/runtime/dist/`。启动器使用依赖中固定Electron43.4.1；读取该依赖入口时，它自带的install.js补齐了原缺失binary。没有增加依赖、运行未知脚本、创建品牌签名包或修改签名；它是Electron通用开发bundle，不是可安装的码头发行包。

本批不会运行`pnpm dev`中的router准备下载，亦不运行会创建并重签本地品牌bundle的开发准备器。以已有构建和Electron通用bundle启动完整产品入口。

启动器：`.cache/final-app/launch-isolated.mjs`。所有业务数据物理位置均为本worktree `.cache/final-app/`，包括data、userData、workspace、claude、codex、shell。假CLI只提供离线模型目录与本地回显，不自动发送模型请求；三个假API目标只监听127.0.0.1；出口探测使用既有E2E fixture。启动器使用macOS sandbox限制公网出站、`security`执行及真实账号目录访问。未替换正式renderer或runtime实现；未复制生产数据库与凭据。

**原启动路径阻塞（现已获准解决）**：本worktree允许范围内即使采用最短`.cache/a`，`.../.cache/a/control/runtime.sock`也有113字节，超过macOS Unix socket限制。产品没有现成socket覆盖参数。监督者#2已批准精确范围；已建立0700目录`/private/tmp/matou-routing-final-8e97o7aj`，其中只有d链接指向本worktree `.cache/final-app/data`，realpath及短socket长度已核对。launch-config位于`.cache/final-app/launch-config.json`。运行中保留链接；退出自己的App且确认进程结束后，仅unlink自己的d并rmdir空目录，绝不沿链接递归删除数据。

常驻命令（短alias已登记，按用户要求由监督者Bash后台启动）：

```sh
cd /Users/icesword/Documents/AIProjects/matou/.claude/worktrees/model-provider-routing
node .cache/final-app/launch-isolated.mjs > .cache/final-app/electron.log 2>&1
```

启动后`.cache/final-app/launch-state.json`记录launcher/App PID、独立数据根、调试端口和本地假上游地址；本批不使用nohup。

## 完整App操作边界

OpenCLI1.8.8现有Browser Bridge/help只提供Chrome标签绑定，无通用Electron renderer或端口接入入口，也没有Matou专用适配器。没有尝试绑定用户Chrome标签，没有用Playwright click、CDP操作或电脑工具绕过。即使完整App启动，正式设置入口、保存重开、恢复默认、错误、键盘和窄屏也须分别记为未验，不能以Chrome组件截图替代。启动日志/调试目标可证明进程和renderer存在，不能证明可见交互成功。

## 我自己做的决定

1. 本轮重新完整执行相关测试和build，不拼旧绿灯；保留整轮失败并只做一次正确场景的定点复验，不修改产品代码。
2. 将完整App与组件、fake上游、fakeCLI、真实订阅分开记录。Terra负责OpenCLI E2E及截图核对，没有其它代码子代理。
3. 复用现有隔离环境与fixture，独立启动器只负责环境、假服务与进程生命周期，不改正式产品逻辑；避免开发准备器的签名与router下载步骤。
4. 路径超限需要超出允许写入目录的短symlink，先申请精确范围，未获得答复不执行。
5. OpenCLI无法接入Electron时明确操作未验，不换工具或修改产品迎合自动化。

## 尚未闭合

完整隔离App的启动结果见后续状态；OpenCLI无法操作其renderer；旧Bypass用例整轮曾超时，具体根因未确定。真实CLI、真实订阅转换、真实账号换号、真实长对话压缩、忙终端性能与reference product可运行对照未验。

## 二次复审修复及最终代码验证（03:34之后）

独立报告`review-round2.md`确认两项缺陷；监督者明确授权优先修复，暂不启动旧build。

- **同名credential**：区分`identityChanged`与`credentialNameChanged`。任何已知身份变化均撤销旧guard；只有异名才禁用/删除旧文件。同名成功保存保留新文件，后续prepare使用新身份；同名失败后，prepare将store中已保存的authIndex/email与实际文件精确比对，拒绝错把新身份当旧账号。原异名回滚仍支持新prepare回到A，旧guard不复活。
- **混合分配的默认档位**：UI与后端patch在有任意显式assignment时校验三档有效目标；未分配档位按同一个默认模型规则解析，已知账号失效、缺key、明确目录缺模型均在保存前拒绝。默认错误行显示供应商/模型，允许改选；全恢复默认保留旧路径可保存。未知API目录继续允许手填/原生目标，不假装模型删除。`ModelContextChoice.resolvedModel?`由native catalog投影，供API默认使用确切模型ID，不从供应商总model或名称猜测。编辑过的built-in API只有仍为官方native端点时才允许OAuth免key例外，与后端一致。
- 保存校验没有读取真实keychain、没有启动router或新增额度/公网查询；同样保留默认官方未登录、三档全部另指合法目标的使用路径。

| 最终修复后结果 | 证据 |
| --- | --- |
| 原缺陷红灯：runtime4项、UI2项失败 | `round2-runtime-red.log` / `round2-ui-red.log` |
| 核心runtime9文件118通过，含同名成功/失败和异名原回归 | `round2-runtime-final.log` |
| 实际RuntimeServer+PTY关联4通过，171筛选排除 | `round2-runtime-server-final.log` |
| 桌面相关10文件139通过 | `round2-desktop-final.log` |
| contracts全9文件105通过 | `round2-contracts-final.log` |
| 最终packages/runtime/desktop完整构建退出0 | `round2-build.log` |
| 最终三处typecheck通过 | `round2-types-*-final.log` |
| 最终OpenCLI组件E2E 1通过、截图及7条fake请求 | `round2-e2e.log`、`final-ui-report.md`与evidence |

首轮完整回归的Bypass超时及定点通过记录仍保留；修复后未再重复全175项或把首轮改称全绿。最终回归只针对变化和调用者，不重复累计多轮通过数。旧测试中的不完整native目录和本应合法却缺fixture key的输入补齐后通过，未放宽产品校验。中间typecheck因contracts声明尚未重建以及一次不必要的必填类型断言失败；重建contracts并改为可选属性投影后通过，不隐藏这些失败日志。

新增决定：主代理自行处理两项逻辑修复，没有派实现子代理或改布局；Terra只复跑OpenCLI及截图。审批短alias后按精确归属创建，真实数据仍在worktree。首次Electron由监督者后台启动，launcher93733/Electron93809退出1；`electron-first-failed.log`显示嵌套sandbox初始化`Operation not permitted`导致GPU失败。监督者#4仅授权此进程增加`--no-sandbox`，外层sandbox-exec公网/keychain拒绝策略保留，产品BrowserWindow及系统策略不改。启动器按授权修正后，由监督者在03:42后台重启成功，最终状态见下表，不继续钻环境问题。

## 最终完整App启动实测与交接

03:43实际读取本实例日志、进程、监听端口、独立socket与数据库，并只读取loopback调试服务的`/json/version`、`/json/list`元数据；没有连接WebSocket、操作renderer DOM或发输入事件。证据：`evidence/electron-launch.json`、`.cache/final-app/electron-second.log`、`launch-state.json`。

| 对象/场景 | 实际状态 | 覆盖结论 |
| --- | --- | --- |
| 完整Electron主进程 | PID9436，launcher9248；从本worktree `apps/desktop`入口启动 | 完整App进程已运行，非Chrome夹具 |
| 正式renderer | 标题“码头”；URL为本worktree `apps/desktop/out/renderer/index.html?...defaultRootDirectory=.../.cache/final-app/workspace` | 构建后的正式renderer目标已存在；未以此冒充视觉/点击验收 |
| runtime | 物理隔离data下`control/runtime.sock`为0600，`matou.sqlite`已创建 | 独立runtime与存储已启动 |
| 调试服务 | `http://127.0.0.1:51806`，lsof确认只监听127.0.0.1 | 仅调试元数据服务，不是可在Chrome中操作完整App的产品URL |
| 本地假上游 | 127.0.0.1:51803、51804、51805，归属于launcher | 仅供此隔离App手动试用；并非真实订阅/外部API |
| 配置归属 | dataAlias realpath仍精确指向本worktree `.cache/final-app/data`；userData独立 | 未改生产数据/已安装App |
| 正式设置入口、保存重开/恢复默认/错误 | OpenCLI不能接Electron，未操作 | **未验**；组件E2E证据另列 |
| 窗口截图、真实键盘、持续窄屏 | 未用其它工具绕过OpenCLI | **未验**；无完整App截图，560px图仅组件夹具 |
| 真实CLI/订阅 | 假CLI只回显；开发账户Python引擎在此worktree缺失，不启动；没有真实登录 | **未验**，不称完整产品全部通过 |

最终代码关联自动验证为 **366项unit/集成通过**（runtime核心118+实际runtime关联4、desktop139、contracts105），组件E2E另1通过。171项仅为定点筛选排除。修改前整轮的1失败/17跳过与定点重验记录继续保留，不能将其改写成整轮全通过。

当前开发版由监督者常驻，**不退出、不删除正在使用的短symlink**。可运行入口、launch-config和命令已交付；若需要停止，请先退出本隔离实例并确认相关PID结束，再按launch-config中cleanup说明只删除自己的link/空目录。没有发布、安装或提交。

本批到此关卡停止源码变更。正式App设置交互、真实CLI/订阅、真实账号换号、真实长对话压缩与reference product对照仍未验证；没有把自动检查、调试目标或组件截图替代这些验收。

## 协调方最终独立复验

- 已阅读实际配置组件、目录校验、角色解析、请求转发、账号绑定撤销、出口集合恢复等实现，并查看正式组件截图。
- 最终独立审查报告 `~/.claude/agent-foreman/runs/matou-routing-review-01/review-round3.md`：上一轮同名凭据与默认档位校验问题均已修，限定范围未发现实质遗漏；不替代真实账号验收。
- 03:45 独立执行10个runtime文件：141项通过；三设置组件：46项通过；命名门禁通过。
- 03:46 独立执行 `pnpm exec playwright test tests/e2e/model-provider-routing.spec.ts`：1项通过，17.8秒。实际浏览器动作由OpenCLI驱动，仍是正式组件+真runtime+本地假上游。
- 独立执行 `pnpm build`：packages/runtime/desktop完整构建退出0。
- 原整轮Bypass超时、固定router跳过和上表实物未验项仍然保留。当前状态为功能代码完成、局部自动验收通过，完整产品验收矩阵未闭合；未提交、合并、安装或发布。
