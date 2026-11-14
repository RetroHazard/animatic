//! Animatic's look: one spacing and type scale for every theme, a dark and a light palette in
//! each colour scheme with readable secondary text, and the semantic colours (warning, error,
//! success, accent) that every page uses instead of fixed colours, so messages stay legible in
//! any theme and scheme.
//!
//! # Colour schemes
//!
//! A [`ColorScheme`] picks the accent family; the theme (dark, light, system) picks the
//! brightness, so every scheme has both palettes. A palette is a few choices on top of one shared
//! structure (the layering of panel, window, widget and stroke colours, the type scale and the
//! semantic warning, error and success colours):
//!
//! - the accent (what [`accent`] returns), the selection fill and its text, and the hyperlink
//!   colour;
//! - a small per-channel tint added to every neutral (backgrounds, strokes, text), so a scheme
//!   feels like a hue without garish backgrounds: at most 8 of 255 steps in any channel.
//!
//! The set: Sakura (the original cherry-blossom pink, untinted), Ocean (cool blue), Forest
//! (teal-leaning green, so it stays apart from the success green), Sunset (warm amber on a
//! cream or umber ground), Wisteria (violet) and Graphite (near-monochrome, for people who want
//! no colour). Links stay a calm blue; in the blue and violet schemes they move to a lighter or
//! darker blue so they never blend into the accent.
//!
//! The active scheme travels in the [`Visuals`] themselves (the accent is the stroke of an
//! active widget, the chrome is derived from the panel), so every helper keeps taking only
//! `&Visuals` and nothing is stored globally. Accessibility is checked, not eyeballed: the tests
//! compute WCAG 2.x contrast for every scheme in both variants.

use animatic_core::settings::ColorScheme;
use egui::{
    Color32, CornerRadius, FontFamily, FontId, Margin, RichText, Shadow, Stroke, TextStyle, Theme,
    Visuals, vec2,
};

/// Text style of section headings inside a page or window (between `Heading` and `Body`).
pub fn section_style() -> TextStyle {
    TextStyle::Name("Section".into())
}

/// Installs the spacing and type scale. Call once at startup, followed by [`set_scheme`]; the
/// theme choice itself (`ctx.set_theme`) stays separate, so `System` keeps following the OS.
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
}

/// Installs the dark and the light palette of `scheme`, replacing the colours of the previous
/// scheme at once (the spacing and type scale stay). The theme in use is not touched.
pub fn set_scheme(ctx: &egui::Context, scheme: ColorScheme) {
    ctx.style_mut_of(Theme::Dark, |style| {
        dark(&mut style.visuals, &palette(scheme, true));
    });
    ctx.style_mut_of(Theme::Light, |style| {
        light(&mut style.visuals, &palette(scheme, false));
    });
}

/// What distinguishes one scheme variant from another; see the module documentation.
struct Palette {
    accent: Color32,
    selection_fill: Color32,
    selection_text: Color32,
    hyperlink: Color32,
    /// Added to every neutral colour, per channel.
    tint: [i16; 3],
}

fn palette(scheme: ColorScheme, dark: bool) -> Palette {
    let rgb = |r, g, b| Color32::from_rgb(r, g, b);
    let (accent, selection_fill, selection_text, hyperlink, tint) = match (scheme, dark) {
        (ColorScheme::Sakura, true) => (
            rgb(232, 98, 156),
            rgb(96, 46, 74),
            rgb(255, 214, 232),
            rgb(176, 186, 255),
            [0, 0, 0],
        ),
        (ColorScheme::Sakura, false) => (
            rgb(176, 40, 108),
            rgb(248, 214, 231),
            rgb(110, 20, 64),
            rgb(52, 70, 176),
            [0, 0, 0],
        ),
        (ColorScheme::Ocean, true) => (
            rgb(79, 168, 232),
            rgb(30, 70, 110),
            rgb(214, 236, 255),
            rgb(160, 208, 255),
            [-3, 1, 5],
        ),
        (ColorScheme::Ocean, false) => (
            rgb(11, 99, 168),
            rgb(196, 222, 246),
            rgb(8, 52, 96),
            rgb(8, 66, 140),
            [-5, -1, 0],
        ),
        (ColorScheme::Forest, true) => (
            rgb(64, 196, 150),
            rgb(24, 84, 68),
            rgb(205, 247, 232),
            rgb(176, 186, 255),
            [-3, 3, 0],
        ),
        (ColorScheme::Forest, false) => (
            rgb(12, 120, 96),
            rgb(190, 234, 216),
            rgb(6, 70, 54),
            rgb(52, 70, 176),
            [-5, 0, -3],
        ),
        (ColorScheme::Sunset, true) => (
            rgb(240, 149, 74),
            rgb(110, 64, 24),
            rgb(255, 229, 200),
            rgb(150, 200, 255),
            [5, 1, -5],
        ),
        (ColorScheme::Sunset, false) => (
            rgb(169, 74, 8),
            rgb(248, 214, 180),
            rgb(100, 44, 0),
            rgb(40, 70, 170),
            [0, -3, -8],
        ),
        (ColorScheme::Wisteria, true) => (
            rgb(176, 140, 255),
            rgb(70, 50, 120),
            rgb(232, 222, 255),
            rgb(140, 200, 255),
            [3, -1, 6],
        ),
        (ColorScheme::Wisteria, false) => (
            rgb(106, 63, 192),
            rgb(226, 216, 250),
            rgb(60, 30, 120),
            rgb(10, 86, 160),
            [-3, -5, 0],
        ),
        (ColorScheme::Graphite, true) => (
            rgb(200, 205, 214),
            rgb(66, 72, 84),
            rgb(236, 239, 245),
            rgb(150, 190, 255),
            [4, 3, -3],
        ),
        (ColorScheme::Graphite, false) => (
            rgb(58, 66, 80),
            rgb(214, 219, 228),
            rgb(30, 36, 48),
            rgb(30, 70, 160),
            [1, 0, -2],
        ),
    };
    Palette {
        accent,
        selection_fill,
        selection_text,
        hyperlink,
        tint,
    }
}

