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
    pos.line_index == 0
}

fn is_at_last_line(lines: &[String], pos: Pos) -> bool {
    pos.line_index == lines.len()
}

fn is_at_start_of_line(pos: Pos) -> bool {
    pos.byte_index == 0
}

fn is_at_end_of_line(lines: &[String], pos: Pos) -> bool {
    pos.byte_index == lines[pos.line_index].len()
}

fn move_to_next_grapheme(lines: &[String], pos: Pos) -> Pos {
    use crate::StrExt;

    Pos {
        line_index: pos.line_index,
        byte_index: lines[pos.line_index]
            .next_grapheme_boundary(pos.byte_index)
            .unwrap(),
    }
}

fn move_to_prev_grapheme(lines: &[String], pos: Pos) -> Pos {
    use crate::StrExt;

    Pos {
        line_index: pos.line_index,
        byte_index: lines[pos.line_index]
            .prev_grapheme_boundary(pos.byte_index)
            .unwrap(),
    }
}

fn move_to_start_of_next_line(pos: Pos) -> Pos {
    Pos {
        line_index: pos.line_index + 1,
        byte_index: 0,
    }
}

fn move_to_end_of_prev_line(lines: &[String], pos: Pos) -> Pos {
    let prev_line_index = pos.line_index - 1;
    Pos {
        line_index: prev_line_index,
        byte_index: lines[prev_line_index].len(),
    }
}

fn move_to_next_line(lines: &[String], pos: Pos, column: Option<usize>) -> (Pos, usize) {
    let column = column.unwrap_or_else(|| {
        layout::byte_index_to_pos(&lines[pos.line_index], pos.byte_index)
            .unwrap()
            .col_index
    });
    let next_line_index = pos.line_index + 1;
    (
        Pos {
            line_index: next_line_index,
            byte_index: layout::pos_to_byte_index(
                &lines[next_line_index],
                layout::Pos {
                    row_index: 0,
                    col_index: column,
                },
            )
            .unwrap_or_else(|| lines[next_line_index].len()),
        },
        column,
    )
}

fn move_to_prev_line(lines: &[String], pos: Pos, column: Option<usize>) -> (Pos, usize) {
    let column = column.unwrap_or_else(|| {
        layout::byte_index_to_pos(&lines[pos.line_index], pos.byte_index)
            .unwrap()
            .col_index
    });
    let prev_line_index = pos.line_index - 1;
    (
        Pos {
            line_index: prev_line_index,
            byte_index: layout::pos_to_byte_index(
                &lines[prev_line_index],
                layout::Pos {
                    row_index: 0,
                    col_index: column,
                },
            )
            .unwrap_or_else(|| lines[prev_line_index].len()),
        },
        column,
    )
}

fn move_to_start_of_line(pos: Pos) -> Pos {
    Pos {
        line_index: pos.line_index,
        byte_index: 0,
    }
}

fn move_to_end_of_line(lines: &[String], pos: Pos) -> Pos {
    Pos {
        line_index: pos.line_index,
        byte_index: lines[pos.line_index].len(),
    }
}
