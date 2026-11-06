//! Cover pictures in the anime detail window.
//!
//! The window asks the worker's cover thread for the picture of the anime it shows and draws
//! whatever has arrived; decoding and downloading never happen on the UI thread (rule 1). A
//! cover is requested when the detail window opens on an anime, once per picture name for as
//! long as the answer is kept, and never for anything that is not on screen.

use std::collections::HashMap;

use animatic_core::settings::AnidbSettings;
use eframe::egui::{self, ColorImage, RichText, TextureHandle, TextureOptions, Vec2};

use crate::worker::{CoverJob, CoverOutcome, Worker};

/// Largest size a cover is drawn at, in points.
const MAX_SIZE: Vec2 = Vec2::new(180.0, 260.0);
/// Size of the placeholder shown while there is no picture.
const PLACEHOLDER_SIZE: Vec2 = Vec2::new(180.0, 120.0);
/// Textures kept at once; pictures that are not on screen are dropped beyond this.
const KEPT: usize = 16;

enum Entry {
    /// Asked for; the worker has not answered yet.
    Loading,
    Ready {
        texture: TextureHandle,
        /// Something that went wrong around the picture (it could not be cached).
        note: Option<String>,
    },
    /// There is no picture to show, and why.
    Unavailable(String),
}

/// Cover state of the UI, keyed by picture name.
#[derive(Default)]
pub struct CoverUi {
    entries: HashMap<String, Entry>,
    /// Anime the detail window showed last frame. A different one, or none, makes failed
    /// lookups worth asking again (the user may have gone online or changed a setting).
    shown_aid: Option<i64>,
}

impl CoverUi {
    /// The worker answered a request.
    pub fn on_loaded(&mut self, ctx: &egui::Context, picname: String, outcome: CoverOutcome) {
        match outcome {
            CoverOutcome::Ready { image, note } => {
                let size = [image.width, image.height];
                let texture = ctx.load_texture(
                    format!("cover:{picname}"),
                    ColorImage::from_rgba_unmultiplied(size, &image.rgba),
                    TextureOptions::LINEAR,
                );
                self.entries.insert(picname, Entry::Ready { texture, note });
            }
            CoverOutcome::Unavailable(why) => {
                self.entries.insert(picname, Entry::Unavailable(why));
            }
            // Not sent because a newer request was waiting; the next frame asks again if the
            // cover is still on screen.
            CoverOutcome::Superseded => {
                self.entries.remove(&picname);
            }
        }
    }

    /// The detail window is closed.
    pub fn window_closed(&mut self) {
        self.shown_aid = None;
    }

    /// Draw the cover of anime `aid` (placeholder text when there is none), asking the worker
    /// for it the first time.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        worker: &Worker,
        settings: &AnidbSettings,
        aid: i64,
        picname: Option<&str>,
    ) {
        if self.shown_aid != Some(aid) {
            self.shown_aid = Some(aid);
            self.entries
                .retain(|_, e| !matches!(e, Entry::Unavailable(_)));
        }
        let Some(picname) = picname.filter(|p| !p.is_empty()) else {
            placeholder(ui, "No cover on record");
            return;
        };
        if !self.entries.contains_key(picname) {
            self.evict(picname);
            self.entries.insert(picname.to_string(), Entry::Loading);
            worker.load_cover(CoverJob {
                aid,
                picname: picname.to_string(),
                image_server: settings.image_server.clone(),
                download: settings.download_covers,
                offline: settings.offline,
            });
        }
        let mut retry = false;
        match &self.entries[picname] {
            Entry::Loading => {
                placeholder_with(ui, |ui| {
                    ui.spinner();
                    ui.label(RichText::new("Loading cover…").weak());
                });
            }
            Entry::Ready { texture, note } => {
                let size = fit(texture.size_vec2());
                ui.vertical(|ui| {
                    ui.add(
                        egui::Image::new(texture)
                            .fit_to_exact_size(size)
                            .corner_radius(4.0)
                            .alt_text("Anime cover"),
                    );
                    if let Some(note) = note {
                        ui.add(egui::Label::new(RichText::new(note).small().weak()).wrap());
                    }
                });
            }
            Entry::Unavailable(why) => {
                ui.vertical(|ui| {
                    placeholder(ui, why);
                    retry = ui
                        .small_button("Try again")
                        .on_hover_text("Asks for the cover again")
                        .clicked();
                });
            }
        }
        if retry {
            self.entries.remove(picname);
            ui.ctx().request_repaint();
        }
    }

    /// Make room for one more texture: drop covers other than `keep` beyond [`KEPT`].
    fn evict(&mut self, keep: &str) {
        while self.entries.len() >= KEPT {
            let Some(victim) = self
                .entries
                .iter()
                .find(|(name, entry)| name.as_str() != keep && !matches!(entry, Entry::Loading))
                .map(|(name, _)| name.clone())
            else {
                return;
            };
            self.entries.remove(&victim);
        }
    }
}

/// `size` scaled down (never up) to fit [`MAX_SIZE`], keeping the aspect ratio.
fn fit(size: Vec2) -> Vec2 {
    let scale = (MAX_SIZE.x / size.x.max(1.0))
        .min(MAX_SIZE.y / size.y.max(1.0))
        .min(1.0);
    size * scale
}

fn placeholder(ui: &mut egui::Ui, text: &str) {
    placeholder_with(ui, |ui| {
        ui.add(egui::Label::new(RichText::new(text).weak()).wrap());
    });
}

fn placeholder_with(ui: &mut egui::Ui, content: impl FnOnce(&mut egui::Ui)) {
    ui.group(|ui| {
        ui.set_width(PLACEHOLDER_SIZE.x);
        ui.set_min_height(PLACEHOLDER_SIZE.y);
        ui.vertical_centered(content);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn covers_are_scaled_down_to_the_window_but_never_up() {
        assert_eq!(fit(Vec2::new(90.0, 130.0)), Vec2::new(90.0, 130.0));
        let tall = fit(Vec2::new(1000.0, 2000.0));
        assert!(tall.y <= MAX_SIZE.y + 0.01 && tall.x <= MAX_SIZE.x);
        assert!((tall.x / tall.y - 0.5).abs() < 0.001);
        let wide = fit(Vec2::new(2000.0, 100.0));
        assert!((wide.x - MAX_SIZE.x).abs() < 0.01);
    }
}
