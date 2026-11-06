//! External player and AVDump process launching without invoking a shell.

use std::path::Path;
use std::process::{Child, Command};

use crate::error::{Error, Result};
use crate::settings::{AvdumpSettings, Player};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayerAction {
    Open,
    Play,
    Enqueue,
}

impl PlayerAction {
    /// Verb used for the action's button and in error messages.
    pub fn label(self) -> &'static str {
        match self {
            PlayerAction::Open => "Open",
            PlayerAction::Play => "Play",
            PlayerAction::Enqueue => "Enqueue",
        }
    }

    /// The player's argument template for this action.
    pub fn template(self, player: &Player) -> &str {
        match self {
            PlayerAction::Open => &player.open,
            PlayerAction::Play => &player.play,
            PlayerAction::Enqueue => &player.enqueue,
        }
    }

    /// Whether `player` can perform the action. Play is always offered (an empty template
    /// just passes the file); Open and Enqueue exist only for players that define them.
    pub fn is_available(self, player: &Player) -> bool {
        match self {
            PlayerAction::Play => true,
            PlayerAction::Open | PlayerAction::Enqueue => !self.template(player).trim().is_empty(),
        }
    }
}

/// Index of the player Play uses: the first marked default, else the first configured.
pub fn default_player(players: &[Player]) -> Option<usize> {
    players
        .iter()
        .position(|p| p.default)
        .or(if players.is_empty() { None } else { Some(0) })
}

/// The player a file action runs with: the user's choice while it still names a configured
/// player, otherwise the default player.
pub fn chosen_player(players: &[Player], chosen: Option<usize>) -> Option<usize> {
    chosen
        .filter(|&index| index < players.len())
        .or_else(|| default_player(players))
}

/// Name shown in the player chooser: the configured name, or "Player N" for an unnamed one.
pub fn player_label(players: &[Player], index: usize) -> String {
    match players.get(index).map(|p| p.name.trim()) {
        Some(name) if !name.is_empty() => name.to_string(),
        _ => format!("Player {}", index + 1),
    }
}

/// Marks `index` as the only default player, or clears every default when `on` is false (the
/// first player is then the default again).
pub fn set_default_player(players: &mut [Player], index: usize, on: bool) {
    for (i, player) in players.iter_mut().enumerate() {
        player.default = on && i == index;
    }
}

pub fn launch_player(player: &Player, action: PlayerAction, media: &Path) -> Result<Child> {
    if player.path.trim().is_empty() {
        return Err(Error::Other("player executable is not configured".into()));
    }
    if !action.is_available(player) {
        return Err(Error::Other(format!(
            "this player has no {} arguments configured",
            action.label().to_ascii_lowercase()
        )));
    }
    let template = action.template(player);
    let path = media.to_string_lossy().into_owned();
    launch(
        &player.path,
        template,
        &[("{path}", &path), ("%file%", &path)],
        true,
    )
}

pub fn launch_avdump(
    settings: &AvdumpSettings,
    media: &Path,
    username: &str,
    port: u16,
) -> Result<Child> {
    if !settings.enabled {
        return Err(Error::Other("AVDump integration is disabled".into()));
    }
    let path = media.to_string_lossy().into_owned();
    let port = port.to_string();
    launch(
        &settings.executable,
        &settings.arguments,
        &[
            ("{path}", &path),
            ("{user}", username),
            ("{key}", &settings.key),
            ("{port}", &port),
        ],
        false,
    )
}

fn launch(
    executable: &str,
    template: &str,
    replacements: &[(&str, &str)],
    append_path: bool,
) -> Result<Child> {
    if executable.trim().is_empty() {
        return Err(Error::Other("external executable is not configured".into()));
    }
    let (mut args, had_path) = expand_arguments(template, replacements)?;
    if append_path
        && !had_path
        && let Some((_, path)) = replacements.first()
    {
        args.push((*path).to_string());
    }
    Command::new(executable)
        .args(args)
        .spawn()
        .map_err(Error::from)
}

