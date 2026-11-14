//! Built-in naming conventions and the synthetic sample file their examples are computed from.
//!
//! A preset is only a recognizable pattern text: the setting stores the text, and a text equal
//! to a preset (line endings and surrounding blanks ignored) is that preset. A custom pattern
//! is therefore never rewritten, and an older settings file needs no migration.

use std::path::Path;

use super::eval::DataSource;
use super::plan::{self, Plan};
use super::{Bindings, Object, Pattern, Value, evaluate};
use crate::settings::{AOM_RENAME_PATTERN, DEFAULT_RENAME_PATTERN};

/// One built-in naming convention.
#[derive(Debug, PartialEq, Eq)]
pub struct Preset {
    /// Stable identifier (never shown).
    pub id: &'static str,
    /// Name in the convention list.
    pub name: &'static str,
    /// One sentence on when to choose it.
    pub description: &'static str,
    /// Pattern text.
    pub source: &'static str,
}

/// The built-in conventions, in list order. The first is the default for new settings.
pub const PRESETS: &[Preset] = &[
    Preset {
        id: "fansub",
        name: "[Group] Title - episode [CRC]",
        description: "The default: the usual fansub release style; keeps the group, version and \
                      checksum.",
        source: DEFAULT_RENAME_PATTERN,
    },
    Preset {
        id: "aom",
        name: "Title - episode - episode title [group](CRC)",
        description: "AniDB O'Matic's default naming.",
        source: AOM_RENAME_PATTERN,
    },
    Preset {
        id: "episode-title",
        name: "Title - episode - episode title",
        description: "Short and readable, without group or checksum.",
        source: include_str!("episode_title_pattern.txt"),
    },
    Preset {
        id: "media-server",
        name: "Title - S01E06 - episode title (media servers)",
        description: "Season and episode numbers for Plex, Jellyfin and Kodi; specials are season \
                      0, other episode types keep their AniDB prefix (C, T, P, O).",
        source: include_str!("media_server_pattern.txt"),
    },
];

/// The preset whose text `pattern` is, if any.
pub fn preset_for(pattern: &str) -> Option<&'static Preset> {
    let text = pattern.trim();
    PRESETS
        .iter()
        .find(|p| text.lines().eq(p.source.trim().lines()))
}

/// Episode numbers of the sample shown in examples.
pub const SAMPLE_EPISODE: i64 = 6;

/// Titles of the synthetic sample.
pub(super) struct SampleSource;

impl DataSource for SampleSource {
    fn anime(&self, _: i64) -> Option<Object> {
        None
    }
    fn episode(&self, _: i64) -> Option<Object> {
        None
    }
    fn language(&self, lang: &str) -> Option<String> {
        Some(lang.to_ascii_lowercase())
    }
    fn anime_titles(&self, _: i64) -> Vec<(i64, String, String)> {
        vec![
            (1, "x-jat".into(), "Reibun no Tabi".into()),
            (4, "en".into(), "Example Journey".into()),
            (4, "ja".into(), "例文の旅".into()),
        ]
    }
    fn episode_titles(&self, _: i64) -> Vec<(String, String)> {
        vec![
            ("en".into(), "The Long Road".into()),
            ("x-jat".into(), "Nagai Michi".into()),
            ("ja".into(), "長い道".into()),
        ]
    }
}

/// A fictitious, fully identified file: regular episode 6, version 2, released by a group.
fn sample_bindings() -> Bindings {
    sample_with(1, SAMPLE_EPISODE, 2, true)
}

/// The sample file as episode `epno` of type `type_id`, release `version`, with or without a
/// release group.
pub(super) fn sample_with(type_id: i64, epno: i64, version: i64, grouped: bool) -> Bindings {
    Bindings {
        anime: Some(
            Object::new("TAniDBAnime")
                .with("Id", Value::Int(1))
                .with("Name", Value::WStr("Reibun no Tabi".into()))
                .with("Year", Value::AStr("2024".into()))
                .with("StartYear", Value::Int(2024))
                .with("CategoryStr", Value::WStr("TV Series".into()))
                .with("TotalEpisodeCount", Value::Int(12)),
        ),
        episode: Some(
            Object::new("TAniDBEpisode")
                .with("Id", Value::Int(2))
                .with("Name", Value::WStr("The Long Road".into()))
                .with("EpisodeNo", Value::Int(epno))
                .with("TypeId", Value::Int(type_id))
                .with("IsSpecial", Value::Bool(type_id != 1))
                .with("EpisodeStr", Value::WStr(episode_str(type_id, epno))),
        ),
        file: Some(
            Object::new("TAniDBFile")
                .with("Id", Value::Int(3))
                .with("Version", Value::Int(version))
                .with("Crc", Value::AStr("eb420c97".into()))
                .with("FileType", Value::AStr("mkv".into()))
                .with("ResolutionString", Value::WStr("1920x1080".into()))
                .with("ResolutionHeight", Value::Int(1080))
                .with("Quality", Value::WStr("high".into()))
                .with("Source", Value::WStr("Blu-ray".into())),
        ),
        group: grouped.then(|| {
            Object::new("TAniDBGroup")
                .with("Id", Value::Int(4))
                .with("Shortname", Value::WStr("ExGroup".into()))
                .with("Name", Value::WStr("Example Group".into()))
        }),
        hash: Some(Object::new("TAniDBLocalHash").with("Crc32", Value::AStr("EB420C97".into()))),
        mylist: None,
        path: String::new(),
        base_path: String::new(),
    }
}

