# T3 Code 前端对接

架构边界见 [T3-001](../decisions/t3-frontend.md)，对话可放在侧边面板或编辑区见 T3-002。

## 当前有效状态（2026-11-25）

- 已开发、安装并在隔离实例截图实测：`crates/t3_client` 是 Effect RPC 客户端（本机 server 发现、token、shell 与对话状态合并）；`crates/t3_chat` 是右侧 T3 Code 面板，对话也能作为编辑区标签页（`tab.rs`，kv `t3_chat_tabs_<workspace id>`，重启恢复）。面板入口不放进标题栏左侧分段按钮（TITLEBAR-009），用 `t3_chat::ToggleFocus` 打开。
- 用户要求对话窗口参考 T3 自己的界面，补齐思考等级和 fast 等模型选项。做法：配色和 Markdown 样式沿用 Ade“对话”（`agent_chat`，辅助函数复制在 `t3_chat/src/style.rs`，不依赖它，它将来可能移除）；工作记录、输入框、三栏模型选择器、权限和计划模式按 T3 网页端布局。英文文案尽量与 `agent_chat` 或 T3 相同，以复用中文翻译。
- 已覆盖：连接与自动重连、必要时启动 `t3 serve`、匹配或添加项目、对话列表、重启后恢复上次打开的对话（kv `t3_chat_open_thread_<workspace id>`）、时间线各类条目、按需拉取命令输出和文件 diff、运行中命令的实时输出、发送、排队、引导（Ctrl/Cmd+Enter 或 Ctrl 点击发送，排队消息可“引导”，也可点铅笔载入输入框重新编辑）、停止、审批、回答提问、模型及其选项、权限、计划模式、附件和图片（选择、拖放、粘贴）、`@` 文件和 `/` 命令补全、“从这里重新编辑”回退检查点、对话置顶与取消置顶（列表行悬停按钮）、后台任务（输入框下方的标记和任务卡片，列表行显示“后台运行中”）、输入框下方的上下文窗口用量圈（2026-11-26，悬停显示 token 明细，已对隔离 server 实测截图）、T3 自带 MCP 工具（2026-11-26，照网页端：工具友好标题、`send_to_user` 作为回复、`html_render` 在浏览器打开、fork/新建对话/交接/通知事件行、密钥/反馈/关联三种卡片、`t3_background_wait` 的等待状态；卡片接口只验证了格式，未对真实请求实测；未做 `mcp-app` 内嵌界面、密钥的 1Password 引用、关联卡片选已连接机器）。附件上传、实时输出、回退、回答提问、模型和模式设置都已对真实 server 验证。
- 标题栏项目小圆点：`workspace::register_agent_activity_source` 可多来源取最紧急；T3 来源在 `t3_chat/src/store.rs`，按项目根目录匹配，运行中或等待后台任务为“工作中”，待回应为“需要你”。面板创建时就连 server，关着也有圆点。未做“完成/失败”提示（T3 无已读状态，需 Ade 侧记录）。
- 未做（尚未与用户确认优先级）：整轮 diff 查看（`getTurnDiff`）、`$` 技能引用（T3 用 skill 上下文记录，不是纯文本）、发出消息里附件图片的缩略图（需 `assets.createUrl`）、远程 `wss://` 与配对、设置项、旧对话迁移、移除 `agent_chat`。`@` 文件目前以 `@相对路径` 纯文本发送，T3 原生是带 mention 上下文记录的标记。

## 协议上容易踩的点

