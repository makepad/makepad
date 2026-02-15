use crate::Point;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Command {
    MoveTo(Point),
    Close,
    LineTo(Point),
}
