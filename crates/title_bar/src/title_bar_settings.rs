use gpui::WindowButtonLayout;
use settings::{RegisterSetting, Settings, SettingsContent};

#[derive(Copy, Clone, Debug, RegisterSetting)]
pub struct TitleBarSettings {
    pub show_onboarding_banner: bool,
    pub show_project_items: bool,
    pub button_layout: Option<WindowButtonLayout>,
}

impl Settings for TitleBarSettings {
    fn from_settings(s: &SettingsContent) -> Self {
        let content = s.title_bar.clone().unwrap();
        TitleBarSettings {
            show_onboarding_banner: content.show_onboarding_banner.unwrap(),
            show_project_items: content.show_project_items.unwrap(),
            button_layout: content.button_layout.unwrap_or_default().into_layout(),
        }
    }
}
