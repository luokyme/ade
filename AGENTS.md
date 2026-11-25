# 项目记忆与阅读入口

- 接手任务或切换工作主题时，按下方工作线索引选择相关记忆；没有相关项则不读，不批量加载整个 `memory/`。
- 涉及已登记设计域的设计、编码、修复或审查，先读取对应决策文件。已读取且仍在上下文中的规则可以复用。
- 根 `AGENTS.md` 维护全局行为与索引，目录内 `AGENTS.md` 只补充该目录规则，不复制根规则。
- 长期行为要求写入适用的 `AGENTS.md`；跨会话上下文写入 `memory/`；用户确认的稳定产品或架构选择写入 `decisions/`。

## 项目记忆

- 工作线索引是统一入口，只列文件路径和涵盖范围，详细上下文保存在 `memory/`。
- 仅收录后续独立任务仍需使用、且难以从代码、配置、Git 历史或现有规则恢复的信息；普通排查、修复、构建和测试结果默认不写入长期记忆。
- 维护前读取 [memory/AGENTS.md](memory/AGENTS.md)，日常查阅无需额外读取维护规则。任务收尾只检查更新必要性，不要求每次产生记忆。

### 工作线索引

| 记忆文件路径 | 涵盖范围 |
| --- | --- |
| [memory/git-ui.md](memory/git-ui.md) | Git 面板的用户偏好与 Repositories 范围确认 |
| [memory/local-deploy.md](memory/local-deploy.md) | 本机部署 Ade 的可行路径、资源加载限制与远端仓库现状 |
| [memory/upstream-sync.md](memory/upstream-sync.md) | 上游 Wu 的同步位置、功能冲突处理与增量合并 |
| [memory/t3-frontend.md](memory/t3-frontend.md) | T3 Code 前端对接的进度、未做事项与隔离测试环境 |

## 设计决策

- 设计决策索引是用户确认的稳定设计选择的统一入口；登记或变更前读取 [decisions/AGENTS.md](decisions/AGENTS.md)。
- 有效决策作为需求、设计和验收依据；与记忆冲突时以决策为准并修正记忆。审查时将违反有效决策列为问题。
- 新要求或方案与有效决策冲突时，说明编号、冲突和影响，由用户决定保持原决策或以新决策取代；用户已明确要求变更的，按该要求登记替代条目，无需重复确认。
- 用户明确确认符合收录门槛的选择后，在当次任务内登记。实现授权不自动确认候选方案和未决假设。

### 设计决策索引

| 决策文件路径 | 编号前缀 | 涵盖范围 |
| --- | --- | --- |
| [decisions/title-bar.md](decisions/title-bar.md) | TITLEBAR | 标题栏与侧栏入口、最近的项目和导航位置、状态栏位置与诊断入口、文件标签位置与样式、悬浮关闭按钮 |
| [decisions/product-name.md](decisions/product-name.md) | PRODUCT | 产品名称 Ade：crate、动作命名空间、URL scheme、环境变量与目录的统一命名；应用名称和桌面标识不加 Dev 后缀 |
| [decisions/git-commit-messages.md](decisions/git-commit-messages.md) | COMMIT | Git 面板默认生成中文 Conventional Commits 提交消息 |
| [decisions/git-panel.md](decisions/git-panel.md) | GITUI | Git 面板分区、历史记录、工作区与历史差异页 |
| [decisions/t3-frontend.md](decisions/t3-frontend.md) | T3 | Ade 作为 T3 Code 前端的架构边界，与现有“对话”功能并行 |

# Rust coding guidelines

* Prioritize code correctness and clarity. Speed and efficiency are secondary priorities unless otherwise specified.
* Do not write organizational or comments that summarize the code. Comments should only be written in order to explain "why" the code is written in some way in the case there is a reason that is tricky / non-obvious.
* Prefer implementing functionality in existing files unless it is a new logical component. Avoid creating many small files.
* Avoid using functions that panic like `unwrap()`, instead use mechanisms like `?` to propagate errors.
* Be careful with operations like indexing which may panic if the indexes are out of bounds.
* Never silently discard errors with `let _ =` on fallible operations. Always handle errors appropriately:
  - Propagate errors with `?` when the calling function should handle them
  - Use `.log_err()` or similar when you need to ignore errors but want visibility
  - Use explicit error handling with `match` or `if let Err(...)` when you need custom logic
  - Example: avoid `let _ = client.request(...).await?;` - use `client.request(...).await?;` instead
