pub mod gen_id_map;

mod gen_id_allocator;

pub use self::{gen_id_allocator::GenIdAllocator, gen_id_map::GenIdMap};

use {
    crate::makepad_micro_serde::{SerBin, DeBin, DeBinErr},
    std::{
        fmt,
        hash::{Hash, Hasher},
        marker::PhantomData,
    }
};

pub struct GenId<Tag> {
    index: usize,
    generation: usize,
    tag: PhantomData<Tag>,
}

impl<Tag> Clone for GenId<Tag> {
    fn clone(&self) -> Self {
        Self {
            index: self.index,
            generation: self.generation,
            tag: PhantomData,
        }
    }
}

impl<Tag> Copy for GenId<Tag> {}

impl<Tag> fmt::Debug for GenId<Tag> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GenId")
            .field("index", &self.index)
            .field("generation", &self.generation)
            .finish()
    }
}

impl<Tag> Eq for GenId<Tag> {}

impl<Tag> Hash for GenId<Tag> {
    fn hash<H>(&self, state: &mut H)
    where
        H: Hasher,
    {
        self.index.hash(state);
        self.generation.hash(state);
    }
}

impl<Tag> PartialEq for GenId<Tag> {
    fn eq(&self, other: &Self) -> bool {
        self.index == other.index && self.generation == other.generation
    }
}

impl<Tag> SerBin for GenId<Tag> {
    fn ser_bin(&self, s: &mut Vec<u8>) {
        self.index.ser_bin(s);
        self.generation.ser_bin(s);
    }
}

impl<Tag> DeBin for GenId<Tag> {
    fn de_bin(o: &mut usize, d: &[u8]) -> Result<Self, DeBinErr> {
        Ok(GenId{
            index: DeBin::de_bin(o, d)?,
            generation: DeBin::de_bin(o, d)?,
            tag: PhantomData,
        })
    }
}