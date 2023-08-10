use crate::widgets::{BlockWidget, InlineWidget};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum InlineInlay {
    Text(String),
    Widget(InlineWidget),
}

#[derive(Clone, Debug, PartialEq)]
pub enum BlockInlay {
    Widget(BlockWidget),
}
