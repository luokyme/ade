use anyhow::{Context as _, Result};
use buffer_diff::{DiffHunkSecondaryStatus, DiffHunkStatus};
use editor::{
    DiffHunkRenderer, DiffStyleControls, Direction, Editor, EditorEvent, EditorSettings,
    SplittableEditor,
    actions::{GoToHunk, GoToPreviousHunk, OpenExcerpts},
};
use git::{
    Commit, Restore, StageAndNext, StageFile, ToggleStaged, UnstageAndNext, UnstageFile,
    repository::RepoPath, status::StageStatus,
};
use gpui::{
    Action, Anchor as PopoverAnchor, AnyElement, App, AppContext as _, Context, Entity,
    EventEmitter, FocusHandle, Focusable, IntoElement, Render, SharedString, Subscription, Task,
    WeakEntity, Window, relative,
};
use i18n::t;
use language::{Anchor, Buffer, OffsetRangeExt as _, Point};
use multi_buffer::{MultiBuffer, PathKey, excerpt_context_lines};
use project::{
    Project, ProjectPath,
    git_store::{Repository, RepositoryId},
    project_settings::ProjectSettings,
};
use settings::{DiffViewStyle, Settings};
use std::{
    any::{Any, TypeId},
    ops::Range,
    sync::Arc,
};
use ui::{ContextMenu, DiffStat, PopoverMenu, Tooltip, prelude::*};
use util::ResultExt as _;
use util::paths::{PathExt as _, PathStyle};
use workspace::{
    Item, ItemHandle, ItemNavHistory, SaveIntent, ToolbarItemLocation, Workspace,
    item::{ItemEvent, SaveOptions},
    notifications::NotifyTaskExt,
    searchable::SearchableItemHandle,
};

pub struct SoloDiffView {
    repository: Entity<Repository>,
    repository_id: RepositoryId,
    repo_path: RepoPath,
    buffer: Entity<Buffer>,
    diff: Entity<buffer_diff::BufferDiff>,
    editor: Entity<SplittableEditor>,
    workspace: WeakEntity<Workspace>,
    showing_full_file: bool,
    navigating_files: bool,
    _subscriptions: Vec<Subscription>,
}

