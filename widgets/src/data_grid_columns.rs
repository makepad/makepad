//! A table's columns as the person arranges them, for any [`DataGrid`] host:
//! which columns are shown, in what order, how wide, and which one sorts.
//!
//! The grid holds no data and knows nothing about what a column means, so
//! every host that lets people choose columns used to carry the same
//! plumbing: a chooser menu raised from a heading, dragged headings folded
//! back into the host's own column order, dragged edges kept per column,
//! columns dropped from the right when the table is too narrow, a sort that
//! cycles on a heading press, and the arrangement written down and read
//! back. This module is that plumbing, once. A host supplies its column
//! type (see [`GridColumn`]), draws its own cells, and stores the text
//! [`GridColumns::serialize`] gives it wherever it keeps settings (off the
//! UI thread).
//!
//! It comes in two parts because they live in different places:
//!
//! * [`GridColumns`] is the arrangement itself: the chosen columns, the
//!   widths the person dragged to, and the sort. It is the host's state
//!   (usually its model), saved and restored.
//! * [`GridColumnFit`] is what one drawn table made of it: the columns that
//!   fit its width, which are also the grid's data columns, in display
//!   order. It is fitted again only when the width or the arrangement
//!   changes, so a sample or a new row never resets an edge being dragged.
//!
//! The grid's own drag order is folded into the arrangement as soon as a
//! heading is dropped ([`GridColumns::fold_order`]), and the next fit hands
//! the grid its columns in that order with an identity column order. So the
//! host's data column `i` is always `fit.columns()[i]`, whatever was
//! dragged.

use crate::{data_grid::DataGrid, makepad_draw::*, menu::MenuRow};
use std::collections::HashMap;
use std::fmt::Debug;
use std::hash::Hash;

/// How a column takes its width when the person has not dragged its edge.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ColumnWidth {
    /// Always this wide.
    Fixed(f64),
    /// A part of the room the fixed columns leave, by `share` against the
    /// other flexible columns, kept within `min..=max`. The table drops
    /// columns before a flexible one goes below `min`.
    Flex { share: f64, min: f64, max: f64 },
    /// `base` wide, and up to `max_extra` more out of room the flexible
    /// columns did not take (sparklines grow a little on a wide table).
    Stretch { base: f64, max_extra: f64 },
}

/// A host's column: a small `Copy` key (usually an enum) that says what it
/// is called and how it behaves. Only the first four methods have to be
/// written.
pub trait GridColumn: Copy + Eq + Hash + Debug + 'static {
    /// The stable name the arrangement is saved under. Never reused for
    /// another column.
    fn code(self) -> &'static str;
    /// The heading.
    fn label(self) -> &'static str;
    fn width(self) -> ColumnWidth;
    /// The chooser's row; the heading by default.
    fn menu_label(self) -> &'static str {
        self.label()
    }
    /// The chooser section the column is listed under. With more than one
    /// section the chooser shows one flyout per section.
    fn section(self) -> Option<&'static str> {
        None
    }
    /// Whether the person may hide it. A column that cannot be hidden is
    /// not in the chooser and is never dropped to fit the width.
    fn hideable(self) -> bool {
        true
    }
    /// Whether its heading can be dragged, and others dropped past it.
    fn movable(self) -> bool {
        true
    }
    fn sortable(self) -> bool {
        true
    }
    /// A first press on the heading sorts falling (biggest first), as
    /// figures read best; text rises by default.
    fn descending_first(self) -> bool {
        false
    }
}

/// Which column orders the rows, and which way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GridSort<K> {
    pub column: K,
    pub descending: bool,
}

/// What a press on the sorted column's heading does next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortCycle {
    /// Turn the direction around; the rows are always sorted.
    Flip,
    /// First way, the other way, then the rows' own order (no sort).
    FlipThenNone,
}

/// The chooser row that puts the defaults back.
pub fn grid_columns_reset_id() -> LiveId {
    live_id!(grid_columns_default)
}

/// The arrangement: see the module docs.
#[derive(Clone, Debug)]
pub struct GridColumns<K: GridColumn> {
    /// Every column the host has, in the chooser's order.
    catalog: &'static [K],
    /// Columns always drawn first, in this order, never chosen, moved or
    /// saved (a pin, a checkbox).
    leading: &'static [K],
    defaults: Vec<K>,
    chosen: Vec<K>,
    widths: HashMap<K, f64>,
    sort: Option<GridSort<K>>,
    /// The sort the rows fall back to when the sorted column goes away, or
    /// the table starts; `None` for rows that have an order of their own.
    fallback: Option<GridSort<K>>,
    cycle: SortCycle,
}

