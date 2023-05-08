use crate::{Pos, Text};

#[derive(Debug)]
pub struct Context<'a> {
    pub text: &'a Text,
}

impl<'a> Context<'a> {
    pub fn move_left(&self, pos: Pos) -> Pos {
        if !Self::is_at_start_of_line(pos) {
            self.move_to_prev_grapheme(pos)
        } else if !Self::is_at_first_line(pos) {
            self.move_to_end_of_prev_line(pos)
        } else {
            pos
        }
    }

    pub fn move_right(&self, pos: Pos) -> Pos {
        if !self.is_at_end_of_line(pos) {
            self.move_to_next_grapheme(pos)
        } else if !self.is_at_last_line(pos) {
            Self::move_to_start_of_next_line(pos)
        } else {
            pos
        }
    }

    pub fn move_up(&self, pos: Pos, column: Option<usize>) -> (Pos, Option<usize>) {
        if Self::is_at_first_line(pos) {
            let (pos, column) = self.move_to_prev_line(pos, column);
            (pos, Some(column))
        } else {
            (Self::move_to_start_of_line(pos), None)
        }
    }

    pub fn move_down(&self, pos: Pos, column: Option<usize>) -> (Pos, Option<usize>) {
        if !self.is_at_last_line(pos) {
            let (pos, column) = self.move_to_next_line(pos, column);
            (pos, Some(column))
        } else {
            (self.move_to_end_of_line(pos), None)
        }
    }

    fn is_at_first_line(pos: Pos) -> bool {
        pos.line == 0
    }

    fn is_at_last_line(&self, pos: Pos) -> bool {
        pos.line == self.text.as_lines().len()
    }

    fn is_at_start_of_line(pos: Pos) -> bool {
        pos.byte == 0
    }

    fn is_at_end_of_line(&self, pos: Pos) -> bool {
        pos.byte == self.text.as_lines()[pos.line].len()
    }

    fn move_to_next_grapheme(&self, pos: Pos) -> Pos {
        use crate::str::StrExt;

        Pos {
            line: pos.line,
            byte: self.text.as_lines()[pos.line]
                .next_grapheme_boundary(pos.byte)
                .unwrap(),
        }
    }

    fn move_to_prev_grapheme(&self, pos: Pos) -> Pos {
        use crate::str::StrExt;

        Pos {
            line: pos.line,
            byte: self.text.as_lines()[pos.line]
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

    fn move_to_end_of_prev_line(&self, pos: Pos) -> Pos {
        let prev_line = pos.line - 1;
        Pos {
            line: prev_line,
            byte: self.text.as_lines()[prev_line].len(),
        }
    }

    fn move_to_next_line(&self, pos: Pos, column: Option<usize>) -> (Pos, usize) {
        let column = column.unwrap_or_else(|| self.byte_to_column(pos.line, pos.byte));
        let next_line = pos.line + 1;
        (
            Pos {
                line: next_line,
                byte: self.column_to_byte(next_line, column),
            },
            column,
        )
    }

    fn move_to_prev_line(&self, pos: Pos, column: Option<usize>) -> (Pos, usize) {
        let column = column.unwrap_or_else(|| self.byte_to_column(pos.line, pos.byte));
        let prev_line = pos.line - 1;
        (
            Pos {
                line: prev_line,
                byte: self.column_to_byte(prev_line, column),
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

    fn move_to_end_of_line(&self, pos: Pos) -> Pos {
        Pos {
            line: pos.line,
            byte: self.text.as_lines()[pos.line].len(),
        }
    }

    fn byte_to_column(&self, _line: usize, byte: usize) -> usize {
        byte
    }

    fn column_to_byte(&self, line: usize, column: usize) -> usize {
        column.min(self.text.as_lines()[line].len())
    }
}
