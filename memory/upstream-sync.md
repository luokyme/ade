# 上游 Wu 同步

## 当前有效状态

- 已同步到上游 `main` 的 `ded7432`（2026-11-26）。其中 Wu 1.5.0（`a3ced28`）是三方合并进来的：Ade 合并提交 `5d6ee22`，第二父提交即 `a3ced28`。
- `a3ced28..ded7432` 的 5 个提交（X11 输入法重连、Node 版本管理器 shim、开发容器、aws-lc 换 ring、swash 去重）是用户 2026-11-27 要求“直接读提交、手工更新”后逐个移植的，**没有合并记录**。所以 `git log HEAD..upstream/main` 仍会列出它们，下次应以 `git log ded7432..upstream/main` 查看增量。
- 用户要求以功能冲突为取舍依据。遇到 Git 文本冲突时，先判断双方功能能否共存。README、发布日志、命名、资源路径和接口形状等差异按 Ade 的现有约定适配。功能确实冲突时保留 Ade 的行为，并说明舍弃的上游行为及影响。

## 已做的功能适配

- 上游分支图接入 Ade 底部可调整高度的历史区，保留单行提交、现有仓库列表和单文件 diff 默认行为。项目标签拖动沿用 Ade 标题栏布局。相关决策见 [GITUI-001](../decisions/git-panel.md) 和 [TITLEBAR](../decisions/title-bar.md)。
- 聊天列表缓存兼容 Ade 的归档、侧边会话和时间分隔。保留 Claude Code、Codex、OpenCode 的原有默认权限，新增 Antigravity 沿用上游的 `auto`。ACP 接口适配 Ade 的后台任务停止字段，Markdown 预览保留隐藏代码块的鼠标选区修复。
- Windows 文件关联图标用 Ade 标识重新生成，ACP 客户端和新增配置沿用 Ade 命名。相关决策见 [PRODUCT](../decisions/product-name.md)。
- 开发容器按 [BUILD-001](../decisions/build-features.md) 默认不编译：`recent_projects` 和 `ade` 的 `dev-container` 特性控制，`remote_servers.rs`、`recent_projects.rs` 里的相关代码用 `#[cfg(feature = "dev-container")]` 隔开。上游以后改动这些位置时，新增的开发容器代码也要放进同一开关。`remote`、`workspace`、`settings` 里的 Docker 连接类型没有隔开。
- 开发容器：路径 `crates/wu*` 对应 `crates/ade*`，`wu_actions` 对应 `ade_actions`，远程服务器二进制名用 `ade-remote-server-*`，文案里的 Wu 改为 Ade。`devcontainer.json` 里 `customizations.zed` 键和 `zed-dc` 等容器内部名沿用上游，保持与现有配置文件兼容。

## 下次同步

1. 执行 `git fetch --no-tags upstream main`，用 `git log --oneline ded7432..upstream/main` 查看增量。
2. 用户本次选择手工移植：逐个读提交，`git show <提交> | sed` 把路径和 `wu_actions` 换成 Ade 命名后 `git apply --reject`，失败的块按功能手工合入。改名后的文案和 Ade 已改写的上下文会导致 locale JSON 块失败，需要按键插入。若改用三方合并，先在临时 worktree 上试，可用 `git show --remerge-diff 5d6ee22` 回看上次的冲突处理。
3. 合入前验证受影响的交互和数据兼容性。按根 AGENTS.md 完成本机安装，更新此处的上游位置，保留影响后续同步的功能取舍。