/// Split `template` into arguments with POSIX-style quoting rules *first*, then substitute the
/// placeholders inside each argument. A substituted value therefore always stays inside the
/// argument that contained its placeholder, whatever spaces, quotes or option-like text it holds,
/// and is never scanned for further placeholders. Returns the arguments and whether a path
/// placeholder (`{path}` or `%file%`) was used.
fn expand_arguments(template: &str, replacements: &[(&str, &str)]) -> Result<(Vec<String>, bool)> {
    let words = shell_words::split(template)
        .map_err(|e| Error::Other(format!("invalid external command arguments: {e}")))?;
    let mut had_path = false;
    let args = words
        .into_iter()
        .map(|word| {
            let mut out = String::with_capacity(word.len());
            let mut rest = word.as_str();
            'scan: while !rest.is_empty() {
                for (needle, value) in replacements {
                    if let Some(tail) = rest.strip_prefix(needle) {
                        had_path |= *needle == "{path}" || *needle == "%file%";
                        out.push_str(value);
                        rest = tail;
                        continue 'scan;
                    }
                }
                let ch = rest.chars().next().expect("rest is not empty");
                out.push(ch);
                rest = &rest[ch.len_utf8()..];
            }
            out
        })
        .collect();
    Ok((args, had_path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_avdump_is_not_spawned() {
        let err =
            launch_avdump(&AvdumpSettings::default(), Path::new("x.mkv"), "user", 1).unwrap_err();
        assert!(err.to_string().contains("disabled"));
    }

    #[test]
    fn malformed_arguments_are_rejected_before_spawn() {
        let p = Player {
            path: "none".into(),
            play: "\"unterminated".into(),
            ..Default::default()
        };
        let err = launch_player(&p, PlayerAction::Play, Path::new("x.mkv")).unwrap_err();
        assert!(err.to_string().contains("invalid external command"));
    }

    const HOSTILE: &str = r#"C:\Media Root\it's "quoted" --fullscreen {user}.mkv"#;

    fn player_args(template: &str, path: &str) -> (Vec<String>, bool) {
        expand_arguments(template, &[("{path}", path), ("%file%", path)]).unwrap()
    }

    #[test]
    fn substituted_values_stay_one_argument() {
        for template in ["{path}", "\"{path}\"", "'{path}'", "%file%", "\"%file%\""] {
            assert_eq!(
                player_args(template, HOSTILE),
                (vec![HOSTILE.to_string()], true),
                "{template}"
            );
        }
        let (args, had_path) = player_args("--start 0 /play \"%file%\" --close", HOSTILE);
        assert!(had_path);
        assert_eq!(args, ["--start", "0", "/play", HOSTILE, "--close"]);
    }

    #[test]
    fn values_are_not_rescanned_for_placeholders() {
        let (args, _) = expand_arguments(
            "-u {user} -k {key} {path}",
            &[
                ("{path}", "{key}.mkv"),
                ("{user}", "a b {path}"),
                ("{key}", "--x \"y\""),
            ],
        )
        .unwrap();
        assert_eq!(args, ["-u", "a b {path}", "-k", "--x \"y\"", "{key}.mkv"]);
    }

    #[test]
    fn placeholders_inside_a_word_keep_the_surrounding_text() {
        let (args, had_path) = player_args("--file={path} --title=\"x %file% y\"", "a b.mkv");
        assert!(had_path);
        assert_eq!(args, ["--file=a b.mkv", "--title=x a b.mkv y"]);
    }

    #[test]
    fn a_template_without_a_path_placeholder_reports_it() {
        assert_eq!(
            player_args("--fullscreen", HOSTILE),
            (vec!["--fullscreen".to_string()], false)
        );
    }

    fn named(name: &str) -> Player {
        Player {
            name: name.into(),
            path: "player".into(),
            ..Default::default()
        }
    }

    #[test]
    fn open_and_enqueue_are_offered_only_when_the_template_is_set() {
        let mut p = named("A");
        assert!(PlayerAction::Play.is_available(&p));
        assert!(!PlayerAction::Open.is_available(&p));
        assert!(!PlayerAction::Enqueue.is_available(&p));
        p.open = "  ".into();
        assert!(!PlayerAction::Open.is_available(&p), "blank is not defined");
        p.open = "/open {path}".into();
        p.enqueue = "--enqueue".into();
        assert!(PlayerAction::Open.is_available(&p));
        assert!(PlayerAction::Enqueue.is_available(&p));
        assert_eq!(PlayerAction::Enqueue.template(&p), "--enqueue");
    }

    #[test]
    fn an_undefined_action_is_refused_before_spawn() {
        let err =
            launch_player(&named("A"), PlayerAction::Enqueue, Path::new("x.mkv")).unwrap_err();
        assert!(err.to_string().contains("no enqueue arguments"), "{err}");
    }

    #[test]
    fn the_chooser_defaults_to_the_default_player() {
        assert_eq!(default_player(&[]), None);
        let mut players = vec![named("A"), named("B"), named("C")];
        assert_eq!(
            default_player(&players),
            Some(0),
            "first when none is marked"
        );
        players[1].default = true;
        assert_eq!(default_player(&players), Some(1));
        assert_eq!(chosen_player(&players, None), Some(1));
        assert_eq!(chosen_player(&players, Some(2)), Some(2), "a choice wins");
        assert_eq!(
            chosen_player(&players, Some(7)),
            Some(1),
            "a choice that no longer exists falls back to the default"
        );
        assert_eq!(chosen_player(&[], Some(0)), None);
    }

    #[test]
    fn chooser_labels_use_the_name_and_number_unnamed_players() {
        let players = vec![named(" VLC "), named("")];
        assert_eq!(player_label(&players, 0), "VLC");
        assert_eq!(player_label(&players, 1), "Player 2");
    }

    #[test]
    fn marking_a_default_clears_the_others() {
        let mut players = vec![named("A"), named("B")];
        players[0].default = true;
        set_default_player(&mut players, 1, true);
        assert_eq!(
            players.iter().map(|p| p.default).collect::<Vec<_>>(),
            [false, true]
        );
        set_default_player(&mut players, 1, false);
        assert!(players.iter().all(|p| !p.default));
        assert_eq!(default_player(&players), Some(0));
    }
}
