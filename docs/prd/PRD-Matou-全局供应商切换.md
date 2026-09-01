# PRD：Matou 全局供应商切换

## 1. 产品结论

Matou 原“会话内切换模型”调整为“全局配置并切换 CLI 供应商”。入口固定在左侧边栏底部的「设置」，设置内新增「模型切换」。

首版只闭合两件事：

1. 为 Claude Code、Codex 新增或编辑供应商配置；
2. 为每个 CLI 选择一个全局使用中的供应商。

不加入用量统计、故障转移、MCP、Skills、Prompt、云同步、多账号和托盘能力。

## 2. 用户场景

- 用户在官方服务和自建网关之间切换，不再逐会话修改模型。
- 用户为 Claude Code 与 Codex 分别维护 API 地址、API Key 和默认模型。
- 多个 Matou 窗口共享同一全局选择，避免窗口之间配置不一致。

## 3. 交互规格

### 3.1 入口

- 移除终端底部 HUD 的模型按钮与模型菜单。
- 左侧边栏底部保留「看板」，右侧新增固定「设置」。
- 点击「设置」后，当前工作区显示设置页；左侧边栏保持可见。
- 设置页顶部显示「设置 · 模型切换」页签；关闭按钮或 `Esc` 返回原工作区。

### 3.2 模型切换页

- 一级设置导航：`AI 服务 / 模型切换`。
- CLI 标签：`Claude Code`、`Codex`。
- 页面展示当前全局供应商、供应商列表和全局影响说明。
- 供应商卡片支持编辑与切换；当前配置按钮禁用。
- 新增/编辑字段：供应商名称、API 地址、API Key、默认模型。
- API 地址必须是 HTTP/HTTPS；编辑时 API Key 留空表示保留原 Key。
- 非官方且非当前供应商可以删除。

### 3.3 生效规则

- Claude Code：切换后自动重启 Matou 中运行的 Claude Code 会话，并恢复已有会话身份。
- Codex：新会话使用新供应商；运行中的 Codex 会话继续保持当前连接。
- API Key 持久化文件权限为当前用户读写，渲染层仅获得 `Key 已配置` 状态。

## 4. 状态与反馈

- 载入：显示“正在载入供应商配置”。
- 保存：按钮显示“保存中”。
- 切换、保存、新增、删除成功：右下角 Toast。
- 地址、名称、默认模型不合法：弹窗内直接显示错误。
- Runtime 尚在连接：设置页保留并提供明确错误状态。

## 5. reference product 交互对照矩阵

| 场景 | reference product 基线 | Matou 结果 | 运行证据 | 差异结论 |
|---|---|---|---|---|
| 左侧边栏保持工作上下文 | 黑色 CLI 工作区内侧栏持续可见 | 设置打开后事项侧栏持续可见 | `docs/acceptance/evidence/prd-02/matou/model-switch-settings.png` | 一致 |
| 终端底部 HUD | reference product HUD 承载会话状态和权限，不承担供应商配置 | 移除旧会话模型入口，保留权限、上下文、任务与路径状态 | `tests/e2e/prd-02-bottom-hud.spec.ts` | 按确认规格调整 |
| 设置页面 | reference product 当前对照范围无同类供应商配置页 | 在 CLI 工作区内部新增设置页，沿用当前 Matou 玻璃侧栏与页签层级 | `docs/acceptance/evidence/prd-02/matou/model-switch-settings.png` | 新增能力，无同范围基线 |
| 弹窗和状态反馈 | reference product 使用居中弹窗与即时反馈 | 新增/编辑使用居中弹窗，切换结果使用 Toast | `apps/desktop/src/renderer/src/hierarchy/ModelSwitchSettings.test.tsx` | 交互模式一致 |

## 6. 验收结果

- 单元与组件测试覆盖配置持久化、Key 掩码、启动环境、模型参数、侧栏入口、CRUD 与切换反馈。
- E2E 覆盖旧模型入口移除、设置入口打开、设置页渲染及返回工作区。
- 构建、类型检查和相关 E2E 均通过。
