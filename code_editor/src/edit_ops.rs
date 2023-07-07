use crate::{move_ops, Diff, Position, Range, Text};

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
        let position = move_ops::move_left(text, range.start());
        let mut builder = Builder::new();
        builder.retain(position - Position::origin());
        builder.delete(range.start() - position);
        builder.finish()
    } else {
        delete(range)
    }
}
