//! Chats run by a T3 Code server. Ade is one more T3 Code client: threads started here also show
//! in T3 Code's web, desktop and mobile apps, and the other way round.

mod panel;
mod store;
mod thread_view;

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
        /// Stops the agent's current turn in this T3 Code thread.
        Stop,
        /// Connects to the T3 Code server again.
        Reconnect,
    ]
);

pub fn init(cx: &mut App) {
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
            });
    })
    .detach();
}
