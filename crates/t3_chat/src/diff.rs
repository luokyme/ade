//! Diffs of the files an agent changed, from the unified diff T3 Code records on a file change,
//! or from the replaced and replacing text when that is all it has.

use crate::style::{code_font, text_faint};
use gpui::{
    AnyElement, App, Hsla, IntoElement, ParentElement, SharedString, Styled, div, hsla, px,
};
use i18n::t;
use t3_client::protocol::TurnItem;
use theme::ActiveTheme as _;
use ui::prelude::*;

const MAX_LINES: usize = 400;
const CONTEXT_LINES: u32 = 3;
const LINE_HEIGHT: f32 = 20.;
const TEXT_SIZE: f32 = 12.;
const HUNK_HEADER_HEIGHT: f32 = 24.;
const ACCENT_BAR_WIDTH: f32 = 3.;
const MARKER_WIDTH: f32 = 22.;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LineKind {
    Context,
    Added,
    Removed,
    Hunk,
}

#[derive(Clone, Debug)]
struct Line {
    kind: LineKind,
    number: Option<u32>,
    text: SharedString,
}

pub struct FileDiff {
    lines: Vec<Line>,
    pub additions: usize,
    pub deletions: usize,
    hidden: usize,
}

impl FileDiff {
    pub fn of(item: &TurnItem) -> Option<Self> {
        if let Some(diff) = item
            .str_field("diffStr")
            .filter(|diff| !diff.trim().is_empty())
        {
            return Some(Self::parse_unified(diff));
        }
        let new = item.str_field("newStr")?;
        let old = item.str_field("oldStr").unwrap_or("");
        Some(Self::from_texts(old, new))
    }

    fn parse_unified(diff: &str) -> Self {
        let mut lines = Vec::new();
        let (mut additions, mut deletions) = (0, 0);
        let mut old_number = 0u32;
        let mut new_number = 0u32;
        let mut in_hunk = false;
        for raw in diff.lines() {
            if let Some(header) = raw.strip_prefix("@@") {
                let (old_start, new_start) = parse_hunk_header(header);
                old_number = old_start;
                new_number = new_start;
                in_hunk = true;
                lines.push(Line {
                    kind: LineKind::Hunk,
                    number: None,
                    text: format!("@@{header}").into(),
                });
                continue;
            }
            if !in_hunk {
                continue;
            }
            let (kind, text) = match raw.chars().next() {
                Some('+') => (LineKind::Added, &raw[1..]),
                Some('-') => (LineKind::Removed, &raw[1..]),
                Some(' ') => (LineKind::Context, &raw[1..]),
                Some('\\') => continue,
                None => (LineKind::Context, ""),
                Some(_) => {
                    in_hunk = false;
                    continue;
                }
            };
            let number = match kind {
                LineKind::Added => {
                    additions += 1;
                    new_number += 1;
                    Some(new_number - 1)
                }
                LineKind::Removed => {
                    deletions += 1;
                    old_number += 1;
                    Some(old_number - 1)
                }
                _ => {
                    old_number += 1;
                    new_number += 1;
                    Some(new_number - 1)
                }
            };
            lines.push(Line {
                kind,
                number,
                text: text.replace('\t', "    ").into(),
            });
        }
        Self::capped(lines, additions, deletions)
    }

    fn from_texts(old: &str, new: &str) -> Self {
        let old_lines = old.lines().collect::<Vec<_>>();
        let new_lines = new.lines().collect::<Vec<_>>();
        let mut lines = Vec::new();
        let (mut additions, mut deletions) = (0, 0);
        let hunks = language::line_diff(old, new);
        let mut shown_until = 0u32;
        for (index, (before, after)) in hunks.iter().enumerate() {
            let leading = before.start.saturating_sub(CONTEXT_LINES).max(shown_until);
            if leading > shown_until || index == 0 {
                lines.push(Line {
                    kind: LineKind::Hunk,
                    number: None,
                    text: SharedString::default(),
                });
            }
            let delta = after.start as i64 - before.start as i64;
            for old_index in leading..before.start {
                lines.push(Line {
                    kind: LineKind::Context,
                    number: Some((old_index as i64 + delta + 1) as u32),
                    text: old_lines
                        .get(old_index as usize)
                        .copied()
                        .unwrap_or_default()
                        .replace('\t', "    ")
                        .into(),
                });
            }
            for old_index in before.clone() {
                deletions += 1;
                lines.push(Line {
                    kind: LineKind::Removed,
                    number: Some(old_index + 1),
                    text: old_lines
                        .get(old_index as usize)
                        .copied()
                        .unwrap_or_default()
                        .replace('\t', "    ")
                        .into(),
                });
            }
            for new_index in after.clone() {
                additions += 1;
                lines.push(Line {
                    kind: LineKind::Added,
                    number: Some(new_index + 1),
                    text: new_lines
                        .get(new_index as usize)
                        .copied()
                        .unwrap_or_default()
                        .replace('\t', "    ")
                        .into(),
                });
            }
            let next_start = hunks
                .get(index + 1)
                .map_or(old_lines.len() as u32, |(next, _)| next.start);
            let trailing_end = (before.end + CONTEXT_LINES)
                .min(next_start)
                .min(old_lines.len() as u32);
            let trailing_delta = after.end as i64 - before.end as i64;
            for old_index in before.end..trailing_end {
                lines.push(Line {
                    kind: LineKind::Context,
                    number: Some((old_index as i64 + trailing_delta + 1) as u32),
                    text: old_lines
                        .get(old_index as usize)
                        .copied()
                        .unwrap_or_default()
                        .replace('\t', "    ")
                        .into(),
                });
            }
            shown_until = trailing_end.max(before.end);
        }
        Self::capped(lines, additions, deletions)
    }

