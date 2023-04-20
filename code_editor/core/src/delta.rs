use super::{DeltaLen, Range, Text};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Delta {
    pub range: Range,
    pub replace_with: Text,
}

impl Delta {
    pub fn to_delta_len(self) -> DeltaLen {
        DeltaLen {
            range: self.range,
            replace_with_len: self.replace_with.len(),
        }
    }
}
