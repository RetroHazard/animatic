//! Pure rules behind the saved window geometry, the per-table column layout and the status row
//! colours. The window code in the `animatic` crate only feeds these functions and applies
//! their results, so the rules can be tested without a window.

use std::collections::BTreeMap;

use crate::library::{Anime, AnimeMylistState, LocalFile};
use crate::settings::{ColorSlot, ColumnState, SlotColor, SortDirection};

// ---------------------------------------------------------------------------------------------
// Window geometry
// ---------------------------------------------------------------------------------------------

/// Smallest inner window size, in points. The window itself is created with this minimum.
pub const MIN_WINDOW_SIZE: [f32; 2] = [900.0, 600.0];
/// Inner window size used when no usable size was saved.
pub const DEFAULT_WINDOW_SIZE: [f32; 2] = [1280.0, 800.0];
/// No window is restored larger than this (the windowing layer clamps to the monitor anyway).
const MAX_WINDOW_EXTENT: f32 = 16_384.0;
/// Positions beyond this are outside every desktop (Windows limits the desktop to +-32767).
const MAX_POSITION: f32 = 32_000.0;
/// A saved position must leave this much of the window's width on the left edge's right side,
/// so that part of the title bar stays reachable.
const MIN_VISIBLE_WIDTH: f32 = 100.0;
/// A saved position may put the top edge at most this far above the desktop origin.
const TITLE_BAR_MARGIN: f32 = 50.0;

/// Where and how large the window opens.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WindowPlacement {
    /// Outer position in points; `None` lets the operating system choose.
    pub position: Option<[f32; 2]>,
    /// Inner size in points, at least [`MIN_WINDOW_SIZE`].
    pub size: [f32; 2],
}

/// Turns the saved `ui.window` (`[left, top, width, height]`: outer position and inner size,
/// in points at UI scale 1) into a placement that is safe to apply.
///
/// An unusable size falls back to [`DEFAULT_WINDOW_SIZE`] and a size below the minimum is
/// raised to it. A position that is not finite, lies outside every desktop, or would leave the
/// title bar unreachable (too far above the desktop or with less than 100 points of the window
/// to the right of the desktop's left edge) is dropped, so the operating system places the
/// window. Monitors are unknown before the window exists, so a position on a monitor that is
/// no longer connected cannot be detected here.
pub fn placement_from_saved(saved: Option<[f32; 4]>) -> WindowPlacement {
    let Some([left, top, width, height]) = saved else {
        return WindowPlacement {
            position: None,
            size: DEFAULT_WINDOW_SIZE,
        };
    };
    let size = if width.is_finite() && height.is_finite() {
        [
            width.clamp(MIN_WINDOW_SIZE[0], MAX_WINDOW_EXTENT),
            height.clamp(MIN_WINDOW_SIZE[1], MAX_WINDOW_EXTENT),
        ]
    } else {
        DEFAULT_WINDOW_SIZE
    };
    let position = plausible_position([left, top], size[0]).then_some([left, top]);
    WindowPlacement { position, size }
}

fn plausible_position(position: [f32; 2], width: f32) -> bool {
    let [left, top] = position;
    left.is_finite()
        && top.is_finite()
        && left.abs() <= MAX_POSITION
        && top.abs() <= MAX_POSITION
        && left + width >= MIN_VISIBLE_WIDTH
        && top >= -TITLE_BAR_MARGIN
}

/// The `ui.window` value for the current outer position and inner size (points at UI scale 1),
/// or `None` when they must not be remembered: not finite, below the minimum size, above the
/// maximum, or a position the next start would not restore (a minimized window reports
/// a far off-screen position).
pub fn window_to_save(position: [f32; 2], size: [f32; 2]) -> Option<[f32; 4]> {
    let size_ok = size.iter().all(|v| v.is_finite())
        && size[0] >= MIN_WINDOW_SIZE[0] - 1.0
        && size[1] >= MIN_WINDOW_SIZE[1] - 1.0
        && size[0] <= MAX_WINDOW_EXTENT
        && size[1] <= MAX_WINDOW_EXTENT;
    (size_ok && plausible_position(position, size[0])).then_some([
        position[0],
        position[1],
        size[0],
        size[1],
    ])
}

// ---------------------------------------------------------------------------------------------
// Column layout
// ---------------------------------------------------------------------------------------------

