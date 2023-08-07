use {
    super::Lines,
    crate::{inlays::BlockInlay, widgets::BlockWidget, Line},
    std::slice::Iter,
};

#[derive(Clone, Debug)]
pub struct Blocks<'a> {
    pub(super) lines: Lines<'a>,
    pub(super) block_inlays: Iter<'a, (usize, BlockInlay)>,
    pub(super) index: usize,
}

impl<'a> Iterator for Blocks<'a> {
    type Item = Block<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self
            .block_inlays
            .as_slice()
            .first()
            .map_or(false, |&(index, _)| index == self.index)
        {
            let (_, block_inlay) = self.block_inlays.next().unwrap();
            return Some(match *block_inlay {
                BlockInlay::Widget(widget) => Block::Widget(widget),
            });
        }
        let line = self.lines.next()?;
        self.index += 1;
        Some(Block::Line {
            is_inlay: false,
            line,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Block<'a> {
    Line { is_inlay: bool, line: Line<'a> },
    Widget(BlockWidget),
}