* When implementing async operations that may fail, ensure errors propagate to the UI layer so users get meaningful feedback.
* Never create files with `mod.rs` paths - prefer `src/some_module.rs` instead of `src/some_module/mod.rs`.
* When creating new crates, prefer specifying the library root path in `Cargo.toml` using `[lib] path = "...rs"` instead of the default `lib.rs`, to maintain consistent and descriptive naming (e.g., `gpui.rs` or `main.rs`).
* Avoid creative additions unless explicitly requested
* Use full words for variable names (no abbreviations like "q" for "queue")
* Use variable shadowing to scope clones in async contexts for clarity, minimizing the lifetime of borrowed references.
  Example:
  ```rust
  executor.spawn({
      let task_ran = task_ran.clone();
      async move {
          *task_ran.borrow_mut() = true;
      }
  });
  ```

# Timers in tests

* In GPUI tests, prefer GPUI executor timers over `smol::Timer::after(...)` when you need timeouts, delays, or to drive `run_until_parked()`:
  - Use `cx.background_executor().timer(duration).await` (or `cx.background_executor.timer(duration).await` in `TestAppContext`) so the work is scheduled on GPUI's dispatcher.
  - Avoid `smol::Timer::after(...)` for test timeouts when you rely on `run_until_parked()`, because it may not be tracked by GPUI's scheduler and can lead to "nothing left to run" when pumping.

# GPUI

GPUI is a UI framework which also provides primitives for state and concurrency management.

## Context

Context types allow interaction with global state, windows, entities, and system services. They are typically passed to functions as the argument named `cx`. When a function takes callbacks they come after the `cx` parameter.

* `App` is the root context type, providing access to global state and read and update of entities.
* `Context<T>` is provided when updating an `Entity<T>`. This context dereferences into `App`, so functions which take `&App` can also take `&Context<T>`.
* `AsyncApp` and `AsyncWindowContext` are provided by `cx.spawn` and `cx.spawn_in`. These can be held across await points.

## `Window`

`Window` provides access to the state of an application window. It is passed to functions as an argument named `window` and comes before `cx` when present. It is used for managing focus, dispatching actions, directly drawing, getting user input state, etc.

## Entities

An `Entity<T>` is a handle to state of type `T`. With `thing: Entity<T>`:

* `thing.entity_id()` returns `EntityId`
* `thing.downgrade()` returns `WeakEntity<T>`
* `thing.read(cx: &App)` returns `&T`.
* `thing.read_with(cx, |thing: &T, cx: &App| ...)` returns the closure's return value.
* `thing.update(cx, |thing: &mut T, cx: &mut Context<T>| ...)` allows the closure to mutate the state, and provides a `Context<T>` for interacting with the entity. It returns the closure's return value.
* `thing.update_in(cx, |thing: &mut T, window: &mut Window, cx: &mut Context<T>| ...)` takes a `AsyncWindowContext` or `VisualTestContext`. It's the same as `update` while also providing the `Window`.

Within the closures, the inner `cx` provided to the closure must be used instead of the outer `cx` to avoid issues with multiple borrows.

Trying to update an entity while it's already being updated must be avoided as this will cause a panic.

`WeakEntity<T>` is a weak handle. It has `read_with`, `update`, and `update_in` methods that work the same, but always return an `anyhow::Result` so that they can fail if the entity no longer exists. This can be useful to avoid memory leaks - if entities have mutually recursive handles to each other they will never be dropped.

## Concurrency

All use of entities and UI rendering occurs on a single foreground thread.

`cx.spawn(async move |cx| ...)` runs an async closure on the foreground thread. Within the closure, `cx` is `&mut AsyncApp`.

When the outer cx is a `Context<T>`, the use of `spawn` instead looks like `cx.spawn(async move |this, cx| ...)`, where `this: WeakEntity<T>` and `cx: &mut AsyncApp`.

To do work on other threads, `cx.background_spawn(async move { ... })` is used. Often this background task is awaited on by a foreground task which uses the results to update state.

Both `cx.spawn` and `cx.background_spawn` return a `Task<R>`, which is a future that can be awaited upon. If this task is dropped, then its work is cancelled. To prevent this one of the following must be done:

* Awaiting the task in some other async context.
* Detaching the task via `task.detach()` or `task.detach_and_log_err(cx)`, allowing it to run indefinitely.
* Storing the task in a field, if the work should be halted when the struct is dropped.

A task which doesn't do anything but provide a value can be created with `Task::ready(value)`.

## Elements

The `Render` trait is used to render some state into an element tree that is laid out using flexbox layout. An `Entity<T>` where `T` implements `Render` is sometimes called a "view".

Example:

```
struct TextWithBorder(SharedString);

impl Render for TextWithBorder {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().border_1().child(self.0.clone())
    }
}
```

Since `impl IntoElement for SharedString` exists, it can be used as an argument to `child`. `SharedString` is used to avoid copying strings, and is either an `&'static str` or `Arc<str>`.

UI components that are constructed just to be turned into elements can instead implement the `RenderOnce` trait, which is similar to `Render`, but its `render` method takes ownership of `self` and receives `&mut App` instead of `&mut Context<Self>`. Types that implement this trait can use `#[derive(IntoElement)]` to use them directly as children.

