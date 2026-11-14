//! Animatic's look: one spacing and type scale for both themes, a dark and a light palette with
//! readable secondary text, and the semantic colours (warning, error, success, accent) that every
//! page uses instead of fixed colours, so messages stay legible in either theme.

use egui::{
    Color32, CornerRadius, FontFamily, FontId, Margin, RichText, Shadow, Stroke, TextStyle, Theme,
    Visuals, vec2,
};

/// Text style of section headings inside a page or window (between `Heading` and `Body`).
pub fn section_style() -> TextStyle {
    TextStyle::Name("Section".into())
}

/// Installs the spacing, type scale and both palettes. Call once at startup; the theme choice
/// itself (`ctx.set_theme`) stays separate, so `System` keeps following the OS.
pub fn install(ctx: &egui::Context) {
    ctx.all_styles_mut(|style| {
        style.text_styles = [
            (
                TextStyle::Small,
                FontId::new(11.0, FontFamily::Proportional),
            ),
            (TextStyle::Body, FontId::new(13.5, FontFamily::Proportional)),
            (
                TextStyle::Button,
                FontId::new(13.5, FontFamily::Proportional),
            ),
            (
                TextStyle::Heading,
                FontId::new(21.0, FontFamily::Proportional),
            ),
            (section_style(), FontId::new(16.0, FontFamily::Proportional)),
            (
                TextStyle::Monospace,
                FontId::new(12.5, FontFamily::Monospace),
            ),
        ]
        .into();
        let spacing = &mut style.spacing;
        spacing.item_spacing = vec2(8.0, 5.0);
        spacing.button_padding = vec2(8.0, 3.0);
        spacing.interact_size.y = 20.0;
        spacing.menu_margin = Margin::same(6);
        spacing.window_margin = Margin::same(12);
        spacing.indent = 18.0;
        let visuals = &mut style.visuals;
        visuals.window_corner_radius = CornerRadius::same(8);
        visuals.menu_corner_radius = CornerRadius::same(6);
        for widget in [
            &mut visuals.widgets.noninteractive,
            &mut visuals.widgets.inactive,
            &mut visuals.widgets.hovered,
            &mut visuals.widgets.active,
            &mut visuals.widgets.open,
        ] {
            widget.corner_radius = CornerRadius::same(5);
        }
    });
    ctx.style_mut_of(Theme::Dark, |style| dark(&mut style.visuals));
    ctx.style_mut_of(Theme::Light, |style| light(&mut style.visuals));
}

fn dark(v: &mut Visuals) {
    let accent = Color32::from_rgb(232, 98, 156);
    v.panel_fill = Color32::from_rgb(30, 32, 37);
    v.window_fill = Color32::from_rgb(37, 40, 46);
    v.faint_bg_color = Color32::from_rgb(36, 38, 44);
    v.extreme_bg_color = Color32::from_rgb(20, 22, 26);
    v.code_bg_color = Color32::from_rgb(44, 47, 54);
    v.window_stroke = Stroke::new(1.0, Color32::from_rgb(62, 66, 74));
    v.window_shadow = Shadow {
        offset: [0, 6],
        blur: 20,
        spread: 0,
        color: Color32::from_black_alpha(110),
    };
    v.popup_shadow = Shadow {
        offset: [0, 4],
        blur: 12,
        spread: 0,
        color: Color32::from_black_alpha(90),
    };
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, Color32::from_rgb(52, 56, 64));
    // Body text and headings: 9:1 on the panel (the egui default is 5:1).
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, Color32::from_rgb(200, 203, 210));
    v.widgets.inactive.weak_bg_fill = Color32::from_rgb(50, 54, 62);
    v.widgets.inactive.bg_fill = Color32::from_rgb(50, 54, 62);
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, Color32::from_rgb(68, 72, 82));
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, Color32::from_rgb(222, 224, 230));
    v.widgets.hovered.weak_bg_fill = Color32::from_rgb(62, 67, 77);
    v.widgets.hovered.bg_fill = Color32::from_rgb(62, 67, 77);
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, accent.gamma_multiply(0.7));
    v.widgets.active.weak_bg_fill = Color32::from_rgb(74, 80, 92);
    v.widgets.active.bg_fill = Color32::from_rgb(74, 80, 92);
    v.widgets.active.bg_stroke = Stroke::new(1.0, accent);
    v.widgets.open.weak_bg_fill = Color32::from_rgb(56, 60, 70);
    v.selection.bg_fill = Color32::from_rgb(96, 46, 74);
    v.selection.stroke = Stroke::new(1.0, Color32::from_rgb(255, 214, 232));
    // Secondary text stays at 5.8:1 so hints and notes are quieter but still readable.
    v.weak_text_color = Some(Color32::from_rgb(152, 157, 168));
    // Links are frequent (every anime name on MyList), so they stay calm instead of taking the
    // accent colour.
    v.hyperlink_color = Color32::from_rgb(176, 186, 255);
    v.warn_fg_color = Color32::from_rgb(255, 210, 90);
    v.error_fg_color = Color32::from_rgb(255, 138, 128);
}