impl SoloDiffView {
    pub fn open_or_focus(
        repo_path: RepoPath,
        repository: Entity<Repository>,
        workspace: WeakEntity<Workspace>,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Entity<Self>>> {
        let Some(workspace_entity) = workspace.upgrade() else {
            return Task::ready(Err(anyhow::anyhow!("workspace was dropped")));
        };

        let existing = workspace_entity
            .read(cx)
            .items_of_type::<SoloDiffView>(cx)
            .find(|item| item.read(cx).matches(&repository, &repo_path, cx));
        if let Some(existing) = existing {
            workspace_entity.update(cx, |workspace, cx| {
                workspace.activate_item(&existing, true, true, window, cx);
            });
            existing.focus_handle(cx).focus(window, cx);
            return Task::ready(Ok(existing));
        }

        let Some(project_path) = repository
            .read(cx)
            .repo_path_to_project_path(&repo_path, cx)
        else {
            return Task::ready(Err(anyhow::anyhow!(
                "could not resolve repository path {:?}",
                repo_path
            )));
        };

        let project = workspace_entity.read(cx).project().clone();
        window.spawn(cx, async move |cx| {
            let buffer = project
                .update(cx, |project, cx| {
                    project.open_buffer(project_path.clone(), cx)
                })
                .await?;
            let diff = project
                .update(cx, |project, cx| {
                    project.open_uncommitted_diff(buffer.clone(), cx)
                })
                .await?;

            workspace_entity.update_in(cx, |workspace, window, cx| {
                let workspace_handle = cx.entity();
                let view = cx.new(|cx| {
                    Self::new(
                        project,
                        repository,
                        repo_path,
                        buffer,
                        diff,
                        workspace_handle,
                        window,
                        cx,
                    )
                });

                workspace.add_item_to_active_pane(Box::new(view.clone()), None, true, window, cx);
                view
            })
        })
    }

    fn new(
        project: Entity<Project>,
        repository: Entity<Repository>,
        repo_path: RepoPath,
        buffer: Entity<Buffer>,
        diff: Entity<buffer_diff::BufferDiff>,
        workspace: Entity<Workspace>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let repository_id = repository.read(cx).id;
        let showing_full_file = EditorSettings::get_global(cx).file_diff.show_full_file;
        let multibuffer = cx
            .new(|cx| Self::build_multibuffer(buffer.clone(), diff.clone(), showing_full_file, cx));
        let editor = cx.new(|cx| {
            let editor = SplittableEditor::new(
                EditorSettings::get_global(cx).diff_view_style,
                multibuffer,
                project.clone(),
                workspace.clone(),
                window,
                cx,
            )
            .with_change_connections();
            editor
                .set_diff_hunk_renderer(Some(Arc::new(CompactHunkControls(cx.weak_entity()))), cx);
            editor.rhs_editor().update(cx, |editor, cx| {
                editor.set_should_serialize(false, cx);
                editor.set_allow_git_diff_scrollbar_markers(showing_full_file, cx);
                let snapshot = editor.snapshot(window, cx);
                editor.go_to_hunk_before_or_after_position(
                    &snapshot,
                    language::Point::new(0, 0),
                    Direction::Next,
                    true,
                    window,
                    cx,
                );
            });
            editor
        });

        let subscriptions = vec![
            cx.observe(&editor, |_, _, cx| cx.notify()),
            cx.subscribe(&diff, |_, _, _: &buffer_diff::BufferDiffEvent, cx| {
                cx.notify()
            }),
            cx.observe(&buffer, |_, _, cx| cx.notify()),
            cx.observe(&repository, |_, _, cx| cx.notify()),
            cx.subscribe(&editor, |_, _, event, cx| {
                if matches!(event, EditorEvent::SelectionsChanged { .. }) {
                    cx.notify();
                }
                cx.emit(event.clone());
            }),
        ];

        Self {
            repository,
            repository_id,
            repo_path,
            buffer,
            diff,
            editor,
            workspace: workspace.downgrade(),
            showing_full_file,
            navigating_files: false,
            _subscriptions: subscriptions,
        }
    }

    fn build_multibuffer(
        buffer: Entity<Buffer>,
        diff: Entity<buffer_diff::BufferDiff>,
        showing_full_file: bool,
        cx: &mut Context<MultiBuffer>,
    ) -> MultiBuffer {
        let (ranges, context_line_count) =
            Self::excerpt_ranges(&buffer, &diff, showing_full_file, cx);

        let mut multibuffer = MultiBuffer::without_headers(buffer.read(cx).capability());
        multibuffer.set_excerpts_for_buffer(buffer, ranges, context_line_count, cx);
        multibuffer.add_diff(diff, cx);
        multibuffer
    }

    fn excerpt_ranges(
        buffer: &Entity<Buffer>,
        diff: &Entity<buffer_diff::BufferDiff>,
        showing_full_file: bool,
        cx: &App,
    ) -> (Vec<Range<Point>>, u32) {
        if showing_full_file {
            (vec![Point::zero()..buffer.read(cx).max_point()], 0)
        } else {
            (
                Self::hunk_ranges(buffer, diff, cx),
                excerpt_context_lines(cx),
            )
        }
    }

    fn hunk_ranges(
        buffer: &Entity<Buffer>,
        diff: &Entity<buffer_diff::BufferDiff>,
        cx: &App,
    ) -> Vec<Range<Point>> {
        let buffer = buffer.read(cx);
        diff.read(cx)
            .snapshot(cx)
            .hunks_intersecting_range(
                Anchor::min_for_buffer(buffer.remote_id())
                    ..Anchor::max_for_buffer(buffer.remote_id()),
                buffer,
            )
            .map(|diff_hunk| diff_hunk.buffer_range.to_point(buffer))
            .collect()
    }

    fn set_showing_full_file(&mut self, showing_full_file: bool, cx: &mut Context<Self>) {
        if self.showing_full_file == showing_full_file {
            return;
        }

        let (ranges, context_line_count) =
            Self::excerpt_ranges(&self.buffer, &self.diff, showing_full_file, cx);

        self.editor.update(cx, |editor, cx| {
            let path = PathKey::for_buffer(&self.buffer, cx);
            editor.remove_excerpts_for_path(path.clone(), cx);
            editor.update_excerpts_for_path(
                path,
                self.buffer.clone(),
                ranges,
                context_line_count,
                self.diff.clone(),
                cx,
            );
            editor.rhs_editor().update(cx, |editor, cx| {
                editor.set_allow_git_diff_scrollbar_markers(showing_full_file, cx);
            });
        });

        self.showing_full_file = showing_full_file;
        cx.notify();
    }

    fn matches(&self, repository: &Entity<Repository>, repo_path: &RepoPath, cx: &App) -> bool {
        self.repository_id == repository.read(cx).id && &self.repo_path == repo_path
    }

    fn button_states(&self, cx: &App) -> SoloDiffButtonStates {
        let editor = self.editor.read(cx).rhs_editor().read(cx);
        let multibuffer = editor.buffer().read(cx);
        let snapshot = multibuffer.snapshot(cx);
        let mut selection = true;

        let mut ranges = editor
            .selections
            .disjoint_anchor_ranges()
            .collect::<Vec<_>>();
        if !ranges.iter().any(|range| range.start != range.end) {
            selection = false;
            let anchor = editor.selections.newest_anchor().head();
            if let Some((_, excerpt_range)) = snapshot.excerpt_containing(anchor..anchor)
                && let Some(range) = snapshot
                    .anchor_in_buffer(excerpt_range.context.start)
                    .zip(snapshot.anchor_in_buffer(excerpt_range.context.end))
                    .map(|(start, end)| start..end)
            {
                ranges = vec![range];
            } else {
                ranges = Vec::new();
            }
        }

        let mut stage = false;
        let mut unstage = false;
        for hunk in editor.diff_hunks_in_ranges(&ranges, &snapshot) {
            match hunk.status.secondary {
                DiffHunkSecondaryStatus::HasSecondaryHunk
                | DiffHunkSecondaryStatus::SecondaryHunkAdditionPending => {
                    stage = true;
                }
                DiffHunkSecondaryStatus::OverlapsWithSecondaryHunk => {
                    stage = true;
                    unstage = true;
                }
                DiffHunkSecondaryStatus::NoSecondaryHunk
                | DiffHunkSecondaryStatus::SecondaryHunkRemovalPending => {
                    unstage = true;
                }
            }
        }

        let stage_status = self
            .repository
            .read(cx)
            .status_for_path(&self.repo_path)
            .map(|entry| entry.status.staging())
            .unwrap_or(StageStatus::Unstaged);

        SoloDiffButtonStates {
            stage,
            unstage,
            restore: stage || unstage,
            selection,
            stage_file: stage_status.has_unstaged(),
            unstage_file: stage_status.has_staged(),
        }
    }

    fn dispatch_action(&self, action: &dyn Action, window: &mut Window, cx: &mut App) {
        self.editor
            .read(cx)
            .rhs_editor()
            .focus_handle(cx)
            .focus(window, cx);
        let action = action.boxed_clone();
        cx.defer(move |cx| {
            cx.dispatch_action(action.as_ref());
        });
    }

    fn change_file_stage(&self, stage: bool, window: &mut Window, cx: &mut Context<Self>) {
        let repository = self.repository.clone();
        let repo_path = self.repo_path.clone();
        let workspace = self.workspace.clone();
        let task = cx.spawn(async move |_, cx| {
            repository
                .update(cx, |repository, cx| {
                    if stage {
                        repository.stage_entries(vec![repo_path], cx)
                    } else {
                        repository.unstage_entries(vec![repo_path], cx)
                    }
                })
                .await
                .with_context(|| {
                    if stage {
                        t!("failed to stage file")
                    } else {
                        t!("failed to unstage file")
                    }
                })
        });
        task.detach_and_notify_err(workspace, window, cx);
    }

    fn open_adjacent_file(&mut self, path: RepoPath, window: &mut Window, cx: &mut Context<Self>) {
        if self.navigating_files {
            return;
        }
        self.navigating_files = true;
        cx.notify();
        let previous_id = cx.entity_id();
        let previous_buffer = self.buffer.clone();
        let pane = self
            .workspace
            .upgrade()
            .and_then(|workspace| workspace.read(cx).pane_for_item_id(previous_id));
        let repository = self.repository.clone();
        let workspace = self.workspace.clone();
        cx.spawn_in(window, async move |this, cx| {
            let opened = cx
                .update(|window, cx| Self::open_or_focus(path, repository, workspace, window, cx))?
                .await;
            this.update(cx, |this, cx| {
                this.navigating_files = false;
                cx.notify();
            })
            .ok();
            let opened = opened?;
            if let Some(pane) = pane {
                let close = pane.update_in(cx, |pane, window, cx| {
                    let index = pane
                        .items()
                        .position(|item| item.item_id() == previous_id)?;
                    (index >= pane.pinned_count()
                        && !previous_buffer.read(cx).is_dirty()
                        && pane
                            .active_item()
                            .is_some_and(|item| item.item_id() == opened.entity_id()))
                    .then(|| pane.close_item_by_id(previous_id, SaveIntent::Close, window, cx))
                })?;
                if let Some(close) = close {
                    close.await?;
                }
            }
            anyhow::Ok(())
        })
        .detach_and_notify_err(self.workspace.clone(), window, cx);
    }

    fn render_more_menu(&self, cx: &Context<Self>) -> impl IntoElement {
        let diff = cx.weak_entity();
        PopoverMenu::new("solo-diff-more")
            .anchor(PopoverAnchor::TopRight)
            .trigger(
                IconButton::new("solo-diff-more-trigger", IconName::Ellipsis)
                    .icon_size(IconSize::Small)
                    .aria_label(t!("More Diff Actions"))
                    .tooltip(Tooltip::text(t!("More Diff Actions"))),
            )
            .menu(move |window, cx| {
                let view = diff.upgrade()?;
                let states = view.read(cx).button_states(cx);
                let focus = view.focus_handle(cx);
                let stage_view = diff.clone();
                let unstage_view = diff.clone();
                Some(ContextMenu::build(window, cx, |menu, _, _| {
                    menu.context(focus)
                        .map(|menu| {
                            if states.selection {
                                menu.action_disabled_when(
                                    !states.stage && !states.unstage,
                                    t!("Toggle Staged"),
                                    ToggleStaged.boxed_clone(),
                                )
                            } else {
                                menu.action_disabled_when(
                                    !states.stage,
                                    t!("Stage and Go to Next Hunk"),
                                    StageAndNext.boxed_clone(),
                                )
                                .action_disabled_when(
                                    !states.unstage,
                                    t!("Unstage and Go to Next Hunk"),
                                    UnstageAndNext.boxed_clone(),
                                )
                            }
                        })
                        .action_disabled_when(
                            !states.restore,
                            t!("Restore Hunk"),
                            Restore.boxed_clone(),
                        )
                        .separator()
                        .when(states.stage_file, |menu| {
                            menu.entry(
                                t!("Stage All"),
                                Some(StageFile.boxed_clone()),
                                move |window, cx| {
                                    stage_view
                                        .update(cx, |view, cx| {
                                            view.change_file_stage(true, window, cx)
                                        })
                                        .log_err();
                                },
                            )
                        })
                        .when(states.unstage_file, |menu| {
                            menu.entry(
                                t!("Unstage All"),
                                Some(UnstageFile.boxed_clone()),
                                move |window, cx| {
                                    unstage_view
                                        .update(cx, |view, cx| {
                                            view.change_file_stage(false, window, cx)
                                        })
                                        .log_err();
                                },
                            )
                        })
                        .action(t!("Commit"), Commit.boxed_clone())
                }))
            })
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let repository = self.repository.read(cx);
        let split_editor = self.editor.read(cx);
        let split_ratio = split_editor.split_ratio(cx);
        let editor = split_editor.rhs_editor().read(cx);
        let buffer = self.buffer.read(cx);
        let file_exists = buffer
            .file()
            .is_some_and(|file| !file.disk_state().is_deleted());
        let diff = self.diff.read(cx).snapshot(cx);
        let (added, removed) = diff.changed_row_counts();
        let snapshot = editor.buffer().read(cx).snapshot(cx);
        let caret = editor.selections.newest_anchor().head();
        let (current_change, changes) =
            snapshot
                .diff_hunks()
                .fold((0, 0), |(current, count), hunk| {
                    let count = count + 1;
                    let current = if caret.cmp(&hunk.multi_buffer_range.start, &snapshot).is_ge() {
                        count
                    } else {
                        current
                    };
                    (current, count)
                });
        let mut previous_file = None;
        let mut next_file = None;
        for entry in repository.status() {
            if repository
                .repo_path_to_project_path(&entry.repo_path, cx)
                .is_none()
            {
                continue;
            }
            match entry.repo_path.cmp(&self.repo_path) {
                std::cmp::Ordering::Less => previous_file = Some(entry.repo_path),
                std::cmp::Ordering::Greater => {
                    next_file = Some(entry.repo_path);
                    break;
                }
                std::cmp::Ordering::Equal => {}
            }
        }

        let path: SharedString = self
            .repo_path
            .display(repository.path_style)
            .into_owned()
            .into();
        let head = if diff.base_text_exists() {
            repository
                .head_commit
                .as_ref()
                .map(|commit| commit.short_sha())
        } else {
            Some(t!("File absent"))
        };
        let working_copy_detail = if !file_exists {
            Some(t!("File absent"))
        } else if buffer.is_dirty() {
            Some(t!("Unsaved"))
        } else {
            None
        };
        let left_version = h_flex()
            .min_w_0()
            .gap_1()
            .child(
                Icon::new(IconName::FileLock)
                    .size(IconSize::Small)
                    .color(Color::Muted),
            )
            .child(Label::new("HEAD").size(LabelSize::Small))
            .children(head.map(|head| {
                Label::new(format!("· {head}"))
                    .size(LabelSize::Small)
                    .color(Color::Muted)
            }));
        let right_version = h_flex()
            .min_w_0()
            .gap_1()
            .child(Label::new(t!("Working Tree")).size(LabelSize::Small))
            .children(working_copy_detail.map(|detail| {
                Label::new(format!("· {detail}"))
                    .size(LabelSize::Small)
                    .color(Color::Muted)
            }));
        let file_path = Label::new(path)
            .size(LabelSize::Small)
            .color(Color::Muted)
            .truncate_middle();
        let focus = self.focus_handle(cx);
        let split = split_editor.diff_view_style() == DiffViewStyle::Split;

        h_flex()
            .id("solo-diff-header")
            .flex_none()
            .w_full()
            .min_h_8()
            .px_2()
            .gap_2()
            .border_b_1()
            .border_color(cx.theme().colors().border_variant)
            .bg(cx.theme().colors().editor_background)
            .map(|header| {
                if let Some(ratio) = split_ratio {
                    header
                        .child(
                            h_flex()
                                .w(relative(ratio))
                                .flex_shrink_1()
                                .min_w_0()
                                .gap_2()
                                .overflow_hidden()
                                .child(left_version)
                                .child(file_path),
                        )
                        .child(
                            h_flex()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .child(right_version),
                        )
                } else {
                    header.child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_2()
                            .overflow_hidden()
                            .child(left_version)
                            .child(Label::new("→").size(LabelSize::Small).color(Color::Muted))
                            .child(right_version)
                            .child(file_path),
                    )
                }
            })
            .child(
                h_flex()
                    .flex_none()
                    .gap_1()
                    .map(|header| {
                        if changes == 0 {
                            header.child(
                                Label::new(t!("No Changes"))
                                    .size(LabelSize::Small)
                                    .color(Color::Muted),
                            )
                        } else {
                            header
                                .child(
                                    Label::new(format!("{} / {changes}", current_change.max(1)))
                                        .size(LabelSize::Small)
                                        .color(Color::Muted),
                                )
                                .child(DiffStat::new(
                                    "solo-diff-stat",
                                    added as usize,
                                    removed as usize,
                                ))
                        }
                    })
                    .child(
                        IconButton::new("solo-diff-open-file", IconName::ArrowUpRight)
                            .icon_size(IconSize::Small)
                            .disabled(!file_exists)
                            .aria_label(t!("Open File"))
                            .tooltip(Tooltip::text(t!("Open File")))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.dispatch_action(&OpenExcerpts, window, cx);
                            })),
                    )
                    .child(
                        IconButton::new("solo-diff-previous-change", IconName::ChevronUp)
                            .icon_size(IconSize::Small)
                            .disabled(changes == 0)
                            .aria_label(t!("Go to Previous Hunk"))
                            .tooltip(Tooltip::for_action_title_in(
                                t!("Go to Previous Hunk"),
                                &GoToPreviousHunk,
                                &focus,
                            ))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.dispatch_action(&GoToPreviousHunk, window, cx);
                            })),
                    )
                    .child(
                        IconButton::new("solo-diff-next-change", IconName::ChevronDown)
                            .icon_size(IconSize::Small)
                            .disabled(changes == 0)
                            .aria_label(t!("Go to Next Hunk"))
                            .tooltip(Tooltip::for_action_title_in(
                                t!("Go to Next Hunk"),
                                &GoToHunk,
                                &focus,
                            ))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.dispatch_action(&GoToHunk, window, cx);
                            })),
                    )
                    .children(
                        [
                            (
                                "solo-diff-previous-file",
                                IconName::ChevronLeft,
                                t!("Previous Changed File"),
                                previous_file,
                            ),
                            (
                                "solo-diff-next-file",
                                IconName::ChevronRight,
                                t!("Next Changed File"),
                                next_file,
                            ),
                        ]
                        .map(|(id, icon, label, path)| {
                            IconButton::new(id, icon)
                                .icon_size(IconSize::Small)
                                .disabled(path.is_none() || self.navigating_files)
                                .aria_label(label.clone())
                                .tooltip(Tooltip::text(label))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    if let Some(path) = &path {
                                        this.open_adjacent_file(path.clone(), window, cx);
                                    }
                                }))
                        }),
                    )
                    .child(
                        IconButton::new(
                            "solo-diff-toggle-excerpts",
                            if self.showing_full_file {
                                IconName::ChevronDownUp
                            } else {
                                IconName::ChevronUpDown
                            },
                        )
                        .icon_size(IconSize::Small)
                        .tooltip(Tooltip::text(if self.showing_full_file {
                            t!("Show Changes Only")
                        } else {
                            t!("Show Full File")
                        }))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.set_showing_full_file(!this.showing_full_file, cx);
                        })),
                    )
                    .child(
                        IconButton::new(
                            "solo-diff-layout",
                            if split {
                                IconName::DiffUnified
                            } else {
                                IconName::DiffSplit
                            },
                        )
                        .icon_size(IconSize::Small)
                        .tooltip(Tooltip::text(if split {
                            t!("Unified")
                        } else {
                            t!("Split")
                        }))
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                let style = if split {
                                    DiffViewStyle::Unified
                                } else {
                                    DiffViewStyle::Split
                                };
                                DiffStyleControls::set_diff_view_style(
                                    &this.editor,
                                    style,
                                    window,
                                    cx,
                                );
                            },
                        )),
                    )
                    .child(self.render_more_menu(cx)),
            )
    }
}

