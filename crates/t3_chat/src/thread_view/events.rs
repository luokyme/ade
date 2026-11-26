//! Rows for what happened around the agent rather than in its work: forks, threads it created,
//! context handed to another model, notifications, and pages it rendered with `html_render`.

use super::ThreadView;
use crate::{
    panel::T3Panel,
    style::{TOOL_TEXT_SIZE, accent, icon, text_faint, ui},
    transcript::{HtmlRender, html_render, single_line},
};
use gpui::{AnyElement, ClickEvent, Context, Hsla, SharedString, Window};
use i18n::t;
use serde_json::Value;
use std::path::PathBuf;
use t3_client::protocol::TurnItem;
use theme::ActiveTheme as _;
use ui::prelude::*;
use util::ResultExt as _;

const HTML_MIME_TYPE: &str = "text/html";
const MAX_FILE_STEM_LENGTH: usize = 120;

impl ThreadView {
    /// Opens another T3 Code thread where this one is shown: in the side panel or in a tab.
    fn open_other_thread(&self, thread_id: String, window: &mut Window, cx: &mut Context<Self>) {
        let in_editor = !self.docked;
        let workspace = self.workspace.clone();
        // The panel may replace this view, so it must not be updated while this one is.
        window.defer(cx, move |window, cx| {
            let Some(workspace) = workspace.upgrade() else {
                return;
            };
            let panel = workspace.read(cx).panel::<T3Panel>(cx);
            if let Some(panel) = panel {
                panel.update(cx, |panel, cx| {
                    panel.open_thread(thread_id, in_editor, window, cx)
                });
            }
        });
    }

