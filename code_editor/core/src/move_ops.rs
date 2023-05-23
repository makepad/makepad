use crate::{layout, text::Pos};

#[derive(Debug)]
pub struct Context<'a> {
    pub lines: &'a [String],
}

impl<'a> Context<'a> {
    fn to_layout_context(&self, line_pos: usize) -> layout::Context<'a> {
        layout::Context {
            line: &self.lines[line_pos],
        }
    }
}

pub fn move_left(context: &Context<'_>, pos: Pos) -> Pos {
    if !is_at_start_of_line(pos) {
        move_to_prev_grapheme(context, pos)
    } else if !is_at_first_line(pos) {
        move_to_end_of_prev_line(context, pos)
    } else {
        pos
    }
}

pub fn move_right(context: &Context<'_>, pos: Pos) -> Pos {
    if !is_at_end_of_line(context, pos) {
        move_to_next_grapheme(context, pos)
    } else if !is_at_last_line(context, pos) {
        move_to_start_of_next_line(pos)
    } else {
        pos
    }
}

pub fn move_up(context: &Context<'_>, pos: Pos, column: Option<usize>) -> (Pos, Option<usize>) {
    if !is_at_first_line(pos) {
        let (pos, column) = move_to_prev_line(context, pos, column);
        (pos, Some(column))
    } else {
        (move_to_start_of_line(pos), None)
    }
}

pub fn move_down(context: &Context<'_>, pos: Pos, column: Option<usize>) -> (Pos, Option<usize>) {
    if !is_at_last_line(context, pos) {
        let (pos, column) = move_to_next_line(context, pos, column);
        (pos, Some(column))
    } else {
        (move_to_end_of_line(context, pos), None)
    }
}

fn is_at_first_line(pos: Pos) -> bool {
    pos.line == 0
}

fn is_at_last_line(context: &Context<'_>, pos: Pos) -> bool {
    pos.line == context.lines.len()
}

fn is_at_start_of_line(pos: Pos) -> bool {
    pos.byte == 0
}

fn is_at_end_of_line(context: &Context<'_>, pos: Pos) -> bool {
    pos.byte == context.lines[pos.line].len()
}

fn move_to_next_grapheme(context: &Context<'_>, pos: Pos) -> Pos {
    use crate::StrExt;

    Pos {
        line: pos.line,
        byte: context.lines[pos.line]
            .next_grapheme_boundary(pos.byte)
            .unwrap(),
    }
}

fn move_to_prev_grapheme(context: &Context<'_>, pos: Pos) -> Pos {
    use crate::StrExt;

    Pos {
        line: pos.line,
        byte: context.lines[pos.line]
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

fn move_to_end_of_prev_line(context: &Context<'_>, pos: Pos) -> Pos {
    let prev_line_pos = pos.line - 1;
    Pos {
        line: prev_line_pos,
        byte: context.lines[prev_line_pos].len(),
    }
}

fn move_to_next_line(context: &Context<'_>, pos: Pos, column: Option<usize>) -> (Pos, usize) {
    let column = column.unwrap_or_else(|| {
        layout::byte_pos_to_pos(&context.to_layout_context(pos.line), pos.byte)
            .unwrap()
            .column
    });
    let next_line_pos = pos.line + 1;
    (
        Pos {
            line: next_line_pos,
            byte: layout::pos_to_byte_pos(
                &context.to_layout_context(next_line_pos),
                layout::Pos { row: 0, column },
            )
            .unwrap_or_else(|| context.lines[next_line_pos].len()),
        },
        column,
    )
}

fn move_to_prev_line(context: &Context<'_>, pos: Pos, column: Option<usize>) -> (Pos, usize) {
    let column = column.unwrap_or_else(|| {
        layout::byte_pos_to_pos(&context.to_layout_context(pos.line), pos.byte)
            .unwrap()
            .column
    });
    let prev_line_pos = pos.line - 1;
    (
        Pos {
            line: prev_line_pos,
            byte: layout::pos_to_byte_pos(
                &context.to_layout_context(prev_line_pos),
                layout::Pos { row: 0, column },
            )
            .unwrap_or_else(|| context.lines[prev_line_pos].len()),
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

fn move_to_end_of_line(context: &Context<'_>, pos: Pos) -> Pos {
    Pos {
        line: pos.line,
        byte: context.lines[pos.line].len(),
    }
}