/// Narrowest a column can be dragged to; also the floor for a saved width.
pub const MIN_COLUMN_WIDTH: f32 = 40.0;
const MAX_COLUMN_WIDTH: f32 = 4_000.0;

/// Column id of a header label: lower-case, every run of other characters becomes `_`
/// (`Last error` -> `last_error`).
pub fn column_id(label: &str) -> String {
    let mut id = String::with_capacity(label.len());
    for c in label.chars() {
        if c.is_alphanumeric() {
            id.extend(c.to_lowercase());
        } else if !id.ends_with('_') && !id.is_empty() {
            id.push('_');
        }
    }
    while id.ends_with('_') {
        id.pop();
    }
    id
}

fn clean_width(width: f32, default: f32) -> f32 {
    if width.is_finite() {
        width.clamp(MIN_COLUMN_WIDTH, MAX_COLUMN_WIDTH)
    } else {
        default
    }
}

/// True when `layout` already is exactly the layout `defaults` describe: same ids in the same
/// order, usable widths, at least one visible column and at most one sorted column (none when
/// the table is not sortable).
fn layout_is_current(layout: &[ColumnState], defaults: &[(&str, f32)], sortable: bool) -> bool {
    layout.len() == defaults.len()
        && layout
            .iter()
            .zip(defaults)
            .all(|(c, (label, _))| c.id == column_id(label) && clean_width(c.width, 0.0) == c.width)
        && layout.iter().any(|c| c.visible)
        && layout.iter().filter(|c| c.sort.is_some()).count() <= usize::from(sortable)
}

/// Builds the layout of a table from what was saved and what the program defines now.
///
/// The result follows `defaults` (header label, default width) in order. A saved column
/// supplies width, visibility and sort direction for the column with the same id; saved
/// columns that no longer exist are dropped and new columns start visible at their default
/// width. Widths are kept finite and between [`MIN_COLUMN_WIDTH`] and 4000. If no column is
/// visible, all are shown. Only the first sorted column keeps its sort, and none does in a
/// table that cannot be sorted.
pub fn merge_columns(
    saved: &[ColumnState],
    defaults: &[(&str, f32)],
    sortable: bool,
) -> Vec<ColumnState> {
    let mut merged: Vec<ColumnState> = defaults
        .iter()
        .map(|(label, default_width)| {
            let id = column_id(label);
            match saved.iter().find(|c| c.id == id) {
                Some(c) => ColumnState {
                    width: clean_width(c.width, *default_width),
                    ..c.clone()
                },
                None => ColumnState {
                    id,
                    width: *default_width,
                    visible: true,
                    sort: None,
                },
            }
        })
        .collect();
    if !merged.iter().any(|c| c.visible) {
        for c in &mut merged {
            c.visible = true;
        }
    }
    let mut seen = false;
    for c in &mut merged {
        if c.sort.is_some() && (seen || !sortable || !c.visible) {
            c.sort = None;
        }
        seen |= c.sort.is_some();
    }
    merged
}

/// The layout of `table`, rebuilt from `defaults` first when it is missing or out of date.
/// Cheap when it is current: no allocation.
pub fn ensure_layout<'a>(
    layouts: &'a mut BTreeMap<String, Vec<ColumnState>>,
    table: &str,
    defaults: &[(&str, f32)],
    sortable: bool,
) -> &'a mut Vec<ColumnState> {
    let current = layouts
        .get(table)
        .is_some_and(|layout| layout_is_current(layout, defaults, sortable));
    if !current {
        let saved = layouts.get(table).map(Vec::as_slice).unwrap_or_default();
        let merged = merge_columns(saved, defaults, sortable);
        layouts.insert(table.to_string(), merged);
    }
    layouts.get_mut(table).expect("layout was just inserted")
}

/// The sorted column of a layout as (column id, direction).
pub fn sort_of(layout: &[ColumnState]) -> Option<(&str, SortDirection)> {
    layout
        .iter()
        .find_map(|c| c.sort.map(|direction| (c.id.as_str(), direction)))
}

