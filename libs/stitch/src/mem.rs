use {
    crate::{
        data::{Data, DataEntity},
        decode::{Decode, DecodeError, Decoder},
        limits::Limits,
        stack::Stack,
        store::{Handle, HandlePair, Store, StoreId, UnguardedHandle},
        trap::Trap,
    },
    std::{error::Error, fmt},
};

/// A Wasm memory.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[repr(transparent)]
pub struct Mem(pub(crate) Handle<MemEntity>);

impl Mem {
    /// Creates a new [`Mem`] with the given [`MemType`].
    ///
    /// # Panics
    ///
    /// If the given [`MemType`] is invalid.
    pub fn new(store: &mut Store, type_: MemType) -> Self {
        assert!(type_.is_valid(), "invalid memory type");
        Self(store.insert_mem(MemEntity::new(type_)))
    }

    /// Returns the [`MemType`] of this [`Mem`].
    pub fn type_(self, store: &Store) -> MemType {
        MemType {
            limits: self.0.as_ref(store).limits(),
        }
    }

    /// Returns the bytes of this [`Mem`] as a slice.
    pub fn bytes(self, store: &Store) -> &[u8] {
        self.0.as_ref(store).bytes()
    }

    /// Returns the bytes of this [`Mem`] as a mutable slice.
    pub fn bytes_mut(self, store: &mut Store) -> &mut [u8] {
        self.0.as_mut(store).bytes_mut()
    }

    /// Returns the size of this [`Mem`] in number of pages.
    pub fn size(&self, store: &Store) -> u32 {
        self.0.as_ref(store).size()
    }

    /// Grows this [`Mem`] by the given number of pages.
    ///
    /// Returns the previous size of this [`Mem`] in number of pages.
    ///
    /// # Errors
    ///
    /// If this [`Mem`] failed to grow.
    pub fn grow(self, store: &mut Store, count: u32) -> Result<u32, MemError> {
        self.0.as_mut(store).grow(count)
    }

    pub(crate) fn init(
        self,
        store: &mut Store,
        dst_offset: u32,
        src_data: Data,
        src_offset: u32,
        count: u32,
    ) -> Result<(), Trap> {
        let (dst_table, src_data) = HandlePair(self.0, src_data.0).as_mut_pair(store);
        dst_table.init(dst_offset, src_data, src_offset, count)
    }

    /// Converts the given [`UnguardedMem`] to a [`Mem`].
    ///
    /// # Safety
    ///
    /// The given [`UnguardedMem`] must be owned by the [`Store`] with the given [`StoreId`].
    pub(crate) unsafe fn from_unguarded(memory: UnguardedMem, store_id: StoreId) -> Self {
        Self(Handle::from_unguarded(memory, store_id))
    }

    /// Converts this [`Mem`] to an [`UnguardedMem`].
    ///
    /// # Panics
    ///
    /// This [`Mem`] is not owned by the [`Store`] with the given [`StoreId`].
    pub(crate) fn to_unguarded(self, store_id: StoreId) -> UnguardedMem {
        self.0.to_unguarded(store_id)
    }
}

/// An unguarded version of [`Mem`].
pub(crate) type UnguardedMem = UnguardedHandle<MemEntity>;

/// The type of a [`Mem`].
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct MemType {
    /// The [`Limits`] of this [`Mem`].
    pub limits: Limits,
}

impl MemType {
    /// Returns `true` if this [`MemType`] is valid.
    ///
    /// A [`MemType`] is valid if its [`Limits`] are valid within range 65_536.
    pub fn is_valid(&self) -> bool {
        self.limits.is_valid(65_536)
    }

    /// Returns `true` if this [`MemType`] is a subtype of the given [`MemType`].
    ///
    /// A [`MemType`] is a subtype of another [`MemType`] if its [`Limits`] are a sublimit of the
    /// other's.
    pub fn is_subtype_of(self, other: Self) -> bool {
        self.limits.is_sublimit_of(other.limits)
    }
}

impl Decode for MemType {
    fn decode(decoder: &mut Decoder<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            limits: Limits::decode(decoder)?,
        })
    }
}

/// An error that can occur when operating on a [`Mem`].
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub enum MemError {
    FailedToGrow,
}

