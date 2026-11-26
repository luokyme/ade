//! A ring in the composer footer showing how full the model's context window is.

use super::ThreadView;
use crate::style::{text_faint, ui};
use gpui::{AnyElement, App, Hsla, PathBuilder, SharedString, canvas, point};
use i18n::t;
use t3_client::state::ContextWindow;
use theme::ActiveTheme as _;
use ui::{Tooltip, prelude::*};

/// T3 Code turns its meter amber and red at the same points.
const CONTEXT_WARNING: f64 = 0.8;
const CONTEXT_DANGER: f64 = 0.9;
const RING_SIZE: f32 = 16.;
const RING_RADIUS: f32 = 6.;
const RING_STROKE: f32 = 1.8;

impl ThreadView {
    pub(super) fn render_context_window(&self, cx: &App) -> Option<AnyElement> {
        let context = self.state.as_ref()?.context_window()?;
        let fraction = context.used_fraction();
        let status = cx.theme().status();
        let color = match fraction {
            Some(fraction) if fraction >= CONTEXT_DANGER => status.error,
            Some(fraction) if fraction >= CONTEXT_WARNING => status.warning,
            Some(_) => cx.theme().colors().text_muted,
            None => text_faint(cx),
        };
        let label: SharedString = fraction
            .map(|fraction| format!("{:.0}%", fraction * 100.0))
            .unwrap_or_else(|| "–".into())
            .into();
        let details: SharedString = context_details(&context).into();
        Some(
            h_flex()
                .id("t3-context-window")
                .h(px(22.))
                .px(px(4.))
                .gap(px(4.))
                .flex_none()
                .text_size(ui(11.))
                .text_color(color)
                .child(ring(fraction.unwrap_or(0.0) as f32, color, cx))
                .child(label)
                .tooltip(move |_, cx| {
                    Tooltip::with_meta(t!("Context window"), None, details.clone(), cx)
                })
                .into_any_element(),
        )
    }
}

fn ring(fraction: f32, color: Hsla, cx: &App) -> impl IntoElement {
    let track = text_faint(cx).opacity(0.25);
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let center = bounds.center();
            let mut arc = |fraction: f32, color| {
                if fraction <= 0.0 {
                    return;
                }
                let steps = (64.0 * fraction).ceil().max(2.0) as usize;
                let mut path = PathBuilder::stroke(px(RING_STROKE));
                for step in 0..=steps {
                    let angle = -std::f32::consts::FRAC_PI_2
                        + std::f32::consts::TAU * fraction * step as f32 / steps as f32;
                    let position = point(
                        center.x + px(RING_RADIUS * angle.cos()),
                        center.y + px(RING_RADIUS * angle.sin()),
                    );
                    if step == 0 {
                        path.move_to(position);
                    } else {
                        path.line_to(position);
                    }
                }
                match path.build() {
                    Ok(path) => window.paint_path(path, color),
                    Err(error) => log::debug!("could not draw the context ring: {error}"),
                }
            };
            arc(1.0, track);
            arc(fraction.clamp(0.0, 1.0), color);
        },
    )
    .size(px(RING_SIZE))
    .flex_none()
}

fn with_separators(count: u64) -> String {
    let digits = count.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}

fn context_details(context: &ContextWindow) -> String {
    let mut lines = match context.max_tokens.filter(|max_tokens| *max_tokens > 0) {
        Some(max_tokens) => vec![
            t!(
                "{used} / {total} tokens",
                used = with_separators(context.used_tokens),
                total = with_separators(max_tokens)
            )
            .to_string(),
            t!(
                "{count} tokens remaining",
                count = with_separators(max_tokens.saturating_sub(context.used_tokens))
            )
            .to_string(),
        ],
        None => vec![
            t!(
                "{count} tokens used",
                count = with_separators(context.used_tokens)
            )
            .to_string(),
            t!("Context limit not reported").to_string(),
        ],
    };
    if let Some(threshold) = context.auto_compact_threshold {
        lines.push(
            t!(
                "Compacts automatically at about {count} tokens",
                count = with_separators(threshold)
            )
            .to_string(),
        );
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn details_show_what_the_agent_reported() {
        let mut context = ContextWindow {
            used_tokens: 21894,
            max_tokens: Some(828400),
            auto_compact_threshold: None,
        };
        assert_eq!(
            context_details(&context),
            "21,894 / 828,400 tokens\n806,506 tokens remaining"
        );
        context.max_tokens = None;
        context.auto_compact_threshold = Some(160000);
        assert_eq!(
            context_details(&context),
            "21,894 tokens used\nContext limit not reported\n\
             Compacts automatically at about 160,000 tokens"
        );
    }
}
