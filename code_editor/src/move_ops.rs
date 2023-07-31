use crate::{Cursor, BiasedPos, Document, Pos};

pub fn move_left(document: &Document<'_>, pos: Pos) -> Pos {
    if !is_at_start_of_line(pos) {
        return move_to_prev_grapheme(document, pos);
    }
    if !is_at_first_line(pos) {
        return move_to_end_of_prev_line(document, pos);
    }
    pos
}

pub fn move_right(document: &Document<'_>, pos: Pos) -> Pos {
    if !is_at_end_of_line(document, pos) {
        return move_to_next_grapheme(document, pos);
    }
    if !is_at_last_line(document, pos) {
        return move_to_start_of_next_line(pos);
    }
    pos
}

pub fn move_up(
    document: &Document<'_>,
    cursor: Cursor,
) -> Cursor {
    if !is_at_first_row_of_line(document, cursor.pos) {
        return move_to_prev_row_of_line(document, cursor);
    }
    if !is_at_first_line(cursor.pos.to_pos()) {
        return move_to_last_row_of_prev_line(document, cursor);
    }
    cursor
}

pub fn move_down(
    document: &Document<'_>,
    cursor: Cursor,
) -> Cursor {
    if !is_at_last_row_of_line(document, cursor.pos) {
        return move_to_next_row_of_line(document, cursor);
    }
    if !is_at_last_line(document, cursor.pos.to_pos()) {
        return move_to_first_row_of_next_line(document, cursor);
    }
    cursor
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
        .byte_affinity_to_row_column((pos.byte, pos.bias), document.settings().tab_column_count)
        .0
        == 0
}

fn is_at_last_row_of_line(document: &Document<'_>, pos: BiasedPos) -> bool {
    let line = document.line(pos.line);
    line.byte_affinity_to_row_column((pos.byte, pos.bias), document.settings().tab_column_count)
        .0
        == line.row_count() - 1
}

fn is_at_first_line(position: Pos) -> bool {
    position.line == 0
}

fn is_at_last_line(document: &Document<'_>, position: Pos) -> bool {
    position.line == document.line_count() - 1
}

fn move_to_prev_grapheme(document: &Document<'_>, position: Pos) -> Pos {
    use crate::str::StrExt;

    Pos {
        line: position.line,
        byte: document.line(position.line).text()[..position.byte]
            .grapheme_indices()
            .next_back()
            .map(|(byte_index, _)| byte_index)
            .unwrap(),
    }
}

fn move_to_next_grapheme(document: &Document<'_>, pos: Pos) -> Pos {
    use crate::str::StrExt;

    let line = document.line(pos.line);
    Pos {
        line: pos.line,
        byte: line.text()[pos.byte..]
            .grapheme_indices()
            .nth(1)
            .map(|(byte, _)| pos.byte + byte)
            .unwrap_or(line.text().len()),
    }
}

fn move_to_end_of_prev_line(document: &Document<'_>, pos: Pos) -> Pos {
    let prev_line = pos.line - 1;
    Pos {
        line: prev_line,
        byte: document.line(prev_line).text().len(),
    }
}

fn move_to_start_of_next_line(position: Pos) -> Pos {
    Pos {
        line: position.line + 1,
        byte: 0,
    }
}

fn move_to_prev_row_of_line(
    document: &Document<'_>,
    cursor: Cursor,
) -> Cursor {
    let line = document.line(cursor.pos.line);
    let (row, mut column) = line
        .byte_affinity_to_row_column((cursor.pos.byte, cursor.pos.bias), document.settings().tab_column_count);
    if let Some(preferred_column) = cursor.col {
        column = preferred_column;
    }
    let (byte, affinity) =
        line.row_column_to_byte_affinity((row - 1, column), document.settings().tab_column_count);
    Cursor {
        pos: BiasedPos::from_pos_and_bias(
            Pos {
                line: cursor.pos.line,
                byte,
            },
            affinity,
        ),
        col: Some(column),
    }
}

fn move_to_next_row_of_line(
    document: &Document<'_>,
    cursor: Cursor,
) -> Cursor {
    let line = document.line(cursor.pos.line);
    let (row, mut column) = line
        .byte_affinity_to_row_column((cursor.pos.byte, cursor.pos.bias), document.settings().tab_column_count);
    if let Some(preferred_column) = cursor.col {
        column = preferred_column;
    }
    let (byte, affinity) =
        line.row_column_to_byte_affinity((row + 1, column), document.settings().tab_column_count);
    Cursor {
        pos: BiasedPos::from_pos_and_bias(
            Pos {
                line: cursor.pos.line,
                byte,
            },
            affinity,
        ),
        col: Some(column),
    }
}

fn move_to_last_row_of_prev_line(
    document: &Document<'_>,
    cursor: Cursor,
) -> Cursor {
    let (_, mut column) = document
        .line(cursor.pos.line)
        .byte_affinity_to_row_column((cursor.pos.byte, cursor.pos.bias), document.settings().tab_column_count);
    if let Some(preferred_column) = cursor.col {
        column = preferred_column;
    }
    let prev_line = cursor.pos.line - 1;
    let prev_line_ref = document.line(prev_line);
    let (byte, affinity) = prev_line_ref.row_column_to_byte_affinity(
        (prev_line_ref.row_count() - 1, column),
        document.settings().tab_column_count,
    );
    Cursor {
        pos: BiasedPos::from_pos_and_bias(
            Pos {
                line: prev_line,
                byte,
            },
            affinity,
        ),
        col: Some(column),
    }
}

fn move_to_first_row_of_next_line(
    document: &Document<'_>,
    cursor: Cursor
) -> Cursor {
    let (_, mut column) = document
        .line(cursor.pos.line)
        .byte_affinity_to_row_column((cursor.pos.byte, cursor.pos.bias), document.settings().tab_column_count);
    if let Some(preferred_column) = cursor.col {
        column = preferred_column;
    }
    let next_line = cursor.pos.line + 1;
    let (byte, affinity) = document
        .line(next_line)
        .row_column_to_byte_affinity((0, column), document.settings().tab_column_count);
    Cursor {
        pos: BiasedPos::from_pos_and_bias(
            Pos {
                line: next_line,
                byte,
            },
            affinity,
        ),
        col: Some(column),
    }
}
