use {
    crate::{
        decode::{Decode, DecodeError, Decoder},
        downcast::{DowncastMut, DowncastRef},
        elem::{Elem, ElemEntity, ElemEntityT},
        extern_ref::UnguardedExternRef,
        func_ref::UnguardedFuncRef,
        limits::Limits,
        ref_::{Ref, RefType, UnguardedRef},
        store::{Handle, HandlePair, Store, StoreId, UnguardedHandle},
        trap::Trap,
    },
    std::{error::Error, fmt},
};

/// A Wasm table.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[repr(transparent)]
pub struct Table(pub(crate) Handle<TableEntity>);

impl Table {
    /// Creates a new [`Table`] with the given [`TableType`] and initialization [`Ref`] in the
    /// given [`Store`].
    ///
    /// # Errors
    ///
    /// - If the [`RefType`] of the initialization [`Ref`] does not match the [`RefType`] of the
    ///   elements in the [`Table`] to be created.
    ///
    /// # Panics
    ///
    /// - If the [`TableType`] is invalid.
    /// - If the initialization [`Ref`] is not owned by the given [`Store`].
    pub fn new(store: &mut Store, type_: TableType, val: Ref) -> Result<Self, TableError> {
        assert!(type_.is_valid(), "invalid table type");
        unsafe { Self::new_unguarded(store, type_, val.to_unguarded(store.id())) }
    }

    /// An unguarded version of [`Table::new`].
    unsafe fn new_unguarded(
        store: &mut Store,
        type_: TableType,
        val: UnguardedRef,
    ) -> Result<Self, TableError> {
        match (type_.elem, val) {
            (RefType::FuncRef, UnguardedRef::FuncRef(val)) => Ok(Self(
                store.insert_table(TableEntity::FuncRef(TableEntityT::new(type_.limits, val))),
            )),
            (RefType::ExternRef, UnguardedRef::ExternRef(val)) => Ok(Self(
                store.insert_table(TableEntity::ExternRef(TableEntityT::new(type_.limits, val))),
            )),
            _ => Err(TableError::ElemTypeMismatch),
        }
    }

    /// Returns the [`TableType`] of this [`Table`].
    pub fn type_(self, store: &Store) -> TableType {
        match self.0.as_ref(store) {
            TableEntity::FuncRef(table) => TableType {
                limits: table.limits(),
                elem: RefType::FuncRef,
            },
            TableEntity::ExternRef(table) => TableType {
                limits: table.limits(),
                elem: RefType::ExternRef,
            },
        }
    }

    /// Returns the element at the given index in this [`Table`].
    ///
    /// # Errors
    ///
    /// - If the access is out of bounds.
    pub fn get(self, store: &Store, idx: u32) -> Option<Ref> {
        self.get_unguarded(store, idx)
            .map(|val| unsafe { Ref::from_unguarded(val, store.id()) })
    }

    /// An unguarded version of [`Table::get`].
    fn get_unguarded(self, store: &Store, idx: u32) -> Option<UnguardedRef> {
        match self.0.as_ref(store) {
            TableEntity::FuncRef(table) => table.get(idx).map(UnguardedRef::FuncRef),
            TableEntity::ExternRef(table) => table.get(idx).map(UnguardedRef::ExternRef),
        }
    }

    /// Sets the element at the given index in this [`Table`] to the given [`Ref`].
    ///
    /// # Errors
    ///
    /// - If the access is out of bounds.
    /// - If the [`RefType`] of the given [`Ref`] does not match the [`RefType`] of the elements in
    ///   this [`Table`].
    ///
    /// # Panics
    ///
    /// - If the given [`Ref`] is not owned by the given [`Store`].
    pub fn set(self, store: &mut Store, idx: u32, val: Ref) -> Result<(), TableError> {
        unsafe { self.set_unguarded(store, idx, val.to_unguarded(store.id())) }
    }

