use crate::{Affinity, Document, Position};

pub fn move_left(document: &Document<'_>, position: Position) -> (Position, Affinity) {
    if !is_at_start_of_line(position) {
        return move_to_prev_grapheme(document, position);
    }
    if !is_at_first_line(position) {
        return move_to_end_of_prev_line(document, position);
    }
    (position, Affinity::Before)
}

pub fn move_right(document: &Document<'_>, position: Position) -> (Position, Affinity) {
    if !is_at_end_of_line(document, position) {
        return move_to_next_grapheme(document, position);
    }
    if !is_at_last_line(document, position) {
        return move_to_start_of_next_line(position);
    }
    (position, Affinity::After)
}

fn is_at_start_of_line(position: Position) -> bool {
    position.byte == 0
}

fn is_at_end_of_line(document: &Document<'_>, position: Position) -> bool {
    position.byte == document.line(position.line).text().len()
}

fn is_at_first_line(position: Position) -> bool {
    position.line == 0
}

fn is_at_last_line(document: &Document<'_>, position: Position) -> bool {
    position.line == document.line_count() - 1
}

fn move_to_prev_grapheme(document: &Document<'_>, position: Position) -> (Position, Affinity) {
    use crate::str::StrExt;

    (
        Position::new(
            position.line,
            document.line(position.line).text()[..position.byte]
                .grapheme_indices()
                .next_back()
                .map(|(byte_index, _)| byte_index)
                .unwrap(),
        ),
        Affinity::After,
    )
}

fn move_to_next_grapheme(document: &Document<'_>, position: Position) -> (Position, Affinity) {
    use crate::str::StrExt;

    let line = document.line(position.line);
    (
        Position::new(
            position.line,
            line.text()[position.byte..]
                .grapheme_indices()
                .nth(1)
                .map(|(byte, _)| position.byte + byte)
                .unwrap_or(line.text().len()),
        ),
        Affinity::Before,
    )
}

fn move_to_end_of_prev_line(document: &Document<'_>, position: Position) -> (Position, Affinity) {
    let prev_line = position.line - 1;
    (
        Position::new(prev_line, document.line(prev_line).text().len()),
        Affinity::After,
    )
}

fn move_to_start_of_next_line(position: Position) -> (Position, Affinity) {
    (Position::new(position.line + 1, 0), Affinity::Before)
}