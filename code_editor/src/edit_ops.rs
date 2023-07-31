use crate::{Diff, Pos, Range, Text};

pub fn replace(range: Range, replace_with: Text) -> Diff {
    use crate::diff::Builder;

    let mut builder = Builder::new();
    builder.retain(range.start() - Pos::default());
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
    builder.retain(range.start() - Pos::default());
    builder.delete(range.length());
    builder.finish()
}

pub fn backspace(text: &mut Text, range: Range) -> Diff {
    use crate::diff::Builder;

    if range.is_empty() {
        let position = prev_position(text, range.start());
        let mut builder = Builder::new();
        builder.retain(position - Pos::default());
        builder.delete(range.start() - position);
        builder.finish()
    } else {
        delete(range)
    }
}

pub fn prev_position(text: &Text, position: Pos) -> Pos {
    use crate::str::StrExt;

    if position.byte > 0 {
        return Pos {
            line: position.line,
            byte: text.as_lines()[position.line][..position.byte]
                .grapheme_indices()
                .next_back()
                .map(|(byte, _)| byte)
                .unwrap(),
        };
    }
    if position.line > 0 {
        let prev_line = position.line - 1;
        return Pos {
            line: prev_line,
            byte: text.as_lines()[prev_line].len(),
        };
    }
    position
}
