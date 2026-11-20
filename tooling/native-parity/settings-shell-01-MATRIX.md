# 设置入口与 Claude Code 供应商验收

本关从 `79b972ff` 的原生供应商命令接线开始。参考源码使用 [设置冻结说明](../../artifacts/native-parity/native/evidence/assignment-ui-01/ui.md)、[供应商冻结说明](../../artifacts/native-parity/native/evidence/provider-management-01/ui.md) 和 [命令说明](../../artifacts/native-parity/native/evidence/provider-management-01/flow.md)。可运行参考为隔离 Electron `924de28d`。原始窗口 PNG 为 2400 × 1560，窗口逻辑尺寸为 1200 × 780，DPR 为 2。

## 实现

- 侧栏底部「设置」整枚按钮支持鼠标和键盘。它覆盖右侧工作区，场景标签与终端隐藏，终端实体继续运行。再次点「设置」、点页眉 × 或按 Escape 关闭，恢复当前终端焦点。
- 左栏「AI 服务」「通用」为静态分组标题，9 点、700 字重、.08em 字距，不参与点击和 Tab。Claude Code 默认打开，切回时重新读取供应商。「语言与时区」保留冻结标题、副标题和空正文，**内容待后续关卡**。
- 「服务供应商」基于脱敏快照。活动项置顶，余项维持原顺序。API 卡显示名称、地址、默认模型和 Key 是否配置。订阅卡只读。切换失败保留当前活动项。
- 新增支持 API 预设和自定义，编辑直接进入表单。提交支持仅保存以及保存后切换。失败保留草稿并展示运行时原文。已保存的 Key 只以存在标记进入桌面，编辑 Key 初始为空，留空不随请求发送。

## 参考产品交互对照

| 场景 | 冻结依据和参考实窗 | 原生实现与验收 |
| --- | --- | --- |
| 设置入口和关闭 | 底栏 38，按钮高 30，图标 16，图文距 7，左右内距 10。设置状态按下，按 Escape 关闭。 [参考设置页](output/settings-shell-01/screenshots/reference-settings.png) | 隐藏 GPUI 探针验证按钮开关、重新挂载、Escape 和终端焦点。原生尚无可开合看板，无法实测从看板切入。 |
| 外壳 | 左分类宽 176，内容内距 24/28/28，关闭按钮 28 × 28，入场 180ms ease-out，减弱动态效果 1ms。 [参考设置页](output/settings-shell-01/screenshots/reference-settings.png) | 色值、尺寸和位置见实窗 PNG 读数。 |
| 分类 | 冻结参考中 AI 服务和通用是分组标题，另两项可点。有冻结副标题的页面使用原文。 [参考语言页](output/settings-shell-01/screenshots/reference-language.png) | 已回到冻结结构。只有 Claude Code 和语言与时区可选。「AI 服务」「通用」仅为分组标题。语言页显示其既有副标题，正文为空，**内容待后续关卡**。 |
| 加载、正常、空和错误 | 首读「正在载入供应商配置…」。失败原文和「重试」。 [参考正常页](output/settings-shell-01/screenshots/reference-settings.png) | 隐藏探针覆盖加载、过期回包、空快照、读失败。真实运行时会从空列表修复出内置官方配置，空与加载用同生产 UI 的受控回包窗口验收。 |
| 活动和切换 | 「使用中」「默认使用」「切换」「切换中…」。活动项置顶，其他按快照顺序。 [参考设置页](output/settings-shell-01/screenshots/reference-settings.png) | 隐藏探针覆盖命令、排序、失败状态和再次发送。原生接口没有现有 Claude 会话迁移结果，仅反馈全局默认和新会话。 |
| 新增 | 四个 API 预设、搜索和自定义。冻结还包含官方及订阅入口。 [参考新增](output/settings-shell-01/screenshots/reference-add.png) · [参考自定义](output/settings-shell-01/screenshots/reference-custom.png) | 本关为 API 新增，订阅登录待后续。弹窗只遮罩右区并在右区居中，宽 520。返工移除了左侧工作区遮挡层。 |
| 编辑与错误 | 编辑起始 Key 为空，已配置显示「已配置 · 留空保持原密钥」。原文错误仍保留编辑器。 [参考编辑](output/settings-shell-01/screenshots/reference-edit.png) | 隐藏探针验证提交命令、Key 留空不入参、错误原文、对话框继续打开及 Escape。实窗保存与重启已核对。 |

