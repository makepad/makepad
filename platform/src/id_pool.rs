use std::{
    cell::RefCell,
    rc::Rc,
    ops::Deref,
    ops::DerefMut,
};

#[derive(Clone, Default, Debug)]
pub struct IdPoolFree(Rc<RefCell<Vec<usize >> >);

#[derive(Default, Debug)]
pub struct IdPool<T> where T: Default {
    pub pool: Vec<IdPoolItem<T >>,
    pub free: IdPoolFree
}

#[derive(Debug)]
pub struct IdPoolItem<T> {
    pub item: T,
    pub generation: u64,
}

impl<T> Deref for IdPoolItem<T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {&self.item}
}

impl<T> DerefMut for IdPoolItem<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {&mut self.item}
}

#[derive(Debug)]
pub struct PoolId {
    pub id: usize,
    pub generation: u64,
    pub free: IdPoolFree
}

impl Drop for PoolId {
    fn drop(&mut self) {
        self.free.0.borrow_mut().push(self.id)
    }
}

impl<T> IdPool<T> where T: Default {
    pub fn alloc(&mut self) -> PoolId {
        if let Some(id) = self.free.0.borrow_mut().pop() {
            self.pool[id].generation += 1;
            PoolId {
                id,
                generation: self.pool[id].generation,
                free: self.free.clone()
            }
        }
        else {
            let id = self.pool.len();
            self.pool.push(IdPoolItem {
                generation: 0,
                item: T::default()
            });
            PoolId {
                id,
                generation: 0,
                free: self.free.clone()
            }
            
        }
    }
}
