use crate::{
    decode::{Decode, DecodeError, Decoder},
    func::{Func, FuncType, UnguardedFunc},
    global::{Global, GlobalType, UnguardedGlobal},
    mem::{Mem, MemType, UnguardedMem},
    store::{Store, StoreId},
    table::{Table, TableType, UnguardedTable},
};

/// A Wasm entity that can be imported or exported.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ExternVal {
    Func(Func),
    Table(Table),
    Memory(Mem),
    Global(Global),
}

impl ExternVal {
    /// Returns the [`ExternType`] of this [`ExternVal`].
    pub fn type_(self, store: &Store) -> ExternType {
        match self {
            Self::Func(func) => ExternType::Func(func.type_(store).clone()),
            Self::Table(table) => ExternType::Table(table.type_(store)),
            Self::Memory(mem) => ExternType::Mem(mem.type_(store)),
            Self::Global(global) => ExternType::Global(global.type_(store)),
        }
    }

    /// Returns `true` if this [`ExternVal`] is a [`Func`].
    pub fn is_func(self) -> bool {
        self.to_func().is_some()
    }

    /// Returns `true` if this [`ExternVal`] is a [`Table`].
    pub fn is_table(self) -> bool {
        self.to_table().is_some()
    }

    /// Returns `true` if this [`ExternVal`] is a [`Mem`].
    pub fn is_mem(self) -> bool {
        self.to_mem().is_some()
    }

    /// Returns `true` if this [`ExternVal`] is a [`Global`].
    pub fn is_global(self) -> bool {
        self.to_global().is_some()
    }

    /// Converts this [`ExternVal`] to a [`Func`], if it is one.
    pub fn to_func(self) -> Option<Func> {
        match self {
            Self::Func(func) => Some(func),
            _ => None,
        }
    }

    /// Converts this [`ExternVal`] to a [`Table`], if it is one.
    pub fn to_table(self) -> Option<Table> {
        match self {
            Self::Table(table) => Some(table),
            _ => None,
        }
    }

    /// Converts this [`ExternVal`] to a [`Mem`], if it is one.
    pub fn to_mem(self) -> Option<Mem> {
        match self {
            Self::Memory(mem) => Some(mem),
            _ => None,
        }
    }

    /// Converts this [`ExternVal`] to a [`Global`], if it is one.
    pub fn to_global(self) -> Option<Global> {
        match self {
            Self::Global(global) => Some(global),
            _ => None,
        }
    }

    /// Converts the given [`UnguardedExternVal`] to an [`ExternVal`].
    ///
    /// # Safety
    ///
    /// Any [`UnguardedHandle`] in the given [`UnguardedExternVal`] must be owned by the [`Store`]
    /// with the given [`StoreId`].
    pub(crate) unsafe fn from_unguarded(val: UnguardedExternVal, store_id: StoreId) -> Self {
        match val {
            UnguardedExternVal::Func(func) => Func::from_unguarded(func, store_id).into(),
            UnguardedExternVal::Table(table) => Table::from_unguarded(table, store_id).into(),
            UnguardedExternVal::Mem(mem) => Mem::from_unguarded(mem, store_id).into(),
            UnguardedExternVal::Global(global) => Global::from_unguarded(global, store_id).into(),
        }
    }

    /// Converts this [`ExternVal`] to an [`UnguardedExternVal`].
    ///
    /// # Panics
    ///
    /// Panics if any [`Handle`] in this [`ExternVal`] is not owned by the [`Store`] with the given
    /// [`StoreId`].
    pub(crate) fn to_unguarded(self, store_id: StoreId) -> UnguardedExternVal {
        match self {
            Self::Func(func) => func.to_unguarded(store_id).into(),
            Self::Table(table) => table.to_unguarded(store_id).into(),
            Self::Memory(mem) => mem.to_unguarded(store_id).into(),
            Self::Global(global) => global.to_unguarded(store_id).into(),
        }
    }
}

impl From<Func> for ExternVal {
    fn from(func: Func) -> Self {
        Self::Func(func)
    }
}

impl From<Table> for ExternVal {
    fn from(table: Table) -> Self {
        Self::Table(table)
    }
}

impl From<Mem> for ExternVal {
    fn from(memory: Mem) -> Self {
        Self::Memory(memory)
    }
}

