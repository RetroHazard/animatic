//! Player actions on Hashed files: the player chooser, "Open in player" and "Enqueue" (shown
//! only for a player that defines those argument templates) and the file context menu.

use animatic_core::external::{PlayerAction, chosen_player, player_label};
use eframe::egui;

use super::AnimaticApp;

/// What a file context-menu entry asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FileAction {
    /// The operating system's default program (not a configured player).
    OpenDefault,
    Player(PlayerAction),
}

/// The entries the context menu offers for the chosen player.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) struct FileMenu {
    /// Name of the player the player actions use, if one is configured.
    pub player: Option<String>,
    pub open_in_player: bool,
    pub enqueue: bool,
}

impl FileMenu {
    /// Entries as (label, action), in menu order. Play is offered whenever a player exists.
    pub fn entries(&self) -> Vec<(String, FileAction)> {
        let mut entries = vec![("Open".to_string(), FileAction::OpenDefault)];
        if let Some(player) = &self.player {
            entries.push((
                format!("Play with {player}"),
                FileAction::Player(PlayerAction::Play),
            ));
            if self.open_in_player {
                entries.push((
                    format!("Open in {player}"),
                    FileAction::Player(PlayerAction::Open),
                ));
            }
            if self.enqueue {
                entries.push((
                    format!("Enqueue in {player}"),
                    FileAction::Player(PlayerAction::Enqueue),
                ));
            }
        }
        entries
    }
}

/// The entries for `players` given the user's chooser state.
pub(super) fn file_menu(
    players: &[animatic_core::settings::Player],
    chosen: Option<usize>,
) -> FileMenu {
    let Some(index) = chosen_player(players, chosen) else {
        return FileMenu::default();
    };
    let player = &players[index];
    FileMenu {
        player: Some(player_label(players, index)),
        open_in_player: PlayerAction::Open.is_available(player),
        enqueue: PlayerAction::Enqueue.is_available(player),
    }
}

/// Draws the context menu entries and returns the one clicked.
pub(super) fn file_menu_ui(ui: &mut egui::Ui, menu: &FileMenu) -> Option<FileAction> {
    for (label, action) in menu.entries() {
        if ui.button(label).clicked() {
            ui.close();
            return Some(action);
        }
    }
    None
}

impl AnimaticApp {
    pub(super) fn file_menu(&self) -> FileMenu {
        file_menu(&self.settings.players, self.selected_player)
    }

    /// The chooser (with several players) and the Open/Enqueue buttons of the chosen player,
    /// drawn after the Play button of the Hashed files toolbar.
    pub(super) fn player_toolbar(&mut self, ui: &mut egui::Ui) {
        let Some(index) = chosen_player(&self.settings.players, self.selected_player) else {
            return;
        };
        let count = self.settings.players.len();
        if count > 1 {
            let labels: Vec<String> = (0..count)
                .map(|i| player_label(&self.settings.players, i))
                .collect();
            let mut pick = index;
            egui::ComboBox::from_id_salt("player_chooser")
                .selected_text(&labels[index])
                .show_ui(ui, |ui| {
                    for (i, label) in labels.iter().enumerate() {
                        ui.selectable_value(&mut pick, i, label);
                    }
                })
                .response
                .on_hover_text("The player used by Play, Open in player and Enqueue");
            if pick != index {
                self.selected_player = Some(pick);
            }
        }
        let selected = self.selected_local.is_some();
        for (action, text) in [
            (PlayerAction::Open, "Open in player"),
            (PlayerAction::Enqueue, "Enqueue"),
        ] {
            if action.is_available(&self.settings.players[index])
                && ui
                    .add_enabled(selected, egui::Button::new(text))
                    .on_hover_text(format!(
                        "Uses the {} arguments of {}",
                        action.label().to_ascii_lowercase(),
                        player_label(&self.settings.players, index)
                    ))
                    .clicked()
            {
                self.play_selected(action);
            }
        }
    }

    pub(super) fn run_file_action(&mut self, action: FileAction) {
        match action {
            FileAction::OpenDefault => {
                if let Some(file) = self
                    .projection
                    .local
                    .iter()
                    .find(|f| Some(f.id) == self.selected_local)
                {
                    let _ = open::that(&file.path);
                }
            }
            FileAction::Player(action) => self.play_selected(action),
        }
    }
}

#[cfg(test)]
mod tests {
    use animatic_core::settings::Player;

    use super::*;

    fn player(name: &str, open: &str, enqueue: &str) -> Player {
        Player {
            name: name.into(),
            path: "player".into(),
            open: open.into(),
            enqueue: enqueue.into(),
            ..Default::default()
        }
    }

    fn labels(menu: &FileMenu) -> Vec<String> {
        menu.entries().into_iter().map(|(label, _)| label).collect()
    }

    #[test]
    fn without_players_only_the_system_open_remains() {
        assert_eq!(labels(&file_menu(&[], None)), ["Open"]);
    }

    #[test]
    fn open_and_enqueue_appear_only_for_defined_templates() {
        let players = [player("Plain", "", ""), player("Rich", "--open", "--add")];
        assert_eq!(
            labels(&file_menu(&players, None)),
            ["Open", "Play with Plain"]
        );
        assert_eq!(
            labels(&file_menu(&players, Some(1))),
            ["Open", "Play with Rich", "Open in Rich", "Enqueue in Rich"]
        );
        let only_enqueue = [player("Q", "", "--add")];
        assert_eq!(
            labels(&file_menu(&only_enqueue, None)),
            ["Open", "Play with Q", "Enqueue in Q"]
        );
    }

    #[test]
    fn the_menu_follows_the_default_player_until_one_is_chosen() {
        let mut players = vec![player("First", "", ""), player("Second", "", "--add")];
        players[1].default = true;
        let menu = file_menu(&players, None);
        assert_eq!(menu.player.as_deref(), Some("Second"));
        assert!(menu.enqueue && !menu.open_in_player);
        let chosen = file_menu(&players, Some(0));
        assert_eq!(chosen.player.as_deref(), Some("First"));
        assert!(!chosen.enqueue);
    }

    #[test]
    fn actions_map_to_the_player_actions() {
        let players = [player("Rich", "--open", "--add")];
        let actions: Vec<_> = file_menu(&players, None)
            .entries()
            .into_iter()
            .map(|(_, action)| action)
            .collect();
        assert_eq!(
            actions,
            [
                FileAction::OpenDefault,
                FileAction::Player(PlayerAction::Play),
                FileAction::Player(PlayerAction::Open),
                FileAction::Player(PlayerAction::Enqueue),
            ]
        );
    }
}
