//! Cards an agent shows to ask the user for something only they can give: a secret
//! (`request_secret`), approval to send a report to the T3 team, or a link to another T3 Code
//! environment. Each is answered through its own server request, not a thread command, and the
//! thread then reports how it ended.

use super::ThreadView;
use crate::{
    model_picker::runtime_mode_info,
    style::{TOOL_TEXT_SIZE, accent, code_font, hairline, icon, icon_button, ink, text_faint, ui},
    transcript::{SecretField, secret_fields, single_line, web_page_host},
};
use anyhow::Result;
use editor::Editor;
use gpui::{
    AnyElement, ClickEvent, Context, Entity, FontWeight, Hsla, SharedString, Subscription, Task,
    Window,
};
use i18n::t;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use t3_client::protocol::{LINK_ACCESS_LEVELS, LinkAnswer, SecretAnswer, TurnItem};
use theme::ActiveTheme as _;
use ui::{Checkbox, ToggleState, Tooltip, prelude::*};

const DEFAULT_LINK_ACCESS: &str = "approval-required";
const MAX_TRACE_ROWS: usize = 12;

/// What the user entered into open cards, by item id.
#[derive(Default)]
pub(super) struct CardInputs {
    /// Text fields, keyed by item id and field.
    editors: HashMap<String, (Entity<Editor>, Subscription)>,
    revealed: HashSet<String>,
    sending: HashSet<String>,
    errors: HashMap<String, SharedString>,
    include_conversation: HashSet<String>,
    link_access: HashMap<String, String>,
}

impl CardInputs {
    fn clear_values(&mut self, item_id: &str) {
        let prefix = format!("{item_id}/");
        self.editors.retain(|key, _| !key.starts_with(&prefix));
        self.revealed.retain(|key| !key.starts_with(&prefix));
    }

    fn forget(&mut self, item_id: &str) {
        self.clear_values(item_id);
        self.sending.remove(item_id);
        self.errors.remove(item_id);
        self.include_conversation.remove(item_id);
        self.link_access.remove(item_id);
    }
}

fn input_key(item_id: &str, field: &str) -> String {
    format!("{item_id}/{field}")
}

impl ThreadView {
    /// A card inherited from the thread this one was forked from can only be answered there.
    fn answerable_here(&self, item: &TurnItem) -> bool {
        self.thread_id.as_deref() == Some(item.thread_id.as_str())
    }

    fn answered_row(&self, name: IconName, text: String, color: Hsla) -> AnyElement {
        h_flex()
            .w_full()
            .gap(px(8.))
            .text_size(px(TOOL_TEXT_SIZE))
            .text_color(color)
            .child(icon(name, 12.).text_color(color))
            .child(div().min_w_0().truncate().child(text))
            .into_any_element()
    }

