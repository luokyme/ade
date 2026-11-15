# Git 提交消息

编号前缀：COMMIT。涵盖范围：Git 面板生成提交消息的默认格式与语言。

### COMMIT-001 默认生成中文 Conventional Commits 提交消息

- 状态：有效
- 决策：Git 面板生成提交消息的默认提示词要求使用 Conventional Commits 格式，描述及正文使用中文，类型及可选 scope 保留英文。
- 理由：统一生成提交消息的结构与阅读语言。
- 依据：用户 2026-11-15 在查看现有提示词后明确要求“需要Conventional Commits，也需要中文”。
- 关联：[默认提示词](../crates/git_ui/src/commit_message_prompt.txt)。
