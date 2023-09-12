use crate::{
    layout::Layout,
    selection::{Affinity, Cursor},
    str::StrExt,
    text::Position,
};

pub fn move_left(cursor: Cursor, layout: &Layout<'_>) -> Cursor {
    if !is_at_start_of_line(cursor) {
        return move_to_prev_grapheme(cursor, layout);
    }
    if !is_at_first_line(cursor) {
        return move_to_end_of_prev_line(cursor, layout);
    }
    cursor
}

pub fn move_right(cursor: Cursor, layout: &Layout<'_>) -> Cursor {
    if !is_at_end_of_line(cursor, layout) {
        return move_to_next_grapheme(cursor, layout);
    }
    if !is_at_last_line(cursor, layout) {
        return move_to_start_of_next_line(cursor);
    }
    cursor
}

pub fn move_up(cursor: Cursor, layout: &Layout<'_>, tab_column_count: usize) -> Cursor {
    if !is_at_first_row_of_line(cursor, layout, tab_column_count) {
        return move_to_prev_row_of_line(cursor, layout, tab_column_count);
    }
    if !is_at_first_line(cursor) {
        return move_to_last_row_of_prev_line(cursor, layout, tab_column_count);
    }
    cursor
}

pub fn move_down(cursor: Cursor, layout: &Layout<'_>, tab_column_count: usize) -> Cursor {
    if !is_at_last_row_of_line(cursor, layout, tab_column_count) {
        return move_to_next_row_of_line(cursor, layout, tab_column_count);
    }
    if !is_at_last_line(cursor, layout) {
        return move_to_first_row_of_next_line(cursor, layout, tab_column_count);
    }
    cursor
}

fn is_at_first_line(cursor: Cursor) -> bool {
    cursor.position.line_index == 0
}

fn is_at_last_line(cursor: Cursor, layout: &Layout<'_>) -> bool {
    cursor.position.line_index == layout.as_text().as_lines().len()
}

fn is_at_start_of_line(cursor: Cursor) -> bool {
    cursor.position.byte_index == 0
}

fn is_at_end_of_line(cursor: Cursor, layout: &Layout<'_>) -> bool {
    cursor.position.byte_index == layout.as_text().as_lines()[cursor.position.line_index].len()
}

fn is_at_first_row_of_line(cursor: Cursor, layout: &Layout<'_>, tab_column_count: usize) -> bool {
    let line = layout.line(cursor.position.line_index);
    let (row, _) = line.logical_to_visual_position(
        cursor.position.byte_index,
        cursor.affinity,
        tab_column_count,
    );
    row == 0
}

fn is_at_last_row_of_line(cursor: Cursor, layout: &Layout<'_>, tab_column_count: usize) -> bool {
    let line = layout.line(cursor.position.line_index);
    let (row, _) = line.logical_to_visual_position(
        cursor.position.byte_index,
        cursor.affinity,
        tab_column_count,
    );
    row == line.row_count() - 1
}

fn move_to_prev_grapheme(cursor: Cursor, layout: &Layout<'_>) -> Cursor {
    Cursor {
        position: Position {
            line_index: cursor.position.line_index,
            byte_index: layout.as_text().as_lines()[cursor.position.line_index]
                [..cursor.position.byte_index]
                .grapheme_indices()
                .next_back()
                .map(|(index, _)| index)
                .unwrap(),
        },
        affinity: Affinity::Before,
        preferred_column_index: None,
    }
}

fn move_to_next_grapheme(cursor: Cursor, layout: &Layout<'_>) -> Cursor {
    let line = &layout.as_text().as_lines()[cursor.position.line_index];
    Cursor {
        position: Position {
            line_index: cursor.position.line_index,
            byte_index: line[cursor.position.byte_index..]
                .grapheme_indices()
                .nth(1)
                .map(|(index, _)| cursor.position.byte_index + index)
                .unwrap_or(line.len()),
        },
        affinity: Affinity::After,
        preferred_column_index: None,
    }
}

