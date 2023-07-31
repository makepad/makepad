use crate::{BiasedTextPos, Cursor, TextPos, View};

pub fn move_left(lines: &[String], pos: TextPos) -> TextPos {
    if !pos.is_at_start_of_line() {
        return move_to_prev_grapheme(lines, pos);
    }
    if !pos.is_at_first_line() {
        return move_to_end_of_prev_line(lines, pos);
    }
    pos
}

pub fn move_right(lines: &[String], pos: TextPos) -> TextPos {
    if !pos.is_at_end_of_line(lines) {
        return move_to_next_grapheme(lines, pos);
    }
    if !pos.is_at_last_line(lines.len()) {
        return move_to_start_of_next_line(pos);
    }
    pos
}

pub fn move_up(view: &View<'_>, cursor: Cursor, tab_width: usize) -> Cursor {
    if !cursor.pos.is_at_first_row_of_line(view) {
        return move_to_prev_row_of_line(view, cursor);
    }
    if !cursor.pos.pos.is_at_first_line() {
        return move_to_last_row_of_prev_line(view, cursor, tab_width);
    }
    cursor
}

pub fn move_down(view: &View<'_>, cursor: Cursor, tab_width: usize) -> Cursor {
    if !cursor.pos.is_at_last_row_of_line(view) {
        return move_to_next_row_of_line(view, cursor, tab_width);
    }
    if !cursor.pos.pos.is_at_last_line(view.text().as_lines().len()) {
        return move_to_first_row_of_next_line(view, cursor, tab_width);
    }
    cursor
}

fn move_to_prev_grapheme(lines: &[String], pos: TextPos) -> TextPos {
    use crate::str::StrExt;

    TextPos {
        line: pos.line,
        byte: lines[pos.line][..pos.byte]
            .grapheme_indices()
            .next_back()
            .map(|(index, _)| index)
            .unwrap(),
    }
}

fn move_to_next_grapheme(lines: &[String], pos: TextPos) -> TextPos {
    use crate::str::StrExt;

    let line = &lines[pos.line];
    TextPos {
        line: pos.line,
        byte: line[pos.byte..]
            .grapheme_indices()
            .nth(1)
            .map(|(index, _)| pos.byte + index)
            .unwrap_or(line.len()),
    }
}

fn move_to_end_of_prev_line(lines: &[String], pos: TextPos) -> TextPos {
    let prev_line = pos.line - 1;
    TextPos {
        line: prev_line,
        byte: lines[prev_line].len(),
    }
}

fn move_to_start_of_next_line(pos: TextPos) -> TextPos {
    TextPos {
        line: pos.line + 1,
        byte: 0,
    }
}

fn move_to_prev_row_of_line(view: &View<'_>, cursor: Cursor) -> Cursor {
    use crate::GridPos;
    
    let line = view.line(cursor.pos.pos.line);
    let mut grid_pos = line.pos_to_grid_pos(
        cursor.pos.biased_line_pos(),
        view.settings().tab_column_count,
    );
    if let Some(col) = cursor.col {
        grid_pos.col = col;
    }
    let pos = line.grid_pos_to_pos(
        GridPos {
            row: grid_pos.row - 1,
            ..grid_pos
        },
        view.settings().tab_column_count,
    );
    Cursor {
        pos: BiasedTextPos::from_line_and_biased_line_pos(cursor.pos.pos.line, pos),
        col: Some(grid_pos.col),
    }
}

fn move_to_next_row_of_line(view: &View<'_>, cursor: Cursor, tab_width: usize) -> Cursor {
    use crate::GridPos;

    let line = view.line(cursor.pos.pos.line);
    let mut grid_pos = line.pos_to_grid_pos(
        cursor.pos.biased_line_pos(),
        tab_width
    );
    if let Some(col) = cursor.col {
        grid_pos.col = col;
    }
    let pos = line.grid_pos_to_pos(
        GridPos {
            row: grid_pos.row + 1,
            ..grid_pos
        },
        tab_width,
    );
    Cursor {
        pos: BiasedTextPos::from_line_and_biased_line_pos(cursor.pos.pos.line, pos),
        col: Some(grid_pos.col),
    }
}

fn move_to_last_row_of_prev_line(view: &View<'_>, cursor: Cursor, tab_width: usize) -> Cursor {
    use crate::GridPos;

    let mut grid_pos = view.line(cursor.pos.pos.line).pos_to_grid_pos(
        cursor.pos.biased_line_pos(),
        tab_width,
    );
    if let Some(col) = cursor.col {
        grid_pos.col = col;
    }
    let prev_line = cursor.pos.pos.line - 1;
    let prev_line_ref = view.line(prev_line);
    let pos = prev_line_ref.grid_pos_to_pos(
        GridPos {
            row: prev_line_ref.row_count() - 1,
            col: grid_pos.col,
        },
        tab_width,
    );
    Cursor {
        pos: BiasedTextPos::from_line_and_biased_line_pos(prev_line, pos),
        col: Some(grid_pos.col),
    }
}

fn move_to_first_row_of_next_line(view: &View<'_>, cursor: Cursor, tab_width: usize) -> Cursor {
    use crate::GridPos;

    let mut grid_pos = view.line(cursor.pos.pos.line).pos_to_grid_pos(
        cursor.pos.biased_line_pos(),
        tab_width,
    );
    if let Some(col) = cursor.col {
        grid_pos.col = col;
    }
    let next_line = cursor.pos.pos.line + 1;
    let pos = view.line(next_line).grid_pos_to_pos(
        GridPos { row: 0, col: grid_pos.col },
        tab_width,
    );
    Cursor {
        pos: BiasedTextPos::from_line_and_biased_line_pos(next_line, pos),
        col: Some(grid_pos.col),
    }
}
