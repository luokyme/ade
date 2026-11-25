use crate::{Send, Stop, store::T3Store};
use editor::Editor;
use gpui::{
    AnyElement, App, ClickEvent, Context, Entity, EventEmitter, FocusHandle, Focusable, KeyContext,
    ScrollHandle, SharedString, Subscription, Task, WeakEntity, Window,
};
use i18n::t;
use language::LanguageRegistry;
use markdown::{Markdown, MarkdownElement, MarkdownFont, MarkdownStyle};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
};
use t3_client::{
    ApprovalDecision, Delivery, ModelSelection,
    protocol::{ThreadStreamItem, TurnItem, commands},
    state::{Applied, ThreadState},
};
use ui::{ContextMenu, PopoverMenu, Tooltip, prelude::*};
use util::ResultExt as _;
use workspace::Workspace;

/// How close to the end the view must be to keep following new output.
const FOLLOW_THRESHOLD: f32 = 32.;
const DEFAULT_RUNTIME_MODE: &str = "full-access";

pub enum ThreadViewEvent {
    Launched,
}

/// One T3 Code thread, or a draft that becomes one when its first message is sent.
pub struct ThreadView {
    store: Entity<T3Store>,
    workspace: WeakEntity<Workspace>,
    languages: Arc<LanguageRegistry>,
    project_id: String,
    project_root: PathBuf,
    thread_id: Option<String>,
    state: Option<ThreadState>,
    generation: usize,
    stream_task: Option<Task<()>>,
    send_task: Option<Task<()>>,
    composer: Entity<Editor>,
    markdown: HashMap<String, Entity<Markdown>>,
    expanded: HashSet<String>,
    scroll: ScrollHandle,
    selection: Option<ModelSelection>,
    error: Option<SharedString>,
    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ThreadViewEvent> for ThreadView {}

impl Focusable for ThreadView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl ThreadView {
    pub fn new(
        store: Entity<T3Store>,
        workspace: WeakEntity<Workspace>,
        languages: Arc<LanguageRegistry>,
        project_id: String,
        project_root: PathBuf,
        thread_id: Option<String>,
        default_selection: Option<ModelSelection>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let composer = cx.new(|cx| {
            let mut editor = Editor::auto_height(1, 10, window, cx);
            editor.set_placeholder_text(&t!("Message the agent"), window, cx);
            editor
        });
        let subscriptions = vec![cx.observe(&store, |this, store, cx| {
            let store = store.read(cx);
            if store.connection().is_some() && store.generation() != this.generation {
                this.subscribe(cx);
            }
            cx.notify();
        })];
        let mut this = Self {
            store,
            workspace,
            languages,
            project_id,
            project_root,
            thread_id,
            state: None,
            generation: 0,
            stream_task: None,
            send_task: None,
            composer,
            markdown: HashMap::default(),
            expanded: HashSet::default(),
            scroll: ScrollHandle::new(),
            selection: default_selection,
            error: None,
            focus_handle: cx.focus_handle(),
            _subscriptions: subscriptions,
        };
        this.subscribe(cx);
        this
    }

    pub fn thread_id(&self) -> Option<&str> {
        self.thread_id.as_deref()
    }

    pub fn composer_focus_handle(&self, cx: &App) -> FocusHandle {
        self.composer.focus_handle(cx)
    }

    fn subscribe(&mut self, cx: &mut Context<Self>) {
        let Some(thread_id) = self.thread_id.clone() else {
            return;
        };
        let store = self.store.read(cx);
        let Some(connection) = store.connection() else {
            self.stream_task = None;
            return;
        };
        self.generation = store.generation();
        let mut stream = match connection.subscribe_thread(&thread_id) {
            Ok(stream) => stream,
            Err(error) => {
                self.error = Some(error.to_string().into());
                return;
            }
        };
        self.state
            .get_or_insert_with(|| ThreadState::new(thread_id));
        self.stream_task = Some(cx.spawn(async move |this, cx| {
            while let Some(next) = stream.next().await {
                let value = match next {
                    Ok(value) => value,
                    Err(error) => {
                        this.update(cx, |this, cx| {
                            this.error = Some(error.to_string().into());
                            cx.notify();
                        })
                        .ok();
                        break;
                    }
                };
                if this.update(cx, |this, cx| this.apply(&value, cx)).is_err() {
                    break;
                }
            }
        }));
    }