fn light(v: &mut Visuals) {
    let accent = Color32::from_rgb(176, 40, 108);
    v.panel_fill = Color32::from_rgb(247, 248, 250);
    v.window_fill = Color32::WHITE;
    v.faint_bg_color = Color32::from_rgb(240, 242, 246);
    v.extreme_bg_color = Color32::WHITE;
    v.code_bg_color = Color32::from_rgb(236, 238, 242);
    v.window_stroke = Stroke::new(1.0, Color32::from_rgb(206, 210, 218));
    v.window_shadow = Shadow {
        offset: [0, 6],
        blur: 20,
        spread: 0,
        color: Color32::from_black_alpha(40),
    };
    v.popup_shadow = Shadow {
        offset: [0, 4],
        blur: 12,
        spread: 0,
        color: Color32::from_black_alpha(32),
    };
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, Color32::from_rgb(220, 223, 229));
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, Color32::from_rgb(38, 42, 50));
    v.widgets.inactive.weak_bg_fill = Color32::from_rgb(232, 235, 240);
    v.widgets.inactive.bg_fill = Color32::from_rgb(232, 235, 240);
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, Color32::from_rgb(200, 205, 214));
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, Color32::from_rgb(28, 32, 40));
    v.widgets.hovered.weak_bg_fill = Color32::from_rgb(222, 226, 234);
    v.widgets.hovered.bg_fill = Color32::from_rgb(222, 226, 234);
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, accent.gamma_multiply(0.7));
    v.widgets.active.weak_bg_fill = Color32::from_rgb(208, 213, 224);
    v.widgets.active.bg_fill = Color32::from_rgb(208, 213, 224);
    v.widgets.active.bg_stroke = Stroke::new(1.0, accent);
    v.widgets.open.weak_bg_fill = Color32::from_rgb(226, 230, 237);
    v.selection.bg_fill = Color32::from_rgb(248, 214, 231);
    v.selection.stroke = Stroke::new(1.0, Color32::from_rgb(110, 20, 64));
    v.weak_text_color = Some(Color32::from_rgb(96, 102, 114));
    v.hyperlink_color = Color32::from_rgb(52, 70, 176);
    v.warn_fg_color = Color32::from_rgb(138, 90, 0);
    v.error_fg_color = Color32::from_rgb(176, 0, 32);
}

/// Brand accent: the cherry blossom of the icon. Never blue or green, which are status hues.
pub fn accent(visuals: &Visuals) -> Color32 {
    if visuals.dark_mode {
        Color32::from_rgb(232, 98, 156)
    } else {
        Color32::from_rgb(176, 40, 108)
    }
}

/// Text colour for something that needs attention but is not a failure.
pub fn warn(visuals: &Visuals) -> Color32 {
    visuals.warn_fg_color
}

/// Text colour for failures and destructive actions.
pub fn error(visuals: &Visuals) -> Color32 {
    visuals.error_fg_color
}

/// Text colour for success.
pub fn ok(visuals: &Visuals) -> Color32 {
    if visuals.dark_mode {
        Color32::from_rgb(129, 199, 132)
    } else {
        Color32::from_rgb(27, 110, 46)
    }
}

/// Background of the navigation sidebar and the status bar, a step away from the page.
pub fn chrome_fill(visuals: &Visuals) -> Color32 {
    if visuals.dark_mode {
        Color32::from_rgb(24, 26, 30)
    } else {
        Color32::from_rgb(236, 238, 242)
    }
}

/// A section heading inside a page or window, with the space that separates sections.
pub fn section(ui: &mut egui::Ui, text: impl Into<String>) {
    ui.add_space(10.0);
    ui.label(
        RichText::new(text)
            .text_style(section_style())
            .color(ui.visuals().strong_text_color()),
    );
    ui.add_space(2.0);
}

/// `n` with the singular or plural noun: `1 file`, `2 files`.
pub fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// Label of an action that destroys data: shown in the error colour, so it does not look like
/// its harmless neighbours.
pub fn destructive(ui: &egui::Ui, text: &str) -> RichText {
    RichText::new(text).color(error(ui.visuals()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use animatic_core::ui_state::contrast_ratio;

    fn hex(c: Color32) -> u32 {
        u32::from(c.r()) << 16 | u32::from(c.g()) << 8 | u32::from(c.b())
    }

    /// Every message colour and secondary text reaches WCAG AA (4.5:1) on the page, the window
    /// and the sidebar of its theme.
    #[test]
    fn message_colours_and_secondary_text_are_readable_in_both_themes() {
        for (name, setup) in [("dark", dark as fn(&mut Visuals)), ("light", light)] {
            let mut v = if name == "dark" {
                Visuals::dark()
            } else {
                Visuals::light()
            };
            setup(&mut v);
            let texts = [
                ("body", v.widgets.noninteractive.fg_stroke.color),
                ("weak", v.weak_text_color.unwrap()),
                ("link", v.hyperlink_color),
                ("warn", warn(&v)),
                ("error", error(&v)),
                ("ok", ok(&v)),
            ];
            for background in [v.panel_fill, v.window_fill, chrome_fill(&v)] {
                for (text, color) in texts {
                    let ratio = contrast_ratio(hex(color), hex(background));
                    assert!(ratio >= 4.5, "{name} {text}: {ratio:.2}");
                }
            }
        }
    }

    #[test]
    fn plural_picks_the_noun_by_count() {
        assert_eq!(plural(0, "file", "files"), "0 files");
        assert_eq!(plural(1, "file", "files"), "1 file");
        assert_eq!(plural(2, "command", "commands"), "2 commands");
    }
}
