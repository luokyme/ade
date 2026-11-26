//! Work the agent left running after its turn, such as a background build: a chip in the
//! composer footer and a card listing each task, with a way to stop them.

use super::ThreadView;
use crate::{
    style::{code_font, format_elapsed, icon, ink, text_faint, ui},
    transcript::unwrap_shell,
};
use gpui::{
    AnyElement, App, ClickEvent, Context, DismissEvent, Entity, EventEmitter, FocusHandle,
    Focusable, FontWeight, KeyDownEvent, SharedString, Subscription, Task, WeakEntity, Window,
    point,
};
use i18n::t;
use std::time::{Duration, Instant};
use t3_client::protocol::{BackgroundTask, commands};
use theme::ActiveTheme as _;
use ui::{ButtonLike, PopoverMenu, Tooltip, prelude::*};
use util::ResultExt as _;

const TICK: Duration = Duration::from_secs(1);
const CARD_WIDTH: f32 = 360.;
/// Like T3 Code, a stop can be sent again if the tasks are still listed after this long.
const STOP_RETRY_AFTER: Duration = Duration::from_secs(10);

pub(super) struct BackgroundStop {
    task_ids: Vec<String>,
    requested_at: Instant,
}

#[derive(Clone, Copy)]
enum TaskKind {
    Command,
    Subagent,
    Other,
}

impl TaskKind {
    fn of(task: &BackgroundTask) -> Self {
        match task.kind.as_str() {
            "command" => Self::Command,
            "subagent" => Self::Subagent,
            _ => Self::Other,
        }
    }

    fn running_label(self) -> SharedString {
        match self {
            Self::Command => t!("Command running"),
            Self::Subagent => t!("Agent running"),
            Self::Other => t!("Task running"),
        }
    }

    fn untitled(self) -> SharedString {
        match self {
            Self::Command => t!("Background command"),
            Self::Subagent => t!("Background agent"),
            Self::Other => t!("Background task"),
        }
    }

    fn icon(self) -> IconName {
        match self {
            Self::Command => IconName::AgentTerminal,
            Self::Subagent => IconName::AgentBot,
            Self::Other => IconName::AgentWidget,
        }
    }
}

struct TaskRow {
    kind: TaskKind,
    title: SharedString,
    command: Option<SharedString>,
    started_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl TaskRow {
    fn elapsed_secs(&self, now: chrono::DateTime<chrono::Utc>) -> Option<u64> {
        let started_at = self.started_at?;
        Some((now - started_at).num_seconds().max(0) as u64)
    }
}

impl ThreadView {
    fn background_tasks(&self, cx: &App) -> Vec<BackgroundTask> {
        self.thread_id
            .as_ref()
            .and_then(|thread_id| self.store.read(cx).shell().threads.get(thread_id))
            .map(|thread| thread.pending_background_tasks.clone())
            .unwrap_or_default()
    }

    pub(super) fn has_background_tasks(&self, cx: &App) -> bool {
        self.thread_id
            .as_ref()
            .and_then(|thread_id| self.store.read(cx).shell().threads.get(thread_id))
            .is_some_and(|thread| !thread.pending_background_tasks.is_empty())
    }

    fn background_rows(&self, cx: &App) -> Vec<TaskRow> {
        self.background_tasks(cx)
            .iter()
            .map(|task| {
                let kind = TaskKind::of(task);
                let item = task
                    .item_id
                    .as_deref()
                    .and_then(|item_id| self.items.iter().find(|item| item.id == item_id));
                let command = task
                    .command
                    .as_deref()
                    .map(str::trim)
                    .filter(|command| !command.is_empty())
                    .map(str::to_string)
                    .or_else(|| {
                        item.filter(|item| item.kind == "command_execution")
                            .and_then(|item| item.str_field("input"))
                            .map(unwrap_shell)
                    })
                    .map(SharedString::from);
                let title: SharedString = match task
                    .description
                    .as_deref()
                    .map(str::trim)
                    .filter(|description| !description.is_empty())
                {
                    Some(description) => SharedString::from(description.to_string()),
                    None => command.clone().unwrap_or_else(|| kind.untitled()),
                };
                let started_at = item
                    .and_then(|item| item.str_field("startedAt"))
                    .and_then(|timestamp| chrono::DateTime::parse_from_rfc3339(timestamp).ok())
                    .map(|timestamp| timestamp.with_timezone(&chrono::Utc));
                TaskRow {
                    kind,
                    command: command.filter(|command| *command != title),
                    title,
                    started_at,
                }
            })
            .collect()
    }

    fn background_stopping(&self, cx: &App) -> bool {
        let Some(stop) = &self.background_stop else {
            return false;
        };
        stop.requested_at.elapsed() < STOP_RETRY_AFTER
            && self
                .background_tasks(cx)
                .iter()
                .any(|task| stop.task_ids.contains(&task.task_id))
    }

