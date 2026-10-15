//! The track table's columns, as the shared column helper
//! ([`makepad_widgets::GridColumns`]) arranges them: which there are, how
//! wide, and how a column orders the tracks when its heading is pressed.
//!
//! A heading press sorts by that column rising, a second press falling, a
//! third puts the list back in its own order (the playlist's, or the
//! search ranking). A track without the figure (no release date known)
//! sorts after every track that has one, whichever way.

use makepad_widgets::{ColumnWidth, GridColumn, GridColumns, SortCycle};
use std::cmp::Ordering;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Column {
    /// The track's place in the list.
    Number,
    Title,
    Artist,
    Album,
    Released,
    Time,
}

/// Every column, in the chooser's order.
pub const ALL: [Column; 6] = [Column::Number, Column::Title, Column::Artist, Column::Album, Column::Released, Column::Time];

/// The arrangement a first start shows: every column, in the list's own
/// order until a heading is pressed.
pub fn layout() -> GridColumns<Column> {
    GridColumns::new(&ALL, &[], ALL.to_vec(), None, SortCycle::FlipThenNone)
}

impl GridColumn for Column {
    fn code(self) -> &'static str {
        match self {
            Column::Number => "number",
            Column::Title => "title",
            Column::Artist => "artist",
            Column::Album => "album",
            Column::Released => "released",
            Column::Time => "time",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Column::Number => "#",
            Column::Title => "Title",
            Column::Artist => "Artist",
            Column::Album => "Album",
            Column::Released => "Released",
            Column::Time => "Time",
        }
    }

    fn menu_label(self) -> &'static str {
        match self {
            Column::Number => "Number",
            Column::Released => "Release Date",
            other => other.label(),
        }
    }

    /// The text columns share the room the figures leave.
    fn width(self) -> ColumnWidth {
        match self {
            Column::Number => ColumnWidth::Fixed(56.0),
            Column::Title => ColumnWidth::Flex { share: 0.42, min: 160.0, max: f64::MAX },
            Column::Artist => ColumnWidth::Flex { share: 0.30, min: 100.0, max: f64::MAX },
            Column::Album => ColumnWidth::Flex { share: 0.28, min: 100.0, max: f64::MAX },
            Column::Released => ColumnWidth::Fixed(108.0),
            Column::Time => ColumnWidth::Fixed(72.0),
        }
    }

    fn hideable(self) -> bool {
        self != Column::Title
    }
}

/// What a track sorts by in one column.
#[derive(Clone, Debug, PartialEq)]
pub enum Key<'a> {
    Text(&'a str),
    /// A number where 0 means "not known": it sorts after every known one.
    Known(u64),
}

/// Order two keys, unknowns last in either direction; `tie` (the list's
/// own order) decides between equals.
pub fn compare(a: &Key, b: &Key, descending: bool, tie: Ordering) -> Ordering {
    let order = match (a, b) {
        (Key::Text(a), Key::Text(b)) => a.cmp(b),
        (Key::Known(0), Key::Known(0)) => return tie,
        (Key::Known(0), Key::Known(_)) => return Ordering::Greater,
        (Key::Known(_), Key::Known(0)) => return Ordering::Less,
        (Key::Known(a), Key::Known(b)) => a.cmp(b),
        _ => Ordering::Equal,
    };
    let order = if descending { order.reverse() } else { order };
    order.then(tie)
}

/// A release date key (`yyyymmdd`, missing parts 0) in words: `1995`,
/// `1997-03`, `1997-03-01`.
pub fn release(key: u32) -> String {
    let (year, month, day) = (key / 10_000, key / 100 % 100, key % 100);
    match (month, day) {
        (0, _) => format!("{year}"),
        (_, 0) => format!("{year}-{month:02}"),
        _ => format!("{year}-{month:02}-{day:02}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_figures_go_last_both_ways() {
        let mut keys = vec![(Key::Known(0), 0), (Key::Known(20), 1), (Key::Known(0), 2), (Key::Known(10), 3)];
        for descending in [false, true] {
            keys.sort_by(|a, b| compare(&a.0, &b.0, descending, a.1.cmp(&b.1)));
            let order: Vec<usize> = keys.iter().map(|k| k.1).collect();
            let known = if descending { [1, 3] } else { [3, 1] };
            assert_eq!(order, vec![known[0], known[1], 0, 2], "descending {descending}");
        }
    }

    #[test]
    fn release_dates_read_as_written() {
        assert_eq!(release(19_950_000), "1995");
        assert_eq!(release(19_970_300), "1997-03");
        assert_eq!(release(19_970_101), "1997-01-01");
    }

    /// A layout saved while the table still had an "Added" column reads
    /// back without it.
    #[test]
    fn an_old_layout_with_the_added_column_still_reads() {
        let mut layout = layout();
        assert!(layout.parse("number 0\ntitle 0\nadded 90\nreleased 0\nsort added up\n"));
        assert_eq!(layout.chosen(), &[Column::Number, Column::Title, Column::Released]);
        assert_eq!(layout.sort(), None);
    }
}
