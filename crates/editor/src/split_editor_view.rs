use std::{cmp, ops::Range};

use collections::{HashMap, HashSet};
use gpui::{
    AbsoluteLength, AnyElement, App, AvailableSpace, Bounds, Context, DragMoveEvent, Element,
    Entity, GlobalElementId, Hsla, InspectorElementId, IntoElement, LayoutId, Length,
    ParentElement, PathBuilder, Pixels, StatefulInteractiveElement, Styled, TextStyleRefinement,
    Window, canvas, div, fill, linear_color_stop, linear_gradient, point, px, size,
};
use multi_buffer::{Anchor, ExcerptBoundaryInfo, MultiBufferDiffHunk};
use smallvec::smallvec;
use text::BufferId;
use theme::ActiveTheme;
use ui::{h_flex, prelude::*, v_flex};

use gpui::ContentMask;

use crate::{
    DisplayRow, Editor, EditorSnapshot, EditorStyle, FILE_HEADER_HEIGHT,
    MULTI_BUFFER_EXCERPT_HEADER_HEIGHT, RowExt, StickyHeaderExcerpt,
    display_map::Block,
    element::{EditorElement, SplitSide, header_jump_data, render_buffer_header},
    scroll::ScrollOffset,
    split::SplittableEditor,
};

const RESIZE_HANDLE_WIDTH: f32 = 8.0;
const CHANGE_CONNECTION_WIDTH: Pixels = px(56.);

#[derive(Debug, Clone)]
struct DraggedSplitHandle;

pub struct SplitEditorState {
    left_ratio: f32,
    visible_left_ratio: f32,
    cached_width: Pixels,
}

impl SplitEditorState {
    pub fn new(_cx: &mut App) -> Self {
        Self {
            left_ratio: 0.5,
            visible_left_ratio: 0.5,
            cached_width: px(0.),
        }
    }

    #[allow(clippy::misnamed_getters)]
    pub fn left_ratio(&self) -> f32 {
        self.visible_left_ratio
    }

    pub fn right_ratio(&self) -> f32 {
        1.0 - self.visible_left_ratio
    }

    fn on_drag_move(
        &mut self,
        drag_event: &DragMoveEvent<DraggedSplitHandle>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let drag_position = drag_event.event.position;
        let bounds = drag_event.bounds;
        let bounds_width = bounds.right() - bounds.left();

        if bounds_width > px(0.) {
            self.cached_width = bounds_width;
        }

        let min_ratio = 0.1;
        let max_ratio = 0.9;

        let new_ratio = (drag_position.x - bounds.left()) / bounds_width;
        self.visible_left_ratio = new_ratio.clamp(min_ratio, max_ratio);
        cx.notify();
    }

    fn commit_ratio(&mut self) {
        self.left_ratio = self.visible_left_ratio;
    }

    fn on_double_click(&mut self) {
        self.left_ratio = 0.5;
        self.visible_left_ratio = 0.5;
    }
}

#[derive(IntoElement)]
pub struct SplitEditorView {
    splittable_editor: Entity<SplittableEditor>,
    style: EditorStyle,
    split_state: Entity<SplitEditorState>,
}

impl SplitEditorView {
    pub fn new(
        splittable_editor: Entity<SplittableEditor>,
        style: EditorStyle,
        split_state: Entity<SplitEditorState>,
    ) -> Self {
        Self {
            splittable_editor,
            style,
            split_state,
        }
    }
}

fn render_resize_handle(
    state: &Entity<SplitEditorState>,
    width: Pixels,
    separator_color: Hsla,
    _window: &mut Window,
    _cx: &mut App,
) -> AnyElement {
    let state_for_click = state.clone();

    div()
        .id("split-resize-container")
        .relative()
        .h_full()
        .flex_shrink_0()
        .w(width)
        .bg(separator_color)
        .child(
            div()
                .id("split-resize-handle")
                .absolute()
                .left((width - px(RESIZE_HANDLE_WIDTH)) / 2.)
                .w(px(RESIZE_HANDLE_WIDTH))
                .h_full()
                .cursor_col_resize()
                .block_mouse_except_scroll()
                .on_click(move |event, _, cx| {
                    if event.click_count() >= 2 {
                        state_for_click.update(cx, |state, cx| {
                            state.on_double_click();
                            cx.notify();
                        });
                    }
                    cx.stop_propagation();
                })
                .on_drag(DraggedSplitHandle, |_, _, _, cx| cx.new(|_| gpui::Empty)),
        )
        .into_any_element()
}