impl EventEmitter<EditorEvent> for SoloDiffView {}

impl Focusable for SoloDiffView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.editor.focus_handle(cx)
    }
}

impl Render for SoloDiffView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex().size_full().child(self.render_header(cx)).child(
            div()
                .flex_1()
                .min_h_0()
                .overflow_hidden()
                .child(self.editor.clone()),
        )
    }
}

struct SoloDiffButtonStates {
    stage: bool,
    unstage: bool,
    restore: bool,
    selection: bool,
    stage_file: bool,
    unstage_file: bool,
}

struct CompactHunkControls(WeakEntity<SplittableEditor>);

impl DiffHunkRenderer for CompactHunkControls {
    fn show_controls_for_inactive_hunks(&self, cx: &App) -> bool {
        ProjectSettings::get_global(cx)
            .git
            .show_stage_restore_buttons
    }

    fn render_hunk_controls(
        &self,
        row: u32,
        status: &DiffHunkStatus,
        hunk_range: Range<multi_buffer::Anchor>,
        is_created_file: bool,
        line_height: Pixels,
        editor: &Entity<Editor>,
        _: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        if !ProjectSettings::get_global(cx)
            .git
            .show_stage_restore_buttons
        {
            return gpui::Empty.into_any_element();
        }
        let Some(operations) = hunk_range
            .start
            .buffer_id()
            .and_then(|id| editor.read(cx).buffer().read(cx).diff_for(id))
            .and_then(|diff| diff.read(cx).operations())
        else {
            return gpui::Empty.into_any_element();
        };
        let stage = status.has_secondary_hunk();
        let split_side = self
            .0
            .upgrade()
            .and_then(|split| split.read(cx).lhs_editor().map(|lhs| lhs == editor));
        let can_toggle = split_side != Some(true)
            && if stage {
                operations.supports_staging()
            } else {
                operations.supports_unstaging()
            };
        let can_restore =
            operations.supports_restore() && !is_created_file && split_side != Some(false);
        let toggle_title = if stage {
            t!("Stage Hunk")
        } else {
            t!("Unstage Hunk")
        };
        let toggle_icon = match status.secondary {
            DiffHunkSecondaryStatus::OverlapsWithSecondaryHunk => IconName::SquareMinus,
            _ if stage => IconName::SquarePlus,
            _ => IconName::Check,
        };
        let start = hunk_range.start;
        h_flex()
            .h(line_height)
            .gap_0p5()
            .mr_1()
            .block_mouse_except_scroll()
            .when(can_restore, |controls| {
                controls.child(
                    IconButton::new(("solo-diff-restore-hunk", row as u64), IconName::ArrowRight)
                        .icon_size(IconSize::Small)
                        .icon_color(Color::Accent)
                        .aria_label(t!("Restore Hunk"))
                        .tooltip(Tooltip::text(t!("Restore Hunk")))
                        .on_click({
                            let editor = editor.clone();
                            move |_, window, cx| {
                                editor.update(cx, |editor, cx| {
                                    editor.restore_diff_hunks_in_ranges(
                                        vec![start..start],
                                        window,
                                        cx,
                                    );
                                })
                            }
                        }),
                )
            })
            .when(can_toggle, |controls| {
                controls.child(
                    IconButton::new(("solo-diff-stage-hunk", row as u64), toggle_icon)
                        .icon_size(IconSize::Small)
                        .icon_color(Color::Accent)
                        .disabled(status.is_pending())
                        .aria_label(toggle_title.clone())
                        .tooltip(Tooltip::text(toggle_title))
                        .on_click({
                            let editor = editor.clone();
                            move |_, window, cx| {
                                editor.update(cx, |editor, cx| {
                                    editor.stage_or_unstage_diff_hunks(
                                        stage,
                                        vec![start..start],
                                        window,
                                        cx,
                                    );
                                })
                            }
                        }),
                )
            })
            .into_any_element()
    }
}