    fn apply(&mut self, value: &Value, cx: &mut Context<Self>) {
        let item = match ThreadStreamItem::parse(value) {
            Ok(item) => item,
            Err(error) => {
                log::warn!("skipping an unreadable T3 Code thread item: {error:#}");
                return;
            }
        };
        let Some(state) = self.state.as_mut() else {
            return;
        };
        let following = is_at_bottom(&self.scroll);
        match state.apply(item) {
            Applied::Unchanged => return,
            Applied::NeedsResubscribe => {
                self.subscribe(cx);
                return;
            }
            Applied::Changed => {}
        }
        self.sync_markdown(cx);
        if following {
            self.scroll.scroll_to_bottom();
        }
        cx.notify();
    }

    fn sync_markdown(&mut self, cx: &mut Context<Self>) {
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let texts = state
            .inherited
            .iter()
            .map(|inherited| &inherited.item)
            .chain(state.timeline())
            .filter(|item| item.kind == "assistant_message")
            .map(|item| (item.id.clone(), item.text().to_string()))
            .collect::<Vec<_>>();
        for (id, text) in texts {
            match self.markdown.get(&id) {
                Some(markdown) => {
                    if markdown.read(cx).source().as_ref() != text {
                        markdown.update(cx, |markdown, cx| markdown.replace(text, cx));
                    }
                }
                None => {
                    let languages = self.languages.clone();
                    let markdown =
                        cx.new(|cx| Markdown::new(text.into(), Some(languages), None, cx));
                    self.markdown.insert(id, markdown);
                }
            }
        }
    }

    fn active_run_id(&self) -> Option<String> {
        self.state
            .as_ref()
            .and_then(ThreadState::active_run)
            .map(|run| run.id.clone())
    }

    fn send(&mut self, _: &Send, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.composer.read(cx).text(cx);
        if text.trim().is_empty() || self.send_task.is_some() {
            return;
        }
        self.error = None;
        self.composer
            .update(cx, |composer, cx| composer.clear(window, cx));
        let task = match self.thread_id.clone() {
            Some(thread_id) => {
                let delivery = if self.active_run_id().is_some() {
                    Delivery::Queue
                } else {
                    Delivery::StartNow
                };
                let command = commands::send_message(&thread_id, &text, &delivery);
                let sent = self.store.read(cx).dispatch(command, cx);
                cx.spawn_in(window, async move |this, cx| {
                    let result = sent.await;
                    this.update_in(cx, |this, window, cx| {
                        this.send_task = None;
                        if let Err(error) = result {
                            this.restore_draft(&text, error, window, cx);
                        }
                    })
                    .ok();
                })
            }
            None => {
                let Some(selection) = self.selection.clone() else {
                    self.restore_draft(
                        &text,
                        anyhow::anyhow!(t!("Choose a model first")),
                        window,
                        cx,
                    );
                    return;
                };
                let input = commands::launch_thread(
                    &self.project_id,
                    &selection,
                    DEFAULT_RUNTIME_MODE,
                    &text,
                );
                let launched = self.store.read(cx).launch_thread(input, cx);
                cx.spawn_in(window, async move |this, cx| {
                    let result = launched.await;
                    this.update_in(cx, |this, window, cx| {
                        this.send_task = None;
                        match result {
                            Ok(thread_id) => {
                                this.thread_id = Some(thread_id);
                                this.subscribe(cx);
                                cx.emit(ThreadViewEvent::Launched);
                            }
                            Err(error) => this.restore_draft(&text, error, window, cx),
                        }
                    })
                    .ok();
                })
            }
        };
        self.send_task = Some(task);
        cx.notify();
    }

    fn restore_draft(
        &mut self,
        text: &str,
        error: anyhow::Error,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.error = Some(format!("{error:#}").into());
        if self.composer.read(cx).text(cx).is_empty() {
            self.composer
                .update(cx, |composer, cx| composer.set_text(text, window, cx));
        }
        cx.notify();
    }

    fn stop(&mut self, _: &Stop, _: &mut Window, cx: &mut Context<Self>) {
        let (Some(thread_id), Some(run_id)) = (self.thread_id.clone(), self.active_run_id()) else {
            return;
        };
        self.run_command(commands::interrupt_run(&thread_id, &run_id), cx);
    }

