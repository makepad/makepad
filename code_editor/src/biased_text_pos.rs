use crate::{Bias, TextPos, View};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct BiasedTextPos {
    pub pos: TextPos,
    pub bias: Bias,
}

impl BiasedTextPos {
    pub fn is_at_first_row_of_line(self, view: &View<'_>) -> bool {
        view.line(self.pos.line)
            .byte_bias_to_row_column((self.pos.byte, self.bias), view.settings().tab_column_count)
            .0
            == 0
    }

    pub fn is_at_last_row_of_line(self, view: &View<'_>) -> bool {
        let line = view.line(self.pos.line);
        line.byte_bias_to_row_column((self.pos.byte, self.bias), view.settings().tab_column_count)
            .0
            == line.row_count() - 1
    }
}

impl From<TextPos> for BiasedTextPos {
    fn from(pos: TextPos) -> Self {
        Self {
            pos,
            bias: Bias::default(),
        }
    }
}
