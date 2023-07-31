use crate::{Document, Bias, BiasedPos, Pos};

pub fn move_left(document: &Document<'_>, position: Pos) -> (BiasedPos, Option<usize>) {
    if !is_at_start_of_line(position) {
        return move_to_prev_grapheme(document, position);
    }
    if !is_at_first_line(position) {
        return move_to_end_of_prev_line(document, position);
    }
    (BiasedPos::from_pos_and_bias(position, Bias::Before), None)
}

pub fn move_right(document: &Document<'_>, position: Pos) -> (BiasedPos, Option<usize>) {
    if !is_at_end_of_line(document, position) {
        return move_to_next_grapheme(document, position);
    }
    if !is_at_last_line(document, position) {
        return move_to_start_of_next_line(position);
    }
    (BiasedPos::from_pos_and_bias(position, Bias::After), None)
}

pub fn move_up(
    document: &Document<'_>,
    pos: BiasedPos,
    preferred_column: Option<usize>,
) -> (BiasedPos, Option<usize>) {
    if !is_at_first_row_of_line(document, pos) {
        return move_to_prev_row_of_line(document, pos, preferred_column);
    }
    if !is_at_first_line(pos.to_pos()) {
        return move_to_last_row_of_prev_line(document, pos, preferred_column);
    }
    (pos, preferred_column)
}

pub fn move_down(
    document: &Document<'_>,
    pos: BiasedPos,
    preferred_column: Option<usize>,
) -> (BiasedPos, Option<usize>) {
    if !is_at_last_row_of_line(document, pos) {
        return move_to_next_row_of_line(document, pos, preferred_column);
    }
    if !is_at_last_line(document, pos.to_pos()) {
        return move_to_first_row_of_next_line(document, pos, preferred_column);
    }
    (pos, preferred_column)
}

fn is_at_start_of_line(position: Pos) -> bool {
    position.byte == 0
}

fn is_at_end_of_line(document: &Document<'_>, position: Pos) -> bool {
    position.byte == document.line(position.line).text().len()
}

fn is_at_first_row_of_line(document: &Document<'_>, pos: BiasedPos) -> bool {
    document
        .line(pos.line)
        .byte_affinity_to_row_column(
            (pos.byte, pos.bias),
            document.settings().tab_column_count,
        )
        .0
        == 0
}

fn is_at_last_row_of_line(document: &Document<'_>, pos: BiasedPos) -> bool {
    let line = document.line(pos.line);
    line.byte_affinity_to_row_column(
        (pos.byte, pos.bias),
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
) -> (BiasedPos, Option<usize>) {
    use crate::str::StrExt;

    (
        BiasedPos::from_pos_and_bias(
            Pos {
                line: position.line,
                byte: document.line(position.line).text()[..position.byte]
                    .grapheme_indices()
                    .next_back()
                    .map(|(byte_index, _)| byte_index)
                    .unwrap(),
            },
            Bias::After,
        ),
        None,
    )
}

fn move_to_next_grapheme(
    document: &Document<'_>,
    position: Pos,
) -> (BiasedPos, Option<usize>) {
    use crate::str::StrExt;

    let line = document.line(position.line);
    (
        BiasedPos::from_pos_and_bias(
            Pos {
                line: position.line,
                byte: line.text()[position.byte..]
                    .grapheme_indices()
                    .nth(1)
                    .map(|(byte, _)| position.byte + byte)
                    .unwrap_or(line.text().len()),
            },
            Bias::Before,
        ),
        None,
    )
}

fn move_to_end_of_prev_line(
    document: &Document<'_>,
    position: Pos,
) -> (BiasedPos, Option<usize>) {
    let prev_line = position.line - 1;
    (
        BiasedPos::from_pos_and_bias(
            Pos {
                line: prev_line,
                byte: document.line(prev_line).text().len(),
            },
            Bias::After,
        ),
        None,
    )
}

fn move_to_start_of_next_line(position: Pos) -> (BiasedPos, Option<usize>) {
    (
        BiasedPos::from_pos_and_bias(
            Pos {
                line: position.line + 1,
                byte: 0,
            },
            Bias::Before,
        ),
        None,
    )
}

fn move_to_prev_row_of_line(
    document: &Document<'_>,
    pos: BiasedPos,
    preferred_column: Option<usize>,
) -> (BiasedPos, Option<usize>) {
    let line = document.line(pos.line);
    let (row, mut column) = line.byte_affinity_to_row_column(
        (pos.byte, pos.bias),
        document.settings().tab_column_count,
    );
    if let Some(preferred_column) = preferred_column {
        column = preferred_column;
    }
    let (byte, affinity) =
        line.row_column_to_byte_affinity((row - 1, column), document.settings().tab_column_count);
    (
        BiasedPos::from_pos_and_bias(
            Pos {
                line: pos.line,
                byte,
            },
            affinity,
        ),
        Some(column),
    )
}

fn move_to_next_row_of_line(
    document: &Document<'_>,
    pos: BiasedPos,
    preferred_column: Option<usize>,
) -> (BiasedPos, Option<usize>) {
    let line = document.line(pos.line);
    let (row, mut column) = line.byte_affinity_to_row_column(
        (pos.byte, pos.bias),
        document.settings().tab_column_count,
    );
    if let Some(preferred_column) = preferred_column {
        column = preferred_column;
    }
    let (byte, affinity) =
        line.row_column_to_byte_affinity((row + 1, column), document.settings().tab_column_count);
    (
        BiasedPos::from_pos_and_bias(
            Pos {
                line: pos.line,
                byte,
            },
            affinity,
        ),
        Some(column),
    )
}

fn move_to_last_row_of_prev_line(
    document: &Document<'_>,
    pos: BiasedPos,
    preferred_column: Option<usize>,
) -> (BiasedPos, Option<usize>) {
    let (_, mut column) = document.line(pos.line).byte_affinity_to_row_column(
        (pos.byte, pos.bias),
        document.settings().tab_column_count,
    );
    if let Some(preferred_column) = preferred_column {
        column = preferred_column;
    }
    let prev_line = pos.line - 1;
    let prev_line_ref = document.line(prev_line);
    let (byte, affinity) = prev_line_ref.row_column_to_byte_affinity(
        (prev_line_ref.row_count() - 1, column),
        document.settings().tab_column_count,
    );
    (
        BiasedPos::from_pos_and_bias(
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
    pos: BiasedPos,
    preferred_column: Option<usize>,
) -> (BiasedPos, Option<usize>) {
    let (_, mut column) = document.line(pos.line).byte_affinity_to_row_column(
        (pos.byte, pos.bias),
        document.settings().tab_column_count,
    );
    if let Some(preferred_column) = preferred_column {
        column = preferred_column;
    }
    let next_line = pos.line + 1;
    let (byte, affinity) = document
        .line(next_line)
        .row_column_to_byte_affinity((0, column), document.settings().tab_column_count);
    (
        BiasedPos::from_pos_and_bias(
            Pos {
                line: next_line,
                byte,
            },
            affinity,
        ),
        Some(column),
    )
}
