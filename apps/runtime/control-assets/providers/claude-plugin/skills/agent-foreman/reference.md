# 派活监工参考

脚本：`<本技能目录>/scripts/foreman.mjs`（下文简写 `foreman`）。运行记录默认在 `~/.claude/agent-foreman/runs/<task>/`，可用环境变量 `FOREMAN_RUN_DIR` 改。

## 命令

| 命令 | 谁用 | 作用 |
|---|---|---|
| `start <task> --cd <dir> [--brief f]` | Claude | 新任务；不带 `--brief` 时从 stdin 读交代书 |
| `send <task> [--message f]` | Claude | 在同一会话里继续（返工、下一关、转达用户决定）；`resume` 是别名 |
| `watch <task>` | Claude | 重新接上事件流（Monitor 到期后用） |
| `status <task>` | Claude | 打印状态 JSON 和最后一条回复 |
| `stop <task>` | Claude | 停止任务并清理整棵进程树 |
| `answer <task> [n]` | Claude | 回答第 n 个提问（省略为最早未答的），内容从 stdin 读 |
| `note <task>` | Claude | 给工作中的执行者留言，不打断它 |
| `ask <task> [--no-wait]` | 执行者 | 提问（运行约定里已教给执行者） |
| `inbox <task>` | 执行者 | 读取新回答和留言 |
| `models [--effort E]` | Claude | 列出候选模型和 `best` 的解析结果 |

## 常用选项

| 选项 | 默认 | 说明 |
|---|---|---|
| `--agent` | `codex` | `dsh` 把任务派给 DSH CLI，`agy` 派给 Antigravity CLI（只在 `start` 时指定，之后的 `send` 沿用）。DSH 的 `--model` 可写 `flash`、`pro` 或完整型号，`--effort` 为 `off`/`low`/`high`/`max`。Antigravity 的 `--model` 默认 `gemini-pro-orchestrator`（Orchestrator，固定型号，没有推理档位，底栏和 ▶ 行不显示档位），也可写系列关键字 `hydrogen` / `argon` / `carbon`（用 `agy models` 解析该系列最新版）或完整型号；`--effort` 为 `low`/`medium`/`high`/`xhigh`，默认 `high`，只配合系列关键字使用，配完整型号或默认 Orchestrator 会直接报错；`send` 只改 `--effort` 时沿用当前系列；型号写错时 agy 直接拒绝，任务判失败、不重试；型号名含 `orchestrator` 时运行约定允许它派子代理（子代理先读原始交代、每步往 progress.log 记一行，因为子代理干活时主事件流是静默的），其他型号仍禁止；找不到模型或额度用完时直接失败，不重试、不换模型 |
| `--model` / `--effort` | `sol` / `high` | Codex 默认 = 主力 sol 系列里版本最新的可用型号 + 高档推理。系列关键字 `astra`（前沿）、`sol`（主力）、`terra`（均衡）、`luna`（快速）都按「该系列最新版本」解析；`best` = 跨系列最高档（astra > sol > terra > luna，再比版本），不按目录排序。操作电脑界面的部分（computer use，含 Blender 建模）不换主模型，由脚本在运行约定里要求主 Codex 用 `spawn_agent` 派 terra 最新版子代理完成（必须带 `fork_turns="none"`，否则模型设置不生效）。`start` 时解析一次，整个任务沿用；出现不认识的系列会 ⚠ 提醒确认；该系列没有可用型号时直接报错，不自动换系列。`codex debug models` 偶尔返回过时的内置目录，脚本会记住 7 天内见过的型号，不会因此降级。显式写型号名则直接用。模型繁忙时脚本会退避重试，持续失败就报告用户 |
| `--idle-min` | 10 | 多久没动静判定卡住，结束本轮并重试 |
| `--warn-min` | 3 | 多久没动静先发 ⏸ 提醒 |
| `--max-min` | 45 | 单轮硬上限 |
| `--retries` / `--backoff` | 3 / `2,5,10` | 卡住或断线后的重试次数和间隔（分钟） |
| `--ask-wait-min` | 10 | 执行者阻塞提问时最长等多久 |
| `--add-dir D` | — | 额外允许写的目录，可重复 |
| `--sandbox` | 关 | 默认不开沙箱（Codex 为 `danger-full-access`，审批策略 `never`）。任务不该碰工作目录外的东西、又想用系统兜底时才加，限制写范围、联网、监听端口、启动浏览器和 `git add` |
| `--network` | 关 | 只在 `--sandbox` 下有意义：沙箱内允许联网 |
| `--full-access` | 开 | 已是默认；对之前受限运行的旧任务 `send` 时用它放开。每轮结束报告工作目录外的新改动 |
| `--no-project-doc` | 关 | 不让 Codex 加载项目的 AGENTS.md（项目规则会让它误以为自己是总指挥时使用；需要的规则直接写进交代书） |
| `--no-watch` | 关 | 启动后不跟踪事件流 |
| `--watch-min` | 29 | 跟踪多久后自动交回（退出码 8，任务继续运行），留在 Monitor 30 分钟上限以内 |

