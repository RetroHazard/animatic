//! One shape for every floating window: no collapse triangle, opened centred over the app
//! instead of on top of the menu bar, and closed by Escape when it is the topmost window.

use eframe::egui::{self, Align2, InnerResponse};

/// A floating window opened at the centre of the app. Its id comes from `title`; give it an
/// explicit `.id(..)` when the title changes while it is open.
pub fn window<'a>(ctx: &egui::Context, title: impl Into<String>) -> egui::Window<'a> {
    egui::Window::new(title.into())
        .collapsible(false)
        .pivot(Align2::CENTER_CENTER)
        .default_pos(ctx.content_rect().center())
}

/// Whether Escape should close the window that produced `shown`: it is the topmost window, no
/// text field is being edited, and no menu, popup or modal dialog is open (those take Escape
/// first). The key is consumed, so one press closes one window.
pub fn escape_closes<R>(ctx: &egui::Context, shown: Option<&InnerResponse<R>>) -> bool {
    let Some(shown) = shown else {
        return false;
    };
    if ctx.text_edit_focused()
        || ctx.any_popup_open()
        || ctx.memory(|m| m.top_modal_layer().is_some())
        || ctx.top_layer_id() != Some(shown.response.layer_id)
    {
        return false;
    }
    ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
}
