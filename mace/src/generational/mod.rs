pub mod arena;

mod id_allocator;

pub use self::{arena::Arena, id_allocator::IdAllocator};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Id {
    index: usize,
    generation: usize,
}