    /// T3 Code has no way to stop one task: `thread.stop` ends everything the thread still runs.
    fn stop_background_tasks(&mut self, cx: &mut Context<Self>) {
        let Some(thread_id) = self.thread_id.clone() else {
            return;
        };
        if self.is_working() || self.background_stopping(cx) {
            return;
        }
        self.background_stop = Some(BackgroundStop {
            task_ids: self
                .background_tasks(cx)
                .into_iter()
                .map(|task| task.task_id)
                .collect(),
            requested_at: Instant::now(),
        });
        let task = self
            .store
            .read(cx)
            .dispatch(commands::stop_thread(&thread_id), cx);
        cx.spawn(async move |this, cx| {
            if let Err(error) = task.await {
                this.update(cx, |this, cx| {
                    this.background_stop = None;
                    this.error = Some(format!("{error:#}").into());
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
        cx.notify();
    }

    pub(super) fn render_background_indicator(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let rows = self.background_rows(cx);
        let label: SharedString = match rows.as_slice() {
            [] => return None,
            [row] => row.kind.running_label(),
            rows => t!("{count} tasks running", count = rows.len()),
        };
        let now = chrono::Utc::now();
        let elapsed = rows
            .iter()
            .filter_map(|row| row.elapsed_secs(now))
            .max()
            .map(format_elapsed);
        let colors = cx.theme().colors();
        let summary = h_flex()
            .h(px(22.))
            .px(px(6.))
            .gap(px(5.))
            .rounded(px(6.))
            .text_size(ui(12.))
            .whitespace_nowrap()
            .text_color(colors.text_muted)
            .hover(|style| style.bg(ink(0.06, cx)).text_color(colors.text))
            .child(
                div()
                    .size(px(6.))
                    .flex_none()
                    .rounded_full()
                    .bg(cx.theme().status().info),
            )
            .child(label)
            .children(elapsed.map(|elapsed| div().text_color(text_faint(cx)).child(elapsed)));
        let view = cx.weak_entity();
        Some(
            PopoverMenu::new("t3-background-tasks")
                .trigger(
                    ButtonLike::new("t3-background-tasks-trigger")
                        .style(ButtonStyle::Transparent)
                        .child(summary),
                )
                .menu(move |_, cx| {
                    let view = view.upgrade()?;
                    Some(cx.new(|cx| BackgroundTasksCard::new(view, cx)))
                })
                .anchor(gpui::Anchor::BottomRight)
                .attach(gpui::Anchor::TopRight)
                .offset(point(px(0.), px(-6.)))
                .into_any_element(),
        )
    }
}

struct BackgroundTasksCard {
    view: WeakEntity<ThreadView>,
    focus_handle: FocusHandle,
    _tick: Task<()>,
    _view_changed: Subscription,
}

impl BackgroundTasksCard {
    fn new(view: Entity<ThreadView>, cx: &mut Context<Self>) -> Self {
        let tick = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(TICK).await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    return;
                }
            }
        });
        Self {
            view: view.downgrade(),
            focus_handle: cx.focus_handle(),
            _tick: tick,
            _view_changed: cx.observe(&view, |_, _, cx| cx.notify()),
        }
    }
}

impl EventEmitter<DismissEvent> for BackgroundTasksCard {}

impl Focusable for BackgroundTasksCard {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for BackgroundTasksCard {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (rows, working, stopping) = self
            .view
            .upgrade()
            .map(|view| {
                let view = view.read(cx);
                (
                    view.background_rows(cx),
                    view.is_working(),
                    view.background_stopping(cx),
                )
            })
            .unwrap_or_else(|| (Vec::new(), false, false));
        let colors = cx.theme().colors();
        let muted = colors.text_muted;
        let faint = text_faint(cx);
        let hover = ink(0.06, cx);
        let font = code_font(cx);
        let now = chrono::Utc::now();
        let can_stop = !rows.is_empty() && !working && !stopping;
        let view = self.view.clone();
        let stop = div()
            .id("t3-stop-background-tasks")
            .flex_none()
            .h(px(20.))
            .px(px(6.))
            .flex()
            .items_center()
            .rounded(px(5.))
            .text_size(ui(11.))
            .text_color(if can_stop { muted } else { faint })
            .when(can_stop, |this| {
                this.cursor_pointer()
                    .hover(|style| style.bg(hover).text_color(colors.text))
            })
            .tooltip(Tooltip::text(if working {
                t!("Stop the current turn first")
            } else {
                t!("Stop everything this thread runs in the background")
            }))
            .child(if stopping {
                t!("Stopping…")
            } else {
                t!("Stop all")
            })
            .when(can_stop, |this| {
                this.on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                    view.update(cx, |view, cx| view.stop_background_tasks(cx))
                        .log_err();
                }))
            });
        v_flex()
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(|_, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    cx.emit(DismissEvent);
                }
            }))
            .on_mouse_down_out(cx.listener(|_, _, _, cx| cx.emit(DismissEvent)))
            .w(px(CARD_WIDTH))
            .p(px(4.))
            .gap(px(2.))
            .border_1()
            .border_color(colors.border)
            .rounded(px(12.))
            .shadow_lg()
            .bg(colors.elevated_surface_background)
            .text_size(ui(12.))
            .text_color(colors.text)
            .child(
                h_flex()
                    .pl(px(8.))
                    .pr(px(2.))
                    .pt(px(4.))
                    .pb(px(2.))
                    .child(
                        div()
                            .flex_1()
                            .text_size(ui(11.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(muted)
                            .child(t!("Background tasks")),
                    )
                    .child(stop),
            )
            .when(rows.is_empty(), |this| {
                this.child(
                    div()
                        .px(px(8.))
                        .pb(px(6.))
                        .text_color(muted)
                        .child(t!("Nothing is running in the background.")),
                )
            })
            .children(rows.into_iter().map(|row| {
                let elapsed = row.elapsed_secs(now).map(format_elapsed);
                v_flex()
                    .px(px(8.))
                    .py(px(3.))
                    .gap(px(2.))
                    .child(
                        h_flex()
                            .gap(px(8.))
                            .child(icon(row.kind.icon(), 14.).text_color(muted))
                            .child(div().flex_1().min_w_0().truncate().child(row.title))
                            .children(
                                elapsed.map(|elapsed| {
                                    div().flex_none().text_color(faint).child(elapsed)
                                }),
                            ),
                    )
                    .when_some(row.command, |this, command| {
                        this.child(
                            div()
                                .pl(px(22.))
                                .text_size(ui(11.))
                                .text_color(muted)
                                .font(font.clone())
                                .truncate()
                                .child(command),
                        )
                    })
            }))
    }
}
