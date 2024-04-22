use {
    crate::{
        extern_::{Extern, UnguardedExtern},
        store::{Store, StoreId},
    },
    std::any::Any,
};

/// A nullable reference to an external object.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ExternRef(Option<Extern>);

impl ExternRef {
    /// Creates a new [`ExternRef`] wrapping the given underlying object.
    pub fn new<T>(store: &mut Store, object: impl Into<Option<T>>) -> Self
    where
        T: Any + Send + Sync + 'static,
    {
        Self(object.into().map(|object| Extern::new(store, object)))
    }

    /// Creates a null [`ExternRef`].
    pub fn null() -> Self {
        Self(None)
    }

    /// Returns `true` if this [`ExternRef`] is null.
    pub fn is_null(self) -> bool {
        self.0.is_none()
    }

    /// Returns a reference to the underlying object if this `ExternRef` is not null.
    pub fn get(self, store: &Store) -> Option<&dyn Any> {
        self.0.as_ref().map(|extern_| extern_.get(store))
    }

    /// Converts the given [`UnguardedExternRef`] to a [`ExternRef`].
    ///
    /// # Safety
    ///
    /// The given [`UnguardedExternRef`] must be owned by the [`Store`] with the given [`StoreId`].
    pub(crate) unsafe fn from_unguarded(extern_: UnguardedExternRef, store_id: StoreId) -> Self {
        Self(extern_.map(|extern_| unsafe { Extern::from_unguarded(extern_, store_id) }))
    }

    /// Converts this [`ExternRef`] to an [`UnguardedExternRef`].
    ///
    /// # Panics
    ///
    /// This [`FuncRef`] is not owned by the [`Store`] with the given [`StoreId`].
    pub(crate) fn to_unguarded(self, store_id: StoreId) -> UnguardedExternRef {
        self.0.map(|extern_| extern_.to_unguarded(store_id))
    }
}

/// An unguarded [`ExternRef`].
pub(crate) type UnguardedExternRef = Option<UnguardedExtern>;
