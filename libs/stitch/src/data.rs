use {
    crate::store::{Handle, Store, StoreId, UnguardedHandle},
    std::sync::Arc,
};

/// A Wasm data segment.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[repr(transparent)]
pub(crate) struct Data(pub(crate) Handle<DataEntity>);

impl Data {
    pub(crate) fn new(store: &mut Store, bytes: Arc<[u8]>) -> Self {
        Self(store.insert_data(DataEntity::new(bytes)))
    }

    pub(crate) fn drop_bytes(self, store: &mut Store) {
        self.0.as_mut(store).drop_bytes();
    }

    pub(crate) unsafe fn from_unguarded(data: UnguardedData, store_id: StoreId) -> Self {
        Self(Handle::from_unguarded(data, store_id))
    }

    pub(crate) fn to_unguarded(self, store_id: StoreId) -> UnguardedData {
        self.0.to_unguarded(store_id)
    }
}

/// An unguarded [`Data`].
pub(crate) type UnguardedData = UnguardedHandle<DataEntity>;

/// The representation of a [`Data`] in the store.
#[derive(Debug)]
pub(crate) struct DataEntity {
    bytes: Option<Arc<[u8]>>,
}

impl DataEntity {
    fn new(bytes: Arc<[u8]>) -> Self {
        Self { bytes: Some(bytes) }
    }

    pub(crate) fn bytes(&self) -> &[u8] {
        self.bytes.as_ref().map_or(&[], |bytes| &bytes)
    }

    pub(crate) fn drop_bytes(&mut self) {
        self.bytes = None;
    }
}
