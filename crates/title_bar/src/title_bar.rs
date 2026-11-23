mod onboarding_banner;
mod title_bar_settings;
mod update_version;

pub use platform_title_bar::{
    self, DraggedWindowTab, MergeAllWindows, MoveTabToNewWindow, PlatformTitleBar,
    ShowNextWindowTab, ShowPreviousWindowTab,
};
use project::repo_identity_path;

use gpui::{
    AnyElement, App, Context, Entity, EntityId, Focusable, InteractiveElement, IntoElement,
    MouseButton, ParentElement, Render, Styled, Subscription, WeakEntity, Window, actions, div,
};
use i18n::t;
use onboarding_banner::OnboardingBanner;
use project::{Project, git_store::GitStoreEvent, trusted_worktrees::TrustedWorktrees};
use remote::RemoteConnectionOptions;
use settings::Settings as _;

use theme::ActiveTheme;
use title_bar_settings::TitleBarSettings;
use ui::{ButtonLike, IconWithIndicator, Indicator, PopoverMenu, TintColor, Tooltip, prelude::*};
use update_version::UpdateVersion;
use util::ResultExt;
use workspace::{
    AgentActivity, GoBack, GoForward, MultiWorkspace, Pane, ProjectGroupKey, TabBarSettings,
    ToggleWorktreeSecurity, Workspace,
};

use ade_actions::OpenRemote;

pub use onboarding_banner::restore_banner;

const MAX_PROJECT_NAME_LENGTH: usize = 40;
const MAX_PROJECT_TABS: usize = 6;
const MAX_PROJECT_TAB_NAME_LENGTH: usize = 24;

actions!(
    title_bar,
    [
        /// Toggles the project menu dropdown.
        ToggleProjectMenu,
        /// Switches to a different git branch.
        SwitchBranch,
        /// A debug action to simulate an update being available to test the update banner UI.
        SimulateUpdateAvailable
    ]
);

pub fn init(cx: &mut App) {
    platform_title_bar::PlatformTitleBar::init(cx);

    cx.observe_new(|workspace: &mut Workspace, window, cx| {
        let Some(window) = window else {
            return;
        };
        let multi_workspace = workspace.multi_workspace().cloned();
        let item = cx.new(|cx| TitleBar::new("title-bar", workspace, multi_workspace, window, cx));
        workspace.set_titlebar_item(item.into(), window, cx);

        workspace.register_action(|workspace, _: &SimulateUpdateAvailable, _window, cx| {
            if let Some(titlebar) = workspace
                .titlebar_item()
                .and_then(|item| item.downcast::<TitleBar>().ok())
            {
                titlebar.update(cx, |titlebar, cx| {
                    titlebar.toggle_update_simulation(cx);
                });
            }
        });
    })
    .detach();
}

fn project_group_name(key: &ProjectGroupKey) -> String {
    let name = key
        .path_list()
        .ordered_paths()
        .filter_map(|path| path.file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(", ");
    util::truncate_and_trailoff(&name, MAX_PROJECT_TAB_NAME_LENGTH)
}

fn render_project_tab(
    index: usize,
    key: ProjectGroupKey,
    activity: Option<AgentActivity>,
) -> impl IntoElement {
    let mut tooltip_lines: Vec<String> = key
        .path_list()
        .ordered_paths()
        .map(|path| path.to_string_lossy().into_owned())
        .collect();
    if let Some(activity) = activity {
        tooltip_lines.push(recent_projects::activity_label(activity).to_string());
    }
    let tooltip: SharedString = tooltip_lines.join("\n").into();
    ButtonLike::new(("project_tab", index))
        .child(
            h_flex()
                .gap_1()
                .child(
                    Label::new(project_group_name(&key))
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                )
                .when_some(activity, |this, activity| {
                    this.child(Indicator::dot().color(recent_projects::activity_color(activity)))
                }),
        )
        .tooltip(Tooltip::text(tooltip))
        .on_click(move |_, window, cx| {
            recent_projects::switch_to_project_group(key.clone(), window, cx)
        })
}

#[derive(Clone)]
struct DraggedProjectTab {
    key: ProjectGroupKey,
    index: usize,
}

impl Render for DraggedProjectTab {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors();
        div()
            .font_ui(cx)
            .px_2()
            .py_0p5()
            .rounded_sm()
            .border_1()
            .border_color(colors.border)
            .bg(colors.elevated_surface_background)
            .child(Label::new(project_group_name(&self.key)).size(LabelSize::Small))
    }
}

