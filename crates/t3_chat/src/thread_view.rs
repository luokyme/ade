mod attachments;
mod completion;

use crate::{
    AcceptCompletion, DismissCompletion, SelectNextCompletion, SelectPreviousCompletion, Send,
    Steer, Stop,
    diff::FileDiff,
    model_picker::{
        ModelPicker, OnMode, OnSelection, PermissionPicker, is_fast, provider_icon,
        reasoning_label, runtime_mode_info,
    },
    store::T3Store,
    style::{
        SPACE_LG, SPACE_MD, SPACE_SM, TOOL_LINE_HEIGHT, TOOL_TEXT_SIZE, accent, code_font,
        composer_surface,
        flavour_seed, flavour_word, format_elapsed, gradient_spinner, hairline, icon, icon_button,
        ink, mix, page, shimmer_title, spinning_icon, text_faint, ui, wash,
    },
    transcript::{
        Class, Row, Tally, Turn, clip_lines, classify, describe, display_path, group_summary,
        group_turns, is_workspace_setup, notice_text, single_line, strip_ansi, todo_progress,
        turn_rows, unwrap_shell, value_text,
    },
};
use editor::{Editor, EditorEvent};
use gpui::ExternalPaths;
use gpui::{
    Animation, AnimationExt as _, AnyElement, App, ClickEvent, ClipboardItem, Context, ElementId,
    Entity, EventEmitter, FocusHandle, Focusable, FontWeight, Hsla, KeyContext, ScrollHandle,
    SharedString, StyleRefinement, Subscription, Task, TextStyleRefinement,
    UnderlineStyle, WeakEntity, Window, ease_out_quint, point,
};
use i18n::t;
use language::LanguageRegistry;
use markdown::{
    CodeBlockRenderer, CopyButtonVisibility, HeadingLevelStyles, Markdown, MarkdownElement,
    MarkdownFont, MarkdownOptions, MarkdownStyle, WrapButtonVisibility,
};
use serde_json::{Map, Value};
use std::{
    collections::{HashMap, HashSet},
    rc::Rc,
    ops::Range,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use t3_client::{
    ApprovalDecision, Delivery, ModelSelection,
    protocol::{ChatAttachment, ProviderInfo, ThreadStreamItem, TurnItem, commands},
    state::{Applied, ThreadState},
};
use theme::ActiveTheme as _;
use ui::{ButtonLike, PopoverMenu, Tooltip, prelude::*};
use util::ResultExt as _;
use workspace::Workspace;

/// How close to the end the view must be to keep following new output.
const FOLLOW_THRESHOLD: f32 = 32.;
const SCROLL_BUTTON_THRESHOLD: f32 = 240.;
const DEFAULT_RUNTIME_MODE: &str = "full-access";
const SECONDARY_MODIFIER: &str = if cfg!(target_os = "macos") { "Cmd" } else { "Ctrl" };
const DEFAULT_INTERACTION_MODE: &str = "default";
const GUTTER: f32 = SPACE_LG;
const MD_BLOCK_GAP: f32 = 12.;
const USER_LINE_HEIGHT: f32 = 22.;
const USER_COLLAPSED_LINES: usize = 5;
const USER_COLLAPSE_CHARS: usize = 400;
const GROUP_HEADER_HEIGHT: f32 = 26.;
const STEP_ROW_HEIGHT: f32 = 26.;
const STEP_INDENT: f32 = 26.;
const OUTPUT_LINE_HEIGHT: f32 = 18.;
const OUTPUT_MAX_LINES: usize = 24;
const INPUT_MAX_LINES: usize = 8;
const COMPOSER_RADIUS: f32 = 18.;
/// How wide the conversation gets in an editor tab, so lines stay readable.
const TAB_CONTENT_WIDTH: f32 = 780.;
const SEND_BUTTON_SIZE: f32 = 28.;
const TEXTAREA_MAX: f32 = 220.;
const FADE_IN: Duration = Duration::from_millis(260);
const COPIED_FEEDBACK: Duration = Duration::from_millis(1200);

pub enum ThreadViewEvent {
    Launched,
    /// The title or status shown on this thread's tab may have changed.
    Changed,
}

/// One T3 Code thread, or a draft that becomes one when its first message is sent.
pub struct ThreadView {
    store: Entity<T3Store>,
    workspace: WeakEntity<Workspace>,
    languages: Arc<LanguageRegistry>,
    project_id: String,
    project_root: PathBuf,
    thread_id: Option<String>,
    /// Shown in the side panel rather than as an editor tab.
    docked: bool,
    state: Option<ThreadState>,
    generation: usize,
    stream_task: Option<Task<()>>,
    send_task: Option<Task<()>>,
    composer: Entity<Editor>,
    items: Vec<TurnItem>,
    turns: Vec<Turn>,
    markdown: HashMap<String, Entity<Markdown>>,
    expanded_turns: HashSet<String>,
    group_overrides: HashMap<String, bool>,
    expanded_steps: HashSet<String>,
    full_output: HashSet<String>,
    expanded_users: HashSet<String>,
    /// Items fetched in full because the stream left their output out, keyed by id and revision.
    full_items: HashMap<String, TurnItem>,
    /// Output streaming in from commands that are still running, by item id.
    live_output: HashMap<String, LiveOutput>,
    /// Running commands the user folded; they open by default so their output shows live.
    collapsed_steps: HashSet<String>,
    completion: Option<completion::Completion>,
    /// The token whose suggestions the user closed, so they stay closed while it is edited.
    dismissed_token: Option<completion::Token>,
    file_search: Task<()>,
    /// Files waiting to go with the next message.
    attachments: Vec<PendingAttachment>,
    /// Images from sent messages, by attachment id, downloaded once from the server.
    sent_images: HashMap<String, Option<Arc<gpui::Image>>>,
    /// The user message whose "edit from here" is waiting for confirmation.
    confirm_rewind: Option<String>,
    fetch_tasks: HashMap<String, Task<()>>,
    copied: Option<SharedString>,
    copied_reset: Option<Task<()>>,
    answers: HashMap<String, HashMap<String, Vec<String>>>,
    answer_editors: HashMap<String, Entity<Editor>>,
    todos_expanded: bool,
    tick: Option<Task<()>>,
    scroll: ScrollHandle,
    /// Keeps the newest output in view until the user scrolls up, and again once they return.
    following: bool,
    selection: Option<ModelSelection>,
    runtime_mode: String,
    interaction_mode: String,
    /// Choices sent to an existing thread that its record does not show yet.
    pending_selection: Option<ModelSelection>,
    pending_runtime_mode: Option<String>,
    pending_interaction_mode: Option<String>,
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

#[derive(Clone)]
enum AttachmentSource {
    Path(PathBuf),
    Clipboard(Arc<gpui::Image>),
}

/// A file attached in the composer, uploaded when the message is sent.
#[derive(Clone)]
struct PendingAttachment {
    name: String,
    source: AttachmentSource,
    is_image: bool,
}

/// The tail of a running command's output, as T3 Code streams it.
struct LiveOutput {
    text: String,
    truncated: bool,
    _task: Task<()>,
}

impl LiveOutput {
    const MAX_CHARS: usize = 64 * 1024;

    fn apply(&mut self, frame: &Value) {
        let text = frame.get("text").and_then(Value::as_str).unwrap_or("");
        match frame.get("kind").and_then(Value::as_str) {
            Some("replace") => self.text = text.to_string(),
            _ => self.text.push_str(text),
        }
        self.truncated |= frame.get("truncated").and_then(Value::as_bool) == Some(true);
        if self.text.len() > Self::MAX_CHARS {
            let mut cut = self.text.len() - Self::MAX_CHARS;
            while !self.text.is_char_boundary(cut) {
                cut += 1;
            }
            self.text.drain(..cut);
            self.truncated = true;
        }
    }
}

fn revision_key(item: &TurnItem) -> String {
    format!(
        "{}@{}",
        item.id,
        item.str_field("updatedAt").unwrap_or_default()
    )
}

/// The thread stream leaves out command output and file diffs; they are fetched when shown.
fn needs_full_item(item: &TurnItem) -> bool {
    if item.is_running() {
        return false;
    }
    match item.kind.as_str() {
        "file_change" => item.str_field("diffStr").is_none() && item.str_field("newStr").is_none(),
        _ => item.bool_field("outputOmitted"),
    }
}

fn parse_time(text: Option<&str>) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(text?)
        .ok()
        .map(|time| time.with_timezone(&chrono::Utc))
}

fn format_timestamp(text: Option<&str>) -> Option<String> {
    parse_time(text).map(|time| {
        time.with_timezone(&chrono::Local)
            .format(&t!("%b %-d, %-I:%M %p"))
            .to_string()
    })
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
        let subscriptions = vec![
            cx.observe(&store, |this, store, cx| {
                let store = store.read(cx);
                if store.connection().is_some() && store.generation() != this.generation {
                    this.subscribe(cx);
                }
                cx.emit(ThreadViewEvent::Changed);
                cx.notify();
            }),
            cx.observe(&composer, |_, _, cx| cx.notify()),
            cx.subscribe(&composer, |this, _, event: &EditorEvent, cx| {
                if matches!(
                    event,
                    EditorEvent::Edited { .. } | EditorEvent::SelectionsChanged { .. }
                ) {
                    this.update_completion(cx);
                }
            }),
        ];
        let mut this = Self {
            store,
            workspace,
            languages,
            project_id,
            project_root,
            thread_id,
            docked: true,
            state: None,
            generation: 0,
            stream_task: None,
            send_task: None,
            composer,
            items: Vec::new(),
            turns: Vec::new(),
            markdown: HashMap::default(),
            expanded_turns: HashSet::default(),
            group_overrides: HashMap::default(),
            expanded_steps: HashSet::default(),
            full_output: HashSet::default(),
            expanded_users: HashSet::default(),
            full_items: HashMap::default(),
            live_output: HashMap::default(),
            collapsed_steps: HashSet::default(),
            completion: None,
            dismissed_token: None,
            file_search: Task::ready(()),
            attachments: Vec::new(),
            sent_images: HashMap::default(),
            confirm_rewind: None,
            fetch_tasks: HashMap::default(),
            copied: None,
            copied_reset: None,
            answers: HashMap::default(),
            answer_editors: HashMap::default(),
            todos_expanded: false,
            tick: None,
            scroll: ScrollHandle::new(),
            following: true,
            selection: default_selection,
            runtime_mode: DEFAULT_RUNTIME_MODE.to_string(),
            interaction_mode: DEFAULT_INTERACTION_MODE.to_string(),
            pending_selection: None,
            pending_runtime_mode: None,
            pending_interaction_mode: None,
            error: None,
            focus_handle: cx.focus_handle(),
            _subscriptions: subscriptions,
        };
        this.store.update(cx, |store, cx| store.ensure_connected(cx));
        this.subscribe(cx);
        this
    }

    pub fn thread_id(&self) -> Option<&str> {
        self.thread_id.as_deref()
    }

    pub fn set_docked(&mut self, docked: bool, cx: &mut Context<Self>) {
        self.docked = docked;
        cx.notify();
    }

    pub fn title(&self, cx: &App) -> SharedString {
        self.thread_id
            .as_deref()
            .and_then(|thread_id| self.store.read(cx).shell().threads.get(thread_id))
            .map(|thread| SharedString::from(thread.title.clone()))
            .unwrap_or_else(|| t!("New thread"))
    }

    pub fn needs_input(&self, _: &App) -> bool {
        self.state
            .as_ref()
            .is_some_and(|state| state.runtime_requests.iter().any(|request| request.is_pending()))
    }

    pub fn provider_driver(&self, cx: &App) -> Option<String> {
        self.current_provider(cx).map(|provider| provider.driver)
    }

    pub fn composer_focus_handle(&self, cx: &App) -> FocusHandle {
        self.composer.focus_handle(cx)
    }

    pub fn is_working(&self) -> bool {
        self.active_run_id().is_some()
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
        match state.apply(item) {
            Applied::Unchanged => return,
            Applied::NeedsResubscribe => {
                self.subscribe(cx);
                return;
            }
            Applied::Changed => {}
        }
        self.rebuild(cx);
        cx.emit(ThreadViewEvent::Changed);
        cx.notify();
    }

    fn rebuild(&mut self, cx: &mut Context<Self>) {
        let Some(state) = self.state.as_ref() else {
            return;
        };
        self.items = state
            .inherited
            .iter()
            .map(|inherited| inherited.item.clone())
            .chain(state.timeline().cloned())
            .collect();
        self.turns = group_turns(&self.items);
        let running = self
            .items
            .iter()
            .filter(|item| item.is_running())
            .map(|item| item.id.clone())
            .collect::<HashSet<_>>();
        self.live_output.retain(|id, _| running.contains(id));
        self.settle_pending();
        self.fetch_sent_images(cx);
        let sources = self
            .items
            .iter()
            .filter_map(|item| {
                let text = match item.kind.as_str() {
                    "assistant_message" | "user_message" | "reasoning" => item.text(),
                    "proposed_plan" => item.str_field("markdown").unwrap_or(""),
                    _ => return None,
                };
                Some((item.id.clone(), item.kind == "user_message", text.to_string()))
            })
            .collect::<Vec<_>>();
        for (id, is_user, text) in sources {
            match self.markdown.get(&id) {
                Some(markdown) => {
                    if markdown.read(cx).source().as_ref() != text {
                        markdown.update(cx, |markdown, cx| markdown.replace(text, cx));
                    }
                }
                None => {
                    let markdown = if is_user {
                        cx.new(|cx| {
                            Markdown::new_with_options(
                                text.into(),
                                None,
                                None,
                                MarkdownOptions {
                                    parse_block_quotes: true,
                                    ..Default::default()
                                },
                                cx,
                            )
                        })
                    } else {
                        let languages = self.languages.clone();
                        cx.new(|cx| Markdown::new(text.into(), Some(languages), None, cx))
                    };
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

    fn run(&self, run_id: Option<&str>) -> Option<&t3_client::protocol::Run> {
        let run_id = run_id?;
        self.state
            .as_ref()?
            .runs
            .iter()
            .rev()
            .find(|run| run.id == run_id)
    }

    fn turn_is_live(&self, turn_index: usize) -> bool {
        let Some(active) = self.active_run_id() else {
            return false;
        };
        let Some(turn) = self.turns.get(turn_index) else {
            return false;
        };
        match turn.run_id.as_deref() {
            Some(run_id) => run_id == active,
            None => turn_index + 1 == self.turns.len(),
        }
    }

    fn send(&mut self, _: &Send, window: &mut Window, cx: &mut Context<Self>) {
        self.submit(false, window, cx);
    }

    fn steer(&mut self, _: &Steer, window: &mut Window, cx: &mut Context<Self>) {
        self.submit(true, window, cx);
    }

    /// Sends the composer's message. While the agent works it is queued after the current turn,
    /// or with `steer`, read by the agent between the current turn's steps.
    fn submit(&mut self, steer: bool, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.composer.read(cx).text(cx);
        if (text.trim().is_empty() && self.attachments.is_empty()) || self.send_task.is_some() {
            return;
        }
        if self.thread_id.is_none() && self.selection.is_none() {
            self.error = Some(t!("Choose a model first"));
            cx.notify();
            return;
        }
        self.error = None;
        self.composer
            .update(cx, |composer, cx| composer.clear(window, cx));
        self.following = true;
        let pending = std::mem::take(&mut self.attachments);
        self.send_task = Some(cx.spawn_in(window, async move |this, cx| {
            let mut uploaded = Vec::with_capacity(pending.len());
            for attachment in &pending {
                let bytes = match &attachment.source {
                    AttachmentSource::Clipboard(image) => Ok(image.bytes.clone()),
                    AttachmentSource::Path(path) => {
                        let path = path.clone();
                        cx.background_spawn(async move { std::fs::read(&path) })
                            .await
                            .map_err(anyhow::Error::from)
                    }
                };
                let upload = bytes.and_then(|bytes| {
                    this.update(cx, |this, cx| {
                        this.store
                            .read(cx)
                            .upload_attachment(attachment.name.clone(), bytes, cx)
                    })
                });
                let result = match upload {
                    Ok(task) => task.await,
                    Err(error) => Err(error),
                };
                match result {
                    Ok(attachment) => uploaded.push(attachment),
                    Err(error) => {
                        this.update_in(cx, |this, window, cx| {
                            this.send_task = None;
                            this.restore_draft(&text, pending.clone(), error, window, cx);
                        })
                        .ok();
                        return;
                    }
                }
            }
            let sent = this.update(cx, |this, cx| this.dispatch_message(&text, &uploaded, steer, cx));
            let result = match sent {
                Ok(task) => task.await,
                Err(error) => Err(error),
            };
            this.update_in(cx, |this, window, cx| {
                this.send_task = None;
                match result {
                    Ok(Some(thread_id)) => {
                        this.thread_id = Some(thread_id);
                        this.subscribe(cx);
                        cx.emit(ThreadViewEvent::Launched);
                    }
                    Ok(None) => {}
                    Err(error) => this.restore_draft(&text, pending, error, window, cx),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// Sends the message to this thread, or starts the thread with it. Resolves to the new
    /// thread's id when it started one.
    fn dispatch_message(
        &self,
        text: &str,
        attachments: &[ChatAttachment],
        steer: bool,
        cx: &App,
    ) -> Task<anyhow::Result<Option<String>>> {
        let store = self.store.read(cx);
        match self.thread_id.as_deref() {
            Some(thread_id) => {
                let delivery = match self.active_run_id() {
                    Some(run_id) if steer => Delivery::Steer { run_id },
                    Some(_) => Delivery::Queue,
                    None => Delivery::StartNow,
                };
                let sent = store.dispatch(
                    commands::send_message(thread_id, text, attachments, &delivery),
                    cx,
                );
                cx.background_spawn(async move { sent.await.map(|_| None) })
            }
            None => {
                let Some(selection) = self.selection.as_ref() else {
                    return Task::ready(Err(anyhow::anyhow!(t!("Choose a model first"))));
                };
                let launched = store.launch_thread(
                    commands::launch_thread(
                        &self.project_id,
                        selection,
                        &self.runtime_mode,
                        &self.interaction_mode,
                        text,
                        attachments,
                    ),
                    cx,
                );
                cx.background_spawn(async move { launched.await.map(Some) })
            }
        }
    }

    fn restore_draft(
        &mut self,
        text: &str,
        attachments: Vec<PendingAttachment>,
        error: anyhow::Error,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.error = Some(format!("{error:#}").into());
        if self.composer.read(cx).text(cx).is_empty() {
            self.composer
                .update(cx, |composer, cx| composer.set_text(text, window, cx));
        }
        if self.attachments.is_empty() {
            self.attachments = attachments;
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

    fn copy(&mut self, key: SharedString, text: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.copied = Some(key);
        self.copied_reset = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(COPIED_FEEDBACK).await;
            this.update(cx, |this, cx| {
                this.copied = None;
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn worktree_root(&self) -> PathBuf {
        self.state
            .as_ref()
            .and_then(|state| state.thread.as_ref())
            .and_then(|thread| thread.worktree_path.as_deref())
            .map(PathBuf::from)
            .unwrap_or_else(|| self.project_root.clone())
    }

    fn open_file(&self, file_name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let path = Path::new(file_name);
        let path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.worktree_root().join(path)
        };
        self.workspace
            .update(cx, |workspace, cx| {
                workspace
                    .open_abs_path(path, Default::default(), window, cx)
                    .detach_and_log_err(cx);
            })
            .log_err();
    }

    fn opens_by_default(item: &TurnItem) -> bool {
        item.kind == "command_execution" && item.is_running() && !is_workspace_setup(item)
    }

    fn is_step_open(&self, item: &TurnItem) -> bool {
        if Self::opens_by_default(item) {
            !self.collapsed_steps.contains(&item.id)
        } else {
            self.expanded_steps.contains(&item.id)
        }
    }

    fn toggle_step_item(&mut self, item: &TurnItem, cx: &mut Context<Self>) {
        if Self::opens_by_default(item) {
            if !self.collapsed_steps.remove(&item.id) {
                self.collapsed_steps.insert(item.id.clone());
            }
            cx.notify();
        } else {
            self.toggle_step(&item.id, cx);
        }
    }

    /// Follows a running command's output while its row is open.
    fn ensure_live_output(&mut self, item: &TurnItem, cx: &mut Context<Self>) {
        if !Self::opens_by_default(item) || self.live_output.contains_key(&item.id) {
            return;
        }
        let (Some(thread_id), Some(connection)) =
            (self.thread_id.clone(), self.store.read(cx).connection())
        else {
            return;
        };
        let mut stream = match connection.subscribe_command_output(&thread_id, &item.id) {
            Ok(stream) => stream,
            Err(error) => {
                log::warn!("could not follow T3 Code command output: {error:#}");
                return;
            }
        };
        let item_id = item.id.clone();
        let task = cx.spawn(async move |this, cx| {
            while let Some(frame) = stream.next().await {
                let frame = match frame {
                    Ok(frame) => frame,
                    Err(error) => {
                        log::warn!("T3 Code command output stopped: {error:#}");
                        break;
                    }
                };
                let updated = this.update(cx, |this, cx| {
                    if let Some(live) = this.live_output.get_mut(&item_id) {
                        live.apply(&frame);
                        cx.notify();
                    }
                });
                if updated.is_err() {
                    break;
                }
            }
        });
        self.live_output.insert(
            item.id.clone(),
            LiveOutput {
                text: String::new(),
                truncated: false,
                _task: task,
            },
        );
    }

    fn toggle_step(&mut self, item_id: &str, cx: &mut Context<Self>) {
        if !self.expanded_steps.remove(item_id) {
            self.expanded_steps.insert(item_id.to_string());
        }
        cx.notify();
    }

    /// The item with its full output, fetching it when the stream left the output out.
    fn full_item<'a>(&'a mut self, item: &'a TurnItem, cx: &mut Context<Self>) -> &'a TurnItem {
        if !needs_full_item(item) {
            return item;
        }
        let key = revision_key(item);
        if !self.full_items.contains_key(&key) && !self.fetch_tasks.contains_key(&key) {
            if let Some(thread_id) = self.thread_id.clone() {
                let task = self
                    .store
                    .read(cx)
                    .get_turn_item(thread_id, item.id.clone(), cx);
                let fetch_key = key.clone();
                self.fetch_tasks.insert(
                    key.clone(),
                    cx.spawn(async move |this, cx| {
                        let result = task.await;
                        this.update(cx, |this, cx| {
                            this.fetch_tasks.remove(&fetch_key);
                            match result {
                                Ok(Some(full)) => {
                                    this.full_items.insert(fetch_key, full);
                                }
                                Ok(None) => {}
                                Err(error) => {
                                    this.error = Some(format!("{error:#}").into());
                                }
                            }
                            cx.notify();
                        })
                        .ok();
                    }),
                );
            }
        }
        self.full_items.get(&key).unwrap_or(item)
    }

    fn ensure_tick(&mut self, cx: &mut Context<Self>) {
        if !self.is_working() {
            self.tick = None;
            return;
        }
        if self.tick.is_some() {
            return;
        }
        self.tick = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let working = this
                    .update(cx, |this, cx| {
                        cx.notify();
                        this.is_working()
                    })
                    .unwrap_or(false);
                if !working {
                    break;
                }
            }
            this.update(cx, |this, _| this.tick = None).ok();
        }));
    }

    fn message_style(window: &Window, cx: &App) -> MarkdownStyle {
        let colors = cx.theme().colors();
        let accent = accent(cx);
        let mut style = MarkdownStyle::themed(MarkdownFont::Editor, window, cx);
        style.base_text_style.font_size = ui(14.).into();
        style.container_style.text.font_size = Some(ui(14.).into());
        style.base_text_style.line_height = ui(22.).into();
        style.base_text_style.color = colors.text;
        style.paragraph_spacing = px(MD_BLOCK_GAP);
        style.soft_break_as_hard_break = true;
        style.list_spacing = px(MD_BLOCK_GAP);
        style.table_cell_padding = point(px(10.), px(8.));
        style.container_style.margin.bottom = Some(px(-MD_BLOCK_GAP).into());
        style.heading.margin.top = Some(px(0.).into());
        style.heading.margin.bottom = Some(px(MD_BLOCK_GAP).into());
        let is_light = cx.theme().appearance.is_light();
        let code = code_text_style(cx);
        style.inline_code = TextStyleRefinement {
            color: Some(accent),
            background_color: Some(accent.opacity(if is_light { 0.10 } else { 0.12 })),
            ..code.clone()
        };
        style.link = TextStyleRefinement {
            color: Some(colors.text),
            underline: Some(UnderlineStyle {
                thickness: px(1.),
                color: Some(colors.text_muted),
                wavy: false,
            }),
            ..Default::default()
        };
        let mut code_block = StyleRefinement::default();
        code_block.padding.left = Some(px(12.).into());
        code_block.padding.right = Some(px(12.).into());
        code_block.padding.top = Some(px(10.).into());
        code_block.padding.bottom = Some(px(10.).into());
        code_block.text = TextStyleRefinement {
            font_size: Some(px(12.5).into()),
            line_height: Some(px(18.).into()),
            ..code
        };
        style.code_block = code_block;
        let heading = |size: f32, line_height: f32| {
            Some(TextStyleRefinement {
                font_size: Some(ui(size).into()),
                line_height: Some(ui(line_height).into()),
                font_weight: Some(FontWeight::SEMIBOLD),
                color: Some(colors.text),
                ..Default::default()
            })
        };
        style.heading_level_styles = Some(HeadingLevelStyles {
            h1: heading(18., 26.),
            h2: heading(16., 24.),
            h3: heading(15., 22.),
            h4: heading(14., 22.),
            h5: heading(14., 22.),
            h6: heading(14., 22.),
        });
        style.rule_color = colors.border;
        style.block_quote_border_color = accent.opacity(0.6);
        style.block_quote_border_width = px(2.);
        style.block_quote = TextStyleRefinement {
            color: Some(colors.text_muted),
            ..Default::default()
        };
        style.selection_background_color = accent.opacity(if is_light { 0.24 } else { 0.35 });
        style
    }

    fn thought_style(window: &Window, cx: &App) -> MarkdownStyle {
        let faint = text_faint(cx);
        let mut style = Self::message_style(window, cx);
        style.base_text_style.font_size = ui(13.).into();
        style.container_style.text.font_size = Some(ui(13.).into());
        style.base_text_style.line_height = px(20.).into();
        style.base_text_style.color = faint;
        style.paragraph_spacing = px(8.);
        style.list_spacing = px(8.);
        style.container_style.margin.bottom = Some(px(-8.).into());
        style.inline_code = TextStyleRefinement {
            color: Some(faint),
            ..code_text_style(cx)
        };
        style
    }

    fn user_style(window: &Window, cx: &App) -> MarkdownStyle {
        let mut style = Self::message_style(window, cx);
        style.base_text_style.line_height = ui(USER_LINE_HEIGHT).into();
        style.container_style.text.line_height = Some(ui(USER_LINE_HEIGHT).into());
        style.container_style.margin.bottom = None;
        style.paragraph_spacing = px(0.);
        style
    }

    fn markdown_element(&self, item_id: &str, style: MarkdownStyle, cx: &Context<Self>) -> Option<AnyElement> {
        let markdown = self.markdown.get(item_id)?.clone();
        let link_view = cx.entity().downgrade();
        Some(
            MarkdownElement::new(markdown, style)
                .code_block_renderer(CodeBlockRenderer::Default {
                    copy_button_visibility: CopyButtonVisibility::VisibleOnHover,
                    wrap_button_visibility: WrapButtonVisibility::VisibleOnHover,
                    border: true,
                })
                .on_url_click(move |url, window, cx| {
                    link_view
                        .update(cx, |this, cx| this.open_link(&url, window, cx))
                        .ok();
                })
                .into_any_element(),
        )
    }

    fn open_link(&self, url: &str, window: &mut Window, cx: &mut Context<Self>) {
        if url.contains("://") || url.starts_with("mailto:") {
            cx.open_url(url);
            return;
        }
        let path = url.split('#').next().unwrap_or(url);
        self.open_file(path, window, cx);
    }

    fn render_user(&self, index: usize, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(item) = self.items.get(index) else {
            return div().into_any_element();
        };
        let text = item.text().to_string();
        let id = item.id.clone();
        let collapsible = text.lines().count() > USER_COLLAPSED_LINES
            || text.chars().count() > USER_COLLAPSE_CHARS;
        let expanded = self.expanded_users.contains(&id);
        let collapsed = collapsible && !expanded;
        let colors = cx.theme().colors();
        let bubble = if cx.theme().appearance.is_light() {
            wash(0.05, cx)
        } else {
            wash(0.08, cx)
        };
        let group: SharedString = format!("t3-user-{id}").into();
        let body = self
            .markdown_element(&id, Self::user_style(window, cx), cx)
            .unwrap_or_else(|| text.clone().into_any_element());
        let copy_key: SharedString = format!("copy-user-{id}").into();
        let toggle_id = id.clone();
        let sent_attachments = self.render_sent_attachments(item, cx);
        let has_text = !text.trim().is_empty();
        v_flex()
            .group(group.clone())
            .w_full()
            .items_end()
            .children(sent_attachments)
            .child(
                v_flex()
                    .when(!has_text, |this| this.hidden())
                    .min_w_0()
                    .max_w(relative(0.88))
                    .px(px(14.))
                    .py(px(9.))
                    .rounded(px(16.))
                    .bg(bubble)
                    .text_size(ui(14.))
                    .line_height(ui(USER_LINE_HEIGHT))
                    .text_color(colors.text)
                    .map(|this| {
                        if collapsed {
                            this.child(
                                div()
                                    .max_h(ui(USER_LINE_HEIGHT * USER_COLLAPSED_LINES as f32))
                                    .overflow_hidden()
                                    .child(body),
                            )
                        } else {
                            this.child(body)
                        }
                    })
                    .when(collapsible, |this| {
                        this.child(
                            h_flex()
                                .id(SharedString::from(format!("t3-user-toggle-{id}")))
                                .mt(px(6.))
                                .gap(px(5.))
                                .text_size(ui(13.))
                                .text_color(colors.text_muted)
                                .cursor_pointer()
                                .hover(|style| style.text_color(colors.text))
                                .child(if expanded {
                                    t!("Show less")
                                } else {
                                    t!("Show more")
                                })
                                .child(
                                    icon(
                                        if expanded {
                                            IconName::AgentArrowUp
                                        } else {
                                            IconName::AgentArrowDown
                                        },
                                        12.,
                                    )
                                    .text_color(colors.text_muted),
                                )
                                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                    if !this.expanded_users.remove(&toggle_id) {
                                        this.expanded_users.insert(toggle_id.clone());
                                    }
                                    cx.notify();
                                })),
                        )
                    }),
            )
            .child(
                self.hover_strip(
                    h_flex().visible_on_hover(group),
                    item.str_field("startedAt"),
                    Some((copy_key, text)),
                    cx,
                )
                .justify_end()
                .children(self.render_rewind_button(item, cx)),
            )
            .children(
                (self.confirm_rewind.as_deref() == Some(item.id.as_str()))
                    .then(|| self.render_rewind_confirmation(item, cx)),
            )
            .into_any_element()
    }

    fn hover_strip(
        &self,
        strip: gpui::Div,
        at: Option<&str>,
        copy: Option<(SharedString, String)>,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let muted = cx.theme().colors().text_muted;
        strip
            .h(px(28.))
            .pt(px(4.))
            .w_full()
            .gap(px(SPACE_SM))
            .when_some(format_timestamp(at), |this, timestamp| {
                this.child(
                    div()
                        .text_size(ui(12.))
                        .text_color(muted.opacity(0.55))
                        .child(timestamp),
                )
            })
            .when_some(copy, |this, (key, text)| {
                let copied = self.copied.as_ref() == Some(&key);
                this.child(
                    icon_button(
                        ElementId::Name(key.clone()),
                        if copied {
                            IconName::AgentCheck
                        } else {
                            IconName::AgentCopy
                        },
                        muted,
                        cx,
                    )
                    .tooltip(Tooltip::text(t!("Copy message")))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        cx.stop_propagation();
                        this.copy(key.clone(), text.clone(), cx)
                    })),
                )
            })
    }

    fn can_rewind(&self, item: &TurnItem) -> bool {
        let (Some(state), Some(run_id)) = (self.state.as_ref(), item.run_id.as_deref()) else {
            return false;
        };
        state.checkpoint_before_run(run_id).is_some()
    }

    fn render_rewind_button(&self, item: &TurnItem, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.can_rewind(item) {
            return None;
        }
        let item_id = item.id.clone();
        let working = self.is_working();
        Some(
            icon_button(
                SharedString::from(format!("rewind-{item_id}")),
                IconName::AgentRestart,
                cx.theme().colors().text_muted,
                cx,
            )
            .tooltip(Tooltip::text(if working {
                t!("Interrupt the current turn before reverting checkpoints.")
            } else {
                t!("Edit from here")
            }))
            .when(working, |this| this.opacity(0.4).cursor_default())
            .when(!working, |this| {
                this.on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.confirm_rewind = Some(item_id.clone());
                    cx.notify();
                }))
            })
            .into_any_element(),
        )
    }

    fn render_rewind_confirmation(&self, item: &TurnItem, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.theme().colors();
        let button = |id: &str, label: SharedString, style: ButtonStyle, restore: Option<bool>| {
            let item = item.clone();
            Button::new(SharedString::from(format!("{id}-{}", item.id)), label)
                .style(style)
                .size(ButtonSize::Medium)
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| match restore {
                    Some(restore_files) => this.rewind(&item, restore_files, window, cx),
                    None => {
                        this.confirm_rewind = None;
                        cx.notify();
                    }
                }))
        };
        v_flex()
            .w_full()
            .mt(px(6.))
            .p(px(12.))
            .gap(px(10.))
            .rounded(px(12.))
            .border_1()
            .border_color(hairline(0.12, cx))
            .bg(colors.elevated_surface_background)
            .text_size(ui(13.))
            .child(
                div()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(colors.text)
                    .child(t!("Edit from here?")),
            )
            .child(div().text_color(colors.text_muted).child(t!(
                "Rewind the chat to before this message. Your prompt returns to the message box. “Revert files too” also undoes the file changes made since then."
            )))
            .child(
                h_flex()
                    .gap(px(6.))
                    .flex_wrap()
                    .child(button("rewind-files", t!("Revert files too"), ButtonStyle::Filled, Some(true)))
                    .child(button(
                        "rewind-keep",
                        t!("Revert and keep changes"),
                        ButtonStyle::Outlined,
                        Some(false),
                    ))
                    .child(button("rewind-cancel", t!("Cancel"), ButtonStyle::Subtle, None)),
            )
            .into_any_element()
    }

    fn rewind(&mut self, item: &TurnItem, restore_files: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.confirm_rewind = None;
        if self.is_working() {
            self.error = Some(t!("Interrupt the current turn before reverting checkpoints."));
            cx.notify();
            return;
        }
        let checkpoint = self
            .state
            .as_ref()
            .zip(item.run_id.as_deref())
            .and_then(|(state, run_id)| state.checkpoint_before_run(run_id))
            .cloned();
        let (Some(thread_id), Some(checkpoint)) = (self.thread_id.clone(), checkpoint) else {
            self.error = Some(t!("This point can't be restored."));
            cx.notify();
            return;
        };
        let command = commands::rollback_checkpoint(
            &thread_id,
            &checkpoint.scope_id,
            &checkpoint.id,
            restore_files,
        );
        let task = self.store.read(cx).dispatch(command, cx);
        let text = item.text().to_string();
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(_) => {
                        if this.composer.read(cx).text(cx).trim().is_empty() {
                            this.composer
                                .update(cx, |composer, cx| composer.set_text(text.clone(), window, cx));
                        }
                    }
                    Err(error) => this.error = Some(format!("{error:#}").into()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn render_reply(&self, index: usize, window: &Window, cx: &Context<Self>) -> AnyElement {
        let Some(item) = self.items.get(index) else {
            return div().into_any_element();
        };
        let body = self
            .markdown_element(&item.id, Self::message_style(window, cx), cx)
            .unwrap_or_else(|| item.text().to_string().into_any_element());
        div()
            .w_full()
            .min_w_0()
            .child(body)
            .with_animation(
                ElementId::Name(format!("t3-reply-fade-{}", item.id).into()),
                Animation::new(FADE_IN).with_easing(ease_out_quint()),
                |this, delta| this.opacity(delta),
            )
            .into_any_element()
    }

    /// A clickable summary line: the "Worked for" header of a turn, or a group of steps.
    fn summary_row(
        &self,
        id: ElementId,
        leading: Option<IconName>,
        title: AnyElement,
        open: bool,
        at: Option<String>,
        cx: &Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let colors = cx.theme().colors();
        let muted = colors.text_muted;
        let group: SharedString = format!("{id:?}-hover").into();
        h_flex()
            .id(id)
            .group(group.clone())
            .h(px(GROUP_HEADER_HEIGHT))
            .w_full()
            .min_w_0()
            .gap(px(8.))
            .cursor_pointer()
            .text_size(ui(13.))
            .text_color(muted)
            .hover(|style| style.text_color(colors.text))
            .when_some(leading, |this, leading| {
                this.child(icon(leading, 14.).text_color(muted))
            })
            .child(
                div()
                    .min_w_0()
                    .flex_shrink_0()
                    .max_w(relative(0.8))
                    .h(px(TOOL_LINE_HEIGHT))
                    .flex()
                    .items_center()
                    .overflow_hidden()
                    .child(title),
            )
            .child(
                icon(
                    if open {
                        IconName::AgentArrowDown
                    } else {
                        IconName::AgentArrowRight
                    },
                    12.,
                )
                .text_color(text_faint(cx)),
            )
            .child(div().flex_1())
            .when_some(at, |this, at| {
                this.child(
                    div()
                        .flex_none()
                        .text_size(ui(11.))
                        .text_color(text_faint(cx))
                        .visible_on_hover(group)
                        .child(at),
                )
            })
    }

    fn turn_key(&self, turn: &Turn) -> String {
        let index = turn.user.unwrap_or(turn.items.start);
        self.items
            .get(index)
            .map(|item| item.id.clone())
            .unwrap_or_else(|| format!("turn-{index}"))
    }

    fn render_worked(&self, turn: &Turn, cx: &mut Context<Self>) -> AnyElement {
        let run = self.run(turn.run_id.as_deref());
        let started = run
            .and_then(|run| parse_time(run.started_at.as_deref()))
            .or_else(|| {
                turn.user
                    .and_then(|user| self.items.get(user))
                    .and_then(|item| parse_time(item.str_field("startedAt")))
            });
        let finished_at = run.and_then(|run| run.completed_at.clone()).or_else(|| {
            turn.last_reply
                .and_then(|reply| self.items.get(reply))
                .and_then(|item| item.str_field("completedAt").map(str::to_string))
        });
        let finished = parse_time(finished_at.as_deref());
        let label = match (started, finished) {
            (Some(started), Some(finished)) if finished > started => t!(
                "Worked for {duration}",
                duration = format_elapsed((finished - started).num_seconds() as u64)
            )
            .to_string(),
            _ => t!("Worked").to_string(),
        };
        let key = self.turn_key(turn);
        let open = self.expanded_turns.contains(&key);
        div()
            .w_full()
            .border_b_1()
            .border_color(hairline(0.08, cx))
            .child(
                self.summary_row(
                    ElementId::Name(format!("t3-worked-{key}").into()),
                    None,
                    SharedString::from(label).into_any_element(),
                    open,
                    format_timestamp(finished_at.as_deref()),
                    cx,
                )
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    if !this.expanded_turns.remove(&key) {
                        this.expanded_turns.insert(key.clone());
                    }
                    cx.notify();
                })),
            )
            .into_any_element()
    }

    fn render_work_group(
        &mut self,
        range: Range<usize>,
        is_live: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let root = self.worktree_root();
        let group = self.items.get(range).unwrap_or_default().to_vec();
        let Some(first) = group.first() else {
            return div().into_any_element();
        };
        let group_key = format!("t3-group-{}", first.id);
        let open = self
            .group_overrides
            .get(&group_key)
            .copied()
            .unwrap_or(is_live);
        let tallies = group
            .iter()
            .map(|item| (describe(item, &root).tally, item.is_failed()))
            .collect::<Vec<_>>();
        let summary: SharedString = group_summary(&tallies).into();
        let colors = cx.theme().colors();
        let title = if is_live {
            shimmer_title(
                summary,
                ElementId::Name(format!("{group_key}-shimmer").into()),
                colors.text_muted,
                colors.text,
            )
        } else {
            summary.into_any_element()
        };
        let at = group
            .last()
            .and_then(|item| item.str_field("completedAt").or_else(|| item.str_field("startedAt")))
            .map(str::to_string);
        let only_thought = matches!(group.as_slice(), [only] if only.kind == "reasoning");
        let header = self
            .summary_row(
                ElementId::Name(format!("{group_key}-header").into()),
                Some(if only_thought {
                    IconName::AgentChatRoundLine
                } else {
                    IconName::AgentWidget
                }),
                title,
                open,
                format_timestamp(at.as_deref()),
                cx,
            )
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.group_overrides.insert(group_key.clone(), !open);
                cx.notify();
            }));
        if only_thought && let Some(only) = group.first() {
            let id = only.id.clone();
            return v_flex()
                .w_full()
                .child(header)
                .when(open, |this| {
                    this.children(
                        self.markdown_element(&id, Self::thought_style(window, cx), cx)
                            .map(|thought| div().pl(px(STEP_INDENT)).pb(px(6.)).child(thought)),
                    )
                })
                .into_any_element();
        }
        let steps = if open {
            group
                .iter()
                .map(|item| self.render_step(item, window, cx))
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        v_flex()
            .w_full()
            .child(header)
            .when(open, |this| this.children(steps))
            .into_any_element()
    }

    fn render_step(&mut self, item: &TurnItem, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let root = self.worktree_root();
        let step = describe(item, &root);
        let colors = cx.theme().colors().clone();
        let danger = cx.theme().status().error;
        let failed = item.is_failed();
        let running = item.is_running();
        let tint = if failed { danger } else { colors.text_muted };
        let key = item.id.clone();
        let expanded = self.is_step_open(item);
        if expanded {
            self.ensure_live_output(item, cx);
        }
        let toggled = item.clone();
        if item.kind == "file_change" {
            // The stream leaves diffs out; fetching them early lets the row show its line counts.
            self.full_item(item, cx);
        }
        let full = self.full_items.get(&revision_key(item)).unwrap_or(item);
        let stats = (item.kind == "file_change")
            .then(|| match (full.i64_field("additions"), full.i64_field("deletions")) {
                (Some(additions), Some(deletions)) => Some((additions as usize, deletions as usize)),
                _ => FileDiff::of(full).map(|diff| (diff.additions, diff.deletions)),
            })
            .flatten();
        let open_path = match item.kind.as_str() {
            "file_change" => item.str_field("fileName").map(str::to_string),
            "dynamic_tool" if step.tally == Tally::Read => item
                .fields
                .get("input")
                .and_then(|input| input.get("file_path").or_else(|| input.get("path")))
                .and_then(Value::as_str)
                .map(str::to_string),
            _ => None,
        };
        // Commands and edits read as themselves, like T3 Code shows them; other steps say what
        // kind of step they are.
        let prefix = match step.tally {
            Tally::Command | Tally::Edit(_) if !step.detail.is_empty() => None,
            _ => Some(step.label.clone()),
        };
        let body = expanded.then(|| self.render_step_detail(item, window, cx));
        let hover_group: SharedString = format!("t3-step-{key}").into();
        let row = h_flex()
            .id(ElementId::Name(format!("{key}-row").into()))
            .group(hover_group.clone())
            .h(px(STEP_ROW_HEIGHT))
            .w_full()
            .min_w_0()
            .gap(px(8.))
            .px(px(4.))
            .rounded(px(6.))
            .cursor_pointer()
            .text_size(ui(13.))
            .hover(|style| style.bg(ink(0.04, cx)))
            .child(icon(step.icon, 14.).text_color(tint))
            .when_some(prefix, |this, prefix| {
                this.child(div().flex_none().text_color(tint).child(prefix))
            })
            .when(!step.detail.is_empty(), |this| {
                this.child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_color(if failed { danger } else { colors.text.opacity(0.85) })
                        .child(step.detail.clone()),
                )
            })
            .when_some(stats, |this, (additions, deletions)| {
                let status = cx.theme().status();
                this.child(
                    h_flex()
                        .flex_none()
                        .gap(px(4.))
                        .font(code_font(cx))
                        .text_size(px(11.))
                        .when(additions > 0, |this| {
                            this.child(div().text_color(status.created).child(format!("+{additions}")))
                        })
                        .when(deletions > 0, |this| {
                            this.child(div().text_color(status.deleted).child(format!("−{deletions}")))
                        }),
                )
            })
            .when(running, |this| {
                this.child(spinning_icon(
                    ElementId::Name(format!("{key}-spinner").into()),
                    IconName::LoadCircle,
                    12.,
                    text_faint(cx),
                ))
            })
            .child(div().flex_1())
            .when_some(open_path, |this, path| {
                this.child(
                    icon_button(
                        ElementId::Name(format!("{key}-open").into()),
                        IconName::AgentArrowUpRight,
                        colors.text_muted,
                        cx,
                    )
                    .size(px(20.))
                    .visible_on_hover(hover_group.clone())
                    .tooltip(Tooltip::text(t!("Open file")))
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        cx.stop_propagation();
                        this.open_file(&path, window, cx)
                    })),
                )
            })
            .child(
                icon(
                    if expanded {
                        IconName::AgentArrowDown
                    } else {
                        IconName::AgentArrowRight
                    },
                    12.,
                )
                .text_color(text_faint(cx)),
            )
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.toggle_step_item(&toggled, cx)
            }));
        v_flex()
            .w_full()
            .child(row)
            .children(body.map(|body| div().pl(px(STEP_INDENT)).pt(px(2.)).child(body)))
            .with_animation(
                ElementId::Name(format!("t3-step-fade-{}", item.id).into()),
                Animation::new(FADE_IN).with_easing(ease_out_quint()),
                |this, delta| this.opacity(delta),
            )
            .into_any_element()
    }

    fn render_show_full(&self, id: String, label: SharedString, item_id: &str, cx: &Context<Self>) -> AnyElement {
        let item_id = item_id.to_string();
        let muted = cx.theme().colors().text_muted;
        div()
            .id(SharedString::from(id))
            .h(px(24.))
            .flex()
            .items_center()
            .text_size(px(TOOL_TEXT_SIZE))
            .text_color(text_faint(cx))
            .cursor_pointer()
            .hover(|style| style.text_color(muted))
            .child(label)
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                cx.stop_propagation();
                this.full_output.insert(item_id.clone());
                cx.notify();
            }))
            .into_any_element()
    }

    fn code_box(&self, sections: Vec<AnyElement>, cx: &App) -> AnyElement {
        let colors = cx.theme().colors();
        v_flex()
            .w_full()
            .min_w_0()
            .mb(px(6.))
            .overflow_hidden()
            .rounded(px(8.))
            .border_1()
            .border_color(colors.border_variant)
            .bg(colors.editor_background)
            .font(code_font(cx))
            .text_size(px(TOOL_TEXT_SIZE))
            .children(sections)
            .into_any_element()
    }

    fn code_lines(
        &self,
        text: &str,
        max_lines: usize,
        color: Hsla,
        show_full: Option<(String, SharedString, &str)>,
        cx: &Context<Self>,
    ) -> AnyElement {
        let clipped = clip_lines(text, max_lines);
        let hidden = clipped.hidden;
        v_flex()
            .w_full()
            .min_w_0()
            .px(px(10.))
            .py(px(6.))
            .children(clipped.lines.into_iter().map(move |line| {
                div()
                    .min_h(px(OUTPUT_LINE_HEIGHT))
                    .w_full()
                    .min_w_0()
                    .text_color(color)
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .child(line)
            }))
            .when(hidden > 0, |this| {
                this.child(
                    div()
                        .h(px(OUTPUT_LINE_HEIGHT))
                        .text_color(text_faint(cx))
                        .child(t!("… {count} more lines", count = hidden)),
                )
                .children(show_full.map(|(id, label, item_id)| {
                    self.render_show_full(id, label, item_id, cx)
                }))
            })
            .into_any_element()
    }

    fn render_step_detail(&mut self, item: &TurnItem, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let item = self.full_item(item, cx).clone();
        let colors = cx.theme().colors();
        let faint = text_faint(cx);
        let full = self.full_output.contains(&item.id);
        let output_lines = if full { usize::MAX } else { OUTPUT_MAX_LINES };
        let loading = needs_full_item(&item) && self.fetch_tasks.contains_key(&revision_key(&item));
        let separator = || {
            div()
                .h(px(1.))
                .w_full()
                .bg(colors.border_variant)
                .into_any_element()
        };
        match item.kind.as_str() {
            "command_execution" if !is_workspace_setup(&item) => {
                let input = unwrap_shell(item.str_field("input").unwrap_or(""));
                let live = self.live_output.get(&item.id).filter(|_| item.is_running());
                let output = match live {
                    Some(live) if live.truncated => format!("…\n{}", strip_ansi(&live.text)),
                    Some(live) => strip_ansi(&live.text),
                    None => strip_ansi(item.str_field("output").unwrap_or("")),
                };
                let mut sections = vec![self.code_lines(
                    &format!("$ {input}"),
                    if full { usize::MAX } else { INPUT_MAX_LINES },
                    colors.text,
                    None,
                    cx,
                )];
                if !output.trim().is_empty() {
                    sections.push(separator());
                    sections.push(self.code_lines(
                        &output,
                        output_lines,
                        colors.text_muted,
                        Some((
                            format!("t3-full-output-{}", item.id),
                            t!("Show full output"),
                            &item.id,
                        )),
                        cx,
                    ));
                } else if loading {
                    sections.push(separator());
                    sections.push(
                        div()
                            .px(px(10.))
                            .py(px(6.))
                            .text_color(faint)
                            .child(t!("Loading output…"))
                            .into_any_element(),
                    );
                }
                if let Some(code) = item.i64_field("exitCode").filter(|code| *code != 0) {
                    sections.push(separator());
                    sections.push(
                        div()
                            .px(px(10.))
                            .py(px(4.))
                            .text_color(cx.theme().status().error)
                            .child(t!("Exit code {code}", code = code))
                            .into_any_element(),
                    );
                }
                self.code_box(sections, cx)
            }
            "file_change" => match FileDiff::of(&item).filter(|diff| !diff.is_empty()) {
                Some(diff) => div().w_full().mb(px(6.)).child(diff.render(cx)).into_any_element(),
                None if loading => div()
                    .pb(px(6.))
                    .text_size(px(TOOL_TEXT_SIZE))
                    .text_color(faint)
                    .child(t!("Loading…"))
                    .into_any_element(),
                None => div()
                    .pb(px(6.))
                    .text_size(px(TOOL_TEXT_SIZE))
                    .text_color(faint)
                    .child(t!("No diff recorded for this change."))
                    .into_any_element(),
            },
            "file_search" => {
                let root = self.worktree_root();
                let results = item
                    .fields
                    .get("results")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                if results.is_empty() {
                    return div()
                        .pb(px(6.))
                        .text_size(px(TOOL_TEXT_SIZE))
                        .text_color(faint)
                        .child(t!("No matches"))
                        .into_any_element();
                }
                let shown = if full { results.len() } else { OUTPUT_MAX_LINES };
                let hidden = results.len().saturating_sub(shown);
                let rows = results
                    .iter()
                    .take(shown)
                    .enumerate()
                    .filter_map(|(position, result)| {
                        let file = result.get("fileName")?.as_str()?.to_string();
                        let line = result.get("line").and_then(Value::as_u64);
                        let preview = result
                            .get("preview")
                            .and_then(Value::as_str)
                            .map(single_line)
                            .unwrap_or_default();
                        let location = match line {
                            Some(line) => format!("{}:{line}", display_path(&file, &root)),
                            None => display_path(&file, &root),
                        };
                        Some(
                            h_flex()
                                .id(SharedString::from(format!("{}-result-{position}", item.id)))
                                .min_h(px(OUTPUT_LINE_HEIGHT))
                                .w_full()
                                .min_w_0()
                                .gap(px(8.))
                                .cursor_pointer()
                                .hover(|style| style.bg(ink(0.04, cx)))
                                .child(div().flex_none().text_color(colors.text).child(location))
                                .child(
                                    div()
                                        .min_w_0()
                                        .truncate()
                                        .text_color(colors.text_muted)
                                        .child(preview),
                                )
                                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                                    this.open_file(&file, window, cx)
                                }))
                                .into_any_element(),
                        )
                    })
                    .collect::<Vec<_>>();
                let mut list = vec![
                    v_flex()
                        .px(px(10.))
                        .py(px(6.))
                        .children(rows)
                        .into_any_element(),
                ];
                if hidden > 0 {
                    list.push(
                        div()
                            .px(px(10.))
                            .child(self.render_show_full(
                                format!("t3-full-results-{}", item.id),
                                t!("Show all {count} matches", count = results.len()),
                                &item.id,
                                cx,
                            ))
                            .into_any_element(),
                    );
                }
                self.code_box(list, cx)
            }
            "web_search" => {
                let results = item
                    .fields
                    .get("results")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                v_flex()
                    .w_full()
                    .pb(px(6.))
                    .gap(px(4.))
                    .text_size(px(TOOL_TEXT_SIZE))
                    .children(results.into_iter().enumerate().filter_map(|(position, result)| {
                        let url = result.get("url")?.as_str()?.to_string();
                        let title = result
                            .get("title")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                            .unwrap_or_else(|| url.clone());
                        Some(
                            v_flex()
                                .id(SharedString::from(format!("{}-web-{position}", item.id)))
                                .min_w_0()
                                .cursor_pointer()
                                .child(
                                    div()
                                        .truncate()
                                        .text_color(colors.text)
                                        .hover(|style| style.underline())
                                        .child(title),
                                )
                                .child(div().truncate().text_color(faint).child(url.clone()))
                                .on_click(move |_, _, cx| cx.open_url(&url)),
                        )
                    }))
                    .into_any_element()
            }
            "reasoning" => div()
                .pb(px(6.))
                .children(self.markdown_element(&item.id, Self::thought_style(window, cx), cx))
                .into_any_element(),
            "todo_list" => div().pb(px(6.)).child(self.render_todo_steps(&item, cx)).into_any_element(),
            "subagent" => {
                let mut sections = vec![self.code_lines(
                    item.str_field("prompt").unwrap_or(""),
                    if full { usize::MAX } else { INPUT_MAX_LINES },
                    colors.text_muted,
                    None,
                    cx,
                )];
                if let Some(result) = item.str_field("result").filter(|result| !result.is_empty()) {
                    sections.push(separator());
                    sections.push(self.code_lines(
                        result,
                        output_lines,
                        colors.text,
                        Some((
                            format!("t3-full-result-{}", item.id),
                            t!("Show full output"),
                            &item.id,
                        )),
                        cx,
                    ));
                }
                self.code_box(sections, cx)
            }
            _ => {
                let input = item
                    .fields
                    .get("input")
                    .map(value_text)
                    .unwrap_or_default();
                let output = item
                    .fields
                    .get("output")
                    .map(value_text)
                    .unwrap_or_default();
                let mut sections = Vec::new();
                if !input.trim().is_empty() {
                    sections.push(self.code_lines(
                        &input,
                        if full { usize::MAX } else { INPUT_MAX_LINES },
                        colors.text,
                        None,
                        cx,
                    ));
                }
                if !output.trim().is_empty() {
                    if !sections.is_empty() {
                        sections.push(separator());
                    }
                    sections.push(self.code_lines(
                        &strip_ansi(&output),
                        output_lines,
                        colors.text_muted,
                        Some((
                            format!("t3-full-output-{}", item.id),
                            t!("Show full output"),
                            &item.id,
                        )),
                        cx,
                    ));
                } else if loading {
                    sections.push(
                        div()
                            .px(px(10.))
                            .py(px(6.))
                            .text_color(faint)
                            .child(t!("Loading output…"))
                            .into_any_element(),
                    );
                }
                if sections.is_empty() {
                    return div().into_any_element();
                }
                self.code_box(sections, cx)
            }
        }
    }

    fn render_todo_steps(&self, item: &TurnItem, cx: &App) -> AnyElement {
        let colors = cx.theme().colors();
        let success = cx.theme().status().success;
        let steps = item
            .fields
            .get("steps")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        v_flex()
            .w_full()
            .gap(px(2.))
            .text_size(px(TOOL_TEXT_SIZE + 1.))
            .children(steps.iter().map(|step| {
                let status = step.get("status").and_then(Value::as_str).unwrap_or("");
                let text = step
                    .get("text")
                    .or_else(|| step.get("step"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let (marker, color) = match status {
                    "completed" => (
                        icon(IconName::AgentCheck, 13.).text_color(success).into_any_element(),
                        text_faint(cx),
                    ),
                    "running" => (
                        div()
                            .size(px(13.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(div().size(px(7.)).rounded_full().bg(accent(cx)))
                            .into_any_element(),
                        colors.text,
                    ),
                    _ => (
                        div()
                            .size(px(13.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                div()
                                    .size(px(9.))
                                    .rounded_full()
                                    .border_1()
                                    .border_color(text_faint(cx)),
                            )
                            .into_any_element(),
                        colors.text_muted,
                    ),
                };
                h_flex()
                    .min_h(px(22.))
                    .gap(px(8.))
                    .items_center()
                    .child(marker)
                    .child(
                        div()
                            .min_w_0()
                            .text_color(color)
                            .when(status == "completed", |this| this.line_through())
                            .child(text),
                    )
            }))
            .into_any_element()
    }

    fn card(&self, cx: &App) -> gpui::Div {
        let colors = cx.theme().colors();
        v_flex()
            .w_full()
            .min_w_0()
            .gap(px(8.))
            .p(px(12.))
            .rounded(px(12.))
            .border_1()
            .border_color(hairline(0.10, cx))
            .bg(mix(page(cx), colors.editor_background, 0.5))
            .text_size(ui(13.))
    }

    fn card_title(&self, name: IconName, title: SharedString, color: Hsla, cx: &App) -> gpui::Div {
        h_flex()
            .gap(px(8.))
            .child(
                div()
                    .size(px(20.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(6.))
                    .bg(color.opacity(0.12))
                    .child(icon(name, 12.).text_color(color)),
            )
            .child(
                div()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(cx.theme().colors().text)
                    .child(title),
            )
    }

    fn render_card_row(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(item) = self.items.get(index).cloned() else {
            return div().into_any_element();
        };
        match item.kind.as_str() {
            "approval_request" => self.render_approval(&item, cx),
            "user_input_request" => self.render_questions(&item, window, cx),
            "error" => self.render_error(&item, cx),
            "checkpoint" => self.render_checkpoint(&item, cx),
            "todo_list" => self.render_tasks(&item, cx),
            "proposed_plan" => {
                let accent = accent(cx);
                self.card(cx)
                    .child(self.card_title(IconName::AgentChecklist, t!("Plan"), accent, cx))
                    .children(self.markdown_element(&item.id, Self::message_style(window, cx), cx))
                    .into_any_element()
            }
            _ => self.render_notice(&item, cx),
        }
    }

    fn render_tasks(&self, item: &TurnItem, cx: &mut Context<Self>) -> AnyElement {
        let (done, total) = todo_progress(item);
        let key = item.id.clone();
        let open = self.expanded_steps.contains(&key);
        v_flex()
            .w_full()
            .child(
                self.summary_row(
                    ElementId::Name(format!("t3-tasks-{key}").into()),
                    Some(IconName::AgentChecklist),
                    t!("Tasks {done}/{total}", done = done, total = total).into_any_element(),
                    open,
                    None,
                    cx,
                )
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.toggle_step(&key, cx))),
            )
            .when(open, |this| {
                this.child(
                    div()
                        .pl(px(STEP_INDENT))
                        .pb(px(4.))
                        .child(self.render_todo_steps(item, cx)),
                )
            })
            .into_any_element()
    }

    fn render_notice(&self, item: &TurnItem, cx: &App) -> AnyElement {
        let level = item.str_field("level").unwrap_or("");
        let color = match level {
            "error" => cx.theme().status().error,
            "warning" => cx.theme().status().warning,
            _ => text_faint(cx),
        };
        h_flex()
            .w_full()
            .gap(px(8.))
            .text_size(ui(12.))
            .text_color(color)
            .child(div().h(px(1.)).w(px(16.)).flex_none().bg(hairline(0.12, cx)))
            .child(div().min_w_0().child(notice_text(item)))
            .child(div().h(px(1.)).flex_1().bg(hairline(0.12, cx)))
            .into_any_element()
    }

    fn render_error(&self, item: &TurnItem, cx: &mut Context<Self>) -> AnyElement {
        let failure = item.fields.get("failure");
        let message = failure
            .and_then(|failure| failure.get("message"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let danger = cx.theme().status().error;
        let danger_muted = mix(danger, cx.theme().colors().text, 0.28).opacity(0.85);
        let copy_key: SharedString = format!("copy-error-{}", item.id).into();
        let copied = self.copied.as_ref() == Some(&copy_key);
        let retry = item.fields.get("retry").and_then(|retry| {
            let attempt = retry.get("attempt")?.as_u64()?;
            Some(match retry.get("maxAttempts").and_then(Value::as_u64) {
                Some(max) => t!("Retrying ({attempt}/{max})…", attempt = attempt, max = max),
                None => t!("Retrying ({attempt})…", attempt = attempt),
            })
        });
        v_flex()
            .w_full()
            .gap(px(6.))
            .rounded(px(10.))
            .border_1()
            .border_color(danger.opacity(0.16))
            .bg(danger.opacity(0.05))
            .px(px(10.))
            .py(px(8.))
            .text_size(px(12.))
            .child(
                h_flex()
                    .gap(px(8.))
                    .child(
                        div()
                            .flex_none()
                            .size(px(20.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(6.))
                            .bg(danger.opacity(0.12))
                            .child(icon(IconName::AgentDangerTriangle, 12.).text_color(danger_muted)),
                    )
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(danger_muted)
                            .child(t!("Error")),
                    )
                    .children(retry.map(|retry| div().text_color(text_faint(cx)).child(retry)))
                    .child(div().flex_1())
                    .child(
                        icon_button(
                            ElementId::Name(copy_key.clone()),
                            if copied {
                                IconName::AgentCheck
                            } else {
                                IconName::AgentCopy
                            },
                            danger_muted,
                            cx,
                        )
                        .size(px(20.))
                        .tooltip(Tooltip::text(t!("Copy message")))
                        .on_click(cx.listener({
                            let message = message.clone();
                            move |this, _: &ClickEvent, _, cx| {
                                this.copy(copy_key.clone(), message.clone(), cx)
                            }
                        })),
                    ),
            )
            .child(
                div()
                    .min_w_0()
                    .w_full()
                    .text_color(cx.theme().colors().text.opacity(0.8))
                    .child(message),
            )
            .into_any_element()
    }

    fn render_checkpoint(&self, item: &TurnItem, cx: &mut Context<Self>) -> AnyElement {
        let root = self.worktree_root();
        let files = item
            .fields
            .get("files")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let total = |name: &str| {
            files
                .iter()
                .filter_map(|file| file.get(name).and_then(Value::as_u64))
                .sum::<u64>()
        };
        let (additions, deletions) = (total("additions"), total("deletions"));
        let colors = cx.theme().colors();
        let status = cx.theme().status();
        let key = item.id.clone();
        let expanded = self.expanded_steps.contains(&key);
        let font = code_font(cx);
        let stats = |additions: u64, deletions: u64| {
            h_flex()
                .flex_none()
                .gap(px(4.))
                .font(font.clone())
                .text_size(px(11.))
                .when(additions > 0, |this| {
                    this.child(
                        div()
                            .text_color(status.created)
                            .child(format!("+{additions}")),
                    )
                })
                .when(deletions > 0, |this| {
                    this.child(
                        div()
                            .text_color(status.deleted)
                            .child(format!("−{deletions}")),
                    )
                })
        };
        v_flex()
            .w_full()
            .rounded(px(10.))
            .border_1()
            .border_color(hairline(0.10, cx))
            .overflow_hidden()
            .text_size(px(TOOL_TEXT_SIZE))
            .child(
                h_flex()
                    .id(SharedString::from(format!("t3-checkpoint-{key}")))
                    .h(px(32.))
                    .px(px(10.))
                    .gap(px(8.))
                    .cursor_pointer()
                    .hover(|style| style.bg(ink(0.03, cx)))
                    .child(icon(IconName::AgentDocument, 14.).text_color(colors.text_muted))
                    .child(
                        div()
                            .text_color(colors.text)
                            .child(if files.len() == 1 {
                                t!("1 file changed")
                            } else {
                                t!("{count} files changed", count = files.len())
                            }),
                    )
                    .child(stats(additions, deletions))
                    .child(div().flex_1())
                    .child(
                        icon(
                            if expanded {
                                IconName::AgentArrowDown
                            } else {
                                IconName::AgentArrowRight
                            },
                            12.,
                        )
                        .text_color(text_faint(cx)),
                    )
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.toggle_step(&key, cx)
                    })),
            )
            .when(expanded, |this| {
                this.child(
                    v_flex()
                        .border_t_1()
                        .border_color(hairline(0.08, cx))
                        .py(px(4.))
                        .children(files.iter().enumerate().filter_map(|(position, file)| {
                            let path = file.get("path")?.as_str()?.to_string();
                            let file_additions =
                                file.get("additions").and_then(Value::as_u64).unwrap_or(0);
                            let file_deletions =
                                file.get("deletions").and_then(Value::as_u64).unwrap_or(0);
                            let kind = file.get("kind").and_then(Value::as_str).unwrap_or("");
                            Some(
                                h_flex()
                                    .id(SharedString::from(format!(
                                        "t3-checkpoint-{}-{position}",
                                        item.id
                                    )))
                                    .h(px(24.))
                                    .px(px(10.))
                                    .gap(px(8.))
                                    .cursor_pointer()
                                    .hover(|style| style.bg(ink(0.04, cx)))
                                    .child(
                                        div()
                                            .min_w_0()
                                            .truncate()
                                            .text_color(if kind == "deleted" {
                                                text_faint(cx)
                                            } else {
                                                colors.text_muted
                                            })
                                            .when(kind == "deleted", |this| this.line_through())
                                            .child(display_path(&path, &root)),
                                    )
                                    .child(stats(file_additions, file_deletions))
                                    .on_click(cx.listener(
                                        move |this, _: &ClickEvent, window, cx| {
                                            this.open_file(&path, window, cx)
                                        },
                                    )),
                            )
                        })),
                )
            })
            .into_any_element()
    }

    fn render_approval(&self, item: &TurnItem, cx: &mut Context<Self>) -> AnyElement {
        let Some(request_id) = item.str_field("requestId").map(str::to_string) else {
            return div().into_any_element();
        };
        let body = item
            .str_field("prompt")
            .or_else(|| item.str_field("reason"))
            .unwrap_or("")
            .to_string();
        let pending = self
            .state
            .as_ref()
            .and_then(|state| state.pending_request(&request_id))
            .is_some_and(|request| request.accepts_decision());
        let kind = item.str_field("requestKind").unwrap_or("");
        let title = match kind {
            "command" | "exec_command" => t!("Run this command?"),
            "file_change" | "apply_patch" | "file-change" => t!("Apply these changes?"),
            _ => t!("The agent asks for permission"),
        };
        if !pending {
            let denied = item.is_failed();
            let color = if denied {
                cx.theme().status().error
            } else {
                text_faint(cx)
            };
            return h_flex()
                .w_full()
                .gap(px(8.))
                .text_size(px(TOOL_TEXT_SIZE))
                .text_color(color)
                .child(
                    icon(
                        if denied {
                            IconName::AgentClose
                        } else {
                            IconName::AgentCheck
                        },
                        12.,
                    )
                    .text_color(color),
                )
                .child(if denied { t!("Denied") } else { t!("Allowed") })
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_color(text_faint(cx))
                        .child(single_line(&body)),
                )
                .into_any_element();
        }
        let warning = cx.theme().status().warning;
        let button = |label: SharedString, decision: ApprovalDecision, style: ButtonStyle| {
            let request_id = request_id.clone();
            Button::new(
                SharedString::from(format!("{request_id}-{decision:?}")),
                label,
            )
            .style(style)
            .size(ButtonSize::Medium)
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.respond(&request_id, decision, cx)
            }))
        };
        self.card(cx)
            .border_color(warning.opacity(0.35))
            .child(self.card_title(IconName::AgentDangerTriangle, title, warning, cx))
            .when(!body.is_empty(), |this| {
                this.child(self.code_box(
                    vec![self.code_lines(&body, 12, cx.theme().colors().text, None, cx)],
                    cx,
                ))
            })
            .child(
                h_flex()
                    .gap(px(6.))
                    .flex_wrap()
                    .child(button(
                        t!("Allow"),
                        ApprovalDecision::Accept,
                        ButtonStyle::Filled,
                    ))
                    .child(button(
                        t!("Allow for this session"),
                        ApprovalDecision::AcceptForSession,
                        ButtonStyle::Outlined,
                    ))
                    .child(button(
                        t!("Deny"),
                        ApprovalDecision::Decline,
                        ButtonStyle::Subtle,
                    )),
            )
            .into_any_element()
    }

    fn answer_editor(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) -> Entity<Editor> {
        if let Some(editor) = self.answer_editors.get(key) {
            return editor.clone();
        }
        let editor = cx.new(|cx| {
            let mut editor = Editor::single_line(window, cx);
            editor.set_placeholder_text(&t!("Type your own answer"), window, cx);
            editor
        });
        self._subscriptions
            .push(cx.observe(&editor, |_, _, cx| cx.notify()));
        self.answer_editors.insert(key.to_string(), editor.clone());
        editor
    }

    fn collect_answers(&self, request_id: &str, questions: &[Value], cx: &App) -> Option<Map<String, Value>> {
        let chosen = self.answers.get(request_id);
        let mut answers = Map::new();
        for question in questions {
            let id = question.get("id")?.as_str()?;
            let multi = question.get("multiSelect").and_then(Value::as_bool) == Some(true);
            let required = question.get("required").and_then(Value::as_bool) != Some(false);
            let mut picked = chosen
                .and_then(|chosen| chosen.get(id))
                .cloned()
                .unwrap_or_default();
            let typed = self
                .answer_editors
                .get(&format!("{request_id}/{id}"))
                .map(|editor| editor.read(cx).text(cx).trim().to_string())
                .filter(|text| !text.is_empty());
            if let Some(typed) = typed {
                if multi {
                    picked.push(typed);
                } else {
                    picked = vec![typed];
                }
            }
            if picked.is_empty() {
                if required && !multi {
                    return None;
                }
                if multi {
                    answers.insert(id.to_string(), Value::Array(Vec::new()));
                }
                continue;
            }
            let value = if multi {
                Value::Array(picked.into_iter().map(Value::String).collect())
            } else {
                Value::String(picked.into_iter().next().unwrap_or_default())
            };
            answers.insert(id.to_string(), value);
        }
        Some(answers)
    }

    fn render_questions(&mut self, item: &TurnItem, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(request_id) = item.str_field("requestId").map(str::to_string) else {
            return div().into_any_element();
        };
        let questions = item
            .fields
            .get("questions")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let pending = self
            .state
            .as_ref()
            .and_then(|state| state.pending_request(&request_id))
            .is_some();
        let colors = cx.theme().colors().clone();
        let accent = accent(cx);
        if !pending {
            return h_flex()
                .w_full()
                .gap(px(8.))
                .text_size(px(TOOL_TEXT_SIZE))
                .text_color(text_faint(cx))
                .child(icon(IconName::AgentCheck, 12.).text_color(text_faint(cx)))
                .child(t!("Answered"))
                .child(
                    div().min_w_0().truncate().child(
                        questions
                            .iter()
                            .filter_map(|question| question.get("header")?.as_str())
                            .collect::<Vec<_>>()
                            .join(" · "),
                    ),
                )
                .into_any_element();
        }
        let mut blocks = Vec::new();
        for question in &questions {
            let Some(question_id) = question.get("id").and_then(Value::as_str).map(str::to_string)
            else {
                continue;
            };
            let header = question.get("header").and_then(Value::as_str).unwrap_or("");
            let text = question.get("question").and_then(Value::as_str).unwrap_or("");
            let multi = question.get("multiSelect").and_then(Value::as_bool) == Some(true);
            let options = question
                .get("options")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let allow_custom = options.is_empty()
                || question.get("allowCustomAnswer").and_then(Value::as_bool) != Some(false);
            let picked = self
                .answers
                .get(&request_id)
                .and_then(|answers| answers.get(&question_id))
                .cloned()
                .unwrap_or_default();
            let option_rows = options.iter().enumerate().filter_map(|(position, option)| {
                let label = option.get("label")?.as_str()?.to_string();
                let description = option
                    .get("description")
                    .and_then(Value::as_str)
                    .filter(|description| description.trim() != label)
                    .unwrap_or("")
                    .to_string();
                let selected = picked.contains(&label);
                let request_id = request_id.clone();
                let question_id = question_id.clone();
                let marker = div()
                    .size(px(14.))
                    .mt(px(3.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .border_1()
                    .border_color(if selected { accent } else { hairline(0.3, cx) })
                    .when(multi, |this| this.rounded(px(3.)))
                    .when(!multi, |this| this.rounded_full())
                    .when(selected, |this| {
                        this.bg(accent).child(icon(IconName::Check, 10.).text_color(page(cx)))
                    });
                Some(
                    h_flex()
                        .id(SharedString::from(format!("{request_id}-{question_id}-{position}")))
                        .items_start()
                        .gap(px(10.))
                        .px(px(10.))
                        .py(px(7.))
                        .rounded(px(8.))
                        .border_1()
                        .border_color(if selected {
                            accent.opacity(0.5)
                        } else {
                            hairline(0.08, cx)
                        })
                        .when(selected, |this| this.bg(accent.opacity(0.08)))
                        .cursor_pointer()
                        .hover(|style| style.bg(ink(0.04, cx)))
                        .child(marker)
                        .child(
                            v_flex()
                                .min_w_0()
                                .child(div().text_color(colors.text).child(label.clone()))
                                .when(!description.is_empty(), |this| {
                                    this.child(
                                        div()
                                            .text_size(ui(12.))
                                            .text_color(colors.text_muted)
                                            .child(description),
                                    )
                                }),
                        )
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            let picked = this
                                .answers
                                .entry(request_id.clone())
                                .or_default()
                                .entry(question_id.clone())
                                .or_default();
                            if multi {
                                if let Some(position) = picked.iter().position(|chosen| *chosen == label) {
                                    picked.remove(position);
                                } else {
                                    picked.push(label.clone());
                                }
                            } else {
                                *picked = vec![label.clone()];
                            }
                            cx.notify();
                        })),
                )
            });
            let option_rows = option_rows.collect::<Vec<_>>();
            let custom = allow_custom
                .then(|| self.answer_editor(&format!("{request_id}/{question_id}"), window, cx));
            blocks.push(
                v_flex()
                    .gap(px(6.))
                    .when(!header.is_empty(), |this| {
                        this.child(
                            div()
                                .text_size(ui(11.))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(text_faint(cx))
                                .child(header.to_uppercase()),
                        )
                    })
                    .child(div().text_color(colors.text).child(text.to_string()))
                    .children(option_rows)
                    .children(custom.map(|editor| {
                        div()
                            .px(px(10.))
                            .py(px(7.))
                            .rounded(px(8.))
                            .border_1()
                            .border_color(hairline(0.10, cx))
                            .bg(colors.editor_background)
                            .child(editor)
                    }))
                    .into_any_element(),
            );
        }
        let ready = self
            .collect_answers(&request_id, &questions, cx)
            .is_some();
        let submit_request = request_id.clone();
        let dismiss_request = request_id.clone();
        self.card(cx)
            .border_color(accent.opacity(0.35))
            .gap(px(12.))
            .child(self.card_title(
                IconName::AgentChatRoundLine,
                t!("The agent has a question"),
                accent,
                cx,
            ))
            .children(blocks)
            .child(
                h_flex()
                    .gap(px(6.))
                    .child(
                        Button::new(SharedString::from(format!("{request_id}-submit")), t!("Submit"))
                            .style(ButtonStyle::Filled)
                            .size(ButtonSize::Medium)
                            .disabled(!ready)
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                this.submit_answers(&submit_request, &questions, cx)
                            })),
                    )
                    .child(
                        Button::new(SharedString::from(format!("{request_id}-dismiss")), t!("Skip"))
                            .style(ButtonStyle::Subtle)
                            .size(ButtonSize::Medium)
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                if let Some(thread_id) = this.thread_id.clone() {
                                    this.run_command(
                                        commands::dismiss_questions(&thread_id, &dismiss_request),
                                        cx,
                                    );
                                }
                            })),
                    ),
            )
            .into_any_element()
    }

    fn submit_answers(&mut self, request_id: &str, questions: &[Value], cx: &mut Context<Self>) {
        let (Some(thread_id), Some(answers)) = (
            self.thread_id.clone(),
            self.collect_answers(request_id, questions, cx),
        ) else {
            return;
        };
        self.run_command(
            commands::answer_questions(&thread_id, request_id, answers),
            cx,
        );
    }

    fn render_working(&self, cx: &App) -> AnyElement {
        let elapsed = self
            .state
            .as_ref()
            .and_then(ThreadState::active_run)
            .and_then(|run| parse_time(run.started_at.as_deref().or(run.requested_at.as_deref())))
            .map(|started| (chrono::Utc::now() - started).num_seconds().max(0) as u64)
            .unwrap_or(0);
        let seed = flavour_seed(self.thread_id.as_deref().unwrap_or(""));
        let waiting = self
            .state
            .as_ref()
            .is_some_and(|state| state.runtime_requests.iter().any(|request| request.is_pending()));
        let label = if waiting {
            t!("Waiting for you…").to_string()
        } else {
            format!("{}…", i18n::translate(flavour_word(seed, elapsed)))
        };
        h_flex()
            .gap(px(SPACE_SM))
            .child(gradient_spinner(
                format!("t3-working-{}", self.thread_id.as_deref().unwrap_or("draft")).into(),
                14.,
            ))
            .child(
                div()
                    .text_size(ui(12.))
                    .text_color(cx.theme().colors().text_muted)
                    .child(label),
            )
            .child(
                div()
                    .text_size(ui(11.))
                    .text_color(text_faint(cx))
                    .child(format_elapsed(elapsed)),
            )
            .into_any_element()
    }

    fn render_turn_actions(&self, turn: &Turn, group: SharedString, cx: &mut Context<Self>) -> AnyElement {
        let text = self
            .items
            .get(turn.items.clone())
            .unwrap_or_default()
            .iter()
            .filter(|item| classify(item) == Class::Reply)
            .map(|item| item.text())
            .collect::<Vec<_>>()
            .join("\n\n");
        let at = turn
            .last_reply
            .and_then(|reply| self.items.get(reply))
            .and_then(|item| item.str_field("completedAt").or_else(|| item.str_field("updatedAt")));
        let key: SharedString = format!("copy-turn-{}", self.turn_key(turn)).into();
        self.hover_strip(h_flex().visible_on_hover(group), at, Some((key, text)), cx)
            .into_any_element()
    }

    fn render_transcript(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut rendered = Vec::new();
        let turns = self.turns.clone();
        let last_index = self.items.len().saturating_sub(1);
        let mut live_shown = false;
        for (turn_index, turn) in turns.iter().enumerate() {
            let live = self.turn_is_live(turn_index);
            live_shown |= live;
            let expanded = self.expanded_turns.contains(&self.turn_key(turn));
            let rows = turn_rows(turn, &self.items, live, expanded);
            let group: SharedString = format!("t3-turn-{}", self.turn_key(turn)).into();
            let mut children: Vec<AnyElement> = Vec::new();
            let mut previous: Option<&Row> = None;
            for row in &rows {
                let element = match row {
                    Row::User(index) => self.render_user(*index, window, cx),
                    Row::Worked => self.render_worked(turn, cx),
                    Row::Work(range) => {
                        let is_live = live && range.contains(&last_index);
                        self.render_work_group(range.clone(), is_live, window, cx)
                    }
                    Row::Reply(index) => self.render_reply(*index, window, cx),
                    Row::Card(index) => self.render_card_row(*index, window, cx),
                    Row::Working => self.render_working(cx),
                    Row::Actions => self.render_turn_actions(turn, group.clone(), cx),
                };
                let gap = match (previous, row) {
                    (None, _) | (_, Row::Actions) => 0.,
                    (Some(Row::User(_)), _) => SPACE_SM,
                    (Some(Row::Work(_) | Row::Worked), _) | (_, Row::Work(_) | Row::Worked) => {
                        SPACE_MD
                    }
                    (_, Row::Working) => SPACE_LG,
                    _ => SPACE_MD,
                };
                children.push(div().w_full().pt(px(gap)).child(element).into_any_element());
                previous = Some(row);
            }
            rendered.push(
                v_flex()
                    .group(group)
                    .w_full()
                    .pt(px(if turn_index == 0 { SPACE_LG } else { SPACE_LG + 4. }))
                    .children(children)
                    .into_any_element(),
            );
        }
        if !live_shown && self.is_working() {
            rendered.push(
                div()
                    .pt(px(SPACE_LG))
                    .child(self.render_working(cx))
                    .into_any_element(),
            );
        }
        rendered
    }

    fn render_queue(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let active_run = self.active_run_id();
        let state = self.state.as_ref()?;
        let thread_id = self.thread_id.clone()?;
        let queue = state.queue();
        if queue.is_empty() {
            return None;
        }
        let colors = cx.theme().colors();
        Some(
            v_flex()
                .mx(px(10.))
                .px(px(4.))
                .pt(px(4.))
                .pb(px(4.))
                .rounded_t(px(12.))
                .border_1()
                .border_b_0()
                .border_color(colors.border)
                .bg(composer_surface(cx))
                .child(
                    div()
                        .px(px(8.))
                        .h(px(22.))
                        .flex()
                        .items_center()
                        .text_size(ui(11.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(text_faint(cx))
                        .child(t!("Queued")),
                )
                .children(queue.into_iter().map(|(run, text)| {
                    let run_id = run.id.clone();
                    let thread_id = thread_id.clone();
                    let steer = active_run.clone().map(|target| {
                        let thread_id = thread_id.clone();
                        let run_id = run_id.clone();
                        div()
                            .id(SharedString::from(format!("steer-{run_id}")))
                            .flex_none()
                            .h(px(20.))
                            .px(px(6.))
                            .flex()
                            .items_center()
                            .rounded(px(5.))
                            .text_size(ui(11.))
                            .text_color(colors.text_muted)
                            .cursor_pointer()
                            .hover(|style| style.bg(ink(0.08, cx)).text_color(colors.text))
                            .tooltip(Tooltip::text(t!("Send it into the current turn now")))
                            .child(t!("Steer"))
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                this.run_command(
                                    commands::promote_to_steer(&thread_id, &run_id, &target),
                                    cx,
                                )
                            }))
                    });
                    h_flex()
                        .h(px(28.))
                        .px(px(8.))
                        .gap(px(8.))
                        .rounded(px(7.))
                        .hover(|style| style.bg(ink(0.04, cx)))
                        .child(icon(IconName::AgentArrowRight, 12.).text_color(text_faint(cx)))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(ui(13.))
                                .text_color(colors.text_muted)
                                .child(single_line(text)),
                        )
                        .children(steer)
                        .child(
                            icon_button(
                                SharedString::from(format!("cancel-{run_id}")),
                                IconName::AgentClose,
                                colors.text_muted,
                                cx,
                            )
                            .size(px(20.))
                            .tooltip(Tooltip::text(t!("Remove from queue")))
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                this.run_command(
                                    commands::cancel_queued_run(&thread_id, &run_id),
                                    cx,
                                )
                            })),
                        )
                }))
                .into_any_element(),
        )
    }

    fn render_todo_tray(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.is_working() {
            return None;
        }
        let active = self.active_run_id()?;
        let todo = self
            .items
            .iter()
            .rev()
            .find(|item| item.kind == "todo_list" && item.run_id.as_deref() == Some(active.as_str()))?
            .clone();
        let (done, total) = todo_progress(&todo);
        if total == 0 {
            return None;
        }
        let colors = cx.theme().colors();
        let current = todo
            .fields
            .get("steps")
            .and_then(Value::as_array)
            .and_then(|steps| {
                steps
                    .iter()
                    .find(|step| step.get("status").and_then(Value::as_str) != Some("completed"))
            })
            .and_then(|step| step.get("text").and_then(Value::as_str))
            .map(str::to_string)
            .unwrap_or_default();
        let expanded = self.todos_expanded;
        Some(
            v_flex()
                .mx(px(10.))
                .px(px(4.))
                .py(px(4.))
                .rounded_t(px(12.))
                .border_1()
                .border_b_0()
                .border_color(colors.border)
                .bg(composer_surface(cx))
                .child(
                    h_flex()
                        .id("t3-todo-toggle")
                        .h(px(26.))
                        .px(px(8.))
                        .gap(px(8.))
                        .rounded(px(7.))
                        .cursor_pointer()
                        .hover(|style| style.bg(ink(0.04, cx)))
                        .text_size(ui(12.))
                        .child(icon(IconName::AgentChecklist, 13.).text_color(colors.text_muted))
                        .child(
                            div()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(colors.text)
                                .child(t!("Todo")),
                        )
                        .child(div().text_color(text_faint(cx)).child(format!("{done}/{total}")))
                        .when(!expanded, |this| {
                            this.child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_color(colors.text_muted)
                                    .child(current),
                            )
                        })
                        .when(expanded, |this| this.child(div().flex_1()))
                        .child(
                            icon(
                                if expanded {
                                    IconName::AgentArrowDown
                                } else {
                                    IconName::AgentArrowUp
                                },
                                12.,
                            )
                            .text_color(text_faint(cx)),
                        )
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                            this.todos_expanded = !this.todos_expanded;
                            cx.notify();
                        })),
                )
                .when(expanded, |this| {
                    this.child(div().px(px(10.)).pb(px(4.)).child(self.render_todo_steps(&todo, cx)))
                })
                .into_any_element(),
        )
    }

    fn current_selection(&self) -> Option<ModelSelection> {
        match &self.thread_id {
            None => self.selection.clone(),
            Some(_) => self.pending_selection.clone().or_else(|| {
                self.state
                    .as_ref()
                    .and_then(|state| state.thread.as_ref())
                    .and_then(|thread| thread.model_selection.clone())
            }),
        }
    }

    fn current_runtime_mode(&self) -> String {
        match &self.thread_id {
            None => self.runtime_mode.clone(),
            Some(_) => self
                .pending_runtime_mode
                .clone()
                .or_else(|| {
                    self.state
                        .as_ref()
                        .and_then(|state| state.thread.as_ref())
                        .and_then(|thread| thread.runtime_mode.clone())
                })
                .unwrap_or_else(|| DEFAULT_RUNTIME_MODE.to_string()),
        }
    }

    fn current_interaction_mode(&self) -> String {
        match &self.thread_id {
            None => self.interaction_mode.clone(),
            Some(_) => self
                .pending_interaction_mode
                .clone()
                .or_else(|| {
                    self.state
                        .as_ref()
                        .and_then(|state| state.thread.as_ref())
                        .and_then(|thread| thread.interaction_mode.clone())
                })
                .unwrap_or_else(|| DEFAULT_INTERACTION_MODE.to_string()),
        }
    }

    fn current_provider(&self, cx: &App) -> Option<ProviderInfo> {
        let selection = self.current_selection()?;
        self.store
            .read(cx)
            .providers()
            .iter()
            .find(|provider| provider.instance_id == selection.instance_id)
            .cloned()
    }

    fn set_selection(&mut self, selection: ModelSelection, cx: &mut Context<Self>) {
        if self.current_selection().is_none_or(|current| {
            current.instance_id != selection.instance_id || current.model != selection.model
        }) {
            self.store.update(cx, |store, cx| {
                store.remember_model_selection(&selection, cx)
            });
        }
        match self.thread_id.clone() {
            None => self.selection = Some(selection),
            Some(thread_id) => {
                self.run_command(commands::set_model_selection(&thread_id, &selection), cx);
                self.pending_selection = Some(selection);
            }
        }
        cx.notify();
    }

    fn set_runtime_mode(&mut self, mode: &str, cx: &mut Context<Self>) {
        match self.thread_id.clone() {
            None => self.runtime_mode = mode.to_string(),
            Some(thread_id) => {
                self.run_command(commands::set_runtime_mode(&thread_id, mode), cx);
                self.pending_runtime_mode = Some(mode.to_string());
            }
        }
        cx.notify();
    }

    fn set_interaction_mode(&mut self, mode: &str, cx: &mut Context<Self>) {
        match self.thread_id.clone() {
            None => self.interaction_mode = mode.to_string(),
            Some(thread_id) => {
                self.run_command(commands::set_interaction_mode(&thread_id, mode), cx);
                self.pending_interaction_mode = Some(mode.to_string());
            }
        }
        cx.notify();
    }

    /// Drops local choices once the thread record shows them, so later changes made from
    /// T3 Code's other apps show here too.
    fn settle_pending(&mut self) {
        let Some(thread) = self.state.as_ref().and_then(|state| state.thread.as_ref()) else {
            return;
        };
        if self.pending_selection.is_some()
            && thread.model_selection.as_ref() == self.pending_selection.as_ref()
        {
            self.pending_selection = None;
        }
        if self.pending_runtime_mode.is_some()
            && thread.runtime_mode.as_deref() == self.pending_runtime_mode.as_deref()
        {
            self.pending_runtime_mode = None;
        }
        if self.pending_interaction_mode.is_some()
            && thread.interaction_mode.as_deref() == self.pending_interaction_mode.as_deref()
        {
            self.pending_interaction_mode = None;
        }
    }

    fn render_model_chip(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.theme().colors();
        let selection = self.current_selection();
        let provider = self.current_provider(cx);
        let model = selection
            .as_ref()
            .zip(provider.as_ref())
            .and_then(|(selection, provider)| provider.model(&selection.model).cloned());
        let brand = provider
            .as_ref()
            .map_or(IconName::AgentBot, |provider| provider_icon(&provider.driver));
        let name = match (&model, &selection) {
            (Some(model), _) => model.short_name.clone().unwrap_or_else(|| model.name.clone()),
            (None, Some(selection)) => selection.model.clone(),
            (None, None) => t!("Choose a model").to_string(),
        };
        let reasoning = selection
            .as_ref()
            .zip(model.as_ref())
            .and_then(|(selection, model)| reasoning_label(selection, model));
        let fast = selection
            .as_ref()
            .zip(model.as_ref())
            .is_some_and(|(selection, model)| is_fast(selection, model));
        let chip = h_flex()
            .h(px(28.))
            .min_w_0()
            .px(px(8.))
            .gap(px(6.))
            .rounded(px(14.))
            .text_size(ui(12.))
            .cursor_pointer()
            .hover(|style| style.bg(ink(0.06, cx)))
            .child(icon(brand, 14.).text_color(colors.text_muted))
            .child(div().min_w_0().truncate().text_color(colors.text).child(name))
            .when_some(reasoning, |this, reasoning| {
                this.child(div().flex_none().text_color(colors.text_muted).child(reasoning))
            })
            .when(fast, |this| {
                this.child(icon(IconName::AgentFastFilled, 12.).text_color(accent(cx)))
            })
            .child(icon(IconName::AgentArrowDown, 11.).text_color(colors.text_muted));
        let store = self.store.clone();
        let view = cx.weak_entity();
        PopoverMenu::new("t3-model-picker")
            .trigger(
                ButtonLike::new("t3-model-picker-trigger")
                    .style(ButtonStyle::Transparent)
                    .child(chip),
            )
            .menu(move |window, cx| {
                let selection = view
                    .upgrade()
                    .and_then(|view| view.read(cx).current_selection())
                    .unwrap_or_else(|| ModelSelection {
                        instance_id: String::new(),
                        model: String::new(),
                        options: None,
                    });
                let view = view.clone();
                let on_change: OnSelection = Rc::new(move |selection, _, cx| {
                    view.update(cx, |this, cx| this.set_selection(selection, cx))
                        .ok();
                });
                let store = store.clone();
                Some(cx.new(|cx| ModelPicker::new(store, selection, on_change, window, cx)))
            })
            .anchor(gpui::Anchor::BottomRight)
            .attach(gpui::Anchor::TopRight)
            .offset(point(px(0.), px(-6.)))
            .into_any_element()
    }

    fn render_composer_footer(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.theme().colors();
        let provider = self.current_provider(cx);
        let mode = self.current_runtime_mode();
        let (mode_icon, mode_label) = runtime_mode_info(&mode);
        let planning = self.current_interaction_mode() == "plan";
        let shows_plan = provider
            .as_ref()
            .is_none_or(|provider| provider.show_interaction_mode_toggle);
        let chip = |id: &'static str, chip_icon: IconName, label: SharedString, active: bool| {
            h_flex()
                .id(id)
                .h(px(22.))
                .px(px(6.))
                .gap(px(5.))
                .rounded(px(6.))
                .text_size(ui(12.))
                .cursor_pointer()
                .text_color(if active { accent(cx) } else { colors.text_muted })
                .hover(|style| style.bg(ink(0.06, cx)).text_color(colors.text))
                .child(icon(chip_icon, 12.).text_color(if active {
                    accent(cx)
                } else {
                    colors.text_muted
                }))
                .child(label)
        };
        let view = cx.weak_entity();
        let permission = PopoverMenu::new("t3-permission-picker")
            .trigger(
                ButtonLike::new("t3-permission-trigger")
                    .style(ButtonStyle::Transparent)
                    .child(chip("t3-permission-chip", mode_icon, mode_label, false)),
            )
            .menu(move |_, cx| {
                let (mode, provider) = view
                    .upgrade()
                    .map(|view| {
                        let view = view.read(cx);
                        (view.current_runtime_mode(), view.current_provider(cx))
                    })
                    .unwrap_or_else(|| (DEFAULT_RUNTIME_MODE.to_string(), None));
                let view = view.clone();
                let on_change: OnMode = Rc::new(move |mode, _, cx| {
                    view.update(cx, |this, cx| this.set_runtime_mode(mode, cx))
                        .ok();
                });
                Some(cx.new(|cx| PermissionPicker::new(mode, provider.as_ref(), on_change, cx)))
            })
            .anchor(gpui::Anchor::BottomRight)
            .attach(gpui::Anchor::TopRight)
            .offset(point(px(0.), px(-6.)));
        h_flex()
            .h(px(26.))
            .px(px(8.))
            .gap(px(4.))
            .when(shows_plan, |this| {
                this.child(
                    chip("t3-plan-toggle", IconName::Notepad, t!("Plan"), planning)
                        .tooltip(Tooltip::text(if planning {
                            t!("Plan mode is on: the agent proposes a plan before changing anything")
                        } else {
                            t!("Turn on plan mode: the agent proposes a plan before changing anything")
                        }))
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            this.set_interaction_mode(if planning { "default" } else { "plan" }, cx)
                        })),
                )
            })
            .child(div().flex_1())
            .child(permission)
            .into_any_element()
    }

    fn render_send_button(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.theme().colors();
        let working = self.is_working();
        let has_text = (!self.composer.read(cx).text(cx).trim().is_empty()
            || !self.attachments.is_empty())
            && self.send_task.is_none();
        let plate = page(cx);
        let focus_handle = self.composer.focus_handle(cx);
        let circle = |id: &'static str| {
            div()
                .id(id)
                .size(px(SEND_BUTTON_SIZE))
                .flex_none()
                .rounded_full()
                .bg(colors.text)
                .flex()
                .items_center()
                .justify_center()
        };
        if working && !has_text {
            return circle("t3-stop")
                .cursor_pointer()
                .hover(|style| style.opacity(0.85))
                .tooltip(move |_, cx| Tooltip::for_action_in(t!("Stop"), &Stop, &focus_handle, cx))
                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.stop(&Stop, window, cx)))
                .child(div().size(px(10.)).rounded(px(2.)).bg(plate))
                .into_any_element();
        }
        circle("t3-send")
            .when(!has_text, |this| this.opacity(0.35))
            .when(has_text, |this| {
                this.cursor_pointer()
                    .hover(|style| style.opacity(0.85))
                    .on_click(cx.listener(|this, event: &ClickEvent, window, cx| {
                        this.submit(event.modifiers().secondary(), window, cx)
                    }))
            })
            .tooltip(move |_, cx| {
                if working {
                    Tooltip::with_meta_in(
                        t!("Queue message"),
                        Some(&Send),
                        t!("{modifier}-click to steer the current turn instead", modifier = SECONDARY_MODIFIER),
                        &focus_handle,
                        cx,
                    )
                } else {
                    Tooltip::for_action_in(t!("Send message"), &Send, &focus_handle, cx)
                }
            })
            .child(icon(IconName::AgentSend, 14.).text_color(plate))
            .into_any_element()
    }

    fn render_composer(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.theme().colors().clone();
        let mut key_context = KeyContext::new_with_defaults();
        key_context.add("T3Composer");
        if self.completion_visible() {
            key_context.add("t3_completion");
        }
        v_flex()
            .key_context(key_context)
            .on_action(cx.listener(Self::send))
            .on_action(cx.listener(Self::steer))
            .on_action(cx.listener(Self::stop))
            .on_action(cx.listener(|this, _: &SelectNextCompletion, _, cx| {
                this.select_next_completion(cx)
            }))
            .on_action(cx.listener(|this, _: &SelectPreviousCompletion, _, cx| {
                this.select_previous_completion(cx)
            }))
            .on_action(cx.listener(|this, _: &AcceptCompletion, window, cx| {
                this.accept_completion(None, window, cx)
            }))
            .on_action(cx.listener(|this, _: &DismissCompletion, _, cx| this.dismiss_completion(cx)))
            .capture_action(cx.listener(|this, _: &editor::actions::Paste, _, cx| {
                if this.paste_attachments(cx) {
                    cx.stop_propagation();
                }
            }))
            .w_full()
            .rounded(px(COMPOSER_RADIUS))
            .border_1()
            .border_color(colors.border)
            .bg(composer_surface(cx))
            .shadow_md()
            .children(self.render_attachment_strip(cx))
            .child(
                div()
                    .px(px(14.))
                    .pt(px(12.))
                    .pb(px(6.))
                    .max_h(px(TEXTAREA_MAX))
                    .text_size(ui(14.))
                    .child(self.composer.clone()),
            )
            .child(
                h_flex()
                    .h(px(40.))
                    .px(px(8.))
                    .pb(px(4.))
                    .gap(px(8.))
                    .child(
                        icon_button("t3-attach", IconName::AgentPaperclip, colors.text_muted, cx)
                            .size(px(28.))
                            .rounded_full()
                            .tooltip(Tooltip::text(t!("Attach files")))
                            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                this.pick_files(window, cx)
                            })),
                    )
                    .child(div().flex_1())
                    .child(div().min_w_0().child(self.render_model_chip(cx)))
                    .child(self.render_send_button(cx)),
            )
            .into_any_element()
    }

    fn render_empty(&self, cx: &App) -> AnyElement {
        let colors = cx.theme().colors();
        let project = self
            .project_root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap(px(10.))
            .px(px(24.))
            .child(gradient_spinner("t3-empty-mark".into(), 28.))
            .child(
                div()
                    .text_size(ui(16.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(colors.text)
                    .child(t!("What should we work on?")),
            )
            .child(
                h_flex()
                    .gap(px(6.))
                    .text_size(ui(12.))
                    .text_color(colors.text_muted)
                    .child(icon(IconName::AgentFolder, 12.).text_color(colors.text_muted))
                    .child(project),
            )
            .child(
                div()
                    .max_w(px(280.))
                    .text_center()
                    .text_size(ui(12.))
                    .text_color(text_faint(cx))
                    .child(t!(
                        "The agent runs in T3 Code. You can follow this thread from T3 Code's web and mobile apps too."
                    )),
            )
            .into_any_element()
    }

    pub fn open_in_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let view = cx.entity();
        let workspace = self.workspace.clone();
        window.defer(cx, move |window, cx| {
            workspace
                .update(cx, |workspace, cx| {
                    crate::tab::open_in_editor(workspace, view, window, cx)
                })
                .log_err();
        });
    }

    pub fn move_to_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let view = cx.entity();
        let workspace = self.workspace.clone();
        window.defer(cx, move |window, cx| {
            workspace
                .update(cx, |workspace, cx| {
                    crate::tab::move_to_panel(workspace, view, window, cx)
                })
                .log_err();
        });
    }

    fn render_scroll_button(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.theme().colors();
        h_flex()
            .absolute()
            .bottom(px(8.))
            .left_0()
            .right_0()
            .justify_center()
            .child(
                h_flex()
                    .id("t3-scroll-to-bottom")
                    .h(px(28.))
                    .px(px(12.))
                    .gap(px(6.))
                    .rounded_full()
                    .border_1()
                    .border_color(colors.border)
                    .bg(colors.elevated_surface_background)
                    .shadow_md()
                    .cursor_pointer()
                    .text_size(ui(12.))
                    .hover(|style| style.bg(colors.element_hover))
                    .child(icon(IconName::AgentArrowDown, 12.).text_color(colors.text_muted))
                    .child(div().text_color(colors.text).child(t!("Scroll to bottom")))
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.following = true;
                        cx.notify();
                    })),
            )
            .into_any_element()
    }
}

