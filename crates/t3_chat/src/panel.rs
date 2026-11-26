use crate::{
    ToggleFocus,
    model_picker::{provider_color, provider_icon},
    store::{Status, T3Store},
    style,
    thread_view::{ThreadView, ThreadViewEvent},
};
use anyhow::Result;
use db::kvp::KeyValueStore;
use gpui::{
    Action, App, AsyncWindowContext, ClickEvent, Context, Entity, EventEmitter, FocusHandle,
    Focusable, Pixels, ScrollWheelEvent, SharedString, Subscription, Task, WeakEntity, Window,
};
use i18n::{n, t};
use language::LanguageRegistry;
use project::Project;
use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use t3_client::{ModelSelection, protocol::ThreadShell};
use ui::{Tooltip, prelude::*};
use util::ResultExt as _;
use workspace::{
    Workspace, WorkspaceId,
    dock::{DockPosition, Panel, PanelEvent},
};

const T3_PANEL_KEY: &str = "T3Panel";

/// How far a two-finger swipe has to travel before it counts as going back.
const SWIPE_BACK_DISTANCE: Pixels = px(120.);
/// A pause this long between scroll events ends one swipe and lets the next one start.
const SWIPE_GAP: Duration = Duration::from_millis(150);

/// A horizontal touchpad swipe that goes back once, like in a browser.
#[derive(Default)]
struct SwipeBack {
    distance: Pixels,
    last_event: Option<Instant>,
    fired: bool,
}

impl SwipeBack {
    /// Returns whether this event completes a swipe to the right.
    fn update(&mut self, delta: gpui::Point<Pixels>, now: Instant) -> bool {
        if self
            .last_event
            .is_none_or(|last| now.duration_since(last) > SWIPE_GAP)
        {
            *self = Self::default();
        }
        self.last_event = Some(now);
        if self.fired {
            return false;
        }
        if delta.x.abs() <= delta.y.abs() {
            self.distance = px(0.);
            return false;
        }
        self.distance = (self.distance + delta.x).max(px(0.));
        self.fired = self.distance >= SWIPE_BACK_DISTANCE;
        self.fired
    }
}

fn open_thread_key(workspace_id: WorkspaceId) -> String {
    format!("t3_chat_open_thread_{}", i64::from(workspace_id))
}

enum Page {
    Threads,
    Thread(Entity<ThreadView>),
}

