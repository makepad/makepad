use alloc::vec::Vec;

pub trait Reserve {
    fn reserve_capacity(&mut self, new_capacity: usize);
}

impl<T> Reserve for Vec<T> {
    #[inline]
    fn reserve_capacity(&mut self, new_capacity: usize) {
        let old_capacity = self.capacity();
        if old_capacity < new_capacity {
            let additional = new_capacity - old_capacity;
            self.reserve(additional);
        }
    }
}
