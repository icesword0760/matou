# Codex 已知问题收口

2026-11-15，分支 `feat/multi-agent-codex`，产品源码 `cc2b1db`。本轮仅处理错误通知、history_base 和启动升级提示；不再尝试外部同父 Fork 并发。没有新增依赖、存储格式或运行时 LLM 请求。

## 1. 失败形态与实现依据

真实 Codex CLI **0.162.1**，项目内临时仓库，独立 CODEX_HOME，`--no-daemon --no-alt-screen`。三个探针都未复制登录凭据，模型端点仅为本机；不通过真实模型调用制造错误，也不执行失败的工具命令来冒充模型失败。原始脚本、hook、rollout事件及可还原的TUI输出位于 [probes](probes)。

| 失败形态 | 真实 TUI / 退出 | 结构化 rollout | hooks / Stop | 结论 |
| --- | --- | --- | --- | --- |
| 本机模型端点返回404 `model_not_found` | 显示 `unexpected status 404 Not Found`，回到输入框；采集结束时Ctrl-C退出码0 | `event_msg.payload.type=task_complete`，同回合 `error.message` 与 `error.codex_error_info.http_connection_failed.http_status_code=404`；last_agent_message=null | SessionStart、UserPromptSubmit、SessionEnd；**无Stop** | 有确定性整轮错误信号。不能等待Stop，也不能以进程退出0判为成功。 |
| 本机不存在端口 `127.0.0.1:1` | 持续 `Reconnecting... waiting for network`；15秒采样后主动中断，退出码0 | task_started / UserMessage；主动中断后才有 `turn_aborted`，不是模型已失败的证据 | SessionStart、UserPromptSubmit、SessionEnd；**无Stop** | 正在重连没有终局错误；不把用户中断判成错误。 |
| 模型请求途中SIGKILL整个CLI | 约7秒后强制结束，进程退出-9 | task_started / UserMessage；没有task_complete.error、error或turn_aborted | SessionStart、UserPromptSubmit；**无Stop / SessionEnd** | 只有结构化进程退出，rollout中无法判定一轮模型失败；沿用进程退出处理，不伪造模型错误通知。 |

本轮 Codex 专属实现：UserPromptSubmit 给出当前turn_id与真实transcript_path后，沿用ProviderHookServer生命周期监听**该文件**变化，读取有界尾部，只接受该turn的 `task_complete.error.message`。发一条标题 **Codex** 的error通知，并更新工作状态为error；同回合重复写入不重复通知。不会扫描全盘、持续轮询或解析TUI文案。新回合清除旧错误锁定；旧run、被取代/抑制/释放的注册不能继续通知。历史失败、普通CommandExecution非零退出和用户turn_aborted均不触发。

自动检查覆盖：真实404载荷、无Stop仍通知、工具失败/中断/历史错误排除、JSON尾行尚未写完、重复事件去重、失败后正常完成、run替换和retire。Claude / Antigravity分支及其通知断言没有改动。

## 2. history_base

预览、内容搜索及会话全文搜索沿 `session_meta.payload.history_base.thread_id` 回溯，最多5层祖先，以UUID防环。允许祖先位于另一个Worktree；只解析Codex会话目录内元数据指向的文件，不把thread_id当文件路径。消息按时间合并，以稳定消息ID去除跨文件副本，并重新生成统一分页/命中索引；缓存纳入各祖先文件版本。存在end_ordinal_exclusive / end_byte_offset时遵守Fork边界，不读入父会话在Fork之后的消息。

祖先缺失时回退当前文件，在预览末尾显示“祖先历史不完整：祖先文件缺失或不可读，仅显示当前会话。”；循环和深度上限也显示原因。标注复用既有system预览项，不新增字段或存储格式。原生载入仍按当前子UUID执行，CLI自身恢复引用历史。

自动检查覆盖单层、跨Worktree、多层、环、缺失、5层上限、字节/ordinal边界与祖先更新后的缓存失效。[真实历史回读](history-live.json)使用上一轮真实当前/新Worktree Fork的**未修改rollout**：两子预览分别6/8条，祖先暗号全文命中2/3条，会话全文列表也包含对应子UUID。没有重跑或扩展外部同父Fork。

## 3. 启动升级提示

[官方配置说明](https://learn.chatgpt.com/docs/config-file/config-reference)与本机0.162.1 schema均确认 `check_for_update_on_startup`。产品所有Codex新建、resume、fork统一加一次性 `-c check_for_update_on_startup=false`；不改用户配置。Claude / Antigravity参数保持原样。

真实开关对照为两份独立CODEX_HOME，version.json都预置刚检查的999.0.0：true出现升级选择界面，false直接进入普通终端；没有向升级界面发送Enter，也没有执行安装。证据见 [对照结果](probes/update-summary.json) 及两份TUI。false探针中Codex自身在隔离配置递增了新模型提示计数（1→2）；该写入与升级配置无关，不是Matou写配置，真实用户文件未改。

## 自动验证与产品包

[验证摘要](validation-summary.json)：184个不同相关用例最终通过。三文件79通过，相邻四文件87通过，runtime Codex17项最终通过，另补验1条覆盖所有managed profile的启动控制测试通过；runtime首跑4个精确启动参数断言仍预期旧参数，按新启动要求仅修Codex断言后4项定点通过，初跑13项不重复计数。144个初跑未选中项与157个定点未选中项不是新增跳过用例。

`pnpm typecheck`、`pnpm check:identifiers`通过。未重跑runtime/desktop全量，也没有重采已接受的性能样本。`CSC_IDENTITY_AUTO_DISCOVERY=false pnpm package:dir`退出0，[六组件指纹](package-sha256.json)对应产品源码cc2b1db；App未签名、未安装到Applications、未发布。

打包App界面结果见 [逐次界面记录](ui-report.md)。祖先UI受控fixture一次通过；错误场景前两次未读到error，原始失败全部保留。第二次请求日志已查明：有两个完整POST，一个105549 bytes未响应，另一个55137 bytes已响应，测试端点的单个pending指针被后来的请求覆盖。主代理修测试端点为回复全部并发请求后，第三次已通过错误/通知/DB/DAG检查，但读取终端时定位器随焦点指向了Shell；修成按session定位后，第四次8.034秒全部通过（retries=0、退出0）。两份旧失败加这份定位失败保留，未改产品或重打包，不把失败覆盖成整套首跑通过。模型404使用真实CLI和受控本机端点，祖先UI使用明确标注的受控rollout，不把两者混称为真实模型/真实Fork。

最终打包App证据：[唯一Codex错误通知](ui-fixed-selector/error-notification.png)、[DAG异常状态](ui-fixed-selector/error-dag.png)、[同回合结构化失败及通知数量](ui-fixed-selector/results/model-error-ui.json)。祖先场景首跑5.133秒通过，错误场景四次分别27.376 / 26.260 / 21.167 / 8.034秒；最终两种不同场景均通过，不把5次尝试计为5个不同用例。

标识门禁曾对三张PNG压缩数据中的偶然短字节串报错；原图备份在忽略的项目缓存，仅无损重压IDAT，解压后的过滤扫描行逐字节相同，其余PNG块原样保留。没有改变像素、视觉证据或放宽门禁。
仓库日志与失败上下文只清理行尾空白；原始输出保留在监督日志及本项目忽略的测试缓存。
