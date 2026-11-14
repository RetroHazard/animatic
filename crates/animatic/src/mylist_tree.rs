//! The MyList table as a tree: one top-level row per anime, its MyList entries (episodes and
//! files) as child rows while the anime is expanded.
//!
//! Everything here is a pure function of the loaded projection, the search text, the sort and
//! the expand state, so it is tested without a window. The application keeps one
//! [`MylistView`]: it regroups only when the projection, the search or the sort changes
//! (rule 16: the projection version is part of the key, so a stale snapshot never outlives its
//! replacement) and flattens the visible rows (anime rows plus the entries of expanded anime)
//! into the one list the virtualized table renders.
//!
//! * Children are always ordered by episode, in AniDB order: regular episodes by number, then
//!   specials, credits, trailers, parodies and others by type and number
//!   ([`animatic_core::library::EpisodeType`]'s order); entries without a known episode last.
//! * The column sort orders the anime rows by their aggregate ([`Summary`]); the default order
//!   is the anime title.
//! * The aggregate shown on screen is computed again for the visible anime rows with the
//!   user's not yet reloaded edits (the overlay); the stored one, used for sorting, reads the
//!   projection only.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use animatic_core::library::{LibraryProjection, MylistItem};

use crate::table::{SortKey, natural_cmp, sort_rows};

/// The group (release group) of the entries of one anime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GroupLabel {
    /// No entry has a group name.
    None,
    /// Every entry has this group.
    One(String),
    /// The entries come from different groups.
    Various,
}

impl GroupLabel {
    /// The cell text.
    pub fn text(&self) -> &str {
        match self {
            GroupLabel::None => "",
            GroupLabel::One(name) => name,
            GroupLabel::Various => "various",
        }
    }
}

/// How much of an anime is watched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Watched {
    None,
    Partly,
    All,
}

/// What the top-level row of an anime says about its entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Summary {
    pub entries: usize,
    /// Distinct episodes among the entries (an entry without a known episode counts once).
    pub episodes: usize,
    pub size: i64,
    /// The state every entry has, `None` when they differ ("Mixed").
    pub state: Option<i64>,
    pub watched: usize,
    pub group: GroupLabel,
    /// Entries with an edit waiting in the command queue.
    pub edits: usize,
    /// Entries with a removal waiting.
    pub removals: usize,
    /// Entries with a blocked queued command.
    pub blocked: usize,
}

impl Summary {
    /// Aggregate `members` (indices into `p.mylist`, ordered by episode so that entries of one
    /// episode are adjacent). `shown` gives the state and watched flag to count for an entry,
    /// which lets the screen include edits that no projection reflects yet.
    pub fn of(
        p: &LibraryProjection,
        members: &[usize],
        shown: impl Fn(&MylistItem) -> (i64, bool),
    ) -> Summary {
        let mut summary = Summary {
            entries: members.len(),
            episodes: 0,
            size: 0,
            state: None,
            watched: 0,
            group: GroupLabel::None,
            edits: 0,
            removals: 0,
            blocked: 0,
        };
        let mut previous_eid = None;
        let mut gid = None;
        let mut mixed_state = false;
        for &i in members {
            let item = &p.mylist[i];
            let (state, viewed) = shown(item);
            let eid = item.episode.as_ref().map_or(item.entry.eid, |e| e.eid);
            if eid == 0 || previous_eid != Some(eid) {
                summary.episodes += 1;
            }
            previous_eid = Some(eid);
            summary.size += item.file.as_ref().map_or(0, |f| f.size);
            summary.watched += usize::from(viewed);
            match summary.state {
                None if !mixed_state => summary.state = Some(state),
                Some(common) if common != state => {
                    summary.state = None;
                    mixed_state = true;
                }
                _ => {}
            }
            let entry_gid = item.file.as_ref().map_or(item.entry.gid, |f| f.gid);
            match gid {
                None => {
                    gid = Some(entry_gid);
                    summary.group = item
                        .file
                        .as_ref()
                        .map(|f| f.group_label())
                        .filter(|label| !label.is_empty())
                        .map_or(GroupLabel::None, GroupLabel::One);
                }
                Some(first) if first != entry_gid => summary.group = GroupLabel::Various,
                _ => {}
            }
            if let Some(pending) = p.mylist_pending.get(&item.entry.lid) {
                summary.edits += usize::from(pending.edit);
                summary.removals += usize::from(pending.delete);
                summary.blocked += usize::from(pending.blocked);
            }
        }
        summary
    }

