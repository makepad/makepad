use {
    crate::store::{Handle, Store, StoreId, UnguardedHandle},
    std::any::Any,
};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct Extern(Handle<ExternEntity>);

impl Extern {
    pub(crate) fn new(store: &mut Store, object: impl Any + Send + Sync + 'static) -> Self {
        Self(store.insert_extern(ExternEntity::new(object)))
    }

    pub(crate) fn get(self, store: &Store) -> &dyn Any {
        self.0.as_ref(store).get()
    }

    pub(crate) unsafe fn from_unguarded(extern_: UnguardedExtern, store_id: StoreId) -> Self {
        Self(Handle::from_unguarded(extern_, store_id))
    }

    pub(crate) fn to_unguarded(self, store_id: StoreId) -> UnguardedExtern {
        self.0.to_unguarded(store_id)
    }
}

pub(crate) type UnguardedExtern = UnguardedHandle<ExternEntity>;

#[derive(Debug)]
pub(crate) struct ExternEntity {
    object: Box<dyn Any + Send + Sync + 'static>,
}

impl ExternEntity {
    fn new(object: impl Any + Send + Sync + 'static) -> Self {
        Self {
            object: Box::new(object),
        }
    }

    fn get(&self) -> &dyn Any {
        &*self.object
    }
}
