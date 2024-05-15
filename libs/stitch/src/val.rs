use {
    crate::{
        decode::{Decode, DecodeError, Decoder},
        extern_ref::{ExternRef, UnguardedExternRef},
        func_ref::{FuncRef, UnguardedFuncRef},
        ref_::{Ref, RefType, UnguardedRef},
        stack::StackSlot,
        store::StoreId,
    },
    std::fmt,
};

/// A Wasm value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Val {
    I32(i32),
    I64(i64),
    F32(f32),
    F64(f64),
    FuncRef(FuncRef),
    ExternRef(ExternRef),
}

impl Val {
    /// Returns a default [`Val`] of the given [`ValType`].
    pub fn default(type_: ValType) -> Self {
        match type_ {
            ValType::I32 => 0i32.into(),
            ValType::I64 => 0i64.into(),
            ValType::F32 => 0f32.into(),
            ValType::F64 => 0f64.into(),
            ValType::FuncRef => FuncRef::null().into(),
            ValType::ExternRef => ExternRef::null().into(),
        }
    }

    /// Returns the [`ValType`] of this [`Val`].
    pub fn type_(self) -> ValType {
        match self {
            Val::I32(_) => ValType::I32,
            Val::I64(_) => ValType::I64,
            Val::F32(_) => ValType::F32,
            Val::F64(_) => ValType::F64,
            Val::FuncRef(_) => ValType::FuncRef,
            Val::ExternRef(_) => ValType::ExternRef,
        }
    }

    /// Returns `true` if this [`Val`] is an `i32`.
    pub fn is_i32(self) -> bool {
        self.to_i32().is_some()
    }

    /// Returns `true` if this [`Val`] is an `i64`.
    pub fn is_i64(self) -> bool {
        self.to_i64().is_some()
    }

    /// Returns `true` if this [`Val`] is an `f32`.
    pub fn is_f32(self) -> bool {
        self.to_f32().is_some()
    }

    /// Returns `true` if this [`Val`] is an `f64`.
    pub fn is_f64(self) -> bool {
        self.to_f64().is_some()
    }

    /// Returns `true` if this [`Val`] is a [`Ref`].
    pub fn is_ref(self) -> bool {
        self.to_ref().is_some()
    }

    /// Returns `true` if this [`Val`] is a [`FuncRef`].
    pub fn is_func_ref(self) -> bool {
        self.to_func_ref().is_some()
    }

    /// Returns `true` if this [`Val`] is an [`ExternRef`].
    pub fn is_extern_ref(self) -> bool {
        self.to_extern_ref().is_some()
    }

    /// Converts this [`Val`] to an `i32`, if it is one.
    pub fn to_i32(self) -> Option<i32> {
        match self {
            Val::I32(val) => Some(val),
            _ => None,
        }
    }

    /// Converts this [`Val`] to an `i64`, if it is one.
    pub fn to_i64(self) -> Option<i64> {
        match self {
            Val::I64(val) => Some(val),
            _ => None,
        }
    }

    /// Converts this [`Val`] to an `f32`, if it is one.
    pub fn to_f32(self) -> Option<f32> {
        match self {
            Val::F32(val) => Some(val),
            _ => None,
        }
    }

    /// Converts this [`Val`] to an `f64`, if it is one.
    pub fn to_f64(self) -> Option<f64> {
        match self {
            Val::F64(val) => Some(val),
            _ => None,
        }
    }

    /// Converts this [`Val`] to a [`Ref`], if it is one.
    pub fn to_ref(self) -> Option<Ref> {
        match self {
            Val::FuncRef(val) => Some(val.into()),
            Val::ExternRef(val) => Some(val.into()),
            _ => None,
        }
    }

    /// Converts this [`Val`] to a [`FuncRef`], if it is one.
    pub fn to_func_ref(self) -> Option<FuncRef> {
        match self {
            Val::FuncRef(val) => Some(val),
            _ => None,
        }
    }

    /// Converts this [`Val`] to an [`ExternRef`], if it is one.
    pub fn to_extern_ref(self) -> Option<ExternRef> {
        match self {
            Val::ExternRef(val) => Some(val),
            _ => None,
        }
    }

    /// Converts the given [`UnguardedVal`] to a [`Val`].
    ///
    /// # Safety
    ///
    /// The [`UnguardedVal`] must be owned by the [`Store`] with the given [`StoreId`].
    pub(crate) unsafe fn from_unguarded(val: UnguardedVal, store_id: StoreId) -> Self {
        match val {
            UnguardedVal::I32(val) => val.into(),
            UnguardedVal::I64(val) => val.into(),
            UnguardedVal::F32(val) => val.into(),
            UnguardedVal::F64(val) => val.into(),
            UnguardedVal::FuncRef(val) => FuncRef::from_unguarded(val, store_id).into(),
            UnguardedVal::ExternRef(val) => ExternRef::from_unguarded(val, store_id).into(),
        }
    }

    /// Converts this [`Val`] to an [`UnguardedVal`].
    ///
    /// # Panics
    ///
    /// This [`Val`] is not owned by the [`Store`] with the given [`StoreId`].
    pub(crate) fn to_unguarded(self, store_id: StoreId) -> UnguardedVal {
        match self {
            Val::I32(val) => val.into(),
            Val::I64(val) => val.into(),
            Val::F32(val) => val.into(),
            Val::F64(val) => val.into(),
            Val::FuncRef(val) => val.to_unguarded(store_id).into(),
            Val::ExternRef(val) => val.to_unguarded(store_id).into(),
        }
    }
}

impl From<i32> for Val {
    fn from(val: i32) -> Self {
        Val::I32(val)
    }
}

impl From<i64> for Val {
    fn from(val: i64) -> Self {
        Val::I64(val)
    }
}

