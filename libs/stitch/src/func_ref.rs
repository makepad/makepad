use crate::{
    func::{Func, UnguardedFunc},
    store::{Handle, StoreId},
};

/// A nullable reference to a [`Func`].
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct FuncRef(pub(crate) Option<Func>);

impl FuncRef {
    /// Creates a new [`FuncRef`].
    pub fn new(func: impl Into<Option<Func>>) -> Self {
        Self(func.into())
    }

    /// Creates a null [`FuncRef`].
    pub fn null() -> Self {
        Self(None)
    }

    /// Returns `true` if this [`FuncRef`] is null.
    pub fn is_null(self) -> bool {
        self.0.is_none()
    }

    /// Returns the underlying [`Func`] if this [`FuncRef`] is not null.
    pub fn get(self) -> Option<Func> {
        self.0
    }

    /// Converts the given [`UnguardedFuncRef`] to a [`FuncRef`].
    ///
    /// # Safety
    ///
    /// The [[`UnguardedFuncRef`] must be owned by the [`Store`] with the given [`StoreId`].
    pub(crate) unsafe fn from_unguarded(func: UnguardedFuncRef, store_id: StoreId) -> Self {
        Self(func.map(|func| unsafe { Func(Handle::from_unguarded(func, store_id)) }))
    }

    /// Converts this [`FuncRef`] to an [`UnguardedFuncRef`].
    ///
    /// # Panics
    ///
    /// This [`FuncRef`] is not owned by the [`Store`] with the given [`StoreId`].
    pub(crate) fn to_unguarded(self, store_id: StoreId) -> UnguardedFuncRef {
        self.0.map(|func| func.0.to_unguarded(store_id))
    }
}

impl From<Func> for FuncRef {
    fn from(func: Func) -> Self {
        Self::new(func)
    }
}

/// An unguarded [`FuncRef`].
pub(crate) type UnguardedFuncRef = Option<UnguardedFunc>;
