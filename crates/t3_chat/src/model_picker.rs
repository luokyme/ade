//! Choosing a thread's model, its options such as reasoning level and fast mode, and its
//! permission mode. Laid out like T3 Code's own picker so both clients read the same.

use crate::{
    store::T3Store,
    style::{accent, hairline, icon, ink, ui},
};
use editor::Editor;
use gpui::{
    App, ClickEvent, Context, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable,
    FontWeight, KeyDownEvent, SharedString, Subscription, Window,
};
use i18n::{n, t};
use serde_json::Value;
use std::rc::Rc;
use t3_client::{
    ModelSelection,
    protocol::{OptionDescriptor, ProviderInfo, ProviderModel},
};
use theme::ActiveTheme as _;
use ui::prelude::*;

const PICKER_WIDTH: f32 = 560.;
const PICKER_HEIGHT: f32 = 320.;
const PROVIDER_COLUMN: f32 = 46.;
const MODEL_COLUMN: f32 = 236.;
const ROW_HEIGHT: f32 = 28.;

pub type OnSelection = Rc<dyn Fn(ModelSelection, &mut Window, &mut App)>;
pub type OnMode = Rc<dyn Fn(&'static str, &mut Window, &mut App)>;

pub fn provider_icon(driver: &str) -> IconName {
    match driver {
        "claudeAgent" | "claude" => IconName::AgentClaude,
        "codex" => IconName::AgentCodex,
        "opencode" => IconName::AgentOpencode,
        "antigravity" => IconName::AgentAntigravity,
        _ => IconName::AgentBot,
    }
}

/// The provider's brand color; OpenAI's mark is monochrome, so Codex and unknown providers follow the theme.
pub fn provider_color(driver: &str) -> Option<gpui::Hsla> {
    let hex = match driver {
        "claudeAgent" | "claude" => 0xD97757,
        "opencode" => 0x3B82F6,
        "antigravity" => 0x8B5CF6,
        _ => return None,
    };
    Some(gpui::rgb(hex).into())
}

/// How a model turns fast mode on: Claude has a `fastMode` switch, Codex a "Fast" service tier.
pub struct FastToggle {
    pub id: String,
    pub on: Value,
    pub off: Value,
}

pub fn fast_toggle(model: &ProviderModel) -> Option<FastToggle> {
    model.options().iter().find_map(|option| {
        if option.is_boolean() {
            let is_fast = option.id == "fastMode" || option.label.to_lowercase().contains("fast");
            return is_fast.then(|| FastToggle {
                id: option.id.clone(),
                on: Value::Bool(true),
                off: Value::Bool(false),
            });
        }
        let fast = option
            .options
            .iter()
            .find(|choice| choice.id == "priority" || choice.label.eq_ignore_ascii_case("fast"))?;
        let standard = option
            .options
            .iter()
            .find(|choice| choice.id != fast.id && choice.is_default)
            .or_else(|| option.options.iter().find(|choice| choice.id != fast.id))?;
        Some(FastToggle {
            id: option.id.clone(),
            on: Value::String(fast.id.clone()),
            off: Value::String(standard.id.clone()),
        })
    })
}

pub fn option_value(selection: &ModelSelection, option: &OptionDescriptor) -> Option<Value> {
    selection
        .option(&option.id)
        .cloned()
        .or_else(|| option.default_value())
}

pub fn is_fast(selection: &ModelSelection, model: &ProviderModel) -> bool {
    fast_toggle(model).is_some_and(|toggle| {
        let current = selection.option(&toggle.id).cloned().or_else(|| {
            model
                .options()
                .iter()
                .find(|option| option.id == toggle.id)
                .and_then(OptionDescriptor::default_value)
        });
        current.as_ref() == Some(&toggle.on)
    })
}

/// The label of the chosen reasoning level, such as "Medium".
pub fn reasoning_label(selection: &ModelSelection, model: &ProviderModel) -> Option<String> {
    let option = model
        .options()
        .iter()
        .find(|option| option.is_reasoning())?;
    let value = option_value(selection, option)?;
    let value = value.as_str()?;
    option
        .options
        .iter()
        .find(|choice| choice.id == value)
        .map(|choice| choice.label.clone())
}

/// Keeps the options the new model also offers with the same choice, so switching models keeps
/// the reasoning level and fast mode where it can.
fn carry_options(from: &ModelSelection, to: &ProviderModel) -> Option<Value> {
    let mut selection = ModelSelection {
        instance_id: String::new(),
        model: to.slug.clone(),
        options: None,
    };
    for option in to.options() {
        let Some(value) = from.option(&option.id) else {
            continue;
        };
        let valid = if option.is_boolean() {
            value.is_boolean()
        } else {
            value
                .as_str()
                .is_some_and(|value| option.options.iter().any(|choice| choice.id == value))
        };
        if valid {
            selection.set_option(&option.id, value.clone());
        }
    }
    selection.options
}

pub struct ModelPicker {
    store: Entity<T3Store>,
    selection: ModelSelection,
    provider: String,
    search: Entity<Editor>,
    on_change: OnSelection,
    _subscription: Subscription,
}

impl EventEmitter<DismissEvent> for ModelPicker {}

impl Focusable for ModelPicker {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.search.focus_handle(cx)
    }
}

