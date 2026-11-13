# 产品名称决策

本文件适用于项目对外名称及其衍生命名空间，编号前缀 `PRODUCT`。

### PRODUCT-001 产品名为 Ade

- 状态：有效
- 决策：项目对外名称统一为 Ade；crate、二进制、动作命名空间、URL scheme、环境变量前缀、配置与数据目录、项目级目录一律使用 ade 命名（`crates/ade`、`crates/ade_actions`、`crates/ade_env_vars`、`ade::`、`ade://`、`ADE_*`、`~/.config/ade`、`.ade/`）。
- 理由：用户要求把项目从 Wu 改名为 Ade，并重新挑选 logo；旧名 "Wu" 取自人名，与新定位不符。
- 依据：用户 2026-11-13 明确要求"本项目改名为ade, 并挑选个合适的logo"。
- 关联：`crates/ade`、`crates/ade_actions`、`crates/ade_env_vars`、`assets/images/ade_logo.svg`、`assets/icons/ade_mark.svg`、`crates/ade/resources/app-icon*.png`。
- 状态（2026-11-13 核对）：域名已切换为 `ade.dstrml.org`，代码、README、website（`website/src/consts.ts`）中的 `REPO_URL` 均已指向 `dstrml/ade`；GitHub 上仓库仍名为 `dstrml/wu`（属组织 `dstrml`），需组织管理员执行改名（`PATCH https://api.github.com/repos/dstrml/wu -d '{"name":"ade"}'` 或组织设置页改名），改名后本地 `origin` 更新为 `git@github.com:dstrml/ade.git`。在改名完成前 `script/install.sh` 的 release 下载地址不可用。
- 补充：个人 fork 仓库为 `git@github.com:luokyme/ade.git`，已推送完整历史。