    pub fn watched_state(&self) -> Watched {
        match self.watched {
            0 => Watched::None,
            n if n == self.entries => Watched::All,
            _ => Watched::Partly,
        }
    }

    /// The Episode column text: `12 eps`, or `15 files` when an episode has several files.
    pub fn episode_text(&self) -> String {
        if self.entries > self.episodes {
            count(self.entries, "file", "files")
        } else {
            count(self.episodes, "ep", "eps")
        }
    }

    /// The hover text of the Episode column.
    pub fn episode_detail(&self) -> String {
        format!(
            "{}, {}",
            count(self.episodes, "episode", "episodes"),
            count(self.entries, "file", "files")
        )
    }

    /// The Queue column text; empty when nothing waits.
    pub fn queue_text(&self) -> String {
        let mut parts = Vec::new();
        if self.edits > 0 {
            parts.push(format!("{} queued", self.edits));
        }
        if self.removals > 0 {
            parts.push(format!("{} removing", self.removals));
        }
        if self.blocked > 0 {
            parts.push(format!("{} blocked", self.blocked));
        }
        parts.join(", ")
    }

    /// Sort key of the Queue column: blocked and removal states after plain edits.
    fn queue_key(&self) -> (usize, usize, usize) {
        (self.blocked, self.removals, self.edits)
    }
}

fn count(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// The entries of one anime that the search lets through.
#[derive(Debug, Clone)]
pub struct Group {
    pub aid: i64,
    /// Indices into `LibraryProjection::mylist`, in episode order.
    pub members: Vec<usize>,
    /// The aggregate of the projection as loaded (what sorting uses).
    pub summary: Summary,
}

/// One row of the flattened table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowRef {
    /// The top-level row of `groups[n]`.
    Anime(usize),
    /// An entry (`p.mylist[item]`) under `groups[group]`.
    Entry { group: usize, item: usize },
}

/// AniDB order of two entries of one anime: episodes by type then number (regular episodes
/// first), then entries without a known episode, each by episode id and finally list id.
pub fn episode_order(a: &MylistItem, b: &MylistItem) -> Ordering {
    let key = |m: &MylistItem| {
        (
            m.episode.is_none(),
            m.episode.as_ref().map(|e| (e.kind, e.epno)),
            m.entry.eid,
            m.entry.lid,
        )
    };
    key(a).cmp(&key(b))
}

/// Group the entries whose anime title contains `query` (lower case; empty lets all through)
/// by anime: anime in title order, entries in episode order.
pub fn build_groups(p: &LibraryProjection, query: &str) -> Vec<Group> {
    let mut by_aid: HashMap<i64, usize> = HashMap::new();
    let mut groups: Vec<Group> = Vec::new();
    for i in 0..p.mylist.len() {
        if !query.is_empty() && !p.keys.mylist[i].contains(query) {
            continue;
        }
        let aid = p.mylist[i].entry.aid;
        let n = *by_aid.entry(aid).or_insert_with(|| {
            groups.push(Group {
                aid,
                members: Vec::new(),
                summary: Summary::of(p, &[], |_| (0, false)),
            });
            groups.len() - 1
        });
        groups[n].members.push(i);
    }
    for group in &mut groups {
        group
            .members
            .sort_by(|&a, &b| episode_order(&p.mylist[a], &p.mylist[b]));
        group.summary = Summary::of(p, &group.members, |m| (m.entry.state, m.entry.viewed));
    }
    groups.sort_by(|a, b| {
        natural_cmp(&p.keys.mylist[a.members[0]], &p.keys.mylist[b.members[0]])
            .then(a.aid.cmp(&b.aid))
    });
    groups
}

