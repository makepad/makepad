pub mod id_allocator;
pub mod id_map;

pub use self::{id_allocator::IdAllocator, id_map::IdMap};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Id {
    index: usize,
    generation: usize,
}