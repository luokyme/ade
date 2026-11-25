use crate::{
    ToggleFocus,
    store::{Status, T3Store},
    thread_view::{ThreadView, ThreadViewEvent},
};
use anyhow::Result;
use gpui::{
    Action, App, AsyncWindowContext, ClickEvent, Context, Entity, EventEmitter, FocusHandle,
    Focusable, Pixels, SharedString, Subscription, Task, WeakEntity, Window,
};
use i18n::{n, t};
use language::LanguageRegistry;
use project::Project;
use std::{path::PathBuf, sync::Arc};
use t3_client::{ModelSelection, protocol::ThreadShell};
use ui::{Tooltip, prelude::*};
use workspace::{
    Workspace,
    dock::{DockPosition, Panel, PanelEvent},
};

const T3_PANEL_KEY: &str = "T3Panel";

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
    adding_project: Option<Task<()>>,
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
            let workspace = cx.weak_entity();
            cx.new(|cx| Self::new(workspace, project, languages, window, cx))
        })
    }

    fn new(
        workspace: WeakEntity<Workspace>,
        project: Entity<Project>,
        languages: Arc<LanguageRegistry>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let store = T3Store::global(cx);
        let subscriptions = vec![
            cx.observe(&store, |_, _, cx| cx.notify()),
            cx.observe(&project, |_, _, cx| cx.notify()),
        ];
        Self {
            store,
            workspace,
            project,
            languages,
            page: Page::Threads,
            adding_project: None,
            error: None,
            focus_handle: cx.focus_handle(),
            _subscriptions: subscriptions,
        }
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
        self.open_view(project_id, root, None, selection, window, cx);
    }

    fn open_thread(&mut self, thread_id: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some((project_id, root)) = self.t3_project(cx) else {
            return;
        };
        self.open_view(project_id, root, Some(thread_id), None, window, cx);
    }

    fn open_view(
        &mut self,
        project_id: String,
        root: PathBuf,
        thread_id: Option<String>,
        selection: Option<ModelSelection>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let store = self.store.clone();
        let workspace = self.workspace.clone();
        let languages = self.languages.clone();
        let view = cx.new(|cx| {
            ThreadView::new(
                store, workspace, languages, project_id, root, thread_id, selection, window, cx,
            )
        });
        self._subscriptions.push(cx.subscribe(
            &view,
            |_, _, event: &ThreadViewEvent, cx| match event {
                ThreadViewEvent::Launched => cx.notify(),
            },
        ));
        let composer = view.read(cx).composer_focus_handle(cx);
        composer.focus(window, cx);
        self.page = Page::Thread(view);
        cx.notify();
    }

    fn back(&mut self, cx: &mut Context<Self>) {
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
            return Label::new(t!("No threads yet. Start one with the + button."))
                .size(LabelSize::Small)
                .color(Color::Muted)
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
        let (detail, color): (SharedString, Color) = if thread.needs_attention() {
            (t!("Needs your input"), Color::Accent)
        } else if thread.is_working() {
            (t!("Working"), Color::Muted)
        } else if thread.last_error.is_some() && thread.status == "failed" {
            (t!("Failed"), Color::Error)
        } else {
            (time_ago(&thread.updated_at, now), Color::Muted)
        };
        let thread_id = thread.id.clone();
        h_flex()
            .id(SharedString::from(format!("t3-thread-{}", thread.id)))
            .w_full()
            .px_2()
            .py_1()
            .gap_2()
            .rounded_md()
            .cursor_pointer()
            .hover(|style| style.bg(colors.element_hover))
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                this.open_thread(thread_id.clone(), window, cx)
            }))
            .when(thread.is_working(), |this| {
                this.child(
                    Icon::new(IconName::LoadCircle)
                        .size(IconSize::XSmall)
                        .color(Color::Muted),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(Label::new(thread.title).size(LabelSize::Small).truncate()),
            )
            .child(Label::new(detail).size(LabelSize::XSmall).color(color))
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
            Page::Thread(view) if status.is_none() => view.clone().into_any_element(),
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