    fn capped(mut lines: Vec<Line>, additions: usize, deletions: usize) -> Self {
        let hidden = lines.len().saturating_sub(MAX_LINES);
        lines.truncate(MAX_LINES);
        Self {
            lines,
            additions,
            deletions,
            hidden,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.lines.iter().all(|line| line.kind == LineKind::Hunk)
    }

    pub fn render(&self, cx: &App) -> AnyElement {
        let font = code_font(cx);
        let colors = cx.theme().colors();
        let status = cx.theme().status();
        let faint = text_faint(cx);
        let hunk_background = if cx.theme().appearance.is_light() {
            hsla(0.6, 0.35, 0.35, 0.07)
        } else {
            hsla(0.6, 0.35, 0.6, 0.05)
        };
        let widest = self
            .lines
            .iter()
            .filter_map(|line| line.number)
            .max()
            .unwrap_or(1)
            .max(1);
        let gutter = ((widest.ilog10() + 1) as f32 * 6.6 + 12.).max(28.);
        let rows = self.lines.iter().map(|line| {
            if line.kind == LineKind::Hunk {
                return div()
                    .h(px(HUNK_HEADER_HEIGHT))
                    .w_full()
                    .flex_none()
                    .flex()
                    .items_center()
                    .px(px(12.))
                    .bg(hunk_background)
                    .font(font.clone())
                    .text_size(px(11.))
                    .text_color(faint)
                    .child(line.text.clone())
                    .into_any_element();
            }
            let (marker, tint): (&str, Option<Hsla>) = match line.kind {
                LineKind::Added => ("+", Some(status.created)),
                LineKind::Removed => ("−", Some(status.deleted)),
                _ => ("", None),
            };
            h_flex()
                .h(px(LINE_HEIGHT))
                .w_full()
                .flex_none()
                .items_start()
                .when_some(tint, |this, tint| this.bg(tint.opacity(0.06)))
                .child(
                    div()
                        .w(px(ACCENT_BAR_WIDTH))
                        .h_full()
                        .flex_none()
                        .when_some(tint, |this, tint| this.bg(tint.opacity(0.55))),
                )
                .child(
                    div()
                        .w(px(gutter))
                        .flex_none()
                        .flex()
                        .justify_end()
                        .pr(px(6.))
                        .font(font.clone())
                        .text_size(px(11.))
                        .line_height(px(LINE_HEIGHT))
                        .text_color(tint.map_or(faint.opacity(0.8), |tint| tint.opacity(0.9)))
                        .child(
                            line.number
                                .map(|number| number.to_string())
                                .unwrap_or_default(),
                        ),
                )
                .child(
                    div()
                        .w(px(MARKER_WIDTH))
                        .flex_none()
                        .flex()
                        .justify_center()
                        .font(font.clone())
                        .text_size(px(TEXT_SIZE))
                        .line_height(px(LINE_HEIGHT))
                        .text_color(tint.unwrap_or(faint))
                        .child(marker),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .font(font.clone())
                        .text_size(px(TEXT_SIZE))
                        .line_height(px(LINE_HEIGHT))
                        .whitespace_nowrap()
                        .text_color(colors.text.opacity(0.92))
                        .child(line.text.clone()),
                )
                .into_any_element()
        });
        v_flex()
            .w_full()
            .min_w_0()
            .overflow_hidden()
            .rounded(px(8.))
            .border_1()
            .border_color(colors.border_variant)
            .bg(colors.editor_background)
            .children(rows)
            .when(self.hidden > 0, |this| {
                this.child(
                    div()
                        .h(px(HUNK_HEADER_HEIGHT))
                        .px(px(12.))
                        .flex()
                        .items_center()
                        .text_size(px(11.))
                        .text_color(faint)
                        .child(t!("… {count} more lines", count = self.hidden)),
                )
            })
            .into_any_element()
    }
}

fn parse_hunk_header(header: &str) -> (u32, u32) {
    let start = |marker: char| {
        header
            .split_whitespace()
            .find_map(|part| part.strip_prefix(marker))
            .and_then(|range| range.split(',').next())
            .and_then(|start| start.parse::<u32>().ok())
            .unwrap_or(1)
    };
    (start('-'), start('+'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(diff: &FileDiff) -> String {
        diff.lines
            .iter()
            .map(|line| match line.kind {
                LineKind::Context => ' ',
                LineKind::Added => '+',
                LineKind::Removed => '-',
                LineKind::Hunk => '@',
            })
            .collect()
    }

    #[test]
    fn unified_diffs_keep_line_numbers() {
        let diff = FileDiff::parse_unified(
            "--- a/x\n+++ b/x\n@@ -3,3 +3,3 @@ fn main\n one\n-two\n+deux\n three\n",
        );
        assert_eq!(kinds(&diff), "@ -+ ");
        assert_eq!((diff.additions, diff.deletions), (1, 1));
        assert_eq!(diff.lines[1].number, Some(3));
        assert_eq!(diff.lines[2].number, Some(4));
        assert_eq!(diff.lines[3].number, Some(4));
        assert_eq!(diff.lines[4].number, Some(5));
    }

    #[test]
    fn replaced_text_becomes_a_diff() {
        let diff = FileDiff::from_texts("a\nb\nc\n", "a\nB\nc\n");
        assert_eq!(kinds(&diff), "@ -+ ");
        assert_eq!((diff.additions, diff.deletions), (1, 1));
    }
}
