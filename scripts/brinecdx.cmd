@echo off
setlocal

powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0brinecdx.ps1" %*
exit /b %ERRORLEVEL%