fn draggable_project_tab(
    index: usize,
    key: ProjectGroupKey,
    active_key: ProjectGroupKey,
    multi_workspace: WeakEntity<MultiWorkspace>,
    tab: impl IntoElement,
) -> impl IntoElement {
    div()
        .id(("project_tab_drag", index))
        .rounded_sm()
        .on_drag(DraggedProjectTab { key, index }, |tab, _, _, cx| {
            cx.new(|_| tab.clone())
        })
        .drag_over::<DraggedProjectTab>(move |style, dragged, _, cx| {
            let style = style
                .bg(cx.theme().colors().drop_target_background)
                .border_color(cx.theme().colors().drop_target_border);
            if index < dragged.index {
                style.border_l_2()
            } else if index > dragged.index {
                style.border_r_2()
            } else {
                style
            }
        })
        .on_drop(move |dragged: &DraggedProjectTab, _, cx| {
            multi_workspace
                .update(cx, |multi_workspace, cx| {
                    let target = shown_drop_index(
                        &multi_workspace.project_group_keys(),
                        &active_key,
                        &dragged.key,
                        index,
                    );
                    multi_workspace.move_project_group(&dragged.key, target, cx);
                })
                .log_err();
        })
        .child(tab)
}

/// Caps a drop index so the dropped tab is not pushed into the hidden "+N" projects.
fn shown_drop_index<K: PartialEq>(keys: &[K], active: &K, dragged: &K, target: usize) -> usize {
    if dragged == active {
        return target;
    }
    keys.iter()
        .filter(|key| *key != dragged)
        .enumerate()
        .filter(|(_, key)| *key != active)
        .nth(MAX_PROJECT_TABS - 2)
        .map_or(target, |(last_shown_index, _)| target.min(last_shown_index))
}

pub struct TitleBar {
    platform_titlebar: Entity<PlatformTitleBar>,
    project: Entity<Project>,
    workspace: WeakEntity<Workspace>,
    multi_workspace: Option<WeakEntity<MultiWorkspace>>,
    _subscriptions: Vec<Subscription>,
    banner: Option<Entity<OnboardingBanner>>,
    update_version: Entity<UpdateVersion>,
    observed_pane: Option<(EntityId, Subscription)>,
}