    fn run_command(&mut self, command: Value, cx: &mut Context<Self>) {
        let task = self.store.read(cx).dispatch(command, cx);
        cx.spawn(async move |this, cx| {
            if let Err(error) = task.await {
                this.update(cx, |this, cx| {
                    this.error = Some(format!("{error:#}").into());
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    fn respond(&mut self, request_id: &str, decision: ApprovalDecision, cx: &mut Context<Self>) {
        let Some(thread_id) = self.thread_id.clone() else {
            return;
        };
        self.run_command(
            commands::respond_to_request(&thread_id, request_id, decision),
            cx,
        );
    }

    fn open_file(&self, file_name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let path = Path::new(file_name);
        let root = self
            .state
            .as_ref()
            .and_then(|state| state.thread.as_ref())
            .and_then(|thread| thread.worktree_path.as_deref())
            .map(PathBuf::from)
            .unwrap_or_else(|| self.project_root.clone());
        let path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            root.join(path)
        };
        self.workspace
            .update(cx, |workspace, cx| {
                workspace
                    .open_abs_path(path, Default::default(), window, cx)
                    .detach_and_log_err(cx);
            })
            .log_err();
    }

    fn toggle_expanded(&mut self, id: &str, cx: &mut Context<Self>) {
        if !self.expanded.remove(id) {
            self.expanded.insert(id.to_string());
        }
        cx.notify();
    }

    fn render_item(
        &self,
        item: &TurnItem,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let colors = cx.theme().colors();
        let element = match item.kind.as_str() {
            "user_message" => h_flex()
                .w_full()
                .justify_end()
                .child(
                    div()
                        .max_w(relative(0.85))
                        .px_3()
                        .py_2()
                        .rounded_lg()
                        .bg(colors.element_background)
                        .child(Label::new(item.text().to_string())),
                )
                .into_any_element(),
            "assistant_message" => {
                let markdown = self.markdown.get(&item.id)?.clone();
                div()
                    .w_full()
                    .child(MarkdownElement::new(
                        markdown,
                        MarkdownStyle::themed(MarkdownFont::Preview, window, cx),
                    ))
                    .into_any_element()
            }
            "reasoning" => {
                let id = item.id.clone();
                let expanded = self.expanded.contains(&item.id);
                let label = if item.is_running() {
                    t!("Thinking…")
                } else {
                    t!("Thought")
                };
                v_flex()
                    .w_full()
                    .gap_1()
                    .child(
                        h_flex()
                            .id(SharedString::from(format!("reasoning-{}", item.id)))
                            .gap_1()
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                this.toggle_expanded(&id, cx)
                            }))
                            .child(
                                Icon::new(if expanded {
                                    IconName::ChevronDown
                                } else {
                                    IconName::ChevronRight
                                })
                                .size(IconSize::XSmall)
                                .color(Color::Muted),
                            )
                            .child(Label::new(label).size(LabelSize::Small).color(Color::Muted)),
                    )
                    .when(expanded, |this| {
                        this.child(
                            div().pl_4().child(
                                Label::new(item.text().to_string())
                                    .size(LabelSize::Small)
                                    .color(Color::Muted),
                            ),
                        )
                    })
                    .into_any_element()
            }
            "command_execution" => {
                let input = item.str_field("input").unwrap_or("").to_string();
                let label = item.title.clone().filter(|title| !title.is_empty());
                self.render_tool_row(
                    item,
                    IconName::Terminal,
                    label.unwrap_or_else(|| format!("$ {input}")),
                    cx,
                )
            }
            "file_change" => {
                let file_name = item.str_field("fileName").unwrap_or("").to_string();
                let mut label = file_name.clone();
                if let (Some(additions), Some(deletions)) =
                    (item.i64_field("additions"), item.i64_field("deletions"))
                {
                    label.push_str(&format!("  +{additions} −{deletions}"));
                }
                let row = self.render_tool_row(item, IconName::File, label, cx);
                div()
                    .id(SharedString::from(format!("file-{}", item.id)))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        this.open_file(&file_name, window, cx)
                    }))
                    .child(row)
                    .into_any_element()
            }
            "approval_request" => self.render_approval(item, cx)?,
            "user_input_request" => {
                let questions = item
                    .fields
                    .get("questions")
                    .and_then(Value::as_array)
                    .map(|questions| {
                        questions
                            .iter()
                            .filter_map(|question| {
                                question
                                    .get("question")
                                    .or_else(|| question.get("header"))
                                    .and_then(Value::as_str)
                            })
                            .collect::<Vec<_>>()
                            .join("\n")
                    })
                    .unwrap_or_default();
                let pending = item.is_running() || item.status == "pending";
                self.render_card(
                    t!("The agent has a question"),
                    questions,
                    pending.then(|| {
                        Label::new(t!("Answer it in T3 Code on the web or your phone."))
                            .size(LabelSize::Small)
                            .color(Color::Muted)
                            .into_any_element()
                    }),
                    cx,
                )
            }
            "error" => {
                let message = item
                    .fields
                    .get("failure")
                    .and_then(|failure| failure.get("message"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                Label::new(message)
                    .size(LabelSize::Small)
                    .color(Color::Error)
                    .into_any_element()
            }
            "system_notice" => Label::new(item.str_field("message").unwrap_or("").to_string())
                .size(LabelSize::Small)
                .color(Color::Muted)
                .into_any_element(),
            "checkpoint" => {
                let count = item
                    .fields
                    .get("files")
                    .and_then(Value::as_array)
                    .map_or(0, Vec::len);
                if count == 0 {
                    return None;
                }
                Label::new(t!("{count} files changed", count = count))
                    .size(LabelSize::Small)
                    .color(Color::Muted)
                    .into_any_element()
            }
            "todo_list" => {
                let steps = item
                    .fields
                    .get("steps")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                v_flex()
                    .gap_0p5()
                    .children(steps.iter().map(|step| {
                        let done = step.get("status").and_then(Value::as_str) == Some("completed");
                        let text = step
                            .get("step")
                            .or_else(|| step.get("text"))
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string();
                        h_flex()
                            .gap_1()
                            .child(
                                Icon::new(if done {
                                    IconName::Check
                                } else {
                                    IconName::Circle
                                })
                                .size(IconSize::XSmall)
                                .color(Color::Muted),
                            )
                            .child(Label::new(text).size(LabelSize::Small))
                    }))
                    .into_any_element()
            }
            // Workspace setup shows as a command; the rest only matter to T3 Code itself.
            "run_interrupt_request" | "run_interrupt_result" | "compaction" | "handoff" => {
                let title = item.title.clone()?;
                Label::new(title)
                    .size(LabelSize::Small)
                    .color(Color::Muted)
                    .into_any_element()
            }
            _ => {
                let title = item.title.clone().filter(|title| !title.is_empty())?;
                self.render_tool_row(item, IconName::Sparkle, title, cx)
            }
        };
        Some(element)
    }

    fn render_tool_row(
        &self,
        item: &TurnItem,
        icon: IconName,
        label: String,
        _cx: &Context<Self>,
    ) -> AnyElement {
        let (status_icon, status_color) = if item.is_running() {
            (IconName::LoadCircle, Color::Muted)
        } else if item.is_failed() {
            (IconName::XCircle, Color::Error)
        } else {
            (IconName::Check, Color::Muted)
        };
        h_flex()
            .w_full()
            .gap_1p5()
            .child(Icon::new(icon).size(IconSize::XSmall).color(Color::Muted))
            .child(
                div().flex_1().min_w_0().child(
                    Label::new(label)
                        .size(LabelSize::Small)
                        .color(Color::Muted)
                        .truncate(),
                ),
            )
            .child(
                Icon::new(status_icon)
                    .size(IconSize::XSmall)
                    .color(status_color),
            )
            .into_any_element()
    }

    fn render_card(
        &self,
        title: SharedString,
        body: String,
        footer: Option<AnyElement>,
        cx: &Context<Self>,
    ) -> AnyElement {
        let colors = cx.theme().colors();
        v_flex()
            .w_full()
            .gap_1()
            .p_2()
            .rounded_md()
            .border_1()
            .border_color(colors.border)
            .bg(colors.editor_background)
            .child(
                Label::new(title)
                    .size(LabelSize::Small)
                    .color(Color::Accent),
            )
            .when(!body.is_empty(), |this| {
                this.child(Label::new(body).size(LabelSize::Small))
            })
            .children(footer)
            .into_any_element()
    }

    fn render_approval(&self, item: &TurnItem, cx: &mut Context<Self>) -> Option<AnyElement> {
        let request_id = item.str_field("requestId")?.to_string();
        let body = item
            .str_field("prompt")
            .or_else(|| item.str_field("reason"))
            .unwrap_or("")
            .to_string();
        let pending = self
            .state
            .as_ref()
            .and_then(|state| state.pending_request(&request_id))
            .filter(|request| request.accepts_decision())
            .is_some();
        let footer = pending.then(|| {
            let button = |label: SharedString, decision: ApprovalDecision, style: ButtonStyle| {
                let request_id = request_id.clone();
                Button::new(
                    SharedString::from(format!("{request_id}-{decision:?}")),
                    label,
                )
                .style(style)
                .size(ButtonSize::Compact)
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.respond(&request_id, decision, cx)
                }))
            };
            h_flex()
                .gap_1()
                .child(button(
                    t!("Allow"),
                    ApprovalDecision::Accept,
                    ButtonStyle::Filled,
                ))
                .child(button(
                    t!("Allow for this session"),
                    ApprovalDecision::AcceptForSession,
                    ButtonStyle::Subtle,
                ))
                .child(button(
                    t!("Deny"),
                    ApprovalDecision::Decline,
                    ButtonStyle::Subtle,
                ))
                .into_any_element()
        });
        Some(self.render_card(t!("The agent asks for permission"), body, footer, cx))
    }

    fn render_queue(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let state = self.state.as_ref()?;
        let thread_id = self.thread_id.clone()?;
        let queue = state.queue();
        if queue.is_empty() {
            return None;
        }
        let colors = cx.theme().colors();
        Some(
            v_flex()
                .gap_1()
                .children(queue.into_iter().map(|(run, text)| {
                    let run_id = run.id.clone();
                    let thread_id = thread_id.clone();
                    h_flex()
                        .gap_1()
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .border_1()
                        .border_dashed()
                        .border_color(colors.border)
                        .child(
                            div().flex_1().min_w_0().child(
                                Label::new(text.to_string())
                                    .size(LabelSize::Small)
                                    .color(Color::Muted)
                                    .truncate(),
                            ),
                        )
                        .child(
                            IconButton::new(
                                SharedString::from(format!("cancel-{run_id}")),
                                IconName::Close,
                            )
                            .icon_size(IconSize::XSmall)
                            .tooltip(Tooltip::text(t!("Remove from queue")))
                            .on_click(cx.listener(
                                move |this, _: &ClickEvent, _, cx| {
                                    this.run_command(
                                        commands::cancel_queued_run(&thread_id, &run_id),
                                        cx,
                                    )
                                },
                            )),
                        )
                }))
                .into_any_element(),
        )
    }

    fn render_model_picker(&self, cx: &mut Context<Self>) -> AnyElement {
        let providers = self.store.read(cx).providers().to_vec();
        let label = self
            .selection
            .as_ref()
            .and_then(|selection| {
                let provider = providers
                    .iter()
                    .find(|provider| provider.instance_id == selection.instance_id)?;
                let model = provider
                    .models
                    .iter()
                    .find(|model| model.slug == selection.model)?;
                Some(format!("{} · {}", provider.display_name, model.name))
            })
            .unwrap_or_else(|| t!("Choose a model").to_string());
        let view = cx.weak_entity();
        PopoverMenu::new("t3-model-picker")
            .trigger(
                Button::new("t3-model-picker-trigger", label)
                    .style(ButtonStyle::Subtle)
                    .size(ButtonSize::Compact)
                    .label_size(LabelSize::Small)
                    .end_icon(Icon::new(IconName::ChevronDown).size(IconSize::XSmall)),
            )
            .menu(move |window, cx| {
                let providers = providers.clone();
                let view = view.clone();
                Some(ContextMenu::build(window, cx, move |mut menu, _, _| {
                    for provider in providers.iter().filter(|provider| provider.is_ready()) {
                        menu = menu.header(provider.display_name.clone());
                        for model in &provider.models {
                            let selection = ModelSelection {
                                instance_id: provider.instance_id.clone(),
                                model: model.slug.clone(),
                                options: None,
                            };
                            let view = view.clone();
                            menu = menu.entry(model.name.clone(), None, move |_, cx| {
                                view.update(cx, |this, cx| {
                                    this.selection = Some(selection.clone());
                                    cx.notify();
                                })
                                .ok();
                            });
                        }
                    }
                    menu
                }))
            })
            .into_any_element()
    }

    fn render_composer(&self, cx: &mut Context<Self>) -> AnyElement {
        let border = cx.theme().colors().border;
        let background = cx.theme().colors().editor_background;
        let model_picker = self
            .thread_id
            .is_none()
            .then(|| self.render_model_picker(cx));
        let working = self.active_run_id().is_some();
        let mut key_context = KeyContext::new_with_defaults();
        key_context.add("T3Composer");
        let send_button = IconButton::new("t3-send", IconName::Send)
            .icon_size(IconSize::Small)
            .tooltip(Tooltip::text(if working {
                t!("Queue message")
            } else {
                t!("Send message")
            }))
            .disabled(self.send_task.is_some())
            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.send(&Send, window, cx)));
        let stop_button = working.then(|| {
            IconButton::new("t3-stop", IconName::Stop)
                .icon_size(IconSize::Small)
                .icon_color(Color::Error)
                .tooltip(Tooltip::text(t!("Stop")))
                .on_click(
                    cx.listener(|this, _: &ClickEvent, window, cx| this.stop(&Stop, window, cx)),
                )
        });
        let footer = h_flex()
            .w_full()
            .gap_1()
            .child(div().flex_1().min_w_0().children(model_picker))
            .children(stop_button)
            .child(send_button);
        v_flex()
            .key_context(key_context)
            .on_action(cx.listener(Self::send))
            .on_action(cx.listener(Self::stop))
            .w_full()
            .gap_1()
            .p_2()
            .rounded_md()
            .border_1()
            .border_color(border)
            .bg(background)
            .child(self.composer.clone())
            .child(footer)
            .into_any_element()
    }
}

