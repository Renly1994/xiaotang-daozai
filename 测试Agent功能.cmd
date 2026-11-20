@echo off
setlocal
chcp 65001 >nul
rem 岛仔 Agent 功能自测入口（真机 E2E：真实 DSH 宿主驱动 → 屏幕上真实鼠标点击 → 桥接校验）
rem 前提：桌面已解锁、没有其它岛仔实例在跑（脚本自己会检查，锁屏/有实例时会直接说明原因）
cd /d "%~dp0"

set EXE=target\x86_64-pc-windows-msvc\release\daozai.exe
set E2E=scripts\agent-e2e.ps1
set REPRO=scripts\agent-e2e-repro.ps1

echo ============================================================
echo  岛仔 Agent 功能自测（用当前构建：%EXE%）
echo ============================================================
echo   1. 全流程验收：审批 + 单选提问 + 完成通知
echo   2. 多轮提问：第1/2题 - 下一题 - 检查答案 - 确认提交
echo   3. 多选提问：行高 + 滚动
echo   4. 完成卡「查看回复」（content=0 的本机说明层）
echo   5. 跳转客户端（DSH 在后台）
echo   6. 跳转客户端（DSH 已在前台）
echo   7. 收起态提示卡上点跳转
echo   8. 长回复滚动（content=2，1500+ 字）
echo   0. 退出
echo ============================================================
set /p CHOICE=请输入序号后回车：

if "%CHOICE%"=="1" goto all
if "%CHOICE%"=="2" goto multiturn
if "%CHOICE%"=="3" goto multi
if "%CHOICE%"=="4" goto donerepro
if "%CHOICE%"=="5" goto jumpback
if "%CHOICE%"=="6" goto jumpfront
if "%CHOICE%"=="7" goto bannerjump
if "%CHOICE%"=="8" goto scroll
if "%CHOICE%"=="0" exit /b 0
echo 没有这个选项：%CHOICE%
exit /b 2

:all
powershell -NoProfile -ExecutionPolicy Bypass -File "%E2E%" -Scenario all -Exe "%EXE%"
goto done

:multiturn
powershell -NoProfile -ExecutionPolicy Bypass -File "%E2E%" -Scenario question-multi-turn -Exe "%EXE%"
goto done

:multi
powershell -NoProfile -ExecutionPolicy Bypass -File "%E2E%" -Scenario question-multi -Exe "%EXE%"
goto done

:donerepro
powershell -NoProfile -ExecutionPolicy Bypass -File "%REPRO%" -Scenario done-repro -Exe "%EXE%"
goto done

:jumpback
powershell -NoProfile -ExecutionPolicy Bypass -File "%REPRO%" -Scenario jump-repro -DshBack -Exe "%EXE%"
goto done

:jumpfront
powershell -NoProfile -ExecutionPolicy Bypass -File "%REPRO%" -Scenario jump-repro -DshFront -Exe "%EXE%"
goto done

:bannerjump
powershell -NoProfile -ExecutionPolicy Bypass -File "%REPRO%" -Scenario banner-jump-repro -Exe "%EXE%"
goto done

:scroll
powershell -NoProfile -ExecutionPolicy Bypass -File "%REPRO%" -Scenario scroll-repro -AgentContent 2 -Exe "%EXE%"
goto done

:done
echo.
echo 截图与判定结果在 docs\superpowers\research\agent-e2e-shots\ 下最新时间戳目录（summary.json 的 ok 字段为准）。
echo 按任意键打开该目录...
pause >nul
for /f "delims=" %%D in ('dir /b /ad /o-d "docs\superpowers\research\agent-e2e-shots"') do (
  start "" "docs\superpowers\research\agent-e2e-shots\%%D"
  goto :eof
)
