use gpui::{AnyElement, IntoElement, Stateful};
use smallvec::SmallVec;

use crate::prelude::*;

const TAB_HEIGHT: Pixels = px(26.);
const TAB_SLOT_SIZE: Pixels = px(18.);

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum TabCloseSide {
    Start,
    End,
}

#[derive(IntoElement, RegisterComponent)]
pub struct Tab {
    div: Stateful<Div>,
    selected: bool,
    compact: bool,
    close_side: TabCloseSide,
    start_slot: Option<AnyElement>,
    end_slot: Option<AnyElement>,
    children: SmallVec<[AnyElement; 2]>,
}

impl Tab {
    pub fn new(id: impl Into<ElementId>) -> Self {
        let id = id.into();
        Self {
            div: div()
                .id(id.clone())
                .debug_selector(|| format!("TAB-{}", id)),
            selected: false,
            compact: false,
            close_side: TabCloseSide::End,
            start_slot: None,
            end_slot: None,
            children: SmallVec::new(),
        }
    }

    pub fn close_side(mut self, close_side: TabCloseSide) -> Self {
        self.close_side = close_side;
        self
    }

    /// Uses compact spacing and displays the end slot as an overlay on hover.
    pub fn compact(mut self) -> Self {
        self.compact = true;
        self
    }

    pub fn start_slot<E: IntoElement>(mut self, element: impl Into<Option<E>>) -> Self {
        self.start_slot = element.into().map(IntoElement::into_any_element);
        self
    }

    pub fn end_slot<E: IntoElement>(mut self, element: impl Into<Option<E>>) -> Self {
        self.end_slot = element.into().map(IntoElement::into_any_element);
        self
    }

    pub fn content_height(cx: &App) -> Pixels {
        DynamicSpacing::Base32.px(cx) - px(1.)
    }

    pub fn container_height(cx: &App) -> Pixels {
        DynamicSpacing::Base32.px(cx)
    }
}

impl InteractiveElement for Tab {
    fn interactivity(&mut self) -> &mut gpui::Interactivity {
        self.div.interactivity()
    }
}

impl StatefulInteractiveElement for Tab {}

impl Toggleable for Tab {
    fn toggle_state(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }
}

impl ParentElement for Tab {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements)
    }
}

impl RenderOnce for Tab {
    #[allow(refining_impl_trait)]
    fn render(self, window: &mut Window, cx: &mut App) -> Stateful<Div> {
        let colors = cx.theme().colors();
        let text_color = if self.selected {
            colors.text
        } else {
            colors.text_muted
        };
        let content = h_flex()
            .flex_1()
            .min_w_0()
            .gap(px(3.))
            .children(self.children);

        let tab = self
            .div
            .group("")
            .flex()
            .flex_none()
            .items_center()
            .h(TAB_HEIGHT)
            .rounded_full()
            .text_color(text_color)
            .cursor_pointer()
            .map(|this| {
                if self.selected {
                    this.bg(colors.element_selected)
                } else {
                    this.hover(|style| style.bg(colors.element_hover))
                }
            });

        if self.compact {
            let title_bar_background = if cfg!(any(target_os = "linux", target_os = "freebsd"))
                && !window.is_window_active()
            {
                colors.title_bar_inactive_background
            } else {
                colors.title_bar_background
            };
            let tab_background = if self.selected {
                colors.element_selected
            } else {
                colors.element_hover
            };

            return tab
                .relative()
                .min_w(TAB_SLOT_SIZE + px(4.))
                .px(px(6.))
                .gap(px(4.))
                .when_some(self.start_slot, |tab, indicator| {
                    tab.child(h_flex().flex_none().child(indicator))
                })
                .child(content)
                .when_some(self.end_slot, |tab, end_slot| {
                    tab.child(
                        h_flex()
                            .absolute()
                            .right(px(2.))
                            .top_0()
                            .bottom_0()
                            .visible_on_hover("")
                            .child(
                                h_flex()
                                    .size(TAB_SLOT_SIZE)
                                    .justify_center()
                                    .rounded_full()
                                    .bg(title_bar_background.blend(tab_background))
                                    .child(end_slot),
                            ),
                    )
                });
        }

        let has_end_slot = self.end_slot.is_some();
        let slot_layer = || h_flex().absolute().inset_0().justify_center();
        let slot = h_flex()
            .relative()
            .flex_none()
            .size(TAB_SLOT_SIZE)
            .when_some(self.start_slot, |slot, indicator| {
                slot.child(
                    slot_layer()
                        .when(has_end_slot, |layer| {
                            layer.group_hover("", |style| style.invisible())
                        })
                        .child(indicator),
                )
            })
            .when_some(self.end_slot, |slot, end_slot| {
                slot.child(slot_layer().child(end_slot))
            });

        tab.pl(px(10.))
            .pr(px(6.))
            .gap(px(5.))
            .map(|this| match self.close_side {
                TabCloseSide::End => this.child(content).child(slot),
                TabCloseSide::Start => this.child(slot).child(content),
            })
    }
}