环境变量：`CODEX_BIN`（Codex 可执行文件，默认 PATH 里的 `codex`）、`DSH_BIN`（默认 PATH 里的 `dsh`；找不到时运行约定里不出现 DSH 那一段）、`AGY_BIN`（默认 PATH 里的 `agy`）、`CODEX_SERVICE_TIER`（默认 `default`，普通速度档）、`FOREMAN_RUN_DIR`。

## 事件符号

▶ 一轮开始 · ⏸ 一段时间没动静 · ↺ 恢复活动 · ✖ 卡住被结束 · ⚠ 重试、回复缺决定清单或越界改动 · ⏱ 单轮超时 · ✅ 本轮完成（后附回复原文） · ❌ 失败 · ■ 已停止 · ❓ 提问 · 💬 回答已送达 · ⏳ 提问等待超时 · ✉ 留言已送达

## watch 退出码

0 完成 · 1 用法错误 · 2 失败 · 3 重试用尽 · 4 超时 · 5 已停止 · 6 监督进程意外退出（用 `stop` 清理残留） · 7 执行者正在等回答（最后三行给出问题和回答命令；回答后重新 `watch`） · 8 跟踪满 `--watch-min` 自动交回，任务仍在运行（按最后一行重新 `watch`）。到期时如果有阻塞提问，按 7 返回。不阻塞提问不会让 watch 退出。无论用 Monitor 还是普通后台命令等待，阻塞提问都会让命令立即结束，所以不会漏掉。

## 显示

- **码头（Matou）终端**：终端里自带 `MATOU_AGENT_STATUS_DIR` 时，任务状态自动显示在底栏「Agents」里，卡片写明 agent、任务名、模型和推理档位；运行蓝点、等待空心点、警示黄、出错红，结束后消失。码头会覆盖 Claude Code 自己的状态条，所以在码头里只看底栏。
- **当前活动**：运行 / 跑完某条命令（内嵌脚本显示为「Python 脚本」，写文件显示为「写入 <文件名>」）、修改某个文件、`说明：` 执行者自己的阶段说明。Codex 一口气生成长内容时 `codex exec` 不发任何事件；脚本在 macOS 上每 5 秒用 `nettop` 读取 Codex 进程的入站字节，在涨就显示「生成中 N秒 · Codex 最近说的打算」，并算作有活动，不会误判卡住。只有网络也没有数据时才提醒「没有动静（网络也没有数据）」并按 `--idle-min` 结束重试。其他系统上没有这项检测，长时间生成可能被判卡住，可调大 `--idle-min`。
- **普通终端**：在项目的 `.claude/settings.local.json` 里配置 `statusLine`，`command` 为 `node <本技能目录>/scripts/foreman-statusline.mjs`，`refreshInterval` 设为 2。它会先原样输出原有的状态条，再为每个任务加一行。

## 已知坑

