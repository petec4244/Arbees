@echo off
REM Wrapper for follow_logs.ps1
REM Usage: follow_logs [core|exec|all|monitors]

setlocal

set "SCRIPT_DIR=%~dp0"
set "ARG="

if /i "%1"=="core" set "ARG=-Core"
if /i "%1"=="c" set "ARG=-Core"
if /i "%1"=="exec" set "ARG=-WithExec"
if /i "%1"=="e" set "ARG=-WithExec"
if /i "%1"=="all" set "ARG=-All"
if /i "%1"=="a" set "ARG=-All"
if /i "%1"=="monitors" set "ARG=-Monitors"
if /i "%1"=="m" set "ARG=-Monitors"

powershell -ExecutionPolicy Bypass -File "%SCRIPT_DIR%follow_logs.ps1" %ARG%
