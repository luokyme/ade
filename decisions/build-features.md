# 构建功能决策

本文件适用于 Ade 默认构建包含哪些可选功能，编号前缀 `BUILD`。

### BUILD-001 开发容器功能默认不编译

- 状态：有效
- 决策：开发容器（Dev Container）功能默认不进入构建。`dev_container` crate 及其界面入口（打开开发容器的命令、远程项目里的“连接开发容器”、打开项目时的开发容器提示）只在启用 `ade` 的 `dev-container` 特性（转发到 `recent_projects/dev-container`）时编译，`./script/install-linux` 的默认构建不含它。`remote`、`workspace`、`settings` 里的 Docker 连接数据类型不受此开关约束。从上游同步开发容器相关改动时，新增代码同样放进该开关。
- 理由：开发容器是上游新增的大块功能（约两万行），用户当前不需要，默认编译会增加构建时间和体积。
- 否决：默认编译开发容器功能；把 Docker 连接类型也纳入开关（牵涉多个 crate 的连接枚举，改动面大而收益小）。
- 依据：用户 2026-11-27 明确要求“默认不要编译container相关功能”。
- 关联：`crates/ade/Cargo.toml`、`crates/recent_projects/Cargo.toml`、`crates/recent_projects/src/remote_servers.rs`、`crates/recent_projects/src/recent_projects.rs`、`crates/ade/src/main.rs`。