fn is_at_bottom(scroll: &ScrollHandle) -> bool {
    let offset = scroll.offset();
    let max_offset = scroll.max_offset();
    -offset.y >= max_offset.y - px(FOLLOW_THRESHOLD)
}

impl Render for ThreadView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut rows = Vec::new();
        if let Some(state) = self.state.as_ref() {
            let items = state
                .inherited
                .iter()
                .map(|inherited| inherited.item.clone())
                .chain(state.timeline().cloned())
                .collect::<Vec<_>>();
            for item in &items {
                if let Some(element) = self.render_item(item, window, cx) {
                    rows.push(element);
                }
            }
        }
        let loading = self.thread_id.is_some()
            && self
                .state
                .as_ref()
                .is_none_or(|state| !state.has_snapshot());
        let working = self.active_run_id().is_some();
        let queue = self.render_queue(cx);
        v_flex()
            .size_full()
            .track_focus(&self.focus_handle)
            .child(
                div()
                    .id("t3-thread-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(
                        v_flex()
                            .p_3()
                            .gap_3()
                            .when(loading, |this| {
                                this.child(
                                    Label::new(t!("Loading…"))
                                        .size(LabelSize::Small)
                                        .color(Color::Muted),
                                )
                            })
                            .when(self.thread_id.is_none(), |this| {
                                this.child(
                                    Label::new(t!(
                                        "The agent runs in T3 Code. You can follow this thread from T3 Code's web and mobile apps too."
                                    ))
                                    .size(LabelSize::Small)
                                    .color(Color::Muted),
                                )
                            })
                            .children(rows)
                            .when(working, |this| {
                                this.child(
                                    LoadingLabel::new(t!("Working"))
                                        .size(LabelSize::Small)
                                        .color(Color::Muted),
                                )
                            }),
                    ),
            )
            .child(
                v_flex()
                    .p_2()
                    .gap_1()
                    .children(queue)
                    .when_some(self.error.clone(), |this, error| {
                        this.child(Label::new(error).size(LabelSize::Small).color(Color::Error))
                    })
                    .child(self.render_composer(cx)),
            )
    }
}
