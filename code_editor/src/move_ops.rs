use crate::{
    selection::{Affinity, Cursor},
    str::StrExt,
    text::Position,
    Session,
};

pub fn move_left(lines: &[String], cursor: Cursor) -> Cursor {
    if !is_at_start_of_line(cursor) {
        return move_to_prev_grapheme(lines, cursor);
    }
    if !is_at_first_line(cursor) {
        return move_to_end_of_prev_line(lines, cursor);
    }
    cursor
}

pub fn move_right(lines: &[String], cursor: Cursor) -> Cursor {
    if !is_at_end_of_line(lines, cursor) {
        return move_to_next_grapheme(lines, cursor);
    }
    if !is_at_last_line(lines, cursor) {
        return move_to_start_of_next_line(cursor);
    }
    cursor
}

pub fn move_up(session: &Session, cursor: Cursor) -> Cursor {
    if !is_at_first_row_of_line(session, cursor) {
        return move_to_prev_row_of_line(session, cursor);
    }
    if !is_at_first_line(cursor) {
        return move_to_last_row_of_prev_line(session, cursor);
    }
    cursor
}

pub fn move_down(session: &Session, cursor: Cursor) -> Cursor {
    if !is_at_last_row_of_line(session, cursor) {
        return move_to_next_row_of_line(session, cursor);
    }
    if !is_at_last_line(session.document().borrow().text().as_lines(), cursor) {
        return move_to_first_row_of_next_line(session, cursor);
    }
    cursor
}

fn is_at_first_line(cursor: Cursor) -> bool {
    cursor.position.line_index == 0
}

fn is_at_last_line(lines: &[String], cursor: Cursor) -> bool {
    cursor.position.line_index == lines.len()
}

fn is_at_start_of_line(cursor: Cursor) -> bool {
    cursor.position.byte_index == 0
}

fn is_at_end_of_line(lines: &[String], cursor: Cursor) -> bool {
    cursor.position.byte_index == lines[cursor.position.line_index].len()
}

fn is_at_first_row_of_line(session: &Session, cursor: Cursor) -> bool {
    session.line(cursor.position.line_index, |line| {
        let (row, _) = line.byte_and_affinity_to_row_and_column(
            cursor.position.byte_index,
            cursor.affinity,
            session.settings().tab_column_count,
        );
        row == 0
    })
}

fn is_at_last_row_of_line(session: &Session, cursor: Cursor) -> bool {
    session.line(cursor.position.line_index, |line| {
        let (row, _) = line.byte_and_affinity_to_row_and_column(
            cursor.position.byte_index,
            cursor.affinity,
            session.settings().tab_column_count,
        );
        row == line.row_count() - 1
    })
}

fn move_to_prev_grapheme(lines: &[String], cursor: Cursor) -> Cursor {
    Cursor {
        position: Position {
            line_index: cursor.position.line_index,
            byte_index: lines[cursor.position.line_index][..cursor.position.byte_index]
                .grapheme_indices()
                .next_back()
                .map(|(index, _)| index)
                .unwrap(),
        },
        affinity: Affinity::Before,
        preferred_column_index: None,
    }
}

fn move_to_next_grapheme(lines: &[String], cursor: Cursor) -> Cursor {
    let line = &lines[cursor.position.line_index];
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

fn move_to_end_of_prev_line(lines: &[String], cursor: Cursor) -> Cursor {
    let prev_line = cursor.position.line_index - 1;
    Cursor {
        position: Position {
            line_index: prev_line,
            byte_index: lines[prev_line].len(),
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

fn move_to_prev_row_of_line(session: &Session, cursor: Cursor) -> Cursor {
    session.line(cursor.position.line_index, |line| {
        let (row, mut column) = line.byte_and_affinity_to_row_and_column(
            cursor.position.byte_index,
            cursor.affinity,
            session.settings().tab_column_count,
        );
        if let Some(preferred_column) = cursor.preferred_column_index {
            column = preferred_column;
        }
        let (byte, affinity) = line.row_and_column_to_byte_and_affinity(
            row - 1,
            column,
            session.settings().tab_column_count,
        );
        Cursor {
            position: Position {
                line_index: cursor.position.line_index,
                byte_index: byte,
            },
            affinity,
            preferred_column_index: Some(column),
        }
    })
}

fn move_to_next_row_of_line(session: &Session, cursor: Cursor) -> Cursor {
    session.line(cursor.position.line_index, |line| {
        let (row, mut column) = line.byte_and_affinity_to_row_and_column(
            cursor.position.byte_index,
            cursor.affinity,
            session.settings().tab_column_count,
        );
        if let Some(preferred_column) = cursor.preferred_column_index {
            column = preferred_column;
        }
        let (byte, affinity) = line.row_and_column_to_byte_and_affinity(
            row + 1,
            column,
            session.settings().tab_column_count,
        );
        Cursor {
            position: Position {
                line_index: cursor.position.line_index,
                byte_index: byte,
            },
            affinity,
            preferred_column_index: Some(column),
        }
    })
}

fn move_to_last_row_of_prev_line(session: &Session, cursor: Cursor) -> Cursor {
    session.line(cursor.position.line_index, |line| {
        let (_, mut column) = line.byte_and_affinity_to_row_and_column(
            cursor.position.byte_index,
            cursor.affinity,
            session.settings().tab_column_count,
        );
        if let Some(preferred_column) = cursor.preferred_column_index {
            column = preferred_column;
        }
        session.line(cursor.position.line_index - 1, |prev_line| {
            let (byte, affinity) = prev_line.row_and_column_to_byte_and_affinity(
                prev_line.row_count() - 1,
                column,
                session.settings().tab_column_count,
            );
            Cursor {
                position: Position {
                    line_index: cursor.position.line_index - 1,
                    byte_index: byte,
                },
                affinity,
                preferred_column_index: Some(column),
            }
        })
    })
}

fn move_to_first_row_of_next_line(session: &Session, cursor: Cursor) -> Cursor {
    session.line(cursor.position.line_index, |line| {
        let (_, mut column) = line.byte_and_affinity_to_row_and_column(
            cursor.position.byte_index,
            cursor.affinity,
            session.settings().tab_column_count,
        );
        if let Some(preferred_column) = cursor.preferred_column_index {
            column = preferred_column;
        }
        session.line(cursor.position.line_index + 1, |next_line| {
            let (byte, affinity) = next_line.row_and_column_to_byte_and_affinity(
                0,
                column,
                session.settings().tab_column_count,
            );
            Cursor {
                position: Position {
                    line_index: cursor.position.line_index + 1,
                    byte_index: byte,
                },
                affinity,
                preferred_column_index: Some(column),
            }
        })
    })
}
