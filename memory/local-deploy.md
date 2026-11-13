# 本机部署 Ade

## 环境事实（2026-11-13 核实）

- 交付走 `./script/install-linux`：它会跑 `script/bundle-linux`（release 构建 + 打包成 `target/release/ade-linux-x86_64.tar.gz`）再调 `script/install.sh` 解包安装。本机已装好 `lld`、`llvm-objcopy`、`musl-gcc` 与 `x86_64-unknown-linux-musl` target，脚本可完整跑通。
- 该脚本用 `--target <host-triple>` 构建，产物落在 `target/x86_64-unknown-linux-gnu/`，**不复用** `target/release` 的缓存；整轮（license 扫描 + 三份二进制）约 40 分钟。
- 安装后的布局（由 `script/install.sh` 决定）：`~/.local/ade.app/{bin/ade,libexec/ade-editor,lib/,share/icons/hicolor/{512x512,1024x1024}/apps/ade.png,share/applications/me.farshed.Ade.desktop}`，`~/.local/bin/ade` 软链到 `~/.local/ade.app/bin/ade`，桌面项 `~/.local/share/applications/me.farshed.Ade.desktop`（`Name=Ade`，不带 `-Dev` 后缀，见 PRODUCT-001）。
- release 构建会内嵌资源（`assets`、`settings`），可脱离 checkout 运行；**debug 构建必须在仓库 checkout 内启动**——`util::fs_embed!` 在 `debug_assertions` 打开时从磁盘读资源，`dev_repo_root()` 找不到 `.git` 就 panic（"dev asset loading requires running from within the checkout"）。把 `target/debug/ade` 拷到 `~/.local` 之类的地方再启动会崩。
- 应用图标 PNG 来自 `crates/ade/resources/app-icon{,-dev}{,@2x}.png`；改完 SVG 标识要重新渲染 PNG 并重装，否则桌面仍是旧图标。渲染用 `magick`（带 RSVG 2.62），比 ImageMagick 内置 SVG 渲染器可靠。

## 仓库与远端

- `origin` = `dstrml/wu`（组织 `dstrml`），`luokyme` = `git@github.com:luokyme/ade.git`（个人 fork，已推送完整历史）。上游改名相关事项见 [decisions/product-name.md](../decisions/product-name.md)。