impl From<Global> for ExternVal {
    fn from(global: Global) -> Self {
        Self::Global(global)
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum UnguardedExternVal {
    Func(UnguardedFunc),
    Table(UnguardedTable),
    Mem(UnguardedMem),
    Global(UnguardedGlobal),
}

impl From<UnguardedFunc> for UnguardedExternVal {
    fn from(func: UnguardedFunc) -> Self {
        Self::Func(func)
    }
}

impl From<UnguardedTable> for UnguardedExternVal {
    fn from(table: UnguardedTable) -> Self {
        Self::Table(table)
    }
}

impl From<UnguardedMem> for UnguardedExternVal {
    fn from(mem: UnguardedMem) -> Self {
        Self::Mem(mem)
    }
}

impl From<UnguardedGlobal> for UnguardedExternVal {
    fn from(global: UnguardedGlobal) -> Self {
        Self::Global(global)
    }
}

/// An descriptor for an [`ExternVal`].
///
/// This is just like an [`ExternVal`], except that each entity is represented by an index into
/// the respective section of a module.
#[derive(Clone, Copy, Debug)]
pub(crate) enum ExternValDesc {
    Func(u32),
    Table(u32),
    Memory(u32),
    Global(u32),
}

impl Decode for ExternValDesc {
    fn decode(decoder: &mut Decoder<'_>) -> Result<Self, DecodeError> {
        match decoder.read_byte()? {
            0x00 => Ok(Self::Func(decoder.decode()?)),
            0x01 => Ok(Self::Table(decoder.decode()?)),
            0x02 => Ok(Self::Memory(decoder.decode()?)),
            0x03 => Ok(Self::Global(decoder.decode()?)),
            _ => Err(DecodeError::new("malformed external value descriptor")),
        }
    }
}

/// The type of an [`ExternVal`].
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum ExternType {
    Func(FuncType),
    Table(TableType),
    Mem(MemType),
    Global(GlobalType),
}

impl ExternType {
    /// Returns `true` if this [`ExternType`] is a [`FuncType`].
    pub fn is_func(self) -> bool {
        self.to_func().is_some()
    }

    /// Returns `true` if this [`ExternType`] is a [`TableType`].
    pub fn is_table(self) -> bool {
        self.to_table().is_some()
    }

    /// Returns `true` if this [`ExternType`] is a [`MemType`].
    pub fn is_mem(self) -> bool {
        self.to_mem().is_some()
    }

    /// Returns `true` if this [`ExternType`] is a [`GlobalType`].
    pub fn is_global(self) -> bool {
        self.to_global().is_some()
    }

    /// Returns the underlying [`FuncType`] if this [`ExternType`] is a [`FuncType`].
    pub fn to_func(self) -> Option<FuncType> {
        match self {
            Self::Func(func_type) => Some(func_type),
            _ => None,
        }
    }

    /// Converts this [`ExternType`] to a [`TableType`], if it is one.
    pub fn to_table(self) -> Option<TableType> {
        match self {
            Self::Table(table_type) => Some(table_type),
            _ => None,
        }
    }

    /// Converts this [`ExternType`] to a [`MemType`], if it is one.
    pub fn to_mem(self) -> Option<MemType> {
        match self {
            Self::Mem(mem_type) => Some(mem_type),
            _ => None,
        }
    }

    /// Converts this [`ExternType`] to a [`GlobalType`], if it is one.
    pub fn to_global(self) -> Option<GlobalType> {
        match self {
            Self::Global(global_type) => Some(global_type),
            _ => None,
        }
    }
}

impl From<FuncType> for ExternType {
    fn from(type_: FuncType) -> Self {
        Self::Func(type_)
    }
}

impl From<TableType> for ExternType {
    fn from(type_: TableType) -> Self {
        Self::Table(type_)
    }
}

impl From<MemType> for ExternType {
    fn from(type_: MemType) -> Self {
        Self::Mem(type_)
    }
}

impl From<GlobalType> for ExternType {
    fn from(type_: GlobalType) -> Self {
        Self::Global(type_)
    }
}

/// A descriptor for an [`ExternType`].
///
/// This is just like an [`ExternType`], except that function types are represented by an index
/// into the type section of a module.
#[derive(Clone, Copy, Debug)]
pub(crate) enum ExternTypeDesc {
    Func(u32),
    Table(TableType),
    Memory(MemType),
    Global(GlobalType),
}

impl Decode for ExternTypeDesc {
    fn decode(decoder: &mut Decoder<'_>) -> Result<Self, DecodeError> {
        match decoder.read_byte()? {
            0x00 => Ok(ExternTypeDesc::Func(decoder.decode()?)),
            0x01 => Ok(ExternTypeDesc::Table(decoder.decode()?)),
            0x02 => Ok(ExternTypeDesc::Memory(decoder.decode()?)),
            0x03 => Ok(ExternTypeDesc::Global(decoder.decode()?)),
            _ => Err(DecodeError::new("malformed external type descriptor"))?,
        }
    }
}