impl ModelPicker {
    pub fn new(
        store: Entity<T3Store>,
        selection: ModelSelection,
        on_change: OnSelection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search = cx.new(|cx| {
            let mut editor = Editor::single_line(window, cx);
            editor.set_placeholder_text(&t!("Search models…"), window, cx);
            editor
        });
        let subscription = cx.observe(&search, |_, _, cx| cx.notify());
        Self {
            store,
            provider: selection.instance_id.clone(),
            selection,
            search,
            on_change,
            _subscription: subscription,
        }
    }

    fn change(&mut self, selection: ModelSelection, window: &mut Window, cx: &mut Context<Self>) {
        self.selection = selection.clone();
        (self.on_change)(selection, window, cx);
        cx.notify();
    }

    fn pick_model(
        &mut self,
        provider: &ProviderInfo,
        model: &ProviderModel,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let selection = ModelSelection {
            instance_id: provider.instance_id.clone(),
            model: model.slug.clone(),
            options: carry_options(&self.selection, model),
        };
        self.change(selection, window, cx);
    }

    fn set_option(&mut self, id: &str, value: Value, window: &mut Window, cx: &mut Context<Self>) {
        let mut selection = self.selection.clone();
        selection.set_option(id, value);
        self.change(selection, window, cx);
    }