impl RenderOnce for SplitEditorView {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let splittable_editor = self.splittable_editor.read(cx);

        assert!(
            splittable_editor.lhs_editor().is_some(),
            "`SplitEditorView` requires `SplittableEditor` to be in split mode"
        );

        let lhs_editor = splittable_editor.lhs_editor().unwrap().clone();
        let rhs_editor = splittable_editor.rhs_editor().clone();
        let show_connections = splittable_editor.shows_change_connections();

        let mut lhs = EditorElement::new(&lhs_editor, self.style.clone());
        let mut rhs = EditorElement::new(&rhs_editor, self.style.clone());

        lhs.set_split_side(SplitSide::Left);
        rhs.set_split_side(SplitSide::Right);
        if show_connections {
            lhs.use_change_connections();
            rhs.use_change_connections();
        }

        let left_ratio = self.split_state.read(cx).left_ratio();
        let right_ratio = self.split_state.read(cx).right_ratio();

        let colors = cx.theme().colors();
        let resize_handle = render_resize_handle(
            &self.split_state,
            if show_connections {
                CHANGE_CONNECTION_WIDTH
            } else {
                px(1.)
            },
            if show_connections {
                colors.editor_background
            } else {
                colors.border_variant
            },
            window,
            cx,
        );
        let connections = show_connections.then(|| {
            let rem_size = lhs.rem_size(cx).unwrap_or(window.rem_size());
            render_change_connections(
                lhs_editor.clone(),
                rhs_editor.clone(),
                self.style.text.line_height_in_pixels(rem_size),
                self.style.status.clone(),
            )
        });

        let state_for_drag = self.split_state.downgrade();
        let state_for_drop = self.split_state.downgrade();

        let buffer_headers = SplitBufferHeadersElement::new(
            lhs_editor.clone(),
            rhs_editor.clone(),
            self.style.clone(),
            show_connections.then_some(SplitSide::Right),
        );
        let left_buffer_headers = show_connections.then(|| {
            SplitBufferHeadersElement::new(
                lhs_editor.clone(),
                rhs_editor.clone(),
                self.style.clone(),
                Some(SplitSide::Left),
            )
        });

        let lhs_editor_for_order = lhs_editor;
        let rhs_editor_for_order = rhs_editor;

        div()
            .id("split-editor-view-container")
            .size_full()
            .relative()
            .child(
                h_flex()
                    .with_dynamic_prepaint_order(move |_window, cx| {
                        let lhs_needs = lhs_editor_for_order.read(cx).has_autoscroll_request();
                        let rhs_needs = rhs_editor_for_order.read(cx).has_autoscroll_request();
                        match (lhs_needs, rhs_needs) {
                            (false, true) => smallvec![2, 1, 0],
                            _ => smallvec![0, 1, 2],
                        }
                    })
                    .id("split-editor-view")
                    .size_full()
                    .on_drag_move::<DraggedSplitHandle>(move |event, window, cx| {
                        state_for_drag
                            .update(cx, |state, cx| {
                                state.on_drag_move(event, window, cx);
                            })
                            .ok();
                    })
                    .on_drop::<DraggedSplitHandle>(move |_, _, cx| {
                        state_for_drop
                            .update(cx, |state, _| {
                                state.commit_ratio();
                            })
                            .ok();
                    })
                    .child(
                        div()
                            .id("split-editor-left")
                            .flex_shrink_1()
                            .min_w_0()
                            .h_full()
                            .flex_basis(DefiniteLength::Fraction(left_ratio))
                            .overflow_hidden()
                            .child(lhs),
                    )
                    .child(resize_handle)
                    .child(
                        div()
                            .id("split-editor-right")
                            .flex_shrink_1()
                            .min_w_0()
                            .h_full()
                            .flex_basis(DefiniteLength::Fraction(right_ratio))
                            .overflow_hidden()
                            .child(rhs),
                    ),
            )
            .children(connections)
            .children(left_buffer_headers)
            .child(buffer_headers)
    }
}

