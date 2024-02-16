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
        let last_from_free_pool = self.free.0.borrow_mut().pop();
        if let Some(id) = last_from_free_pool {
            self.pool[id].generation += 1;
            PoolId {
                id,
                generation: self.pool[id].generation,
                free: self.free.clone()
            }
        }
        else {
            self.alloc_new(None)
        }
    }

    pub fn alloc_new(&mut self, item: Option<T>) -> PoolId {
        let id = self.pool.len();
        self.pool.push(IdPoolItem {
            generation: 0,
            item: item.unwrap_or_else(|| T::default())
        });
        PoolId {
            id,
            generation: 0,
            free: self.free.clone()
        }
    }

    pub fn alloc_with_reuse_filter<F>(&mut self, mut filter: F, item: T) -> PoolId 
    where F: FnMut(&IdPoolItem<T>) -> bool {
        let maybe_free_id = self.free.0.borrow_mut()
            .iter()
            .enumerate()
            .find_map(|(index, &id)| {
                if filter(&self.pool[id]) {
                    Some((index, id))
                } else {
                    None
                }
            });
    
        if let Some((index, id)) = maybe_free_id {
            self.free.0.borrow_mut().remove(index);
            self.pool[id].generation += 1;
            self.pool[id].item = item;
    
            let pool_id = PoolId {
                id,
                generation: self.pool[id].generation,
                free: self.free.clone()
            };
            pool_id
        } else {
            self.alloc_new(Some(item))
        }
    }
}