The style methods on elements are similar to those used by Tailwind CSS.

If some attributes or children of an element tree are conditional, `.when(condition, |this| ...)` can be used to run the closure only when `condition` is true. Similarly, `.when_some(option, |this, value| ...)` runs the closure when the `Option` has a value.

## Input events

Input event handlers can be registered on an element via methods like `.on_click(|event, window, cx: &mut App| ...)`.

Often event handlers will want to update the entity that's in the current `Context<T>`. The `cx.listener` method provides this - its use looks like `.on_click(cx.listener(|this: &mut T, event, window, cx: &mut Context<T>| ...)`.

## Actions

Actions are dispatched via user keyboard interaction or in code via `window.dispatch_action(SomeAction.boxed_clone(), cx)` or `focus_handle.dispatch_action(&SomeAction, window, cx)`.

Actions with no data are defined with the `actions!(some_namespace, [SomeAction, AnotherAction])` macro call. Otherwise the `Action` derive macro is used. Doc comments on actions are displayed to the user.

Action handlers can be registered on an element via the event handler `.on_action(|action, window, cx| ...)`. Like other event handlers, this is often used with `cx.listener`.

## Notify

When a view's state has changed in a way that may affect its rendering, it should call `cx.notify()`. This will cause the view to be rerendered. It will also cause any observe callbacks registered for the entity with `cx.observe` to be called.

## Entity events

While updating an entity (`cx: Context<T>`), it can emit an event using `cx.emit(event)`. Entities register which events they can emit by declaring `impl EventEmitter<EventType> for EntityType {}`.

Other entities can then register a callback to handle these events by doing `cx.subscribe(other_entity, |this, other_entity, event, cx| ...)`. This will return a `Subscription` which deregisters the callback when dropped.  Typically `cx.subscribe` happens when creating a new entity and the subscriptions are stored in a `_subscriptions: Vec<Subscription>` field.

## Build guidelines

- Use `./script/clippy` instead of `cargo clippy`
- 每次完成修改后、交付前，必须在项目根目录执行 `./script/install-linux`，完成编辑器与 CLI 的编译、本机安装以及桌面入口更新。完整发布打包单独使用 `./script/bundle-linux`，不作为本机安装的前置步骤。仅执行 `cargo build -p ade --bin ade`、`cargo check` 或测试不能替代安装交付；用户本次明确要求其他方式或跳过时，遵循该指示。
- 构建失败时处理与本次修改相关的问题；环境或既有问题阻塞时，明确说明原因，不宣称构建成功。交付时区分源码验证、完整构建、安装和桌面入口更新、应用是否已重启；安装失败必须说明阻塞，不能只构建后宣称已更新。

# Pull request hygiene

When an agent opens or updates a pull request, it must:

- Use a clear, correctly capitalized, imperative PR title (for example, `Fix crash in project panel`).
- Avoid conventional commit prefixes in PR titles (`fix:`, `feat:`, `docs:`, etc.).
- Avoid trailing punctuation in PR titles.
- Optionally prefix the title with a crate name when one crate is the clear scope (for example, `git_ui: Add history view`).
- Include a `Release Notes:` section as the final section in the PR body.
- Use one bullet under `Release Notes:`:
  - `- Added ...`, `- Fixed ...`, or `- Improved ...` for user-facing changes, or
  - `- N/A` for docs-only and other non-user-facing changes.
- Format release notes exactly with a blank line after the heading, for example:

```
Release Notes:

- N/A
```

# Rules Hygiene

These rules are read by every agent session. Keep them high-signal.

用户明确要求维护规则、记忆系统或长期工作要求时，可直接更新相应文件；下方禁止顺手新增规则的约束适用于普通功能与修复工作。记忆和设计决策分别遵循各目录的维护规则。

## After any agentic session
If you discover a non-obvious pattern that would help future sessions, include a **"Suggested rules additions"** heading in your PR description with the proposed text. Do **not** edit rules inline during normal feature/fix work. Reviewers decide what gets merged.

## High bar for new rules
Editing or clarifying existing rules is always welcome. New rules must meet **all three** criteria:
1. **Non-obvious** — someone familiar with the codebase would still get it wrong without the rule.
2. **Repeatedly encountered** — it came up more than once (multiple hits in one session counts).
3. **Specific enough to act on** — a concrete instruction, not a vague principle.

## What NOT to put in rules
Avoid architectural descriptions of a crate (module layout, data flow, key types). These go stale fast and the agent can gather them by reading the code. Rules should be **traps to avoid**, not **maps to follow**.

## No drive-by additions
Rules emerge from validated patterns, not one-off observations. The workflow is:
1. Agent notes a pattern during a session.
2. Team validates the pattern in code review.
3. A dedicated commit adds the rule with context on *why* it exists.
