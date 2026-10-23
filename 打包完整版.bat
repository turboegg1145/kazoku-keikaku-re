@echo off
chcp 65001 >nul
setlocal
if "%~1"=="" (
    echo.
    echo   把游戏文件夹拖到这个 bat 上
    echo   ^(里面有 KIZUNAR.EXE 的那个文件夹^)
    echo.
    pause
    exit /b 1
)
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0tools\pack-ipa.ps1" -GameDir "%~1"
echo.
pause
