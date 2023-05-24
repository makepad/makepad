use crate::{buf::EditKind, text::Pos, Diff, SelSet, Text};

pub struct Context<'a> {
    pub text: &'a Text,
    pub sels: &'a SelSet,
}

pub fn insert(context: &Context<'_>, replace_with: &Text) -> (EditKind, Diff) {
    use crate::diff;

    let mut builder = diff::Builder::new();
    let mut prev_sel_end = Pos::default();
    for sel in context.sels {
        builder.retain(sel.start() - prev_sel_end);
        builder.delete(context.text.get(sel.range()));
        builder.insert(replace_with.clone());
        prev_sel_end = sel.end();
    }
    (EditKind::Insert, builder.finish())
}

pub fn delete(context: &Context<'_>) -> (EditKind, Diff) {
    use crate::{diff, text::Range};

    let mut builder = diff::Builder::new();
    let mut prev_sel_end = Pos::default();
    for sel in context.sels {
        if sel.is_empty() {
            let start = prev_pos(context, sel.cursor);
            builder.retain(start - prev_sel_end);
            builder.delete(context.text.get(Range {
                start,
                end: sel.cursor,
            }));
        } else {
            builder.retain(sel.start() - prev_sel_end);
            builder.delete(context.text.get(sel.range()));
        }
        prev_sel_end = sel.end();
    }
    (EditKind::Delete, builder.finish())
}

fn prev_pos(context: &Context<'_>, pos: Pos) -> Pos {
    use crate::StrExt;

    if pos.byte > 0 {
        Pos {
            line: pos.line,
            byte: context.text.as_lines()[pos.line]
                .prev_grapheme_boundary(pos.byte)
                .unwrap(),
        }
    } else if pos.line > 0 {
        let prev_line_pos = pos.line - 1;
        Pos {
            line: prev_line_pos,
            byte: context.text.as_lines()[prev_line_pos].len(),
        }
    } else {
        pos
    }
}
