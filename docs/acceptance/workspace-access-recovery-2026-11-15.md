# 工作区目录访问恢复（2026-11-15）

## 原因与证据

- SceneCraft 原路径存在，目录权限为 `drwxr-xr-x`，终端执行 `fs.access(R_OK | X_OK)` 成功；运行中的 Matou 将其记录为 `invalid / no-access`。随后 matou、CharInfinity 也出现相同提示。
- 旧主进程 PID 7309 从 13:29 起运行；`lsof -p 7309 -a -d txt` 显示实际映射的程序已位于 `~/.matou/backups/global-provider-fAhTHI/replaced.app/Contents/MacOS/码头`，inode 442864213。安装路径下的新文件 inode 为 443460665。
- macOS tccd 日志记录 `proc_pidpath_audittoken() failed from PID[7309]: (#2) No such file or directory`、`Failed to create attribution chain from message`。因此不是项目目录丢失，而是运行中应用被移动替换后，旧进程的权限归属识别失败。
- 当日安装记录 `evidence/global-provider-follow-2026-11-15/install.json` 所描述的安装策略保留了旧进程（`runningUserAppRestarted: false`）；恢复目录只重新关联项目位置，不能修复旧进程的系统身份。

## 恢复与验收

- 通过应用正常退出，再从 `/Applications/码头.app` 启动；确认旧主进程及 runtime PID 7365 均退出。
- 新主进程 PID 32316 映射安装路径下的程序，inode 443460665，与磁盘文件一致。
- 真实界面：matou、CharInfinity 与 SceneCraft 的路径失效标记消失；进入 SceneCraft → 交互，原会话卡片仍在，目录不可用横幅消失，新建事项、页签和 Shell 入口可用。
- 部分会话显示应用既有的按需恢复入口（启动仅自动恢复最近 16 条）；没有向用户的 AI 会话发送测试消息，没有验收所有历史会话或后台 monitor 的继续运行。
- 本次未修改权限、项目目录、运行时代码或数据库。无需编译与代码回归测试。

## 避免再次触发

本机手工安装不能移动或覆盖仍在运行的应用后，仅声明“下次重启生效”。应先正常退出并确认旧主进程及 runtime 已结束，再备份、替换和启动，最后核对实际加载路径与工作区状态。若暂时不能退出，则暂存新版本，延后替换正在运行的应用。