fn render_change_connections(
    left: Entity<Editor>,
    right: Entity<Editor>,
    line_height: Pixels,
    colors: theme::StatusColors,
) -> impl IntoElement {
    canvas(
        move |bounds, window, cx| {
            let left_bounds = left.read(cx).last_bounds().copied()?;
            let right_bounds = right.read(cx).last_bounds().copied()?;
            let left_snapshot = left.update(cx, |editor, cx| editor.snapshot(window, cx));
            let right_snapshot = right.update(cx, |editor, cx| editor.snapshot(window, cx));
            let start_x = left_bounds.right();
            let end_x = right_bounds.left();
            if end_x <= start_x {
                return None;
            }
            let gap =
                Bounds::from_corners(point(start_x, bounds.top()), point(end_x, bounds.bottom()));

            let left_scroll =
                window.pixel_snap_f64(left_snapshot.scroll_position().y * f64::from(line_height));
            let right_scroll =
                window.pixel_snap_f64(right_snapshot.scroll_position().y * f64::from(line_height));
            let position = |scroll, row: DisplayRow| {
                bounds.top() + Pixels::from(row.as_f64() * f64::from(line_height) - scroll)
            };
            let span = |snapshot: &EditorSnapshot, hunk: &MultiBufferDiffHunk, scroll| {
                let rows = snapshot.diff_hunk_display_rows(hunk);
                position(scroll, rows.start)..position(scroll, rows.end)
            };
            let mut ribbons = Vec::new();
            let mut insertion_markers = Vec::new();
            let mut controls = Vec::new();
            let fold_color = cx.theme().colors().border_variant;
            for (left_row, right_row, block) in
                left_snapshot.paired_diff_boundaries(&right_snapshot)
            {
                if !matches!(block, Block::ExcerptBoundary { .. }) {
                    continue;
                }
                let left_top = position(left_scroll, left_row) + line_height / 2.;
                let right_top = position(right_scroll, right_row) + line_height / 2.;
                let from = left_top..left_top + px(1.);
                let to = right_top..right_top + px(1.);
                if from.end.max(to.end) >= gap.top()
                    && from.start.min(to.start) <= gap.bottom()
                    && let Some(path) = change_connection_path(gap, from, to)
                {
                    ribbons.push((path, fold_color.into()));
                }
            }
            for (left_hunk, hunk) in left_snapshot.paired_diff_hunks(&right_snapshot) {
                let to = span(&right_snapshot, &hunk, right_scroll);
                let matching_left = left_hunk
                    .as_ref()
                    .map(|hunk| (span(&left_snapshot, hunk, left_scroll), hunk));
                if let Some((from, _)) = matching_left.as_ref()
                    && (!from.is_empty() || !to.is_empty())
                    && from.end.max(to.end) >= gap.top()
                    && from.start.min(to.start) <= gap.bottom()
                {
                    let mut left_span = from.clone();
                    let mut right_span = to.clone();
                    for (endpoint, pane, color) in [
                        (&mut left_span, left_bounds, colors.created_border),
                        (&mut right_span, right_bounds, colors.deleted_border),
                    ] {
                        if endpoint.is_empty() {
                            // Give the ribbon and the marker the same pixel-aligned edge.
                            let top = window.pixel_snap(endpoint.start);
                            *endpoint = top..top + px(1.);
                            let marker =
                                Bounds::new(point(pane.left(), top), size(pane.size.width, px(1.)))
                                    .intersect(&pane);
                            if marker.size.height > px(0.) {
                                insertion_markers.push(fill(marker, color));
                            }
                        }
                    }
                    let color = if hunk.status.is_added() {
                        colors.created_background.into()
                    } else if hunk.status.is_deleted() {
                        colors.deleted_background.into()
                    } else {
                        linear_gradient(
                            90.,
                            linear_color_stop(colors.deleted_background, 0.),
                            linear_color_stop(colors.created_background, 1.),
                        )
                    };
                    if let Some(path) = change_connection_path(gap, left_span, right_span) {
                        ribbons.push((path, color));
                    }
                }

                let endpoints = matching_left
                    .map(|(from, hunk)| (&left, hunk, from.start, true))
                    .into_iter()
                    .chain([(&right, &hunk, to.start, false)]);
                window.with_content_mask(Some(ContentMask { bounds: gap }), |window| {
                    for (editor, hunk, y, left_side) in endpoints {
                        if y < gap.top() || y + line_height > gap.bottom() {
                            continue;
                        }
                        let renderer = editor.read(cx).diff_hunk_renderer();
                        let mut element = renderer.render_hunk_controls(
                            hunk.row_range.start.0,
                            &hunk.status,
                            hunk.multi_buffer_range.clone(),
                            hunk.is_created_file(),
                            line_height,
                            editor,
                            window,
                            cx,
                        );
                        let size =
                            element.layout_as_root(size(px(24.), line_height).into(), window, cx);
                        let x = if left_side {
                            gap.left() + px(2.)
                        } else {
                            gap.right() - size.width - px(2.)
                        };
                        window.with_absolute_element_offset(point(x, y), |window| {
                            element.prepaint(window, cx);
                        });
                        controls.push(element);
                    }
                });
            }
            Some((gap, ribbons, insertion_markers, controls))
        },
        |_, connections, window, cx| {
            if let Some((bounds, ribbons, insertion_markers, controls)) = connections {
                for marker in insertion_markers {
                    window.paint_quad(marker);
                }
                window.with_content_mask(Some(ContentMask { bounds }), |window| {
                    for (path, color) in ribbons {
                        window.paint_path(path, color);
                    }
                    for mut control in controls {
                        control.paint(window, cx);
                    }
                });
            }
        },
    )
    .absolute()
    .inset_0()
    .size_full()
}

