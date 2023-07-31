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
    if !cursor.biased_pos.is_at_first_row_of_line(view) {
        return move_to_prev_row_of_line(view, cursor);
    }
    if !cursor.biased_pos.pos.is_at_first_line() {
        return move_to_last_row_of_prev_line(view, cursor);
    }
    cursor
}

pub fn move_down(view: &View<'_>, cursor: Cursor) -> Cursor {
    if !cursor.biased_pos.is_at_last_row_of_line(view) {
        return move_to_next_row_of_line(view, cursor);
    }
    if !cursor
        .biased_pos
        .pos
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
            .map(|(index, _)| index)
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
            .map(|(index, _)| pos.byte + index)
            .unwrap_or(line.len()),
    }
}

fn move_to_end_of_prev_line(lines: &[String], pos: Pos) -> Pos {
    let prev_line = pos.line - 1;
    Pos {
        line: prev_line,
        byte: lines[prev_line].len(),
    }
}

fn move_to_start_of_next_line(pos: Pos) -> Pos {
    Pos {
        line: pos.line + 1,
        byte: 0,
    }
}

fn move_to_prev_row_of_line(view: &View<'_>, cursor: Cursor) -> Cursor {
    use crate::Point;

    let line = view.line(cursor.biased_pos.pos.line);
    let mut point = line.biased_byte_to_point(cursor.biased_pos.biased_byte());
    if let Some(column) = cursor.column {
        point.column = column;
    }
    let biased_byte = line.point_to_biased_byte(
        Point {
            row: point.row - 1,
            ..point
        }
    );
    Cursor {
        biased_pos: BiasedPos::from_line_and_biased_byte(cursor.biased_pos.pos.line, biased_byte),
        column: Some(point.column),
    }
}

fn move_to_next_row_of_line(view: &View<'_>, cursor: Cursor) -> Cursor {
    use crate::Point;

    let line = view.line(cursor.biased_pos.pos.line);
    let mut point = line.biased_byte_to_point(cursor.biased_pos.biased_byte());
    if let Some(column) = cursor.column {
        point.column = column;
    }
    let biased_byte = line.point_to_biased_byte(
        Point {
            row: point.row + 1,
            ..point
        }
    );
    Cursor {
        biased_pos: BiasedPos::from_line_and_biased_byte(cursor.biased_pos.pos.line, biased_byte),
        column: Some(point.column),
    }
}

fn move_to_last_row_of_prev_line(view: &View<'_>, cursor: Cursor) -> Cursor {
    use crate::Point;

    let mut point = view
        .line(cursor.biased_pos.pos.line)
        .biased_byte_to_point(cursor.biased_pos.biased_byte());
    if let Some(column) = cursor.column {
        point.column = column;
    }
    let prev_line = cursor.biased_pos.pos.line - 1;
    let prev_line_ref = view.line(prev_line);
    let biased_byte = prev_line_ref.point_to_biased_byte(
        Point {
            row: prev_line_ref.height() - 1,
            column: point.column,
        }
    );
    Cursor {
        biased_pos: BiasedPos::from_line_and_biased_byte(prev_line, biased_byte),
        column: Some(point.column),
    }
}

fn move_to_first_row_of_next_line(view: &View<'_>, cursor: Cursor) -> Cursor {
    use crate::Point;

    let mut point = view
        .line(cursor.biased_pos.pos.line)
        .biased_byte_to_point(cursor.biased_pos.biased_byte());
    if let Some(column) = cursor.column {
        point.column = column;
    }
    let next_line = cursor.biased_pos.pos.line + 1;
    let biased_byte = view.line(next_line).point_to_biased_byte(
        Point {
            row: 0,
            column: point.column,
        }
    );
    Cursor {
        biased_pos: BiasedPos::from_line_and_biased_byte(next_line, biased_byte),
        column: Some(point.column),
    }
}
