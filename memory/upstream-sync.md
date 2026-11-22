# 上游 Wu 同步

## 当前有效状态

- 已合入上游 `main` 的 Wu 1.5.0，最后同步的上游提交为 `a3ced2864abd933055b0d0afc5821e6cd9b0db2a`。Ade 的合并提交为 `5d6ee224dbf9bc3f9a608d8712933beea51e1123`，第二父提交就是该上游位置。此次同步覆盖共同基线 `41b71ce` 之后的 33 个上游提交。
- 第一轮拣入的四个提交随第一父提交 `04ce9b0` 保留在 Ade 历史中，最终三方合并已覆盖它们在上游的原始提交。下次从 `a3ced28` 之后继续。
- 用户在这次同步中明确要求以功能冲突为取舍依据。遇到 Git 文本冲突时，先判断双方功能能否共存。README、发布日志、命名、资源路径和接口形状等差异按 Ade 的现有约定适配。功能确实冲突时保留 Ade 的行为，并说明舍弃的上游行为及影响。

## 已做的功能适配

- 上游分支图接入 Ade 底部可调整高度的历史区，保留单行提交、现有仓库列表和单文件 diff 默认行为。项目标签拖动沿用 Ade 标题栏布局。相关决策见 [GITUI-001](../decisions/git-panel.md) 和 [TITLEBAR](../decisions/title-bar.md)。
- 聊天列表缓存兼容 Ade 的归档、侧边会话和时间分隔。保留 Claude Code、Codex、OpenCode 的原有默认权限，新增 Antigravity 沿用上游的 `auto`。ACP 接口适配 Ade 的后台任务停止字段，Markdown 预览保留隐藏代码块的鼠标选区修复。
- Windows 文件关联图标用 Ade 标识重新生成，ACP 客户端和新增配置沿用 Ade 命名。相关决策见 [PRODUCT](../decisions/product-name.md)。

## 下次同步

1. 执行 `git fetch --no-tags upstream main`，用 `git log --oneline HEAD..upstream/main` 查看尚未进入当前分支历史的提交。若 `a3ced28` 仍是上游祖先，`git log --oneline a3ced28..upstream/main` 可查看相对本次同步的上游增量。
2. 在临时 worktree 上从当前 Ade 分支做三方合并。先看相关设计决策，再按功能处理冲突。可用 `git show --remerge-diff 5d6ee22` 回看此次解决冲突的补丁。
3. 合入前验证受影响的交互和数据兼容性。按根 AGENTS.md 完成本机安装，更新此处的上游位置与 Ade 合并提交，保留影响后续同步的功能取舍。
