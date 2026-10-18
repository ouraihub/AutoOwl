@echo off
rem Build the Tauri desktop executable with the MSVC environment loaded.
set "MSVC_ROOT=%AUTOOWL_MSVC%"
if "%MSVC_ROOT%"=="" set "MSVC_ROOT=D:\Applications\msvc-portable\msvc"

rem The default rustup toolchain is GNU, which lacks dlltool.exe; Tauri needs MSVC.
if "%RUSTUP_TOOLCHAIN%"=="" set "RUSTUP_TOOLCHAIN=1.92.0-x86_64-pc-windows-msvc"

where /q cl.exe
if errorlevel 1 (
  if not exist "%MSVC_ROOT%\setup_x64.bat" (
    echo ERROR: MSVC toolchain not found at "%MSVC_ROOT%\setup_x64.bat" 1>&2
    exit /b 1
  )
  call "%MSVC_ROOT%\setup_x64.bat" >nul
)

where /q cl.exe || (
  echo ERROR: cl.exe was not found after MSVC environment setup. 1>&2
  exit /b 1
)

pnpm tauri build %*
exit /b %ERRORLEVEL%
