use {
    crate::{
        live_ptr::LiveModuleId, makepad_derive_live::*, makepad_live_tokenizer::LiveId, LiveType,
    },
    std::{
        any::TypeId,
        cell::RefCell,
        collections::{hash_map::Entry, BTreeSet, HashMap},
        rc::Rc,
    },
};

#[derive(Clone)]
pub struct LiveComponentInfo {
    pub name: LiveId,
    pub module_id: LiveModuleId,
}

pub trait LiveComponentRegistry {
    fn ref_cast_type_id(&self) -> LiveType;
    fn get_component_info(&self, name: LiveId) -> Option<LiveComponentInfo>;
    fn component_type(&self) -> LiveId;
    fn get_module_set(&self, set: &mut BTreeSet<LiveModuleId>);
}

#[derive(Default, Clone)]
pub struct LiveComponentRegistries(
    pub Rc<RefCell<HashMap<LiveType, Box<dyn LiveComponentRegistry>>>>,
);

generate_any_trait_api!(LiveComponentRegistry);

impl LiveComponentRegistries {
    pub fn find_component(&self, ty: LiveId, name: LiveId) -> Option<LiveComponentInfo> {
        let reg = self.0.borrow();
        for entry in reg.values() {
            if entry.component_type() == ty {
                return entry.get_component_info(name);
            }
        }
        None
    }

    pub fn new() -> Self {
        Self(Rc::new(RefCell::new(HashMap::new())))
    }

    pub fn get<T: 'static + LiveComponentRegistry>(&self) -> std::cell::Ref<'_, T> {
        std::cell::Ref::map(self.0.borrow(), |v| {
            v.get(&TypeId::of::<T>())
                .unwrap()
                .downcast_ref::<T>()
                .unwrap()
        })
    }

    pub fn get_or_create<T: 'static + Default + LiveComponentRegistry>(
        &self,
    ) -> std::cell::RefMut<'_, T> {
        let reg = self.0.borrow_mut();
        std::cell::RefMut::map(reg, |v| {
            match v.entry(TypeId::of::<T>()) {
                Entry::Occupied(o) => o.into_mut(),
                Entry::Vacant(v) => v.insert(Box::<T>::default()),
            }
            .downcast_mut::<T>()
            .unwrap()
        })
    }
}
