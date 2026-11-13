# 产品名称决策

本文件适用于项目对外名称及其衍生命名空间，编号前缀 `PRODUCT`。

### PRODUCT-001 产品名为 Ade

- 状态：有效
- 决策：项目对外名称统一为 Ade；crate、二进制、动作命名空间、URL scheme、环境变量前缀、配置与数据目录、项目级目录一律使用 ade 命名（`crates/ade`、`crates/ade_actions`、`crates/ade_env_vars`、`ade::`、`ade://`、`ADE_*`、`~/.config/ade`、`.ade/`）。
- 理由：用户要求把项目从 Wu 改名为 Ade，并重新挑选 logo；旧名 "Wu" 取自人名，与新定位不符。
- 依据：用户 2026-11-13 明确要求"本项目改名为ade, 并挑选个合适的logo"。
- 关联：`crates/ade`、`crates/ade_actions`、`crates/ade_env_vars`、`assets/images/ade_logo.svg`、`assets/icons/ade_mark.svg`、`crates/ade/resources/app-icon*.png`。
- 状态（2026-11-13 核对）：仓库与站点归属见 PRODUCT-002；原 `dstrml/wu` 不再改名（无权限），所有对外链接已迁到 `luokyme/ade` 与 `https://luokyme.github.io/ade`。

### PRODUCT-002 仓库与站点归属 luokyme

- 状态：有效
- 决策：项目的对外仓库、release 下载源、文档站点与全部外链统一指向 luokyme 名下：仓库 `luokyme/ade`，站点 `https://luokyme.github.io/ade`；代码、脚本、README、桌面/flatpak/snap/Windows 打包资源中不得再出现 `dstrml.org` 域名或 `dstrml/ade`、`dstrml/wu` 仓库地址。
- 理由：项目已从 dstrml fork，用户不再拥有原仓库与域名的权限，继续引用会让安装脚本、自动更新检查与文档链接指向无权访问的位置。
- 依据：用户 2026-11-13 明确要求“dstrml.org 肯定得全部改掉，我是 fork 出来的，现在没有原来那些权限”。
- 关联：`website/src/consts.ts`、`script/install.sh`、`crates/auto_update/src/auto_update.rs`、`crates/ade/src/ade.rs`。
