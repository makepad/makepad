pub mod arena;

mod index_allocator;

pub use self::{arena::Arena, index_allocator::IndexAllocator};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Index {
    index: usize,
    generation: usize,
}