    fn render_providers(
        &self,
        providers: &[ProviderInfo],
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let colors = cx.theme().colors();
        v_flex()
            .w(px(PROVIDER_COLUMN))
            .flex_none()
            .h_full()
            .py(px(8.))
            .gap(px(4.))
            .items_center()
            .border_r_1()
            .border_color(hairline(0.08, cx))
            .children(providers.iter().map(|provider| {
                let selected = provider.instance_id == self.provider;
                let instance = provider.instance_id.clone();
                div()
                    .id(SharedString::from(format!(
                        "t3-provider-{}",
                        provider.instance_id
                    )))
                    .size(px(32.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(8.))
                    .cursor_pointer()
                    .when(selected, |this| this.bg(ink(0.08, cx)))
                    .hover(|style| style.bg(ink(0.06, cx)))
                    .tooltip(ui::Tooltip::text(provider.display_name.clone()))
                    .child(icon(provider_icon(&provider.driver), 16.).text_color(
                        provider_color(&provider.driver).unwrap_or(if selected {
                            colors.text
                        } else {
                            colors.text_muted
                        }),
                    ))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.provider = instance.clone();
                        cx.notify();
                    }))
            }))
    }

    fn render_models(
        &self,
        providers: &[ProviderInfo],
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let colors = cx.theme().colors();
        let query = self.search.read(cx).text(cx).trim().to_lowercase();
        let provider = providers
            .iter()
            .find(|provider| provider.instance_id == self.provider)
            .cloned();
        let mut models = provider
            .as_ref()
            .map(|provider| provider.models.clone())
            .unwrap_or_default();
        // A legacy model stays listed only while it is the current choice, so the selection
        // never vanishes from its own picker.
        models.retain(|model| {
            let current =
                self.selection.instance_id == self.provider && self.selection.model == model.slug;
            (!model.is_legacy || current)
                && (query.is_empty()
                    || model.name.to_lowercase().contains(&query)
                    || model.slug.to_lowercase().contains(&query))
        });
        let rows = provider.map(|provider| {
            let brand = provider_icon(&provider.driver);
            let brand_color = provider_color(&provider.driver).unwrap_or(colors.text);
            models
                .into_iter()
                .map(|model| {
                    let selected = self.selection.instance_id == provider.instance_id
                        && self.selection.model == model.slug;
                    let picked = (provider.clone(), model.clone());
                    h_flex()
                        .id(SharedString::from(format!(
                            "t3-model-{}-{}",
                            provider.instance_id, model.slug
                        )))
                        .h(px(ROW_HEIGHT))
                        .px(px(8.))
                        .gap(px(8.))
                        .rounded(px(6.))
                        .cursor_pointer()
                        .when(selected, |this| this.bg(ink(0.06, cx)))
                        .hover(|style| style.bg(ink(0.08, cx)))
                        .child(icon(brand, 14.).text_color(brand_color))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_color(if model.is_legacy {
                                    colors.text_muted
                                } else {
                                    colors.text
                                })
                                .child(if model.name.is_empty() {
                                    model.slug
                                } else {
                                    model.name
                                }),
                        )
                        .when(selected, |this| {
                            this.child(icon(IconName::Check, 13.).text_color(colors.text_muted))
                        })
                        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                            this.pick_model(&picked.0, &picked.1, window, cx)
                        }))
                        .into_any_element()
                })
                .collect::<Vec<_>>()
        });
        v_flex()
            .w(px(MODEL_COLUMN))
            .flex_none()
            .h_full()
            .border_r_1()
            .border_color(hairline(0.08, cx))
            .child(
                h_flex()
                    .h(px(38.))
                    .mx(px(8.))
                    .gap(px(8.))
                    .border_b_1()
                    .border_color(hairline(0.10, cx))
                    .child(icon(IconName::MagnifyingGlass, 13.).text_color(colors.text_muted))
                    .child(div().flex_1().min_w_0().child(self.search.clone())),
            )
            .child(
                v_flex()
                    .id("t3-model-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p(px(6.))
                    .gap(px(2.))
                    .children(rows.into_iter().flatten()),
            )
    }

    fn render_options(
        &self,
        model: Option<&ProviderModel>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let colors = cx.theme().colors();
        let fast = model.and_then(fast_toggle);
        let options = model
            .map(|model| model.options().to_vec())
            .unwrap_or_default()
            .into_iter()
            .filter(|option| fast.as_ref().is_none_or(|fast| fast.id != option.id))
            .filter(|option| option.is_boolean() || option.options.len() > 1)
            .collect::<Vec<_>>();
        let sections = options.into_iter().map(|option| {
            let current = option_value(&self.selection, &option);
            let header = div()
                .h(px(26.))
                .px(px(8.))
                .flex()
                .items_center()
                .text_size(ui(11.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(colors.text_muted)
                .child(option.label.clone());
            if option.is_boolean() {
                let on = current.as_ref() == Some(&Value::Bool(true));
                let id = option.id.clone();
                return v_flex()
                    .child(
                        h_flex()
                            .id(SharedString::from(format!("t3-option-{}", option.id)))
                            .h(px(ROW_HEIGHT))
                            .px(px(8.))
                            .gap(px(8.))
                            .rounded(px(6.))
                            .cursor_pointer()
                            .hover(|style| style.bg(ink(0.06, cx)))
                            .child(div().flex_1().text_color(colors.text).child(option.label))
                            .child(switch(on, cx))
                            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                                this.set_option(&id, Value::Bool(!on), window, cx)
                            })),
                    )
                    .into_any_element();
            }
            v_flex()
                .child(header)
                .children(option.options.iter().map(|choice| {
                    let selected =
                        current.as_ref().and_then(Value::as_str) == Some(choice.id.as_str());
                    let id = option.id.clone();
                    let value = Value::String(choice.id.clone());
                    h_flex()
                        .id(SharedString::from(format!(
                            "t3-option-{}-{}",
                            option.id, choice.id
                        )))
                        .h(px(ROW_HEIGHT))
                        .px(px(8.))
                        .gap(px(8.))
                        .rounded(px(6.))
                        .cursor_pointer()
                        .when(selected, |this| this.bg(ink(0.06, cx)))
                        .hover(|style| style.bg(ink(0.08, cx)))
                        .child(
                            div()
                                .flex_1()
                                .text_color(colors.text)
                                .child(choice.label.clone()),
                        )
                        .when(selected, |this| {
                            this.child(icon(IconName::Check, 13.).text_color(colors.text_muted))
                        })
                        .when_some(choice.description.clone(), |this, description| {
                            this.tooltip(ui::Tooltip::text(description))
                        })
                        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                            this.set_option(&id, value.clone(), window, cx)
                        }))
                }))
                .into_any_element()
        });
        v_flex()
            .id("t3-model-options")
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_y_scroll()
            .p(px(6.))
            .gap(px(6.))
            .children(sections)
            .when(model.is_none(), |this| {
                this.child(
                    div()
                        .p(px(8.))
                        .text_color(colors.text_muted)
                        .child(t!("Choose a model")),
                )
            })
    }

    fn render_footer(
        &self,
        model: Option<&ProviderModel>,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement> {
        let fast = model.and_then(fast_toggle)?;
        let on = model.is_some_and(|model| is_fast(&self.selection, model));
        let segment = |id: &'static str,
                       label: SharedString,
                       selected: bool,
                       value: Value,
                       fast_icon: bool| {
            let option = fast.id.clone();
            h_flex()
                .id(id)
                .h(px(24.))
                .px(px(10.))
                .gap(px(4.))
                .rounded(px(6.))
                .cursor_pointer()
                .text_size(ui(12.))
                .text_color(if selected {
                    cx.theme().colors().text
                } else {
                    cx.theme().colors().text_muted
                })
                .when(selected, |this| this.bg(ink(0.10, cx)))
                .hover(|style| style.bg(ink(0.06, cx)))
                .when(fast_icon, |this| {
                    this.child(
                        icon(IconName::AgentFastFilled, 12.).text_color(if selected {
                            accent(cx)
                        } else {
                            cx.theme().colors().text_muted
                        }),
                    )
                })
                .child(label)
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    this.set_option(&option, value.clone(), window, cx)
                }))
        };
        Some(
            h_flex()
                .h(px(40.))
                .px(px(10.))
                .border_t_1()
                .border_color(hairline(0.08, cx))
                .child(
                    h_flex()
                        .p(px(2.))
                        .gap(px(2.))
                        .rounded(px(8.))
                        .bg(ink(0.04, cx))
                        .child(segment(
                            "t3-speed-standard",
                            t!("Standard"),
                            !on,
                            fast.off.clone(),
                            false,
                        ))
                        .child(segment(
                            "t3-speed-fast",
                            t!("Fast"),
                            on,
                            fast.on.clone(),
                            true,
                        )),
                ),
        )
    }
}

