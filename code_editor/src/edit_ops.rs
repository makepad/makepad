use crate::{Text, TextDiff, TextPos, TextRange};

pub fn replace(range: TextRange, replace_with: Text) -> TextDiff {
    use crate::text_diff::Builder;

    let mut builder = Builder::new();
    builder.retain(range.start() - TextPos::default());
    builder.delete(range.length());
    builder.insert(replace_with);
    builder.finish()
}

pub fn enter(range: TextRange) -> TextDiff {
    replace(range, "\n".into())
}

pub fn delete(range: TextRange) -> TextDiff {
    use crate::text_diff::Builder;

    let mut builder = Builder::new();
    builder.retain(range.start() - TextPos::default());
    builder.delete(range.length());
    builder.finish()
}

pub fn backspace(text: &mut Text, range: TextRange) -> TextDiff {
    use crate::text_diff::Builder;

    if range.is_empty() {
        let position = prev_position(text, range.start());
        let mut builder = Builder::new();
        builder.retain(position - TextPos::default());
        builder.delete(range.start() - position);
        builder.finish()
    } else {
        delete(range)
    }
}

pub fn prev_position(text: &Text, position: TextPos) -> TextPos {
    use crate::str::StrExt;

    if position.byte > 0 {
        return TextPos {
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
        return TextPos {
            line: prev_line,
            byte: text.as_lines()[prev_line].len(),
        };
    }
    position
}
