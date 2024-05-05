use {
    crate::{
        decode::{Decode, DecodeError, Decoder},
        downcast::{DowncastMut, DowncastRef},
        extern_ref::UnguardedExternRef,
        func_ref::UnguardedFuncRef,
        store::{Handle, Store, StoreId, UnguardedHandle},
        val::{UnguardedVal, Val, ValType},
    },
    std::{error::Error, fmt},
};

/// A Wasm global.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[repr(transparent)]
pub struct Global(pub(crate) Handle<GlobalEntity>);

impl Global {
    /// Creates a new [`Global`] with the given [`GlobalType`] and initialization [`Val`] in the
    /// given [`Store`].
    ///
    /// # Errors
    ///
    /// If the [`ValType`] of the initialiation [`Val`] does not match the [`ValType`] of the
    /// [`Global`] to be created.
    ///
    /// # Panics
    ///
    /// If the initialization [`Val`] is not owned by the given [`Store`].
    pub fn new(store: &mut Store, type_: GlobalType, val: Val) -> Result<Self, GlobalError> {
        unsafe { Global::new_unguarded(store, type_, val.to_unguarded(store.id())) }
    }

    /// An unguarded version of [`Global::new`].
    unsafe fn new_unguarded(
        store: &mut Store,
        type_: GlobalType,
        val: UnguardedVal,
    ) -> Result<Self, GlobalError> {
        match (type_.val, val) {
            (ValType::I32, UnguardedVal::I32(val)) => Ok(Self(
                store.insert_global(GlobalEntity::I32(GlobalEntityT::new(type_.mut_, val))),
            )),
            (ValType::I64, UnguardedVal::I64(val)) => Ok(Self(
                store.insert_global(GlobalEntity::I64(GlobalEntityT::new(type_.mut_, val))),
            )),
            (ValType::F32, UnguardedVal::F32(val)) => Ok(Self(
                store.insert_global(GlobalEntity::F32(GlobalEntityT::new(type_.mut_, val))),
            )),
            (ValType::F64, UnguardedVal::F64(val)) => Ok(Self(
                store.insert_global(GlobalEntity::F64(GlobalEntityT::new(type_.mut_, val))),
            )),
            (ValType::FuncRef, UnguardedVal::FuncRef(val)) => Ok(Self(
                store.insert_global(GlobalEntity::FuncRef(GlobalEntityT::new(type_.mut_, val))),
            )),
            (ValType::ExternRef, UnguardedVal::ExternRef(val)) => Ok(Self(
                store.insert_global(GlobalEntity::ExternRef(GlobalEntityT::new(type_.mut_, val))),
            )),
            _ => Err(GlobalError::ValTypeMismatch),
        }
    }

    /// Returns the [`GlobalType`] of this [`Global`].
    pub fn type_(self, store: &Store) -> GlobalType {
        match self.0.as_ref(store) {
            GlobalEntity::I32(global) => GlobalType {
                mut_: global.mut_(),
                val: ValType::I32,
            },
            GlobalEntity::I64(global) => GlobalType {
                mut_: global.mut_(),
                val: ValType::I64,
            },
            GlobalEntity::F32(global) => GlobalType {
                mut_: global.mut_(),
                val: ValType::F32,
            },
            GlobalEntity::F64(global) => GlobalType {
                mut_: global.mut_(),
                val: ValType::F64,
            },
            GlobalEntity::FuncRef(global) => GlobalType {
                mut_: global.mut_(),
                val: ValType::FuncRef,
            },
            GlobalEntity::ExternRef(global) => GlobalType {
                mut_: global.mut_(),
                val: ValType::ExternRef,
            },
        }
    }

    /// Returns the value of this [`Global`].
    pub fn get(self, store: &Store) -> Val {
        unsafe { Val::from_unguarded(self.get_unguarded(store), store.id()) }
    }

    /// An unguarded version of [`Global::get`].
    fn get_unguarded(self, store: &Store) -> UnguardedVal {
        match self.0.as_ref(store) {
            GlobalEntity::I32(global) => UnguardedVal::I32(global.get()),
            GlobalEntity::I64(global) => UnguardedVal::I64(global.get()),
            GlobalEntity::F32(global) => UnguardedVal::F32(global.get()),
            GlobalEntity::F64(global) => UnguardedVal::F64(global.get()),
            GlobalEntity::FuncRef(global) => UnguardedVal::FuncRef(global.get()),
            GlobalEntity::ExternRef(global) => UnguardedVal::ExternRef(global.get()),
        }
    }

