//! The look of Ade's own chat, so T3 Code threads read the same. Kept apart from `agent_chat`,
//! which this crate is meant to outlive.

use gpui::{
    Animation, AnimationExt as _, AnyElement, App, Bounds, ContentMask, ElementId, Font, Hsla,
    IntoElement, ParentElement, Rems, SharedString, Styled, Svg, TextAlign, TextRun, canvas, div,
    hsla, point, px, rems, rgb, size, svg,
};
use i18n::{n, t};
use std::time::Duration;
use theme::ActiveTheme as _;
use theme_settings::ThemeSettings;
use settings::Settings as _;
use ui::{IconName, prelude::*};
use util::ResultExt as _;

pub const SPACE_SM: f32 = 8.;
pub const SPACE_MD: f32 = 12.;
pub const SPACE_LG: f32 = 16.;
pub const TOOL_TEXT_SIZE: f32 = 12.;
pub const TOOL_LINE_HEIGHT: f32 = 18.;
const WORKING_ANIMATION_FPS: f32 = 12.;
const INK_HAIRLINE_SCALE: f32 = 1.35;
const SHIMMER_HALF_WIDTH: f32 = 0.36;
const SHIMMER_STRIP_WIDTH: f32 = 2.;
const SHIMMER_PERIOD: Duration = Duration::from_millis(3400);

pub fn ui(pixels_at_default: f32) -> Rems {
    rems(pixels_at_default / 16.0)
}

fn is_dark(cx: &App) -> bool {
    !cx.theme().appearance.is_light()
}

pub fn ink(alpha: f32, cx: &App) -> Hsla {
    if is_dark(cx) {
        hsla(0.0, 0.0, 1.0, alpha)
    } else {
        hsla(0.0, 0.0, 0.0, alpha)
    }
}

/// Hairline ink, scaled up on light themes so 1px edges stay legible.
pub fn hairline(alpha: f32, cx: &App) -> Hsla {
    if is_dark(cx) {
        hsla(0.0, 0.0, 1.0, alpha)
    } else {
        hsla(0.0, 0.0, 0.0, (alpha * INK_HAIRLINE_SCALE).min(0.5))
    }
}

pub fn wash(alpha: f32, cx: &App) -> Hsla {
    if is_dark(cx) {
        hsla(0.0, 0.0, 0.92, alpha)
    } else {
        hsla(0.0, 0.0, 0.10, alpha)
    }
}

pub fn page(cx: &App) -> Hsla {
    cx.theme().colors().background.opacity(1.0)
}

pub fn mix(from: Hsla, to: Hsla, amount: f32) -> Hsla {
    let from = from.to_rgb();
    let to = to.to_rgb();
    let amount = amount.clamp(0.0, 1.0);
    gpui::Rgba {
        r: lerp(from.r, to.r, amount),
        g: lerp(from.g, to.g, amount),
        b: lerp(from.b, to.b, amount),
        a: lerp(from.a, to.a, amount),
    }
    .into()
}

fn lerp(from: f32, to: f32, amount: f32) -> f32 {
    from + (to - from) * amount
}

pub fn text_faint(cx: &App) -> Hsla {
    cx.theme().colors().text_placeholder
}

pub fn accent(cx: &App) -> Hsla {
    cx.theme().colors().text_accent
}

/// Opaque so trays tucked behind the composer stay hidden.
pub fn composer_surface(cx: &App) -> Hsla {
    let input = cx.theme().colors().element_background;
    mix(page(cx), input.opacity(1.0), input.a)
}

pub fn code_font(cx: &App) -> Font {
    ThemeSettings::get_global(cx).buffer_font.clone()
}

pub fn icon(name: IconName, size: f32) -> Svg {
    svg().path(name.path()).size(px(size)).flex_none()
}

pub fn working_animation(period: Duration) -> Animation {
    Animation::new(period)
        .repeat()
        .with_max_fps(WORKING_ANIMATION_FPS)
}

/// A small square button that only shows an icon, like the copy button under a message.
pub fn icon_button(
    id: impl Into<ElementId>,
    name: IconName,
    color: Hsla,
    cx: &App,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id.into())
        .size(px(24.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.))
        .cursor_pointer()
        .hover(|style| style.bg(ink(0.08, cx)))
        .child(icon(name, 14.).text_color(color))
}

