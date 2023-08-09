use crate::{selection::Affinity, str::StrExt, Point, Session};

pub fn move_left(point: Point, lines: &[String]) -> Point {
    if !is_at_start_of_line(point) {
        return move_to_prev_grapheme(point, lines);
    }
    if !is_at_first_line(point) {
        return move_to_end_of_prev_line(point, lines);
    }
    point
}

pub fn move_right(point: Point, lines: &[String]) -> Point {
    if !is_at_end_of_line(point, lines) {
        return move_to_next_grapheme(point, lines);
    }
    if !is_at_last_line(point, lines) {
        return move_to_start_of_next_line(point);
    }
    point
}

pub fn move_up(
    point: Point,
    affinity: Affinity,
    preferred_column: Option<usize>,
    session: &Session,
) -> (Point, Affinity, Option<usize>) {
    if !is_at_first_row_of_line(point, affinity, session) {
        return move_to_prev_row_of_line(point, affinity, preferred_column, session);
    }
    if !is_at_first_line(point) {
        return move_to_last_row_of_prev_line(point, affinity, preferred_column, session);
    }
    (point, affinity, preferred_column)
}

fn is_at_first_line(point: Point) -> bool {
    point.line == 0
}

fn is_at_last_line(point: Point, lines: &[String]) -> bool {
    point.line == lines.len()
}

fn is_at_start_of_line(point: Point) -> bool {
    point.byte == 0
}

fn is_at_end_of_line(point: Point, lines: &[String]) -> bool {
    point.byte == lines[point.line].len()
}

fn is_at_first_row_of_line(point: Point, affinity: Affinity, session: &Session) -> bool {
    session.line(point.line, |line| {
        let (row, _) = line.byte_and_affinity_to_row_and_column(
            point.byte,
            affinity,
            session.settings().tab_column_count,
        );
        row == 0
    })
}

fn is_at_last_row_of_line(point: Point, affinity: Affinity, session: &Session) -> bool {
    session.line(point.line, |line| {
        let (row, _) = line.byte_and_affinity_to_row_and_column(
            point.byte,
            affinity,
            session.settings().tab_column_count,
        );
        row == line.row_count() - 1
    })
}

fn move_to_prev_grapheme(point: Point, lines: &[String]) -> Point {
    Point {
        line: point.line,
        byte: lines[point.line][..point.byte]
            .grapheme_indices()
            .next_back()
            .map(|(index, _)| index)
            .unwrap(),
    }
}

fn move_to_next_grapheme(point: Point, lines: &[String]) -> Point {
    let line = &lines[point.line];
    Point {
        line: point.line,
        byte: line[point.byte..]
            .grapheme_indices()
            .nth(1)
            .map(|(index, _)| point.byte + index)
            .unwrap_or(line.len()),
    }
}

fn move_to_end_of_prev_line(point: Point, lines: &[String]) -> Point {
    let prev_line = point.line - 1;
    Point {
        line: prev_line,
        byte: lines[prev_line].len(),
    }
}

fn move_to_start_of_next_line(point: Point) -> Point {
    Point {
        line: point.line + 1,
        byte: 0,
    }
}

fn move_to_prev_row_of_line(
    point: Point,
    affinity: Affinity,
    preferred_column: Option<usize>,
    session: &Session,
) -> (Point, Affinity, Option<usize>) {
    session.line(point.line, |line| {
        let (row, mut column) = line.byte_and_affinity_to_row_and_column(
            point.byte,
            affinity,
            session.settings().tab_column_count,
        );
        if let Some(preferred_column) = preferred_column {
            column = preferred_column;
        }
        let (byte, affinity) = line.row_and_column_to_byte_and_affinity(
            row - 1,
            column,
            session.settings().tab_column_count,
        );
        (
            Point {
                line: point.line,
                byte,
            },
            affinity,
            Some(column),
        )
    })
}

/*
fn move_to_next_row_of_line(view: &View<'_>, cursor: Cursor) -> Cursor {
    use crate::Point;

    let line = view.line(cursor.biased_pos.pos.line);
    let mut point = line.biased_byte_to_point(cursor.biased_pos.biased_byte());
    if let Some(column) = cursor.column {
        point.column = column;
    }
    let biased_byte = line.point_to_biased_byte(Point {
        row: point.row + 1,
        ..point
    });
    Cursor {
        biased_pos: BiasedPos::from_line_and_biased_byte(cursor.biased_pos.pos.line, biased_byte),
        column: Some(point.column),
    }
}
*/

fn move_to_last_row_of_prev_line(
    point: Point,
    affinity: Affinity,
    preferred_column: Option<usize>,
    session: &Session,
) -> (Point, Affinity, Option<usize>) {
    session.line(point.line, |line| {
        let (_, mut column) = line.byte_and_affinity_to_row_and_column(
            point.byte,
            affinity,
            session.settings().tab_column_count,
        );
        if let Some(preferred_column) = preferred_column {
            column = preferred_column;
        }
        session.line(point.line - 1, |prev_line| {
            let (byte, affinity) = prev_line.row_and_column_to_byte_and_affinity(
                prev_line.row_count() - 1,
                column,
                session.settings().tab_column_count,
            );
            (
                Point {
                    line: point.line - 1,
                    byte,
                },
                affinity,
                Some(column),
            )
        })
    })
}

/*
fn move_to_first_row_of_next_line(view: &View<'_>, cursor: Cursor) -> Cursor {
    use crate::Point;

    let mut point = view
        .line(cursor.biased_pos.pos.line)
        .biased_byte_to_point(cursor.biased_pos.biased_byte());
    if let Some(column) = cursor.column {
        point.column = column;
    }
    let next_line = cursor.biased_pos.pos.line + 1;
    let biased_byte = view.line(next_line).point_to_biased_byte(Point {
        row: 0,
        column: point.column,
    });
    Cursor {
        biased_pos: BiasedPos::from_line_and_biased_byte(next_line, biased_byte),
        column: Some(point.column),
    }
}
*/