pub struct T3Panel {
    store: Entity<T3Store>,
    workspace: WeakEntity<Workspace>,
    project: Entity<Project>,
    languages: Arc<LanguageRegistry>,
    page: Page,
    workspace_id: Option<WorkspaceId>,
    /// The thread that was open when this workspace last closed, opened again once it is known.
    restore_thread: Option<String>,
    adding_project: Option<Task<()>>,
    view_subscription: Option<Subscription>,
    swipe_back: SwipeBack,
    error: Option<SharedString>,
    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl T3Panel {
    pub async fn load(
        workspace: WeakEntity<Workspace>,
        mut cx: AsyncWindowContext,
    ) -> Result<Entity<Self>> {
        workspace.update_in(&mut cx, |workspace, window, cx| {
            let project = workspace.project().clone();
            let languages = workspace.app_state().languages.clone();
            let workspace_id = workspace.database_id();
            let restore_thread = workspace_id.and_then(|id| {
                KeyValueStore::global(cx)
                    .read_kvp(&open_thread_key(id))
                    .log_err()
                    .flatten()
            });
            let workspace = cx.weak_entity();
            cx.new(|cx| {
                let mut panel = Self::new(workspace, project, languages, window, cx);
                panel.workspace_id = workspace_id;
                panel.restore_thread = restore_thread;
                panel.try_restore(window, cx);
                panel
            })
        })
    }

    fn new(
        workspace: WeakEntity<Workspace>,
        project: Entity<Project>,
        languages: Arc<LanguageRegistry>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let store = T3Store::global(cx);
        // Connect right away so the title bar can show running threads while the panel is closed.
        store.update(cx, |store, cx| store.ensure_connected(cx));
        let subscriptions = vec![
            cx.observe_in(&store, window, |this, _, window, cx| {
                this.try_restore(window, cx);
                cx.notify()
            }),
            cx.observe(&project, |_, _, cx| cx.notify()),
        ];
        Self {
            store,
            workspace,
            project,
            languages,
            page: Page::Threads,
            workspace_id: None,
            restore_thread: None,
            adding_project: None,
            view_subscription: None,
            swipe_back: SwipeBack::default(),
            error: None,
            focus_handle: cx.focus_handle(),
            _subscriptions: subscriptions,
        }
    }

    fn try_restore(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(thread_id) = self.restore_thread.as_deref() else {
            return;
        };
        let store = self.store.read(cx);
        let shell = store.shell();
        let has_snapshot = shell.synchronized || shell.sequence > 0 || !shell.projects.is_empty();
        if !matches!(store.status(), Status::Connected) || !has_snapshot {
            return;
        }
        let in_tab = self
            .workspace
            .upgrade()
            .is_some_and(|workspace| crate::tab::tab_for_thread(workspace.read(cx), thread_id, cx).is_some());
        let known = store.shell().threads.contains_key(thread_id) && !in_tab;
        let Some((project_id, root)) = self.t3_project(cx).filter(|_| known) else {
            self.restore_thread = None;
            return;
        };
        let thread_id = thread_id.to_string();
        self.restore_thread = None;
        if matches!(self.page, Page::Threads) {
            self.open_view(project_id, root, Some(thread_id), None, false, window, cx);
        }
    }

    fn remember_open_thread(&self, thread_id: Option<&str>, cx: &mut App) {
        let Some(workspace_id) = self.workspace_id else {
            return;
        };
        let key = open_thread_key(workspace_id);
        let thread_id = thread_id.map(str::to_string);
        let db = KeyValueStore::global(cx);
        db::write_and_log(cx, move || async move {
            match thread_id {
                Some(thread_id) => db.write_kvp(key, thread_id).await,
                None => db.delete_kvp(key).await,
            }
        });
    }

    fn workspace_root(&self, cx: &App) -> Option<PathBuf> {
        self.project
            .read(cx)
            .visible_worktrees(cx)
            .next()
            .map(|worktree| worktree.read(cx).abs_path().to_path_buf())
    }

    fn t3_project(&self, cx: &App) -> Option<(String, PathBuf)> {
        let root = self.workspace_root(cx)?;
        let project = self.store.read(cx).shell().project_for_root(&root)?;
        Some((project.id.clone(), root))
    }

    fn default_selection(&self, project_id: &str, cx: &App) -> Option<ModelSelection> {
        let store = self.store.read(cx);
        if let Some(selection) = store.last_model_selection() {
            return Some(selection.clone());
        }
        let project_default = store
            .shell()
            .projects
            .iter()
            .find(|project| project.id == project_id)
            .and_then(|project| project.default_model_selection.clone());
        project_default.or_else(|| {
            let provider = store
                .providers()
                .iter()
                .find(|provider| provider.is_ready())?;
            let model = provider.default_model()?;
            Some(ModelSelection {
                instance_id: provider.instance_id.clone(),
                model: model.slug.clone(),
                options: None,
            })
        })
    }

    pub fn new_thread(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((project_id, root)) = self.t3_project(cx) else {
            return;
        };
        let selection = self.default_selection(&project_id, cx);
        self.open_view(project_id, root, None, selection, true, window, cx);
    }

    fn open_thread(
        &mut self,
        thread_id: String,
        in_editor: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let tab = self
            .workspace
            .upgrade()
            .and_then(|workspace| crate::tab::tab_for_thread(workspace.read(cx), &thread_id, cx));
        if let Some(tab) = tab {
            let workspace = self.workspace.clone();
            window.defer(cx, move |window, cx| {
                workspace
                    .update(cx, |workspace, cx| {
                        workspace.activate_item(&tab, true, true, window, cx);
                    })
                    .log_err();
            });
            return;
        }
        let Some((project_id, root)) = self.t3_project(cx) else {
            return;
        };
        if in_editor {
            let view = self.create_view(project_id, root, Some(thread_id), None, window, cx);
            view.update(cx, |view, cx| view.open_in_editor(window, cx));
        } else {
            self.open_view(project_id, root, Some(thread_id), None, true, window, cx);
        }
    }

    fn open_view(
        &mut self,
        project_id: String,
        root: PathBuf,
        thread_id: Option<String>,
        selection: Option<ModelSelection>,
        focus: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.restore_thread = None;
        self.remember_open_thread(thread_id.as_deref(), cx);
        let view = self.create_view(project_id, root, thread_id, selection, window, cx);
        self.adopt_view(view, focus, window, cx);
    }

    fn create_view(
        &self,
        project_id: String,
        root: PathBuf,
        thread_id: Option<String>,
        selection: Option<ModelSelection>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<ThreadView> {
        let store = self.store.clone();
        let workspace = self.workspace.clone();
        let languages = self.languages.clone();
        cx.new(|cx| {
            ThreadView::new(
                store, workspace, languages, project_id, root, thread_id, selection, window, cx,
            )
        })
    }

    fn adopt_view(
        &mut self,
        view: Entity<ThreadView>,
        focus: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.view_subscription = Some(cx.subscribe(
            &view,
            |this, view, event: &ThreadViewEvent, cx| match event {
                ThreadViewEvent::Launched => {
                    let thread_id = view.read(cx).thread_id().map(str::to_string);
                    this.remember_open_thread(thread_id.as_deref(), cx);
                    cx.notify()
                }
                ThreadViewEvent::Changed => cx.notify(),
            },
        ));
        if focus {
            let composer = view.read(cx).composer_focus_handle(cx);
            composer.focus(window, cx);
        }
        self.page = Page::Thread(view);
        cx.notify();
    }

    /// The thread open in this panel, if any.
    pub fn thread_view(&self) -> Option<Entity<ThreadView>> {
        match &self.page {
            Page::Thread(view) => Some(view.clone()),
            Page::Threads => None,
        }
    }

    /// Lets go of a thread that is moving to an editor tab.
    pub fn release_view(&mut self, view: &Entity<ThreadView>, cx: &mut Context<Self>) {
        if matches!(&self.page, Page::Thread(open) if open == view) {
            self.view_subscription = None;
            self.remember_open_thread(None, cx);
            self.page = Page::Threads;
            cx.notify();
        }
    }

    /// Shows a thread that moved here from an editor tab.
    pub fn show_view(&mut self, view: Entity<ThreadView>, window: &mut Window, cx: &mut Context<Self>) {
        let thread_id = view.read(cx).thread_id().map(str::to_string);
        self.restore_thread = None;
        self.remember_open_thread(thread_id.as_deref(), cx);
        self.adopt_view(view, true, window, cx);
    }

    fn on_scroll_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !matches!(self.page, Page::Thread(_)) || event.modifiers.modified() {
            return;
        }
        let delta = event.delta.pixel_delta(window.line_height());
        if self.swipe_back.update(delta, Instant::now()) {
            self.back(cx);
        }
    }

    fn back(&mut self, cx: &mut Context<Self>) {
        self.view_subscription = None;
        self.remember_open_thread(None, cx);
        self.page = Page::Threads;
        cx.notify();
    }

    fn add_project(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.workspace_root(cx) else {
            return;
        };
        let title = root
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| root.to_string_lossy().to_string());
        let task =
            self.store
                .read(cx)
                .create_project(title, root.to_string_lossy().to_string(), cx);
        self.error = None;
        self.adding_project = Some(cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.adding_project = None;
                if let Err(error) = result {
                    this.error = Some(format!("{error:#}").into());
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors();
        let store = self.store.read(cx);
        let title: SharedString = match &self.page {
            Page::Thread(view) => view
                .read(cx)
                .thread_id()
                .and_then(|thread_id| store.shell().threads.get(thread_id))
                .map(|thread| SharedString::from(thread.title.clone()))
                .unwrap_or_else(|| t!("New thread")),
            Page::Threads => "T3 Code".into(),
        };
        let connected = matches!(store.status(), Status::Connected);
        let can_start_thread = connected && self.t3_project(cx).is_some();
        h_flex()
            .h_8()
            .px_2()
            .gap_1()
            .border_b_1()
            .border_color(colors.border)
            .when(matches!(self.page, Page::Thread(_)), |this| {
                this.child(
                    IconButton::new("t3-back", IconName::ArrowLeft)
                        .icon_size(IconSize::Small)
                        .tooltip(Tooltip::text(t!("Back to threads")))
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.back(cx))),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(Label::new(title).size(LabelSize::Small).truncate()),
            )
            .when(matches!(self.page, Page::Thread(_)), |this| {
                this.child(
                    IconButton::new("t3-open-in-editor", IconName::GenericMaximize)
                        .icon_size(IconSize::Small)
                        .tooltip(Tooltip::text(t!("Open in Editor")))
                        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                            if let Some(view) = this.thread_view() {
                                view.update(cx, |view, cx| view.open_in_editor(window, cx));
                            }
                        })),
                )
            })
            .when(can_start_thread, |this| {
                this.child(
                    IconButton::new("t3-new-thread", IconName::Plus)
                        .icon_size(IconSize::Small)
                        .tooltip(Tooltip::text(t!("New thread")))
                        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                            this.new_thread(window, cx)
                        })),
                )
            })
    }

    fn render_status(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let status = self.store.read(cx).status().clone();
        let (message, action): (SharedString, Option<(SharedString, bool)>) = match status {
            Status::Connected => return None,
            Status::Idle | Status::Connecting => (t!("Connecting to T3 Code…"), None),
            Status::StartingServer => (t!("Starting the T3 Code server…"), None),
            Status::NotRunning => (
                t!(
                    "The T3 Code server isn't running. Ade can start it, or run `t3 service install` to keep it running for your phone."
                ),
                Some((t!("Start T3 Code"), true)),
            ),
            Status::NotInstalled => (
                t!("Install T3 Code to use its agents here. Ade looks for the `t3` command."),
                Some((t!("Try Again"), false)),
            ),
            Status::Failed(message) => (message, Some((t!("Reconnect"), false))),
        };
        Some(
            v_flex()
                .p_3()
                .gap_2()
                .child(
                    Label::new(message)
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                )
                .when_some(action, |this, (label, start_server)| {
                    this.child(
                        Button::new("t3-connect", label)
                            .style(ButtonStyle::Filled)
                            .size(ButtonSize::Compact)
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                this.store
                                    .update(cx, |store, cx| store.connect(start_server, cx));
                            })),
                    )
                })
                .into_any_element(),
        )
    }

    fn render_threads(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(root) = self.workspace_root(cx) else {
            return Label::new(t!("Open a folder to use T3 Code here."))
                .size(LabelSize::Small)
                .color(Color::Muted)
                .into_any_element();
        };
        let Some((project_id, _)) = self.t3_project(cx) else {
            let adding = self.adding_project.is_some();
            return v_flex()
                .gap_2()
                .child(
                    Label::new(t!(
                        "{folder} isn't a T3 Code project yet.",
                        folder = root.display()
                    ))
                    .size(LabelSize::Small)
                    .color(Color::Muted),
                )
                .child(
                    Button::new("t3-add-project", t!("Add to T3 Code"))
                        .style(ButtonStyle::Filled)
                        .size(ButtonSize::Compact)
                        .disabled(adding)
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.add_project(cx))),
                )
                .into_any_element();
        };
        let store = self.store.read(cx);
        let threads = store
            .shell()
            .project_threads(&project_id)
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        if threads.is_empty() {
            return v_flex()
                .w_full()
                .pt(px(48.))
                .items_center()
                .gap(px(12.))
                .child(
                    Label::new(t!("No threads yet."))
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                )
                .child(
                    Button::new("t3-first-thread", t!("New thread"))
                        .style(ButtonStyle::Filled)
                        .size(ButtonSize::Medium)
                        .start_icon(Icon::new(IconName::Plus).size(IconSize::Small))
                        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                            this.new_thread(window, cx)
                        })),
                )
                .into_any_element();
        }
        let now = chrono::Utc::now();
        v_flex()
            .gap_0p5()
            .children(
                threads
                    .into_iter()
                    .map(|thread| self.render_thread_row(thread, now, cx)),
            )
            .into_any_element()
    }

    fn render_thread_row(
        &self,
        thread: ThreadShell,
        now: chrono::DateTime<chrono::Utc>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = cx.theme().colors();
        let accent = colors.text_accent;
        let error = cx.theme().status().error;
        let faint = colors.text_placeholder;
        let driver = thread.model_selection.as_ref().map(|selection| {
            self.store
                .read(cx)
                .providers()
                .iter()
                .find(|provider| provider.instance_id == selection.instance_id)
                .map_or(selection.instance_id.as_str(), |provider| {
                    provider.driver.as_str()
                })
        });
        let agent_icon = provider_icon(driver.unwrap_or_default());
        let agent_color = provider_color(driver.unwrap_or_default()).unwrap_or(colors.text);
        let failed = thread.last_error.is_some() && thread.status == "failed";
        let (detail, detail_color): (SharedString, gpui::Hsla) = if thread.needs_attention() {
            (t!("Needs your input"), accent)
        } else if thread.is_working() {
            (t!("Working"), colors.text_muted)
        } else if failed {
            (t!("Failed"), error)
        } else {
            (time_ago(&thread.updated_at, now), faint)
        };
        let indicator = if thread.needs_attention() {
            Some(
                div()
                    .size(px(7.))
                    .flex_none()
                    .rounded_full()
                    .bg(accent)
                    .into_any_element(),
            )
        } else if thread.is_working() {
            Some(style::spinning_icon(
                SharedString::from(format!("t3-thread-spinner-{}", thread.id)),
                IconName::LoadCircle,
                12.,
                colors.text_muted,
            ))
        } else if failed {
            Some(
                style::icon(IconName::AgentDangerTriangle, 12.)
                    .text_color(error)
                    .into_any_element(),
            )
        } else {
            None
        };
        let thread_id = thread.id.clone();
        let hover_group = "t3-thread-row";
        let open_in_editor = t!("Open in Editor");
        h_flex()
            .id(SharedString::from(format!("t3-thread-{}", thread.id)))
            .group(hover_group)
            .w_full()
            .h(px(34.))
            .px(px(10.))
            .gap(px(10.))
            .rounded(px(8.))
            .cursor_pointer()
            .hover(|style| style.bg(colors.element_hover))
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                this.open_thread(thread_id.clone(), false, window, cx)
            }))
            .child(style::icon(agent_icon, 14.).text_color(agent_color))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(style::ui(13.))
                    .text_color(colors.text)
                    .child(thread.title),
            )
            .children(indicator)
            .child(
                h_flex()
                    .relative()
                    .h_full()
                    .min_w(ButtonSize::Default.rems())
                    .flex_none()
                    .justify_end()
                    .child(
                        div()
                            .group_hover(hover_group, |style| style.invisible())
                            .text_size(style::ui(11.))
                            .text_color(detail_color)
                            .child(detail),
                    )
                    .child(
                        h_flex().absolute().inset_0().justify_end().child(
                            IconButton::new("t3-open-thread-in-editor", IconName::GenericMaximize)
                                .icon_size(IconSize::Small)
                                .aria_label(open_in_editor.clone())
                                .tooltip(Tooltip::text(open_in_editor))
                                .visible_on_hover(hover_group)
                                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                                    this.open_thread(thread.id.clone(), true, window, cx)
                                })),
                        ),
                    ),
            )
            .into_any_element()
    }
}