fn switch(on: bool, cx: &App) -> impl IntoElement {
    let track = if on { accent(cx) } else { ink(0.16, cx) };
    div()
        .w(px(26.))
        .h(px(16.))
        .flex_none()
        .rounded_full()
        .bg(track)
        .p(px(2.))
        .flex()
        .when(on, |this| this.justify_end())
        .child(div().size(px(12.)).rounded_full().bg(gpui::white()))
}

impl Render for ModelPicker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let providers = self
            .store
            .read(cx)
            .providers()
            .iter()
            .filter(|provider| provider.is_ready())
            .cloned()
            .collect::<Vec<_>>();
        if !providers
            .iter()
            .any(|provider| provider.instance_id == self.provider)
            && let Some(first) = providers.first()
        {
            self.provider = first.instance_id.clone();
        }
        let model = providers
            .iter()
            .find(|provider| provider.instance_id == self.selection.instance_id)
            .and_then(|provider| provider.model(&self.selection.model))
            .cloned();
        let colors = cx.theme().colors();
        v_flex()
            .w(px(PICKER_WIDTH))
            .h(px(PICKER_HEIGHT))
            .rounded(px(12.))
            .border_1()
            .border_color(colors.border)
            .bg(colors.elevated_surface_background)
            .shadow_lg()
            .overflow_hidden()
            .text_size(ui(13.))
            .on_mouse_down_out(cx.listener(|_, _, _, cx| cx.emit(DismissEvent)))
            .on_key_down(cx.listener(|_, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    cx.emit(DismissEvent);
                }
            }))
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .items_start()
                    .child(self.render_providers(&providers, cx))
                    .child(self.render_models(&providers, cx))
                    .child(self.render_options(model.as_ref(), cx)),
            )
            .children(self.render_footer(model.as_ref(), cx))
    }
}

