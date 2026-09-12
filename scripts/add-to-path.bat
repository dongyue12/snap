@echo off
chcp 936 >nul
setlocal EnableExtensions EnableDelayedExpansion

:: 必须在 shift 之前取：shift 会改写 %0，而 %~dp0 是从 %0 推出来的
set "HERE=%~dp0"

echo ==========================================
echo   把 snap 加入 PATH
echo ==========================================
echo.

:: 参数翻译：/machine /remove /dry-run -> PowerShell 的 -Scope Machine 等
set "ARGS="
:parse
if "%~1"=="" goto run
if /i "%~1"=="/machine" ( set "ARGS=!ARGS! -Scope Machine" & shift & goto parse )
if /i "%~1"=="/remove"  ( set "ARGS=!ARGS! -Remove" & shift & goto parse )
if /i "%~1"=="/dry-run" ( set "ARGS=!ARGS! -DryRun" & shift & goto parse )
if /i "%~1"=="/?" ( goto usage )
set "ARGS=!ARGS! %~1" & shift & goto parse

:run
:: 只有系统级修改才需要管理员权限
echo !ARGS! | findstr /i "Machine" >nul 2>&1
if !errorlevel! equ 0 (
    net session >nul 2>&1
    if !errorlevel! neq 0 (
        echo [错误] 修改系统级 PATH 需要管理员权限。
        echo        请右键本文件，选择"以管理员身份运行"。
        echo.
        pause
        exit /b 1
    )
)

:: %~dp0 自带末尾反斜杠，直接拼脚本名即可
powershell -NoProfile -ExecutionPolicy Bypass -File "!HERE!add-to-path.ps1" !ARGS!
set "RC=!errorlevel!"

echo.
if !RC! neq 0 echo （如果提示权限不足，请右键本文件选择"以管理员身份运行"）
pause
exit /b !RC!

:usage
echo 用法:
echo   add-to-path.bat            加入当前用户的 PATH（不需要管理员）
echo   add-to-path.bat /machine   加入系统 PATH（需要管理员）
echo   add-to-path.bat /remove    从 PATH 移除
echo   add-to-path.bat /dry-run   只显示会做什么，不真正修改
echo.
echo 本脚本要放在 snap.exe 旁边；修改前会自动把原 PATH 备份到 TEMP 目录。
pause
exit /b 0