impl<K: GridColumn> GridColumns<K> {
    pub fn new(catalog: &'static [K], leading: &'static [K], defaults: Vec<K>, fallback: Option<GridSort<K>>, cycle: SortCycle) -> Self {
        let mut columns = Self { catalog, leading, defaults: Vec::new(), chosen: Vec::new(), widths: HashMap::new(), sort: fallback, fallback, cycle };
        columns.defaults = columns.complete(defaults);
        columns.chosen = columns.defaults.clone();
        columns
    }

    /// The chosen columns after the leading ones, in display order.
    pub fn chosen(&self) -> &[K] {
        &self.chosen
    }

    pub fn is_chosen(&self, column: K) -> bool {
        self.chosen.contains(&column)
    }

    pub fn leading(&self) -> &[K] {
        self.leading
    }

    /// The width the person dragged `column` to, if they did.
    pub fn width(&self, column: K) -> Option<f64> {
        self.widths.get(&column).copied()
    }

    pub fn sort(&self) -> Option<GridSort<K>> {
        self.sort
    }

    /// Every column the person cannot hide, in the list, before the rest if
    /// it was missing.
    fn complete(&self, mut columns: Vec<K>) -> Vec<K> {
        columns.retain(|c| !self.leading.contains(c));
        let mut seen = Vec::with_capacity(columns.len());
        columns.retain(|c| if seen.contains(c) { false } else { seen.push(*c); true });
        let missing: Vec<K> = self.catalog.iter().copied().filter(|c| !c.hideable() && !self.leading.contains(c) && !columns.contains(c)).collect();
        missing.into_iter().chain(columns).collect()
    }

    /// A heading was pressed: move the sort on for it. Returns `false`, and
    /// changes nothing, for a leading or unsortable column.
    pub fn press_heading(&mut self, column: K) -> bool {
        if !column.sortable() || self.leading.contains(&column) {
            return false;
        }
        let first = column.descending_first();
        self.sort = match (self.sort, self.cycle) {
            (Some(sort), SortCycle::Flip) if sort.column == column => Some(GridSort { column, descending: !sort.descending }),
            (Some(sort), SortCycle::FlipThenNone) if sort.column == column && sort.descending == first => Some(GridSort { column, descending: !first }),
            (Some(sort), SortCycle::FlipThenNone) if sort.column == column => None,
            _ => Some(GridSort { column, descending: first }),
        };
        true
    }

    /// The chooser: every column the person may hide, the shown ones
    /// checked, one flyout per section when there is more than one, and a
    /// row that puts the defaults back.
    pub fn menu_rows(&self) -> Vec<MenuRow> {
        let row = |column: K| MenuRow::new(LiveId::from_str(column.code()), column.menu_label()).checked(self.chosen.contains(&column));
        let listed: Vec<K> = self.catalog.iter().copied().filter(|c| c.hideable() && !self.leading.contains(c)).collect();
        let mut sections: Vec<Option<&'static str>> = Vec::new();
        for column in &listed {
            if !sections.contains(&column.section()) {
                sections.push(column.section());
            }
        }
        let mut rows = Vec::new();
        if sections.len() > 1 {
            for section in sections {
                let title = section.unwrap_or("Other");
                let submenu = listed.iter().copied().filter(|c| c.section() == section).map(row).collect();
                rows.push(MenuRow::new(LiveId::from_str(title), title).submenu(submenu));
            }
        } else {
            rows.extend(listed.iter().copied().map(row));
        }
        rows.push(MenuRow::separator());
        rows.push(MenuRow::new(grid_columns_reset_id(), "Default Columns"));
        rows
    }

    /// A chooser row was picked: show or hide its column (one shown again
    /// goes back beside its neighbours in the chooser's order), or put the
    /// defaults back. Returns whether anything changed.
    pub fn apply_pick(&mut self, id: LiveId) -> bool {
        if id == grid_columns_reset_id() {
            self.chosen = self.defaults.clone();
            self.widths.clear();
        } else {
            let Some(column) = self.catalog.iter().copied().find(|c| LiveId::from_str(c.code()) == id) else { return false };
            if !column.hideable() || self.leading.contains(&column) {
                return false;
            }
            if let Some(at) = self.chosen.iter().position(|c| *c == column) {
                self.chosen.remove(at);
            } else {
                let rank = |c: K| self.catalog.iter().position(|x| *x == c).unwrap_or(usize::MAX);
                let at = self.chosen.iter().position(|c| rank(*c) > rank(column)).unwrap_or(self.chosen.len());
                self.chosen.insert(at, column);
            }
        }
        if self.sort.is_some_and(|sort| !self.chosen.contains(&sort.column)) {
            self.sort = self.fallback;
        }
        true
    }