/// Header click on column `index`: ascending, then descending, then the default order again.
/// Sorting by another column replaces the previous sort.
pub fn cycle_sort(layout: &mut [ColumnState], index: usize) {
    let Some(column) = layout.get(index) else {
        return;
    };
    let next = match column.sort {
        None => Some(SortDirection::Ascending),
        Some(SortDirection::Ascending) => Some(SortDirection::Descending),
        Some(SortDirection::Descending) => None,
    };
    for c in layout.iter_mut() {
        c.sort = None;
    }
    layout[index].sort = next;
}

/// Shows or hides column `index`. Refused (false) when it would hide the last visible column.
/// Hiding the sorted column also returns the table to its default order.
pub fn set_column_visible(layout: &mut [ColumnState], index: usize, visible: bool) -> bool {
    if index >= layout.len() {
        return false;
    }
    if !visible && layout[index].visible && layout.iter().filter(|c| c.visible).count() <= 1 {
        return false;
    }
    layout[index].visible = visible;
    if !visible {
        layout[index].sort = None;
    }
    true
}

// ---------------------------------------------------------------------------------------------
// Status row colours
// ---------------------------------------------------------------------------------------------

/// Background and text colour of a tinted row, both 0xRRGGBB.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RowColors {
    pub background: u32,
    pub text: u32,
}