## MATRIX 待补项

- **内容待后续关卡**。语言与时区的正文。Claude Code 下方的模型分配、对话压缩模型和派活也未接入。
- 模型发现与刷新、删除、订阅账号相关交互、上下文窗口编辑。
- 切换说明正文待能力落地后补。原生在供应商列表下方保留加粗的「切换会影响所有 码头 窗口。」。冻结正文为「所有 Claude Code 会话跟随全局供应商；暂缓会话就绪后自动切换，旧会话再次打开时也会应用，历史保留。上下文窗口可在「编辑」中设置。」待暂缓会话自动切换、旧会话重开生效和上下文窗口编辑落地后补齐。[hierarchyShell.ts:188–190，正文续行 191](../../apps/desktop/src/renderer/src/i18n/messages/hierarchyShell.ts#L188-L191)
- 缺「默认窗口」徽标，随上下文窗口能力一起补。参考实窗在活动卡名称「Anthropic 官方」右侧、「使用中」之后；原生同一名称行止于「使用中」。[参考实窗](output/settings-shell-01/screenshots/reference-settings.png) · [描边复测实窗](output/settings-shell-01/border/screenshots/settings-final.png) · [context-window-ui.tsx:69、93](../../apps/desktop/src/renderer/src/hierarchy/context-window-ui.tsx#L69) · [provider-setup.css:159](../../apps/desktop/src/renderer/src/hierarchy/provider-setup.css#L159)
- 冻结活动行处于非末行时有 1 点蓝色底边。原生活动卡仍统一使用灰色外框，此多卡边界待补。
- 应用主题接线与深色实窗。GPUI 的原生可访问性接口待补，现有按下状态通过视觉和键盘操作表达。
- 完整像素一致性、Reduce Motion 和窗口断点的真实窗口覆盖。

## 真实窗口证据

所有截图均在本机隔离窗口取得。两套应用使用各自测试数据根，供应商地址为 `.invalid`，Key 为虚构测试值。

| 场景 | 实窗结果 |
| --- | --- |
| 正常 / 加载 / 空 / 错误 | [描边复测后正常](output/settings-shell-01/border/screenshots/settings-final.png) · [加载](output/settings-shell-01/screenshots/native-loading.png) · [空配置](output/settings-shell-01/screenshots/native-empty.png) · [读取错误](output/settings-shell-01/screenshots/native-load-error.png)。加载和空配置使用可见 GPUI 探针的受控回包。真实运行时读取损坏 JSON 时显示 `Invalid provider configuration JSON`。移走损坏夹具后点「重试」，成功恢复官方单卡。页眉 × 返回终端。 |
| 首轮四项导航（历史） | [AI 服务](output/settings-shell-01/screenshots/native-ai.png) · [通用](output/settings-shell-01/screenshots/native-general.png) · [语言与时区](output/settings-shell-01/screenshots/native-language.png)。首轮曾将两个分组标做成页面。这组旧图仅留档，返工版已将两个入口恢复为静态标题。 |
| 新增与写入失败 | [最终新增选择页](output/settings-shell-01/screenshots/native-add-final.png) · [已填写且 Key 遮蔽](output/settings-shell-01/screenshots/native-custom.png) · [真实保存错误](output/settings-shell-01/screenshots/native-save-error.png)。第一次「仅保存」显示 `Could not write provider configuration`，名称、地址、默认模型、Opus 和「高」均保留。解除隔离写入故障后，直接重试成功。 |
| 编辑、切换与焦点 | 编辑改名和默认模型后保存，Key 留空，卡片继续显示「Key 已配置」。点击「切换」后活动项置顶并出现「使用中」。关闭设置后，未点击终端便输入命令，终端输出 `SETTINGS_FOCUS_OK`。[切换结果](output/settings-shell-01/screenshots/native-switched.png) · [终端焦点](output/settings-shell-01/screenshots/native-terminal-focus.png) |
| 重启保留 | 以同一隔离数据根重启，活动项和所有测试字段保留。编辑仍提示「已配置 · 留空保持原密钥」。已存 Opus 映射和「高」可读。配置文件权限为 0600，虚构测试 Key 的值跨编辑和重启保持，进程日志未检出该测试值。[重启后](output/settings-shell-01/screenshots/native-restarted.png) · [编辑 Key 提示](output/settings-shell-01/screenshots/native-restarted-edit.png) · [映射与强度](output/settings-shell-01/screenshots/native-restarted-mapping.png) · [脱敏落盘回读](output/settings-shell-01/native-readback.json) |

## 返工实窗证据

| 场景 | 结果 |
| --- | --- |
| 左导航 | 点击两个分组标题均留在「语言与时区」页。点击 Claude Code 返回供应商页。只显示语言页冻结标题、副标题和空正文。 [设置](output/settings-shell-01/rework/screenshots/settings.png) · [语言与时区](output/settings-shell-01/rework/screenshots/language.png) |
| 左侧工作区栏 | 在同一「编辑供应商」弹窗开启时滚动隔离工作区的事项，并在左栏点击标题折叠工作区。右区编辑表单保持打开。 [滚动前](output/settings-shell-01/rework/screenshots/sidebar-modal-before.png) · [滚动后](output/settings-shell-01/rework/screenshots/sidebar-modal-scroll.png) · [折叠后](output/settings-shell-01/rework/screenshots/sidebar-modal-collapse.png) |
| 角色强度提交 | 旧 Opus「高」点击保存时显示「opus 的推理强度不受当前模型支持，请重新选择。」改成「中」后，隔离写入故障显示 `Could not write provider configuration`，表单仍保留「中」。两次失败后文件仍为原值「高」和修订 1。移除故障后直接重试成功，重新打开编辑确认原模型和「中」，落盘修订 2。 [校验失败](output/settings-shell-01/rework/screenshots/effort-invalid.png) · [写入失败与草稿](output/settings-shell-01/rework/screenshots/effort-write-failed.png) · [重试成功后重新打开](output/settings-shell-01/rework/screenshots/effort-restored.png) · [落盘回读](output/settings-shell-01/rework/effort-readback.json) |

## 像素读数

使用同尺寸 2400 × 1560 窗口 PNG，无阴影，DPR 2。下表位置均按窗口逻辑点表示，区间右端不含。按截图色块读取，比较窗口的右工作区。没有掩罩，也没有额外容差。 [返工前设置读数](output/settings-shell-01/rework/pixels-before.json) · [返工后设置读数](output/settings-shell-01/rework/pixels-after.json) · [弹窗读数](output/settings-shell-01/dialog-pixels.json) · [测量脚本](measure-settings-shell.py)

| 部位 | 冻结参考实窗 | 最终原生实窗 |
| --- | --- | --- |
| 正文画布与导航分隔 | `#f7f8fa`，正文从 x=436 开始，分类列 176 | 同值 |
| 关闭按钮 | 实线扫描 x=1144–1172，跨度 28 | 同值 |
| 活动 API 卡填充 | y=185 扫描 x=467–1171。x=850 扫描 y=126.5–206.5，80 点。原始 RGB `239,245,254` | 横向和返工后纵向同值，返工前为 y=126.5–204.5、78 点。原始 RGB `238,245,254`。 |
| 活动 API 卡描边与高度来源 | 卡片最小高 80、内距 14。列表外侧顶底各 1 点边线，参考单卡没有自己的底边线。实窗顶底采样 RGB `232,234,236`。 | 旧原生外框 80 点含顶底各 1 点边框，剩 78 点填充。外框改为 82 点后，填充恢复 80 点。内距仍为 14，历史蓝边采样为 `118,159,232`。本轮单卡描边复测见下表。 |
| 新增弹窗 | 白色净宽 518，中心 x=730，对应含边框外宽 520。白色纵跨度 734。参考还有账号选择，弹窗高度达到上限。 | 同宽、同中心。白色纵跨度 626，差 108 点。本关只显示 API 预设和自定义。 |
| 上一轮右区头部与供应商首卡裁切（历史） | 比较窗口逻辑 x=260–1200、y=0–210，789,600 个设备像素 | 返工前 325,920 个、返工后 325,853 个像素存在至少一个 RGB 通道差异，包含文本、描边、卡片填充与内容差异。本轮复测活动单卡。 |

### 活动单卡描边复测

参考和原生截图均为 2400 × 1560，以下为设备像素坐标和 PNG 原始 RGB。区间端点均含。活动外框用 `rgba(LINE)` 叠在普通卡底色上，蓝色填充从框内开始。仅改线色时测得 `222,228,237`，将填充分层后四边与参考同为 `232,234,236`。左蓝条仍占 4 个设备像素，内填充高度仍为 80 点。 [冻结参考](output/settings-shell-01/screenshots/reference-settings.png) · [描边复测实窗](output/settings-shell-01/border/screenshots/settings-final.png) · [逐点读数](output/settings-shell-01/border/card-pixels.json)

| 部位 | 设备像素坐标 | 冻结参考 RGB | 原生修正前 RGB | 原生修正后 RGB |
| --- | --- | --- | --- | --- |
| 左外缘 | y=370，x=928–929 | `232,234,236` | `118,159,232` | `232,234,236` |
| 左侧 2 点蓝条 | y=370，x=930–933 | `73,126,208` | `73,125,208` | `73,125,208` |
| 右外缘 | y=370，x=2342–2343 | `232,234,236` | `118,159,232` | `232,234,236` |
| 上外缘 | x=1700，y=251–252 | `232,234,236` | `118,159,232` | `232,234,236` |
| 下外缘 | x=1700，y=413–414 | `232,234,236` | `118,159,232` | `232,234,236` |
| 填充 | y=370，x=934 与 1700 | `239,245,254` | `238,245,254` | `238,245,254` |

## 自检与剩余差异

- 返工在隔离构建副本验证，使用已提交基线 `79a28578` 和本任务逐文件核对的桌面源码。桌面单测和集成测试 32 项通过。隐藏 GPUI 探针覆盖键盘入口开关、Tab 跳过分组标、分类实际切换和重读、旧回包、加载、空快照、读取失败、活动排序、切换/失败/重试、保存/失败、保存后切换失败时保留既有 ID、编辑器 Escape 与终端焦点。探针不接模型网络。
- 桌面原生构建、examples 编译、Rust 格式、标识门禁和 `git diff --check` 通过。角色强度统一在提交时校验，返工实窗覆盖了不支持值、修正强度、真实写入错误、草稿保留和解除故障后的重试。地址变化仍按草稿原字面判断。
- 看板目前只有入口外观，没有可打开的原生看板状态。「先开看板，再开设置」待看板具备真实入口后验证。
- 新增选择页只含 API，官方及订阅账号入口与筛选待后续。订阅卡从快照展示资料与状态，本轮实窗没有订阅夹具。角色模型可手填，未接已存模型选择下拉。原有模型信息参与强度限制。Key 一直遮蔽，省略明文切换与获取 Key 链接。
- 原生为右工作区遮罩，弹窗开启时左侧工作区栏仍可滚动和点击。参考底层的模糊、字体栅格、部分字距和字号未完全复现。输入正文使用现有 12 点原生输入，参考为 14 点。实窗未量动画时间和断点，深色未验。
- 编辑真实窗口曾出现一次未保存确认，选择「继续编辑」后成功保存，未确认首次触发原因。
