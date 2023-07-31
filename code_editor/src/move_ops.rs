use crate::{BiasedPos, Cursor, Pos, View};

pub fn move_left(lines: &[String], pos: Pos) -> Pos {
    if !pos.is_at_start_of_line() {
        return move_to_prev_grapheme(lines, pos);
    }
    if !pos.is_at_first_line() {
        return move_to_end_of_prev_line(lines, pos);
    }
    pos
}

pub fn move_right(lines: &[String], pos: Pos) -> Pos {
    if !pos.is_at_end_of_line(lines) {
        return move_to_next_grapheme(lines, pos);
    }
    if !pos.is_at_last_line(lines.len()) {
        return move_to_start_of_next_line(pos);
    }
    pos
}

pub fn move_up(view: &View<'_>, cursor: Cursor) -> Cursor {
    if !cursor.pos.is_at_first_row_of_line(view) {
        return move_to_prev_row_of_line(view, cursor);
    }
    if !cursor.pos.to_pos().is_at_first_line() {
        return move_to_last_row_of_prev_line(view, cursor);
    }
    cursor
}

pub fn move_down(view: &View<'_>, cursor: Cursor) -> Cursor {
    if !cursor.pos.is_at_last_row_of_line(view) {
        return move_to_next_row_of_line(view, cursor);
    }
    if !cursor
        .pos
        .to_pos()
        .is_at_last_line(view.text().as_lines().len())
    {
        return move_to_first_row_of_next_line(view, cursor);
    }
    cursor
}

fn move_to_prev_grapheme(lines: &[String], pos: Pos) -> Pos {
    use crate::str::StrExt;

    Pos {
        line: pos.line,
        byte: lines[pos.line][..pos.byte]
            .grapheme_indices()
            .next_back()
            .map(|(byte_index, _)| byte_index)
            .unwrap(),
    }
}

fn move_to_next_grapheme(lines: &[String], pos: Pos) -> Pos {
    use crate::str::StrExt;

    let line = &lines[pos.line];
    Pos {
        line: pos.line,
        byte: line[pos.byte..]
            .grapheme_indices()
            .nth(1)
            .map(|(byte, _)| pos.byte + byte)
            .unwrap_or(line.len()),
    }
}

fn move_to_end_of_prev_line(lines: &[String], pos: Pos) -> Pos {
    let prev_line_pos = pos.line - 1;
    Pos {
        line: prev_line_pos,
        byte: lines[prev_line_pos].len(),
    }
}

fn move_to_start_of_next_line(pos: Pos) -> Pos {
    Pos {
        line: pos.line + 1,
        byte: 0,
    }
}

fn move_to_prev_row_of_line(view: &View<'_>, cursor: Cursor) -> Cursor {
    let line = view.line(cursor.pos.line);
    let (row, mut column) = line.byte_bias_to_row_column(
        (cursor.pos.byte, cursor.pos.bias),
        view.settings().tab_column_count,
    );
    if let Some(preferred_column) = cursor.col {
        column = preferred_column;
    }
    let (byte, bias) =
        line.row_column_to_byte_bias((row - 1, column), view.settings().tab_column_count);
    Cursor {
        pos: BiasedPos::from_pos_and_bias(
            Pos {
                line: cursor.pos.line,
                byte,
            },
            bias,
        ),
        col: Some(column),
    }
}

fn move_to_next_row_of_line(view: &View<'_>, cursor: Cursor) -> Cursor {
    let line = view.line(cursor.pos.line);
    let (row, mut column) = line.byte_bias_to_row_column(
        (cursor.pos.byte, cursor.pos.bias),
        view.settings().tab_column_count,
    );
    if let Some(preferred_column) = cursor.col {
        column = preferred_column;
    }
    let (byte, bias) =
        line.row_column_to_byte_bias((row + 1, column), view.settings().tab_column_count);
    Cursor {
        pos: BiasedPos::from_pos_and_bias(
            Pos {
                line: cursor.pos.line,
                byte,
            },
            bias,
        ),
        col: Some(column),
    }
}

fn move_to_last_row_of_prev_line(view: &View<'_>, cursor: Cursor) -> Cursor {
    let (_, mut column) = view.line(cursor.pos.line).byte_bias_to_row_column(
        (cursor.pos.byte, cursor.pos.bias),
        view.settings().tab_column_count,
    );
    if let Some(preferred_column) = cursor.col {
        column = preferred_column;
    }
    let prev_line = cursor.pos.line - 1;
    let prev_line_ref = view.line(prev_line);
    let (byte, bias) = prev_line_ref.row_column_to_byte_bias(
        (prev_line_ref.row_count() - 1, column),
        view.settings().tab_column_count,
    );
    Cursor {
        pos: BiasedPos::from_pos_and_bias(
            Pos {
                line: prev_line,
                byte,
            },
            bias,
        ),
        col: Some(column),
    }
}

fn move_to_first_row_of_next_line(view: &View<'_>, cursor: Cursor) -> Cursor {
    let (_, mut column) = view.line(cursor.pos.line).byte_bias_to_row_column(
        (cursor.pos.byte, cursor.pos.bias),
        view.settings().tab_column_count,
    );
    if let Some(preferred_column) = cursor.col {
        column = preferred_column;
    }
    let next_line = cursor.pos.line + 1;
    let (byte, bias) = view
        .line(next_line)
        .row_column_to_byte_bias((0, column), view.settings().tab_column_count);
    Cursor {
        pos: BiasedPos::from_pos_and_bias(
            Pos {
                line: next_line,
                byte,
            },
            bias,
        ),
        col: Some(column),
    }
}
