use std::ops::{AddAssign, SubAssign};

pub trait Info: Copy + AddAssign + SubAssign {
    fn new() -> Self;
}