    /// A heading was dropped somewhere else: `order` is the grid's data
    /// column at each display position (`ColumnOrderChanged`), over the
    /// columns `fit` drew. The drawn columns take their new order; a chosen
    /// column the width left out keeps its place after them.
    pub fn fold_order(&mut self, fit: &GridColumnFit<K>, order: &[usize]) {
        let moved: Vec<K> = order.iter().filter_map(|&c| fit.columns.get(c).copied()).filter(|c| !self.leading.contains(c)).collect();
        let hidden: Vec<K> = self.chosen.iter().copied().filter(|c| !moved.contains(c)).collect();
        self.chosen = moved.into_iter().chain(hidden).collect();
    }

    /// A column edge was dragged (`ColumnResized`, by data column): the
    /// width is kept for that column. Returns whether it was one the
    /// arrangement keeps.
    pub fn set_width(&mut self, fit: &GridColumnFit<K>, data_col: usize, width: f64) -> bool {
        match fit.columns.get(data_col) {
            Some(column) if !self.leading.contains(column) => {
                self.widths.insert(*column, width);
                true
            }
            _ => false,
        }
    }

    /// One `code width` line per chosen column in display order, a width of
    /// 0 meaning the table's own; the sort follows as `sort code up|down`.
    pub fn serialize(&self) -> String {
        let mut out = String::new();
        for column in &self.chosen {
            let width = self.widths.get(column).copied().unwrap_or(0.0);
            out.push_str(&format!("{} {}\n", column.code(), width.round() as u32));
        }
        if let Some(sort) = self.sort {
            out.push_str(&format!("sort {} {}\n", sort.column.code(), if sort.descending { "down" } else { "up" }));
        }
        out
    }

    /// Read back what [`Self::serialize`] wrote, skipping unknown or
    /// repeated names and implausible widths. Returns `false` and changes
    /// nothing when no column in it is known.
    pub fn parse(&mut self, text: &str) -> bool {
        let find = |code: &str| self.catalog.iter().copied().find(|c| c.code() == code);
        let mut chosen = Vec::new();
        let mut widths = HashMap::new();
        let mut sort = None;
        for line in text.lines().take(self.catalog.len() * 2 + 2) {
            let mut parts = line.split_whitespace();
            match parts.next() {
                Some("sort") => {
                    let column = parts.next().and_then(find);
                    let descending = match parts.next() {
                        Some("down") => true,
                        Some("up") => false,
                        _ => continue,
                    };
                    if let Some(column) = column.filter(|c| c.sortable()) {
                        sort = Some(GridSort { column, descending });
                    }
                }
                Some(code) => {
                    let Some(column) = find(code) else { continue };
                    if self.leading.contains(&column) || chosen.contains(&column) {
                        continue;
                    }
                    if let Some(width) = parts.next().and_then(|w| w.parse::<f64>().ok()).filter(|w| *w >= 16.0 && *w <= 4000.0) {
                        widths.insert(column, width);
                    }
                    chosen.push(column);
                }
                None => {}
            }
        }
        if chosen.is_empty() {
            return false;
        }
        self.chosen = self.complete(chosen);
        self.widths = widths;
        self.sort = sort.filter(|s| self.chosen.contains(&s.column) || self.leading.contains(&s.column)).or(self.fallback);
        true
    }
}

/// The columns one table drew: see the module docs.
#[derive(Clone, Debug)]
pub struct GridColumnFit<K: GridColumn> {
    columns: Vec<K>,
    fitted: Option<(f64, Vec<K>)>,
    /// Width kept free at the right, where the vertical scroll bar is
    /// drawn over the columns.
    pub gutter: f64,
}

impl<K: GridColumn> Default for GridColumnFit<K> {
    fn default() -> Self {
        Self { columns: Vec::new(), fitted: None, gutter: 14.0 }
    }
}