impl From<f32> for Val {
    fn from(val: f32) -> Self {
        Val::F32(val)
    }
}

impl From<f64> for Val {
    fn from(val: f64) -> Self {
        Val::F64(val)
    }
}

impl From<FuncRef> for Val {
    fn from(val: FuncRef) -> Self {
        Val::FuncRef(val)
    }
}

impl From<ExternRef> for Val {
    fn from(val: ExternRef) -> Self {
        Val::ExternRef(val)
    }
}

impl From<Ref> for Val {
    fn from(val: Ref) -> Self {
        match val {
            Ref::FuncRef(val) => val.into(),
            Ref::ExternRef(val) => val.into(),
        }
    }
}

/// An unguarded [`Val`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum UnguardedVal {
    I32(i32),
    I64(i64),
    F32(f32),
    F64(f64),
    FuncRef(UnguardedFuncRef),
    ExternRef(UnguardedExternRef),
}

impl UnguardedVal {
    /// Reads an [`UnguardedVal`] of the given [`ValType`] from the given stack slot.
    pub(crate) unsafe fn read_from_stack(ptr: *const StackSlot, type_: ValType) -> Self {
        let val = match type_ {
            ValType::I32 => (*ptr.cast::<i32>()).into(),
            ValType::I64 => (*ptr.cast::<i64>()).into(),
            ValType::F32 => (*ptr.cast::<f32>()).into(),
            ValType::F64 => (*ptr.cast::<f64>()).into(),
            ValType::FuncRef => (*ptr.cast::<UnguardedFuncRef>()).into(),
            ValType::ExternRef => (*ptr.cast::<UnguardedExternRef>()).into(),
        };
        val
    }

    /// Writes this [`UnguardedVal`] to the given stack slot.
    pub(crate) unsafe fn write_to_stack(self, ptr: *mut StackSlot) {
        match self {
            UnguardedVal::I32(val) => *ptr.cast() = val,
            UnguardedVal::I64(val) => *ptr.cast() = val,
            UnguardedVal::F32(val) => *ptr.cast() = val,
            UnguardedVal::F64(val) => *ptr.cast() = val,
            UnguardedVal::FuncRef(val) => *ptr.cast() = val,
            UnguardedVal::ExternRef(val) => *ptr.cast() = val,
        }
    }
}

impl From<i32> for UnguardedVal {
    fn from(val: i32) -> Self {
        UnguardedVal::I32(val)
    }
}

impl From<i64> for UnguardedVal {
    fn from(val: i64) -> Self {
        UnguardedVal::I64(val)
    }
}

impl From<f32> for UnguardedVal {
    fn from(val: f32) -> Self {
        UnguardedVal::F32(val)
    }
}

impl From<f64> for UnguardedVal {
    fn from(val: f64) -> Self {
        UnguardedVal::F64(val)
    }
}

impl From<UnguardedRef> for UnguardedVal {
    fn from(val: UnguardedRef) -> Self {
        match val {
            UnguardedRef::FuncRef(val) => val.into(),
            UnguardedRef::ExternRef(val) => val.into(),
        }
    }
}

impl From<UnguardedFuncRef> for UnguardedVal {
    fn from(val: UnguardedFuncRef) -> Self {
        UnguardedVal::FuncRef(val)
    }
}

impl From<UnguardedExternRef> for UnguardedVal {
    fn from(val: UnguardedExternRef) -> Self {
        UnguardedVal::ExternRef(val)
    }
}

/// The type of a [`Val`].
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ValType {
    I32,
    I64,
    F32,
    F64,
    FuncRef,
    ExternRef,
}

impl ValType {
    /// Returns `true` if this [`ValType`] is a number type.
    pub fn is_num(self) -> bool {
        match self {
            Self::I32 | Self::I64 | Self::F32 | Self::F64 => true,
            _ => false,
        }
    }

    /// Returns `true` if this [`ValType`] is a `RefType`.
    pub fn is_ref(self) -> bool {
        self.to_ref().is_some()
    }

    /// Converts this [`ValType`] to a `RefType`, if it is one.
    pub fn to_ref(self) -> Option<RefType> {
        match self {
            Self::FuncRef => Some(RefType::FuncRef),
            Self::ExternRef => Some(RefType::ExternRef),
            _ => None,
        }
    }

    /// Returns the index of the register to be used for [`Val`]s of this [`ValType`].
    pub(crate) fn reg_idx(self) -> usize {
        match self {
            ValType::I32 | ValType::I64 | ValType::FuncRef | ValType::ExternRef => 0,
            ValType::F32 | ValType::F64 => 1,
        }
    }
}

impl Decode for ValType {
    fn decode(decoder: &mut Decoder<'_>) -> Result<Self, DecodeError> {
        match decoder.read_byte()? {
            0x6F => Ok(Self::ExternRef),
            0x70 => Ok(Self::FuncRef),
            0x7C => Ok(Self::F64),
            0x7D => Ok(Self::F32),
            0x7E => Ok(Self::I64),
            0x7F => Ok(Self::I32),
            _ => Err(DecodeError::new("malformed value type")),
        }
    }
}

impl fmt::Display for ValType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::I32 => write!(f, "i32"),
            Self::I64 => write!(f, "i64"),
            Self::F32 => write!(f, "f32"),
            Self::F64 => write!(f, "f64"),
            Self::FuncRef => write!(f, "funcref"),
            Self::ExternRef => write!(f, "externref"),
        }
    }
}

impl From<RefType> for ValType {
    fn from(val: RefType) -> Self {
        match val {
            RefType::FuncRef => Self::FuncRef,
            RefType::ExternRef => Self::ExternRef,
        }
    }
}
