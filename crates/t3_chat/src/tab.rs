//! T3 Code threads as editor tabs, and moving them between the editor and the side panel.

use crate::{
    MoveToPanel, T3Panel,
    model_picker::{provider_color, provider_icon},
    store::T3Store,
    thread_view::{ThreadView, ThreadViewEvent},
};
use anyhow::{Context as _, Result};
use db::kvp::KeyValueStore;
use gpui::{
    AnimationExt as _, AnyElement, App, AppContext as _, Context, Entity, SharedString, Task,
    Transformation, WeakEntity, Window, percentage, svg,
};
use i18n::t;
use project::Project;
use std::{collections::BTreeMap, time::Duration};
use ui::{Tooltip, prelude::*};
use util::ResultExt as _;
use workspace::{
    ItemId, Workspace, WorkspaceId,
    item::{Item, ItemEvent, SerializableItem, TabContentParams},
};

fn tabs_key(workspace_id: WorkspaceId) -> String {
    format!("t3_chat_tabs_{}", i64::from(workspace_id))
}

/// Which thread each open tab shows, keyed by the tab's item id.
fn read_tabs(workspace_id: WorkspaceId, cx: &App) -> BTreeMap<u64, String> {
    KeyValueStore::global(cx)
        .read_kvp(&tabs_key(workspace_id))
        .log_err()
        .flatten()
        .and_then(|value| serde_json::from_str(&value).log_err())
        .unwrap_or_default()
}

fn write_tabs(workspace_id: WorkspaceId, tabs: BTreeMap<u64, String>, cx: &App) -> Task<Result<()>> {
    let db = KeyValueStore::global(cx);
    let key = tabs_key(workspace_id);
    cx.background_spawn(async move {
        if tabs.is_empty() {
            db.delete_kvp(key).await
        } else {
            db.write_kvp(key, serde_json::to_string(&tabs)?).await
        }
    })
}

/// Moves a thread out of the side panel into the active editor pane, or focuses the tab that
/// already shows it.
pub fn open_in_editor(
    workspace: &mut Workspace,
    view: Entity<ThreadView>,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    if let Some(panel) = workspace.panel::<T3Panel>(cx) {
        panel.update(cx, |panel, cx| panel.release_view(&view, cx));
    }
    view.update(cx, |view, cx| view.set_docked(false, cx));
    workspace.add_item_to_active_pane(Box::new(view), None, true, window, cx);
}

pub fn move_to_panel(
    workspace: &mut Workspace,
    view: Entity<ThreadView>,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let Some(panel) = workspace.panel::<T3Panel>(cx) else {
        return;
    };
    for pane in workspace.panes().to_vec() {
        pane.update(cx, |pane, cx| {
            if pane.index_for_item(&view).is_some() {
                pane.remove_item(view.entity_id(), false, true, window, cx);
            }
        });
    }
    view.update(cx, |view, cx| view.set_docked(true, cx));
    panel.update(cx, |panel, cx| panel.show_view(view, window, cx));
    cx.defer_in(window, |workspace, window, cx| {
        workspace.focus_panel::<T3Panel>(window, cx);
    });
}

/// The tab showing `thread_id`, if one is open.
pub fn tab_for_thread(workspace: &Workspace, thread_id: &str, cx: &App) -> Option<Entity<ThreadView>> {
    workspace
        .items_of_type::<ThreadView>(cx)
        .find(|view| view.read(cx).thread_id() == Some(thread_id))
}

impl Item for ThreadView {
    type Event = ThreadViewEvent;

    fn tab_content(&self, params: TabContentParams, _: &Window, cx: &App) -> AnyElement {
        let waiting = self.needs_input(cx);
        let working = self.is_working();
        h_flex()
            .gap_1()
            .child(
                Label::new(self.title(cx))
                    .single_line()
                    .color(params.text_color()),
            )
            .when(waiting, |this| {
                this.child(div().size(px(6.)).rounded_full().bg(cx.theme().colors().text_accent))
            })
            .when(!waiting && working, |this| {
                this.child(
                    svg()
                        .path(IconName::LoadCircle.path())
                        .flex_none()
                        .size(IconSize::XSmall.rems())
                        .text_color(Color::Muted.color(cx))
                        .with_animation(
                            "t3-tab-working",
                            gpui::Animation::new(Duration::from_secs(2)).repeat(),
                            |icon, delta| icon.with_transformation(Transformation::rotate(percentage(delta))),
                        ),
                )
            })
            .into_any_element()
    }