/// What a column takes before the room is shared out: its dragged width,
/// or its fixed or base width, or a flexible column's least.
fn least_width<K: GridColumn>(column: K, layout: &GridColumns<K>) -> f64 {
    layout.width(column).unwrap_or(match column.width() {
        ColumnWidth::Fixed(width) => width,
        ColumnWidth::Flex { min, .. } => min,
        ColumnWidth::Stretch { base, .. } => base,
    })
}

impl<K: GridColumn> GridColumnFit<K> {
    /// The drawn columns, which are the grid's data columns, in order.
    pub fn columns(&self) -> &[K] {
        &self.columns
    }

    pub fn column(&self, data_col: usize) -> Option<K> {
        self.columns.get(data_col).copied()
    }

    /// Fit `layout` to `width` when either changed since the last fit:
    /// the leading columns, then the chosen ones, dropping from the right
    /// whatever the person may hide until the rest fit at their least.
    /// Returns whether it fitted again; then the grid needs
    /// [`Self::configure`] in the same draw.
    pub fn fit(&mut self, width: f64, layout: &GridColumns<K>) -> bool {
        let same = self.fitted.as_ref().is_some_and(|(w, chosen)| (w - width).abs() <= 0.5 && chosen.as_slice() == layout.chosen());
        if same {
            return false;
        }
        self.fitted = Some((width, layout.chosen().to_vec()));
        let room = (width - self.gutter).max(0.0);
        let mut shown: Vec<K> = layout.leading().iter().chain(layout.chosen()).copied().collect();
        loop {
            let least: f64 = shown.iter().map(|c| least_width(*c, layout)).sum();
            if least <= room {
                break;
            }
            let Some(last) = shown.iter().rposition(|c| c.hideable() && !layout.leading().contains(c)) else { break };
            shown.remove(last);
        }
        self.columns = shown;
        true
    }

    /// Hand a grid of `rows` rows the fitted columns: count, labels, an
    /// identity order (the drag order already lives in the arrangement),
    /// widths, and which columns sort and move. Flexible columns share the
    /// room the others leave; room they do not take widens the stretching
    /// columns a little, and whatever is still over goes to the last
    /// column. A dragged width is kept as it is.
    pub fn configure(&self, cx: &mut Cx, grid: &mut DataGrid, rows: usize, width: f64, layout: &GridColumns<K>) {
        grid.set_grid_size(rows, self.columns.len());
        let identity: Vec<usize> = (0..self.columns.len()).collect();
        grid.set_col_order(cx, &identity);
        grid.set_col_labels(self.columns.iter().map(|c| c.label().to_string()).collect());
        grid.set_col_widths(cx, &self.widths(width, layout));
        grid.set_unsortable_cols(self.columns.iter().enumerate().filter(|(_, c)| !c.sortable() || layout.leading().contains(c)).map(|(i, _)| i).collect());
        grid.set_unmovable_cols(self.columns.iter().enumerate().filter(|(_, c)| !c.movable() || layout.leading().contains(c)).map(|(i, _)| i).collect());
    }

    /// The width of each fitted column for a table `width` wide.
    pub fn widths(&self, width: f64, layout: &GridColumns<K>) -> Vec<f64> {
        let room = (width - self.gutter).max(0.0);
        let is_flex = |c: K| layout.width(c).is_none() && matches!(c.width(), ColumnWidth::Flex { .. });
        let fixed: f64 = self.columns.iter().filter(|c| !is_flex(**c)).map(|c| least_width(*c, layout)).sum();
        let spare = (room - fixed).max(0.0);
        let shares: f64 = self.columns.iter().filter_map(|c| match c.width() {
            ColumnWidth::Flex { share, .. } if is_flex(*c) => Some(share.max(0.0)),
            _ => None,
        }).sum();
        let mut widths: Vec<f64> = self.columns.iter().map(|c| match c.width() {
            ColumnWidth::Flex { share, min, max } if is_flex(*c) => {
                let part = if shares > 0.0 { spare * share.max(0.0) / shares } else { 0.0 };
                part.min(max).max(min)
            }
            _ => least_width(*c, layout),
        }).collect();
        let used: f64 = widths.iter().sum();
        let mut rest = (room - used).max(0.0);
        let stretchy: Vec<usize> = self.columns.iter().enumerate().filter(|(_, c)| layout.width(**c).is_none() && matches!(c.width(), ColumnWidth::Stretch { .. })).map(|(i, _)| i).collect();
        if !stretchy.is_empty() {
            let each = rest / stretchy.len() as f64;
            for &i in &stretchy {
                if let ColumnWidth::Stretch { max_extra, .. } = self.columns[i].width() {
                    let extra = each.min(max_extra);
                    widths[i] += extra;
                    rest -= extra;
                }
            }
        }
        if let Some(last) = widths.last_mut() {
            *last += rest.max(0.0);
        }
        widths
    }

