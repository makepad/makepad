use crate::{str::StrExt, Point};

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

fn move_to_prev_grapheme(point: Point, lines: &[String], ) -> Point {
    Point {
        line: point.line,
        byte: lines[point.line][..point.byte]
            .grapheme_indices()
            .next_back()
            .map(|(index, _)| index)
            .unwrap(),
    }
}

fn move_to_next_grapheme(point: Point, lines: &[String], ) -> Point {
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