//! The virtualized data table shared by every page, with its saved column layout (widths,
//! visibility, sort order), header sorting, the column chooser and status row tinting.
//!
//! The rules (merging a saved layout, cycling the sort, picking colours) live in
//! `animatic_core::ui_state`; this module draws them and sorts the row indices.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use animatic_core::library::LibraryProjection;
use animatic_core::settings::{ColumnState, RowStyle, SortDirection};
use animatic_core::ui_state::{self, MIN_COLUMN_WIDTH, RowColors};
use eframe::egui::{self, Align, Color32, Layout, RichText, Stroke, pos2};
use egui_extras::{Column, TableBuilder};

/// The sorted column of a table as (column id, direction); `None` keeps the default order.
pub type SortKey = Option<(String, SortDirection)>;

/// The sort currently chosen for `table`, read from the saved layout.
pub fn sort_key(layouts: &BTreeMap<String, Vec<ColumnState>>, table: &str) -> SortKey {
    layouts
        .get(table)
        .and_then(|layout| ui_state::sort_of(layout))
        .map(|(id, direction)| (id.to_string(), direction))
}

/// Virtualized table: a fixed header and a body that only lays out the visible rows. Cells
/// truncate instead of wrapping so every row keeps the height the body was told. The columns
/// have a combined minimum width; when the page is narrower the whole table (header and rows
/// together) scrolls horizontally so every column stays reachable.
///
/// Column widths, visibility and (unless [`DataTable::fixed_order`]) the sort order come from
/// and go back to `layouts` (`ui.columns`), keyed by the table id. A click on a header sorts
/// by that column (ascending, descending, default order); a right click opens the column
/// chooser.
pub struct DataTable<'a, 'c> {
    ui: &'a mut egui::Ui,
    id: &'c str,
    columns: &'c [(&'c str, f32)],
    layouts: &'c mut BTreeMap<String, Vec<ColumnState>>,
    sortable: bool,
}

pub fn data_table<'a, 'c>(
    ui: &'a mut egui::Ui,
    id: &'c str,
    columns: &'c [(&'c str, f32)],
    layouts: &'c mut BTreeMap<String, Vec<ColumnState>>,
) -> DataTable<'a, 'c> {
    DataTable {
        ui,
        id,
        columns,
        layouts,
        sortable: true,
    }
}

/// What the header asked for during a frame.
#[derive(Default)]
struct HeaderActions {
    sort_by: Option<usize>,
    show_hide: Option<(usize, bool)>,
}

impl DataTable<'_, '_> {
    /// The row order carries meaning (the queue runs in id order), so headers do not sort.
    pub fn fixed_order(mut self) -> Self {
        self.sortable = false;
        self
    }

    pub fn body(self, add_body: impl FnOnce(TableBodyView<'_>)) {
        let Self {
            ui,
            id,
            columns,
            layouts,
            sortable,
        } = self;
        let layout = ui_state::ensure_layout(layouts, id, columns, sortable);
        let header_height = row_height(ui);
        let spacing = ui.spacing().item_spacing.x;
        let shown: Vec<bool> = layout.iter().map(|c| c.visible).collect();
        let visible: Vec<usize> = (0..layout.len()).filter(|&i| shown[i]).collect();
        let minimum: f32 = visible.iter().map(|&i| layout[i].width + spacing).sum();
        let width = ui.available_width().max(minimum);
        let last = visible.len().saturating_sub(1);
        let mut actions = HeaderActions::default();
        let mut measured: Vec<(usize, f32)> = Vec::with_capacity(visible.len());
        egui::ScrollArea::horizontal()
            .id_salt((id, "horizontal"))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_width(width);
                let mut table = TableBuilder::new(ui)
                    .id_salt(id)
                    .striped(true)
                    .resizable(true)
                    .cell_layout(Layout::left_to_right(Align::Center))
                    .auto_shrink([false, false])
                    .min_scrolled_height(0.0);
                for (n, &i) in visible.iter().enumerate() {
                    let column = if n == last {
                        // The last column fills the remaining width; its own width is only
                        // the least it keeps.
                        Column::remainder().at_least(columns[i].1)
                    } else {
                        Column::initial(layout[i].width).at_least(MIN_COLUMN_WIDTH)
                    };
                    table = table.column(column.clip(true));
                }
                let table = table.header(header_height, |mut header| {
                    for (n, &i) in visible.iter().enumerate() {
                        let mut cell_width = f32::NAN;
                        header.col(|ui| {
                            // The room the table gave this cell is the column width.
                            cell_width = ui.max_rect().width();
                            header_cell(ui, columns, layout, i, sortable, &mut actions);
                        });
                        if n != last {
                            measured.push((i, cell_width));
                        }
                    }
                });
                table.body(|body| add_body(TableBodyView { body, shown }));
            });
        // Resizing writes back the width the header cell actually has.
        for (i, cell_width) in measured {
            if cell_width.is_finite()
                && cell_width >= MIN_COLUMN_WIDTH
                && (layout[i].width - cell_width).abs() > 0.5
            {
                layout[i].width = cell_width;
            }
        }
        if let Some(i) = actions.sort_by {
            ui_state::cycle_sort(layout, i);
            ui.ctx().request_repaint();
        }
        if let Some((i, visible)) = actions.show_hide {
            ui_state::set_column_visible(layout, i, visible);
            ui.ctx().request_repaint();
        }
    }
}

