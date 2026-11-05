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

#[test]
fn default_pattern_produces_consumer_visible_name() {
    let pattern = Pattern::compile(include_str!("default_pattern.txt")).unwrap();
    let bindings = Bindings {
        anime: Some(
            Object::new("TAniDBAnime")
                .with("Id", Value::Int(1))
                .with("Name", Value::WStr("Fallback".into()))
                .with("TotalEpisodeCount", Value::Int(12)),
        ),
        episode: Some(
            Object::new("TAniDBEpisode")
                .with("Id", Value::Int(2))
                .with("Name", Value::WStr("Fallback episode".into()))
                .with("EpisodeNo", Value::Int(3))
                .with("EpisodeTypeCount", Value::Int(12))
                .with("TypeId", Value::Int(1)),
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
    };
    let source = Fixture {
        anime_titles: vec![(1, "x-jat".into(), "Nisemonogatari".into())],
        episode_titles: vec![("en".into(), "Karen Bee, Part One".into())],
    };
    let result = evaluate(&pattern, &bindings, &source).unwrap();
    assert_eq!(
        result.result,
        "Nisemonogatari - 03v2 - Karen Bee, Part One [Coalgirls](EB420C97).mkv"
    );
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
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
