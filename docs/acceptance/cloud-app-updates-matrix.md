# Matou 云端更新交互对照矩阵

## 对照边界

该能力属于 Matou 应用级发布与恢复，不属于 Kooky CLI 黑色工作区。因此 Kooky 同范围基线标记为“不适用”，以用户确认的 HTML Mockup 为可见交互基线，并保持 Matou 原有窗口标题与应用级导航。

## 场景矩阵

| 场景 | Mockup 基线 | Matou 实际结果 | 运行证据 | 差异结论 |
| --- | --- | --- | --- | --- |
| 发现新版本 | 右上角紧凑入口，自动展开版本、大小、日期和摘要；由用户启动下载 | 350px 浮层锚定顶栏右侧，终端仍可见可输入，未自动下载 | `evidence/app-updates/matou/available.png` | 闭合 |
| 后台下载 | 入口显示环形进度，浮层显示百分比、已下载/总量、速度和剩余时间 | 进度事件实时刷新；关闭浮层只收起界面，主进程下载继续 | `evidence/app-updates/matou/downloading.png` | 闭合 |
| 下载完成，会话空闲 | 主操作为“重启并更新”，次操作为“退出时安装” | 操作层级与文案一致；重启前等待 Runtime 持久化并关闭 | `evidence/app-updates/matou/downloaded-idle.png` | 闭合 |
| 下载完成，存在活动会话 | 明确显示活动数，默认等待空闲，仍保留立即更新和退出安装 | 未归档的 `starting/running/needs-input` 唯一会话计数；空闲阈值到 0 时仅安装一次，排队期可取消 | `AppUpdateControl.test.tsx` + `active-app-sessions.test.ts` | 闭合（自动化交互证据） |
| 检查或下载异常 | 保留当前版本，给出可理解说明和重试入口 | 网络类异常转为用户文案，“重新检查”不离开当前工作区 | `evidence/app-updates/matou/error.png` | 闭合 |
| 更新后首次启动 | 只显示一次更新成功轻提示 | 对比本地记录版本和当前版本，5 秒后自动收起，同版本不再显示 | `AppUpdateControl.test.tsx` | 闭合 |

## 几何与可用性结果

- 实际浮层：350px 宽、16px 圆角、右边距 8px，紧贴 48px 应用顶栏下方。
- 浮层为非模态；终端画面保持可见，Escape 和外部点击均可收起。
- 屏幕阅读器可读取入口状态、下载百分比、浮层名称和关闭按钮。
- `prefers-reduced-motion` 下浮层和轻提示去除位移/缩放运动。
