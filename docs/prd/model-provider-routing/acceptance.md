# 模型分配：本地交付与审查修复

日期：2026-11-17。正式组件、本地浏览器/runtime E2E及第五批三项后端修复已交付，停在审查关卡；不代表整个 PRD 或完整 Electron 产品验收完成。没有修改原型、主仓库、生产配置、真实账户或已安装应用；没有提交、发布、安装、新依赖、付费或真实外部 API 请求。

## 正式用户行为

模型分配位于现有供应商列表及影响提示之后、对话压缩之前。Opus/Sonnet/Haiku 各自选择供应商和模型，缺项沿用默认供应商的该原生档位，显示实际名称，不把供应商总启动模型当作三个档位模型。

切换供应商清空模型；明确失效账号、已删除供应商/模型、缺模型、缺 API key 都保留原选择并行内反馈。API 没有显式目录时可下拉切换手填 ID。目录读取失败保留缓存和草稿，显示重试，不把失败当成全目录删除。恢复默认只清草稿，仍需保存。

保存只调用 `agent-model-settings.patch-assignments`；成功提示“模型分配已保存，新会话将使用此配置”。说明为“新会话使用此分配，正在运行的会话不受影响”。成功后保存按钮用 aria-disabled 保留焦点并阻止重复写入。父设置页复用既有 provider/accounts/agent snapshot，不新增账号或额度查询；重试同时刷新provider和目录，不重复启动订阅设置读取；新读取序号阻止过期 GET 覆盖刚保存的分配。

## 本批文件

| 文件 | 变化 |
| --- | --- |
| apps/desktop/src/renderer/src/hierarchy/ClaudeRoleAssignments.tsx | 正式三档编辑、默认目标、校验、手填、保存/重试、恢复默认、中英文本与焦点行为 |
| apps/desktop/src/renderer/src/hierarchy/claude-role-assignment-options.ts | 纯目录/默认档位/失效判断；显式目录优先于旧mapping补入候选 |
| apps/desktop/src/renderer/src/hierarchy/claude-role-assignments.css | 直接复用已确认Mockup区域规则，34px控件、三列、640px以下纵排，无新视觉方案 |
| apps/desktop/src/renderer/src/hierarchy/ClaudeRoleAssignments.test.tsx | 12项组件场景 |
| apps/desktop/src/renderer/src/hierarchy/ModelSwitchSettings.tsx | 插入正式区块、复用父数据、读取失败/重试及过期读取保护 |
| apps/desktop/src/renderer/src/hierarchy/ModelSwitchSettings.test.tsx | 现有卡片查询限定卡片区域；fake client提供合法agent snapshot |
| tests/e2e/model-provider-routing.spec.ts | OpenCLI驱动正式设置组件→真实RuntimeServer/SQLite/store/PTY→三个loopback上游；保存重开、计数、压缩、零fallback、并发patch与PID |
| docs/prd/model-provider-routing/evidence/ | 实际截图、OpenCLI动作与请求记录 |

Task4界面批次保留Task1–3 runtime/contracts；其后后端审查修复见文末。

## 可复跑结果

```sh
pnpm --filter @matou/desktop exec vitest run \
 src/renderer/src/hierarchy/ClaudeRoleAssignments.test.tsx \
 src/renderer/src/hierarchy/ModelSwitchSettings.test.tsx \
 src/renderer/src/hierarchy/AgentModelSettings.test.tsx
pnpm --filter @matou/desktop typecheck
opencli doctor
pnpm exec playwright test tests/e2e/model-provider-routing.spec.ts
pnpm check:identifiers
git diff --check
```

| 记录（工作区 .cache/，另复制到监督 logs/） | 结果 |
| --- | --- |
| task4-red.log | 新组件尚不存在时红灯，未当成通过 |
| task4-first.log | 第一轮8项组件通过 |
| task4-desktop-final.log + task4-parent-delivery-green.log | 最终去重3文件41项通过：新分配12、父设置14、压缩15，无跳过；父设置补目录重试不重复读取订阅用例 |
| task4-types-delivery-final.log | desktop typecheck通过 |
| task4-e2e-delivery.log | 1项本地浏览器/runtime E2E通过，17.6秒，无跳过 |
| task4-identifiers.log | 命名门禁通过；git diff --check通过 |

本批合计 **41项组件/集成 + 1项本地E2E通过**，不重复累加中间轮次。没有重跑无改动的Task1–3全量测试，没有做完整App构建或安装。