/// A neutral colour of the structure, shifted by the scheme's tint.
fn neutral(base: [u8; 3], tint: [i16; 3]) -> Color32 {
    let channel = |i: usize| (i16::from(base[i]) + tint[i]).clamp(0, 255) as u8;
    Color32::from_rgb(channel(0), channel(1), channel(2))
}

fn dark(v: &mut Visuals, p: &Palette) {
    let n = |r, g, b| neutral([r, g, b], p.tint);
    let accent = p.accent;
    v.panel_fill = n(30, 32, 37);
    v.window_fill = n(37, 40, 46);
    v.faint_bg_color = n(36, 38, 44);
    v.extreme_bg_color = n(20, 22, 26);
    v.code_bg_color = n(44, 47, 54);
    v.window_stroke = Stroke::new(1.0, n(62, 66, 74));
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
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, n(52, 56, 64));
    // Body text and headings: 9:1 on the panel (the egui default is 5:1).
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, n(200, 203, 210));
    v.widgets.inactive.weak_bg_fill = n(50, 54, 62);
    v.widgets.inactive.bg_fill = n(50, 54, 62);
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, n(68, 72, 82));
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, n(222, 224, 230));
    v.widgets.hovered.weak_bg_fill = n(62, 67, 77);
    v.widgets.hovered.bg_fill = n(62, 67, 77);
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, accent.gamma_multiply(0.7));
    v.widgets.active.weak_bg_fill = n(74, 80, 92);
    v.widgets.active.bg_fill = n(74, 80, 92);
    // The accent of this scheme: [`accent`] reads it back from here.
    v.widgets.active.bg_stroke = Stroke::new(1.0, accent);
    v.widgets.open.weak_bg_fill = n(56, 60, 70);
    v.selection.bg_fill = p.selection_fill;
    v.selection.stroke = Stroke::new(1.0, p.selection_text);
    // Secondary text stays at 5.8:1 so hints and notes are quieter but still readable.
    v.weak_text_color = Some(n(152, 157, 168));
    // Links are frequent (every anime name on MyList), so they stay calm instead of taking the
    // accent colour.
    v.hyperlink_color = p.hyperlink;
    v.warn_fg_color = Color32::from_rgb(255, 210, 90);
    v.error_fg_color = Color32::from_rgb(255, 138, 128);
}

fn light(v: &mut Visuals, p: &Palette) {
    let n = |r, g, b| neutral([r, g, b], p.tint);
    let accent = p.accent;
    v.panel_fill = n(247, 248, 250);
    v.window_fill = n(255, 255, 255);
    v.faint_bg_color = n(240, 242, 246);
    v.extreme_bg_color = n(255, 255, 255);
    v.code_bg_color = n(236, 238, 242);
    v.window_stroke = Stroke::new(1.0, n(206, 210, 218));
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
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, n(220, 223, 229));
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, n(38, 42, 50));
    v.widgets.inactive.weak_bg_fill = n(232, 235, 240);
    v.widgets.inactive.bg_fill = n(232, 235, 240);
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, n(200, 205, 214));
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, n(28, 32, 40));
    v.widgets.hovered.weak_bg_fill = n(222, 226, 234);
    v.widgets.hovered.bg_fill = n(222, 226, 234);
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, accent.gamma_multiply(0.7));
    v.widgets.active.weak_bg_fill = n(208, 213, 224);
    v.widgets.active.bg_fill = n(208, 213, 224);
    // The accent of this scheme: [`accent`] reads it back from here.
    v.widgets.active.bg_stroke = Stroke::new(1.0, accent);
    v.widgets.open.weak_bg_fill = n(226, 230, 237);
    v.selection.bg_fill = p.selection_fill;
    v.selection.stroke = Stroke::new(1.0, p.selection_text);
    v.weak_text_color = Some(n(96, 102, 114));
    v.hyperlink_color = p.hyperlink;
    v.warn_fg_color = Color32::from_rgb(138, 90, 0);
    v.error_fg_color = Color32::from_rgb(176, 0, 32);
}