    /// An unguarded version of [`Table::set`].
    unsafe fn set_unguarded(
        self,
        store: &mut Store,
        idx: u32,
        val: UnguardedRef,
    ) -> Result<(), TableError> {
        match (self.0.as_mut(store), val) {
            (TableEntity::FuncRef(table), UnguardedRef::FuncRef(val)) => table.set(idx, val),
            (TableEntity::ExternRef(table), UnguardedRef::ExternRef(val)) => table.set(idx, val),
            _ => Err(TableError::ElemTypeMismatch),
        }
    }

    /// Returns the size of this [`Table`] in number of elements.
    pub fn size(&self, store: &Store) -> u32 {
        match self.0.as_ref(store) {
            TableEntity::FuncRef(table) => table.size(),
            TableEntity::ExternRef(table) => table.size(),
        }
    }

    /// Grows this [`Table`] by the given number of elements with the given initialization [`Ref`].
    ///
    /// Returns the previous size of this [`Table`] in number of elements.
    ///
    /// # Errors
    ///
    /// - If the [`RefType`] of the given initialization [`Ref`] does not match the [`RefType`] of
    ///   the elements in this [`Table`].
    /// - If this [`Table`] failed to grow.
    ///
    /// # Panics
    ///
    /// - If the given initialization [`Ref`] is not owned by the given [`Store`].
    pub fn grow(self, store: &mut Store, val: Ref, count: u32) -> Result<(), TableError> {
        unsafe { self.grow_unguarded(store, val.to_unguarded(store.id()), count) }
    }

    /// An unguarded version of [`Table::grow`].
    unsafe fn grow_unguarded(
        self,
        store: &mut Store,
        val: UnguardedRef,
        count: u32,
    ) -> Result<(), TableError> {
        match (self.0.as_mut(store), val) {
            (TableEntity::FuncRef(table), UnguardedRef::FuncRef(val)) => table.grow(val, count),
            (TableEntity::ExternRef(table), UnguardedRef::ExternRef(val)) => table.grow(val, count),
            _ => Err(TableError::ElemTypeMismatch),
        }
        .map(|_| ())
    }

    pub(crate) fn init(
        self,
        store: &mut Store,
        dst_idx: u32,
        src_elem: Elem,
        src_idx: u32,
        count: u32,
    ) -> Result<(), Trap> {
        let (dst_table, src_elem) = HandlePair(self.0, src_elem.0).as_mut_pair(store);
        match (dst_table, src_elem) {
            (TableEntity::FuncRef(table), ElemEntity::FuncRef(src_elem)) => {
                table.init(dst_idx, src_elem, src_idx, count)
            }
            (TableEntity::ExternRef(table), ElemEntity::ExternRef(src_elem)) => {
                table.init(dst_idx, src_elem, src_idx, count)
            }
            _ => panic!(),
        }
    }

    /// Converts the given [`UnguardedTable`] to a [`Table`].
    ///
    /// # Safety
    ///
    /// The given [`UnguardedTable`] must be owned by the [`Store`] with the given [`StoreId`].
    pub(crate) unsafe fn from_unguarded(table: UnguardedTable, store_id: StoreId) -> Self {
        Self(Handle::from_unguarded(table, store_id))
    }

    /// Converts this [`Table`] to an [`UnguardedTable`].
    ///
    /// # Panics
    ///
    /// This [`Table`] is not owned by the [`Store`] with the given [`StoreId`].
    pub(crate) fn to_unguarded(self, store_id: StoreId) -> UnguardedTable {
        self.0.to_unguarded(store_id)
    }
}

/// An unguarded version of [`Table`].
pub(crate) type UnguardedTable = UnguardedHandle<TableEntity>;

/// The type of a [`Table`].
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TableType {
    /// The [`Limits`] of the [`Table`].
    pub limits: Limits,
    /// The [`RefType`] of the elements in the [`Table`].
    pub elem: RefType,
}

impl TableType {
    /// Returns `true` if this [`TableType`] is valid.
    ///
    /// A [`TableType`] is valid if its [`Limits`] are valid within range `u32::MAX`.
    pub fn is_valid(self) -> bool {
        if !self.limits.is_valid(u32::MAX) {
            return false;
        }
        true
    }

