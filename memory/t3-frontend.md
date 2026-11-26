# T3 Code 前端对接

架构边界见 [T3-001](../decisions/t3-frontend.md)，对话可放在侧边面板或编辑区见 T3-002。

## 当前有效状态（2026-11-25）

- 已开发、本机安装，并在隔离实例里截图实测：`crates/t3_client` 实现 Effect RPC 客户端、本机 server 发现与 token、shell 和对话状态合并；`crates/t3_chat` 提供右侧停靠的 T3 Code 面板，对话也能作为编辑区标签页打开（`tab.rs`，标签页按工作区存在 kv `t3_chat_tabs_<workspace id>`，重启后恢复）。面板入口没有放进标题栏左侧分段按钮（受 TITLEBAR-009 约束），通过 `t3_chat::ToggleFocus` 和面板按钮打开。
- 用户要求对话窗口“使劲优化”，并要求参考 T3 自己的界面、补齐思考等级和 fast 等模型选项。当前做法：整体沿用 Ade “对话”（`agent_chat`）的配色和 Markdown 样式（辅助函数复制在 `t3_chat/src/style.rs`，不依赖 `agent_chat`，它将来可能移除）；工作记录、输入框、模型选择器、权限选择按 T3 网页端的布局做：“工作了 Xs”分隔行、紧凑的单行步骤、“任务 x/y”行、三栏模型选择器（提供方、可搜索模型列表、按模型 `optionDescriptors` 生成的选项）、标准/快速切换、输入框下方的计划模式和四档权限。英文文案尽量与 `agent_chat` 或 T3 相同，以复用已有中文翻译。
- 已覆盖：连接与自动重连、必要时启动 `t3 serve`、匹配或添加项目、对话列表、重启后恢复上次打开的对话（kv `t3_chat_open_thread_<workspace id>`）、时间线各类条目、按需拉取命令输出和文件 diff、运行中命令的实时输出、发送、排队、引导（Ctrl/Cmd+Enter 或 Ctrl 点击发送，排队消息可“引导”）、停止、审批、回答提问、模型及其选项、权限、计划模式、附件和图片（选择、拖放、粘贴）、`@` 文件和 `/` 命令补全、“从这里重新编辑”回退检查点。附件上传、实时输出、回退、回答提问、模型和模式设置都已对真实 server 验证。
- 标题栏项目小圆点：`workspace::register_agent_activity_source` 支持多个来源，取最紧急的一个；T3 来源在 `t3_chat/src/store.rs`，按项目根目录匹配 T3 项目，运行中为“工作中”，待回应的提问或审批为“需要你”。面板创建时就连接 server，所以面板关着也有圆点。未做“完成/失败”提示（T3 没有已读状态，需要在 Ade 侧记录）。
- 未做（尚未与用户确认优先级）：整轮 diff 查看（`getTurnDiff`）、`$` 技能引用（T3 用 skill 上下文记录，不是纯文本）、发出消息里附件图片的缩略图（需 `assets.createUrl`）、远程 `wss://` 与配对、设置项、旧对话迁移、移除 `agent_chat`。`@` 文件目前以 `@相对路径` 纯文本发送，T3 原生是带 mention 上下文记录的标记。

## 协议上容易踩的点