/// Accent of the active colour scheme (Sakura's is the cherry blossom of the icon). Schemes
/// whose accent is a status hue (Forest, Sunset) keep it apart from the success and warning
/// colours, and every status message is also written out, so colour is never the only cue.
pub fn accent(visuals: &Visuals) -> Color32 {
    visuals.widgets.active.bg_stroke.color
}

/// The accent `scheme` has in the dark or the light theme, for previews (the colour scheme
/// choice in Options) that show a scheme other than the active one.
pub fn scheme_accent(scheme: ColorScheme, dark_mode: bool) -> Color32 {
    palette(scheme, dark_mode).accent
}

/// A small rounded square filled with `color`, outlined in the text colour so it shows on any
/// background.
pub fn swatch(ui: &mut egui::Ui, color: Color32) {
    let side = 14.0;
    let (rect, _) = ui.allocate_exact_size(vec2(side, side), egui::Sense::hover());
    ui.painter().rect(
        rect,
        CornerRadius::same(4),
        color,
        Stroke::new(1.0, ui.visuals().widgets.noninteractive.fg_stroke.color),
        egui::StrokeKind::Inside,
    );
}

/// Text colour on a surface filled with the accent (the primary buttons): the dark theme's
/// darkest background, which its light accents need, or white on the darker accents of the
/// light theme. Contrast is at least 4.5:1 in every scheme.
pub fn on_accent(visuals: &Visuals) -> Color32 {
    if visuals.dark_mode {
        visuals.extreme_bg_color
    } else {
        Color32::WHITE
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

/// Background of the navigation sidebar and the status bar, a step away from the page: the
/// panel fill, darker, so it carries the scheme's tint.
pub fn chrome_fill(visuals: &Visuals) -> Color32 {
    let step = if visuals.dark_mode {
        [6, 6, 7]
    } else {
        [11, 10, 8]
    };
    let panel = visuals.panel_fill;
    Color32::from_rgb(
        panel.r().saturating_sub(step[0]),
        panel.g().saturating_sub(step[1]),
        panel.b().saturating_sub(step[2]),
    )
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

    fn ratio(a: Color32, b: Color32) -> f64 {
        contrast_ratio(hex(a), hex(b))
    }

    /// The visuals of one scheme variant, as `set_scheme` installs them over egui's defaults.
    fn visuals(scheme: ColorScheme, dark_mode: bool) -> Visuals {
        let mut v = if dark_mode {
            Visuals::dark()
        } else {
            Visuals::light()
        };
        let p = palette(scheme, dark_mode);
        if dark_mode {
            dark(&mut v, &p);
        } else {
            light(&mut v, &p);
        }
        v
    }

    fn variants() -> Vec<(String, ColorScheme, Visuals)> {
        ColorScheme::ALL
            .into_iter()
            .flat_map(|scheme| {
                [true, false].map(|dark_mode| {
                    let name = format!(
                        "{} {}",
                        scheme.label(),
                        if dark_mode { "dark" } else { "light" }
                    );
                    (name, scheme, visuals(scheme, dark_mode))
                })
            })
            .collect()
    }

    /// Every message colour and secondary text reaches WCAG AA (4.5:1) on the page, the window
    /// and the sidebar of every scheme and theme.
    #[test]
    fn message_colours_and_secondary_text_are_readable_in_every_scheme() {
        for (name, _, v) in variants() {
            let texts = [
                ("weak", v.weak_text_color.unwrap()),
                ("link", v.hyperlink_color),
                ("warn", warn(&v)),
                ("error", error(&v)),
                ("ok", ok(&v)),
                ("accent", accent(&v)),
            ];
            for background in [v.panel_fill, v.window_fill, chrome_fill(&v)] {
                for (text, color) in texts {
                    let r = ratio(color, background);
                    assert!(r >= 4.5, "{name} {text}: {r:.2}");
                }
            }
        }
    }

    /// Body and strong text reach AAA (7:1) on the page, the window and the sidebar.
    #[test]
    fn body_and_strong_text_reach_aaa_in_every_scheme() {
        for (name, _, v) in variants() {
            let texts = [
                ("body", v.widgets.noninteractive.fg_stroke.color),
                ("strong", v.strong_text_color()),
            ];
            for background in [v.panel_fill, v.window_fill, chrome_fill(&v)] {
                for (text, color) in texts {
                    let r = ratio(color, background);
                    assert!(r >= 7.0, "{name} {text}: {r:.2}");
                }
            }
        }
    }

    /// Selected text (the selection stroke) is readable on the selection fill, the fill can be
    /// told from the page, and the accent is visible on the page as a non-text element.
    #[test]
    fn selection_and_accent_are_visible_in_every_scheme() {
        for (name, _, v) in variants() {
            let r = ratio(v.selection.stroke.color, v.selection.bg_fill);
            assert!(r >= 4.5, "{name} selection text: {r:.2}");
            let r = ratio(v.selection.bg_fill, v.panel_fill);
            assert!(r >= 1.15, "{name} selection fill against the page: {r:.2}");
            for background in [v.panel_fill, v.window_fill, chrome_fill(&v)] {
                let r = ratio(accent(&v), background);
                assert!(r >= 3.0, "{name} accent as an element: {r:.2}");
            }
        }
    }

    /// The label of a primary button (accent fill) is readable in every scheme.
    #[test]
    fn text_on_the_accent_fill_is_readable_in_every_scheme() {
        for (name, _, v) in variants() {
            let r = ratio(on_accent(&v), accent(&v));
            assert!(r >= 4.5, "{name} text on the accent: {r:.2}");
        }
    }

    /// Button labels are readable on the fill of their own widget state.
    #[test]
    fn widget_text_is_readable_on_widget_fills_in_every_scheme() {
        for (name, _, v) in variants() {
            for (state, widget) in [
                ("inactive", &v.widgets.inactive),
                ("hovered", &v.widgets.hovered),
                ("active", &v.widgets.active),
            ] {
                let r = ratio(widget.fg_stroke.color, widget.bg_fill);
                assert!(r >= 4.5, "{name} {state} widget text: {r:.2}");
            }
        }
    }

    /// The accent helper returns the scheme's own accent, and the default scheme keeps the
    /// original pink.
    #[test]
    fn accent_helper_follows_the_scheme_and_sakura_keeps_the_pink() {
        for (name, scheme, v) in variants() {
            assert_eq!(
                accent(&v),
                palette(scheme, v.dark_mode).accent,
                "{name} accent"
            );
        }
        assert_eq!(
            accent(&visuals(ColorScheme::Sakura, true)),
            Color32::from_rgb(232, 98, 156)
        );
        assert_eq!(
            accent(&visuals(ColorScheme::Sakura, false)),
            Color32::from_rgb(176, 40, 108)
        );
        let sakura = visuals(ColorScheme::Sakura, true);
        assert_eq!(sakura.panel_fill, Color32::from_rgb(30, 32, 37));
        assert_eq!(chrome_fill(&sakura), Color32::from_rgb(24, 26, 30));
        let sakura = visuals(ColorScheme::Sakura, false);
        assert_eq!(sakura.panel_fill, Color32::from_rgb(247, 248, 250));
        assert_eq!(chrome_fill(&sakura), Color32::from_rgb(236, 238, 242));
    }

    #[test]
    fn every_scheme_is_distinct_and_its_tint_stays_subtle() {
        let accents: Vec<_> = ColorScheme::ALL
            .iter()
            .map(|s| palette(*s, true).accent)
            .collect();
        for (i, a) in accents.iter().enumerate() {
            assert!(!accents[..i].contains(a), "duplicate accent {a:?}");
        }
        for scheme in ColorScheme::ALL {
            for dark_mode in [true, false] {
                let p = palette(scheme, dark_mode);
                assert!(p.tint.iter().all(|t| t.abs() <= 8), "{scheme:?} tint");
            }
        }
    }

    /// Installing a scheme replaces the colours of the previous one in both themes.
    #[test]
    fn set_scheme_switches_both_themes() {
        let ctx = egui::Context::default();
        install(&ctx);
        for scheme in ColorScheme::ALL {
            set_scheme(&ctx, scheme);
            for (theme, dark_mode) in [(Theme::Dark, true), (Theme::Light, false)] {
                let v = ctx.style_of(theme).visuals.clone();
                assert_eq!(accent(&v), palette(scheme, dark_mode).accent, "{scheme:?}");
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
