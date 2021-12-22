pub mod gen_id_allocator;
pub mod gen_id_map;

pub use self::{
    gen_id_allocator::GenIdAllocator,
    gen_id_map::GenIdMap,
};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct GenId {
    pub index: usize,
    pub generation: usize,
}
