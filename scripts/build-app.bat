@echo off
setlocal

rem Build one release executable at dist\autoowl\autoowl.exe.
for %%R in ("%~dp0..") do set "ROOT=%%~fR\"
set "MSVC_ROOT=%AUTOOWL_MSVC%"
if "%MSVC_ROOT%"=="" set "MSVC_ROOT=D:\Applications\msvc-portable\msvc"
set "SOURCE=%ROOT%src-tauri\target\release"
set "DIST=%ROOT%dist\autoowl"
rem The default rustup toolchain is GNU, which lacks dlltool.exe; Tauri needs MSVC.
if "%RUSTUP_TOOLCHAIN%"=="" set "RUSTUP_TOOLCHAIN=1.92.0-x86_64-pc-windows-msvc"

echo [1/4] Checking Tauri CLI...
call pnpm --dir "%ROOT%." tauri --version >nul 2>nul
if errorlevel 1 (
  echo ERROR: Tauri CLI is unavailable. Run pnpm install. 1>&2
  goto fail
)

echo [2/4] Loading MSVC environment...
where /q cl.exe
if errorlevel 1 (
  if not exist "%MSVC_ROOT%\setup_x64.bat" (
    echo ERROR: MSVC toolchain not found at "%MSVC_ROOT%\setup_x64.bat" 1>&2
    echo Set AUTOOWL_MSVC to the folder containing setup_x64.bat. 1>&2
    goto fail
  )
  call "%MSVC_ROOT%\setup_x64.bat" >nul
)
where /q cl.exe || (
  echo ERROR: cl.exe was not found after MSVC environment setup. 1>&2
  goto fail
)

echo [3/4] Building release executable...
call pnpm --dir "%ROOT%." tauri build --no-bundle
if errorlevel 1 goto fail
if not exist "%SOURCE%\autoowl.exe" (
  echo ERROR: build succeeded but autoowl.exe is missing. 1>&2
  goto fail
)

echo [4/4] Collecting artifacts...
if not exist "%DIST%" mkdir "%DIST%"
copy /y "%SOURCE%\autoowl.exe" "%DIST%\autoowl.exe" >nul
if errorlevel 1 goto fail

echo Done: %DIST%\autoowl.exe
exit /b 0

:fail
echo ERROR: release build failed. 1>&2
exit /b 1
