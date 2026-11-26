//! Files and images sent with a message: attached from a picker, dropped on the thread, or
//! pasted, then shown as chips in the composer and on the sent message.

use super::{AttachmentSource, PendingAttachment, ThreadView};
use crate::style::{hairline, icon, ink, ui};
use file_icons::FileIcons;
use gpui::{
    AnyElement, App, ClickEvent, ClipboardEntry, Context, ImageFormat, ObjectFit, PathPromptOptions,
    SharedString, Window, img,
};
use i18n::t;
use serde_json::Value;
use std::{path::PathBuf, sync::Arc};
use t3_client::protocol::{MAX_FILE_BYTES, TurnItem, attachment_media_type};
use theme::ActiveTheme as _;
use ui::{Tooltip, prelude::*};
use util::ResultExt as _;

const THUMB_SIZE: f32 = 44.;
const SENT_IMAGE_MAX: f32 = 240.;
/// More than this many files per message is more likely a mistaken drop than intent.
const MAX_ATTACHMENTS: usize = 20;

fn extension(format: ImageFormat) -> &'static str {
    match format {
        ImageFormat::Png => "png",
        ImageFormat::Jpeg => "jpg",
        ImageFormat::Webp => "webp",
        ImageFormat::Gif => "gif",
        ImageFormat::Svg => "svg",
        ImageFormat::Bmp => "bmp",
        ImageFormat::Tiff => "tiff",
        _ => "png",
    }
}

impl ThreadView {
    pub(super) fn attach_paths(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        for path in paths {
            if self.attachments.len() >= MAX_ATTACHMENTS {
                self.error = Some(t!("A message can carry up to {count} files.", count = MAX_ATTACHMENTS));
                break;
            }
            let Some(name) = path.file_name().map(|name| name.to_string_lossy().into_owned()) else {
                continue;
            };
            match std::fs::metadata(&path) {
                Ok(metadata) if metadata.is_dir() => {
                    self.error = Some(t!("{name} is a folder; mention it with @ instead.", name = name));
                    continue;
                }
                Ok(metadata) if metadata.len() > MAX_FILE_BYTES => {
                    self.error = Some(t!("{name} is larger than 50 MB.", name = name));
                    continue;
                }
                Ok(_) => {}
                Err(error) => {
                    self.error = Some(format!("{name}: {error}").into());
                    continue;
                }
            }
            if self
                .attachments
                .iter()
                .any(|attached| matches!(&attached.source, AttachmentSource::Path(existing) if *existing == path))
            {
                continue;
            }
            let (_, is_image) = attachment_media_type(&name);
            self.attachments.push(PendingAttachment {
                name,
                source: AttachmentSource::Path(path),
                is_image,
            });
        }
        cx.notify();
    }