impl Item for SoloDiffView {
    type Event = EditorEvent;

    fn tab_icon(&self, _window: &Window, _cx: &App) -> Option<Icon> {
        Some(Icon::new(IconName::Diff).color(Color::Muted))
    }

    fn tab_content_text(&self, _detail: usize, cx: &App) -> SharedString {
        self.buffer
            .read(cx)
            .file()
            .and_then(|file| {
                Some(
                    file.full_path(cx)
                        .file_name()?
                        .to_string_lossy()
                        .to_string(),
                )
            })
            .unwrap_or_else(|| {
                self.repo_path
                    .as_ref()
                    .display(PathStyle::local())
                    .into_owned()
            })
            .into()
    }

    fn tab_tooltip_text(&self, cx: &App) -> Option<SharedString> {
        Some(
            self.buffer
                .read(cx)
                .file()
                .map(|file| file.full_path(cx).compact().to_string_lossy().into_owned())
                .unwrap_or_else(|| {
                    self.repo_path
                        .as_ref()
                        .display(PathStyle::local())
                        .into_owned()
                })
                .into(),
        )
    }

    fn to_item_events(event: &EditorEvent, f: &mut dyn FnMut(ItemEvent)) {
        Editor::to_item_events(event, f)
    }

    fn deactivated(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.editor.deactivated(window, cx);
    }

