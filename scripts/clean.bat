@echo off
setlocal

rem Remove repository build artifacts. Use --deep to remove node_modules too.
for %%R in ("%~dp0..") do set "ROOT=%%~fR\"
set "DEEP="
set "YES="
for %%F in (%*) do (
  if /i "%%~F"=="--deep" set "DEEP=1"
  if /i "%%~F"=="-d" set "DEEP=1"
  if /i "%%~F"=="--yes" set "YES=1"
  if /i "%%~F"=="-y" set "YES=1"
  if /i "%%~F"=="--help" goto usage
  if /i "%%~F"=="-h" goto usage
)

tasklist 2>nul | findstr /i "autoowl.exe cargo.exe rustc.exe" >nul
if not errorlevel 1 goto running

echo Will remove build, .svelte-kit, dist, and src-tauri\target.
if defined DEEP echo Will also remove node_modules.
if defined YES goto confirmed
choice /c YN /n /m "Proceed? [Y/N] "
if errorlevel 2 exit /b 1

:confirmed
call :remove "%ROOT%build"
call :remove "%ROOT%.svelte-kit"
call :remove "%ROOT%dist"
call :remove "%ROOT%src-tauri\target"
if defined DEEP call :remove "%ROOT%node_modules"
echo Done.
exit /b 0

:remove
if not exist "%~1" goto :eof
echo Removing %~1
rd /s /q "%~1"
if exist "%~1" (
  echo ERROR: could not remove "%~1". 1>&2
  exit /b 1
)
goto :eof

:running
echo ERROR: autoowl or Rust build processes are running. Stop them before cleaning. 1>&2
tasklist 2>nul | findstr /i "autoowl.exe cargo.exe rustc.exe"
exit /b 1

:usage
echo Usage: scripts\clean.bat [--deep] [--yes]
echo   --deep  Also remove node_modules.
echo   --yes   Skip confirmation.
exit /b 0
