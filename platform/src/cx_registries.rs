use {
    std::{
        collections::{
            HashMap,
            hash_map::Entry
        },
        any::{Any, TypeId},
        rc::Rc,
        cell::RefCell,
    },
};

#[derive(Clone)]
pub struct CxRegistries(pub Rc<RefCell<HashMap<TypeId, Box<dyn Any >> >>);

pub trait CxRegistryNew{
    fn new()->Self;
}

impl CxRegistries {
    pub fn new() -> Self {
        Self (Rc::new(RefCell::new(HashMap::new())))
    }
    
    pub fn get<T: 'static>(&self) -> std::cell::Ref<'_, T> {
        std::cell::Ref::map(
            self.0.borrow(),
            | v | v
                .get(&TypeId::of::<T>()).unwrap()
                .downcast_ref::<T>().unwrap()
        )
    }

    pub fn get_or_create<T: 'static + CxRegistryNew>(&self) -> std::cell::RefMut<'_, T> 
    {
        let reg = self.0.borrow_mut();
        std::cell::RefMut::map(
            reg,
            | v |
            match v.entry(TypeId::of::<T>()) {
                Entry::Occupied(o) => o.into_mut(),
                Entry::Vacant(v) => v.insert(Box::new(T::new()))
            }
            .downcast_mut::<T>().unwrap()
        )
    }
}
