use crate::{sel_set::Sel, Diff, Text};

pub fn insert(text: &Text, sels: impl IntoIterator<Item = Sel>, replace_with: &Text) -> Diff {
    use crate::{diff, text::Pos};

    let sels = sels.into_iter();
    let mut builder = diff::Builder::new();
    let mut prev_sel_end = Pos::default();
    for sel in sels {
        builder.retain(sel.start() - prev_sel_end);
        builder.delete(text.get(sel.range()));
        builder.insert(replace_with.clone());
        prev_sel_end = sel.end();
    }
    builder.finish()
}

pub fn delete(text: &Text, sels: impl IntoIterator<Item = Sel>) -> Diff {
    use crate::{
        diff, move_ops,
        text::{Pos, Range},
    };

    let mut builder = diff::Builder::new();
    let mut prev_sel_end = Pos::default();
    for sel in sels {
        if sel.is_empty() {
            let start = move_ops::move_left(text, sel.cursor);
            builder.retain(start - prev_sel_end);
            builder.delete(text.get(Range {
                start,
                end: sel.cursor,
            }));
        } else {
            builder.retain(sel.start() - prev_sel_end);
            builder.delete(text.get(sel.range()));
        }
        prev_sel_end = sel.end();
    }
    builder.finish()
}
