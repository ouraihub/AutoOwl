@echo off
rem Run cargo after injecting the Windows MSVC environment.
rem Set AUTOOWL_MSVC to the folder containing setup_x64.bat when needed.

set "MSVC_ROOT=%AUTOOWL_MSVC%"
if "%MSVC_ROOT%"=="" set "MSVC_ROOT=D:\Applications\msvc-portable\msvc"

rem The rustup default here is the GNU toolchain, whose linker needs dlltool.exe.
rem Pin MSVC so `cargo` matches the cl.exe environment loaded below.
if "%RUSTUP_TOOLCHAIN%"=="" set "RUSTUP_TOOLCHAIN=1.92.0-x86_64-pc-windows-msvc"

where /q cl.exe
if not errorlevel 1 goto run

if not exist "%MSVC_ROOT%\setup_x64.bat" (
  echo ERROR: MSVC toolchain not found at "%MSVC_ROOT%\setup_x64.bat" 1>&2
  echo Set AUTOOWL_MSVC to the folder containing setup_x64.bat. 1>&2
  exit /b 1
)

call "%MSVC_ROOT%\setup_x64.bat" >nul
where /q cl.exe
if errorlevel 1 (
  echo ERROR: cl.exe was not found after MSVC environment setup. 1>&2
  exit /b 1
)

:run
set "MANIFEST=%~dp0..\src-tauri\Cargo.toml"
if "%1"=="" (
  echo Usage: scripts\cargo-msvc.bat check^|test^|clippy^|build [cargo args]
  exit /b 2
)
set "SUBCOMMAND=%1"
shift
set "ARGS="
:collect
if "%1"=="" goto dispatch
set "ARGS=%ARGS% %1"
shift
goto collect

:dispatch
cargo %SUBCOMMAND% --manifest-path "%MANIFEST%"%ARGS%
exit /b %ERRORLEVEL%
