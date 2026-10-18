# Windows BAT 脚本使用说明

以下命令默认在项目根目录执行：

```powershell
cd <项目根目录>
```

## 使用前准备

1. 安装 Node.js、pnpm、Rust `x86_64-pc-windows-msvc`。
2. 准备 MSVC C++ 编译环境，其中必须包含 `cl.exe` 和微软的 `link.exe`。
3. 首次使用先安装前端依赖：

```powershell
pnpm install
```

脚本默认从下面的位置加载便携版 MSVC：

```text
D:\Applications\msvc-portable\msvc\setup_x64.bat
```

如果 MSVC 位于其他目录，请在当前 PowerShell 会话中设置：

```powershell
$env:AUTOOWL_MSVC = 'D:\你的目录\msvc'
```

该目录下必须存在 `setup_x64.bat`。脚本使用 `cl.exe` 判断 MSVC 是否可用，避免错误调用 Git 自带的 `link.exe`。

## 直接生成发布 EXE

```powershell
scripts\build-app.bat
```

该脚本会依次检查 Tauri CLI、加载 MSVC、执行 release 构建，并将最终文件复制到：

```text
dist\autoowl\autoowl.exe
```

这是日常发布便携版程序时应使用的脚本。它使用 `--no-bundle`，只生成可直接运行的 EXE，不生成 NSIS 或 MSI 安装包。

## 启动开发环境

```powershell
scripts\tauri-dev.bat
```

该脚本加载 MSVC 环境后运行 `pnpm tauri dev`，会启动 Vite 开发服务和 Tauri 桌面窗口。退出开发时关闭桌面窗口并终止命令行中的开发进程。

## 执行 Tauri 构建

`tauri-build.bat` 是 Tauri CLI 的通用构建包装器，会把后续参数原样传给 `tauri build`。

Debug EXE，不生成安装包：

```powershell
scripts\tauri-build.bat --debug --no-bundle
```

输出位置：

```text
src-tauri\target\debug\autoowl.exe
```

Release 构建并按 `tauri.conf.json` 生成 bundle：

```powershell
scripts\tauri-build.bat
```

普通用户需要单个发布 EXE 时，应优先使用 `build-app.bat`。

## 执行 Rust 命令

`cargo-msvc.bat` 会先加载 MSVC，再把命令转发给当前项目的 Cargo manifest。

```powershell
scripts\cargo-msvc.bat check
scripts\cargo-msvc.bat test
scripts\cargo-msvc.bat fmt -- --check
scripts\cargo-msvc.bat clippy --all-targets -- -D warnings
scripts\cargo-msvc.bat build --release
```

不要在普通终端直接执行 Cargo 构建后再根据 Git `link.exe` 的报错排查代码；Windows 下统一使用这个包装脚本。

## 清理构建产物

交互确认后清理：

```powershell
scripts\clean.bat
```

跳过确认：

```powershell
scripts\clean.bat --yes
```

同时删除 `node_modules`：

```powershell
scripts\clean.bat --deep --yes
```

默认清理以下目录：

- `build`
- `.svelte-kit`
- `dist`
- `src-tauri\target`

`--deep` 额外删除 `node_modules`，之后必须重新执行 `pnpm install`。如果 `autoowl.exe`、`cargo.exe` 或 `rustc.exe` 正在运行，脚本会拒绝清理，需先关闭相关程序。

## 推荐工作流

开发和验证：

```powershell
pnpm verify
scripts\tauri-dev.bat
```

生成交付 EXE：

```powershell
pnpm verify
scripts\build-app.bat
```

重新进行干净构建：

```powershell
scripts\clean.bat --yes
pnpm install
pnpm verify
scripts\build-app.bat
```

## 常见错误

### `link: extra operand` 或调用 Git 的 `link.exe`

说明 Cargo 没有进入 MSVC 环境。请使用 `cargo-msvc.bat`、`tauri-dev.bat` 或 `build-app.bat`，并检查：

```powershell
Test-Path "$env:AUTOOWL_MSVC\setup_x64.bat"
```

### `error calling dlltool 'dlltool.exe': program not found`

Cargo 用了本机默认的 GNU 工具链（`rustup default` 为 `stable-x86_64-pc-windows-gnu`）。所有脚本都会在
未设置 `RUSTUP_TOOLCHAIN` 时固定为 `1.92.0-x86_64-pc-windows-msvc`；若仍报错，先安装它：

```powershell
rustup toolchain install 1.92.0-x86_64-pc-windows-msvc
```

### `Tauri CLI is unavailable`

在项目根目录重新安装依赖：

```powershell
pnpm install
pnpm tauri --version
```

### 清理脚本提示程序正在运行

关闭 autoowl 窗口和正在执行的 Cargo/Rust 命令后重试。清理脚本不会自动终止这些进程，以免误杀其他正在进行的构建。