    /// Sets the value of this [`Global`] to the given [`Val`].
    ///
    /// # Errors
    ///
    /// - If the global is immutable.
    /// - If the [`ValType`] of the given [`Val`] does not match the [`ValType`] of this [`Global`].
    ///
    /// # Panics
    ///
    /// If the given [`Val`] is not owned by the given [`Store`].
    pub fn set(self, store: &mut Store, val: Val) -> Result<(), GlobalError> {
        unsafe { self.set_unguarded(store, val.to_unguarded(store.id())) }
    }

    /// An unguarded version of [`Global::set`].
    unsafe fn set_unguarded(self, store: &mut Store, val: UnguardedVal) -> Result<(), GlobalError> {
        if self.type_(store).mut_ != Mut::Var {
            return Err(GlobalError::Immutable);
        }
        match (self.0.as_mut(store), val) {
            (GlobalEntity::I32(global), UnguardedVal::I32(val)) => Ok(global.set(val)),
            (GlobalEntity::I64(global), UnguardedVal::I64(val)) => Ok(global.set(val)),
            (GlobalEntity::F32(global), UnguardedVal::F32(val)) => Ok(global.set(val)),
            (GlobalEntity::F64(global), UnguardedVal::F64(val)) => Ok(global.set(val)),
            (GlobalEntity::FuncRef(global), UnguardedVal::FuncRef(val)) => Ok(global.set(val)),
            (GlobalEntity::ExternRef(global), UnguardedVal::ExternRef(val)) => Ok(global.set(val)),
            _ => Err(GlobalError::ValTypeMismatch),
        }
    }

    /// Converts the given [`UnguardedGlobal`] to a [`Global`].
    ///
    /// # Safety
    ///
    /// The given [`UnguardedGlobal`] must be owned by the [`Store`] with the given [`StoreId`].
    pub(crate) unsafe fn from_unguarded(global: UnguardedGlobal, store_id: StoreId) -> Self {
        Self(Handle::from_unguarded(global, store_id))
    }

    /// Converts this [`Global`] to an [`UnguardedGlobal`].
    ///
    /// # Panics
    ///
    /// If this [`Global`] is not owned by the [`Store`] with the given [`StoreId`].
    pub(crate) fn to_unguarded(self, store_id: StoreId) -> UnguardedGlobal {
        self.0.to_unguarded(store_id).into()
    }
}

/// An unguarded version of [`Global`].
pub(crate) type UnguardedGlobal = UnguardedHandle<GlobalEntity>;

/// The type of a [`Global`].
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct GlobalType {
    /// The [`Mut`] of the [`Global`]
    pub mut_: Mut,
    /// The [`ValType`] of the [`Global`].
    pub val: ValType,
}

impl Decode for GlobalType {
    fn decode(decoder: &mut Decoder<'_>) -> Result<Self, DecodeError> {
        let val = decoder.decode()?;
        let mut_ = decoder.decode()?;
        Ok(Self { val, mut_ })
    }
}

/// The mutability of a `Global`.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Mut {
    /// The global is a constant.
    Const,
    /// The global is a variable.
    Var,
}

impl Decode for Mut {
    fn decode(decoder: &mut Decoder<'_>) -> Result<Self, DecodeError> {
        match decoder.read_byte()? {
            0x00 => Ok(Self::Const),
            0x01 => Ok(Self::Var),
            _ => Err(DecodeError::new("malformed mutability")),
        }
    }
}

/// An error which can occur when operating on a [`Global`].
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub enum GlobalError {
    Immutable,
    ValTypeMismatch,
}

impl fmt::Display for GlobalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GlobalError::Immutable => write!(f, "global is immutable"),
            GlobalError::ValTypeMismatch => write!(f, "value type mismatch"),
        }
    }
}

impl Error for GlobalError {}

/// The representation of a [`Global`] in a [`Store`].
#[derive(Debug)]
pub(crate) enum GlobalEntity {
    I32(GlobalEntityT<i32>),
    I64(GlobalEntityT<i64>),
    F32(GlobalEntityT<f32>),
    F64(GlobalEntityT<f64>),
    FuncRef(GlobalEntityT<UnguardedFuncRef>),
    ExternRef(GlobalEntityT<UnguardedExternRef>),
}

impl GlobalEntity {
    /// Returns a reference to the inner value of this [`GlobalEntity`] if it is a
    /// [`GlobalEntityT<T>`].
    pub(crate) fn downcast_ref<T>(&self) -> Option<&GlobalEntityT<T>>
    where
        GlobalEntityT<T>: DowncastRef<Self>,
    {
        GlobalEntityT::downcast_ref(self)
    }

    /// Returns a mutable reference to the inner value of this [`GlobalEntity`] if it is a
    /// [`GlobalEntityT<T>`].
    pub(crate) fn downcast_mut<T>(&mut self) -> Option<&mut GlobalEntityT<T>>
    where
        GlobalEntityT<T>: DowncastMut<Self>,
    {
        GlobalEntityT::downcast_mut(self)
    }
}

