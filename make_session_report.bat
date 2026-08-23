@echo off
setlocal
cd /d "%~dp0"
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0make_session_report.ps1"
if errorlevel 1 (
    echo.
    echo A report generalasa sikertelen.
)
echo.
pause
