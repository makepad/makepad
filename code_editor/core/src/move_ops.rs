use crate::{layout, text::Pos};

pub fn move_left(lines: &[String], pos: Pos) -> Pos {
    if !is_at_start_of_line(pos) {
        move_to_prev_grapheme(lines, pos)
    } else if !is_at_first_line(pos) {
        move_to_end_of_prev_line(lines, pos)
    } else {
        pos
    }
}

pub fn move_right(lines: &[String], pos: Pos) -> Pos {
    if !is_at_end_of_line(lines, pos) {
        move_to_next_grapheme(lines, pos)
    } else if !is_at_last_line(lines, pos) {
        move_to_start_of_next_line(pos)
    } else {
        pos
    }
}

pub fn move_up(lines: &[String], pos: Pos, column: Option<usize>) -> (Pos, Option<usize>) {
    if !is_at_first_line(pos) {
        let (pos, column) = move_to_prev_line(lines, pos, column);
        (pos, Some(column))
    } else {
        (move_to_start_of_line(pos), None)
    }
}

pub fn move_down(lines: &[String], pos: Pos, column: Option<usize>) -> (Pos, Option<usize>) {
    if !is_at_last_line(lines, pos) {
        let (pos, column) = move_to_next_line(lines, pos, column);
        (pos, Some(column))
    } else {
        (move_to_end_of_line(lines, pos), None)
    }
}

fn is_at_first_line(pos: Pos) -> bool {
    pos.line == 0
}

fn is_at_last_line(lines: &[String], pos: Pos) -> bool {
    pos.line == lines.len()
}

fn is_at_start_of_line(pos: Pos) -> bool {
    pos.byte == 0
}

fn is_at_end_of_line(lines: &[String], pos: Pos) -> bool {
    pos.byte == lines[pos.line].len()
}

fn move_to_next_grapheme(lines: &[String], pos: Pos) -> Pos {
    use crate::StrExt;

    Pos {
        line: pos.line,
        byte: lines[pos.line].next_grapheme_boundary(pos.byte).unwrap(),
    }
}

fn move_to_prev_grapheme(lines: &[String], pos: Pos) -> Pos {
    use crate::StrExt;

    Pos {
        line: pos.line,
        byte: lines[pos.line].prev_grapheme_boundary(pos.byte).unwrap(),
    }
}

fn move_to_start_of_next_line(pos: Pos) -> Pos {
    Pos {
        line: pos.line + 1,
        byte: 0,
    }
}

fn move_to_end_of_prev_line(lines: &[String], pos: Pos) -> Pos {
    let prev_line_pos = pos.line - 1;
    Pos {
        line: prev_line_pos,
        byte: lines[prev_line_pos].len(),
    }
}

fn move_to_next_line(lines: &[String], pos: Pos, column: Option<usize>) -> (Pos, usize) {
    let column = column.unwrap_or_else(|| {
        layout::byte_pos_to_pos(&lines[pos.line], pos.byte)
            .unwrap()
            .column
    });
    let next_line_pos = pos.line + 1;
    (
        Pos {
            line: next_line_pos,
            byte: layout::pos_to_byte_pos(&lines[next_line_pos], layout::Pos { row: 0, column })
                .unwrap_or_else(|| lines[next_line_pos].len()),
        },
        column,
    )
}

fn move_to_prev_line(lines: &[String], pos: Pos, column: Option<usize>) -> (Pos, usize) {
    let column = column.unwrap_or_else(|| {
        layout::byte_pos_to_pos(&lines[pos.line], pos.byte)
            .unwrap()
            .column
    });
    let prev_line_pos = pos.line - 1;
    (
        Pos {
            line: prev_line_pos,
            byte: layout::pos_to_byte_pos(&lines[prev_line_pos], layout::Pos { row: 0, column })
                .unwrap_or_else(|| lines[prev_line_pos].len()),
        },
        column,
    )
}

fn move_to_start_of_line(pos: Pos) -> Pos {
    Pos {
        line: pos.line,
        byte: 0,
    }
}

fn move_to_end_of_line(lines: &[String], pos: Pos) -> Pos {
    Pos {
        line: pos.line,
        byte: lines[pos.line].len(),
    }
}
