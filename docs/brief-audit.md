# 任务：订阅请求转换与安全浏览器集成的独立审计（三轮）

## 目标

对 `feat/login-browser-integration` 分支从 `2658035` 到 HEAD 的全部改动做独立审计，对照四个对标项目，找出我们漏掉的功能差异。只审功能，不审许可证和边界情况。

## 审计范围

1. 路由器补丁：`tooling/model-router/patches/antigravity-client-identity.patch`（以及构建后的实际源码）。
2. Matou 集成：安全浏览器启动器、登录改道、面板卡片、客户端提醒、Antigravity 出口保护、订阅会话降噪。
3. 提示词清洗：计费行、身份句、竞品词、代码块保护、扫描窗口、Codex developer 文本。

## 对标项目（逐个对照，不接受泛泛而谈）

- `/tmp/router-research/repos/Antigravity-Manager`（只读行为参考，不要复制）
- `/tmp/router-research/repos/antigravity-claude-proxy`
- `/tmp/router-research/repos/sub2api`
- `/tmp/router-research/repos/CLIProxyAPI`（上游最新版；我们 pinned 的是 8.0.69）

如果 `/tmp/router-research` 不存在，重新浅克隆；不要改这些仓库。

## 审计方法

对每个能力项输出一行矩阵：

| 能力 | 对标项目怎么做（文件:行号） | 我们怎么做（文件:行号） | 差异 | 严重度 | 建议 |

严重度：

- **高**：会导致请求失败、工具调用失败、用户消息被误改。
- **中**：降低成功率、暴露客户端特征、缓存失效。
- **低**：文案、注释、测试覆盖。

要求：

- 每条结论必须有文件:行号证据；没有证据的猜测不写。
- 只交疑点清单，不写“通过/不通过”；由我裁决。
- 不修改任何代码，只读审计。
- 许可证、边界情况、发版清单不在范围内。

## 三轮安排

### 第一轮：查漏

对照四个项目，覆盖至少：

- 计费行剥离
- 身份句/身份归一
- 竞品词归一
- 代码块与行内代码豁免
- 扫描窗口
- 作用范围（system vs 用户消息 vs 工具/schema）
- Codex developer 文本
- 官方 Claude 路径是否被误改
- 请求头/UA/TLS 形态
- JSON Schema 清洗
- tool_choice / functionCall 归集 / MCP 工具结果
- session ID / 缓存前缀
- thinking budget / max_tokens 关系
- 多轮工具调用与签名

产出：矩阵 + 疑点清单。

### 第二轮：复核与未覆盖面

- 复核第一轮判定为“需要补”的项是否已修、是否引入回归。
- 再扫一遍未覆盖区域：官方 Claude 路径、prompt 缓存、工具/schema 隔离、非订阅会话。
- 产出：剩余疑点 + 回归风险。

### 第三轮：最终功能完整性

- 逐项确认关键路径都有测试覆盖，行为与对标项目一致或差异有明确理由。
- 找出仍遗漏的清洗项或协议细节。
- 产出：最终疑点清单（可以为空）。

## 交付

每轮回复给出：

- 矩阵（新增/更新部分）；
- 疑点清单，按严重度排序；
- 每条的文件:行号和建议动作；
- 本轮未覆盖或无法判断的项。

## 范围

- 工作目录：`/Users/icesword/Documents/AIProjects/matou-login-integration`
- 只读；不改代码、不改配置、不提交。
- 不能碰主目录、其他 worktree、`safe-browser` 项目。