    fn act_as_type<'a>(
        &'a self,
        type_id: TypeId,
        self_handle: &'a Entity<Self>,
        cx: &'a App,
    ) -> Option<gpui::AnyEntity> {
        if type_id == TypeId::of::<Self>() {
            Some(self_handle.clone().into())
        } else if type_id == TypeId::of::<SplittableEditor>() {
            None
        } else {
            self.editor.act_as_type(type_id, cx)
        }
    }

    fn as_searchable(&self, _: &Entity<Self>, _: &App) -> Option<Box<dyn SearchableItemHandle>> {
        Some(Box::new(self.editor.clone()))
    }

    fn for_each_project_item(
        &self,
        cx: &App,
        f: &mut dyn FnMut(gpui::EntityId, &dyn project::ProjectItem),
    ) {
        self.editor.for_each_project_item(cx, f)
    }

    fn active_project_path(&self, cx: &App) -> Option<ProjectPath> {
        self.editor.read(cx).active_project_path(cx)
    }

    fn set_nav_history(
        &mut self,
        nav_history: ItemNavHistory,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.editor.update(cx, |editor, cx| {
            editor.rhs_editor().update(cx, |editor, _| {
                editor.set_nav_history(Some(nav_history));
            })
        });
    }

    fn navigate(
        &mut self,
        data: Arc<dyn Any + Send>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.editor.update(cx, |editor, cx| {
            editor
                .rhs_editor()
                .update(cx, |editor, cx| editor.navigate(data, window, cx))
        })
    }

    fn breadcrumb_location(&self, _: &App) -> ToolbarItemLocation {
        ToolbarItemLocation::Hidden
    }

    fn added_to_workspace(
        &mut self,
        workspace: &mut Workspace,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.editor.update(cx, |editor, cx| {
            editor.rhs_editor().update(cx, |editor, cx| {
                editor.added_to_workspace(workspace, window, cx)
            })
        });
    }

    fn can_save(&self, cx: &App) -> bool {
        self.editor.read(cx).rhs_editor().read(cx).can_save(cx)
    }

    fn is_dirty(&self, cx: &App) -> bool {
        self.buffer.read(cx).is_dirty()
    }

    fn save(
        &mut self,
        options: SaveOptions,
        project: Entity<Project>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<Result<()>> {
        self.editor.save(options, project, window, cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;
    use multi_buffer::MultiBufferRow;

    #[gpui::test]
    fn test_changes_only_multibuffer_has_one_buffer_and_expand_controls(cx: &mut TestAppContext) {
        let base_text = (0..20)
            .map(|line| format!("line {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        let current_text = base_text.replace("line 10", "changed line 10");
        let buffer = cx.new(|cx| Buffer::local(current_text, cx));
        let diff = cx.new(|cx| {
            buffer_diff::BufferDiff::new_with_base_text(
                &base_text,
                &buffer.read(cx).text_snapshot(),
                cx,
            )
        });
        let multibuffer = cx.new(|cx| SoloDiffView::build_multibuffer(buffer, diff, false, cx));

        let (is_singleton, buffer_count, has_expand_controls) =
            multibuffer.update(cx, |multibuffer, cx| {
                (
                    multibuffer.is_singleton(),
                    multibuffer.snapshot(cx).all_buffer_ids().count(),
                    multibuffer
                        .snapshot(cx)
                        .row_infos(MultiBufferRow(0))
                        .any(|row| row.expand_info.is_some()),
                )
            });

        assert!(!is_singleton);
        assert_eq!(buffer_count, 1);
        assert!(has_expand_controls);
    }
}
