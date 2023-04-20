use crate::{Range, Size};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct DeltaLen {
    pub range: Range,
    pub replace_with_len: Size,
}