- shell 订阅在真实快照后还会再发一个带 `resolvedRepositoryIdentityRoots` 的 `snapshot`，其中线程列表为空，只用来补项目的仓库身份，不能当整体替换（已按此处理，见 `ShellStreamItem::ProjectIdentities`）。只读第一条快照的测试发现不了这个问题。
- 线程流里命令输出（`outputOmitted`）和文件改动的 `diffStr` 会被省略，需要时用 `orchestration.getTurnItem` 取完整条目，T3 网页端的 “Open diff” 也是这么做的。
- 回答提问：`runtime-request.respond` 带 `answers`，按问题 id 填选项 label 或自由文本，多选填数组；T3 会把回答作为一条用户消息交给智能体。
- 附件：先 `attachments.createUploadUrl` 拿到附件 id 和签名地址，再把原始字节 `POST` 到 server 源地址加该相对地址（`Content-Length` 必须等于声明大小，成功返回 204），最后在 `message.dispatch` 的 `attachments` 里按 id 引用。图片只接受 png/jpeg/gif/webp，单张 10 MB；其他文件 50 MB。
- 回退：每个 checkpoint scope 先有序号 0 的基线，之后每轮一个；回退到第 N 轮之前就是 `checkpoint.rollback` 到序号 N−1，`restoreFiles` 决定是否还原文件。server 是异步执行，稍后才把该轮标成 `rolled_back`。
- 引导：新消息用 `message.dispatch` 的 `steer_active`；已排队的消息用 `queued-message.promote-to-steer`。
- 模型选项全部来自 server 配置里每个模型的 `capabilities.optionDescriptors`，选择以 `[{id, value}]` 放进 `modelSelection.options`。快速模式各家不同：Claude 是布尔 `fastMode`，Codex 是 `serviceTier` 的 `priority` 选项。已有对话用 `thread.model-selection.set`、`thread.runtime-mode.set`、`thread.interaction-mode.set` 修改；provider 的 `supportedRuntimeModes` 为空表示四档权限都支持。

## 开发与验证环境

- 本机装有 `t3` v0.1.12（`~/.local/bin/t3`，实际运行 `~/Downloads/T3-Code-x86_64.AppImage`）。用户真实数据在 `~/.t3/userdata`，开发时不要对它启动 server，也不要写入。
- 测试用隔离 server：`t3 serve --base-dir /tmp/t3dev --port 37791 --host 127.0.0.1`，token 用 `t3 auth session issue --base-dir /tmp/t3dev --token-only` 签发。测试项目是 `/tmp/t3play`。`/tmp` 下的内容重启后会消失，需要时重建。
- 真实 server 联调：`T3_LIVE_HOME=/tmp/t3dev T3_LIVE_TOKEN_FILE=<token 文件> cargo test -p t3_client --test live -- --ignored`；加 `T3_LIVE_PROJECT`、`T3_LIVE_PROMPT` 会新建一个对话并跑完一轮（消耗真实智能体额度，第一个就绪的 provider 当时是 Codex）。Ade 读取 `T3CODE_HOME` 选择 server，默认 `~/.t3`。
- 界面实测做法（本机 KDE Wayland，没有输入注入工具）：用 `env -u WAYLAND_DISPLAY T3CODE_HOME=/tmp/t3dev ~/.local/ade.app/libexec/ade-editor --user-data-dir <临时目录> /tmp/t3play` 起隔离实例走 XWayland；在其 `db/0-dev/db.sqlite` 的 `workspaces` 表设 `right_dock_visible=1, right_dock_active_panel='T3 Code Panel'`，在 `kv_store` 写上面的打开对话 key，再启动；用 `xprop -root _NET_CLIENT_LIST` 按 `_NET_WM_PID` 找窗口，`import -window <id>` 只截该窗口。XTEST 和 XSendEvent 点击对 GPUI 无效；要看展开状态或弹出菜单，只能临时加环境变量开关（如用 `PopoverMenuHandle::show`），截图后删掉再重装。不要用 `spectacle -a`，活动窗口可能是用户自己的 Ade。
- 看 T3 原版界面：隔离 server 启动时会打印本机 `Pairing URL`，用 `google-chrome --headless=new --remote-debugging-port=<端口> --user-data-dir=<临时目录>` 打开它，再用 Node 22 自带的 WebSocket 走 DevTools 协议点击和截图；线程地址是 `/<projectId>/<threadId>`。Chrome 扩展当时未连接。
- 清理时注意：用户自己的 T3 server（`serve --base-dir ~/.t3`）也会留下 `--appimage-mount` 进程和 `/tmp/t3-token-*`，只按启动时间和工作目录停掉自己起的那些。
- T3 Code 源码参考：`github.com/pingdotgg/t3code`。协议定义在 `packages/contracts/src/{rpc,orchestrationV2}.ts`，客户端合并逻辑在 `packages/client-runtime/src/state/orchestrationV2Projection.ts`。本机没有源码时，可以挂载 AppImage（`--appimage-mount`）读 `resources/app.asar` 里的 `apps/server/dist/*.mjs` 和网页端 bundle。主干迭代很快，对齐协议时以本机 `t3 --version` 对应的版本为准。