fn code_text_style(cx: &App) -> TextStyleRefinement {
    let font = code_font(cx);
    TextStyleRefinement {
        font_family: Some(font.family),
        font_features: Some(font.features),
        font_fallbacks: font.fallbacks,
        font_weight: Some(font.weight),
        font_style: Some(font.style),
        ..Default::default()
    }
}

fn is_at_bottom(scroll: &ScrollHandle) -> bool {
    distance_from_bottom(scroll) <= FOLLOW_THRESHOLD
}

fn distance_from_bottom(scroll: &ScrollHandle) -> f32 {
    let offset = scroll.offset();
    let max_offset = scroll.max_offset();
    f32::from(max_offset.y + offset.y).max(0.)
}

impl Render for ThreadView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_tick(cx);
        if is_at_bottom(&self.scroll) {
            self.following = true;
        }
        if self.following {
            self.scroll.scroll_to_bottom();
        }
        let loading = self.thread_id.is_some()
            && self
                .state
                .as_ref()
                .is_none_or(|state| !state.has_snapshot());
        let is_empty = self.thread_id.is_none() && self.send_task.is_none();
        let transcript = if is_empty {
            Vec::new()
        } else {
            self.render_transcript(window, cx)
        };
        let show_scroll_button = !is_empty
            && !self.following
            && distance_from_bottom(&self.scroll) > SCROLL_BUTTON_THRESHOLD;
        let queue = self.render_queue(cx);
        let todos = self.render_todo_tray(cx);
        let body = if is_empty {
            self.render_empty(cx)
        } else {
            div()
                .relative()
                .size_full()
                .child(
                    div()
                        .id("t3-thread-scroll")
                        .size_full()
                        .overflow_y_scroll()
                        .track_scroll(&self.scroll)
                        .on_scroll_wheel(cx.listener(|this, event: &gpui::ScrollWheelEvent, _, _| {
                            if event.delta.pixel_delta(px(16.)).y > px(0.) {
                                this.following = false;
                            }
                        }))
                        .child(
                            v_flex()
                                .w_full()
                                .when(!self.docked, |this| this.max_w(px(TAB_CONTENT_WIDTH)).mx_auto())
                                .px(px(GUTTER))
                                .pb(px(SPACE_LG * 2.))
                                .when(loading, |this| {
                                    this.child(
                                        h_flex()
                                            .pt(px(SPACE_LG))
                                            .gap(px(8.))
                                            .child(gradient_spinner("t3-loading".into(), 14.))
                                            .child(
                                                Label::new(t!("Loading…"))
                                                    .size(LabelSize::Small)
                                                    .color(Color::Muted),
                                            ),
                                    )
                                })
                                .children(transcript),
                        ),
                )
                .when(show_scroll_button, |this| {
                    this.child(self.render_scroll_button(cx))
                })
                .into_any_element()
        };
        let drop_highlight = cx.theme().colors().drop_target_background;
        v_flex()
            .size_full()
            .track_focus(&self.focus_handle)
            .drag_over::<ExternalPaths>(move |style, _, _, _| style.bg(drop_highlight))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                this.attach_paths(paths.paths().to_vec(), cx)
            }))
            .when(!self.docked, |this| this.bg(cx.theme().colors().editor_background))
            .child(div().flex_1().min_h_0().child(body))
            .child(
                v_flex()
                    .w_full()
                    .when(!self.docked, |this| this.max_w(px(TAB_CONTENT_WIDTH + 20.)).mx_auto())
                    .px(px(10.))
                    .pb(px(10.))
                    .pt(px(4.))
                    .when_some(self.error.clone(), |this, error| {
                        this.child(
                            h_flex()
                                .px(px(10.))
                                .pb(px(6.))
                                .gap(px(6.))
                                .text_size(ui(12.))
                                .text_color(cx.theme().status().error)
                                .child(
                                    icon(IconName::AgentDangerTriangle, 12.)
                                        .text_color(cx.theme().status().error),
                                )
                                .child(div().flex_1().min_w_0().child(error))
                                .child(
                                    icon_button(
                                        "t3-dismiss-error",
                                        IconName::AgentClose,
                                        cx.theme().colors().text_muted,
                                        cx,
                                    )
                                    .size(px(18.))
                                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                        this.error = None;
                                        cx.notify();
                                    })),
                                ),
                        )
                    })
                    .children(todos)
                    .children(queue)
                    .children(self.render_completion(cx).map(|menu| div().pb(px(6.)).child(menu)))
                    .child(self.render_composer(cx))
                    .child(div().pt(px(4.)).child(self.render_composer_footer(cx))),
            )
    }
}