impl Component for Tab {
    fn scope() -> ComponentScope {
        ComponentScope::Navigation
    }

    fn description() -> &'static str {
        "A rounded tab chip for tabbed interfaces, with selected and unselected states."
    }

    fn preview(_window: &mut Window, _cx: &mut App) -> AnyElement {
        v_flex()
            .gap_6()
            .children(vec![example_group_with_title(
                "Variations",
                vec![
                    single_example(
                        "Default",
                        Tab::new("default").child("Default Tab").into_any_element(),
                    ),
                    single_example(
                        "Selected",
                        Tab::new("selected")
                            .toggle_state(true)
                            .child("Selected Tab")
                            .into_any_element(),
                    ),
                ],
            )])
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, rc::Rc};

    use gpui::{Modifiers, TestAppContext, point};

    use super::*;

    #[gpui::test]
    fn compact_tab_close_button_overlays_the_title_on_hover(cx: &mut TestAppContext) {
        struct TestTabs(Rc<Cell<bool>>);

        impl Render for TestTabs {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                let closed = self.0.clone();
                h_flex()
                    .gap_1()
                    .child(
                        Tab::new("closable")
                            .compact()
                            .end_slot(
                                IconButton::new("close", IconName::Close)
                                    .on_click(move |_, _, _| closed.set(true)),
                            )
                            .child("main.rs"),
                    )
                    .child(Tab::new("without-close").compact().child("main.rs"))
            }
        }

        cx.update(|cx| {
            let settings_store = settings::SettingsStore::test(cx);
            cx.set_global(settings_store);
            theme_settings::init(theme::LoadThemes::JustBase, cx);
        });
        let closed = Rc::new(Cell::new(false));
        let (_, cx) = cx.add_window_view(|_, _| TestTabs(closed.clone()));

        let tab = cx.debug_bounds("TAB-closable").expect("closable tab");
        let without_close = cx
            .debug_bounds("TAB-without-close")
            .expect("tab without close");
        cx.simulate_mouse_move(
            point(tab.left(), tab.bottom() + px(8.)),
            None,
            Modifiers::default(),
        );
        cx.run_until_parked();

        assert_eq!(tab.size.width, without_close.size.width);
        assert!(cx.debug_bounds("ICON-Close").is_none());

        cx.simulate_mouse_move(tab.center(), None, Modifiers::default());
        cx.run_until_parked();
        let button = cx
            .debug_bounds("ICON-Close")
            .expect("close button on hover");
        assert!(tab.contains(&button.center()));
        assert!(button.center().x > tab.center().x);
        assert_eq!(cx.debug_bounds("TAB-closable"), Some(tab));

        cx.simulate_click(button.center(), Modifiers::default());
        assert!(closed.get());
    }
}
