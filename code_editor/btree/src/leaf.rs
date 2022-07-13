use crate::Info;

pub trait Leaf: Clone {
    type Info: Info<Self>;

    fn move_left(&mut self, other: &mut Self, end: usize);
    fn move_right(&mut self, other: &mut Self, end: usize);
}
