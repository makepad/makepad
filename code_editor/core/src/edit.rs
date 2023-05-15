use crate::{Cursor, Diff, Text};

pub fn insert(text: &Text, cursors: impl IntoIterator<Item = Cursor>, replace_with: &Text) -> Diff {
    use crate::{diff, Pos};

    let cursors = cursors.into_iter();
    let mut builder = diff::Builder::new();
    let mut prev_cursor_end = Pos::default();
    for cursor in cursors {
        builder.retain(cursor.start() - prev_cursor_end);
        builder.delete(text.get(cursor.range()));
        builder.insert(replace_with.clone());
        prev_cursor_end = cursor.end();
    }
    builder.finish()
}

pub fn delete(text: &Text, cursors: impl IntoIterator<Item = Cursor>) -> Diff {
    use crate::{diff, mv, Pos, Range};

    let mut builder = diff::Builder::new();
    let mut prev_cursor_end = Pos::default();
    for cursor in cursors {
        if cursor.is_empty() {
            let start = mv::move_left(text, cursor.caret);
            builder.retain(start - prev_cursor_end);
            builder.delete(text.get(Range {
                start,
                end: cursor.caret,
            }));
        } else {
            builder.retain(cursor.start() - prev_cursor_end);
            builder.delete(text.get(cursor.range()));
        }
        prev_cursor_end = cursor.end();
    }
    builder.finish()
}