/// Header of column `i`: its label (a button that sorts) and the column chooser menu.
fn header_cell(
    ui: &mut egui::Ui,
    columns: &[(&str, f32)],
    layout: &[ColumnState],
    i: usize,
    sortable: bool,
    actions: &mut HeaderActions,
) {
    let label = columns[i].0;
    let direction = layout[i].sort;
    let text = match direction {
        Some(SortDirection::Ascending) => format!("{label} \u{25B2}"),
        Some(SortDirection::Descending) => format!("{label} \u{25BC}"),
        None => label.to_string(),
    };
    let button = egui::Button::new(RichText::new(text).strong())
        .frame(false)
        .truncate();
    let hint = match (sortable, direction) {
        (false, _) => "Right-click to choose columns".to_string(),
        (true, None) => format!("Sort by {label}. Right-click to choose columns"),
        (true, Some(SortDirection::Ascending)) => {
            format!(
                "Sorted by {label}, ascending. Click for descending. Right-click to choose columns"
            )
        }
        (true, Some(SortDirection::Descending)) => {
            format!(
                "Sorted by {label}, descending. Click for the default order. Right-click to choose columns"
            )
        }
    };
    let response = ui.add(button).on_hover_text(hint);
    if sortable && response.clicked() {
        actions.sort_by = Some(i);
    }
    response.context_menu(|ui| {
        ui.label(RichText::new("Columns").strong());
        let visible_count = layout.iter().filter(|c| c.visible).count();
        for (index, (name, _)) in columns.iter().enumerate() {
            let mut visible = layout[index].visible;
            let only_one = visible && visible_count <= 1;
            let toggled = ui
                .add_enabled(!only_one, egui::Checkbox::new(&mut visible, *name))
                .on_disabled_hover_text("At least one column must stay visible")
                .changed();
            if toggled {
                actions.show_hide = Some((index, visible));
            }
        }
    });
}

pub fn row_height(ui: &egui::Ui) -> f32 {
    ui.spacing().interact_size.y
}

/// The body of a [`DataTable`]: rows whose cells skip the hidden columns.
pub struct TableBodyView<'a> {
    body: egui_extras::TableBody<'a>,
    shown: Vec<bool>,
}

impl TableBodyView<'_> {
    pub fn rows(
        self,
        row_height: f32,
        total_rows: usize,
        mut add_row: impl FnMut(Row<'_, '_, '_>),
    ) {
        let shown = self.shown;
        self.body.rows(row_height, total_rows, |row| {
            add_row(Row {
                row,
                shown: &shown,
                next: 0,
                tint: None,
                marked: false,
            });
        });
    }
}

/// One row. Call [`Row::col`] once per column of the table, in order; cells of hidden columns
/// are skipped.
pub struct Row<'s, 'a, 'b> {
    row: egui_extras::TableRow<'a, 'b>,
    shown: &'s [bool],
    next: usize,
    tint: Option<RowColors>,
    /// The soft marker bar has been painted at the left edge of the row.
    marked: bool,
}

