use super::plan::{PatternKind, classify};
use super::*;

#[derive(Default)]
struct Fixture {
    anime_titles: Vec<(i64, String, String)>,
    episode_titles: Vec<(String, String)>,
}

impl DataSource for Fixture {
    fn anime(&self, _: i64) -> Option<Object> {
        None
    }
    fn episode(&self, _: i64) -> Option<Object> {
        None
    }
    fn language(&self, lang: &str) -> Option<String> {
        Some(lang.to_string())
    }
    fn anime_titles(&self, _: i64) -> Vec<(i64, String, String)> {
        self.anime_titles.clone()
    }
    fn episode_titles(&self, _: i64) -> Vec<(String, String)> {
        self.episode_titles.clone()
    }
}

/// Metadata for one file; `total` and `type_count` are the cached counts that used to steer
/// the built-in default's episode padding.
fn sample(total: i64, type_count: i64, type_id: i64, epno: i64) -> Bindings {
    Bindings {
        anime: Some(
            Object::new("TAniDBAnime")
                .with("Id", Value::Int(1))
                .with("Name", Value::WStr("Fallback".into()))
                .with("TotalEpisodeCount", Value::Int(total)),
        ),
        episode: Some(
            Object::new("TAniDBEpisode")
                .with("Id", Value::Int(2))
                .with("Name", Value::WStr("Fallback episode".into()))
                .with("EpisodeNo", Value::Int(epno))
                .with("EpisodeTypeCount", Value::Int(type_count))
                .with("TypeId", Value::Int(type_id)),
        ),
        file: Some(
            Object::new("TAniDBFile")
                .with("Version", Value::Int(2))
                .with("Crc", Value::AStr("eb420c97".into()))
                .with("FileType", Value::AStr("mkv".into())),
        ),
        group: Some(
            Object::new("TAniDBGroup")
                .with("Shortname", Value::WStr("Coalgirls".into()))
                .with("Name", Value::WStr("Coal Girls".into())),
        ),
        hash: None,
        mylist: None,
        path: String::new(),
        base_path: String::new(),
    }
}

fn titles() -> Fixture {
    Fixture {
        anime_titles: vec![(1, "x-jat".into(), "Nisemonogatari".into())],
        episode_titles: vec![("en".into(), "Karen Bee, Part One".into())],
    }
}

fn default_name(bindings: &Bindings) -> String {
    let pattern = Pattern::compile(include_str!("default_pattern.txt")).unwrap();
    let result = evaluate(&pattern, bindings, &titles()).unwrap();
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    result.result
}

#[test]
fn default_pattern_produces_consumer_visible_name() {
    assert_eq!(
        default_name(&sample(12, 12, 1, 3)),
        "Nisemonogatari - 03v2 - Karen Bee, Part One [Coalgirls](EB420C97).mkv"
    );
}

#[test]
fn default_episode_padding_ignores_cached_counts() {
    let name = default_name(&sample(12, 12, 1, 3));
    // The same episode keeps its name whatever totals or per-type counts were cached.
    for (total, type_count) in [(0, 0), (1, 1), (9, 9), (13, 3), (150, 150), (1000, 7)] {
        assert_eq!(default_name(&sample(total, type_count, 1, 3)), name);
    }
}

#[test]
fn default_episode_numbers_use_two_digits_or_more_with_type_prefixes() {
    let number = |type_id, epno| {
        let name = default_name(&sample(12, 12, type_id, epno));
        let rest = name.strip_prefix("Nisemonogatari - ").unwrap();
        rest.split(" - ").next().unwrap().to_string()
    };
    assert_eq!(number(1, 7), "07v2");
    assert_eq!(number(1, 100), "100v2");
    assert_eq!(number(1, 1234), "1234v2");
    assert_eq!(number(2, 1), "S01v2");
    assert_eq!(number(3, 12), "C12v2");
    assert_eq!(number(4, 2), "T02v2");
    assert_eq!(number(5, 3), "P03v2");
    assert_eq!(number(6, 120), "O120v2");
}

#[test]
fn custom_unpadded_and_unicode_patterns_are_unaffected() {
    let pattern =
        Pattern::compile("A.Name + ' 第' + E.EpisodeNo + '話 ' + 'é' + '.' + F.FileType").unwrap();
    let mut b = sample(12, 12, 1, 3);
    b.anime = Some(
        Object::new("TAniDBAnime")
            .with("Id", Value::Int(1))
            .with("Name", Value::WStr("化物語".into())),
    );
    let result = evaluate(&pattern, &b, &Fixture::default()).unwrap();
    assert_eq!(result.result, "化物語 第3話 é.mkv");
    assert_eq!(classify("A.Name + ' ' + E.EpisodeNo"), PatternKind::Custom);
}

#[test]
fn built_in_defaults_are_recognized_but_custom_patterns_are_not() {
    assert_eq!(
        classify(&include_str!("default_pattern.txt").replace('\n', "\r\n")),
        PatternKind::StableDefault
    );
    assert_eq!(classify(" \r\n\t"), PatternKind::Empty);
    let edited = include_str!("default_pattern.txt").replace("'v'", "'ver'");
    assert_eq!(classify(&edited), PatternKind::Custom);
}

#[test]
fn result_path_stays_beside_source_and_rejects_empty_components() {
    let old = Path::new("library/show/old.mkv");
    assert_eq!(
        target_path(old, "new.mkv", "").unwrap(),
        Some(PathBuf::from("library/show/new.mkv"))
    );
    assert_eq!(target_path(old, "old.mkv", "").unwrap(), None);
    assert!(target_path(old, "season//new.mkv", "").is_err());
}

