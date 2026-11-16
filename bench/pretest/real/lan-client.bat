@echo off
rem Client side of the real-LAN test, for Windows.
rem Usage: lan-client.bat <address of the machine running lan-server.sh>
rem Keep i3t-pretest.exe and lan-tests.txt in the same folder as this file.
setlocal EnableDelayedExpansion

if "%~1"=="" (
  echo usage: lan-client.bat ^<address of the server machine^>
  exit /b 1
)
set "S=%~1"
set "EXE=%~dp0i3t-pretest.exe"
set "TESTS=%~dp0lan-tests.txt"
set "OUT=%~dp0lan-results.jsonl"
set "ERR=%~dp0lan-errors.txt"

> "%OUT%" echo # server=%S% started=%DATE% %TIME%
> "%ERR%" echo # stderr of each test

for /f "usebackq eol=# tokens=1,*" %%a in ("%TESTS%") do (
  set "LABEL=%%a"
  set "ARGS=%%b"
  set "ARGS=!ARGS:{S}=%S%!"
  echo [!LABEL!]
  >> "%OUT%" echo # !LABEL!
  >> "%ERR%" echo # !LABEL!
  "%EXE%" !ARGS! >> "%OUT%" 2>> "%ERR%"
  if errorlevel 1 echo     FAILED, see lan-errors.txt
)

echo.
echo Done. Please send back lan-results.jsonl and lan-errors.txt from this folder.
endlocal
