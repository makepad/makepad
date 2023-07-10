use crate::{Diff, Position, Range, Text};

pub fn replace(range: Range, replace_with: Text) -> Diff {
    use crate::diff::Builder;

    let mut builder = Builder::new();
    builder.retain(range.start() - Position::default());
    builder.delete(range.length());
    builder.insert(replace_with);
    builder.finish()
}

pub fn enter(range: Range) -> Diff {
    replace(range, "\n".into())
}

pub fn delete(range: Range) -> Diff {
    use crate::diff::Builder;

    let mut builder = Builder::new();
    builder.retain(range.start() - Position::default());
    builder.delete(range.length());
    builder.finish()
}

pub fn backspace(text: &mut Text, range: Range) -> Diff {
    use crate::diff::Builder;

    if range.is_empty() {
        let position = prev_position(text, range.start());
        let mut builder = Builder::new();
        builder.retain(position - Position::default());
        builder.delete(range.start() - position);
        builder.finish()
    } else {
        delete(range)
    }
}

pub fn prev_position(text: &Text, position: Position) -> Position {
    use crate::str::StrExt;

    if position.byte > 0 {
        return Position::new(
            position.line,
            text.as_lines()[position.line][..position.byte]
                .grapheme_indices()
                .next_back()
                .map(|(byte, _)| byte)
                .unwrap(),
        );
    }
    if position.line > 0 {
        let prev_line = position.line - 1;
        return Position::new(prev_line, text.as_lines()[prev_line].len());
    }
    position
}