# matou-launch-video

码头 B 站发布视频的独立 Remotion 工程。**不属于** pnpm workspace，本目录下一律用 `npm`，不要在仓库根目录用 `pnpm add`。

## 前置条件

- Node v22+
- `edge-tts`：`uv tool install edge-tts`（默认读取 `~/.local/bin/edge-tts`，可用 `EDGE_TTS` 环境变量覆盖路径）
- `ffmpeg`（含 `ffprobe`，需在 `PATH` 上）
- 首次使用先 `npm install`（会装 Remotion，耗时数分钟）

## 常用命令

- `npm run tts`：读取 `script/narration.json`，用 edge-tts 逐段合成配音，产出 `public/audio/<id>.mp3`、`<id>.cues.json` 和 `manifest.json`
- `npm run record`：录制页面素材（Playwright，见 `tests/e2e/launch-video/record`）
- `npm run studio`：本地预览 Remotion 工程
- `npm run render` / `npm run render:short`：渲染正片 / 60 秒短版
- `npm run cover`：导出封面图
- `npm run test`：跑 vitest

## 目录说明

- `script/narration.json`：十段说辞文案（说辞 v2 定稿）
- `tts/synthesize.mjs`：配音合成脚本
- `public/audio/`、`public/recordings/`、`out/`：生成产物，已在 `.gitignore` 中排除，不提交
