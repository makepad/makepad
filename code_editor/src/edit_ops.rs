use crate::{Diff, Position, Range, Text};

pub fn replace(range: Range, replace_with: Text) -> Diff {
    use crate::diff::Builder;

    let mut builder = Builder::new();
    builder.retain(range.start() - Position::origin());
    builder.delete(range.length());
    builder.insert(replace_with);
    builder.finish()
}

pub fn delete(range: Range) -> Diff {
    use crate::diff::Builder;

    let mut builder = Builder::new();
    builder.retain(range.start() - Position::origin());
    builder.delete(range.length());
    builder.finish()
}

pub fn backspace(text: &mut Text, range: Range) -> Diff {
    use crate::diff::Builder;

    if range.is_empty() {
        let position = prev_position(text, range.start());
        let mut builder = Builder::new();
        builder.retain(position - Position::origin());
        builder.delete(range.start() - position);
        builder.finish()
    } else {
        delete(range)
    }
}

pub fn prev_position(text: &Text, position: Position) -> Position {
    use crate::str::StrExt;

    if position.byte_index > 0 {
        return Position::new(
            position.line_index,
            text.as_lines()[position.line_index][..position.byte_index]
                .grapheme_indices()
                .next_back()
                .map(|(byte_index, _)| byte_index)
                .unwrap(),
        );
    }
    if position.line_index > 0 {
        let prev_line_index = position.line_index - 1;
        return Position::new(prev_line_index, text.as_lines()[prev_line_index].len());
    }
    position
}