    fn card_editor(
        &mut self,
        key: &str,
        placeholder: &str,
        initial_text: Option<&str>,
        masked: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<Editor> {
        if let Some((editor, _)) = self.cards.editors.get(key) {
            return editor.clone();
        }
        let editor = cx.new(|cx| {
            let mut editor = Editor::single_line(window, cx);
            editor.set_placeholder_text(placeholder, window, cx);
            if let Some(text) = initial_text {
                editor.set_text(text, window, cx);
            }
            editor.set_masked(masked, cx);
            editor
        });
        let subscription = cx.observe(&editor, |_, _, cx| cx.notify());
        self.cards
            .editors
            .insert(key.to_string(), (editor.clone(), subscription));
        editor
    }

    fn card_text(&self, key: &str, cx: &App) -> String {
        self.cards
            .editors
            .get(key)
            .map(|(editor, _)| editor.read(cx).text(cx).trim().to_string())
            .unwrap_or_default()
    }

    /// A text box in a card. Secret ones hide what is typed until the eye is clicked.
    fn card_input(
        &mut self,
        key: String,
        placeholder: &str,
        initial_text: Option<&str>,
        secret: Option<&str>,
        invalid: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let editor = self.card_editor(
            &key,
            placeholder,
            initial_text,
            secret.is_some(),
            window,
            cx,
        );
        let revealed = self.cards.revealed.contains(&key);
        let colors = cx.theme().colors();
        h_flex()
            .w_full()
            .min_w_0()
            .gap(px(4.))
            .pl(px(10.))
            .pr(px(4.))
            .py(px(4.))
            .min_h(px(32.))
            .rounded(px(8.))
            .border_1()
            .border_color(if invalid {
                cx.theme().status().error.opacity(0.6)
            } else {
                hairline(0.10, cx)
            })
            .bg(colors.editor_background)
            .child(div().flex_1().min_w_0().child(editor.clone()))
            .when_some(secret, |this, label| {
                let tooltip = if revealed {
                    t!("Hide {label}", label = label)
                } else {
                    t!("Show {label}", label = label)
                };
                this.child(
                    icon_button(
                        SharedString::from(format!("{key}-reveal")),
                        if revealed {
                            IconName::EyeOff
                        } else {
                            IconName::Eye
                        },
                        text_faint(cx),
                        cx,
                    )
                    .tooltip(Tooltip::text(tooltip))
                    .on_click(cx.listener(
                        move |this, _: &ClickEvent, _, cx| {
                            let revealed = !this.cards.revealed.remove(&key);
                            if revealed {
                                this.cards.revealed.insert(key.clone());
                            }
                            editor.update(cx, |editor, cx| editor.set_masked(!revealed, cx));
                            cx.notify();
                        },
                    )),
                )
            })
    }

    fn field_label(&self, label: SharedString, cx: &App) -> gpui::Div {
        div()
            .text_size(ui(12.))
            .font_weight(FontWeight::MEDIUM)
            .text_color(cx.theme().colors().text_muted)
            .child(label)
    }

    fn card_error(&self, item_id: &str, cx: &App) -> Option<AnyElement> {
        let error = self.cards.errors.get(item_id)?.clone();
        Some(
            div()
                .text_size(ui(12.))
                .text_color(cx.theme().status().error)
                .child(error)
                .into_any_element(),
        )
    }

    fn card_footnote(&self, name: IconName, text: SharedString, cx: &App) -> gpui::Div {
        h_flex()
            .min_w_0()
            .flex_1()
            .gap(px(6.))
            .text_size(ui(12.))
            .text_color(text_faint(cx))
            .child(icon(name, 12.).text_color(text_faint(cx)))
            .child(div().min_w_0().child(text))
    }

    /// Waits for an answer to reach the server. On success the card stays disabled until the
    /// thread reports how it ended; on failure it shows why and can be answered again.
    fn track_answer(&mut self, item_id: String, task: Task<Result<()>>, cx: &mut Context<Self>) {
        if !self.cards.sending.insert(item_id.clone()) {
            return;
        }
        self.cards.errors.remove(&item_id);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(()) => this.cards.clear_values(&item_id),
                    Err(error) => {
                        this.cards.sending.remove(&item_id);
                        this.cards
                            .errors
                            .insert(item_id, format!("{error:#}").into());
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn answer_secret(
        &mut self,
        thread_id: String,
        item_id: String,
        answer: SecretAnswer,
        cx: &mut Context<Self>,
    ) {
        if self.cards.sending.contains(&item_id) {
            return;
        }
        let task =
            self.store
                .read(cx)
                .answer_secret_request(thread_id, item_id.clone(), answer, cx);
        self.track_answer(item_id, task, cx);
    }

    fn answer_feedback(
        &mut self,
        thread_id: String,
        item_id: String,
        send: bool,
        cx: &mut Context<Self>,
    ) {
        if self.cards.sending.contains(&item_id) {
            return;
        }
        let include_conversation = self.cards.include_conversation.contains(&item_id);
        let task = self.store.read(cx).answer_feedback_request(
            thread_id,
            item_id.clone(),
            send,
            include_conversation,
            cx,
        );
        self.track_answer(item_id, task, cx);
    }

    fn answer_link(
        &mut self,
        thread_id: String,
        item_id: String,
        answer: LinkAnswer,
        cx: &mut Context<Self>,
    ) {
        if self.cards.sending.contains(&item_id) {
            return;
        }
        let task = self
            .store
            .read(cx)
            .answer_link_request(thread_id, item_id.clone(), answer, cx);
        self.track_answer(item_id, task, cx);
    }

    fn decline_button(
        &self,
        id: String,
        label: SharedString,
        disabled: bool,
        on_click: impl Fn(&mut Self, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> Button {
        Button::new(SharedString::from(id), label)
            .style(ButtonStyle::Subtle)
            .size(ButtonSize::Medium)
            .disabled(disabled)
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| on_click(this, cx)))
    }

    fn confirm_button(
        &self,
        id: String,
        label: SharedString,
        disabled: bool,
        on_click: impl Fn(&mut Self, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> Button {
        Button::new(SharedString::from(id), label)
            .style(ButtonStyle::Filled)
            .size(ButtonSize::Medium)
            .disabled(disabled)
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| on_click(this, cx)))
    }

    pub(super) fn render_secret_request(
        &mut self,
        item: &TurnItem,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let label = item.str_field("label").unwrap_or("").trim().to_string();
        let status = item.str_field("secretStatus").unwrap_or("pending");
        if status != "pending" || !self.answerable_here(item) {
            if status != "pending" {
                self.cards.forget(&item.id);
            }
            let (name, outcome) = match status {
                "pending" => (
                    IconName::Clock,
                    t!("Waiting for an answer in the original thread"),
                ),
                "saved" => (IconName::Lock, t!("Saved securely and kept private")),
                "declined" => (IconName::AgentClose, t!("Declined")),
                _ => (IconName::AgentClose, t!("Request ended")),
            };
            return self.answered_row(
                name,
                format!("{} · {outcome}", single_line(&label)),
                text_faint(cx),
            );
        }

        let item_id = item.id.clone();
        let thread_id = item.thread_id.clone();
        let fields = secret_fields(item);
        let named_fields = item.fields.contains_key("fields");
        let sending = self.cards.sending.contains(&item_id);
        let invalid = self.cards.errors.contains_key(&item_id);
        let colors = cx.theme().colors().clone();
        let accent = accent(cx);

        let mut inputs = Vec::new();
        for field in &fields {
            let input = self.card_input(
                input_key(&item_id, &field.key),
                &field.placeholder,
                None,
                field.secret.then_some(field.label.as_str()),
                invalid,
                window,
                cx,
            );
            inputs.push(if named_fields {
                v_flex()
                    .gap(px(4.))
                    .child(self.field_label(field.label.clone().into(), cx))
                    .child(input)
                    .into_any_element()
            } else {
                input.into_any_element()
            });
        }

        let reason = item.str_field("reason").unwrap_or("").trim().to_string();
        let page = item
            .str_field("url")
            .and_then(|url| Some((url.to_string(), web_page_host(url)?.to_string())));
        let footnote = match (item.str_field("delivery"), named_fields) {
            (Some("file"), false) => t!(
                "Kept out of chat, in a private file this agent's commands can read for 24 hours"
            ),
            (Some("file"), true) => {
                t!("Kept out of chat, in private files this agent's commands can read for 24 hours")
            }
            _ => t!("Stored securely, never shown to the agent"),
        };
        let ready = !sending
            && self
                .secret_answer(&item_id, &fields, named_fields, cx)
                .is_some();
        let save = {
            let item_id = item_id.clone();
            let thread_id = thread_id.clone();
            let fields = fields.clone();
            move |this: &mut Self, cx: &mut Context<Self>| {
                if let Some(answer) = this.secret_answer(&item_id, &fields, named_fields, cx) {
                    this.answer_secret(thread_id.clone(), item_id.clone(), answer, cx);
                }
            }
        };
        let save_on_enter = save.clone();
        let decline = {
            let item_id = item_id.clone();
            move |this: &mut Self, cx: &mut Context<Self>| {
                this.answer_secret(
                    thread_id.clone(),
                    item_id.clone(),
                    SecretAnswer::Decline,
                    cx,
                )
            }
        };

        self.card(cx)
            .border_color(accent.opacity(0.35))
            .gap(px(12.))
            .on_action(cx.listener(move |this, _: &menu::Confirm, _, cx| save_on_enter(this, cx)))
            .child(
                v_flex()
                    .gap(px(4.))
                    .child(self.card_title(IconName::Lock, label.into(), accent, cx))
                    .when(!reason.is_empty(), |this| {
                        this.child(div().text_color(colors.text_muted).child(reason))
                    }),
            )
            .when_some(page, |this, (url, host)| {
                this.child(
                    v_flex()
                        .items_start()
                        .gap(px(4.))
                        .child(
                            Button::new(
                                SharedString::from(format!("{item_id}-open")),
                                t!("Open {host}", host = host),
                            )
                            .style(ButtonStyle::Outlined)
                            .size(ButtonSize::Medium)
                            .end_icon(Icon::new(IconName::ArrowUpRight).size(IconSize::XSmall))
                            .on_click({
                                let url = url.clone();
                                move |_, _, cx| cx.open_url(&url)
                            }),
                        )
                        .child(
                            div()
                                .font(code_font(cx))
                                .text_size(ui(11.))
                                .text_color(text_faint(cx))
                                .child(url),
                        ),
                )
            })
            .child(v_flex().gap(px(10.)).children(inputs))
            .children(self.card_error(&item_id, cx))
            .child(
                h_flex()
                    .w_full()
                    .flex_wrap()
                    .gap(px(8.))
                    .justify_between()
                    .child(self.card_footnote(IconName::Lock, footnote, cx))
                    .child(
                        h_flex()
                            .flex_none()
                            .gap(px(6.))
                            .child(self.decline_button(
                                format!("{item_id}-decline"),
                                t!("Decline"),
                                sending,
                                decline,
                                cx,
                            ))
                            .child(self.confirm_button(
                                format!("{item_id}-save"),
                                t!("Save securely"),
                                !ready,
                                save,
                                cx,
                            )),
                    ),
            )
            .into_any_element()
    }

    /// The values typed into a secret card, once every field has one.
    fn secret_answer(
        &self,
        item_id: &str,
        fields: &[SecretField],
        named_fields: bool,
        cx: &App,
    ) -> Option<SecretAnswer> {
        let mut values = Vec::new();
        for field in fields {
            let value = self.card_text(&input_key(item_id, &field.key), cx);
            if value.is_empty() {
                return None;
            }
            values.push((field.key.clone(), value));
        }
        if named_fields {
            return (!values.is_empty()).then_some(SecretAnswer::SaveValues(values));
        }
        let (_, value) = values.into_iter().next()?;
        Some(SecretAnswer::Save(value))
    }

    pub(super) fn render_feedback_request(
        &mut self,
        item: &TurnItem,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let status = item.str_field("feedbackStatus").unwrap_or("pending");
        let preview = item.fields.get("preview").cloned().unwrap_or(Value::Null);
        let field = |name: &str| {
            preview
                .get(name)
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string()
        };
        let subject = field("subject");
        if status != "pending" || !self.answerable_here(item) {
            if status != "pending" {
                self.cards.forget(&item.id);
            }
            let (name, outcome) = match status {
                "pending" => (IconName::Clock, t!("Waiting in the original thread")),
                "sent" => (IconName::AgentCheck, t!("Sent")),
                "declined" => (IconName::AgentClose, t!("Not sent")),
                _ => (IconName::AgentClose, t!("Closed without sending")),
            };
            return self.answered_row(
                name,
                format!(
                    "{} · {} · {outcome}",
                    t!("Feedback for the T3 team"),
                    single_line(&subject)
                ),
                text_faint(cx),
            );
        }

        let item_id = item.id.clone();
        let thread_id = item.thread_id.clone();
        let sending = self.cards.sending.contains(&item_id);
        let include = self.cards.include_conversation.contains(&item_id);
        let colors = cx.theme().colors().clone();
        let accent = accent(cx);
        let kind = match field("kind").as_str() {
            "bug" => t!("Bug"),
            "confusing" => t!("Confusing"),
            "missing_capability" => t!("Missing capability"),
            "wrong_docs" => t!("Wrong docs"),
            _ => t!("Other"),
        };
        let rows = [
            (t!("Kind"), kind.to_string()),
            (t!("About"), subject),
            (t!("Expected"), field("expected")),
            (t!("Happened"), field("actual")),
            (t!("Did instead"), field("workaround")),
        ];
        let details = rows
            .into_iter()
            .filter(|(_, value)| !value.is_empty())
            .map(|(label, value)| {
                v_flex()
                    .gap(px(2.))
                    .child(self.field_label(label, cx))
                    .child(div().text_color(colors.text).child(value))
            })
            .collect::<Vec<_>>();
        let trace = preview
            .get("trace")
            .and_then(|trace| trace.get("entries"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let trace_rows = trace
            .iter()
            .take(MAX_TRACE_ROWS)
            .map(|entry| {
                let turn = entry.get("turn").and_then(Value::as_i64).unwrap_or(0);
                let what = match entry.get("type").and_then(Value::as_str) {
                    Some("user") => t!("You").to_string(),
                    Some("assistant") => t!("Agent").to_string(),
                    _ => {
                        let tool = entry.get("tool").and_then(Value::as_str).unwrap_or("");
                        match entry.get("outcome").and_then(Value::as_str) {
                            Some("error") => format!("{tool} · {}", t!("Failure")),
                            _ => tool.to_string(),
                        }
                    }
                };
                div()
                    .text_size(ui(12.))
                    .text_color(colors.text_muted)
                    .child(t!("Turn {turn} · {what}", turn = turn, what = what))
            })
            .collect::<Vec<_>>();
        let more_trace = trace.len().saturating_sub(MAX_TRACE_ROWS);
        let text_turns = item
            .fields
            .get("conversationTrace")
            .and_then(|trace| trace.get("textTurns"))
            .and_then(Value::as_i64);
        let checkbox = text_turns.map(|text_turns| {
            let this = cx.entity().downgrade();
            let toggle_id = item_id.clone();
            v_flex()
                .gap(px(2.))
                .child(
                    Checkbox::new(
                        SharedString::from(format!("{item_id}-conversation")),
                        ToggleState::from(include),
                    )
                    .label(t!("Include the conversation"))
                    .disabled(sending)
                    .on_click(move |_, _, cx| {
                        this.update(cx, |this, cx| {
                            if !this.cards.include_conversation.remove(&toggle_id) {
                                this.cards.include_conversation.insert(toggle_id.clone());
                            }
                            cx.notify();
                        })
                        .ok();
                    }),
                )
                .child(
                    div()
                        .pl(px(22.))
                        .text_size(ui(12.))
                        .text_color(text_faint(cx))
                        .child(t!(
                            "Adds the text of your messages and the agent's replies in the last {count} turns.",
                            count = text_turns
                        )),
                )
        });
        let send = {
            let item_id = item_id.clone();
            let thread_id = thread_id.clone();
            move |this: &mut Self, cx: &mut Context<Self>| {
                this.answer_feedback(thread_id.clone(), item_id.clone(), true, cx)
            }
        };
        let decline = {
            let item_id = item_id.clone();
            move |this: &mut Self, cx: &mut Context<Self>| {
                this.answer_feedback(thread_id.clone(), item_id.clone(), false, cx)
            }
        };

        self.card(cx)
            .border_color(accent.opacity(0.35))
            .gap(px(12.))
            .child(self.card_title(
                IconName::Send,
                t!("Send this report to the T3 team?"),
                accent,
                cx,
            ))
            .child(v_flex().gap(px(8.)).children(details))
            .when(!trace_rows.is_empty(), |this| {
                this.child(
                    v_flex()
                        .gap(px(2.))
                        .p(px(8.))
                        .rounded(px(8.))
                        .bg(ink(0.03, cx))
                        .children(trace_rows)
                        .when(more_trace > 0, |this| {
                            this.child(
                                div()
                                    .text_size(ui(12.))
                                    .text_color(text_faint(cx))
                                    .child(format!("+{more_trace}")),
                            )
                        }),
                )
            })
            .children(checkbox)
            .children(self.card_error(&item_id, cx))
            .child(
                h_flex()
                    .w_full()
                    .flex_wrap()
                    .gap(px(8.))
                    .justify_between()
                    .child(self.card_footnote(
                        IconName::Lock,
                        t!("This is everything that leaves your machine. Nothing is sent unless you press Send."),
                        cx,
                    ))
                    .child(
                        h_flex()
                            .flex_none()
                            .gap(px(6.))
                            .child(self.decline_button(
                                format!("{item_id}-decline"),
                                t!("Don't send"),
                                sending,
                                decline,
                                cx,
                            ))
                            .child(self.confirm_button(
                                format!("{item_id}-send"),
                                t!("Send"),
                                sending,
                                send,
                                cx,
                            )),
                    ),
            )
            .into_any_element()
    }

    pub(super) fn render_link_request(
        &mut self,
        item: &TurnItem,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let status = item.str_field("linkStatus").unwrap_or("pending");
        let named_url = item.str_field("url").map(str::trim).map(str::to_string);
        let target = item
            .str_field("label")
            .filter(|_| named_url.is_none())
            .map(str::to_string)
            .or_else(|| named_url.clone());
        if status != "pending" || !self.answerable_here(item) {
            if status != "pending" {
                self.cards.forget(&item.id);
            }
            let linked = item
                .str_field("linkedLabel")
                .map(str::to_string)
                .or_else(|| named_url.clone());
            let (name, outcome, failed) = match status {
                "pending" => (
                    IconName::Clock,
                    t!("Waiting for an answer in the original thread").to_string(),
                    false,
                ),
                "linked" => {
                    let access = item.str_field("linkedAccess").map(access_label);
                    let text = match (linked, access) {
                        (Some(target), Some(access)) => {
                            format!("{} · {access}", t!("Linked {target}", target = target))
                        }
                        (Some(target), None) => t!("Linked {target}", target = target).to_string(),
                        (None, Some(access)) => format!("{} · {access}", t!("Linked a machine")),
                        (None, None) => t!("Linked a machine").to_string(),
                    };
                    (IconName::Link, text, false)
                }
                "declined" => (IconName::AgentClose, t!("Declined").to_string(), false),
                "failed" => (
                    IconName::AgentDangerTriangle,
                    match item.str_field("failure") {
                        Some(failure) => {
                            t!("Could not link: {failure}", failure = failure).to_string()
                        }
                        None => t!("Could not link: the other environment refused").to_string(),
                    },
                    true,
                ),
                _ => (IconName::AgentClose, t!("Request ended").to_string(), false),
            };
            let color = if failed {
                cx.theme().status().error
            } else {
                text_faint(cx)
            };
            return self.answered_row(name, outcome, color);
        }

        let item_id = item.id.clone();
        let thread_id = item.thread_id.clone();
        let sending = self.cards.sending.contains(&item_id);
        let invalid = self.cards.errors.contains_key(&item_id);
        let colors = cx.theme().colors().clone();
        let accent = accent(cx);
        let access = self
            .cards
            .link_access
            .get(&item_id)
            .cloned()
            .or_else(|| {
                item.str_field("requestedAccess")
                    .filter(|access| LINK_ACCESS_LEVELS.contains(access))
                    .map(str::to_string)
            })
            .unwrap_or_else(|| DEFAULT_LINK_ACCESS.to_string());
        let title = match &target {
            Some(target) => t!("Link {target} to this environment", target = target),
            None => t!("Link a machine to this environment"),
        };
        let reason = item.str_field("reason").unwrap_or("").trim().to_string();
        let address_key = input_key(&item_id, "address");
        let code_key = input_key(&item_id, "pairingCode");
        let address = self.card_input(
            address_key.clone(),
            "https://",
            named_url.as_deref(),
            None,
            invalid,
            window,
            cx,
        );
        let code_label = t!("Pairing code");
        let code = self.card_input(
            code_key.clone(),
            &t!("Paste the code"),
            None,
            Some(&code_label),
            invalid,
            window,
            cx,
        );
        let access_chips = LINK_ACCESS_LEVELS
            .iter()
            .map(|level| {
                let selected = *level == access;
                let item_id = item_id.clone();
                div()
                    .id(SharedString::from(format!("{item_id}-access-{level}")))
                    .px(px(8.))
                    .py(px(3.))
                    .rounded(px(6.))
                    .border_1()
                    .text_size(ui(12.))
                    .border_color(if selected {
                        accent.opacity(0.5)
                    } else {
                        hairline(0.10, cx)
                    })
                    .when(selected, |this| this.bg(accent.opacity(0.08)))
                    .text_color(if selected {
                        colors.text
                    } else {
                        colors.text_muted
                    })
                    .cursor_pointer()
                    .hover(|style| style.bg(ink(0.04, cx)))
                    .child(access_label(level))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.cards
                            .link_access
                            .insert(item_id.clone(), level.to_string());
                        cx.notify();
                    }))
            })
            .collect::<Vec<_>>();
        let ready = !sending
            && !self.card_text(&address_key, cx).is_empty()
            && !self.card_text(&code_key, cx).is_empty();
        let link = {
            let item_id = item_id.clone();
            let thread_id = thread_id.clone();
            move |this: &mut Self, cx: &mut Context<Self>| {
                let url = this.card_text(&address_key, cx);
                let pairing_code = this.card_text(&code_key, cx);
                if url.is_empty() || pairing_code.is_empty() {
                    return;
                }
                let access = this
                    .cards
                    .link_access
                    .get(&item_id)
                    .cloned()
                    .unwrap_or_else(|| access.clone());
                this.answer_link(
                    thread_id.clone(),
                    item_id.clone(),
                    LinkAnswer::Link {
                        url,
                        access,
                        pairing_code,
                    },
                    cx,
                );
            }
        };
        let link_on_enter = link.clone();
        let decline = {
            let item_id = item_id.clone();
            move |this: &mut Self, cx: &mut Context<Self>| {
                this.answer_link(thread_id.clone(), item_id.clone(), LinkAnswer::Decline, cx)
            }
        };

        self.card(cx)
            .border_color(accent.opacity(0.35))
            .gap(px(12.))
            .on_action(cx.listener(move |this, _: &menu::Confirm, _, cx| {
                link_on_enter(this, cx)
            }))
            .child(
                v_flex()
                    .gap(px(4.))
                    .child(self.card_title(IconName::Link, title, accent, cx))
                    .when(!reason.is_empty(), |this| {
                        this.child(div().text_color(colors.text_muted).child(reason))
                    }),
            )
            .child(
                v_flex()
                    .gap(px(10.))
                    .child(
                        v_flex()
                            .gap(px(4.))
                            .child(self.field_label(t!("Address"), cx))
                            .child(address),
                    )
                    .child(
                        v_flex()
                            .gap(px(4.))
                            .child(self.field_label(code_label.clone(), cx))
                            .child(code)
                            .child(
                                div()
                                    .text_size(ui(12.))
                                    .text_color(text_faint(cx))
                                    .child(t!("Create a link on a machine: Settings → Sharing & access → Authorized clients → Create link, then paste its code here.")),
                            ),
                    )
                    .child(
                        v_flex()
                            .gap(px(4.))
                            .child(self.field_label(t!("Access"), cx))
                            .child(h_flex().flex_wrap().gap(px(6.)).children(access_chips)),
                    ),
            )
            .children(self.card_error(&item_id, cx))
            .child(
                h_flex()
                    .w_full()
                    .flex_wrap()
                    .gap(px(8.))
                    .justify_between()
                    .child(self.card_footnote(
                        IconName::Link,
                        t!("Agents here can then launch, message and wait on threads there. Revoke it in that machine's Sharing & access."),
                        cx,
                    ))
                    .child(
                        h_flex()
                            .flex_none()
                            .gap(px(6.))
                            .child(self.decline_button(
                                format!("{item_id}-decline"),
                                t!("Decline"),
                                sending,
                                decline,
                                cx,
                            ))
                            .child(self.confirm_button(
                                format!("{item_id}-link"),
                                t!("Link"),
                                !ready,
                                link,
                                cx,
                            )),
                    ),
            )
            .into_any_element()
    }
}

fn access_label(access: &str) -> SharedString {
    if access == "read-only" {
        t!("Read only")
    } else {
        runtime_mode_info(access).1
    }
}