fn time_ago(timestamp: &str, now: chrono::DateTime<chrono::Utc>) -> SharedString {
    let Ok(time) = chrono::DateTime::parse_from_rfc3339(timestamp) else {
        return SharedString::default();
    };
    let seconds = (now - time.with_timezone(&chrono::Utc))
        .num_seconds()
        .max(0);
    let minutes = seconds / 60;
    let hours = minutes / 60;
    let days = hours / 24;
    if minutes < 1 {
        t!("now")
    } else if hours < 1 {
        t!("{count}m", count = minutes)
    } else if days < 1 {
        t!("{count}h", count = hours)
    } else {
        t!("{count}d", count = days)
    }
}

impl EventEmitter<PanelEvent> for T3Panel {}

impl Focusable for T3Panel {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        match &self.page {
            Page::Thread(view) => view.focus_handle(cx),
            Page::Threads => self.focus_handle.clone(),
        }
    }
}

impl Render for T3Panel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let status = self.render_status(cx);
        let body = match &self.page {
            Page::Thread(view) if status.is_none() => div()
                .size_full()
                .on_scroll_wheel(cx.listener(Self::on_scroll_wheel))
                .child(view.clone())
                .into_any_element(),
            Page::Thread(_) => div().into_any_element(),
            Page::Threads if status.is_none() => div()
                .id("t3-thread-list")
                .size_full()
                .overflow_y_scroll()
                .p_2()
                .child(self.render_threads(cx))
                .into_any_element(),
            Page::Threads => div().into_any_element(),
        };
        v_flex()
            .key_context("T3Panel")
            .track_focus(&self.focus_handle)
            .size_full()
            .child(self.render_header(cx))
            .children(status)
            .when_some(self.error.clone(), |this, error| {
                this.child(
                    div()
                        .px_3()
                        .child(Label::new(error).size(LabelSize::Small).color(Color::Error)),
                )
            })
            .child(div().flex_1().min_h_0().child(body))
    }
}