impl Row<'_, '_, '_> {
    pub fn index(&self) -> usize {
        self.row.index()
    }

    /// Tints the cells added after this call: a soft band with a marker bar, or a solid fill
    /// with readable text on top, as the colours say. `None` leaves the row as it is. Colour
    /// is only ever an extra: the status is also written in the row.
    pub fn tint(&mut self, colors: Option<RowColors>) {
        self.tint = colors;
    }

    pub fn col(&mut self, add_cell_contents: impl FnOnce(&mut egui::Ui)) {
        let shown = self.shown.get(self.next).copied().unwrap_or(true);
        self.next += 1;
        if !shown {
            return;
        }
        let tint = self.tint;
        let marker = !std::mem::replace(&mut self.marked, true);
        self.row.col(|ui| {
            if let Some(colors) = tint {
                paint_tint(ui, colors, marker);
            }
            add_cell_contents(ui);
        });
    }
}

fn rgb(value: u32) -> Color32 {
    Color32::from_rgb((value >> 16) as u8, (value >> 8) as u8, value as u8)
}

fn packed(color: Color32) -> u32 {
    (u32::from(color.r()) << 16) | (u32::from(color.g()) << 8) | u32::from(color.b())
}

/// Width in points of the full-colour marker bar at the left edge of a soft row.
const MARKER_WIDTH: f32 = 3.0;

/// Soft row band: the panel colour blended toward the status colour `slot`, weaker for the
/// expected states than for those asking for action, and weakened further until the theme
/// `text` colour reads on it at [`ui_state::MIN_TEXT_CONTRAST`].
fn soft_fill(panel: Color32, text: Color32, slot: u32, attention: bool, dark: bool) -> Color32 {
    let mut k: f32 = match (attention, dark) {
        (false, true) => 0.14,
        (false, false) => 0.22,
        (true, true) => 0.26,
        (true, false) => 0.32,
    };
    loop {
        let fill = panel.lerp_to_gamma(rgb(slot), k);
        if k <= 0.0
            || ui_state::contrast_ratio(packed(fill), packed(text)) >= ui_state::MIN_TEXT_CONTRAST
        {
            return fill;
        }
        k = (k - 0.02).max(0.0);
    }
}

/// Paints the whole cell (including the gaps between cells and rows). Soft: the tinted band,
/// plus the marker bar when this is the first visible cell of the row; the cell keeps the
/// theme's text and widget visuals. Solid: the full status colour, with the cell's text in the
/// colour chosen for that background.
fn paint_tint(ui: &mut egui::Ui, colors: RowColors, marker: bool) {
    let half_gap = ui.spacing().item_spacing * 0.5;
    let band = ui.max_rect().expand2(half_gap);
    // `with_clip_rect` only narrows the cell's clip, which left the gaps (and the marker bar,
    // which sits in the gap) unpainted; the band needs the clip widened by the same half gap.
    let mut painter = ui.painter().clone();
    painter.set_clip_rect(ui.clip_rect().expand2(half_gap));
    let status = rgb(colors.background);
    if colors.style == RowStyle::Soft {
        let visuals = ui.visuals();
        let fill = soft_fill(
            visuals.panel_fill,
            visuals.text_color(),
            colors.background,
            colors.attention,
            visuals.dark_mode,
        );
        painter.rect_filled(band, 0.0, fill);
        if marker {
            let bar = egui::Rect::from_min_max(
                band.left_top(),
                pos2(band.left() + MARKER_WIDTH, band.bottom()),
            );
            painter.rect_filled(bar, 0.0, status);
        }
        return;
    }
    painter.rect_filled(band, 0.0, status);
    let text = rgb(colors.text);
    let visuals = ui.visuals_mut();
    visuals.override_text_color = Some(text);
    visuals.hyperlink_color = text;
    // Clickable labels and buttons take their text from the widget visuals instead, and their
    // dark default fill would swallow dark text, so the fill follows the row colour (a little
    // toward the text colour so the widget still stands out; more while hovered or pressed).
    for (widget, mix) in [
        (&mut visuals.widgets.noninteractive, 0.0),
        (&mut visuals.widgets.inactive, 0.12),
        (&mut visuals.widgets.hovered, 0.22),
        (&mut visuals.widgets.active, 0.32),
        (&mut visuals.widgets.open, 0.22),
    ] {
        widget.fg_stroke.color = text;
        let fill = status.lerp_to_gamma(text, mix);
        widget.weak_bg_fill = fill;
        widget.bg_fill = fill;
    }
    // A selected row keeps a visible highlight and readable text on the solid background; the
    // heavier outline in the text colour keeps it apparent on any status colour.
    visuals.selection.bg_fill = status.lerp_to_gamma(text, 0.35);
    visuals.selection.stroke = Stroke::new(2.0, text);
}