fn change_connection_path(
    gap: Bounds<Pixels>,
    left: Range<Pixels>,
    right: Range<Pixels>,
) -> Option<gpui::Path<Pixels>> {
    if left.is_empty() && right.is_empty() {
        return None;
    }
    let start = gap.left();
    let end = gap.right();
    let bend = (end - start) * 0.53;
    let mut outline = PathBuilder::fill();
    outline.move_to(point(start, left.start));
    outline.cubic_bezier_to(
        point(end, right.start),
        point(start + bend, left.start),
        point(end - bend, right.start),
    );
    outline.line_to(point(end, right.end));
    outline.cubic_bezier_to(
        point(start, left.end),
        point(end - bend, right.end),
        point(start + bend, left.end),
    );
    outline.close();
    outline.build().ok()
}

struct SplitBufferHeadersElement {
    lhs_editor: Entity<Editor>,
    rhs_editor: Entity<Editor>,
    style: EditorStyle,
    pane: Option<SplitSide>,
}

impl SplitBufferHeadersElement {
    fn new(
        lhs_editor: Entity<Editor>,
        rhs_editor: Entity<Editor>,
        style: EditorStyle,
        pane: Option<SplitSide>,
    ) -> Self {
        Self {
            lhs_editor,
            rhs_editor,
            style,
            pane,
        }
    }
}

struct BufferHeaderLayout {
    element: AnyElement,
}

struct SplitBufferHeadersPrepaintState {
    content_bounds: Bounds<Pixels>,
    sticky_header: Option<AnyElement>,
    non_sticky_headers: Vec<BufferHeaderLayout>,
}

impl IntoElement for SplitBufferHeadersElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for SplitBufferHeadersElement {
    type RequestLayoutState = ();
    type PrepaintState = SplitBufferHeadersPrepaintState;