E2E以 Playwright **仅作测试runner**；所有浏览器操作由OpenCLI执行，未使用Playwright page/Electron自动点击，也未使用其它电脑操作工具。临时Vite入口挂载未经替换的正式 `ModelSwitchSettings`；HTTP `/rpc` 只转发真实 `RuntimeServer` 请求/响应，不mock配置RPC。配置真实写入隔离SQLite和provider store；随后启动真实runtime的node-pty，由隔离假CLI发送模型请求。

临时数据在 `.cache/model-provider-routing-e2e/<pid>/`；账号/config目录隔离，不读取真实凭据。使用3个本地API模拟目标，没有声称ChatGPT OAuth/router协议转换已经通过。测试关闭自己的浏览器session、Vite、PTY与上游；保留本地数据供复查。

## Mockup / reference product 交互对照矩阵

设计基准只读：原仓库 `docs/prd/mockups/model-provider-routing-2026-11-17/index.html`、`default.png`、`saved.png`、`error-model.png`。本批没有取得 reference product 同范围原始可运行产品的新增证据，因此下表以已确认Mockup作UI基准，不宣称reference product逐项1:1或PRD整体验收通过。

| 场景 | Mockup基准 | 正式组件/实际结果 | 证据与结论 |
| --- | --- | --- | --- |
| 插入位置/默认 | 供应商列表与压缩之间，三档三列 | 正式父设置页同位置、同控件关系 | evidence/default.png；Terra查看对照，视觉关系通过，未做逐像素差异 |
| 改供应商 | 清模型、缺模型禁存 | 受控select清model，无静默跨provider复用 | 组件测试；evidence/editing.png及opencli.json |
| 默认目标 | 未分配沿用默认，显示最终目标 | 显式mapping优先；否则使用native目录档位名称，绝不读取总model替代 | 组件默认/无映射API测试；E2E Haiku跟随第三目标 |
| 保存/重开 | 保存提示、按钮焦点不丢 | 真实patch落库，关闭设置后重开读回，aria-disabled保留焦点 | evidence/saved.png、opencli.json；实际scrollTop前后均443.5 |
| 失效模型 | 原选择、红框、底色、行内说明 | 显式目录移除gpt-local后重开，保留旧值并禁存 | evidence/error-model.png；Terra已看图，错误层级一致 |
| 失效账号/删provider | 保留旧值，允许改选或恢复 | 行内错误；恢复默认清草稿且需保存 | 组件测试；未做真实账号掉线 |
| 目录读取失败/刷新 | 草稿保留，可重试 | 父读取失败不清目录；dirty草稿不被provider更新重置 | 组件测试，未制造真实服务断线 |
| 手填API ID | 沿用简单下拉/手填模式 | 仅无权威目录API可手填，无新增弹窗 | 组件测试，真实UI手填未单独OpenCLI操作 |
| 恢复默认 | 清三档但需保存 | E2E先恢复，读库仍有旧分配；点保存才清库 | 真实RPC读回和最终requests.json |
| 与压缩并发 | 不干扰其它设置 | Promise.all并发legacy set与assignment patch，随后UI保存默认仍保留压缩provider | requests.json真实持久化结果 |
| 活会话保持 | 新会话生效，活会话不变 | 修改分配/压缩期间PID保持9974 | requests.json；未做忙终端性能采样 |
| 三目标请求 | 各档独立 | Opus→gpt-local/local-a；Sonnet→deepseek-local/local-b；Haiku→haiku-local/local-c；三者均有message/count | requests.json，共7条loopback请求，压缩跟随Sonnet |
| 错误不回落 | 失败明确 | invalid role返回502，上游仍仅7条 | requests.json假CLI结果；当时未覆盖任意自定义model范围；第五批已补source消息/count回归，见文末 |
| 键盘 | 原生select/button，Enter/Space | 组件级Space保存及重复Enter防写通过；OpenCLI focus+Enter未触发保存 | task4-allocation-delivery.log通过；task4-e2e-browser-final.log探针失败，真实键盘未闭合 |
| 窄屏 | 640px下逐档纵排，带字段标签 | 560px临时截图中供应商/模型纵排，错误随行，无明显裁切 | evidence/narrow-560.png；Terra已查看，仅截图验证，未做持续窄屏交互 |
| 中英文 | 正式语言，无演示条 | 组件英文文案/选择读回通过；浏览器中文截图 | 单元测试；英文真实浏览器未单独验收 |

## 浏览器执行与中间失败

浏览器由按约定委派的 `gpt-6.1-terra / high` 执行OpenCLI，主代理负责实现和整合；没有委派DSH。