/// Permission modes in the order T3 Code shows them, from most to least supervised.
pub const RUNTIME_MODES: [(&str, IconName, &str, &str); 4] = [
    (
        "approval-required",
        IconName::Lock,
        n!("Supervised"),
        n!("Ask before commands and file changes."),
    ),
    (
        "auto-accept-edits",
        IconName::Pencil,
        n!("Auto-accept edits"),
        n!("Auto-approve edits, ask before other actions."),
    ),
    (
        "auto",
        IconName::Sparkle,
        n!("Auto"),
        n!("Supported providers approve routine actions; others still ask."),
    ),
    (
        "full-access",
        IconName::Warning,
        n!("Full access"),
        n!("Allow commands and edits without prompts."),
    ),
];

pub fn runtime_mode_info(mode: &str) -> (IconName, SharedString) {
    RUNTIME_MODES
        .iter()
        .find(|(id, ..)| *id == mode)
        .map(|(_, icon, label, _)| (*icon, i18n::translate(label)))
        .unwrap_or((IconName::Warning, i18n::translate(RUNTIME_MODES[3].2)))
}

pub struct PermissionPicker {
    mode: String,
    supported: Vec<&'static str>,
    on_change: OnMode,
    focus_handle: FocusHandle,
}

impl EventEmitter<DismissEvent> for PermissionPicker {}

impl Focusable for PermissionPicker {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl PermissionPicker {
    pub fn new(
        mode: String,
        provider: Option<&ProviderInfo>,
        on_change: OnMode,
        cx: &mut Context<Self>,
    ) -> Self {
        let supported = RUNTIME_MODES
            .iter()
            .map(|(id, ..)| *id)
            .filter(|id| provider.is_none_or(|provider| provider.supports_runtime_mode(id)))
            .collect();
        Self {
            mode,
            supported,
            on_change,
            focus_handle: cx.focus_handle(),
        }
    }
}

impl Render for PermissionPicker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors();
        let current = RUNTIME_MODES
            .iter()
            .find(|(id, ..)| *id == self.mode)
            .copied()
            .unwrap_or(RUNTIME_MODES[3]);
        v_flex()
            .track_focus(&self.focus_handle)
            .w(px(300.))
            .p(px(12.))
            .gap(px(8.))
            .rounded(px(12.))
            .border_1()
            .border_color(colors.border)
            .bg(colors.elevated_surface_background)
            .shadow_lg()
            .text_size(ui(13.))
            .on_mouse_down_out(cx.listener(|_, _, _, cx| cx.emit(DismissEvent)))
            .on_key_down(cx.listener(|_, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    cx.emit(DismissEvent);
                }
            }))
            .child(
                div()
                    .text_size(ui(11.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(colors.text_muted)
                    .child(t!("Permissions")),
            )
            .child(
                h_flex()
                    .p(px(2.))
                    .gap(px(2.))
                    .rounded(px(8.))
                    .bg(ink(0.05, cx))
                    .children(RUNTIME_MODES.iter().map(|(id, mode_icon, label, _)| {
                        let id: &'static str = id;
                        let enabled = self.supported.contains(&id);
                        let selected = self.mode == id;
                        div()
                            .id(id)
                            .flex_1()
                            .h(px(26.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(6.))
                            .when(selected, |this| this.bg(ink(0.10, cx)))
                            .when(!enabled, |this| this.opacity(0.35))
                            .when(enabled, |this| {
                                this.cursor_pointer()
                                    .hover(|style| style.bg(ink(0.07, cx)))
                                    .on_click(cx.listener(
                                        move |this, _: &ClickEvent, window, cx| {
                                            this.mode = id.to_string();
                                            (this.on_change)(id, window, cx);
                                            cx.notify();
                                        },
                                    ))
                            })
                            .tooltip(ui::Tooltip::text(i18n::translate(label)))
                            .child(icon(*mode_icon, 14.).text_color(if selected {
                                colors.text
                            } else {
                                colors.text_muted
                            }))
                    })),
            )
            .child(
                v_flex()
                    .gap(px(2.))
                    .child(
                        div()
                            .text_color(colors.text)
                            .child(i18n::translate(current.2)),
                    )
                    .child(
                        div()
                            .text_size(ui(12.))
                            .text_color(colors.text_muted)
                            .child(i18n::translate(current.3)),
                    ),
            )
    }
}