- shell 订阅在真实快照后还会再发一个带 `resolvedRepositoryIdentityRoots` 的 `snapshot`，其中线程列表为空，只用来补项目的仓库身份，不能当整体替换（已按此处理，见 `ShellStreamItem::ProjectIdentities`）。只读第一条快照的测试发现不了这个问题。
- 线程流里命令输出（`outputOmitted`）和文件改动的 `diffStr` 会被省略，需要时用 `orchestration.getTurnItem` 取完整条目，T3 网页端的 “Open diff” 也是这么做的。
- 回答提问：`runtime-request.respond` 的 `answers` 按问题 id 填选项 label 或文本，多选填数组。
- 附件：`attachments.createUploadUrl` 拿 id 和相对地址，把原始字节 `POST` 过去（`Content-Length` 须等于声明大小，成功 204），再在 `message.dispatch` 的 `attachments` 里按 id 引用。图片限 png/jpeg/gif/webp、10 MB，其他 50 MB。
- 回退：每个 checkpoint scope 先有序号 0 的基线，之后每轮一个；回退到第 N 轮之前就是 `checkpoint.rollback` 到序号 N−1，`restoreFiles` 决定是否还原文件。server 是异步执行，稍后才把该轮标成 `rolled_back`。
- 置顶：`thread.pin` 的 `orderKey` 由客户端算（移植 server 的 `pinOrderKeyBetween`，排在全部置顶最前）；不带 key 会排在有 key 的后面。拖动排序（`thread.pin.reorder`）未做。
- 后台任务：直接用 shell 线程的 `pendingBackgroundTasks`；带 `awaited`（`t3_background_wait`）或子智能体、后台任务类的算“等待中”。T3 0.1.12 不能单独结束某一个，Stop 发不带 runId 的 `thread.stop` 结束全部，回合运行中禁用。未对真实后台任务实测。
- 用户卡片 `secret_request`、`feedback_request`、`link_request` 用单独的 `secrets.answerRequest`、`agentFeedback.answerRequest`、`peerLinks.answerRequest` 回答，不走 dispatch；失败错误只带 `reason`，文案要客户端自己出。
- 线程流里 `dynamic_tool` 的 output 是 server `compactDynamicToolOutput` 的摘要（`sentMessage`、`htmlRender` 等，失败为 `isError`），`send_to_user`、`html_render` 靠它识别。T3 工具名各家写法不同（`mcp__t3-code__x`、`t3-code.x` 等），识别规则移植自网页端。
- 上下文用量：线程快照的 `providerTurns[].tokenUsage`（`usedTokens`、`maxTokens`、`autoCompactThreshold`）按 `thread.activeProviderThreadId` 过滤后取 `updatedAt` 最新的一条；之后完成的 `compaction` 条目以 `afterTokenCount` 为准；都没有时用 provider thread 的 `contextUsage`。`provider-turn.updated` 只更新状态时不带 `tokenUsage`，要保留旧值；`provider-thread.updated`（非占位）会切换当前 session。算法照搬网页端，见 `ThreadState::context_window`。
- 引导：新消息用 `message.dispatch` 的 `steer_active`；已排队的消息用 `queued-message.promote-to-steer`。
- 编辑排队消息：`queued-run.edit` 带 `runId`、`text`；省略 `attachments` 保留原文件，传了就整体替换。原文件从隐藏的 `user_message` 条目（按 `messageId`）读取，回传时保留 `source` 等未知字段。run 开始后 server 拒绝编辑。只验证了命令格式，未对真实排队实测。
- 模型选项全部来自 server 配置里每个模型的 `capabilities.optionDescriptors`，选择以 `[{id, value}]` 放进 `modelSelection.options`。快速模式各家不同：Claude 是布尔 `fastMode`，Codex 是 `serviceTier` 的 `priority` 选项。已有对话用 `thread.model-selection.set`、`thread.runtime-mode.set`、`thread.interaction-mode.set` 修改；provider 的 `supportedRuntimeModes` 为空表示四档权限都支持。

## 开发与验证环境

- 本机装有 `t3` v0.1.12（`~/.local/bin/t3`，实际运行 `~/Downloads/T3-Code-x86_64.AppImage`）。用户真实数据在 `~/.t3/userdata`，开发时不要对它启动 server，也不要写入。
- 测试用隔离 server：`t3 serve --base-dir /tmp/t3dev --port 37791 --host 127.0.0.1`，token 用 `t3 auth session issue --base-dir /tmp/t3dev --token-only` 签发。测试项目是 `/tmp/t3play`。`/tmp` 下的内容重启后会消失，需要时重建。
- 真实 server 联调：`T3_LIVE_HOME=/tmp/t3dev T3_LIVE_TOKEN_FILE=<token 文件> cargo test -p t3_client --test live -- --ignored`；加 `T3_LIVE_PROJECT`、`T3_LIVE_PROMPT` 会新建一个对话并跑完一轮（消耗真实智能体额度，第一个就绪的 provider 当时是 Codex）。Ade 读取 `T3CODE_HOME` 选择 server，默认 `~/.t3`。
- 界面实测（KDE Wayland，无输入注入）：`env -u WAYLAND_DISPLAY T3CODE_HOME=/tmp/t3dev ~/.local/ade.app/libexec/ade-editor --user-data-dir <临时目录> /tmp/t3play` 走 XWayland；先在其 `db/0-dev/db.sqlite` 的 `workspaces` 设 `right_dock_visible=1, right_dock_active_panel='T3 Code Panel'`，`kv_store` 写打开对话 key；按 `_NET_WM_PID` 找窗口后 `import -window <id>` 截图。XTEST/XSendEvent 点击对 GPUI 无效，看展开或弹出状态只能临时加环境变量开关，截完删掉重装。不要用 `spectacle -a`（可能截到用户自己的 Ade）。
- 看 T3 原版界面：用无头 Chrome（`--remote-debugging-port`）打开隔离 server 打印的 `Pairing URL`，用 Node 22 的 WebSocket 走 DevTools 协议点击截图；线程地址 `/<projectId>/<threadId>`。
- 清理时只停自己起的进程：用户自己的 T3 server（`--base-dir ~/.t3`）也有 `--appimage-mount` 进程和 `/tmp/t3-token-*`。用 `pgrep -f` 杀进程时模式别和当前命令行重合，否则会杀掉自己的 shell。
- T3 Code 源码：`github.com/pingdotgg/t3code`，协议在 `packages/contracts/src/{rpc,orchestrationV2}.ts`。本机没有源码时，挂载 AppImage（`--appimage-mount`）读 `resources/app.asar` 里的 `apps/server/dist/*.mjs` 和网页端 bundle（`client/assets`）。主干迭代快，以本机 `t3 --version` 的版本为准。
