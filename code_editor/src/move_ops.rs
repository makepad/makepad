use crate::{Affinity, Document, Pos};

pub fn move_left(document: &Document<'_>, position: Pos) -> ((Pos, Affinity), Option<usize>) {
    if !is_at_start_of_line(position) {
        return move_to_prev_grapheme(document, position);
    }
    if !is_at_first_line(position) {
        return move_to_end_of_prev_line(document, position);
    }
    ((position, Affinity::Before), None)
}

pub fn move_right(document: &Document<'_>, position: Pos) -> ((Pos, Affinity), Option<usize>) {
    if !is_at_end_of_line(document, position) {
        return move_to_next_grapheme(document, position);
    }
    if !is_at_last_line(document, position) {
        return move_to_start_of_next_line(position);
    }
    ((position, Affinity::After), None)
}

pub fn move_up(
    document: &Document<'_>,
    (position, affinity): (Pos, Affinity),
    preferred_column: Option<usize>,
) -> ((Pos, Affinity), Option<usize>) {
    if !is_at_first_row_of_line(document, (position, affinity)) {
        return move_to_prev_row_of_line(document, (position, affinity), preferred_column);
    }
    if !is_at_first_line(position) {
        return move_to_last_row_of_prev_line(document, (position, affinity), preferred_column);
    }
    ((position, affinity), preferred_column)
}

pub fn move_down(
    document: &Document<'_>,
    (position, affinity): (Pos, Affinity),
    preferred_column: Option<usize>,
) -> ((Pos, Affinity), Option<usize>) {
    if !is_at_last_row_of_line(document, (position, affinity)) {
        return move_to_next_row_of_line(document, (position, affinity), preferred_column);
    }
    if !is_at_last_line(document, position) {
        return move_to_first_row_of_next_line(document, (position, affinity), preferred_column);
    }
    ((position, affinity), preferred_column)
}

fn is_at_start_of_line(position: Pos) -> bool {
    position.byte == 0
}

fn is_at_end_of_line(document: &Document<'_>, position: Pos) -> bool {
    position.byte == document.line(position.line).text().len()
}

fn is_at_first_row_of_line(document: &Document<'_>, (position, affinity): (Pos, Affinity)) -> bool {
    document
        .line(position.line)
        .byte_affinity_to_row_column(
            (position.byte, affinity),
            document.settings().tab_column_count,
        )
        .0
        == 0
}

fn is_at_last_row_of_line(document: &Document<'_>, (position, affinity): (Pos, Affinity)) -> bool {
    let line = document.line(position.line);
    line.byte_affinity_to_row_column(
        (position.byte, affinity),
        document.settings().tab_column_count,
    )
    .0 == line.row_count() - 1
}

fn is_at_first_line(position: Pos) -> bool {
    position.line == 0
}

fn is_at_last_line(document: &Document<'_>, position: Pos) -> bool {
    position.line == document.line_count() - 1
}

fn move_to_prev_grapheme(
    document: &Document<'_>,
    position: Pos,
) -> ((Pos, Affinity), Option<usize>) {
    use crate::str::StrExt;

    (
        (
            Pos {
                line: position.line,
                byte: document.line(position.line).text()[..position.byte]
                    .grapheme_indices()
                    .next_back()
                    .map(|(byte_index, _)| byte_index)
                    .unwrap(),
            },
            Affinity::After,
        ),
        None,
    )
}

fn move_to_next_grapheme(
    document: &Document<'_>,
    position: Pos,
) -> ((Pos, Affinity), Option<usize>) {
    use crate::str::StrExt;

    let line = document.line(position.line);
    (
        (
            Pos {
                line: position.line,
                byte: line.text()[position.byte..]
                    .grapheme_indices()
                    .nth(1)
                    .map(|(byte, _)| position.byte + byte)
                    .unwrap_or(line.text().len()),
            },
            Affinity::Before,
        ),
        None,
    )
}

fn move_to_end_of_prev_line(
    document: &Document<'_>,
    position: Pos,
) -> ((Pos, Affinity), Option<usize>) {
    let prev_line = position.line - 1;
    (
        (
            Pos {
                line: prev_line,
                byte: document.line(prev_line).text().len(),
            },
            Affinity::After,
        ),
        None,
    )
}

fn move_to_start_of_next_line(position: Pos) -> ((Pos, Affinity), Option<usize>) {
    (
        (
            Pos {
                line: position.line + 1,
                byte: 0,
            },
            Affinity::Before,
        ),
        None,
    )
}

fn move_to_prev_row_of_line(
    document: &Document<'_>,
    (position, affinity): (Pos, Affinity),
    preferred_column: Option<usize>,
) -> ((Pos, Affinity), Option<usize>) {
    let line = document.line(position.line);
    let (row, mut column) = line.byte_affinity_to_row_column(
        (position.byte, affinity),
        document.settings().tab_column_count,
    );
    if let Some(preferred_column) = preferred_column {
        column = preferred_column;
    }
    let (byte, affinity) =
        line.row_column_to_byte_affinity((row - 1, column), document.settings().tab_column_count);
    (
        (
            Pos {
                line: position.line,
                byte,
            },
            affinity,
        ),
        Some(column),
    )
}

fn move_to_next_row_of_line(
    document: &Document<'_>,
    (position, affinity): (Pos, Affinity),
    preferred_column: Option<usize>,
) -> ((Pos, Affinity), Option<usize>) {
    let line = document.line(position.line);
    let (row, mut column) = line.byte_affinity_to_row_column(
        (position.byte, affinity),
        document.settings().tab_column_count,
    );
    if let Some(preferred_column) = preferred_column {
        column = preferred_column;
    }
    let (byte, affinity) =
        line.row_column_to_byte_affinity((row + 1, column), document.settings().tab_column_count);
    (
        (
            Pos {
                line: position.line,
                byte,
            },
            affinity,
        ),
        Some(column),
    )
}

fn move_to_last_row_of_prev_line(
    document: &Document<'_>,
    (position, affinity): (Pos, Affinity),
    preferred_column: Option<usize>,
) -> ((Pos, Affinity), Option<usize>) {
    let (_, mut column) = document.line(position.line).byte_affinity_to_row_column(
        (position.byte, affinity),
        document.settings().tab_column_count,
    );
    if let Some(preferred_column) = preferred_column {
        column = preferred_column;
    }
    let prev_line = position.line - 1;
    let prev_line_ref = document.line(prev_line);
    let (byte, affinity) = prev_line_ref.row_column_to_byte_affinity(
        (prev_line_ref.row_count() - 1, column),
        document.settings().tab_column_count,
    );
    (
        (
            Pos {
                line: prev_line,
                byte,
            },
            affinity,
        ),
        Some(column),
    )
}

fn move_to_first_row_of_next_line(
    document: &Document<'_>,
    (position, affinity): (Pos, Affinity),
    preferred_column: Option<usize>,
) -> ((Pos, Affinity), Option<usize>) {
    let (_, mut column) = document.line(position.line).byte_affinity_to_row_column(
        (position.byte, affinity),
        document.settings().tab_column_count,
    );
    if let Some(preferred_column) = preferred_column {
        column = preferred_column;
    }
    let next_line = position.line + 1;
    let (byte, affinity) = document
        .line(next_line)
        .row_column_to_byte_affinity((0, column), document.settings().tab_column_count);
    (
        (
            Pos {
                line: next_line,
                byte,
            },
            affinity,
        ),
        Some(column),
    )
}