impl Render for TitleBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.multi_workspace.is_none() {
            if let Some(mw) = self
                .workspace
                .upgrade()
                .and_then(|ws| ws.read(cx).multi_workspace().cloned())
            {
                self.multi_workspace = Some(mw.clone());
                self.observe_multi_workspace(&mw, cx);
                self.platform_titlebar.update(cx, |titlebar, _cx| {
                    titlebar.set_multi_workspace(mw);
                });
            }
        }

        let title_bar_settings = *TitleBarSettings::get_global(cx);
        let button_layout = title_bar_settings.button_layout;
        let active_pane = self
            .workspace
            .upgrade()
            .map(|workspace| workspace.read(cx).active_pane().clone());
        if let Some(pane) = &active_pane
            && self
                .observed_pane
                .as_ref()
                .is_none_or(|(pane_id, _)| *pane_id != pane.entity_id())
        {
            let subscription = cx.observe(pane, |_, _, cx| cx.notify());
            self.observed_pane = Some((pane.entity_id(), subscription));
        }
        let file_tabs = active_pane
            .as_ref()
            .and_then(|pane| pane.update(cx, |pane, cx| pane.render_title_bar_tabs(window, cx)));
        let status_bar = self.workspace.upgrade().and_then(|workspace| {
            let workspace = workspace.read(cx);
            (workspace.status_bar_visible(cx) && workspace.status_bar_in_title_bar(cx))
                .then(|| workspace.status_bar().clone())
        });
        let sidebar_switcher = self.workspace.upgrade().and_then(|workspace| {
            workspace::dock::Dock::render_sidebar_switcher(
                workspace.read(cx).left_dock(),
                window,
                cx,
            )
        });

        let mut project_name = None;
        if let Some(worktree) = self.effective_active_worktree(cx) {
            let repository = self.get_repository_for_worktree(&worktree, cx);
            let worktree_abs_path = worktree.read(cx).abs_path();
            project_name = worktree
                .read(cx)
                .root_name()
                .file_name()
                .map(|name| SharedString::from(name.to_string()));
            if let Some(repo) = &repository {
                let repo = repo.read(cx);
                let identity = repo_identity_path(&repo.common_dir_abs_path, repo.path_style);

                let display_name = if identity.extension() == Some(std::ffi::OsStr::new("git")) {
                    identity.file_stem().and_then(|n| n.to_str())
                } else {
                    repo.path_style.file_name(identity)
                };

                if let Some(repo_name) = display_name {
                    let visible_worktrees_in_repo = self.visible_worktrees_in_repository(repo, cx);
                    let name = if visible_worktrees_in_repo == 1 {
                        if let Ok(relative) =
                            worktree_abs_path.strip_prefix(&*repo.work_directory_abs_path)
                        {
                            if relative.as_os_str().is_empty() {
                                repo_name.to_string()
                            } else {
                                format!("{}/{}", repo_name, relative.display())
                            }
                        } else {
                            repo_name.to_string()
                        }
                    } else {
                        repo_name.to_string()
                    };
                    project_name = Some(SharedString::from(name));
                }
            }
        }

        let left = h_flex()
            .h_full()
            .min_w_0()
            .gap_2()
            .children(sidebar_switcher)
            .children(self.render_restricted_mode(cx))
            .when(title_bar_settings.show_project_items, |title_bar| {
                let projects = self.render_project_tabs(project_name, window, cx);
                title_bar.children(self.render_project_host(cx)).child(
                    h_flex()
                        .id("title-bar-project-tabs")
                        .min_w_0()
                        .overflow_x_scroll()
                        .child(h_flex().flex_none().child(projects)),
                )
            })
            .children(
                active_pane
                    .as_ref()
                    .and_then(|pane| self.render_navigation_buttons(pane, cx)),
            )
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());

        let right = h_flex()
            .min_w_0()
            .pr_0p5()
            .gap_0p5()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .children(status_bar)
            .when(title_bar_settings.show_onboarding_banner, |title_bar| {
                title_bar.children(self.banner.clone())
            })
            .child(self.update_version.clone());

        self.platform_titlebar.update(cx, |this, _| {
            this.set_button_layout(button_layout);
            this.set_children([h_flex()
                .w_full()
                .h_full()
                .justify_between()
                .gap_1()
                .child(left)
                .children(file_tabs.map(|tabs| h_flex().flex_1().min_w(rems(8.)).child(tabs)))
                .child(right)
                .into_any_element()]);
        });
        self.platform_titlebar.clone().into_any_element()
    }
}