- **Monitor 到期不会连带结束任务**：`start`/`send` 经中间进程两级启动监督进程，由系统托管，Monitor 只结束跟踪那一层。万一遇到退出码 6，用 `stop` 清理，再 `send` 让它从中断处继续，工作区改动和会话都保留。
- **监工进程被强杀或崩溃后不会一直显示「运行中」**：下一次 `status`/`watch`/`send`/`resume`/`stop` 发现「状态还在运行、进程却都不在」时，会自动把它改成失败并注明「监工进程意外退出（未留下结束状态）」，`watch` 仍然返回 6。监工自己遇到未捕获异常也会先写下失败状态再退出。磁盘写满这类写盘失败不再把监工带走：状态暂存内存，恢复后自动补写（feed 里会各有一条提示）。
- 脚本会加载你的 Codex 配置（MCP 工具服务、插件、记忆、钩子），让 Codex 和你直接使用时能力一致；连不上的个别服务只在日志里记错误，不影响工作。另外固定加 `--skip-git-repo-check`（工作目录不在 git 仓库也能跑）、`--disable unbounded_connection_retries`（断线直接报错，由监工重试）和 `service_tier`。配置里如果设置了自定义指令文件 `model_instructions_file`，监工拒绝派活，先去掉那一行。
- 加了 `--sandbox` 的任务：`ps` 被拒绝，监听端口报 `listen EPERM`，Chromium/Electron 启动失败，`git add` 报 `.git/index.lock: Operation not permitted`，工作目录下的 `.agents/` 只读。遇到就去掉 `--sandbox`。
- Codex 会清理每条命令的进程组，它用 `nohup … &` 起的常驻服务会被结束。需要常驻的服务由你在外面用后台 Bash 启动，执行者只负责连接。
- 交代书里允许写的范围必须包含所有输出路径，否则执行者会停下来问，白白浪费一轮。工作目录要先建好再派活。
- 工作区里有其他会话的未提交改动时，必须在交代书里写明保留、禁止 stash / reset，并在验收时用 `git diff` 确认这些改动还在。
- 别用 heredoc 里的 `$(cat 文件)` 展开交代书：`<<'EOF'` 带引号时 shell 不展开，脚本收到的就是那一行命令文本（2026-11-15 真实出现过一次，白跑一轮）。要把正文放文件里就用 `--brief <文件>`。脚本现在会拒收「整份内容就是 `$(…)` 或反引号命令」的交代书，退出码 1，不建任务目录，名字可以立刻重发。
- MCP 工具服务在 Codex 启动后陆续接入，远程服务要 10～30 秒才出现在可调用工具里；运行约定已要求执行者看不到时等 30 秒再查。某个服务需要授权时，请用户运行 `codex mcp login <服务名>`；名字只差大小写的重复服务会互相冲突，保留插件自带的那份。
- 并行：多个任务可以同时运行，各有独立的监工进程和运行记录；码头底栏和状态条会逐条列出。同一仓库里两个任务写到同一批文件会互相覆盖，用不重叠的 `--cd`、`git worktree` 或改为串行避免；Codex 并行超过 3 个时更容易出现限流（429 / at capacity）。
- 执行者看不到你看到的图片和界面。审美判断要转成具体数值，并要求它交出截图或样片给你看。
- 要跑测试的交代书写明解释器的绝对路径（例如 Python），执行者的 PATH 可能和你 shell 里的不一样。
- 没有 git 的目录也能派活，但审查包没法用 `git diff` 生成：改动前让执行者把要改的文件备份到工作区，验收时用备份做 diff。
- Antigravity（`--agent agy`）：以 agy 的 result 事件为准，它给出结果后偶尔挂着不退出，10 秒后会被结束。它容易把「当前目录」理解成 `--add-dir` 的目录，运行约定已写明工作目录的绝对路径，交代书里也尽量写绝对路径。额度用完时报「Antigravity 额度用完（型号）」并失败，不会重试；agy 的 `ask_question` 工具在这里没人回答，运行约定已要求它改用 `ask` 命令。它写完回复后偶尔多吐出一个空的工具调用，agy 会把这一轮标成 ERROR（`Malformed function call`）：有回复时脚本收下回复按完成处理，并发一行 ⚠ 提醒「没做完就用 send 接着做」；回复为空时按繁忙重试。其他出错的轮次如果写了回复，❌ 那条也会附上它。
- DSH 无头模式报 `MISSING_CREDENTIAL`：它的 headless 配置默认关掉了凭据插件，在 `~/.dsh/profiles/headless/` 的用户补丁里把 `credentials` 插件打开。