    fn id(&self) -> Option<gpui::ElementId> {
        Some(
            match self.pane {
                Some(SplitSide::Left) => "split-left-buffer-headers",
                _ => "split-buffer-headers",
            }
            .into(),
        )
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        _cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = gpui::Style::default();
        style.position = gpui::Position::Absolute;
        style.inset.top = DefiniteLength::Fraction(0.0).into();
        style.inset.left = DefiniteLength::Fraction(0.0).into();
        style.size.width = Length::Definite(DefiniteLength::Fraction(1.0));
        style.size.height = Length::Definite(DefiniteLength::Fraction(1.0));
        let layout_id = window.request_layout(style, [], _cx);
        (layout_id, ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        if bounds.size.width <= px(0.) || bounds.size.height <= px(0.) {
            return SplitBufferHeadersPrepaintState {
                content_bounds: bounds,
                sticky_header: None,
                non_sticky_headers: Vec::new(),
            };
        }

        let rem_size = self.rem_size();
        let text_style = TextStyleRefinement {
            font_size: Some(self.style.text.font_size),
            line_height: Some(self.style.text.line_height),
            ..Default::default()
        };

        window.with_rem_size(rem_size, |window| {
            window.with_text_style(Some(text_style), |window| {
                Self::prepaint_inner(self, bounds, window, cx)
            })
        })
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let rem_size = self.rem_size();
        let text_style = TextStyleRefinement {
            font_size: Some(self.style.text.font_size),
            line_height: Some(self.style.text.line_height),
            ..Default::default()
        };

        window.with_rem_size(rem_size, |window| {
            window.with_text_style(Some(text_style), |window| {
                window.with_content_mask(
                    Some(ContentMask {
                        bounds: prepaint.content_bounds,
                    }),
                    |window| {
                        for header_layout in &mut prepaint.non_sticky_headers {
                            header_layout.element.paint(window, cx);
                        }

                        if let Some(mut sticky_header) = prepaint.sticky_header.take() {
                            sticky_header.paint(window, cx);
                        }
                    },
                );
            });
        });
    }
}

impl SplitBufferHeadersElement {
    fn rem_size(&self) -> Option<Pixels> {
        match self.style.text.font_size {
            AbsoluteLength::Pixels(pixels) => {
                let rem_size_scale = {
                    let default_font_size_scale = 14. / ui::BASE_REM_SIZE_IN_PX;
                    let default_font_size_delta = 1. - default_font_size_scale;
                    1. + default_font_size_delta
                };

                Some(pixels * rem_size_scale)
            }
            AbsoluteLength::Rems(rems) => Some(rems.to_pixels(ui::BASE_REM_SIZE_IN_PX.into())),
        }
    }

    fn prepaint_inner(
        &mut self,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) -> SplitBufferHeadersPrepaintState {
        let line_height = window.line_height();
        let source = self
            .rhs_editor
            .update(cx, |editor, cx| editor.snapshot(window, cx));
        let pane = self.pane.filter(|_| source.uses_diff_scroll_map());
        if self.pane == Some(SplitSide::Left) && pane.is_none() {
            return SplitBufferHeadersPrepaintState {
                content_bounds: bounds,
                sticky_header: None,
                non_sticky_headers: Vec::new(),
            };
        }
        let left_snapshot = (self.pane == Some(SplitSide::Left)).then(|| {
            self.lhs_editor
                .update(cx, |editor, cx| editor.snapshot(window, cx))
        });
        let snapshot = left_snapshot.as_ref().unwrap_or(&source);
        let pane_editor = match pane {
            Some(SplitSide::Left) => Some(&self.lhs_editor),
            Some(SplitSide::Right) => Some(&self.rhs_editor),
            None => None,
        };
        let bounds = pane_editor
            .and_then(|editor| editor.read(cx).last_bounds())
            .map_or(bounds, |pane_bounds| pane_bounds.intersect(&bounds));
        let scroll_position = snapshot.scroll_position();

        // Compute right margin to avoid overlapping the scrollbar
        let content_bounds = self.content_bounds(bounds, cx);
        let available_width = (content_bounds.size.width
            - pane_editor
                .unwrap_or(&self.rhs_editor)
                .read(cx)
                .last_right_margin())
        .max(Pixels::ZERO);

        let visible_height_in_lines = content_bounds.size.height / line_height;
        let max_row = snapshot.max_point().row();
        let start_row = cmp::min(DisplayRow(scroll_position.y.floor() as u32), max_row);
        let end_row = cmp::min(
            (scroll_position.y + visible_height_in_lines as f64).ceil() as u32,
            max_row.next_row().0,
        );
        let end_row = DisplayRow(end_row);

        let (selected_buffer_ids, latest_selection_anchors) =
            self.compute_selection_info(&source, cx);

        let sticky_header = if snapshot.buffer_snapshot().show_headers() {
            snapshot
                .sticky_header_excerpt(scroll_position.y)
                .and_then(|sticky_excerpt| {
                    self.build_sticky_header(
                        sticky_excerpt,
                        snapshot,
                        &source,
                        scroll_position,
                        content_bounds,
                        available_width,
                        line_height,
                        &selected_buffer_ids,
                        &latest_selection_anchors,
                        start_row,
                        end_row,
                        window,
                        cx,
                    )
                })
        } else {
            None
        };

        let sticky_header_excerpt_id = snapshot
            .sticky_header_excerpt(scroll_position.y)
            .map(|e| e.excerpt);

        let non_sticky_headers = self.build_non_sticky_headers(
            snapshot,
            &source,
            scroll_position,
            content_bounds,
            available_width,
            line_height,
            start_row,
            end_row,
            &selected_buffer_ids,
            &latest_selection_anchors,
            sticky_header_excerpt_id,
            window,
            cx,
        );

        SplitBufferHeadersPrepaintState {
            content_bounds,
            sticky_header,
            non_sticky_headers,
        }
    }

