//! Options → Appearance → Customize status colors: edits `ui.colors`, the colors tables tint
//! their rows with, for the slots the tables use. The other (AOM compatibility) slots are
//! not listed; a "Reset all" leaves them alone.

use animatic_core::settings::{ColorSlot, RowStyle, TintMode, UiSettings};
use animatic_core::ui_state::{
    MIN_TEXT_CONTRAST, TABLE_SLOT_GROUPS, readable_text, row_colors, text_contrast_warning,
};
use eframe::egui::{self, Align, Layout, RichText};

use crate::table::tint_preview;

/// Width reserved for a slot's name, so the controls after it line up.
const LABEL_WIDTH: f32 = 150.0;
/// Width of the sample row.
const PREVIEW_WIDTH: f32 = 170.0;

/// What a slot is called in the editor and the text its sample row shows (the status as the
/// tables write it).
fn describe(slot: ColorSlot) -> (&'static str, &'static str) {
    match slot {
        ColorSlot::LocalFileInvalidCrc => ("CRC mismatch", "Mismatch"),
        ColorSlot::LocalFileUnknown => ("Not identified", "Not identified"),
        ColorSlot::LocalFileInMylist => ("In MyList", "In MyList · lid 123"),
        ColorSlot::LocalFile => ("Other files (plain)", "Identified, not in MyList"),
        ColorSlot::AnimeInMylist => ("In MyList", "3 eps / 5 files"),
        ColorSlot::Anime => ("Not in MyList (plain)", "Sample Anime"),
        ColorSlot::File => ("Not watched (plain)", "Sample file"),
        ColorSlot::Watched => ("Watched", "Watched · lid 123"),
        // Not listed in the editor.
        other => (other.label(), other.label()),
    }
}

fn unpack(rgb: u32) -> [u8; 3] {
    [(rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8]
}

fn pack(rgb: [u8; 3]) -> u32 {
    (u32::from(rgb[0]) << 16) | (u32::from(rgb[1]) << 8) | u32::from(rgb[2])
}

fn customised(ui: &UiSettings, slot: ColorSlot) -> bool {
    ui.colors.contains_key(&format!("{slot:?}"))
}

/// The collapsible editor. Edits go straight into `settings`, so the tables re-tint at once;
/// Save options persists them like any other setting.
pub(super) fn options_ui(ui: &mut egui::Ui, settings: &mut UiSettings) {
    egui::CollapsingHeader::new("Customize status colors")
        .id_salt("options-status-colors")
        .show(ui, |ui| {
            if !settings.colored_rows {
                ui.label(
                    RichText::new(
                        "Turn on \"Tint table rows by status\" to see these colors in the tables.",
                    )
                    .weak(),
                );
            }
            ui.add_enabled_ui(settings.colored_rows, |ui| {
                for (group, slots) in TABLE_SLOT_GROUPS {
                    ui.add_space(6.0);
                    ui.label(RichText::new(group).strong());
                    for &slot in slots {
                        slot_row(ui, settings, slot);
                    }
                }
                ui.add_space(8.0);
                let any = TABLE_SLOT_GROUPS
                    .iter()
                    .flat_map(|(_, slots)| slots.iter())
                    .any(|&slot| customised(settings, slot));
                if ui
                    .add_enabled(any, egui::Button::new("Reset all"))
                    .on_hover_text("Put every status color above back to its default")
                    .clicked()
                {
                    for (_, slots) in TABLE_SLOT_GROUPS {
                        for &slot in slots {
                            settings.reset_color(slot);
                        }
                    }
                }
            });
        });
}

fn slot_row(ui: &mut egui::Ui, settings: &mut UiSettings, slot: ColorSlot) {
    let (name, sample) = describe(slot);
    let mut color = settings.color(slot);
    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(LABEL_WIDTH, ui.spacing().interact_size.y),
            Layout::left_to_right(Align::Center),
            |ui| {
                ui.set_min_width(LABEL_WIDTH);
                ui.label(name);
            },
        );
        let mut background = unpack(color.background);
        if ui
            .color_edit_button_srgb(&mut background)
            .on_hover_text("Background (status) color")
            .changed()
        {
            color.background = pack(background);
        }
        let mut own_text = color.text.is_some();
        if ui
            .checkbox(&mut own_text, "Text")
            .on_hover_text(
                "Choose the text color too. A row with its own text color is always drawn \
                 solid. Without it the text is black or white, whichever reads better.",
            )
            .changed()
        {
            color.text = own_text.then(|| readable_text(color.background));
        }
        ui.add_enabled_ui(own_text, |ui| {
            let mut text = unpack(
                color
                    .text
                    .unwrap_or_else(|| readable_text(color.background)),
            );
            if ui
                .color_edit_button_srgb(&mut text)
                .on_hover_text("Text color")
                .changed()
            {
                color.text = Some(pack(text));
            }
        });
        let reset = ui
            .add_enabled(customised(settings, slot), egui::Button::new("Reset"))
            .on_hover_text("Put this status back to its default color")
            .clicked();
        if reset {
            settings.reset_color(slot);
            color = settings.color(slot);
        } else if color != settings.color(slot) {
            settings.set_color(slot, color);
        }
        // The sample is drawn by the table's own code for the row style chosen now.
        let mode = match settings.row_style {
            RowStyle::Soft => TintMode::Soft,
            RowStyle::Solid => TintMode::Solid,
        };
        tint_preview(
            ui,
            row_colors(mode, &settings.colors, slot),
            PREVIEW_WIDTH,
            sample,
        );
    });
    if let Some(ratio) = text_contrast_warning(color) {
        ui.colored_label(crate::style::warn(ui.visuals()), warning_text(name, ratio));
    }
}

fn warning_text(name: &str, ratio: f64) -> String {
    format!(
        "{name}: this text color has a contrast of only {ratio:.1}:1 on its background \
         ({MIN_TEXT_CONTRAST}:1 or more is easy to read). You can keep it."
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use animatic_core::settings::SlotColor;

    #[test]
    fn colors_pack_and_unpack_losslessly() {
        for rgb in [0x000000, 0xFFFFFF, 0x123456, 0xCB97FF] {
            assert_eq!(pack(unpack(rgb)), rgb);
        }
    }

    #[test]
    fn every_listed_slot_has_an_editor_name_and_sample() {
        for (_, slots) in TABLE_SLOT_GROUPS {
            for &slot in slots {
                let described = describe(slot);
                // The fallback arm answers with the slot's AOM label for both strings.
                assert_ne!(
                    described,
                    (slot.label(), slot.label()),
                    "{slot:?} needs an editor name and sample"
                );
            }
        }
    }

    #[test]
    fn customised_follows_the_stored_entries() {
        let mut ui = UiSettings::default();
        assert!(!customised(&ui, ColorSlot::Watched));
        ui.set_color(
            ColorSlot::Watched,
            SlotColor {
                background: 0x112233,
                text: None,
            },
        );
        assert!(customised(&ui, ColorSlot::Watched));
    }
}
