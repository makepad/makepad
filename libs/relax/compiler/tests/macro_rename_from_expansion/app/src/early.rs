use crate::names::{double, triple};

pub const SIX: u32 = triple!(2);

pub fn twice(x: u32) -> u32 {
    double!(x)
}