    /// Returns `true` if this [`TableType`] is a subtype of the given [`TableType`].
    ///
    /// A [`TableType`] is a subtype of another [`TableType`] if its [`Limits`] are a sublimit of
    /// the other's and the [`RefType`] of its elements is the same as the other's.
    pub fn is_subtype_of(self, other: Self) -> bool {
        if !self.limits.is_sublimit_of(other.limits) {
            return false;
        }
        if self.elem != other.elem {
            return false;
        }
        true
    }
}

impl Decode for TableType {
    fn decode(decoder: &mut Decoder<'_>) -> Result<Self, DecodeError> {
        let elem = decoder.decode()?;
        let limits = decoder.decode()?;
        Ok(Self { limits, elem })
    }
}

/// An error that can occur when operating on a [`Table`].
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub enum TableError {
    AccessOutOfBounds,
    ElemTypeMismatch,
    FailedToGrow,
}

impl fmt::Display for TableError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AccessOutOfBounds => write!(f, "table access out of bounds"),
            Self::ElemTypeMismatch => write!(f, "table element type mismatch"),
            Self::FailedToGrow => write!(f, "table failed to grow"),
        }
    }
}

impl Error for TableError {}

/// The representation of a [`Table`] in a [`Store`].
#[derive(Debug)]
pub(crate) enum TableEntity {
    FuncRef(TableEntityT<UnguardedFuncRef>),
    ExternRef(TableEntityT<UnguardedExternRef>),
}

impl TableEntity {
    /// Returns a reference to the inner value of this [`TableEntity`] if it is a
    /// [`TableEntityT<T>`].
    pub(crate) fn downcast_ref<T>(&self) -> Option<&TableEntityT<T>>
    where
        TableEntityT<T>: DowncastRef<Self>,
    {
        TableEntityT::downcast_ref(self)
    }

    /// Returns a mutable reference to the inner value of this [`TableEntity`] if it is a
    /// [`TableEntityT<T>`].
    pub(crate) fn downcast_mut<T>(&mut self) -> Option<&mut TableEntityT<T>>
    where
        TableEntityT<T>: DowncastMut<Self>,
    {
        TableEntityT::downcast_mut(self)
    }
}

/// A typed [`TableEntity`].
#[derive(Debug)]
pub(crate) struct TableEntityT<T> {
    max: Option<u32>,
    elems: Vec<T>,
}