impl Panel for T3Panel {
    fn persistent_name() -> &'static str {
        "T3 Code Panel"
    }

    fn panel_key() -> &'static str {
        T3_PANEL_KEY
    }

    fn position(&self, _: &Window, _: &App) -> DockPosition {
        DockPosition::Right
    }

    fn position_is_valid(&self, position: DockPosition) -> bool {
        matches!(position, DockPosition::Right)
    }

    fn set_position(&mut self, _: DockPosition, _: &mut Window, _: &mut Context<Self>) {}

    fn default_size(&self, _: &Window, _: &App) -> Pixels {
        px(360.)
    }

    fn icon(&self, _: &Window, _: &App) -> Option<IconName> {
        Some(IconName::Chat)
    }

    fn icon_tooltip(&self, _: &Window, _: &App) -> Option<&'static str> {
        Some(n!("T3 Code"))
    }

    fn toggle_action(&self) -> Box<dyn Action> {
        Box::new(ToggleFocus)
    }

    fn activation_priority(&self) -> u32 {
        9
    }

    fn set_active(&mut self, active: bool, _: &mut Window, cx: &mut Context<Self>) {
        if active {
            self.store
                .update(cx, |store, cx| store.ensure_connected(cx));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::point;

    #[test]
    fn swipe_right_goes_back_once_per_gesture() {
        let mut swipe = SwipeBack::default();
        let start = Instant::now();
        let step = point(px(40.), px(2.));
        assert!(!swipe.update(step, start));
        assert!(!swipe.update(step, start + Duration::from_millis(16)));
        assert!(swipe.update(step, start + Duration::from_millis(32)));
        assert!(!swipe.update(step, start + Duration::from_millis(48)));
        let next_gesture = start + Duration::from_millis(48) + SWIPE_GAP * 2;
        assert!(swipe.update(point(px(150.), px(0.)), next_gesture));
    }

    #[test]
    fn vertical_scrolling_and_leftward_swipes_do_not_go_back() {
        let mut swipe = SwipeBack::default();
        let start = Instant::now();
        for index in 0..20 {
            let now = start + Duration::from_millis(index * 16);
            assert!(!swipe.update(point(px(5.), px(40.)), now));
            assert!(!swipe.update(point(px(-60.), px(0.)), now));
        }
    }
}
