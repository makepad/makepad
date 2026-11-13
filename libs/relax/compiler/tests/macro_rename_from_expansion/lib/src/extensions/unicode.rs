use crate::subtags::{subtag, Subtag};

pub const TRUE_VALUE: Subtag = subtag!("true");
pub static FALSE_VALUE: Subtag = subtag!("false");

pub fn is_true(s: Subtag) -> bool {
    s == TRUE_VALUE || s == subtag!("yes")
}