    pub(super) fn pick_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some(t!("Attach")),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Some(paths) = picked.await.ok().and_then(|result| result.log_err()).flatten() else {
                return;
            };
            this.update(cx, |this, cx| this.attach_paths(paths, cx)).log_err();
        })
        .detach();
    }

    /// Attaches images or files from the clipboard. Returns false when it held neither, so the
    /// editor pastes the text itself.
    pub(super) fn paste_attachments(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(item) = cx.read_from_clipboard() else {
            return false;
        };
        let mut attached = false;
        for entry in item.entries() {
            match entry {
                ClipboardEntry::Image(image) => {
                    let name = format!(
                        "pasted-image-{}.{}",
                        self.attachments.len() + 1,
                        extension(image.format)
                    );
                    let (_, is_image) = attachment_media_type(&name);
                    self.attachments.push(PendingAttachment {
                        name,
                        source: AttachmentSource::Clipboard(Arc::new(image.clone())),
                        is_image,
                    });
                    attached = true;
                }
                ClipboardEntry::ExternalPaths(paths) => {
                    self.attach_paths(paths.paths().to_vec(), cx);
                    attached = true;
                }
                _ => {}
            }
        }
        if attached {
            cx.notify();
        }
        attached
    }

    pub(super) fn render_attachment_strip(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.attachments.is_empty() {
            return None;
        }
        let colors = cx.theme().colors();
        Some(
            h_flex()
                .flex_wrap()
                .gap(px(6.))
                .px(px(12.))
                .pt(px(10.))
                .children(self.attachments.iter().enumerate().map(|(index, attachment)| {
                    let preview = match (&attachment.source, attachment.is_image) {
                        (AttachmentSource::Path(path), true) => img(path.clone())
                            .size(px(THUMB_SIZE - 2.))
                            .rounded(px(7.))
                            .object_fit(ObjectFit::Cover)
                            .into_any_element(),
                        (AttachmentSource::Clipboard(image), _) => img(image.clone())
                            .size(px(THUMB_SIZE - 2.))
                            .rounded(px(7.))
                            .object_fit(ObjectFit::Cover)
                            .into_any_element(),
                        (AttachmentSource::Path(path), false) => {
                            file_icon(path, cx).size(IconSize::Medium).into_any_element()
                        }
                    };
                    let group: SharedString = format!("t3-attachment-{index}").into();
                    h_flex()
                        .group(group.clone())
                        .relative()
                        .h(px(THUMB_SIZE))
                        .max_w(px(220.))
                        .gap(px(8.))
                        .pr(px(10.))
                        .rounded(px(8.))
                        .border_1()
                        .border_color(hairline(0.10, cx))
                        .bg(ink(0.03, cx))
                        .child(
                            div()
                                .size(px(THUMB_SIZE - 2.))
                                .flex_none()
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(7.))
                                .overflow_hidden()
                                .child(preview),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_size(ui(12.))
                                .text_color(colors.text_muted)
                                .child(attachment.name.clone()),
                        )
                        .child(
                            div()
                                .id(SharedString::from(format!("t3-remove-attachment-{index}")))
                                .absolute()
                                .top(px(-6.))
                                .right(px(-6.))
                                .size(px(18.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded_full()
                                .border_1()
                                .border_color(colors.border)
                                .bg(colors.elevated_surface_background)
                                .cursor_pointer()
                                .visible_on_hover(group)
                                .tooltip(Tooltip::text(t!("Remove")))
                                .child(icon(IconName::AgentClose, 10.).text_color(colors.text_muted))
                                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                    if index < this.attachments.len() {
                                        this.attachments.remove(index);
                                    }
                                    cx.notify();
                                })),
                        )
                }))
                .into_any_element(),
        )
    }

    /// Starts downloading the images of sent messages that are not cached yet. `None` in the cache
    /// marks a download in flight or failed, so a missing image is retried only after a reconnect.
    pub(super) fn fetch_sent_images(&mut self, cx: &mut Context<Self>) {
        let wanted = self
            .items
            .iter()
            .filter_map(|item| item.fields.get("attachments")?.as_array())
            .flatten()
            .filter(|attachment| attachment.get("type").and_then(Value::as_str) == Some("image"))
            .filter_map(|attachment| {
                let id = attachment.get("id").and_then(Value::as_str)?;
                let name = attachment.get("name").and_then(Value::as_str).unwrap_or(id);
                let mime_type = attachment.get("mimeType").and_then(Value::as_str).unwrap_or("image/png");
                Some((id.to_string(), name.to_string(), mime_type.to_string()))
            })
            .filter(|(id, _, _)| !self.sent_images.contains_key(id))
            .collect::<Vec<_>>();
        for (id, name, mime_type) in wanted {
            self.sent_images.insert(id.clone(), None);
            let format = ImageFormat::from_mime_type(&mime_type).unwrap_or(ImageFormat::Png);
            let download = self.store.read(cx).download_attachment(id.clone(), name, mime_type, cx);
            self.fetch_tasks.insert(
                format!("sent-image-{id}"),
                cx.spawn(async move |this, cx| {
                    let result = download.await;
                    this.update(cx, |this, cx| {
                        this.fetch_tasks.remove(&format!("sent-image-{id}"));
                        match result {
                            Ok(bytes) => {
                                this.sent_images
                                    .insert(id, Some(Arc::new(gpui::Image::from_bytes(format, bytes))));
                            }
                            Err(error) => log::warn!("downloading a sent image failed: {error:#}"),
                        }
                        cx.notify();
                    })
                    .log_err();
                }),
            );
        }
    }

    /// Chips for the files a sent message carried.
    pub(super) fn render_sent_attachments(&self, item: &TurnItem, cx: &App) -> Option<AnyElement> {
        let attachments = item.fields.get("attachments")?.as_array()?;
        if attachments.is_empty() {
            return None;
        }
        let colors = cx.theme().colors();
        Some(
            h_flex()
                .flex_wrap()
                .justify_end()
                .gap(px(6.))
                .pb(px(6.))
                .children(attachments.iter().filter_map(|attachment| {
                    let name = attachment.get("name").and_then(Value::as_str)?.to_string();
                    let is_image = attachment.get("type").and_then(Value::as_str) == Some("image");
                    let loaded = attachment
                        .get("id")
                        .and_then(Value::as_str)
                        .and_then(|id| self.sent_images.get(id))
                        .cloned()
                        .flatten();
                    if let Some(image) = loaded.filter(|_| is_image) {
                        return Some(
                            div()
                                .rounded(px(10.))
                                .border_1()
                                .border_color(hairline(0.10, cx))
                                .overflow_hidden()
                                .child(
                                    img(image)
                                        .max_w(px(SENT_IMAGE_MAX))
                                        .max_h(px(SENT_IMAGE_MAX))
                                        .object_fit(ObjectFit::Contain),
                                )
                                .into_any_element(),
                        );
                    }
                    Some(
                        h_flex()
                            .h(px(28.))
                            .max_w(px(240.))
                            .px(px(8.))
                            .gap(px(6.))
                            .rounded(px(8.))
                            .border_1()
                            .border_color(hairline(0.10, cx))
                            .bg(ink(0.03, cx))
                            .text_size(ui(12.))
                            .text_color(colors.text_muted)
                            .child(if is_image {
                                Icon::new(IconName::Image).size(IconSize::Small).color(Color::Muted)
                            } else {
                                file_icon(&PathBuf::from(&name), cx)
                            })
                            .child(div().min_w_0().truncate().child(name))
                            .into_any_element(),
                    )
                }))
                .into_any_element(),
        )
    }
}

fn file_icon(path: &std::path::Path, cx: &App) -> Icon {
    match FileIcons::get_icon(path, cx) {
        Some(theme_icon) => Icon::from_path(theme_icon),
        None => Icon::new(IconName::AgentDocument),
    }
    .size(IconSize::Small)
    .color(Color::Muted)
}