    fn content_bounds(&self, bounds: Bounds<Pixels>, cx: &App) -> Bounds<Pixels> {
        // Left hand side and right hand side horizontal scrollbars are
        // independent, so we clip the bottom if either is visible.
        let horizontal_scrollbar_height =
            (self.lhs_editor.read(cx).last_horizontal_scrollbar_visible()
                || self.rhs_editor.read(cx).last_horizontal_scrollbar_visible())
            .then_some(self.style.scrollbar_width)
            .unwrap_or(Pixels::ZERO);

        Bounds::new(
            bounds.origin,
            size(
                bounds.size.width,
                (bounds.size.height - horizontal_scrollbar_height).max(Pixels::ZERO),
            ),
        )
    }

    fn compute_selection_info(
        &self,
        snapshot: &EditorSnapshot,
        cx: &App,
    ) -> (HashSet<BufferId>, HashMap<BufferId, Anchor>) {
        let editor = self.rhs_editor.read(cx);
        let all_selections = editor
            .selections
            .all::<crate::Point>(&snapshot.display_snapshot);
        let all_anchor_selections = editor.selections.all_anchors(&snapshot.display_snapshot);

        let mut selected_buffer_ids = HashSet::default();
        for selection in &all_selections {
            for buffer_id in snapshot
                .buffer_snapshot()
                .buffer_ids_for_range(selection.range())
            {
                selected_buffer_ids.insert(buffer_id);
            }
        }

        let mut anchors_by_buffer: HashMap<BufferId, (usize, Anchor)> = HashMap::default();
        for selection in all_anchor_selections.iter() {
            let head = selection.head();
            if let Some((text_anchor, _)) = snapshot.buffer_snapshot().anchor_to_buffer_anchor(head)
            {
                anchors_by_buffer
                    .entry(text_anchor.buffer_id)
                    .and_modify(|(latest_id, latest_anchor)| {
                        if selection.id > *latest_id {
                            *latest_id = selection.id;
                            *latest_anchor = head;
                        }
                    })
                    .or_insert((selection.id, head));
            }
        }
        let latest_selection_anchors = anchors_by_buffer
            .into_iter()
            .map(|(buffer_id, (_, anchor))| (buffer_id, anchor))
            .collect();

        (selected_buffer_ids, latest_selection_anchors)
    }

