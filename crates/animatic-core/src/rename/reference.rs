//! The short in-application reference for writing a custom pattern. Every code sample is run
//! against the synthetic sample file ([`super::presets::sample_value`]) when shown, so the
//! results the reference displays are always the engine's own.

/// One code sample and what it is for.
#[derive(Debug)]
pub struct Entry {
    pub code: &'static str,
    pub meaning: &'static str,
}

/// A titled group of samples, with optional explanatory lines before them.
#[derive(Debug)]
pub struct Section {
    pub title: &'static str,
    pub notes: &'static [&'static str],
    pub entries: &'static [Entry],
}

const fn e(code: &'static str, meaning: &'static str) -> Entry {
    Entry { code, meaning }
}

/// Basics, the values a pattern can read, functions, worked recipes and pitfalls.
pub const SECTIONS: &[Section] = &[
    Section {
        title: "How a pattern works",
        notes: &[
            "A pattern runs from the first line to the last. The value of the last line is the \
             new file name, including the extension.",
            "Text is written in single quotes ('like this') and joined with +.",
            "set('name', value) stores a value for later lines; afterwards name is that value. \
             Lines that only set values add nothing to the file name.",
            "if(condition, then, else) chooses one of two values; leave out else for nothing.",
            "A backslash (\\) in the result creates a folder; a name without one stays in the \
             file's folder (or the managed folder's rename destination).",
            "The characters : ? * \" < > | are not allowed in a file name. Replace them, as the \
             built-in conventions do with replace().",
            "AniDB numbers each episode type separately: regular episodes 1, 2, 3, …, and \
             specials (S), credits such as openings and endings (C), trailers (T), parodies \
             (P) and other episodes (O) each from 1. F.Version is 2 to 5 for a corrected \
             release (v2 to v5). The built-in conventions write all of them, for example 06v2, \
             S15, C05 and T08v2.",
            "Lines starting with // are comments. Upper and lower case do not matter in names.",
            "Press \"Validate on selected file\" to try the pattern on a real file before \
             renaming anything.",
        ],
        entries: &[],
    },
    Section {
        title: "Anime and episode",
        notes: &["The example values are those of a fictitious sample file."],
        entries: &[
            e("A.Name", "Main anime title"),
            e("at('x-jat')", "Romanized anime title"),
            e(
                "at('en')",
                "Official English anime title (empty when there is none)",
            ),
            e("at('ja')", "Official Japanese anime title"),
            e("A.Year", "Year or years of airing"),
            e("A.CategoryStr", "Anime type: TV Series, OVA, Movie, …"),
            e("E.EpisodeNo", "Episode number"),
            e(
                "padl(E.EpisodeNo, 2, '0')",
                "Episode number with at least two digits",
            ),
            e(
                "E.EpisodeStr",
                "Episode number with its type letter (S special, C credit, T trailer, P parody, \
                 O other)",
            ),
            e(
                "E.TypeId",
                "Episode type: 1 regular, 2 special, 3 credit, 4 trailer, 5 parody, 6 other",
            ),
            e(
                "if(E.IsSpecial, 'special', 'regular')",
                "E.IsSpecial is True for every episode that is not a regular episode",
            ),
            e("et('en')", "English episode title"),
            e("et('x-jat')", "Romanized episode title"),
        ],
    },
    Section {
        title: "File and release group",
        notes: &[
            "G (the group) and H (the local hash) are missing for some files: always use them \
             inside if(G, …) or if(H, …), as below.",
        ],
        entries: &[
            e("F.FileType", "Extension without the dot"),
            e(
                "F.Version",
                "Release version: 1, or 2 to 5 for a corrected release",
            ),
            e(
                "if(F.Version > 1, 'v' + F.Version, '')",
                "Version suffix only for v2 and later",
            ),
            e("uc(F.Crc)", "CRC32 checksum in capitals"),
            e("F.ResolutionString", "Video resolution"),
            e("F.Quality", "Quality as rated by AniDB"),
            e("F.Source", "Source: TV, DVD, Blu-ray, Web, …"),
            e("if(G, G.Shortname, '')", "Short group name"),
            e("if(G, G.Name, '')", "Full group name"),
            e(
                "if(H, H.Crc32, '')",
                "CRC32 Animatic computed when it hashed the file",
            ),
        ],
    },
    Section {
        title: "Text functions",
        notes: &[],
        entries: &[
            e("uc(A.Name)", "Capitals (letters A to Z only)"),
            e("lc(A.Name)", "Lower case (letters A to Z only)"),
            e("tc('the long road')", "Capitalize Each Word"),
            e(
                "replace(A.Name, ' ', '.')",
                "Replace every occurrence of a text",
            ),
            e(
                "limit(A.Name, 8, '…')",
                "Shorten to at most 8 characters, ending in …",
            ),
            e("substr(A.Name, 1, 7)", "7 characters starting at the first"),
            e("length(A.Name)", "Number of characters"),
            e("trim('  text  ')", "Remove spaces at both ends"),
            e(
                "padl(E.EpisodeNo, 3, '0')",
                "Pad on the left to 3 characters",
            ),
            e(
                "join(' - ', A.Name, '', E.EpisodeNo)",
                "Join the non-empty values with a separator",
            ),
            e(
                "in(F.Source, 'Blu-ray', 'DVD')",
                "True when the first value equals any of the others",
            ),
            e(
                "contains('Road', et('en'))",
                "True when the second text contains the first",
            ),
        ],
    },
    Section {
        title: "Recipes",
        notes: &["Copy a recipe into the editor and adapt it."],
        entries: &[
            e(
                "set('title', at('en'))\nif(length(title) = 0, set('title', A.Name))\n\
                 title + ' - ' + padl(E.EpisodeNo, 2, '0') + '.' + F.FileType",
                "English title when there is one, otherwise the main title",
            ),
            e(
                "set('group', '')\nif(G, set('group', '[' + G.Shortname + '] '))\n\
                 group + A.Name + ' - ' + padl(E.EpisodeNo, 2, '0') + '.' + F.FileType",
                "Group in front, left out when the file has no group",
            ),
            e(
                "set('folder', replace(A.Name, ':', ' -'))\n\
                 folder + '\\' + folder + ' - ' + padl(E.EpisodeNo, 2, '0') + '.' + F.FileType",
                "A folder per anime (removing colons, which file names cannot contain)",
            ),
            e(
                "set('sub', if(E.IsSpecial, 'Specials\\', ''))\n\
                 sub + A.Name + ' - ' + E.EpisodeStr + '.' + F.FileType",
                "Specials, credits and trailers in a Specials subfolder",
            ),
            e(
                "set('ep', E.EpisodeStr)\n\
                 if(E.TypeId = 3, set('ep', if(contains('Ending', et('en')), 'ED', 'OP') + \
                 padl(E.EpisodeNo, 2, '0')))\n\
                 A.Name + ' - ' + ep + '.' + F.FileType",
                "Openings and endings as OP01 and ED01 instead of AniDB's credit number (C01). \
                 AniDB has one credit type for both; this tells them apart by the English \
                 episode title, which is usually \"Opening\" or \"Ending\".",
            ),
            e(
                "A.Name + ' (' + A.Year + ') - ' + padl(E.EpisodeNo, 2, '0') + ' [' + \
                 F.ResolutionString + '].' + F.FileType",
                "Year and resolution in the name",
            ),
        ],
    },
    Section {
        title: "Common mistakes",
        notes: &[
            "A misspelled name does not cause an error: it becomes the word False in the file \
             name. Always read the result.",
            "Numbers next to each other are added: 'E' + E.EpisodeNo + F.Version gives E8 for \
             episode 6 version 2. Put text between them: 'E' + E.EpisodeNo + 'v' + F.Version.",
            "Set a variable before using it: an unset variable is False.",
            "Every line that is not a set(…) or if(…, set(…)) becomes part of the name.",
            "Comments must be on their own line; a // after code is a syntax error.",
        ],
        entries: &[],
    },
];

