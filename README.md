# AutoOwl

[GitHub](https://github.com/ouraihub/AutoOwl) · Rust + Tauri 2 + Svelte 5 的桌面音乐应用：12 个平台聚合搜索、在线播放、批量下载与标签写入，并可让 AI 帮你搜歌、放歌。

## 让 AI 帮你搜歌、放歌

两种方式，共用同一套工具：`search_songs`、`play_song`、`pause`、`resume`、`next`、`previous`、`player_status`。

**应用内「AI 对话」**：在「AI 接入」页的模型服务里添加服务商、填 API Key 并启用一个模型，然后在「AI 对话」里说"放一首千千阙歌"。工具直接在进程内执行，不需要启动 MCP 服务。设计见 `docs/AI对话设计.md`。

**外部 AI（MCP）**：本机 MCP 服务基于 [Aether](https://github.com/ouraihub/Aether) 的 `aether-mcp-server`，只监听 `127.0.0.1`，需令牌：

1. 打开「AI 接入」页：启动服务，勾选权限（搜索 / 播放控制），创建令牌。
2. 页面上直接复制 Claude Code 命令或 `mcpServers` JSON，粘贴到 AI 客户端。
3. 对 AI 说"放一首千千阙歌"：它会调 `search_songs` 再调 `play_song`，歌在应用里播放。

命令行（脚本或偏好 shell 的 AI 用）：

```bat
set AUTOOWL_TOKEN=autoowl_...
autoowl-cli search 千千阙歌
autoowl-cli play 0
autoowl-cli status
```

日志：数据目录 `logs/autoowl.log`，「日志」页可查看和调级别；`RUST_LOG` 环境变量优先。

Aether 按提交固定（与 AutoNote 相同做法）：`src-tauri/Cargo.toml` 里所有 Aether crate 的 `rev`，与
`package.json` 中 `@ouraihub/aether-ui`、`@ouraihub/aether-presets` 的 `#<commit>` 必须同步升级。构建不需要本地 Aether 目录。

## 开发

设计文档：`docs/概要设计.md`、`docs/详细设计.md`、`docs/AI对话设计.md`；规范见 `docs/工程规范与质量门禁.md`。

构建、清理和 MSVC 环境由 [desktop-kit](https://github.com/ouraihub/desktop-kit) 提供。Windows 首次使用：`pnpm install`，再运行
`node_modules\@ouraihub\desktop-kit\msvc\setup-msvc.bat` 下载便携 MSVC（默认 `D:\Applications\msvc-portable\msvc`，可用
`PORTABLE_MSVC_ROOT` 修改）。Rust 工具链由 `src-tauri/rust-toolchain.toml` 固定为 1.92.0 MSVC，本机默认是 GNU 也不受影响。

```powershell
pnpm verify        # 全部门禁：check、lint、test、cargo fmt/clippy/test、pnpm build
pnpm dev:app       # tauri dev
pnpm build:app     # 门禁 + 安装包和便携 exe → dist\candidate-windows\，并更新 dist\AutoOwl\autoowl.exe
pnpm clean         # 删除编译产物（保留 data\）；--deep 连 node_modules 一起删
pnpm exec desktop-kit cargo <参数>   # 其他 cargo 命令，自动带 MSVC 和 manifest
```

## 许可证

[AGPL-3.0](LICENSE)。音源协议的实现参考了 [guohuiyuan/music-lib](https://github.com/guohuiyuan/music-lib)（AGPL-3.0）。