fn move_to_end_of_prev_line(cursor: Cursor, layout: &Layout<'_>) -> Cursor {
    let prev_line = cursor.position.line_index - 1;
    Cursor {
        position: Position {
            line_index: prev_line,
            byte_index: layout.as_text().as_lines()[prev_line].len(),
        },
        affinity: Affinity::Before,
        preferred_column_index: None,
    }
}

fn move_to_start_of_next_line(cursor: Cursor) -> Cursor {
    Cursor {
        position: Position {
            line_index: cursor.position.line_index + 1,
            byte_index: 0,
        },
        affinity: Affinity::After,
        preferred_column_index: None,
    }
}

fn move_to_prev_row_of_line(
    cursor: Cursor,
    layout: &Layout<'_>,
    tab_column_count: usize,
) -> Cursor {
    let line = layout.line(cursor.position.line_index);
    let (row_index, mut column_index) = line.logical_to_visual_position(
        cursor.position.byte_index,
        cursor.affinity,
        tab_column_count,
    );
    if let Some(preferred_column_index) = cursor.preferred_column_index {
        column_index = preferred_column_index;
    }
    let (byte_index, affinity) =
        line.visual_to_logical_position(row_index - 1, column_index, tab_column_count);
    Cursor {
        position: Position {
            line_index: cursor.position.line_index,
            byte_index,
        },
        affinity,
        preferred_column_index: Some(column_index),
    }
}

fn move_to_next_row_of_line(
    cursor: Cursor,
    layout: &Layout<'_>,
    tab_column_count: usize,
) -> Cursor {
    let line = layout.line(cursor.position.line_index);
    let (row_index, mut column_index) = line.logical_to_visual_position(
        cursor.position.byte_index,
        cursor.affinity,
        tab_column_count,
    );
    if let Some(preferred_column_index) = cursor.preferred_column_index {
        column_index = preferred_column_index;
    }
    let (byte, affinity) =
        line.visual_to_logical_position(row_index + 1, column_index, tab_column_count);
    Cursor {
        position: Position {
            line_index: cursor.position.line_index,
            byte_index: byte,
        },
        affinity,
        preferred_column_index: Some(column_index),
    }
}

fn move_to_last_row_of_prev_line(
    cursor: Cursor,
    layout: &Layout<'_>,
    tab_column_count: usize,
) -> Cursor {
    let line = layout.line(cursor.position.line_index);
    let (_, mut column_index) = line.logical_to_visual_position(
        cursor.position.byte_index,
        cursor.affinity,
        tab_column_count,
    );
    if let Some(preferred_column_index) = cursor.preferred_column_index {
        column_index = preferred_column_index;
    }
    let prev_line = layout.line(cursor.position.line_index - 1);
    let (byte_index, affinity) = prev_line.visual_to_logical_position(
        prev_line.row_count() - 1,
        column_index,
        tab_column_count,
    );
    Cursor {
        position: Position {
            line_index: cursor.position.line_index - 1,
            byte_index,
        },
        affinity,
        preferred_column_index: Some(column_index),
    }
}

fn move_to_first_row_of_next_line(
    cursor: Cursor,
    layout: &Layout<'_>,
    tab_column_count: usize,
) -> Cursor {
    let line = layout.line(cursor.position.line_index);
    let (_, mut column_index) = line.logical_to_visual_position(
        cursor.position.byte_index,
        cursor.affinity,
        tab_column_count,
    );
    if let Some(preferred_column_index) = cursor.preferred_column_index {
        column_index = preferred_column_index;
    }
    let next_line = layout.line(cursor.position.line_index + 1);
    let (byte_index, affinity) =
        next_line.visual_to_logical_position(0, column_index, tab_column_count);
    Cursor {
        position: Position {
            line_index: cursor.position.line_index + 1,
            byte_index,
        },
        affinity,
        preferred_column_index: Some(column_index),
    }
}