/// A sample row for Options → Status colors: a chip of `width` points painted with the very
/// code a table row uses (`None`: the row is not tinted, so the chip is plain), with `text`
/// in it. The soft marker bar is included.
pub fn tint_preview(ui: &mut egui::Ui, colors: Option<RowColors>, width: f32, text: &str) {
    let size = egui::vec2(width, ui.spacing().interact_size.y);
    ui.allocate_ui_with_layout(size, Layout::left_to_right(Align::Center), |ui| {
        ui.set_min_size(size);
        if let Some(colors) = colors {
            paint_tint(ui, colors, true);
        }
        ui.add_space(MARKER_WIDTH + 2.0);
        ui.label(text);
    });
}

// ---------------------------------------------------------------------------------------------
// Sorting
// ---------------------------------------------------------------------------------------------

/// Case-insensitive comparison in which runs of digits compare by value (`ep2` < `ep10`).
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let run = |chars: &mut std::iter::Peekable<std::str::Chars<'_>>| {
                    let mut digits = String::new();
                    while let Some(c) = chars.next_if(char::is_ascii_digit) {
                        digits.push(c);
                    }
                    digits
                };
                let (x, y) = (run(&mut a), run(&mut b));
                let (x, y) = (x.trim_start_matches('0'), y.trim_start_matches('0'));
                let order = x.len().cmp(&y.len()).then_with(|| x.cmp(y));
                if order != Ordering::Equal {
                    return order;
                }
            }
            (Some(x), Some(y)) => {
                a.next();
                b.next();
                let order = x.to_lowercase().cmp(y.to_lowercase());
                if order != Ordering::Equal {
                    return order;
                }
            }
        }
    }
}

/// Stable sort of row indices; equal rows keep their default order, in both directions.
pub(crate) fn sort_rows(
    rows: &mut [usize],
    sort: &SortKey,
    compare: impl Fn(&str, usize, usize) -> Ordering,
) {
    let Some((column, direction)) = sort else {
        return;
    };
    rows.sort_by(|&a, &b| {
        let order = compare(column, a, b);
        match direction {
            SortDirection::Ascending => order,
            SortDirection::Descending => order.reverse(),
        }
    });
}

/// Orders the rows of the Anime and Advanced search tables.
pub fn sort_anime(p: &LibraryProjection, rows: &mut [usize], sort: &SortKey) {
    let mylist = |i: usize| {
        p.states
            .get(&p.anime[i].aid)
            .map_or((0, 0), |s| (s.eps, s.files))
    };
    sort_rows(rows, sort, |column, a, b| match column {
        "title" => natural_cmp(&p.keys.anime[a], &p.keys.anime[b]),
        "type" => p.anime[a].type_name().cmp(p.anime[b].type_name()),
        "year" => natural_cmp(&p.anime[a].year, &p.anime[b].year),
        "episodes" => p.anime[a].episodes.cmp(&p.anime[b].episodes),
        "rating" => p.anime[a].rating.cmp(&p.anime[b].rating),
        "mylist" => mylist(a).cmp(&mylist(b)),
        _ => Ordering::Equal,
    });
}

/// Orders the rows of the Hashed files table.
pub fn sort_local(p: &LibraryProjection, rows: &mut [usize], sort: &SortKey) {
    // The search key is the lower-cased path, so no per-comparison allocation.
    let file_name = |i: usize| {
        let path = p.keys.local[i].as_str();
        path.rsplit(['/', '\\']).next().unwrap_or(path)
    };
    let crc = |i: usize| {
        let f = &p.local[i];
        if f.crc_mismatch() {
            "mismatch".to_string()
        } else {
            f.crc32.as_deref().unwrap_or_default().to_ascii_lowercase()
        }
    };
    sort_rows(rows, sort, |column, a, b| match column {
        "file" => natural_cmp(file_name(a), file_name(b)),
        "size" => p.local[a].size.cmp(&p.local[b].size),
        "anidb" => p.local[a].fid.cmp(&p.local[b].fid),
        "mylist" => p.local[a].lid.cmp(&p.local[b].lid),
        "crc" => crc(a).cmp(&crc(b)),
        "path" => natural_cmp(&p.keys.local[a], &p.keys.local[b]),
        _ => Ordering::Equal,
    });
}