const GSPIN_TOP_TINT: u32 = 0xB6D3EF;
const GSPIN_MIDDLE_TINT: u32 = 0xEDB185;
const GSPIN_BOTTOM_TINT: u32 = 0xF888A0;
const GSPIN_DIM: f32 = 0.3;
const GRADIENT_SPIN_MS: u64 = 900;
const LOGO_PATH: &str = "icons/ade_mark.svg";
const LOGO_ASPECT: f32 = 628. / 644.;
const LOGO_BANDS: usize = 6;

fn gspin_opacity(phase: f32, dim: f32) -> f32 {
    let phase = phase.rem_euclid(1.0);
    if phase < 0.45 {
        lerp(1.0, dim, phase / 0.45)
    } else if phase < 0.92 {
        dim
    } else {
        lerp(dim, 1.0, (phase - 0.92) / 0.08)
    }
}

fn logo_band_tint(band: usize) -> Hsla {
    let depth = band as f32 / (LOGO_BANDS - 1) as f32;
    if depth < 0.5 {
        mix(rgb(GSPIN_TOP_TINT).into(), rgb(GSPIN_MIDDLE_TINT).into(), depth * 2.)
    } else {
        mix(
            rgb(GSPIN_MIDDLE_TINT).into(),
            rgb(GSPIN_BOTTOM_TINT).into(),
            depth * 2. - 1.,
        )
    }
}

/// Ade's mark with a wave rising through it, shown while the agent works.
pub fn gradient_spinner(id: SharedString, height: f32) -> impl IntoElement {
    let width = height * LOGO_ASPECT;
    let band_height = height / LOGO_BANDS as f32;
    div()
        .flex_none()
        .relative()
        .w(px(width))
        .h(px(height))
        .children((0..LOGO_BANDS).map(|band| {
            let top = band as f32 * band_height;
            let phase = (LOGO_BANDS - 1 - band) as f32 / LOGO_BANDS as f32;
            div()
                .absolute()
                .left_0()
                .top(px(top))
                .w(px(width))
                .h(px(band_height))
                .overflow_hidden()
                .child(
                    svg()
                        .path(LOGO_PATH)
                        .absolute()
                        .left_0()
                        .top(px(-top))
                        .w(px(width))
                        .h(px(height))
                        .text_color(logo_band_tint(band)),
                )
                .with_animation(
                    ElementId::Name(format!("{id}-{band}").into()),
                    working_animation(Duration::from_millis(GRADIENT_SPIN_MS)),
                    move |band, delta| band.opacity(gspin_opacity(delta - phase, GSPIN_DIM)),
                )
        }))
}

pub fn spinning_icon(id: impl Into<ElementId>, name: IconName, size: f32, color: Hsla) -> AnyElement {
    icon(name, size)
        .text_color(color)
        .with_animation(
            id.into(),
            working_animation(Duration::from_secs(1)),
            |this, delta| {
                this.with_transformation(gpui::Transformation::rotate(gpui::percentage(delta)))
            },
        )
        .into_any_element()
}

const FLAVOUR_WORDS: [&str; 21] = [
    n!("Reckoning"),
    n!("Thinking"),
    n!("Pondering"),
    n!("Scheming"),
    n!("Brewing"),
    n!("Weaving"),
    n!("Tinkering"),
    n!("Musing"),
    n!("Composing"),
    n!("Sifting"),
    n!("Untangling"),
    n!("Distilling"),
    n!("Sketching"),
    n!("Plotting"),
    n!("Riffing"),
    n!("Combobulating"),
    n!("Percolating"),
    n!("Marinating"),
    n!("Noodling"),
    n!("Puzzling"),
    n!("Conjuring"),
];
const FLAVOUR_ROTATE_SECS: u64 = 7;

pub fn flavour_word(seed: u64, elapsed_secs: u64) -> &'static str {
    let step = elapsed_secs / FLAVOUR_ROTATE_SECS;
    FLAVOUR_WORDS[(seed.wrapping_add(step) % FLAVOUR_WORDS.len() as u64) as usize]
}

pub fn flavour_seed(text: &str) -> u64 {
    text.bytes().fold(0xcbf29ce484222325u64, |hash, byte| {
        (hash ^ byte as u64).wrapping_mul(0x100000001b3)
    })
}

