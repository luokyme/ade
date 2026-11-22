mod diagnostic_renderer;

#[cfg(test)]
mod diagnostics_tests;

pub fn init(cx: &mut gpui::App) {
    editor::set_diagnostic_renderer(diagnostic_renderer::DiagnosticRenderer, cx);
}