    /// The grid's sort indicator for `layout`'s sort: the display column
    /// and whether it rises.
    pub fn sort_indicator(&self, layout: &GridColumns<K>) -> Option<(usize, bool)> {
        let sort = layout.sort()?;
        self.columns.iter().position(|c| *c == sort.column).map(|display| (display, !sort.descending))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    enum Col {
        Pin,
        Name,
        Cpu,
        Graph,
        Pid,
        User,
    }

    const ALL: [Col; 6] = [Col::Pin, Col::Name, Col::Cpu, Col::Graph, Col::Pid, Col::User];

    impl GridColumn for Col {
        fn code(self) -> &'static str {
            match self {
                Col::Pin => "pin",
                Col::Name => "name",
                Col::Cpu => "cpu",
                Col::Graph => "graph",
                Col::Pid => "pid",
                Col::User => "user",
            }
        }
        fn label(self) -> &'static str {
            self.code()
        }
        fn width(self) -> ColumnWidth {
            match self {
                Col::Pin => ColumnWidth::Fixed(28.0),
                Col::Name => ColumnWidth::Flex { share: 1.0, min: 120.0, max: 360.0 },
                Col::Graph => ColumnWidth::Stretch { base: 104.0, max_extra: 40.0 },
                _ => ColumnWidth::Fixed(80.0),
            }
        }
        fn section(self) -> Option<&'static str> {
            match self {
                Col::Cpu | Col::Graph => Some("CPU"),
                _ => Some("General"),
            }
        }
        fn hideable(self) -> bool {
            !matches!(self, Col::Pin | Col::Name)
        }
        fn descending_first(self) -> bool {
            matches!(self, Col::Cpu)
        }
    }

    fn layout(cycle: SortCycle) -> GridColumns<Col> {
        GridColumns::new(&ALL, &[Col::Pin], vec![Col::Name, Col::Cpu, Col::Pid], Some(GridSort { column: Col::Cpu, descending: true }), cycle)
    }

    #[test]
    fn a_narrow_table_drops_hideable_columns_from_the_right() {
        let mut layout = layout(SortCycle::Flip);
        layout.apply_pick(LiveId::from_str("graph"));
        layout.apply_pick(LiveId::from_str("user"));
        assert_eq!(layout.chosen(), &[Col::Name, Col::Cpu, Col::Graph, Col::Pid, Col::User]);
        let mut fit = GridColumnFit::default();
        assert!(fit.fit(1000.0, &layout));
        assert_eq!(fit.columns(), &[Col::Pin, Col::Name, Col::Cpu, Col::Graph, Col::Pid, Col::User]);
        assert!(!fit.fit(1000.0, &layout), "nothing changed, nothing fitted");
        // 28 + 120 + 80 + 104 = 332 fits in 350 - 14; the pid and user go.
        assert!(fit.fit(350.0, &layout));
        assert_eq!(fit.columns(), &[Col::Pin, Col::Name, Col::Cpu, Col::Graph]);
        // Never below the columns that cannot be hidden.
        fit.fit(10.0, &layout);
        assert_eq!(fit.columns(), &[Col::Pin, Col::Name]);
    }

    #[test]
    fn widths_share_the_room_and_the_rest_goes_to_the_last_column() {
        let mut layout = layout(SortCycle::Flip);
        layout.apply_pick(LiveId::from_str("graph"));
        let mut fit = GridColumnFit::default();
        fit.fit(1014.0, &layout);
        let widths = fit.widths(1014.0, &layout);
        // Pin 28, Cpu 80, Graph 104 + 40, Pid 80; the name caps at 360 and
        // what is left widens the last column.
        assert_eq!(fit.columns(), &[Col::Pin, Col::Name, Col::Cpu, Col::Graph, Col::Pid]);
        assert_eq!(widths[1], 360.0);
        assert_eq!(widths[3], 144.0);
        assert_eq!(widths.iter().sum::<f64>(), 1000.0);
        // A dragged width is kept as it is.
        layout.set_width(&fit, 2, 150.0);
        fit.fit(1015.0, &layout);
        assert_eq!(fit.widths(1015.0, &layout)[2], 150.0);
    }

    #[test]
    fn a_column_shown_again_goes_back_beside_its_neighbours() {
        let mut layout = layout(SortCycle::Flip);
        assert!(layout.apply_pick(LiveId::from_str("graph")));
        assert_eq!(layout.chosen(), &[Col::Name, Col::Cpu, Col::Graph, Col::Pid]);
        assert!(!layout.apply_pick(LiveId::from_str("name")), "the name cannot be hidden");
        assert!(!layout.apply_pick(LiveId::from_str("pin")));
        // Hiding the sorted column falls back to the table's own sort.
        layout.press_heading(Col::Pid);
        layout.apply_pick(LiveId::from_str("pid"));
        assert_eq!(layout.sort(), Some(GridSort { column: Col::Cpu, descending: true }));
        layout.apply_pick(grid_columns_reset_id());
        assert_eq!(layout.chosen(), &[Col::Name, Col::Cpu, Col::Pid]);
    }

    #[test]
    fn a_dropped_heading_folds_into_the_chosen_order() {
        let mut layout = layout(SortCycle::Flip);
        layout.apply_pick(LiveId::from_str("user"));
        let mut fit = GridColumnFit::default();
        // Pin, Name, Cpu, Pid fit; the user is left out.
        fit.fit(14.0 + 28.0 + 120.0 + 80.0 + 80.0, &layout);
        assert_eq!(fit.columns(), &[Col::Pin, Col::Name, Col::Cpu, Col::Pid]);
        // Pid dragged before the name.
        layout.fold_order(&fit, &[0, 3, 1, 2]);
        assert_eq!(layout.chosen(), &[Col::Pid, Col::Name, Col::Cpu, Col::User]);
    }

    #[test]
    fn a_heading_flips_or_cycles_back_to_no_sort() {
        let mut flip = layout(SortCycle::Flip);
        flip.press_heading(Col::Name);
        assert_eq!(flip.sort(), Some(GridSort { column: Col::Name, descending: false }));
        flip.press_heading(Col::Name);
        flip.press_heading(Col::Name);
        assert_eq!(flip.sort(), Some(GridSort { column: Col::Name, descending: false }));
        assert!(!flip.press_heading(Col::Pin));
        let mut cycle = layout(SortCycle::FlipThenNone);
        cycle.press_heading(Col::Cpu);
        assert_eq!(cycle.sort(), Some(GridSort { column: Col::Cpu, descending: false }), "cpu was sorted falling already");
        cycle.press_heading(Col::Cpu);
        assert_eq!(cycle.sort(), None);
        cycle.press_heading(Col::Cpu);
        assert_eq!(cycle.sort(), Some(GridSort { column: Col::Cpu, descending: true }), "figures start falling");
    }

    #[test]
    fn the_arrangement_reads_back_and_junk_is_skipped() {
        let mut layout = layout(SortCycle::Flip);
        layout.apply_pick(LiveId::from_str("user"));
        let mut fit = GridColumnFit::default();
        fit.fit(2000.0, &layout);
        layout.set_width(&fit, 3, 140.0);
        layout.press_heading(Col::Pid);
        let text = layout.serialize() + "bogus 3\ncpu 90\npin 20\nsort nope up\n";
        let mut back = self::layout(SortCycle::Flip);
        assert!(back.parse(&text));
        assert_eq!(back.chosen(), layout.chosen());
        assert_eq!(back.width(Col::User), None, "a width of 0 is the table's own");
        assert_eq!(back.width(Col::Pid), Some(140.0));
        assert_eq!(back.sort(), Some(GridSort { column: Col::Pid, descending: false }));
        // The name is never lost, and nothing known changes nothing.
        let mut short = self::layout(SortCycle::Flip);
        assert!(short.parse("cpu 0\n"));
        assert_eq!(short.chosen(), &[Col::Name, Col::Cpu]);
        assert!(!short.parse("junk\n"));
        assert_eq!(short.chosen(), &[Col::Name, Col::Cpu]);
    }

    #[test]
    fn the_chooser_has_a_flyout_per_section_and_the_defaults_row() {
        let layout = layout(SortCycle::Flip);
        let rows = layout.menu_rows();
        let titles: Vec<&str> = rows.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(titles, vec!["CPU", "General", "", "Default Columns"], "sections in the catalog's order");
        let general: Vec<&str> = rows[1].submenu.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(general, vec!["pid", "user"], "the pin and the name are not offered");
        assert!(rows[0].submenu.iter().any(|r| r.label == "cpu" && r.mark == crate::menu::MenuMark::Check));
    }
}
