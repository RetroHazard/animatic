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

pub fn launch_player(player: &Player, action: PlayerAction, media: &Path) -> Result<Child> {
    if player.path.trim().is_empty() {
        return Err(Error::Other("player executable is not configured".into()));
    }
    let template = match action {
        PlayerAction::Open => &player.open,
        PlayerAction::Play => &player.play,
        PlayerAction::Enqueue => &player.enqueue,
    };
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
    let mut expanded = template.to_string();
    let mut had_path = false;
    for (needle, value) in replacements {
        if expanded.contains(needle) {
            if *needle == "{path}" || *needle == "%file%" {
                had_path = true;
            }
            expanded = expanded.replace(needle, value);
        }
    }
    let mut args = shell_words::split(&expanded)
        .map_err(|e| Error::Other(format!("invalid external command arguments: {e}")))?;
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
}