/// `E.EpisodeStr`: the type letter and the unpadded number.
fn episode_str(type_id: i64, epno: i64) -> String {
    let prefix = match type_id {
        2 => "S",
        3 => "C",
        4 => "T",
        5 => "P",
        6 => "O",
        _ => "",
    };
    format!("{prefix}{epno}")
}

/// What the whole pattern `source` would name the sample file, with every check a real
/// rename applies (sanitizing, path rules, warnings).
pub fn sample_plan(source: &str) -> Plan {
    plan::plan(
        source,
        &sample_bindings(),
        &SampleSource,
        Path::new("sample-download.mkv"),
    )
}

/// The value of one expression (or several lines) for the sample file, as text. Truth values
/// are shown as `True`/`False`. Warnings and errors are an `Err` with the reason.
pub fn sample_value(source: &str) -> Result<String, String> {
    let pattern = Pattern::compile(source).map_err(|e| e.message)?;
    let evaluation =
        evaluate(&pattern, &sample_bindings(), &SampleSource).map_err(|e| e.message)?;
    match evaluation.warnings.first() {
        Some(warning) => Err(warning.clone()),
        None => Ok(evaluation.result),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(preset: &str) -> String {
        let preset = PRESETS.iter().find(|p| p.id == preset).unwrap();
        let plan = sample_plan(preset.source);
        assert!(plan.can_apply(), "{plan:?}");
        plan.result
    }

    #[test]
    fn every_preset_names_the_sample_as_its_convention_promises() {
        assert_eq!(
            name("aom"),
            "Reibun no Tabi - 06v2 - The Long Road [ExGroup](EB420C97).mkv"
        );
        assert_eq!(
            name("fansub"),
            "[ExGroup] Reibun no Tabi - 06v2 [EB420C97].mkv"
        );
        assert_eq!(
            name("episode-title"),
            "Reibun no Tabi - 06 - The Long Road.mkv"
        );
        assert_eq!(
            name("media-server"),
            "Reibun no Tabi - S01E06 - The Long Road.mkv"
        );
    }

    #[test]
    fn specials_versions_and_missing_groups_follow_each_convention() {
        let run = |preset: &str, type_id: i64, epno: i64, version: i64, grouped: bool| {
            let b = sample_with(type_id, epno, version, grouped);
            let source = PRESETS.iter().find(|p| p.id == preset).unwrap().source;
            let plan = plan::plan(source, &b, &SampleSource, Path::new("x.mkv"));
            assert!(plan.can_apply(), "{plan:?}");
            plan.result
        };
        // Every AniDB episode type, with and without a later release version, in the
        // `Title - S15 [CRC]` / `C05` / `T08v2` style that AniDB O'Matic users match in media
        // players (for example a Kodi regular expression).
        for (type_id, epno, version, expected) in [
            (1, 5, 1, "05"),
            (1, 10, 2, "10v2"),
            (1, 112, 3, "112v3"),
            (2, 15, 1, "S15"),
            (3, 5, 1, "C05"),
            (4, 8, 2, "T08v2"),
            (5, 1, 1, "P01"),
            (6, 2, 4, "O02v4"),
        ] {
            assert_eq!(
                run("fansub", type_id, epno, version, true),
                format!("[ExGroup] Reibun no Tabi - {expected} [EB420C97].mkv")
            );
        }
        assert_eq!(
            run("fansub", 2, 1, 1, false),
            "Reibun no Tabi - S01 [EB420C97].mkv"
        );
        // The episode table of the wiki's "Built-in naming conventions": the number part each
        // convention writes for each AniDB episode type.
        let number = |preset: &str, type_id, epno, version| {
            let name = run(preset, type_id, epno, version, true);
            let rest = name.strip_prefix("Reibun no Tabi - ").unwrap().to_string();
            match preset {
                "aom" => rest.split(" - ").next().unwrap().to_string(),
                _ => rest.split([' ', '.']).next().unwrap().to_string(),
            }
        };
        let rows: [(i64, i64, i64, [&str; 3]); 6] = [
            (1, 10, 2, ["10v2", "10", "S01E10"]),
            (2, 15, 1, ["S15", "S15", "S00E15"]),
            (3, 5, 1, ["C05", "C05", "C05"]),
            (4, 8, 2, ["T08v2", "T08", "T08"]),
            (5, 1, 1, ["P01", "P01", "P01"]),
            (6, 2, 1, ["O02", "O02", "O02"]),
        ];
        for (type_id, epno, version, [default, episode_title, media]) in rows {
            assert_eq!(number("aom", type_id, epno, version), default);
            assert_eq!(
                number("episode-title", type_id, epno, version),
                episode_title
            );
            assert_eq!(number("media-server", type_id, epno, version), media);
        }
    }

    #[test]
    fn presets_are_recognized_by_text_whatever_the_line_endings() {
        for preset in PRESETS {
            let crlf = format!("\r\n{}\r\n", preset.source.replace('\n', "\r\n"));
            assert_eq!(preset_for(&crlf), Some(preset));
        }
        let edited = PRESETS[0].source.replace("' ['", "' ('");
        assert_eq!(preset_for(&edited), None);
        assert_eq!(preset_for(""), None);
    }

    #[test]
    fn preset_ids_and_names_are_unique() {
        for (i, a) in PRESETS.iter().enumerate() {
            for b in &PRESETS[i + 1..] {
                assert_ne!(a.id, b.id);
                assert_ne!(a.name, b.name);
                assert_ne!(a.source.trim(), b.source.trim());
            }
        }
    }
}
