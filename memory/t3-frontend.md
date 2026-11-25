# T3 Code 前端对接

架构边界见 [T3-001](../decisions/t3-frontend.md)。

## 当前有效状态（2026-11-25）

- 第一阶段已开发并完成本机安装，`t3_client` 已对隔离 server 联调通过，界面尚未实测：`crates/t3_client` 实现 Effect RPC 客户端、本机 server 发现与 token、shell 和对话状态合并；`crates/t3_chat` 提供右侧停靠的 T3 Code 面板。面板入口没有放进标题栏左侧分段按钮（受 TITLEBAR-009 约束），通过 `t3_chat::ToggleFocus` 和面板按钮打开。
- 已覆盖：连接与自动重连、必要时启动 `t3 serve`、按工作区根目录匹配或添加项目、对话列表、对话时间线（消息、思考、命令、文件改动、检查点、待办、错误）、发送、排队、停止、移出队列、审批的允许和拒绝、新对话的模型选择。
- 未做（尚未与用户确认优先级）：回答智能体提问（目前只提示去网页或手机回答）、附件和图片、引导（steer）、检查点回退和 diff、远程 `wss://` 与配对、设置项、旧对话迁移、移除 `agent_chat`。

## 开发与验证环境

- 本机装有 `t3` v0.1.12（`~/.local/bin/t3`）。用户真实数据在 `~/.t3/userdata`，开发时不要对它启动 server，也不要写入。
- 测试用隔离 server：`t3 serve --base-dir /tmp/t3dev --port 37791 --host 127.0.0.1`，token 用 `t3 auth session issue --base-dir /tmp/t3dev --token-only` 签发。测试项目是 `/tmp/t3play`。`/tmp` 下的内容重启后会消失，需要时重建。
- 真实 server 联调：`T3_LIVE_HOME=/tmp/t3dev T3_LIVE_TOKEN_FILE=<token 文件> cargo test -p t3_client --test live -- --ignored`。Ade 读取 `T3CODE_HOME` 选择 server，默认 `~/.t3`。
- T3 Code 源码参考：`github.com/pingdotgg/t3code`。协议定义在 `packages/contracts/src/{rpc,orchestrationV2}.ts`，客户端合并逻辑在 `packages/client-runtime/src/state/orchestrationV2Projection.ts`。主干迭代很快，对齐协议时以本机 `t3 --version` 对应的版本为准。