1. 首次fixture将node-pty打进临时bundle，native路径失真，在浏览器之前失败。改为现有runtime构建相同的依赖打包清单，node-pty通过绝对安装路径external，未改生产构建。
2. 第二次Vite入口在.cache目录，找不到React。显式复用desktop已安装React/react-dom后通过，未安装依赖。
3. 第三次完整点击E2E通过；Terra发现错误截图未入视口，后续补focus错误行。
4. 键盘补验OpenCLI显示focus成功并发送Enter，但未出现保存提示。没有把它解释为已通过或修改产品迎合工具；保留失败日志，键盘探针未闭合。
5. 最终恢复点击流程，补滚动不变断言、错误focus与560截图，完整E2E再次通过。先前的状态截图已由最终结果覆盖，失败日志另留。
6. 父设置旧测试的宽泛文本查询与新增option重名；限定卡片查询并补合法agent snapshot。中间误插到一个特殊Promise桩的同步返回已纠正，最终41项无未处理错误。

## 我自己做的决定

1. 复用父组件既有snapshot/accounts，不新增额度查询；只在明确手动重试或现有刷新触发时读设置。
2. 用局部纯helper处理目录和默认展示，权威provider目录判失效，补入旧mapping候选不能掩盖已删除模型。
3. 保存成功用aria-disabled保留按钮焦点；异步保存期间用ref防重复，草稿在失败和列表刷新时保留。
4. 父读取加请求序号并在patch成功后作废旧读取，防止晚返回GET覆盖新分配。
5. 沿用项目现有zh/en组件文案方式和原型CSS，不增视觉探索、弹窗、依赖或画布流程。
6. 本地E2E采用正式父设置组件+真实RPC/runtime的隔离浏览器入口；Playwright只运行测试，浏览器动作全部OpenCLI。没有把它说成完整Electron应用验收。
7. Terra验收决定：错误截图需先focus失效模型行；560截图只作为临时视口的视觉证据，不冒充持续窄屏操作；键盘探针失败如实交回。

## 未闭合与下一批边界

- 真实键盘Enter/Space、持续窄屏交互、英文浏览器、真实账号、真实订阅协议转换、原生CLI Task子代理、忙终端交互性能、完整Electron设置入口，以及reference product运行对照仍待验收。
- 没有派独立代码审查代理（当次仅允许Terra界面操作代理），实现者自查不冒充独立审查。
- 三项后端审查问题已在第五批修复，并补本地HTTP及账号生命周期验证；本E2E的“非法档位不回落”本身不能覆盖这些问题。详见[第五批修复与验证](review-fixes.md)。
- 当前交付可供继续审查，不把Task4代码和本地E2E成功作为Task5全部完成或整个功能产品验收通过。

## 第五批后端修复状态

三项均已修复并通过本地验证：自定义模型名不误分流，custom压缩纳入安全预算，订阅精确目标在换号/删除/close时撤销并取消未完成请求。回滚不复活旧绑定，同账号只改模型设置保留绑定。

最终相关回归去重572项通过（runtime333、desktop134、contracts105），另17项既有固定router集成未执行；runtime/desktop/contracts类型检查、命名门禁及diff检查通过。前批真实UI截图保留，本批未重跑界面或完整Electron，未声称固定router转换或真实账号通过。范围、负向控制、中间失败和复跑证据见[第五批修复与验证](review-fixes.md)。

## 最终构建及二次复审后状态

[最终构建与隔离应用记录](final-app-acceptance.md)区分修改前后结果。03:27整轮runtime332通过、1项旧Bypass场景超时、17项既有router跳过；单独复验通过，仍保留整轮失败。desktop134/contracts105与前次组件E2E通过，首次完整build通过。

03:34复审确认的同名credential误删及混合分配默认目标缺少保存校验已获授权修复；增加同名成功/失败、默认模型/账号失效、清空退出、全部指定与未知目录测试。修复后重新完整build、三处types及OpenCLI E2E；具体最终数字、截图、App启动结果和限制以上述最终记录为准，旧批572通过不能替代当前整轮状态。

最终修复后：核心runtime118+实际runtime关联4、desktop139、contracts105共366项通过，组件E2E另1通过；完整build、三处types与命名/diff检查通过。监督者已常驻启动隔离完整Electron（PID9436、launcher9248），正式renderer及独立runtime/socket/SQLite就绪，证据`evidence/electron-launch.json`。OpenCLI无法接入Electron，完整App设置交互与截图仍未验；保留修改前整轮超时，不称整体验收全部通过。
