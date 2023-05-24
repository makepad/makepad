use crate::{buf::EditKind, text::Pos, Diff, SelSet, Text};

pub struct Context<'a> {
    pub text: &'a Text,
    pub sels: &'a SelSet,
}

pub fn insert(text: &Text, sels: &SelSet, replace_with: &Text) -> (EditKind, Diff) {
    use crate::diff;

    let mut builder = diff::Builder::new();
    let mut prev_sel_end = Pos::default();
    for sel in sels {
        builder.retain(sel.start() - prev_sel_end);
        builder.delete(text.get(sel.range()));
        builder.insert(replace_with.clone());
        prev_sel_end = sel.end();
    }
    (EditKind::Insert, builder.finish())
}

pub fn delete(text: &Text, sels: &SelSet) -> (EditKind, Diff) {
    use crate::{diff, move_ops, text::Range};

    let mut builder = diff::Builder::new();
    let mut prev_sel_end = Pos::default();
    for sel in sels {
        if sel.is_empty() {
            let start = move_ops::move_left(text.as_lines(), sel.cursor_pos);
            builder.retain(start - prev_sel_end);
            builder.delete(text.get(Range {
                start,
                end: sel.cursor_pos,
            }));
        } else {
            builder.retain(sel.start() - prev_sel_end);
            builder.delete(text.get(sel.range()));
        }
        prev_sel_end = sel.end();
    }
    (EditKind::Delete, builder.finish())
}
