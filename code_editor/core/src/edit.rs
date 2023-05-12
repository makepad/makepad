use crate::{cursor_set, Diff, Text};

pub fn insert(spans: impl Iterator<Item = cursor_set::Span>, replace_with: &Text) -> Diff {
    use crate::diff;

    let mut builder = diff::Builder::new();
    for span in spans {
        if span.is_sel {
            builder.delete(span.len);
            builder.insert(replace_with.clone());
        } else {
            builder.retain(span.len);
        }
    }
    builder.finish()
}

pub fn delete(text: &Text, spans: impl Iterator<Item = cursor_set::Span>) -> Diff {
    use crate::{diff, mv, Len, Pos};

    let mut builder = diff::Builder::new();
    let mut prev_pos = Pos::default();
    let mut pos = Pos::default();
    for span in spans {
        if span.is_sel {
            if span.len == Len::default() {
                let new_pos = mv::move_left(text, pos);
                let len = pos - new_pos;
                pos = new_pos;
                builder.retain(pos - prev_pos);
                builder.delete(len);
            } else {
                builder.retain(pos - prev_pos);
                builder.delete(span.len);
            }
            prev_pos = pos;
        } else {
            pos += span.len;
        }
    }
    builder.finish()
}