/// Orders the anime rows (`order` holds indices into `groups`) by the sort column's aggregate.
/// Equal rows keep the title order of `groups`; without a sort nothing changes.
pub fn sort_anime_rows(
    p: &LibraryProjection,
    groups: &[Group],
    order: &mut [usize],
    sort: &SortKey,
) {
    sort_rows(order, sort, |column, a, b| {
        let (a, b) = (&groups[a], &groups[b]);
        let (sa, sb) = (&a.summary, &b.summary);
        match column {
            "anime" => natural_cmp(&p.keys.mylist[a.members[0]], &p.keys.mylist[b.members[0]]),
            "episode" => (sa.episodes, sa.entries).cmp(&(sb.episodes, sb.entries)),
            "group" => natural_cmp(sa.group.text(), sb.group.text()),
            "state" => sa
                .state
                .unwrap_or(i64::MAX)
                .cmp(&sb.state.unwrap_or(i64::MAX)),
            // Compare the watched fractions without dividing.
            "watched" => (sa.watched * sb.entries).cmp(&(sb.watched * sa.entries)),
            "size" => sa.size.cmp(&sb.size),
            "queue" => sa.queue_key().cmp(&sb.queue_key()),
            _ => Ordering::Equal,
        }
    });
}

/// The visible rows: every anime row, followed by its entries while the anime is expanded.
pub fn flatten(groups: &[Group], order: &[usize], expanded: &HashSet<i64>) -> Vec<RowRef> {
    let mut rows = Vec::with_capacity(order.len());
    for &n in order {
        rows.push(RowRef::Anime(n));
        if expanded.contains(&groups[n].aid) {
            rows.extend(
                groups[n]
                    .members
                    .iter()
                    .map(|&item| RowRef::Entry { group: n, item }),
            );
        }
    }
    rows
}

type ViewKey = (u64, String, SortKey);

/// The grouped, sorted and flattened My list table, rebuilt only when its inputs change.
#[derive(Default)]
pub struct MylistView {
    key: Option<ViewKey>,
    /// Counts regroupings, so the flat rows know their groups changed.
    built: u64,
    groups: Vec<Group>,
    by_aid: HashMap<i64, usize>,
    order: Vec<usize>,
    expanded: HashSet<i64>,
    /// Counts changes of `expanded`.
    expand_version: u64,
    flat_for: Option<(u64, u64)>,
    rows: Vec<RowRef>,
}

impl MylistView {
    /// Regroup and re-sort when the projection `version`, the lower-case `query` or the `sort`
    /// changed, and re-flatten when that or the expand state changed.
    pub fn refresh(&mut self, p: &LibraryProjection, version: u64, query: &str, sort: &SortKey) {
        let key = (version, query.to_string(), sort.clone());
        if self.key.as_ref() != Some(&key) {
            self.groups = build_groups(p, query);
            self.by_aid = self
                .groups
                .iter()
                .enumerate()
                .map(|(n, g)| (g.aid, n))
                .collect();
            self.order = (0..self.groups.len()).collect();
            sort_anime_rows(p, &self.groups, &mut self.order, sort);
            self.key = Some(key);
            self.built += 1;
        }
        if self.flat_for != Some((self.built, self.expand_version)) {
            self.rows = flatten(&self.groups, &self.order, &self.expanded);
            self.flat_for = Some((self.built, self.expand_version));
        }
    }

    pub fn rows(&self) -> &[RowRef] {
        &self.rows
    }

    pub fn group(&self, n: usize) -> &Group {
        &self.groups[n]
    }

    /// The group of anime `aid`, if the search lets it through.
    pub fn group_of(&self, aid: i64) -> Option<&Group> {
        self.by_aid.get(&aid).map(|&n| &self.groups[n])
    }

    pub fn is_expanded(&self, aid: i64) -> bool {
        self.expanded.contains(&aid)
    }

    pub fn toggle(&mut self, aid: i64) {
        if !self.expanded.remove(&aid) {
            self.expanded.insert(aid);
        }
        self.expand_version += 1;
    }

    /// Expand every anime of the table.
    pub fn expand_all(&mut self) {
        self.expanded.extend(self.groups.iter().map(|g| g.aid));
        self.expand_version += 1;
    }

    pub fn collapse_all(&mut self) {
        self.expanded.clear();
        self.expand_version += 1;
    }

