@echo off
setlocal
cd /d "%~dp0"

echo.
echo ==========================================
echo        ACC TELEMETRY SESSION REPORT
echo ==========================================
echo.

echo Creating report from latest telemetry...
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0make_report.ps1"

if errorlevel 1 (
  echo.
  echo A report generalasa sikertelen.
  pause
  exit /b 1
)

echo.
echo KESZ.
pause
