//! Chats run by a T3 Code server. Ade is one more T3 Code client: threads started here also show
//! in T3 Code's web, desktop and mobile apps, and the other way round.

mod diff;
mod model_picker;
mod panel;
mod store;
mod style;
mod tab;
mod thread_view;
mod transcript;

use gpui::{App, actions};
use workspace::Workspace;

pub use panel::T3Panel;
pub use store::{Status, T3Store};

actions!(
    t3_chat,
    [
        /// Shows or hides the T3 Code panel.
        ToggleFocus,
        /// Starts a new T3 Code thread in this project.
        NewThread,
        /// Sends the message in the T3 Code message box.
        Send,
        /// Sends the message into the agent's current turn instead of queueing it.
        Steer,
        /// Stops the agent's current turn in this T3 Code thread.
        Stop,
        /// Connects to the T3 Code server again.
        Reconnect,
        /// Opens the T3 Code thread from the side panel in an editor tab.
        OpenInEditor,
        /// Moves the T3 Code thread in this tab back to the side panel.
        MoveToPanel,
        /// Highlights the next suggestion in the T3 Code message box.
        SelectNextCompletion,
        /// Highlights the previous suggestion in the T3 Code message box.
        SelectPreviousCompletion,
        /// Inserts the highlighted suggestion in the T3 Code message box.
        AcceptCompletion,
        /// Closes the suggestions in the T3 Code message box.
        DismissCompletion,
    ]
);

pub fn init(cx: &mut App) {
    workspace::register_agent_activity_source(store::project_agent_activity, cx);
    workspace::register_serializable_item::<thread_view::ThreadView>(cx);
    cx.observe_new(|workspace: &mut Workspace, _, _| {
        workspace
            .register_action(|workspace, _: &ToggleFocus, window, cx| {
                workspace.toggle_panel_focus::<T3Panel>(window, cx);
            })
            .register_action(|workspace, _: &NewThread, window, cx| {
                if let Some(panel) = workspace.focus_panel::<T3Panel>(window, cx) {
                    panel.update(cx, |panel, cx| panel.new_thread(window, cx));
                }
            })
            .register_action(|_, _: &Reconnect, _, cx| {
                T3Store::global(cx).update(cx, |store, cx| store.connect(false, cx));
            })
            .register_action(|workspace, _: &OpenInEditor, window, cx| {
                let view = workspace
                    .panel::<T3Panel>(cx)
                    .and_then(|panel| panel.read(cx).thread_view());
                if let Some(view) = view {
                    view.update(cx, |view, cx| view.open_in_editor(window, cx));
                }
            })
            .register_action(|workspace, _: &MoveToPanel, window, cx| {
                let view = workspace
                    .active_item(cx)
                    .and_then(|item| item.downcast::<thread_view::ThreadView>());
                if let Some(view) = view {
                    view.update(cx, |view, cx| view.move_to_panel(window, cx));
                }
            });
    })
    .detach();
}
