use std::mem;
use std::ops::{Index, IndexMut};

#[derive(Clone, Debug)]
pub struct Arena<T> {
    slots: Vec<Slot<T>>,
    first_free_slot_index: Option<usize>,
}
impl<T> Arena<T> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, value: T) -> usize {
        if let Some(index) = self.first_free_slot_index {
            if let Slot::Free {
                next_free_slot_index,
            } = self.slots[index]
            {
                self.slots[index] = Slot::Used { value };
                self.first_free_slot_index = next_free_slot_index;
                return index;
            }
            panic!();
        }
        let index = self.slots.len();
        self.slots.push(Slot::Used { value });
        index
    }

    pub fn remove(&mut self, index: usize) -> T {
        if let Slot::Used { value } = mem::replace(
            &mut self.slots[index],
            Slot::Free {
                next_free_slot_index: self.first_free_slot_index,
            },
        ) {
            self.first_free_slot_index = Some(index);
            return value;
        }
        panic!();
    }
}

impl<T> Default for Arena<T> {
    fn default() -> Self {
        Self {
            slots: Vec::new(),
            first_free_slot_index: None,
        }
    }
}

impl<T> Index<usize> for Arena<T> {
    type Output = T;

    fn index(&self, index: usize) -> &Self::Output {
        if let Slot::Used { value } = &self.slots[index] {
            return value;
        }
        panic!();
    }
}

impl<T> IndexMut<usize> for Arena<T> {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        if let Slot::Used { value } = &mut self.slots[index] {
            return value;
        }
        panic!();
    }
}

#[derive(Clone, Debug)]
enum Slot<T> {
    Free { next_free_slot_index: Option<usize> },
    Used { value: T },
}