impl TitleBar {
    pub fn new(
        id: impl Into<ElementId>,
        workspace: &Workspace,
        multi_workspace: Option<WeakEntity<MultiWorkspace>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let project = workspace.project().clone();
        let git_store = project.read(cx).git_store().clone();
        let mut subscriptions = vec![cx.observe(workspace.left_dock(), |_, _, cx| cx.notify())];
        subscriptions.push(
            cx.observe(&workspace.weak_handle().upgrade().unwrap(), |_, _, cx| {
                cx.notify()
            }),
        );

        subscriptions.push(
            cx.subscribe(&git_store, move |_, _, event, cx| match event {
                GitStoreEvent::ActiveRepositoryChanged(_)
                | GitStoreEvent::RepositoryUpdated(_, _, true) => {
                    cx.notify();
                }
                _ => {}
            }),
        );
        if let Some(workspace_entity) = workspace.weak_handle().upgrade() {
            subscriptions.push(cx.subscribe(
                &workspace_entity,
                |_, _, event: &workspace::Event, cx| {
                    if matches!(event, workspace::Event::WorktreeCreationChanged) {
                        cx.notify();
                    }
                },
            ));
        }
        subscriptions.push(cx.observe_button_layout_changed(window, |_, _, cx| cx.notify()));
        if let Some(trusted_worktrees) = TrustedWorktrees::try_get_global(cx) {
            subscriptions.push(cx.subscribe(&trusted_worktrees, |_, _, _, cx| {
                cx.notify();
            }));
        }

        let update_version = cx.new(|cx| UpdateVersion::new(cx));
        let platform_titlebar = cx.new(|cx| {
            let mut titlebar = PlatformTitleBar::new(id, cx);
            if let Some(mw) = multi_workspace.clone() {
                titlebar = titlebar.with_multi_workspace(mw);
            }
            titlebar
        });

        if let Some(changes) = workspace::agent_activity_changes(cx) {
            subscriptions.push(cx.observe(&changes, |_, _, cx| cx.notify()));
        }

        let banner = None;

        let mut this = Self {
            platform_titlebar,
            workspace: workspace.weak_handle(),
            multi_workspace: multi_workspace.clone(),
            project,
            _subscriptions: subscriptions,
            banner,
            update_version,
            observed_pane: None,
        };
        if let Some(multi_workspace) = multi_workspace {
            this.observe_multi_workspace(&multi_workspace, cx);
        }
        this
    }

    fn observe_multi_workspace(
        &mut self,
        multi_workspace: &WeakEntity<MultiWorkspace>,
        cx: &mut Context<Self>,
    ) {
        if let Some(multi_workspace) = multi_workspace.upgrade() {
            self._subscriptions
                .push(cx.observe(&multi_workspace, |_, _, cx| cx.notify()));
        }
    }

    fn worktree_count(&self, cx: &App) -> usize {
        self.project.read(cx).visible_worktrees(cx).count()
    }

    fn toggle_update_simulation(&mut self, cx: &mut Context<Self>) {
        self.update_version
            .update(cx, |banner, cx| banner.update_simulation(cx));
        cx.notify();
    }

    /// Returns the worktree to display in the title bar.
    /// - Prefer the worktree owning the project's active repository
    /// - Fall back to the first visible worktree
    pub fn effective_active_worktree(&self, cx: &App) -> Option<Entity<project::Worktree>> {
        let project = self.project.read(cx);

        if let Some(repo) = project.active_repository(cx) {
            let repo = repo.read(cx);
            let repo_path = &repo.work_directory_abs_path;

            for worktree in project.visible_worktrees(cx) {
                let worktree_path = worktree.read(cx).abs_path();
                if worktree_path == *repo_path || worktree_path.starts_with(repo_path.as_ref()) {
                    return Some(worktree);
                }
            }
        }

        project.visible_worktrees(cx).next()
    }

    fn get_repository_for_worktree(
        &self,
        worktree: &Entity<project::Worktree>,
        cx: &App,
    ) -> Option<Entity<project::git_store::Repository>> {
        let project = self.project.read(cx);
        let git_store = project.git_store().read(cx);
        let worktree_path = worktree.read(cx).abs_path();

        git_store
            .repositories()
            .values()
            .filter(|repo| {
                let repo_path = &repo.read(cx).work_directory_abs_path;
                worktree_path == *repo_path || worktree_path.starts_with(repo_path.as_ref())
            })
            .max_by_key(|repo| repo.read(cx).work_directory_abs_path.as_os_str().len())
            .cloned()
    }

    fn visible_worktrees_in_repository(
        &self,
        repository: &project::git_store::Repository,
        cx: &App,
    ) -> usize {
        let repo_path = &repository.work_directory_abs_path;
        self.project
            .read(cx)
            .visible_worktrees(cx)
            .filter(|worktree| {
                let worktree_path = worktree.read(cx).abs_path();
                worktree_path == *repo_path || worktree_path.starts_with(repo_path.as_ref())
            })
            .count()
    }