pub fn format_elapsed(secs: u64) -> String {
    if secs < 60 {
        t!("{seconds}s", seconds = secs).to_string()
    } else if secs < 3_600 {
        t!(
            "{minutes}m {seconds}s",
            minutes = secs / 60,
            seconds = secs % 60
        )
        .to_string()
    } else if secs < 86_400 {
        t!(
            "{hours}h {minutes}m",
            hours = secs / 3_600,
            minutes = (secs % 3_600) / 60
        )
        .to_string()
    } else {
        t!(
            "{days}d {hours}h",
            days = secs / 86_400,
            hours = (secs % 86_400) / 3_600
        )
        .to_string()
    }
}

fn shimmer_amount(x: f32, phase: f32) -> f32 {
    let primary_center = -2.5 + phase.clamp(0., 1.) * 6.;
    (-2..=2)
        .map(|copy| primary_center + copy as f32 * 3.)
        .map(|center| (1. - (x - center).abs() / SHIMMER_HALF_WIDTH).clamp(0., 1.))
        .fold(0., f32::max)
}

fn shimmer_overlay(text: SharedString, phase: f32, base: Hsla, peak: Hsla) -> impl IntoElement {
    canvas(
        move |bounds, window, _| {
            let style = window.text_style();
            let font = style.font();
            let font_size = style.font_size.to_pixels(window.rem_size());
            let line_height = style.line_height_in_pixels(window.rem_size());
            let run = |color: Hsla| TextRun {
                len: text.len(),
                font: font.clone(),
                color,
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let text_system = window.text_system();
            let probe =
                text_system.shape_line(text.clone(), font_size, &[run(peak)], None);
            let text_width = f32::from(probe.width()).min(f32::from(bounds.size.width));
            let strip_count = (text_width / SHIMMER_STRIP_WIDTH).ceil() as usize;
            let base_line = text_system.shape_line(text.clone(), font_size, &[run(base)], None);
            let mut strips = Vec::with_capacity(strip_count);
            for strip in 0..strip_count {
                let left = strip as f32 * SHIMMER_STRIP_WIDTH;
                let right = ((strip + 1) as f32 * SHIMMER_STRIP_WIDTH).min(text_width);
                let x = (left + right) * 0.5 / text_width.max(1.);
                let amount = shimmer_amount(x, phase);
                if amount <= 0.001 {
                    continue;
                }
                let line = text_system.shape_line(
                    text.clone(),
                    font_size,
                    &[run(mix(base, peak, amount))],
                    None,
                );
                strips.push((left, right, line));
            }
            (base_line, strips, line_height)
        },
        move |bounds, (base_line, strips, line_height), window, cx| {
            base_line
                .paint(bounds.origin, line_height, TextAlign::Left, None, window, cx)
                .log_err();
            for (left, right, line) in strips {
                let mask = ContentMask {
                    bounds: Bounds {
                        origin: point(bounds.origin.x + px(left), bounds.origin.y),
                        size: size(px(right - left), bounds.size.height),
                    },
                };
                window.with_content_mask(Some(mask), |window| {
                    line.paint(
                        bounds.origin,
                        line_height,
                        TextAlign::Left,
                        None,
                        window,
                        cx,
                    )
                    .log_err();
                });
            }
        },
    )
    .absolute()
    .inset_0()
}

/// A title with a highlight sweeping across it, for the group the agent is working in.
pub fn shimmer_title(text: SharedString, id: ElementId, base: Hsla, peak: Hsla) -> AnyElement {
    div()
        .relative()
        .h_full()
        .min_w_0()
        .flex_1()
        .overflow_hidden()
        .child(div().text_color(gpui::transparent_black()).child(text.clone()))
        .with_animation(id, working_animation(SHIMMER_PERIOD), move |title, phase| {
            title.child(shimmer_overlay(text.clone(), phase, base, peak))
        })
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elapsed_uses_two_units() {
        assert_eq!(format_elapsed(12), "12s");
        assert_eq!(format_elapsed(184), "3m 4s");
        assert_eq!(format_elapsed(3_720), "1h 2m");
    }

    #[test]
    fn flavour_words_rotate_every_seven_seconds() {
        let seed = flavour_seed("thread-1");
        assert_eq!(flavour_word(seed, 0), flavour_word(seed, 6));
        assert_ne!(flavour_word(seed, 0), flavour_word(seed, 7));
    }
}
