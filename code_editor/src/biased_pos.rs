use crate::{Bias, BiasedUsize, Pos, View};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct BiasedPos {
    pub pos: Pos,
    pub bias: Bias,
}

impl BiasedPos {
    pub fn from_line_and_biased_byte(line: usize, biased_byte: BiasedUsize) -> Self {
        Self {
            pos: Pos {
                line,
                byte: biased_byte.value
            },
            bias: biased_byte.bias
        }
    }

    pub fn is_at_first_row_of_line(self, view: &View<'_>) -> bool {
        view.line(self.pos.line)
            .biased_byte_to_point(self.biased_byte(), view.settings().tab_width)
            .row
            == 0
    }

    pub fn is_at_last_row_of_line(self, view: &View<'_>) -> bool {
        let line = view.line(self.pos.line);
        line.biased_byte_to_point(self.biased_byte(), view.settings().tab_width)
            .row
            == line.height() - 1
    }

    pub fn biased_byte(self) -> BiasedUsize {
        BiasedUsize {
            value: self.pos.byte,
            bias: self.bias,
        }
    }
}

impl From<Pos> for BiasedPos {
    fn from(pos: Pos) -> Self {
        Self {
            pos,
            ..Self::default()
        }
    }
}