#[cfg(test)]
mod tests {
    use super::super::presets::{SampleSource, sample_value, sample_with};
    use super::super::{DataSource, Object, Pattern, evaluate};
    use super::*;

    #[test]
    fn every_reference_sample_runs_cleanly_on_the_sample_file() {
        for section in SECTIONS {
            for entry in section.entries {
                let value =
                    sample_value(entry.code).unwrap_or_else(|e| panic!("{}: {e}", entry.code));
                assert!(!value.is_empty(), "{} is empty", entry.code);
                assert_ne!(value, "False", "{} reads something unknown", entry.code);
            }
        }
    }

    #[test]
    fn the_documented_mistake_and_its_fix_behave_as_described() {
        assert_eq!(sample_value("'E' + E.EpisodeNo + F.Version").unwrap(), "E8");
        assert_eq!(
            sample_value("'E' + E.EpisodeNo + 'v' + F.Version").unwrap(),
            "E6v2"
        );
    }

    #[test]
    fn the_opening_and_ending_recipe_names_credits_by_their_title() {
        let recipe = SECTIONS
            .iter()
            .flat_map(|s| s.entries)
            .find(|e| e.code.contains("'ED', 'OP'"))
            .unwrap();
        let pattern = Pattern::compile(recipe.code).unwrap();
        let name = |type_id, title: &str| {
            let b = sample_with(type_id, 2, 1, true);
            struct Titled<'a>(&'a str);
            impl DataSource for Titled<'_> {
                fn anime(&self, _: i64) -> Option<Object> {
                    None
                }
                fn episode(&self, _: i64) -> Option<Object> {
                    None
                }
                fn language(&self, lang: &str) -> Option<String> {
                    Some(lang.into())
                }
                fn anime_titles(&self, aid: i64) -> Vec<(i64, String, String)> {
                    SampleSource.anime_titles(aid)
                }
                fn episode_titles(&self, _: i64) -> Vec<(String, String)> {
                    vec![("en".into(), self.0.into())]
                }
            }
            evaluate(&pattern, &b, &Titled(title)).unwrap().result
        };
        assert_eq!(name(3, "Ending"), "Reibun no Tabi - ED02.mkv");
        assert_eq!(name(3, "Opening 2"), "Reibun no Tabi - OP02.mkv");
        assert_eq!(name(4, "Ending"), "Reibun no Tabi - T2.mkv");
    }
}