impl<T> TableEntityT<T>
where
    T: Copy,
{
    /// Creates a new [`TableEntityT`] with the given [`Limits`] and initialization value.
    fn new(limits: Limits, val: T) -> Self {
        let min = limits.min as usize;
        Self {
            max: limits.max,
            elems: vec![val; min],
        }
    }

    /// Returns the [`Limits`] of this [`TableEntity`].
    fn limits(&self) -> Limits {
        Limits {
            min: u32::try_from(self.elems.len()).unwrap(),
            max: self.max,
        }
    }

    /// Returns the element at the given index in this [`TableEntity`].
    ///
    /// # Errors
    ///
    /// If the access is out of bounds.
    pub(crate) fn get(&self, idx: u32) -> Option<T> {
        let idx = idx as usize;
        let elem = self.elems.get(idx)?;
        Some(*elem)
    }

    /// Sets the element at the given index in this [`TableEntity`] to the given value.
    ///
    /// # Errors
    ///
    /// If the access is out of bounds.
    pub(crate) fn set(&mut self, idx: u32, val: T) -> Result<(), TableError> {
        let idx = idx as usize;
        let elem = self
            .elems
            .get_mut(idx)
            .ok_or(TableError::AccessOutOfBounds)?;
        *elem = val;
        Ok(())
    }

    /// Returns the size of this [`TableEntity`] in number of elements.
    pub(crate) fn size(&self) -> u32 {
        self.elems.len() as u32
    }

    /// Grows this [`TableEntity`] by the given number of elements with the given initialization
    /// value.
    ///
    /// Returns the previous size of this [`TableEntity`] in number of elements.
    ///
    /// # Errors
    ///
    /// If this [`TableEntity`] failed to grow.
    pub(crate) fn grow(&mut self, val: T, count: u32) -> Result<u32, TableError> {
        if count > self.max.unwrap_or(u32::MAX) - self.size() {
            return Err(TableError::FailedToGrow)?;
        }
        let count = count as usize;
        let size = self.size();
        self.elems.resize(self.elems.len() + count, val);
        Ok(size)
    }

    pub(crate) fn fill(&mut self, idx: u32, val: T, count: u32) -> Result<(), Trap> {
        let idx = idx as usize;
        let count = count as usize;
        let elems = self
            .elems
            .get_mut(idx..)
            .and_then(|elems| elems.get_mut(..count))
            .ok_or(Trap::TableAccessOutOfBounds)?;
        elems.fill(val);
        Ok(())
    }

    pub(crate) fn copy(
        &mut self,
        dst_idx: u32,
        src_table: &TableEntityT<T>,
        src_idx: u32,
        count: u32,
    ) -> Result<(), Trap> {
        let dst_idx = dst_idx as usize;
        let src_idx = src_idx as usize;
        let count = count as usize;
        let dst_elems = self
            .elems
            .get_mut(dst_idx..)
            .and_then(|elems| elems.get_mut(..count))
            .ok_or(Trap::TableAccessOutOfBounds)?;
        let src_elems = src_table
            .elems
            .get(src_idx..)
            .and_then(|elems| elems.get(..count))
            .ok_or(Trap::TableAccessOutOfBounds)?;
        dst_elems.copy_from_slice(src_elems);
        Ok(())
    }

    pub(crate) fn copy_within(
        &mut self,
        dst_idx: u32,
        src_idx: u32,
        count: u32,
    ) -> Result<(), Trap> {
        let dst_idx = dst_idx as usize;
        let src_idx = src_idx as usize;
        let count = count as usize;
        if count > self.elems.len()
            || dst_idx > self.elems.len() - count
            || src_idx > self.elems.len() - count
        {
            return Err(Trap::TableAccessOutOfBounds)?;
        }
        self.elems.copy_within(src_idx..src_idx + count, dst_idx);
        Ok(())
    }

    pub(crate) fn init(
        &mut self,
        dst_idx: u32,
        src_elem: &ElemEntityT<T>,
        src_idx: u32,
        count: u32,
    ) -> Result<(), Trap> {
        let dst_idx = dst_idx as usize;
        let src_idx = src_idx as usize;
        let count = count as usize;
        let dst_elems = self
            .elems
            .get_mut(dst_idx..)
            .and_then(|elems| elems.get_mut(..count))
            .ok_or(Trap::TableAccessOutOfBounds)?;
        let src_elems = src_elem
            .elems()
            .get(src_idx..)
            .and_then(|elems| elems.get(..count))
            .ok_or(Trap::TableAccessOutOfBounds)?;
        dst_elems.copy_from_slice(src_elems);
        Ok(())
    }
}

impl DowncastRef<TableEntity> for TableEntityT<UnguardedFuncRef> {
    fn downcast_ref(table: &TableEntity) -> Option<&Self> {
        match table {
            TableEntity::FuncRef(table) => Some(table),
            _ => None,
        }
    }
}

impl DowncastMut<TableEntity> for TableEntityT<UnguardedFuncRef> {
    fn downcast_mut(table: &mut TableEntity) -> Option<&mut Self> {
        match table {
            TableEntity::FuncRef(table) => Some(table),
            _ => None,
        }
    }
}

impl DowncastRef<TableEntity> for TableEntityT<UnguardedExternRef> {
    fn downcast_ref(table: &TableEntity) -> Option<&Self> {
        match table {
            TableEntity::ExternRef(table) => Some(table),
            _ => None,
        }
    }
}

impl DowncastMut<TableEntity> for TableEntityT<UnguardedExternRef> {
    fn downcast_mut(table: &mut TableEntity) -> Option<&mut Self> {
        match table {
            TableEntity::ExternRef(table) => Some(table),
            _ => None,
        }
    }
}
