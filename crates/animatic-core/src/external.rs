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
}
