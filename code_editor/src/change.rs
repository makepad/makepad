use crate::{Point, Range, Text};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Change {
    pub drift: Drift,
    pub kind: ChangeKind,
}

impl Change {
    pub fn invert(self, text: &Text) -> Self {
        Self {
            drift: self.drift,
            kind: match self.kind {
                ChangeKind::Insert(point, text) => {
                    ChangeKind::Delete(Range::from_start_and_extent(point, text.extent()))
                }
                ChangeKind::Delete(range) => {
                    ChangeKind::Insert(range.start(), text.slice(range))
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Drift {
    Before,
    After,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum ChangeKind {
    Insert(Point, Text),
    Delete(Range),
}
