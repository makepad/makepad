use crate::{state::View, Position, Text};

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

pub fn move_up(
    view: &View<'_>,
    position: Position,
    column_index: Option<usize>,
) -> (Position, Option<usize>) {
    if !is_at_first_row_of_line(view, position) {
        let (position, column_index) = move_to_prev_row_of_line(view, position, column_index);
        return (position, Some(column_index));
    }
    if !is_at_first_line(position) {
        let (position, column_index) = move_to_last_row_of_prev_line(view, position, column_index);
        return (position, Some(column_index));
    }
    (position, column_index)
}

pub fn move_down(
    view: &View<'_>,
    position: Position,
    column_index: Option<usize>,
) -> (Position, Option<usize>) {
    if !is_at_last_row_of_line(view, position) {
        let (position, column_index) = move_to_next_row_of_line(view, position, column_index);
        return (position, Some(column_index));
    }
    if !is_at_first_line(position) {
        let (position, column_index) = move_to_first_row_of_next_line(view, position, column_index);
        return (position, Some(column_index));
    }
    (position, column_index)
}

fn is_at_start_of_line(position: Position) -> bool {
    position.byte_index == 0
}

fn is_at_end_of_line(text: &Text, position: Position) -> bool {
    position.byte_index == text.as_lines()[position.line_index].len()
}

fn is_at_first_row_of_line(view: &View<'_>, position: Position) -> bool {
    view.line(position.line_index)
        .is_at_first_row(position.byte_index)
}

fn is_at_last_row_of_line(view: &View<'_>, position: Position) -> bool {
    view.line(position.line_index)
        .is_at_last_row(position.byte_index)
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

fn move_to_prev_row_of_line(
    view: &View,
    position: Position,
    column_index: Option<usize>,
) -> (Position, usize) {
    let line = view.line(position.line_index);
    let (row_index, column_index) = if let Some(column_index) = column_index {
        let (row_index, _) = line
            .byte_index_to_row_column_index(position.byte_index, view.settings().tab_column_count);
        (row_index, column_index)
    } else {
        line.byte_index_to_row_column_index(position.byte_index, view.settings().tab_column_count)
    };
    (
        Position::new(
            position.line_index,
            line.row_column_index_to_byte_index(
                row_index - 1,
                column_index,
                view.settings().tab_column_count,
            ),
        ),
        column_index,
    )
}

fn move_to_next_row_of_line(
    view: &View,
    position: Position,
    column_index: Option<usize>,
) -> (Position, usize) {
    let line = view.line(position.line_index);
    let (row_index, column_index) = if let Some(column_index) = column_index {
        let (row_index, _) = line
            .byte_index_to_row_column_index(position.byte_index, view.settings().tab_column_count);
        (row_index, column_index)
    } else {
        line.byte_index_to_row_column_index(position.byte_index, view.settings().tab_column_count)
    };
    (
        Position::new(
            position.line_index,
            line.row_column_index_to_byte_index(
                row_index + 1,
                column_index,
                view.settings().tab_column_count,
            ),
        ),
        column_index,
    )
}

fn move_to_last_row_of_prev_line(
    view: &View<'_>,
    position: Position,
    column_index: Option<usize>,
) -> (Position, usize) {
    let column_index = column_index.unwrap_or_else(|| {
        let (_, column_index) = view
            .line(position.line_index)
            .byte_index_to_row_column_index(position.byte_index, view.settings().tab_column_count);
        column_index
    });
    let prev_line_index = position.line_index - 1;
    let prev_line = view.line(prev_line_index);
    (
        Position::new(
            prev_line_index,
            prev_line.row_column_index_to_byte_index(
                prev_line.row_count() - 1,
                column_index,
                view.settings().tab_column_count,
            ),
        ),
        column_index,
    )
}

fn move_to_first_row_of_next_line(
    view: &View<'_>,
    position: Position,
    column_index: Option<usize>,
) -> (Position, usize) {
    let column_index = column_index.unwrap_or_else(|| {
        let (_, column_index) = view
            .line(position.line_index)
            .byte_index_to_row_column_index(position.byte_index, view.settings().tab_column_count);
        column_index
    });
    let next_line_index = position.line_index + 1;
    (
        Position::new(
            next_line_index,
            view.line(next_line_index).row_column_index_to_byte_index(
                0,
                column_index,
                view.settings().tab_column_count,
            ),
        ),
        column_index,
    )
}