    fn tab_content_text(&self, _detail: usize, cx: &App) -> SharedString {
        self.title(cx)
    }

    fn tab_tooltip_text(&self, cx: &App) -> Option<SharedString> {
        Some(self.title(cx))
    }

    fn tab_icon(&self, _: &Window, cx: &App) -> Option<Icon> {
        let driver = self.provider_driver(cx).unwrap_or_default();
        let icon = Icon::new(provider_icon(&driver));
        Some(match provider_color(&driver) {
            Some(color) => icon.color(Color::Custom(color)),
            None => icon,
        })
    }

    fn tab_bar_buttons(&mut self, _: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        vec![
            IconButton::new("t3-move-to-panel", IconName::AgentDockRight)
                .icon_size(IconSize::Small)
                .tooltip(Tooltip::text(t!("Move to Side Panel")))
                .on_click(cx.listener(|this, _, window, cx| this.move_to_panel(window, cx)))
                .into_any_element(),
        ]
    }

    fn tab_extra_context_menu_actions(
        &self,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Vec<(SharedString, Box<dyn gpui::Action>)> {
        vec![(t!("Move to Side Panel"), Box::new(MoveToPanel))]
    }

    fn to_item_events(event: &Self::Event, f: &mut dyn FnMut(ItemEvent)) {
        match event {
            ThreadViewEvent::Launched | ThreadViewEvent::Changed => f(ItemEvent::UpdateTab),
        }
    }
}

impl SerializableItem for ThreadView {
    fn serialized_item_kind() -> &'static str {
        "T3ChatThread"
    }

    fn cleanup(
        workspace_id: WorkspaceId,
        alive_items: Vec<ItemId>,
        _: &mut Window,
        cx: &mut App,
    ) -> Task<Result<()>> {
        let mut tabs = read_tabs(workspace_id, cx);
        tabs.retain(|item_id, _| alive_items.contains(item_id));
        write_tabs(workspace_id, tabs, cx)
    }

    fn deserialize(
        project: Entity<Project>,
        workspace: WeakEntity<Workspace>,
        workspace_id: WorkspaceId,
        item_id: ItemId,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Entity<Self>>> {
        let thread_id = read_tabs(workspace_id, cx).remove(&item_id);
        let Some(thread_id) = thread_id else {
            return Task::ready(Err(anyhow::anyhow!("no T3 Code thread saved for this tab")));
        };
        let root = project
            .read(cx)
            .visible_worktrees(cx)
            .next()
            .map(|worktree| worktree.read(cx).abs_path().to_path_buf());
        let Some(root) = root else {
            return Task::ready(Err(anyhow::anyhow!("the workspace has no folder for this thread")));
        };
        let languages = match workspace
            .upgrade()
            .map(|workspace| workspace.read(cx).app_state().languages.clone())
            .context("the workspace closed")
        {
            Ok(languages) => languages,
            Err(error) => return Task::ready(Err(error)),
        };
        let store = T3Store::global(cx);
        let project_id = store
            .read(cx)
            .shell()
            .project_for_root(&root)
            .map(|project| project.id.clone())
            .unwrap_or_default();
        let view = cx.new(|cx| {
            let mut view = ThreadView::new(
                store,
                workspace,
                languages,
                project_id,
                root,
                Some(thread_id),
                None,
                window,
                cx,
            );
            view.set_docked(false, cx);
            view
        });
        Task::ready(Ok(view))
    }

    fn serialize(
        &mut self,
        workspace: &mut Workspace,
        item_id: ItemId,
        _closing: bool,
        cx: &mut Context<Self>,
    ) -> Option<Task<Result<()>>> {
        let workspace_id = workspace.database_id()?;
        let thread_id = self.thread_id()?.to_string();
        let mut tabs = read_tabs(workspace_id, cx);
        if tabs.get(&item_id) == Some(&thread_id) {
            return None;
        }
        tabs.insert(item_id, thread_id);
        Some(write_tabs(workspace_id, tabs, cx))
    }

    fn should_serialize(&self, event: &Self::Event) -> bool {
        matches!(event, ThreadViewEvent::Launched)
    }
}