    fn event_row(
        &self,
        name: IconName,
        label: String,
        color: Hsla,
        action: Option<(String, SharedString, String)>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        h_flex()
            .w_full()
            .gap(px(8.))
            .text_size(px(TOOL_TEXT_SIZE))
            .text_color(color)
            .child(icon(name, 12.).text_color(color))
            .child(div().min_w_0().truncate().child(label))
            .when_some(action, |this, (id, label, thread_id)| {
                this.child(
                    Button::new(SharedString::from(id), label)
                        .style(ButtonStyle::Subtle)
                        .size(ButtonSize::Compact)
                        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                            this.open_other_thread(thread_id.clone(), window, cx)
                        })),
                )
            })
            .into_any_element()
    }

    pub(super) fn render_fork(&self, item: &TurnItem, cx: &mut Context<Self>) -> AnyElement {
        let source = item.fields.get("source");
        let from_run = source
            .and_then(|source| source.get("type"))
            .and_then(Value::as_str)
            == Some("run");
        let (label, action, thread_id) = if from_run {
            (
                t!("Forked from conversation"),
                t!("Open source conversation"),
                source
                    .and_then(|source| source.get("threadId"))
                    .and_then(Value::as_str),
            )
        } else {
            (
                t!("Conversation fork"),
                t!("Open fork"),
                item.str_field("targetThreadId"),
            )
        };
        let action =
            thread_id.map(|thread_id| (format!("{}-open", item.id), action, thread_id.to_string()));
        self.event_row(
            IconName::GitBranch,
            label.to_string(),
            text_faint(cx),
            action,
            cx,
        )
    }

    pub(super) fn render_thread_created(
        &self,
        item: &TurnItem,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let label = match item.title.as_deref().map(str::trim) {
            Some(title) if !title.is_empty() => {
                format!("{} · {}", t!("Created thread"), single_line(title))
            }
            _ => t!("Created thread").to_string(),
        };
        let action = item.str_field("targetThreadId").map(|thread_id| {
            (
                format!("{}-open", item.id),
                t!("Open chat"),
                thread_id.to_string(),
            )
        });
        self.event_row(
            IconName::AgentChatRoundLine,
            label,
            text_faint(cx),
            action,
            cx,
        )
    }

    pub(super) fn render_handoff(&self, item: &TurnItem, cx: &mut Context<Self>) -> AnyElement {
        let model_of = |selection: &Value| {
            selection
                .get("model")
                .and_then(Value::as_str)
                .or_else(|| selection.get("instanceId").and_then(Value::as_str))
                .map(str::to_string)
        };
        let mut from = item
            .fields
            .get("fromModelSelections")
            .and_then(Value::as_array)
            .map(|selections| selections.iter().filter_map(model_of).collect::<Vec<_>>())
            .unwrap_or_default();
        if from.is_empty() {
            from = item
                .fields
                .get("fromProviderInstanceIds")
                .and_then(Value::as_array)
                .map(|ids| {
                    ids.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
        }
        let to = item
            .str_field("toModel")
            .or_else(|| item.str_field("toProviderInstanceId"))
            .unwrap_or("");
        let title = if item.is_running() {
            t!("Context handoff pending")
        } else {
            t!("Context handoff")
        };
        let label = if from.is_empty() {
            format!("{title} · {to}")
        } else {
            format!("{title} · {} → {to}", from.join(", "))
        };
        let color = if item.status == "failed" {
            cx.theme().status().error
        } else {
            text_faint(cx)
        };
        self.event_row(IconName::AgentArrowRight, label, color, None, cx)
    }

    pub(super) fn render_notification(
        &self,
        item: &TurnItem,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let summary = item.str_field("summary").unwrap_or("").trim();
        let detail = item.str_field("detail").unwrap_or("").trim();
        let label = if detail.is_empty() {
            single_line(summary)
        } else {
            format!("{} · {}", single_line(summary), single_line(detail))
        };
        let (name, color) = match item.str_field("outcome") {
            Some("failed") => (IconName::AgentDangerTriangle, cx.theme().status().error),
            Some("completed") => (IconName::AgentCheck, text_faint(cx)),
            Some("cancelled") => (IconName::AgentClose, text_faint(cx)),
            _ => (IconName::Bell, text_faint(cx)),
        };
        let action = item
            .fields
            .get("source")
            .and_then(|source| source.get("childThreadId"))
            .and_then(Value::as_str)
            .map(|thread_id| {
                (
                    format!("{}-open", item.id),
                    t!("Open chat"),
                    thread_id.to_string(),
                )
            });
        self.event_row(name, label, color, action, cx)
    }

    pub(super) fn render_html_page(&self, item: &TurnItem, cx: &mut Context<Self>) -> AnyElement {
        let Some(page) = html_render(item) else {
            return div().into_any_element();
        };
        let accent = accent(cx);
        let opening = self.fetch_tasks.contains_key(&page_task_key(&page));
        self.card(cx)
            .child(
                h_flex()
                    .w_full()
                    .gap(px(8.))
                    .justify_between()
                    .child(div().min_w_0().flex_1().child(self.card_title(
                        IconName::AgentDocument,
                        page.title.clone().into(),
                        accent,
                        cx,
                    )))
                    .child(
                        Button::new(
                            SharedString::from(format!("{}-open", item.id)),
                            t!("Open in browser"),
                        )
                        .style(ButtonStyle::Outlined)
                        .size(ButtonSize::Medium)
                        .disabled(opening)
                        .end_icon(Icon::new(IconName::ArrowUpRight).size(IconSize::XSmall))
                        .on_click(cx.listener(
                            move |this, _: &ClickEvent, _, cx| {
                                this.open_html_page(page.clone(), cx)
                            },
                        )),
                    ),
            )
            .child(
                div()
                    .text_size(ui(12.))
                    .text_color(text_faint(cx))
                    .child(t!("A page the agent rendered. Ade can't show it inline.")),
            )
            .into_any_element()
    }

    /// Downloads the page and opens it in the system browser.
    fn open_html_page(&mut self, page: HtmlRender, cx: &mut Context<Self>) {
        let key = page_task_key(&page);
        if self.fetch_tasks.contains_key(&key) {
            return;
        }
        let file_name = format!("{}.html", file_stem(&page.title));
        let download = self.store.read(cx).download_attachment(
            page.attachment_id.clone(),
            file_name.clone(),
            HTML_MIME_TYPE.to_string(),
            cx,
        );
        let directory = std::env::temp_dir()
            .join("ade-t3-pages")
            .join(file_stem(&page.attachment_id));
        let task = cx.spawn(async move |this, cx| {
            let saved = async {
                let bytes = download.await?;
                let path: PathBuf = directory.join(&file_name);
                cx.background_spawn({
                    let path = path.clone();
                    async move {
                        std::fs::create_dir_all(&directory)?;
                        std::fs::write(&path, bytes)?;
                        anyhow::Ok(())
                    }
                })
                .await?;
                anyhow::Ok(path)
            }
            .await;
            this.update(cx, |this, cx| {
                this.fetch_tasks.remove(&key);
                match saved {
                    Ok(path) => cx.open_with_system(&path),
                    Err(error) => {
                        this.error = Some(format!("{error:#}").into());
                    }
                }
                cx.notify();
            })
            .log_err();
        });
        self.fetch_tasks.insert(page_task_key(&page), task);
        cx.notify();
    }
}

fn page_task_key(page: &HtmlRender) -> String {
    format!("html-page:{}", page.attachment_id)
}

/// A file name that is safe on every platform, from a page title or id.
fn file_stem(text: &str) -> String {
    let cleaned = text
        .chars()
        .map(|character| {
            if character.is_control() || r#"\/:*?"<>|"#.contains(character) {
                ' '
            } else {
                character
            }
        })
        .collect::<String>();
    let stem = cleaned
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(MAX_FILE_STEM_LENGTH)
        .collect::<String>();
    if stem.is_empty() {
        "Page".to_string()
    } else {
        stem
    }
}
