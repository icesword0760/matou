# 正常静置重启补测

原最终包 app.asar SHA-256 `69e4cc57bf526447b0e697a5df184f0364ff68d5b11418da9cfdc4761962d597`。单次执行通过，退出码 0，整次 60.0 秒，无额外重试。

会话 UUID `01a1df3f-8840-78a0-b954-9af370ab98db`；重启静置 10003 ms，数据库 work_status 为 starting，DAG 节点保持 status-starting（共享可见文案为“运行中”），界面没有“已恢复”；静置时恢复横幅为 null。随后以不含暗号的问题发起新 turn `01a1df40-0b65-79c3-8130-f47a6f4c4f50`，同 UUID 的 assistant final_answer 与 task_complete 均为 `cedar-brook-573`。结构化证据 `closure-restart-static.json`、`closure-restart.json`；截图在本目录的上一级。

这是调整测试以符合已确认的启动中过渡显示后的独立补测；原先横幅断言失败的现场保留在 `closure/codex/`。没有改产品代码或执行 #5。
