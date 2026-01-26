use {
    std::{
        any::TypeId,
        cell::RefCell,
        rc::Rc,
        collections::{
            HashMap,
            hash_map::Entry
        }
    },
    makepad_script::makepad_live_id::LiveId,
};

#[derive(Clone)]
pub struct ComponentInfo {
    pub name: LiveId,
}

pub trait ComponentRegistry {
    fn ref_cast_type_id(&self) -> TypeId;
    fn get_component_info(&self, name: LiveId) -> Option<ComponentInfo>;
    fn component_type(&self) -> LiveId;
}

impl dyn ComponentRegistry {
    pub fn is<T: ComponentRegistry + 'static>(&self) -> bool {
        let t = TypeId::of::<T>();
        let concrete = self.ref_cast_type_id();
        t == concrete
    }
    pub fn downcast_ref<T: ComponentRegistry + 'static>(&self) -> Option<&T> {
        if self.is::<T>() {
            Some(unsafe { &*(self as *const dyn ComponentRegistry as *const T) })
        } else {
            None
        }
    }
    pub fn downcast_mut<T: ComponentRegistry + 'static>(&mut self) -> Option<&mut T> {
        if self.is::<T>() {
            Some(unsafe { &mut *(self as *const dyn ComponentRegistry as *mut T) })
        } else {
            None
        }
    }
}

#[derive(Default, Clone)]
pub struct ComponentRegistries(pub Rc<RefCell<HashMap<TypeId, Box<dyn ComponentRegistry>>>>);

impl ComponentRegistries {
    pub fn find_component(&self, ty: LiveId, name: LiveId) -> Option<ComponentInfo> {
        let reg = self.0.borrow();
        for entry in reg.values() {
            if entry.component_type() == ty {
                return entry.get_component_info(name)
            }
        }
        None
    }
    
    pub fn new() -> Self {
        Self(Rc::new(RefCell::new(HashMap::new())))
    }
    
    pub fn get<T: 'static + ComponentRegistry>(&self) -> std::cell::Ref<'_, T> {
        std::cell::Ref::map(
            self.0.borrow(),
            |v| v
                .get(&TypeId::of::<T>()).unwrap()
                .downcast_ref::<T>().unwrap()
        )
    }
    
    pub fn get_or_create<T: 'static + Default + ComponentRegistry>(&self) -> std::cell::RefMut<'_, T> {
        let reg = self.0.borrow_mut();
        std::cell::RefMut::map(
            reg,
            |v|
            match v.entry(TypeId::of::<T>()) {
                Entry::Occupied(o) => o.into_mut(),
                Entry::Vacant(v) => v.insert(Box::<T>::default())
            }
            .downcast_mut::<T>().unwrap()
        )
    }
}
