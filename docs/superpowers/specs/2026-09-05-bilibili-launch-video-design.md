# 码头 B 站发布视频：全自动制作流水线设计

日期：2026-09-05。状态：已与作者确认。

## 目标

不需要作者出镜或配音，从仓库出发一键产出一条约 5 分钟的 B 站发布视频（1080p，另有 60 秒短版），
替换进 B 站草稿「开了12个Claude Code之后，我被逼着写了这个开源工具」后发布。

## 已确认的决定

| 项 | 决定 |
|---|---|
| 形式 | 第一人称「我」，合成配音，真实码头 App 被 Playwright 驱动，点击处自动推近，字幕烧入 |
| 时长 | 主片约 5 分 10 秒（说辞 1484 字，-8% 语速配音 302 秒 + 每段 0.8 秒尾；演示动作全部发生在配音之内，不额外增加时长） |
| 配音 | edge-tts `zh-CN-YunxiNeural`，`--rate=-8%`；不满意再升级 MiniMax |
| 演示数据 | 全部构造数据，复用 `tests/e2e/readme-capture` 的隔离环境和 Claude 替身；片尾声明 |
| 不提 | Codex（当前不支持）；不点名批评其他终端工具，先肯定再转折 |
| 录制显示器 | 副屏（内建 Retina 屏，3024×1964 物理像素，缩放 2 倍）整屏可用 |
| 画幅 | 窗口 1512×850 CSS 像素（16:9），采集 3024×1700 物理像素；成片 1920×1080 等比缩小，推近 1.5 倍内无损 |
| 剪辑引擎 | Remotion 4（代码即时间轴），独立工程 `marketing/launch-video/`，不进 pnpm 工作区 |
| 采集方式 | Electron `webContents.beginFrameSubscription` 取合成帧 → 主进程按 30fps 恒定帧率写入 ffmpeg 管道（rawvideo bgra → h264 yuv420p）；不依赖 macOS 屏幕录制权限 |
| 事件日志 | 录制时记录每次点击、鼠标移动目标、章节标记（相对录制起点的毫秒）；Remotion 据此生成推近镜头和光标 |
| 节奏对齐 | 录制脚本读取每段配音的字幕 cue，等到说辞念到关键词时才执行对应操作 |

## 说辞（v2，已确认）

见 `marketing/launch-video/script/narration.json`，与 `~/Downloads/matou-tts-samples/说辞v2.txt` 一致。
十段：intro、why、structure、focus、persist、fork-dag、ai-control、board-notify、model-switch、outro。

## 分镜（每段画面）

| 段 | 画面 |
|---|---|
| intro | Remotion 合成：十几个终端窗口层叠乱切，最后停在红色报错；不进 App |
| why | 码头主界面全景缓慢推近；不点名批评其他终端工具，先肯定再转折，画面只有字幕不放任何第三方 logo |
| structure | 依次点击工作空间、事项、画布页签、卡片，镜头随点击推近；最后推到底部 HUD 逐项停留 |
| focus | 五张卡片并排，依次点击，焦点展开其余收窄 |
| persist | 退出 App 再启动（两段录制拼接），一切原位；新建卡片，打开「载入 Claude Code 会话」对话框，左列表右预览，载入 |
| fork-dag | Claude 给出三方案；点 Fork 生成子卡片；打开 DAG（方案 A 运行、方案 B 等待、回归退出码 1）；点节点跳转；返回父会话；蓝框闪烁 |
| ai-control | 右卡片输入自然语言，Claude 替身真实执行 `mt read left`；再输入一句，替身真实执行 `mt fork children`，三张子卡片出现，DAG 长出三条线 |
| board-notify | 看板拖拽事项到阻塞；通知中心分级列表，点一条滑到现场 |
| model-switch | 打开设置里的模型切换，新增供应商 DeepSeek，点切换 |
| outro | Remotion 合成：文字卡（早期预览 / 仅 Apple Silicon / 未签名 / 构造数据）、GitHub 地址、QQ 群 |

## 不做的事

- 不录真人声音，不出镜。
- 不接真实 Claude Code（避免 token 消耗与不可控输出）。
- 不用 OpenCut 或 ffmpeg 滤镜图做效果。
- 不在本流水线内做 B 站上传，最后一步由浏览器自动化替换草稿视频。

## 验收

1. `cd marketing/launch-video && npm run render` 产出 `out/matou-launch-1080p.mp4`，时长 5:00 到 5:30。
   （录制脚本按字幕 cue 等待，所有演示动作都发生在配音之内，成片 ≈ 配音总长 + 每段尾巴，不会再被动作拉长。）
2. 字幕与配音对齐误差小于 200ms（抽查三处）。
3. 每段的关键操作出现在说辞提到它的 1.2 秒内（抽查 fork、DAG、mt read left）。
   （`mt-read` 标记落在 Claude 替身真实执行完 `mt read` 之后，比纯 UI 点击多出命令自身的耗时。）
4. 逐帧检查无黑帧、无窗口尺寸跳变、光标位置与点击位置一致。
5. 60 秒短版 `out/matou-launch-60s.mp4` 产出。
