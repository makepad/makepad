use crate::{Position, Text};

pub fn move_left(text: &Text, position: Position) -> Position {
    if !is_at_start_of_line(position) {
        return move_to_prev_grapheme(text, position);
    }
    if !is_at_first_line(position) {
        return move_to_end_of_prev_line(text, position);
    }
    position
}

pub fn move_right(text: &Text, position: Position) -> Position {
    if !is_at_end_of_line(text, position) {
        return move_to_next_grapheme(text, position);
    }
    if !is_at_last_line(text, position) {
        return move_to_start_of_next_line(position);
    }
    position
}

fn is_at_start_of_line(position: Position) -> bool {
    position.byte_index == 0
}

fn is_at_end_of_line(text: &Text, position: Position) -> bool {
    position.byte_index == text.as_lines()[position.line_index].len()
}

fn is_at_first_line(position: Position) -> bool {
    position.line_index == 0
}

fn is_at_last_line(text: &Text, position: Position) -> bool {
    position.line_index == text.as_lines().len()
}

fn move_to_prev_grapheme(text: &Text, position: Position) -> Position {
    use crate::str::StrExt;

    Position::new(
        position.line_index,
        text.as_lines()[position.line_index][..position.byte_index]
            .grapheme_indices()
            .next_back()
            .map(|(byte_index, _)| byte_index)
            .unwrap(),
    )
}

fn move_to_next_grapheme(text: &Text, position: Position) -> Position {
    use crate::str::StrExt;

    let line = &text.as_lines()[position.line_index];
    Position::new(
        position.line_index,
        line[position.byte_index..]
            .grapheme_indices()
            .nth(1)
            .map(|(byte_index, _)| position.byte_index + byte_index)
            .unwrap_or(line.len()),
    )
}

fn move_to_end_of_prev_line(text: &Text, position: Position) -> Position {
    let prev_line_index = position.line_index - 1;
    Position::new(prev_line_index, text.as_lines()[prev_line_index].len())
}

fn move_to_start_of_next_line(position: Position) -> Position {
    Position::new(position.line_index + 1, 0)
}