    fn render_remote_project_connection(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let workspace = self.workspace.clone();

        let options = self.project.read(cx).remote_connection_options(cx)?;
        let host: SharedString = options.display_name().into();

        let (nickname, tooltip_title, icon) = match options {
            RemoteConnectionOptions::Ssh(options) => (
                options.nickname.map(|nick| nick.into()),
                t!("Remote Project"),
                IconName::Server,
            ),
            RemoteConnectionOptions::Wsl(_) => (None, t!("Remote Project"), IconName::Linux),
            #[cfg(any(test, feature = "test-support"))]
            RemoteConnectionOptions::Mock(_) => (
                None,
                SharedString::new_static("Mock Remote Project"),
                IconName::Server,
            ),
        };

        let nickname = nickname.unwrap_or_else(|| host.clone());

        let (indicator_color, meta) = match self.project.read(cx).remote_connection_state(cx)? {
            remote::ConnectionState::Connecting => {
                (Color::Info, t!("Connecting to: {host}", host = host))
            }
            remote::ConnectionState::Connected => {
                (Color::Success, t!("Connected to: {host}", host = host))
            }
            remote::ConnectionState::HeartbeatMissed => (
                Color::Warning,
                t!(
                    "Connection attempt to {host} missed. Retrying...",
                    host = host
                ),
            ),
            remote::ConnectionState::Reconnecting => (
                Color::Warning,
                t!("Lost connection to {host}. Reconnecting...", host = host),
            ),
            remote::ConnectionState::Disconnected => {
                (Color::Error, t!("Disconnected from {host}", host = host))
            }
        };

        let icon_color = match self.project.read(cx).remote_connection_state(cx)? {
            remote::ConnectionState::Connecting => Color::Info,
            remote::ConnectionState::Connected => Color::Default,
            remote::ConnectionState::HeartbeatMissed => Color::Warning,
            remote::ConnectionState::Reconnecting => Color::Warning,
            remote::ConnectionState::Disconnected => Color::Error,
        };

        Some(
            PopoverMenu::new("remote-project-menu")
                .menu(move |window, cx| {
                    let workspace_entity = workspace.upgrade()?;
                    let fs = workspace_entity.read(cx).project().read(cx).fs().clone();
                    Some(recent_projects::RemoteServerProjects::popover(
                        fs,
                        workspace.clone(),
                        None,
                        window,
                        cx,
                    ))
                })
                .trigger_with_tooltip(
                    ButtonLike::new("remote_project")
                        .selected_style(ButtonStyle::Tinted(TintColor::Accent))
                        .child(
                            h_flex()
                                .gap_2()
                                .max_w_32()
                                .child(
                                    IconWithIndicator::new(
                                        Icon::new(icon).size(IconSize::Small).color(icon_color),
                                        Some(Indicator::dot().color(indicator_color)),
                                    )
                                    .indicator_border_color(Some(
                                        cx.theme().colors().title_bar_background,
                                    ))
                                    .into_any_element(),
                                )
                                .child(Label::new(nickname).size(LabelSize::Small).truncate()),
                        ),
                    move |_window, cx| {
                        Tooltip::with_meta(
                            tooltip_title.clone(),
                            Some(&OpenRemote::default()),
                            meta.clone(),
                            cx,
                        )
                    },
                )
                .anchor(gpui::Anchor::TopLeft)
                .into_any_element(),
        )
    }

    pub fn render_restricted_mode(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let has_restricted_worktrees =
            TrustedWorktrees::has_restricted_worktrees(&self.project.read(cx).worktree_store(), cx);
        if !has_restricted_worktrees {
            return None;
        }

        let button = Button::new("restricted_mode_trigger", t!("Restricted Mode"))
            .style(ButtonStyle::Tinted(TintColor::Warning))
            .label_size(LabelSize::Small)
            .color(Color::Warning)
            .start_icon(
                Icon::new(IconName::Warning)
                    .size(IconSize::Small)
                    .color(Color::Warning),
            )
            .tooltip(|_, cx| {
                Tooltip::with_meta(
                    t!("You're in Restricted Mode"),
                    Some(&ToggleWorktreeSecurity),
                    t!("Mark this project as trusted and unlock all features"),
                    cx,
                )
            })
            .on_click({
                cx.listener(move |this, _, window, cx| {
                    this.workspace
                        .update(cx, |workspace, cx| {
                            workspace.show_worktree_trust_security_modal(true, window, cx)
                        })
                        .log_err();
                })
            });

        if ui::utils::MACOS_SDK_26_OR_LATER {
            // Make up for Tahoe's traffic light buttons having less spacing around them
            Some(div().child(button).ml_0p5().into_any_element())
        } else {
            Some(button.into_any_element())
        }
    }