    pub fn anime_count(&self) -> usize {
        self.groups.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use animatic_core::library::{
        Anime, Episode, EpisodeType, FileRec, MylistEntry, MylistPending,
    };
    use animatic_core::settings::SortDirection;

    fn ascending(column: &str) -> SortKey {
        Some((column.to_string(), SortDirection::Ascending))
    }

    fn descending(column: &str) -> SortKey {
        Some((column.to_string(), SortDirection::Descending))
    }

    /// A projection under construction: anime, then their entries.
    struct Fixture {
        p: LibraryProjection,
    }

    impl Fixture {
        fn new() -> Self {
            Self {
                p: LibraryProjection::default(),
            }
        }

        fn anime(&mut self, aid: i64, name: &str) {
            self.p.anime.push(Anime {
                aid,
                name: name.into(),
                ..Anime::default()
            });
        }

        #[allow(clippy::too_many_arguments)]
        fn entry(
            &mut self,
            aid: i64,
            kind: EpisodeType,
            epno: i64,
            state: i64,
            viewed: bool,
            size: i64,
            gid: i64,
        ) -> usize {
            let lid = self.p.mylist.len() as i64 + 1;
            let eid = aid * 1000 + (kind as i64) * 100 + epno;
            self.p.mylist.push(MylistItem {
                entry: MylistEntry {
                    lid,
                    aid,
                    eid,
                    gid,
                    state,
                    viewed,
                    ..MylistEntry::default()
                },
                file: Some(FileRec {
                    fid: lid,
                    aid,
                    eid,
                    gid,
                    size,
                    group_short: (gid != 0).then(|| format!("G{gid}")),
                    ..FileRec::default()
                }),
                episode: Some(Episode {
                    eid,
                    aid,
                    epno,
                    kind,
                    ..Episode::default()
                }),
            });
            self.p.mylist.len() - 1
        }

        /// Fills the search keys the projection normally builds.
        fn finish(mut self) -> LibraryProjection {
            let anime = &self.p.anime;
            self.p.keys.mylist_anime = self
                .p
                .mylist
                .iter()
                .map(|m| anime.iter().position(|a| a.aid == m.entry.aid))
                .collect();
            self.p.keys.mylist = self
                .p
                .keys
                .mylist_anime
                .iter()
                .zip(&self.p.mylist)
                .map(|(a, m)| match a {
                    Some(a) => anime[*a].name.to_ascii_lowercase(),
                    None => format!("aid {}", m.entry.aid),
                })
                .collect();
            self.p
        }
    }

    use EpisodeType::{Credit, Other, Regular, Special, Trailer};

    fn episodes_of(p: &LibraryProjection, group: &Group) -> Vec<String> {
        group
            .members
            .iter()
            .map(|&i| p.mylist[i].episode.as_ref().unwrap().epno_str())
            .collect()
    }

    #[test]
    fn entries_of_an_anime_follow_the_anidb_episode_order() {
        let mut f = Fixture::new();
        f.anime(1, "Synthetic Show");
        // Inserted in the irregular order a database scan gives.
        for (kind, epno) in [
            (Other, 1),
            (Regular, 10),
            (Credit, 2),
            (Special, 1),
            (Regular, 2),
            (Trailer, 1),
            (Credit, 1),
            (Regular, 1),
            (Special, 10),
            (Special, 2),
        ] {
            f.entry(1, kind, epno, 1, false, 10, 1);
        }
        let p = f.finish();
        let groups = build_groups(&p, "");
        assert_eq!(groups.len(), 1);
        assert_eq!(
            episodes_of(&p, &groups[0]),
            ["1", "2", "10", "S1", "S2", "S10", "C1", "C2", "T1", "O1"]
        );
    }

    #[test]
    fn entries_without_an_episode_come_last_and_files_of_one_episode_stay_together() {
        let mut f = Fixture::new();
        f.anime(1, "Synthetic Show");
        let a = f.entry(1, Regular, 2, 1, false, 10, 1);
        let b = f.entry(1, Regular, 1, 1, false, 10, 1);
        let c = f.entry(1, Regular, 2, 1, false, 10, 2);
        let d = f.entry(1, Regular, 1, 1, false, 10, 2);
        let mut p = f.finish();
        p.mylist[a].episode = None;
        p.mylist[a].entry.eid = 0;
        let groups = build_groups(&p, "");
        // Episode 1 twice, episode 2 once, then the entry whose episode is unknown.
        assert_eq!(groups[0].members, [b, d, c, a]);
        assert_eq!(groups[0].summary.episodes, 3);
        assert_eq!(groups[0].summary.entries, 4);
        assert_eq!(groups[0].summary.episode_text(), "4 files");
    }

    #[test]
    fn anime_are_grouped_by_aid_in_title_order_and_search_filters_by_title() {
        let mut f = Fixture::new();
        f.anime(2, "Beta Show 10");
        f.anime(1, "Beta Show 2");
        f.anime(3, "Alpha Show");
        for aid in [2, 1, 3, 1, 2, 3] {
            f.entry(aid, Regular, 1, 1, false, 10, 1);
        }
        let p = f.finish();
        let groups = build_groups(&p, "");
        assert_eq!(
            groups.iter().map(|g| g.aid).collect::<Vec<_>>(),
            [3, 1, 2],
            "natural title order"
        );
        assert!(groups.iter().all(|g| g.members.len() == 2));
        let found = build_groups(&p, "beta show 1");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].aid, 2);
        assert_eq!(
            found[0].members.len(),
            2,
            "a matching anime keeps its entries"
        );
        assert!(build_groups(&p, "gamma").is_empty());
    }

    #[test]
    fn the_summary_reports_common_values_and_mixed_ones() {
        let mut f = Fixture::new();
        f.anime(1, "Synthetic Show");
        f.entry(1, Regular, 1, 1, true, 100, 7);
        f.entry(1, Regular, 2, 1, true, 200, 7);
        let p = f.finish();
        let all = Summary::of(&p, &[0, 1], |m| (m.entry.state, m.entry.viewed));
        assert_eq!(
            (all.entries, all.episodes, all.size, all.state),
            (2, 2, 300, Some(1))
        );
        assert_eq!(all.watched_state(), Watched::All);
        assert_eq!(all.group, GroupLabel::One("G7".into()));
        assert_eq!(all.episode_text(), "2 eps");
        assert_eq!(all.queue_text(), "");

        // The overlay of a just-saved edit shows before the projection reloads.
        let mixed = Summary::of(&p, &[0, 1], |m| {
            if m.entry.lid == 1 {
                (3, false)
            } else {
                (m.entry.state, m.entry.viewed)
            }
        });
        assert_eq!(mixed.state, None);
        assert_eq!(mixed.watched_state(), Watched::Partly);
        let none = Summary::of(&p, &[0, 1], |_| (1, false));
        assert_eq!(none.watched_state(), Watched::None);
        assert_eq!(Summary::of(&p, &[0], |_| (2, false)).episode_text(), "1 ep");
    }

    #[test]
    fn mixed_states_stay_mixed_and_groups_are_various() {
        let mut f = Fixture::new();
        f.anime(1, "Synthetic Show");
        f.entry(1, Regular, 1, 1, false, 1, 1);
        f.entry(1, Regular, 2, 2, false, 1, 2);
        f.entry(1, Regular, 3, 1, false, 1, 1);
        let p = f.finish();
        let s = Summary::of(&p, &[0, 1, 2], |m| (m.entry.state, m.entry.viewed));
        assert_eq!(
            s.state, None,
            "a later equal value must not restore a common state"
        );
        assert_eq!(s.group, GroupLabel::Various);
        assert_eq!(s.group.text(), "various");
    }

    #[test]
    fn the_queue_column_counts_pending_edits_removals_and_blocked_commands() {
        let mut f = Fixture::new();
        f.anime(1, "Synthetic Show");
        for epno in 1..=3 {
            f.entry(1, Regular, epno, 1, false, 1, 1);
        }
        let mut p = f.finish();
        p.mylist_pending.insert(
            1,
            MylistPending {
                edit: true,
                ..MylistPending::default()
            },
        );
        p.mylist_pending.insert(
            2,
            MylistPending {
                delete: true,
                blocked: true,
                ..MylistPending::default()
            },
        );
        let s = Summary::of(&p, &[0, 1, 2], |m| (m.entry.state, m.entry.viewed));
        assert_eq!(s.queue_text(), "1 queued, 1 removing, 1 blocked");
    }

    fn sorted(p: &LibraryProjection, sort: &SortKey) -> Vec<i64> {
        let groups = build_groups(p, "");
        let mut order: Vec<usize> = (0..groups.len()).collect();
        sort_anime_rows(p, &groups, &mut order, sort);
        order.into_iter().map(|n| groups[n].aid).collect()
    }

    fn library() -> LibraryProjection {
        let mut f = Fixture::new();
        f.anime(1, "Alpha");
        f.anime(2, "Beta");
        f.anime(3, "Gamma");
        // Alpha: 2 files, 1 watched, 300 bytes; Beta: 1 file, watched, 900; Gamma: 3 files, 600.
        f.entry(1, Regular, 1, 1, true, 100, 1);
        f.entry(1, Regular, 2, 2, false, 200, 1);
        f.entry(2, Regular, 1, 3, true, 900, 1);
        f.entry(3, Regular, 1, 1, false, 100, 1);
        f.entry(3, Regular, 2, 1, false, 200, 2);
        f.entry(3, Regular, 3, 1, false, 300, 1);
        f.finish()
    }

    #[test]
    fn anime_rows_sort_by_their_aggregate_and_children_keep_episode_order() {
        let p = library();
        assert_eq!(sorted(&p, &None), [1, 2, 3], "default: title order");
        assert_eq!(sorted(&p, &descending("anime")), [3, 2, 1]);
        assert_eq!(sorted(&p, &ascending("size")), [1, 3, 2]);
        assert_eq!(sorted(&p, &descending("size")), [2, 3, 1]);
        assert_eq!(sorted(&p, &ascending("episode")), [2, 1, 3]);
        assert_eq!(
            sorted(&p, &ascending("state")),
            [3, 2, 1],
            "mixed sorts last"
        );
        assert_eq!(sorted(&p, &ascending("watched")), [3, 1, 2]);
        assert_eq!(sorted(&p, &descending("watched")), [2, 1, 3]);
        assert_eq!(
            sorted(&p, &ascending("group")),
            [1, 2, 3],
            "G1 before various"
        );
        assert_eq!(sorted(&p, &ascending("no_such_column")), [1, 2, 3]);
        // Whatever the sort, the entries of an anime stay in episode order.
        let groups = build_groups(&p, "");
        for g in &groups {
            let numbers: Vec<_> = g
                .members
                .iter()
                .map(|&i| p.mylist[i].episode.as_ref().unwrap().epno)
                .collect();
            assert!(numbers.windows(2).all(|w| w[0] <= w[1]));
        }
    }

    #[test]
    fn flattening_lists_the_entries_of_expanded_anime_only() {
        let p = library();
        let groups = build_groups(&p, "");
        let order = [0, 1, 2];
        let none = flatten(&groups, &order, &HashSet::new());
        assert_eq!(none, [RowRef::Anime(0), RowRef::Anime(1), RowRef::Anime(2)]);
        let beta = flatten(&groups, &order, &HashSet::from([2]));
        assert_eq!(
            beta,
            [
                RowRef::Anime(0),
                RowRef::Anime(1),
                RowRef::Entry { group: 1, item: 2 },
                RowRef::Anime(2),
            ]
        );
    }

    #[test]
    fn the_view_keeps_expand_state_across_reloads_and_rebuilds_on_new_inputs() {
        let p = library();
        let mut view = MylistView::default();
        view.refresh(&p, 1, "", &None);
        assert_eq!(view.rows().len(), 3, "collapsed by default");
        view.toggle(3);
        view.refresh(&p, 1, "", &None);
        assert_eq!(view.rows().len(), 6);
        assert!(view.is_expanded(3));
        // A new projection (a newer snapshot) regroups; expanded anime stay expanded.
        view.refresh(&p, 2, "", &None);
        assert_eq!(view.rows().len(), 6);
        // The search narrows the table to the matching anime.
        view.refresh(&p, 2, "alp", &None);
        assert_eq!(view.rows(), [RowRef::Anime(0)]);
        assert_eq!(view.group(0).aid, 1);
        assert!(view.group_of(3).is_none(), "filtered out");
        view.refresh(&p, 2, "", &None);
        view.expand_all();
        view.refresh(&p, 2, "", &None);
        assert_eq!(view.rows().len(), 3 + 6);
        view.collapse_all();
        view.refresh(&p, 2, "", &None);
        assert_eq!(view.rows().len(), 3);
        assert_eq!(view.anime_count(), 3);
        // Sorting reorders the anime rows only.
        view.refresh(&p, 2, "", &descending("anime"));
        assert_eq!(view.rows()[0], RowRef::Anime(2));
        assert_eq!(view.group(2).aid, 3);
    }
}