impl fmt::Display for MemError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FailedToGrow => write!(f, "memory failed to grow"),
        }
    }
}

impl Error for MemError {}

/// The representation of a [`Mem`] in a [`Store`].
#[derive(Debug)]
pub(crate) struct MemEntity {
    max: Option<u32>,
    bytes: Vec<u8>,
}

impl MemEntity {
    /// Creates a new [`MemEntity`] with the given [`MemType`].
    fn new(type_: MemType) -> Self {
        Self {
            max: type_.limits.max,
            bytes: vec![0; (type_.limits.min as usize).checked_mul(PAGE_SIZE).unwrap()],
        }
    }

    /// Returns the [`Limits`] of this [`MemEntity`].
    fn limits(&self) -> Limits {
        Limits {
            min: self.size(),
            max: self.max,
        }
    }

    /// Returns the bytes of this [`MemEntity`] as a slice.
    pub(crate) fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Returns the bytes of this [`MemEntity`] as a mutable slice.
    pub(crate) fn bytes_mut(&mut self) -> &mut [u8] {
        &mut self.bytes
    }

    /// Returns the size of this [`MemEntity`] in number of pages.
    pub(crate) fn size(&self) -> u32 {
        u32::try_from(self.bytes.len() / PAGE_SIZE).unwrap()
    }

    /// Grows this [`MemEntity`] by the given number of pages.
    ///
    /// Returns the previous size of this [`MemEntity`] in number of pages.
    ///
    /// # Errors
    ///
    /// If this [`MemEntity`] failed to grow.
    pub(crate) fn grow(&mut self, count: u32) -> Result<u32, MemError> {
        let mut stack = Stack::lock();
        unsafe { self.grow_with_stack(count, &mut stack) }
    }

    pub(crate) unsafe fn grow_with_stack(
        &mut self,
        count: u32,
        stack: &mut Stack,
    ) -> Result<u32, MemError> {
        if count > self.max.unwrap_or(65_536) - self.size() {
            return Err(MemError::FailedToGrow);
        }
        let old_data = self.bytes.as_mut_ptr();
        let old_size = self.size();
        let new_size = self.size() + count;
        self.bytes
            .resize((new_size as usize).checked_mul(PAGE_SIZE).unwrap(), 0);
        let new_data = self.bytes.as_mut_ptr();
        let mut ptr = stack.ptr();
        while ptr != stack.base_ptr() {
            ptr = *ptr.offset(-3).cast();
            if *ptr.offset(-2).cast::<*mut u8>() == old_data {
                *ptr.offset(-2).cast() = new_data;
                *ptr.offset(-1).cast() = new_size;
            }
        }
        Ok(old_size)
    }

    pub(crate) fn fill(&mut self, idx: u32, val: u8, count: u32) -> Result<(), Trap> {
        let idx = idx as usize;
        let count = count as usize;
        let bytes = self
            .bytes
            .get_mut(idx..)
            .and_then(|bytes| bytes.get_mut(..count))
            .ok_or(Trap::MemAccessOutOfBounds)?;
        bytes.fill(val);
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
        if count > self.bytes.len()
            || dst_idx > self.bytes.len() - count
            || src_idx > self.bytes.len() - count
        {
            return Err(Trap::MemAccessOutOfBounds);
        }
        self.bytes.copy_within(src_idx..src_idx + count, dst_idx);
        Ok(())
    }

    pub(crate) fn init(
        &mut self,
        dst_idx: u32,
        src_data: &DataEntity,
        src_idx: u32,
        count: u32,
    ) -> Result<(), Trap> {
        let dst_idx = dst_idx as usize;
        let src_idx = src_idx as usize;
        let count = count as usize;
        let dst_bytes = self
            .bytes
            .get_mut(dst_idx..)
            .and_then(|bytes| bytes.get_mut(..count))
            .ok_or(Trap::MemAccessOutOfBounds)?;
        let src_bytes = src_data
            .bytes()
            .get(src_idx..)
            .and_then(|bytes| bytes.get(..count))
            .ok_or(Trap::MemAccessOutOfBounds)?;
        dst_bytes.copy_from_slice(src_bytes);
        Ok(())
    }
}

const PAGE_SIZE: usize = 65_536;