    fn render_navigation_buttons(&self, pane: &Entity<Pane>, cx: &App) -> Option<AnyElement> {
        if !TabBarSettings::get_global(cx).show_nav_history_buttons {
            return None;
        }
        let workspace = self.workspace.upgrade()?;

        let (can_go_back, can_go_forward) = {
            let pane = pane.read(cx);
            (pane.can_navigate_backward(), pane.can_navigate_forward())
        };
        let focus_handle = workspace.read(cx).focus_handle(cx);

        let go_back = IconButton::new("navigate_backward", IconName::ArrowLeft)
            .icon_size(IconSize::Small)
            .disabled(!can_go_back)
            .on_click({
                let workspace = self.workspace.clone();
                let pane = pane.downgrade();
                move |_, window, cx| {
                    let pane = pane.clone();
                    workspace
                        .update(cx, |workspace, cx| {
                            workspace.go_back(pane, window, cx).detach_and_log_err(cx)
                        })
                        .log_err();
                }
            })
            .tooltip({
                let focus_handle = focus_handle.clone();
                move |_window, cx| {
                    Tooltip::for_action_in(t!("Go Back"), &GoBack, &focus_handle, cx)
                }
            });

        let go_forward = IconButton::new("navigate_forward", IconName::ArrowRight)
            .icon_size(IconSize::Small)
            .disabled(!can_go_forward)
            .on_click({
                let workspace = self.workspace.clone();
                let pane = pane.downgrade();
                move |_, window, cx| {
                    let pane = pane.clone();
                    workspace
                        .update(cx, |workspace, cx| {
                            workspace
                                .go_forward(pane, window, cx)
                                .detach_and_log_err(cx)
                        })
                        .log_err();
                }
            })
            .tooltip(move |_window, cx| {
                Tooltip::for_action_in(t!("Go Forward"), &GoForward, &focus_handle, cx)
            });

        Some(
            h_flex()
                .flex_none()
                .child(go_back)
                .child(go_forward)
                .into_any_element(),
        )
    }

    pub fn render_project_host(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.project.read(cx).is_via_remote_server() {
            return self.render_remote_project_connection(cx);
        }

        None
    }

    fn render_project_tabs(
        &self,
        active_name: Option<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let multi_workspace = self.multi_workspace.as_ref().and_then(|mw| mw.upgrade());
        let keys = multi_workspace
            .as_ref()
            .map(|mw| mw.read(cx).project_group_keys())
            .unwrap_or_default();
        let active_key = self.project.read(cx).project_group_key(cx);
        let Some(multi_workspace) =
            multi_workspace.filter(|_| keys.len() > 1 && keys.contains(&active_key))
        else {
            return self
                .render_project_name(active_name, window, cx)
                .into_any_element();
        };

        let colors = cx.theme().colors();
        let box_background = colors.element_background.opacity(0.5);
        let current_background = colors.text.opacity(0.12);
        let current_border = colors.border;
        let mut other_tabs_left = MAX_PROJECT_TABS - 1;
        let mut hidden_count = 0;
        let mut tabs = h_flex()
            .p_0p5()
            .gap_1()
            .rounded_md()
            .bg(box_background);
        for (index, key) in keys.into_iter().enumerate() {
            if key == active_key {
                let tab = div()
                    .rounded_sm()
                    .border_1()
                    .border_color(current_border)
                    .bg(current_background)
                    .child(self.render_project_name(active_name.clone(), window, cx));
                tabs = tabs.child(draggable_project_tab(
                    index,
                    key,
                    active_key.clone(),
                    multi_workspace.downgrade(),
                    tab,
                ));
            } else if other_tabs_left == 0 {
                hidden_count += 1;
            } else {
                other_tabs_left -= 1;
                let roots = multi_workspace.read(cx).project_group_roots(&key, cx);
                let activity = workspace::project_agent_activity(&roots, cx);
                let tab = render_project_tab(index, key.clone(), activity);
                tabs = tabs.child(draggable_project_tab(
                    index,
                    key,
                    active_key.clone(),
                    multi_workspace.downgrade(),
                    tab,
                ));
            }
        }
        tabs.when(hidden_count > 0, |tabs| {
            tabs.child(
                Button::new("more_projects", format!("+{hidden_count}"))
                    .label_size(LabelSize::Small)
                    .color(Color::Muted)
                    .on_click(|_, window, cx| {
                        window.dispatch_action(Box::new(ade_actions::OpenRecent::default()), cx)
                    })
                    .tooltip(|_window, cx| {
                        Tooltip::for_action(
                            t!("More Open Projects"),
                            &ade_actions::OpenRecent::default(),
                            cx,
                        )
                    }),
            )
        })
        .into_any_element()
    }