/// Orders the rows of the Groups table.
pub fn sort_groups(p: &LibraryProjection, rows: &mut [usize], sort: &SortKey) {
    sort_rows(rows, sort, |column, a, b| match column {
        "name" => natural_cmp(&p.keys.groups[a].0, &p.keys.groups[b].0),
        "short_name" => natural_cmp(&p.keys.groups[a].1, &p.keys.groups[b].1),
        "rating" => p.groups[a].rating.cmp(&p.groups[b].rating),
        "votes" => p.groups[a].votes.cmp(&p.groups[b].votes),
        "website" => p.groups[a].url.is_some().cmp(&p.groups[b].url.is_some()),
        _ => Ordering::Equal,
    });
}

/// Orders the rows of the Genres table.
pub fn sort_genres(p: &LibraryProjection, rows: &mut [usize], sort: &SortKey) {
    sort_rows(rows, sort, |column, a, b| match column {
        "genre" => natural_cmp(&p.keys.genres[a], &p.keys.genres[b]),
        "anime" => p.genres[a].2.cmp(&p.genres[b].2),
        _ => Ordering::Equal,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use animatic_core::library::{Anime, Group, LocalFile};

    fn ascending(column: &str) -> SortKey {
        Some((column.to_string(), SortDirection::Ascending))
    }

    fn descending(column: &str) -> SortKey {
        Some((column.to_string(), SortDirection::Descending))
    }

    #[test]
    fn natural_order_reads_numbers_by_value_and_ignores_case() {
        let mut names = vec!["Ep10", "ep2", "EP1", "ep02b", "Ep 3", "ep1"];
        names.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(names, ["Ep 3", "EP1", "ep1", "ep2", "ep02b", "Ep10"]);
        assert_eq!(natural_cmp("a", "A"), Ordering::Equal);
        assert_eq!(natural_cmp("", "a"), Ordering::Less);
        assert_eq!(natural_cmp("007", "7"), Ordering::Equal);
        assert_eq!(natural_cmp("a9", "a10"), Ordering::Less);
    }

    fn anime_projection() -> LibraryProjection {
        let mut p = LibraryProjection::default();
        for (aid, name, episodes, rating) in [
            (1, "Synthetic Show 10", 12, 700),
            (2, "synthetic show 2", 24, 900),
            (3, "Another Synthetic", 12, 500),
        ] {
            p.anime.push(Anime {
                aid,
                name: name.into(),
                episodes,
                rating,
                ..Anime::default()
            });
            p.keys.anime.push(name.to_lowercase());
        }
        p
    }

    #[test]
    fn rows_without_a_sort_keep_the_default_order() {
        let p = anime_projection();
        let mut rows = vec![0, 1, 2];
        sort_anime(&p, &mut rows, &None);
        assert_eq!(rows, [0, 1, 2]);
        sort_anime(&p, &mut rows, &ascending("no_such_column"));
        assert_eq!(rows, [0, 1, 2]);
    }

    #[test]
    fn anime_sort_by_title_uses_natural_order_in_both_directions() {
        let p = anime_projection();
        let mut rows = vec![0, 1, 2];
        sort_anime(&p, &mut rows, &ascending("title"));
        assert_eq!(rows, [2, 1, 0]);
        sort_anime(&p, &mut rows, &descending("title"));
        assert_eq!(rows, [0, 1, 2]);
    }

    #[test]
    fn equal_rows_keep_their_default_order_even_when_descending() {
        let p = anime_projection();
        let mut rows = vec![0, 1, 2];
        sort_anime(&p, &mut rows, &descending("episodes"));
        assert_eq!(rows, [1, 0, 2]);
        let mut rows = vec![0, 1, 2];
        sort_anime(&p, &mut rows, &ascending("episodes"));
        assert_eq!(rows, [0, 2, 1]);
    }

    #[test]
    fn hashed_files_sort_by_file_name_not_by_folder() {
        let mut p = LibraryProjection::default();
        for (path, size) in [("/b/zeta.mkv", 5), ("/a/Alpha.mkv", 9), ("/c/mid.mkv", 7)] {
            p.local.push(LocalFile {
                path: path.into(),
                size,
                ..LocalFile::default()
            });
            p.keys.local.push(path.to_lowercase());
        }
        let mut rows = vec![0, 1, 2];
        sort_local(&p, &mut rows, &ascending("file"));
        assert_eq!(rows, [1, 2, 0]);
        sort_local(&p, &mut rows, &ascending("size"));
        assert_eq!(rows, [0, 2, 1]);
        sort_local(&p, &mut rows, &ascending("path"));
        assert_eq!(rows, [1, 0, 2]);
    }

    #[test]
    fn unidentified_hashed_files_sort_before_identified_ones() {
        let mut p = LibraryProjection::default();
        for fid in [Some(30), None, Some(4)] {
            p.local.push(LocalFile {
                fid,
                ..LocalFile::default()
            });
            p.keys.local.push(String::new());
        }
        let mut rows = vec![0, 1, 2];
        sort_local(&p, &mut rows, &ascending("anidb"));
        assert_eq!(rows, [1, 2, 0]);
    }

    #[test]
    fn groups_sort_by_votes() {
        let mut p = LibraryProjection::default();
        for (name, votes) in [("b", 3), ("a", 10), ("c", 1)] {
            p.groups.push(Group {
                name: name.into(),
                votes,
                ..Group::default()
            });
            p.keys.groups.push((name.into(), String::new()));
        }
        let mut rows = vec![0, 1, 2];
        sort_groups(&p, &mut rows, &descending("votes"));
        assert_eq!(rows, [1, 0, 2]);
        sort_groups(&p, &mut rows, &ascending("name"));
        assert_eq!(rows, [1, 0, 2]);
    }

    #[test]
    fn sort_key_reads_the_saved_layout() {
        let mut layouts = BTreeMap::new();
        assert_eq!(sort_key(&layouts, "groups"), None);
        layouts.insert(
            "groups".to_string(),
            vec![ColumnState {
                id: "votes".into(),
                width: 80.0,
                visible: true,
                sort: Some(SortDirection::Descending),
            }],
        );
        assert_eq!(sort_key(&layouts, "groups"), descending("votes"));
    }

    /// Panel and text colours of both themes: egui's own and every scheme's palettes.
    fn theme_colours() -> Vec<(String, Color32, Color32, bool)> {
        let mut themes = Vec::new();
        for (name, dark) in [("dark", true), ("light", false)] {
            let egui_default = if dark {
                egui::Visuals::dark()
            } else {
                egui::Visuals::light()
            };
            themes.push((
                name.to_string(),
                egui_default.panel_fill,
                egui_default.text_color(),
                dark,
            ));
            for scheme in animatic_core::settings::ColorScheme::ALL {
                let ctx = egui::Context::default();
                crate::style::install(&ctx);
                crate::style::set_scheme(&ctx, scheme);
                let theme = if dark {
                    egui::Theme::Dark
                } else {
                    egui::Theme::Light
                };
                let app = ctx.style_of(theme).visuals.clone();
                themes.push((
                    format!("{name} {}", scheme.label()),
                    app.panel_fill,
                    app.text_color(),
                    dark,
                ));
            }
        }
        themes
    }

    #[test]
    fn soft_row_bands_keep_theme_text_readable_for_every_default_slot() {
        for (theme, panel, text, dark) in theme_colours() {
            for slot in animatic_core::settings::ColorSlot::ALL {
                let fill = soft_fill(
                    panel,
                    text,
                    slot.default_background(),
                    slot.is_attention(),
                    dark,
                );
                assert!(
                    ui_state::contrast_ratio(packed(fill), packed(text)) >= 4.5,
                    "{slot:?} on {theme}"
                );
            }
        }
    }

    #[test]
    fn soft_row_bands_tint_the_panel_and_attention_is_stronger() {
        for (theme, panel, text, dark) in theme_colours()
            .into_iter()
            .filter(|(name, ..)| name.contains(' '))
        {
            let expected = soft_fill(panel, text, 0xFF7070, false, dark);
            let attention = soft_fill(panel, text, 0xFF7070, true, dark);
            assert_ne!(expected, panel, "{theme}: tint must show");
            let distance = |c: Color32| {
                let d = |a: u8, b: u8| i32::from(a).abs_diff(i32::from(b));
                d(c.r(), panel.r()) + d(c.g(), panel.g()) + d(c.b(), panel.b())
            };
            assert!(distance(attention) > distance(expected), "{theme}");
        }
    }

    #[test]
    fn soft_bands_weaken_the_tint_until_the_text_reads() {
        // A white slot colour at the full attention blend would be too light for the text.
        let panel = Color32::from_rgb(30, 32, 37);
        let text = Color32::from_rgb(200, 203, 210);
        let fill = soft_fill(panel, text, 0xFFFFFF, true, true);
        assert!(ui_state::contrast_ratio(packed(fill), packed(text)) >= 4.5);
        assert_ne!(fill, panel);
    }
}
