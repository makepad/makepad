//! What the popups read from a long literal, read once per change of the
//! source: a list's numbers, a table's cells. A control's own write moves
//! them in place (the changed item re-read, the ones after it shifted), so
//! a drag over a list of 140,000 numbers never reads the list again.

use crate::data::*;
use crate::data_items;
use std::ops::Range;
use std::rc::Rc;

pub(crate) type Items = Rc<Vec<(Range<usize>, f64)>>;

#[derive(Default)]
pub(crate) struct Caches {
    /// The source's revision: one more per change.
    pub rev: u64,
    list: Option<(u64, usize, Items)>,
    table: Option<(u64, usize, Rc<Table>)>,
}

impl Caches {
    /// The source was replaced: everything is read again when asked for.
    pub fn changed(&mut self) {
        self.rev += 1;
    }

    /// The numbers of the list at `start`.
    pub fn list_items(&mut self, source: &str, start: usize) -> Items {
        if let Some((rev, at, items)) = &self.list {
            if *rev == self.rev && *at == start {
                return items.clone();
            }
        }
        let items = Rc::new(data_items(source, start));
        self.list = Some((self.rev, start, items.clone()));
        items
    }

    /// The records of the table at `start`.
    pub fn table_at(&mut self, source: &str, start: usize) -> Option<Rc<Table>> {
        if let Some((rev, at, table)) = &self.table {
            if *rev == self.rev && *at == start {
                return Some(table.clone());
            }
        }
        let table = Rc::new(table_of(&lit_at(source, start)?)?);
        self.table = Some((self.rev, start, table.clone()));
        Some(table)
    }

    /// `start..end` of the source was replaced by `text` (`source` is the
    /// text after): what was read moves with it.
    pub fn spliced(&mut self, source: &str, start: usize, end: usize, text: &str) {
        let prev = self.rev;
        self.rev += 1;
        if let Some((rev, at, items)) = &mut self.list {
            if *rev == prev && *at < start && list_spliced(Rc::make_mut(items), start, end, text) {
                *rev = self.rev;
            }
        }
        if let Some((rev, at, table)) = &mut self.table {
            if *rev == prev && *at < start && table_spliced(Rc::make_mut(table), source, start, end, text) {
                *rev = self.rev;
            }
        }
    }
}

fn shift(r: &Range<usize>, delta: isize) -> Range<usize> {
    (r.start as isize + delta) as usize..(r.end as isize + delta) as usize
}

/// A list's numbers after `start..end` became `text`: the numbers in the
/// replaced run read from `text`, the ones after shifted. `false` when the
/// run cuts through a number (read the list again).
pub(crate) fn list_spliced(items: &mut Vec<(Range<usize>, f64)>, start: usize, end: usize, text: &str) -> bool {
    let a = items.partition_point(|(r, _)| r.end <= start);
    let b = items.partition_point(|(r, _)| r.start < end).max(a);
    if items[a..b].iter().any(|(r, _)| r.start < start || r.end > end) {
        return false;
    }
    let delta = text.len() as isize - (end - start) as isize;
    let wrapped = format!("[{text}]");
    let fresh: Vec<(Range<usize>, f64)> = data_items(&wrapped, 0).into_iter().map(|(r, v)| (r.start - 1 + start..r.end - 1 + start, v)).collect();
    let added = fresh.len();
    items.splice(a..b, fresh);
    for (r, _) in &mut items[a + added..] {
        *r = shift(r, delta);
    }
    true
}

/// A table's cells after `start..end` (exactly one cell) became `text`:
/// that cell read again, the ones after shifted. `false` otherwise.
pub(crate) fn table_spliced(table: &mut Table, source: &str, start: usize, end: usize, text: &str) -> bool {
    let cells = &mut table.cells;
    let a = cells.partition_point(|c| c.range.end <= start);
    let b = cells.partition_point(|c| c.range.start < end).max(a);
    if b != a + 1 || cells[a].range != (start..end) {
        return false;
    }
    let value = match lit_at(source, start) {
        Some(Lit::Num { value, range }) if range.end == start + text.len() => CellValue::Num(value),
        Some(Lit::Str { text: s, range }) if range.end == start + text.len() => CellValue::Str(s),
        Some(Lit::Color { rgba, range }) if range.end == start + text.len() => CellValue::Color(rgba),
        _ => return false,
    };
    let delta = text.len() as isize - (end - start) as isize;
    cells[a].range = start..start + text.len();
    cells[a].value = value;
    for c in &mut cells[a + 1..] {
        c.range = shift(&c.range, delta);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(src: &str, range: Range<usize>, text: &str) -> String {
        format!("{}{}{}", &src[..range.start], text, &src[range.end..])
    }

    #[test]
    fn a_write_moves_the_list_in_place() {
        let src = "let L = [0.5, -2, 3.25,\n 4, 5]";
        let start = src.find('[').unwrap();
        let mut caches = Caches::default();
        let before = caches.list_items(src, start);
        // `-2` becomes `-10.75`.
        let r = before[1].0.clone();
        let after_src = apply(src, r.clone(), "-10.75");
        caches.spliced(&after_src, r.start, r.end, "-10.75");
        let moved = caches.list_items(&after_src, start);
        assert_eq!(*moved, data_items(&after_src, start));
        // A pair `x, y` rewritten at once.
        let r = moved[2].0.start..moved[3].0.end;
        let next = apply(&after_src, r.clone(), "3.5,\n 4.5");
        caches.spliced(&next, r.start, r.end, "3.5,\n 4.5");
        assert_eq!(*caches.list_items(&next, start), data_items(&next, start));
    }

    #[test]
    fn a_write_moves_the_table_in_place() {
        let src = "[{n: \"a\", v: 1}, {n: \"b\", v: 2}]";
        let mut caches = Caches::default();
        let t = caches.table_at(src, 0).unwrap();
        let r = t.cells[1].range.clone();
        let next = apply(src, r.clone(), "12.5");
        caches.spliced(&next, r.start, r.end, "12.5");
        let moved = caches.table_at(&next, 0).unwrap();
        assert_eq!(*moved, table_of(&lit_at(&next, 0).unwrap()).unwrap());
        let r = moved.cells[2].range.clone();
        let next2 = apply(&next, r.clone(), "\"bee\"");
        caches.spliced(&next2, r.start, r.end, "\"bee\"");
        assert_eq!(*caches.table_at(&next2, 0).unwrap(), table_of(&lit_at(&next2, 0).unwrap()).unwrap());
    }
}
