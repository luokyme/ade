# T3 Code 前端决策

本文件适用于 Ade 对接 T3 Code 的架构边界，以及与现有“对话”功能的关系，编号前缀 `T3`。

### T3-001 Ade 作为 T3 Code 的另一个前端

- 状态：有效
- 决策：Ade 的 T3 对话以 T3 Code 官方 server 为后端。智能体进程、对话、队列、审批和检查点都由 server 拥有。Ade 和 T3 Code 的 web、桌面、手机客户端平级，通过 server 的 WebSocket RPC 协议接入，自己实现客户端运行时和界面。Ade 不修改、不分叉 T3 Code server，跟随官方版本升级。迁移期间保留 Ade 现有的“对话”（`agent_chat`、`agent_harness`），两者并行。
- 理由：借助 T3 Code 的维护跟进 Claude Code、Codex 等智能体的升级，避免 Ade 自己跟不上。同一个 server 上的对话可以直接在 T3 Code 手机端查看和操作。
- 否决：继续只靠 Ade 自己的 `agent_harness` 跟进智能体协议，维护负担仍在 Ade 和上游 Wu 身上，也没有手机端。分叉或修改 T3 Code server，会失去跟随官方维护的意义。
- 依据：用户 2026-11-25 明确提出“把当前ade的后端直接调用t3, 本项目只做对应的前端”，确认“是ade实现t3的前端部分吧？仿佛是t3的另一个前端一样”，并要求“现在开始开发，但暂时先不要移除ade的'对话'功能”。
- 关联：`crates/t3_client`、`crates/t3_chat`、`crates/agent_chat`、`crates/agent_harness`。