    fn build_sticky_header(
        &self,
        StickyHeaderExcerpt { excerpt }: StickyHeaderExcerpt<'_>,
        snapshot: &EditorSnapshot,
        source: &EditorSnapshot,
        scroll_position: gpui::Point<ScrollOffset>,
        bounds: Bounds<Pixels>,
        available_width: Pixels,
        line_height: Pixels,
        selected_buffer_ids: &HashSet<BufferId>,
        latest_selection_anchors: &HashMap<BufferId, Anchor>,
        start_row: DisplayRow,
        end_row: DisplayRow,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<AnyElement> {
        let (excerpt, jump_row) = self.source_header(
            snapshot,
            source,
            excerpt,
            DisplayRow(scroll_position.y as u32),
        )?;
        let jump_data = header_jump_data(
            source,
            jump_row,
            FILE_HEADER_HEIGHT + MULTI_BUFFER_EXCERPT_HEADER_HEIGHT,
            excerpt,
            latest_selection_anchors,
        );

        let editor_bg_color = cx.theme().colors().editor_background;
        let selected = selected_buffer_ids.contains(&excerpt.buffer_id());

        let mut header = v_flex()
            .id("sticky-buffer-header")
            .w(available_width)
            .relative()
            .child(
                div()
                    .w(available_width)
                    .h(FILE_HEADER_HEIGHT as f32 * line_height)
                    .bg(linear_gradient(
                        0.,
                        linear_color_stop(editor_bg_color.opacity(0.), 0.),
                        linear_color_stop(editor_bg_color, 0.6),
                    ))
                    .absolute()
                    .top_0(),
            )
            .child(
                render_buffer_header(
                    &self.rhs_editor,
                    excerpt,
                    false,
                    selected,
                    true,
                    jump_data,
                    window,
                    cx,
                )
                .into_any_element(),
            )
            .into_any_element();

        let mut origin = bounds.origin;

        for (block_row, block) in snapshot.blocks_in_range(start_row..end_row) {
            if !block.is_buffer_header() {
                continue;
            }

            if block_row.0 <= scroll_position.y as u32 {
                continue;
            }

            let max_row = block_row.0.saturating_sub(FILE_HEADER_HEIGHT);
            let offset = scroll_position.y - max_row as f64;

            if offset > 0.0 {
                origin.y -= Pixels::from(offset * f64::from(line_height));
            }
            break;
        }

        let available_size = size(
            AvailableSpace::Definite(available_width),
            AvailableSpace::MinContent,
        );

        Self::prepaint_header(&mut header, origin, available_size, bounds, window, cx);

        Some(header)
    }

    fn build_non_sticky_headers(
        &self,
        snapshot: &EditorSnapshot,
        source: &EditorSnapshot,
        scroll_position: gpui::Point<ScrollOffset>,
        bounds: Bounds<Pixels>,
        available_width: Pixels,
        line_height: Pixels,
        start_row: DisplayRow,
        end_row: DisplayRow,
        selected_buffer_ids: &HashSet<BufferId>,
        latest_selection_anchors: &HashMap<BufferId, Anchor>,
        sticky_header: Option<&ExcerptBoundaryInfo>,
        window: &mut Window,
        cx: &mut App,
    ) -> Vec<BufferHeaderLayout> {
        let mut headers = Vec::new();

        for (block_row, block) in snapshot.blocks_in_range(start_row..end_row) {
            let (excerpt, is_folded) = match block {
                Block::BufferHeader { excerpt, .. } => {
                    if sticky_header == Some(excerpt) {
                        continue;
                    }
                    (excerpt, false)
                }
                Block::FoldedBuffer { first_excerpt, .. } => (first_excerpt, true),
                // ExcerptBoundary is just a separator line, not a buffer header
                Block::ExcerptBoundary { .. } | Block::Custom(_) | Block::Spacer { .. } => continue,
            };

            let Some((excerpt, jump_row)) =
                self.source_header(snapshot, source, excerpt, block_row)
            else {
                continue;
            };
            let selected = selected_buffer_ids.contains(&excerpt.buffer_id());
            let jump_data = header_jump_data(
                source,
                jump_row,
                block.height(),
                excerpt,
                latest_selection_anchors,
            );

            let mut header = render_buffer_header(
                &self.rhs_editor,
                excerpt,
                is_folded,
                selected,
                false,
                jump_data,
                window,
                cx,
            )
            .into_any_element();

            let y_offset = (block_row.0 as f64 - scroll_position.y) * f64::from(line_height);
            let origin = point(bounds.origin.x, bounds.origin.y + Pixels::from(y_offset));

            let available_size = size(
                AvailableSpace::Definite(available_width),
                AvailableSpace::MinContent,
            );

            Self::prepaint_header(&mut header, origin, available_size, bounds, window, cx);

            headers.push(BufferHeaderLayout { element: header });
        }

        headers
    }

    fn source_header<'a>(
        &self,
        snapshot: &EditorSnapshot,
        source: &'a EditorSnapshot,
        excerpt: &'a ExcerptBoundaryInfo,
        row: DisplayRow,
    ) -> Option<(&'a ExcerptBoundaryInfo, DisplayRow)> {
        if self.pane != Some(SplitSide::Left) {
            return Some((excerpt, row));
        }
        let id = snapshot
            .buffer_snapshot()
            .diff_for_buffer_id(excerpt.buffer_id())?
            .buffer_id();
        source
            .blocks_in_range(DisplayRow(0)..source.max_point().row().next_row())
            .find_map(|(row, block)| {
                let excerpt = match block {
                    Block::BufferHeader { excerpt, .. } => excerpt,
                    Block::FoldedBuffer { first_excerpt, .. } => first_excerpt,
                    _ => return None,
                };
                (excerpt.buffer_id() == id).then_some((excerpt, row))
            })
    }

    fn prepaint_header(
        header: &mut AnyElement,
        origin: gpui::Point<Pixels>,
        available_size: gpui::Size<AvailableSpace>,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) {
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            header.prepaint_as_root(origin, available_size, window, cx);
        });
    }
}