/// A typed [`GlobalEntity`].
#[derive(Debug)]
pub(crate) struct GlobalEntityT<T> {
    mut_: Mut,
    val: T,
}

impl<T> GlobalEntityT<T>
where
    T: Copy,
{
    /// Creates a new [`GlobalEntityT`] with the given [`Mut`] and value.
    fn new(mut_: Mut, val: T) -> Self {
        Self { mut_, val }
    }

    /// Returns the [`Mut`] of this [`GlobalEntityT`].
    fn mut_(&self) -> Mut {
        self.mut_
    }

    /// Returns the value of this [`GlobalEntityT`].
    pub(crate) fn get(&self) -> T {
        self.val
    }

    /// Sets the value of this [`GlobalEntityT`] to the given value.
    pub(crate) fn set(&mut self, val: T) {
        self.val = val;
    }
}

impl DowncastRef<GlobalEntity> for GlobalEntityT<i32> {
    fn downcast_ref(global: &GlobalEntity) -> Option<&GlobalEntityT<i32>> {
        match global {
            GlobalEntity::I32(global) => Some(global),
            _ => None,
        }
    }
}

impl DowncastMut<GlobalEntity> for GlobalEntityT<i32> {
    fn downcast_mut(global: &mut GlobalEntity) -> Option<&mut GlobalEntityT<i32>> {
        match global {
            GlobalEntity::I32(global) => Some(global),
            _ => None,
        }
    }
}

impl DowncastRef<GlobalEntity> for GlobalEntityT<i64> {
    fn downcast_ref(global: &GlobalEntity) -> Option<&GlobalEntityT<i64>> {
        match global {
            GlobalEntity::I64(global) => Some(global),
            _ => None,
        }
    }
}

impl DowncastMut<GlobalEntity> for GlobalEntityT<i64> {
    fn downcast_mut(global: &mut GlobalEntity) -> Option<&mut GlobalEntityT<i64>> {
        match global {
            GlobalEntity::I64(global) => Some(global),
            _ => None,
        }
    }
}

impl DowncastRef<GlobalEntity> for GlobalEntityT<f32> {
    fn downcast_ref(global: &GlobalEntity) -> Option<&GlobalEntityT<f32>> {
        match global {
            GlobalEntity::F32(global) => Some(global),
            _ => None,
        }
    }
}

impl DowncastMut<GlobalEntity> for GlobalEntityT<f32> {
    fn downcast_mut(global: &mut GlobalEntity) -> Option<&mut GlobalEntityT<f32>> {
        match global {
            GlobalEntity::F32(global) => Some(global),
            _ => None,
        }
    }
}

impl DowncastRef<GlobalEntity> for GlobalEntityT<f64> {
    fn downcast_ref(global: &GlobalEntity) -> Option<&GlobalEntityT<f64>> {
        match global {
            GlobalEntity::F64(global) => Some(global),
            _ => None,
        }
    }
}

impl DowncastMut<GlobalEntity> for GlobalEntityT<f64> {
    fn downcast_mut(global: &mut GlobalEntity) -> Option<&mut GlobalEntityT<f64>> {
        match global {
            GlobalEntity::F64(global) => Some(global),
            _ => None,
        }
    }
}

impl DowncastRef<GlobalEntity> for GlobalEntityT<UnguardedFuncRef> {
    fn downcast_ref(global: &GlobalEntity) -> Option<&GlobalEntityT<UnguardedFuncRef>> {
        match global {
            GlobalEntity::FuncRef(global) => Some(global),
            _ => None,
        }
    }
}

impl DowncastMut<GlobalEntity> for GlobalEntityT<UnguardedFuncRef> {
    fn downcast_mut(global: &mut GlobalEntity) -> Option<&mut GlobalEntityT<UnguardedFuncRef>> {
        match global {
            GlobalEntity::FuncRef(global) => Some(global),
            _ => None,
        }
    }
}

impl DowncastRef<GlobalEntity> for GlobalEntityT<UnguardedExternRef> {
    fn downcast_ref(global: &GlobalEntity) -> Option<&GlobalEntityT<UnguardedExternRef>> {
        match global {
            GlobalEntity::ExternRef(global) => Some(global),
            _ => None,
        }
    }
}

impl DowncastMut<GlobalEntity> for GlobalEntityT<UnguardedExternRef> {
    fn downcast_mut(global: &mut GlobalEntity) -> Option<&mut GlobalEntityT<UnguardedExternRef>> {
        match global {
            GlobalEntity::ExternRef(global) => Some(global),
            _ => None,
        }
    }
}
