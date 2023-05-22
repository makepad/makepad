use crate::{Pos, Text};

pub fn move_left(text: &Text, pos: Pos) -> Pos {
    if !is_at_start_of_line(pos) {
        move_to_prev_grapheme(text, pos)
    } else if !is_at_first_line(pos) {
        move_to_end_of_prev_line(text, pos)
    } else {
        pos
    }
}

pub fn move_right(text: &Text, pos: Pos) -> Pos {
    if !is_at_end_of_line(text, pos) {
        move_to_next_grapheme(text, pos)
    } else if !is_at_last_line(text, pos) {
        move_to_start_of_next_line(pos)
    } else {
        pos
    }
}

pub fn move_up(text: &Text, pos: Pos, column: Option<usize>) -> (Pos, Option<usize>) {
    if !is_at_first_line(pos) {
        let (pos, column) = move_to_prev_line(text, pos, column);
        (pos, Some(column))
    } else {
        (move_to_start_of_line(pos), None)
    }
}

pub fn move_down(text: &Text, pos: Pos, column: Option<usize>) -> (Pos, Option<usize>) {
    if !is_at_last_line(text, pos) {
        let (pos, column) = move_to_next_line(text, pos, column);
        (pos, Some(column))
    } else {
        (move_to_end_of_line(text, pos), None)
    }
}

fn is_at_first_line(pos: Pos) -> bool {
    pos.line == 0
}

fn is_at_last_line(text: &Text, pos: Pos) -> bool {
    pos.line == text.as_lines().len()
}

fn is_at_start_of_line(pos: Pos) -> bool {
    pos.byte == 0
}

fn is_at_end_of_line(text: &Text, pos: Pos) -> bool {
    pos.byte == text.as_lines()[pos.line].len()
}

fn move_to_next_grapheme(text: &Text, pos: Pos) -> Pos {
    use crate::StrExt;

    Pos {
        line: pos.line,
        byte: text.as_lines()[pos.line]
            .next_grapheme_boundary(pos.byte)
            .unwrap(),
    }
}

fn move_to_prev_grapheme(text: &Text, pos: Pos) -> Pos {
    use crate::StrExt;

    Pos {
        line: pos.line,
        byte: text.as_lines()[pos.line]
            .prev_grapheme_boundary(pos.byte)
            .unwrap(),
    }
}

fn move_to_start_of_next_line(pos: Pos) -> Pos {
    Pos {
        line: pos.line + 1,
        byte: 0,
    }
}

fn move_to_end_of_prev_line(text: &Text, pos: Pos) -> Pos {
    let prev_line = pos.line - 1;
    Pos {
        line: prev_line,
        byte: text.as_lines()[prev_line].len(),
    }
}

fn move_to_next_line(text: &Text, pos: Pos, column: Option<usize>) -> (Pos, usize) {
    let column = column.unwrap_or_else(|| byte_to_column(text, pos.line, pos.byte));
    let next_line = pos.line + 1;
    (
        Pos {
            line: next_line,
            byte: column_to_byte(text, next_line, column),
        },
        column,
    )
}

fn move_to_prev_line(text: &Text, pos: Pos, column: Option<usize>) -> (Pos, usize) {
    let column = column.unwrap_or_else(|| byte_to_column(text, pos.line, pos.byte));
    let prev_line = pos.line - 1;
    (
        Pos {
            line: prev_line,
            byte: column_to_byte(text, prev_line, column),
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

fn move_to_end_of_line(text: &Text, pos: Pos) -> Pos {
    Pos {
        line: pos.line,
        byte: text.as_lines()[pos.line].len(),
    }
}

fn byte_to_column(text: &Text, line: usize, byte: usize) -> usize {
    use {crate::layout, std::ops::ControlFlow};

    match layout::layout(&text.as_lines()[line], |event| {
        if event.byte_pos == byte {
            return ControlFlow::Break(event.pos.column);
        }
        ControlFlow::Continue(())
    }) {
        ControlFlow::Break(column) => column,
        _ => panic!(),
    }
}

fn column_to_byte(text: &Text, line: usize, column: usize) -> usize {
    column.min(text.as_lines()[line].len())
}