fn channel(value: u32) -> f64 {
    let c = f64::from(value & 0xFF) / 255.0;
    if c <= 0.03928 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn luminance(rgb: u32) -> f64 {
    0.2126 * channel(rgb >> 16) + 0.7152 * channel(rgb >> 8) + 0.0722 * channel(rgb)
}

/// WCAG contrast ratio between two 0xRRGGBB colours, from 1 to 21.
pub fn contrast_ratio(a: u32, b: u32) -> f64 {
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

/// Black or white, whichever reads better on `background`.
pub fn readable_text(background: u32) -> u32 {
    if contrast_ratio(background, 0x000000) >= contrast_ratio(background, 0xFFFFFF) {
        0x000000
    } else {
        0xFFFFFF
    }
}

/// The colours of a row whose status maps to `slot`, or `None` for an untinted row: status
/// colours are off, or the slot is a neutral one the user did not customise.
///
/// A text colour set for the slot is used as is; otherwise the text is black or white,
/// whichever contrasts more with the background.
///
/// Takes `ui.colored_rows` and `ui.colors` separately so a table can hold its column layout
/// (`ui.columns`) mutably while its rows are being coloured.
pub fn row_colors(
    colored_rows: bool,
    overrides: &BTreeMap<String, SlotColor>,
    slot: ColorSlot,
) -> Option<RowColors> {
    if !colored_rows {
        return None;
    }
    let custom = overrides.get(&format!("{slot:?}")).copied();
    if slot.is_neutral() && custom.is_none() {
        return None;
    }
    let color = custom.unwrap_or(SlotColor {
        background: slot.default_background(),
        text: None,
    });
    Some(RowColors {
        background: color.background,
        text: color
            .text
            .unwrap_or_else(|| readable_text(color.background)),
    })
}

/// Slot of a hashed file row. A CRC mismatch outranks an unidentified file, which outranks a
/// watched one, which outranks one that is merely in MyList. The same facts appear as text in
/// the table (`Mismatch`, `Unknown`, `lid N`, `watched`).
pub fn local_file_slot(file: &LocalFile) -> ColorSlot {
    if file.crc_mismatch() {
        ColorSlot::LocalFileInvalidCrc
    } else if !file.known() {
        ColorSlot::LocalFileUnknown
    } else if file.in_mylist() && file.viewed {
        ColorSlot::Watched
    } else if file.in_mylist() {
        ColorSlot::LocalFileInMylist
    } else {
        ColorSlot::LocalFile
    }
}

/// Slot of an anime row: fully watched, else in MyList, else plain.
pub fn anime_slot(anime: &Anime, state: Option<&AnimeMylistState>) -> ColorSlot {
    match state {
        Some(s) if s.all_watched(anime) => ColorSlot::Watched,
        Some(_) => ColorSlot::AnimeInMylist,
        None => ColorSlot::Anime,
    }
}

/// Slot of a MyList entry row: watched, else plain.
pub fn mylist_slot(watched: bool) -> ColorSlot {
    if watched {
        ColorSlot::Watched
    } else {
        ColorSlot::File
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::UiSettings;

    fn col(id: &str, width: f32, visible: bool, sort: Option<SortDirection>) -> ColumnState {
        ColumnState {
            id: id.into(),
            width,
            visible,
            sort,
        }
    }

    const DEFAULTS: [(&str, f32); 3] = [("Title", 300.0), ("Short name", 100.0), ("ID", 60.0)];

    #[test]
    fn no_saved_window_opens_at_the_default_size_and_lets_the_system_place_it() {
        assert_eq!(
            placement_from_saved(None),
            WindowPlacement {
                position: None,
                size: DEFAULT_WINDOW_SIZE
            }
        );
    }

    #[test]
    fn a_saved_window_is_restored_exactly() {
        let placement = placement_from_saved(Some([120.0, 80.0, 1400.0, 900.0]));
        assert_eq!(placement.position, Some([120.0, 80.0]));
        assert_eq!(placement.size, [1400.0, 900.0]);
    }

    #[test]
    fn a_saved_size_is_kept_between_the_minimum_and_a_sane_maximum() {
        let small = placement_from_saved(Some([10.0, 10.0, 300.0, 250.0]));
        assert_eq!(small.size, MIN_WINDOW_SIZE);
        let huge = placement_from_saved(Some([10.0, 10.0, 1.0e9, 1.0e9]));
        assert_eq!(huge.size, [MAX_WINDOW_EXTENT, MAX_WINDOW_EXTENT]);
        let broken = placement_from_saved(Some([10.0, 10.0, f32::NAN, 800.0]));
        assert_eq!(broken.size, DEFAULT_WINDOW_SIZE);
    }

    #[test]
    fn positions_that_hide_the_window_are_dropped_but_the_size_is_kept() {
        for position in [
            [-31_000.0, -31_000.0], // where Windows parks minimized windows
            [-5_000.0, 100.0],      // left of every desktop
            [40_000.0, 100.0],      // right of every desktop
            [100.0, -400.0],        // title bar above the desktop
            [f32::NAN, 100.0],
            [100.0, f32::INFINITY],
        ] {
            let placement = placement_from_saved(Some([position[0], position[1], 1280.0, 800.0]));
            assert_eq!(placement.position, None, "{position:?}");
            assert_eq!(placement.size, [1280.0, 800.0]);
        }
        // A window partly left of the desktop (a left neighbour monitor) is still reachable.
        let partly = placement_from_saved(Some([-300.0, 0.0, 1280.0, 800.0]));
        assert_eq!(partly.position, Some([-300.0, 0.0]));
    }

    #[test]
    fn only_restorable_window_geometry_is_saved() {
        assert_eq!(
            window_to_save([50.0, 60.0], [1000.0, 700.0]),
            Some([50.0, 60.0, 1000.0, 700.0])
        );
        assert_eq!(
            window_to_save([-32_000.0, -32_000.0], [1000.0, 700.0]),
            None
        );
        assert_eq!(window_to_save([50.0, 60.0], [100.0, 40.0]), None);
        assert_eq!(window_to_save([50.0, 60.0], [f32::NAN, 700.0]), None);
        // What is saved restores to the same geometry.
        let saved = window_to_save([-300.0, 20.0], [1111.0, 777.0]);
        let placement = placement_from_saved(saved);
        assert_eq!(placement.position, Some([-300.0, 20.0]));
        assert_eq!(placement.size, [1111.0, 777.0]);
    }

    #[test]
    fn column_ids_come_from_the_header_text() {
        assert_eq!(column_id("Title"), "title");
        assert_eq!(column_id("Short name"), "short_name");
        assert_eq!(column_id("ID"), "id");
        assert_eq!(column_id("Last error"), "last_error");
        assert_eq!(column_id("  Odd -- label! "), "odd_label");
    }

    #[test]
    fn defaults_give_every_column_visible_at_its_default_width_and_no_sort() {
        let layout = merge_columns(&[], &DEFAULTS, true);
        assert_eq!(
            layout,
            vec![
                col("title", 300.0, true, None),
                col("short_name", 100.0, true, None),
                col("id", 60.0, true, None),
            ]
        );
    }

    #[test]
    fn saved_columns_keep_width_visibility_and_sort_and_follow_the_current_order() {
        let saved = vec![
            col("id", 75.0, false, None),
            col("title", 420.0, true, Some(SortDirection::Descending)),
            col("removed_column", 90.0, true, None),
        ];
        let layout = merge_columns(&saved, &DEFAULTS, true);
        assert_eq!(
            layout,
            vec![
                col("title", 420.0, true, Some(SortDirection::Descending)),
                col("short_name", 100.0, true, None), // new column
                col("id", 75.0, false, None),
            ]
        );
    }

    #[test]
    fn broken_saved_values_are_repaired() {
        let saved = vec![
            col("title", f32::NAN, true, Some(SortDirection::Ascending)),
            col("short_name", 5.0, true, Some(SortDirection::Descending)),
            col("id", 1.0e9, true, None),
        ];
        let layout = merge_columns(&saved, &DEFAULTS, true);
        assert_eq!(layout[0].width, 300.0); // not finite: the default
        assert_eq!(layout[1].width, MIN_COLUMN_WIDTH);
        assert_eq!(layout[2].width, MAX_COLUMN_WIDTH);
        // Only the first sorted column keeps its sort.
        assert_eq!(layout[0].sort, Some(SortDirection::Ascending));
        assert_eq!(layout[1].sort, None);
        // A table that cannot be sorted drops every saved sort.
        assert!(
            merge_columns(&saved, &DEFAULTS, false)
                .iter()
                .all(|c| c.sort.is_none())
        );
    }

    #[test]
    fn a_layout_with_every_column_hidden_shows_all_of_them() {
        let saved = vec![
            col("title", 300.0, false, None),
            col("short_name", 100.0, false, None),
            col("id", 60.0, false, None),
        ];
        assert!(
            merge_columns(&saved, &DEFAULTS, true)
                .iter()
                .all(|c| c.visible)
        );
    }

    #[test]
    fn ensure_layout_keeps_a_current_layout_and_repairs_a_stale_one() {
        let mut layouts = BTreeMap::new();
        layouts.insert(
            "groups".to_string(),
            vec![
                col("title", 410.0, true, None),
                col("short_name", 100.0, true, None),
                col("id", 60.0, true, None),
            ],
        );
        let kept = ensure_layout(&mut layouts, "groups", &DEFAULTS, true);
        assert_eq!(kept[0].width, 410.0);

        // The program grew a column: the saved widths survive, the new column is appended.
        let wider = [
            ("Title", 300.0),
            ("Short name", 100.0),
            ("ID", 60.0),
            ("Votes", 80.0),
        ];
        let grown = ensure_layout(&mut layouts, "groups", &wider, true);
        assert_eq!(grown.len(), 4);
        assert_eq!(grown[0].width, 410.0);
        assert_eq!(grown[3], col("votes", 80.0, true, None));

        // A table never seen before is created from its defaults.
        let fresh = ensure_layout(&mut layouts, "genres", &DEFAULTS, true);
        assert_eq!(fresh.len(), 3);
        assert!(layouts.contains_key("genres"));
    }

    #[test]
    fn header_clicks_cycle_ascending_descending_default_and_move_between_columns() {
        let mut layout = merge_columns(&[], &DEFAULTS, true);
        cycle_sort(&mut layout, 0);
        assert_eq!(sort_of(&layout), Some(("title", SortDirection::Ascending)));
        cycle_sort(&mut layout, 0);
        assert_eq!(sort_of(&layout), Some(("title", SortDirection::Descending)));
        cycle_sort(&mut layout, 0);
        assert_eq!(sort_of(&layout), None);
        cycle_sort(&mut layout, 0);
        cycle_sort(&mut layout, 2);
        assert_eq!(sort_of(&layout), Some(("id", SortDirection::Ascending)));
        assert_eq!(layout.iter().filter(|c| c.sort.is_some()).count(), 1);
        cycle_sort(&mut layout, 99); // out of range: nothing happens
    }

    #[test]
    fn the_last_visible_column_cannot_be_hidden_and_hiding_the_sorted_column_clears_the_sort() {
        let mut layout = merge_columns(&[], &DEFAULTS, true);
        cycle_sort(&mut layout, 1);
        assert!(set_column_visible(&mut layout, 1, false));
        assert_eq!(sort_of(&layout), None);
        assert!(set_column_visible(&mut layout, 0, false));
        assert!(
            !set_column_visible(&mut layout, 2, false),
            "last visible column"
        );
        assert!(layout[2].visible);
        assert!(set_column_visible(&mut layout, 0, true));
    }

    #[test]
    fn text_is_black_or_white_by_contrast_and_every_default_colour_is_readable() {
        assert_eq!(readable_text(0x000000), 0xFFFFFF);
        assert_eq!(readable_text(0xFFFFFF), 0x000000);
        for slot in ColorSlot::ALL {
            let background = slot.default_background();
            assert!(
                contrast_ratio(background, readable_text(background)) >= 4.5,
                "{slot:?} default colour is hard to read"
            );
        }
    }

    #[test]
    fn rows_are_tinted_by_status_only_when_status_colours_are_on() {
        let mut ui = UiSettings::default();
        let colors = row_colors(ui.colored_rows, &ui.colors, ColorSlot::LocalFileUnknown).unwrap();
        assert_eq!(
            colors.background,
            ColorSlot::LocalFileUnknown.default_background()
        );
        assert_eq!(colors.text, readable_text(colors.background));
        ui.colored_rows = false;
        assert_eq!(
            row_colors(ui.colored_rows, &ui.colors, ColorSlot::LocalFileUnknown),
            None
        );
    }

    #[test]
    fn plain_rows_stay_untinted_until_the_user_customises_the_slot() {
        let mut ui = UiSettings::default();
        assert_eq!(
            row_colors(ui.colored_rows, &ui.colors, ColorSlot::LocalFile),
            None
        );
        ui.set_color(
            ColorSlot::LocalFile,
            crate::settings::SlotColor {
                background: 0x102030,
                text: Some(0xEEEEEE),
            },
        );
        assert_eq!(
            row_colors(ui.colored_rows, &ui.colors, ColorSlot::LocalFile),
            Some(RowColors {
                background: 0x102030,
                text: 0xEEEEEE
            })
        );
    }

    #[test]
    fn a_customised_status_colour_replaces_the_default() {
        let mut ui = UiSettings::default();
        ui.set_color(
            ColorSlot::Watched,
            crate::settings::SlotColor {
                background: 0x000080,
                text: None,
            },
        );
        let colors = row_colors(ui.colored_rows, &ui.colors, ColorSlot::Watched).unwrap();
        assert_eq!(colors.background, 0x000080);
        assert_eq!(colors.text, 0xFFFFFF);
    }

    fn local(fid: Option<i64>, lid: Option<i64>, viewed: bool) -> LocalFile {
        LocalFile {
            crc32: Some("AABBCCDD".into()),
            anidb_crc: Some("aabbccdd".into()),
            fid,
            lid,
            viewed,
            ..LocalFile::default()
        }
    }

    #[test]
    fn hashed_file_rows_rank_crc_mismatch_over_unknown_over_watched_over_in_mylist() {
        let mut f = local(None, None, false);
        f.anidb_crc = Some("00000000".into());
        assert_eq!(local_file_slot(&f), ColorSlot::LocalFileInvalidCrc);
        // Unidentified files have no AniDB CRC; unknown is the status.
        assert_eq!(
            local_file_slot(&local(None, None, false)),
            ColorSlot::LocalFileUnknown
        );
        assert_eq!(
            local_file_slot(&local(Some(1), Some(2), true)),
            ColorSlot::Watched
        );
        assert_eq!(
            local_file_slot(&local(Some(1), Some(2), false)),
            ColorSlot::LocalFileInMylist
        );
        assert_eq!(
            local_file_slot(&local(Some(1), None, true)),
            ColorSlot::LocalFile
        );
    }

    #[test]
    fn anime_rows_rank_watched_over_in_mylist_over_plain() {
        let anime = Anime {
            episodes: 12,
            ..Anime::default()
        };
        assert_eq!(anime_slot(&anime, None), ColorSlot::Anime);
        let partial = AnimeMylistState {
            eps: 12,
            watched_eps: 5,
            ..AnimeMylistState::default()
        };
        assert_eq!(anime_slot(&anime, Some(&partial)), ColorSlot::AnimeInMylist);
        let done = AnimeMylistState {
            eps: 12,
            watched_eps: 12,
            ..AnimeMylistState::default()
        };
        assert_eq!(anime_slot(&anime, Some(&done)), ColorSlot::Watched);
    }

    #[test]
    fn mylist_rows_are_tinted_when_watched() {
        assert_eq!(mylist_slot(true), ColorSlot::Watched);
        assert_eq!(mylist_slot(false), ColorSlot::File);
    }
}