    fn render_project_name(
        &self,
        name: Option<SharedString>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let workspace = self.workspace.clone();

        let is_project_selected = name.is_some();

        let display_name = if let Some(ref name) = name {
            util::truncate_and_trailoff(name, MAX_PROJECT_NAME_LENGTH)
        } else {
            t!("Open Recent Project").to_string()
        };

        let focus_handle = workspace
            .upgrade()
            .map(|w| w.read(cx).focus_handle(cx))
            .unwrap_or_else(|| cx.focus_handle());

        let window_project_groups: Vec<_> = self
            .multi_workspace
            .as_ref()
            .and_then(|mw| mw.upgrade())
            .map(|mw| mw.read(cx).project_group_keys())
            .unwrap_or_default();

        PopoverMenu::new("recent-projects-menu")
            .menu(move |window, cx| {
                Some(recent_projects::RecentProjects::popover(
                    workspace.clone(),
                    window_project_groups.clone(),
                    None,
                    focus_handle.clone(),
                    window,
                    cx,
                ))
            })
            .trigger_with_tooltip(
                Button::new("project_name_trigger", display_name)
                    .label_size(LabelSize::Small)
                    .tab_index(0isize)
                    .when(self.worktree_count(cx) > 1, |this| {
                        this.end_icon(
                            Icon::new(IconName::ChevronDown)
                                .size(IconSize::XSmall)
                                .color(Color::Muted),
                        )
                    })
                    .selected_style(ButtonStyle::Tinted(TintColor::Accent))
                    .when(!is_project_selected, |s| s.color(Color::Muted)),
                move |_window, cx| {
                    Tooltip::for_action(
                        t!("Recent Projects"),
                        &ade_actions::OpenRecent::default(),
                        cx,
                    )
                },
            )
            .anchor(gpui::Anchor::TopLeft)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use util::path_list::PathList;

    #[test]
    fn dropped_tabs_stay_among_the_shown_tabs() {
        let keys = ["a", "b", "c", "d", "e", "f", "g", "h"];
        // With "h" active, "f" and "g" are hidden, so dropping "a" onto "h" puts it before "f".
        assert_eq!(shown_drop_index(&keys, &"h", &"a", 7), 4);
        assert_eq!(shown_drop_index(&keys, &"h", &"a", 4), 4);
        assert_eq!(shown_drop_index(&keys, &"h", &"e", 0), 0);
        assert_eq!(shown_drop_index(&keys, &"h", &"h", 0), 0);
        assert_eq!(shown_drop_index(&keys, &"a", &"b", 5), 5);
        assert_eq!(shown_drop_index(&keys[..4], &"d", &"a", 3), 3);
    }

    #[test]
    fn project_tab_names_use_folder_names() {
        let key = |paths: &[&str]| ProjectGroupKey::new(None, PathList::new(paths));
        assert_eq!(project_group_name(&key(&["/code/ade"])), "ade");
        assert_eq!(
            project_group_name(&key(&["/code/api", "/code/web"])),
            "api, web"
        );
        assert_eq!(
            project_group_name(&key(&["/code/a-very-long-project-folder-name"])),
            "a-very-long-project-fold…"
        );
    }
}
