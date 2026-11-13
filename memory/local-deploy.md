# 本机部署 Ade

## 环境事实（2026-11-13 核实）

- 本机已装好 `lld`、`llvm-objcopy`、`musl-gcc` 与 `x86_64-unknown-linux-musl` target。本机安装与发布打包已分开，入口见根 [AGENTS.md](../AGENTS.md#build-guidelines) 与 [README.md](../README.md#building)；旧的“三份二进制 + 许可证扫描 + 压缩包”安装耗时和缓存路径不再适用。
- 安装后的布局（由 `script/install.sh` 决定）：`~/.local/ade.app/{bin/ade,libexec/ade-editor,share/icons/hicolor/{512x512,1024x1024}/apps/ade.png,share/applications/me.farshed.Ade.desktop}`；发布包另带 `lib/`，本机安装直接使用系统库。`~/.local/bin/ade` 软链到 `~/.local/ade.app/bin/ade`，桌面项 `~/.local/share/applications/me.farshed.Ade.desktop`（`Name=Ade`，不带 `-Dev` 后缀，见 PRODUCT-001）。
- release 构建会内嵌资源（`assets`、`settings`），可脱离 checkout 运行；**debug 构建必须在仓库 checkout 内启动**——`util::fs_embed!` 在 `debug_assertions` 打开时从磁盘读资源，`dev_repo_root()` 找不到 `.git` 就 panic（"dev asset loading requires running from within the checkout"）。把 `target/debug/ade` 拷到 `~/.local` 之类的地方再启动会崩。
- 应用图标 PNG 来自 `crates/ade/resources/app-icon{,-dev}{,@2x}.png`；改完 SVG 标识要重新渲染 PNG 并重装，否则桌面仍是旧图标。渲染用 `magick`（带 RSVG 2.62），比 ImageMagick 内置 SVG 渲染器可靠。

## 仓库与远端

- `origin` = `git@github.com:luokyme/ade.git`（自己的 fork，push 用它）；`upstream` = `https://github.com/dstrml/wu.git`（只读参考，已无写权限）。仓库与站点的归属决策见 [decisions/product-name.md](../decisions/product-name.md) 的 PRODUCT-002。
