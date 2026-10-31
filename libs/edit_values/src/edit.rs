//! The one edit format and the undo-per-gesture contract.

use crate::site::{SiteId, Value};

/// Replace `range` of file `file` with `text`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Splice {
    pub file: usize,
    pub range: std::ops::Range<usize>,
    pub text: String,
}

/// One change of an edit: a value set by id (the document writes it its own way), or bytes.
#[derive(Clone, Debug, PartialEq)]
pub enum Change {
    Set { id: SiteId, value: Value },
    Splice(Splice),
}

/// What a control, a panel, an inline code control or a handle sends.
///
/// The contract every [`EditSink`] keeps:
/// - every edit with the same `gesture` folds into one undo step;
/// - `done: false` is live: patched or previewed (only the newest is evaluated), never
///   committed on its own;
/// - `done: true` commits the gesture and re-evaluates; a commit whose result does not load is
///   undone and reported.
#[derive(Clone, Debug, PartialEq)]
pub struct Edit {
    pub changes: Vec<Change>,
    pub gesture: u64,
    pub done: bool,
}

impl Edit {
    pub fn set(id: impl Into<SiteId>, value: Value, gesture: u64, done: bool) -> Edit {
        Edit { changes: vec![Change::Set { id: id.into(), value }], gesture, done }
    }
    /// Replace `range` of file 0 with `text`.
    pub fn splice(range: std::ops::Range<usize>, text: impl Into<String>, gesture: u64, done: bool) -> Edit {
        Edit { changes: vec![Change::Splice(Splice { file: 0, range, text: text.into() })], gesture, done }
    }
    /// The edit's byte changes.
    pub fn splices(&self) -> impl Iterator<Item = &Splice> {
        self.changes.iter().filter_map(|c| if let Change::Splice(s) = c { Some(s) } else { None })
    }
}

/// A document host taking edits.
pub trait EditSink {
    fn edit(&mut self, edit: Edit) -> Result<(), String>;
}

/// Sort splices of one file from the end, merge identical ones, and refuse overlaps, so they
/// apply in one pass without moving each other.
pub fn normalize_splices(mut splices: Vec<Splice>) -> Result<Vec<Splice>, String> {
    splices.sort_by(|a, b| (b.file, b.range.start, b.range.end).cmp(&(a.file, a.range.start, a.range.end)));
    splices.dedup();
    for w in splices.windows(2) {
        if w[0].file == w[1].file && w[1].range.end > w[0].range.start {
            return Err(format!("overlapping edits at {}..{} and {}..{}", w[1].range.start, w[1].range.end, w[0].range.start, w[0].range.end));
        }
    }
    Ok(splices)
}

/// `source` (file `file`) with the splices of that file applied.
pub fn apply_splices(source: &str, file: usize, splices: &[Splice]) -> Result<String, String> {
    let mine: Vec<Splice> = splices.iter().filter(|s| s.file == file).cloned().collect();
    let mut out = source.to_string();
    for s in normalize_splices(mine)? {
        if s.range.end > out.len() || s.range.start > s.range.end || !out.is_char_boundary(s.range.start) || !out.is_char_boundary(s.range.end) {
            return Err(format!("edit {}..{} is outside the text", s.range.start, s.range.end));
        }
        out.replace_range(s.range.clone(), &s.text);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sp(a: usize, b: usize, t: &str) -> Splice {
        Splice { file: 0, range: a..b, text: t.into() }
    }

    #[test]
    fn splices_apply_from_the_end_and_refuse_overlaps() {
        let src = "radius: 4, height: 12";
        let out = apply_splices(src, 0, &[sp(8, 9, "5.5"), sp(19, 21, "30")]).unwrap();
        assert_eq!(out, "radius: 5.5, height: 30");
        assert!(apply_splices(src, 0, &[sp(8, 12, "x"), sp(10, 14, "y")]).is_err());
        assert!(apply_splices(src, 0, &[sp(8, 99, "x")]).is_err());
        // Another file's splices are not this one's.
        assert_eq!(apply_splices(src, 0, &[Splice { file: 1, range: 0..3, text: "x".into() }]).unwrap(), src);
    }
}
