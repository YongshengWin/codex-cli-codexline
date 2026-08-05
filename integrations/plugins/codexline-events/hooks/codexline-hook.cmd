@echo off
if "%CODEXLINE_HOOK_BIN%"=="" exit /b 0
if "%CODEXLINE_EVENT_ENDPOINT%"=="" exit /b 0
"%CODEXLINE_HOOK_BIN%" hook
