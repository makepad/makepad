use crate::{Bias, BiasedLinePos, TextPos, View};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct BiasedTextPos {
    pub pos: TextPos,
    pub bias: Bias,
}

impl BiasedTextPos {
    pub fn from_line_and_biased_line_pos(line: usize, pos: BiasedLinePos) -> Self {
        Self {
            pos: TextPos {
                line,
                byte: pos.pos
            },
            bias: pos.bias
        }
    }

    pub fn is_at_first_row_of_line(self, view: &View<'_>) -> bool {
        view.line(self.pos.line)
            .pos_to_grid_pos(self.biased_line_pos(), view.settings().tab_width)
            .row
            == 0
    }

    pub fn is_at_last_row_of_line(self, view: &View<'_>) -> bool {
        let line = view.line(self.pos.line);
        line.pos_to_grid_pos(self.biased_line_pos(), view.settings().tab_width)
            .row
            == line.row_count() - 1
    }

    pub fn biased_line_pos(self) -> BiasedLinePos {
        BiasedLinePos {
            pos: self.pos.byte,
            bias: self.bias,
        }
    }
}

impl From<TextPos> for BiasedTextPos {
    fn from(pos: TextPos) -> Self {
        Self {
            pos,
            ..Self::default()
        }
    }
}
