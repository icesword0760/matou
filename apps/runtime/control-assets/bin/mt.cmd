@echo off
if "%MATOU_CONTROL_NODE_EXECUTABLE%"=="" (
  echo mt 仅在 Matou 托管终端中可用：缺少运行入口 1>&2
  exit /b 5
)
set "ELECTRON_RUN_AS_NODE=1"
"%MATOU_CONTROL_NODE_EXECUTABLE%" "%~dp0..\..\mt-cli.cjs" %*