#[test]
fn preprocessing_and_user_functions_execute_as_one_pattern() {
    let p =
        Pattern::compile("function('label', '['+param1+']')\nset('x', label('ok'))\nx").unwrap();
    let result = evaluate(&p, &Bindings::default(), &Fixture::default()).unwrap();
    assert_eq!(result.result, "[ok]");
    assert!(
        result
            .variables
            .iter()
            .any(|(n, v)| n.eq_ignore_ascii_case("x") && v == "[ok]")
    );
}

fn plan_for(pattern: &str, bindings: &Bindings) -> plan::Plan {
    plan::plan(
        pattern,
        bindings,
        &titles(),
        Path::new("library/show/old.mkv"),
    )
}

#[test]
fn plan_with_the_default_pattern_yields_a_target_beside_the_file() {
    let plan = plan_for(include_str!("default_pattern.txt"), &sample(12, 12, 1, 3));
    assert!(plan.can_apply(), "{plan:?}");
    assert_eq!(
        plan.target.unwrap(),
        PathBuf::from(
            "library/show/Nisemonogatari - 03v2 - Karen Bee, Part One [Coalgirls](EB420C97).mkv"
        )
    );
}

#[test]
fn plan_reports_syntax_errors_and_blank_patterns_without_a_target() {
    for pattern in ["set('a', ", "  \r\n "] {
        let plan = plan_for(pattern, &sample(12, 12, 1, 3));
        assert!(!plan.can_apply());
        assert!(plan.target.is_none());
        assert!(!plan.already_named());
        assert_eq!(plan.errors.len(), 1, "{plan:?}");
    }
}

#[test]
fn plan_rejects_empty_and_boolean_results_instead_of_naming_files_after_them() {
    for pattern in ["''", "if(1=2, 'x.mkv')", "set('x', 'y')", "1=1"] {
        let plan = plan_for(pattern, &sample(12, 12, 1, 3));
        assert!(plan.target.is_none(), "{pattern}: {plan:?}");
        assert!(!plan.errors.is_empty(), "{pattern}: {plan:?}");
        assert!(!plan.already_named() && !plan.can_apply());
    }
}

#[test]
fn plan_with_evaluation_warnings_is_not_applicable() {
    // `lc` called with two arguments is a soft error in the evaluator.
    let plan = plan_for("lc('A', 'B') + '.mkv'", &sample(12, 12, 1, 3));
    assert!(!plan.warnings.is_empty(), "{plan:?}");
    assert!(!plan.can_apply());
}

#[test]
fn plan_that_names_the_file_as_it_already_is_is_neither_error_nor_applicable() {
    let plan = plan_for("'old.mkv'", &sample(12, 12, 1, 3));
    assert!(plan.already_named(), "{plan:?}");
    assert!(!plan.can_apply());
}

#[test]
fn plan_requires_fetched_metadata_for_the_stable_default_only() {
    let old = Path::new("library/show/old.mkv");
    let mut pending_title = sample(12, 12, 1, 3);
    pending_title.anime = Some(
        Object::new("TAniDBAnime")
            .with("Id", Value::Int(1))
            .with("Name", Value::WStr(objects::TITLE_PENDING.into())),
    );
    let stable = include_str!("default_pattern.txt");
    let blocked = plan::plan(stable, &pending_title, &titles(), old);
    assert!(
        blocked.target.is_none() && !blocked.errors.is_empty(),
        "{blocked:?}"
    );
    let no_number = sample(12, 12, 1, 0);
    let blocked = plan::plan(stable, &no_number, &titles(), old);
    assert!(
        blocked.target.is_none() && !blocked.errors.is_empty(),
        "{blocked:?}"
    );
    // Valid custom patterns are judged by their own result, never by metadata heuristics.
    for pattern in [
        "'hash-' + F.Crc + '.' + F.FileType",
        "A.Name + ' ' + E.EpisodeNo + '.mkv'",
    ] {
        let ok = plan::plan(pattern, &no_number, &titles(), old);
        assert!(ok.can_apply(), "{pattern}: {ok:?}");
    }
}

#[test]
fn targets_must_be_real_names() {
    let old = Path::new("library/show/old.mkv");
    assert!(target_path(old, "", "").is_err());
    assert!(target_path(old, "  ", "").is_err());
    for bad in [
        "ab:c.mkv",
        "a?b.mkv",
        "x<y>.mkv",
        "tab\u{1}.mkv",
        "NUL.mkv",
        "com1",
        "lpt9.txt",
        "COM¹",
        "com².mkv",
        "Com³.tar.gz",
        "LPT¹",
        "lpt².txt",
        "LPT³.mkv",
    ] {
        let err = target_path(old, bad, "").unwrap_err();
        assert!(err.contains("invalid file name"), "{bad}: {err}");
    }
    // Multi-byte names shaped like device names must not trip the byte checks.
    assert!(target_path(old, "éé.mkv", "").unwrap().is_some());
    assert!(target_path(old, "com10.mkv", "").unwrap().is_some());
    // A drive root keeps its colon.
    assert!(target_path(old, "D:\\Anime\\x.mkv", "").unwrap().is_some());
}

#[test]
fn component_length_limit_counts_what_the_file_system_counts() {
    let old = Path::new("library/show/old.mkv");
    let name = |units: usize| format!("{}.mkv", "é".repeat(units));
    // 'é' is 2 bytes in UTF-8 but one UTF-16 unit.
    assert!(target_path(old, &name(100), "").is_ok());
    assert!(target_path(old, &name(260), "").is_err());
    // 130 of them: 264 bytes (too long on POSIX) but only 134 UTF-16 units (fine on Windows).
    assert_eq!(target_path(old, &name(130), "").is_ok(), cfg!(windows));
}
