use {
    crate::{
        aliasable_box::AliasableBox,
        data::DataEntity,
        elem::ElemEntity,
        engine::Engine,
        extern_::ExternEntity,
        func::{FuncEntity, FuncType},
        global::GlobalEntity,
        mem::MemEntity,
        table::TableEntity,
    },
    std::{
        collections::HashMap,
        fmt,
        hash::{Hash, Hasher},
        ptr::NonNull,
    },
};

/// A Wasm store.
#[derive(Debug)]
pub struct Store {
    engine: Engine,
    id: StoreId,
    types: FuncTypeInterner,
    funcs: Vec<AliasableBox<FuncEntity>>,
    tables: Vec<AliasableBox<TableEntity>>,
    mems: Vec<AliasableBox<MemEntity>>,
    globals: Vec<AliasableBox<GlobalEntity>>,
    elems: Vec<AliasableBox<ElemEntity>>,
    datas: Vec<AliasableBox<DataEntity>>,
    externs: Vec<AliasableBox<ExternEntity>>,
}

impl Store {
    pub fn new(engine: Engine) -> Self {
        let id = StoreId::new();
        Self {
            engine,
            id,
            types: FuncTypeInterner::new(id),
            funcs: Vec::new(),
            tables: Vec::new(),
            mems: Vec::new(),
            globals: Vec::new(),
            elems: Vec::new(),
            datas: Vec::new(),
            externs: Vec::new(),
        }
    }

    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    pub(crate) fn id(&self) -> StoreId {
        self.id
    }

    pub(crate) fn resolve_type(&self, type_: InternedFuncType) -> &FuncType {
        self.types.resolve(type_)
    }

    pub(crate) fn get_or_intern_type(&mut self, type_: &FuncType) -> InternedFuncType {
        self.types.get_or_intern(type_)
    }

    /// Inserts the given [`FuncEntity`] into this [`Store`].
    ///
    /// Returns a [`Handle`] to the inserted [`FuncEntity`].
    pub(crate) fn insert_func(&mut self, func: FuncEntity) -> Handle<FuncEntity> {
        let func = AliasableBox::from_box(Box::new(func));
        let handle = unsafe { Handle::from_unguarded(AliasableBox::as_raw(&func), self.id) };
        self.funcs.push(func);
        handle
    }

    /// Inserts the given [`TableEntity`] into this [`Store`].
    ///
    /// Returns a [`Handle`] to the inserted [`TableEntity`].
    pub(crate) fn insert_table(&mut self, table: TableEntity) -> Handle<TableEntity> {
        let table = AliasableBox::from_box(Box::new(table));
        let handle = unsafe { Handle::from_unguarded(AliasableBox::as_raw(&table), self.id) };
        self.tables.push(table);
        handle
    }

    /// Inserts the given [`MemEntity`] into this [`Store`].
    ///
    /// Returns a [`Handle`] to the inserted [`MemEntity`].
    pub(crate) fn insert_mem(&mut self, mem: MemEntity) -> Handle<MemEntity> {
        let mem = AliasableBox::from_box(Box::new(mem));
        let handle = unsafe { Handle::from_unguarded(AliasableBox::as_raw(&mem), self.id) };
        self.mems.push(mem);
        handle
    }

    /// Inserts the given [`GlobalEntity`] into this [`Store`].
    ///
    /// Returns a [`Handle`] to the inserted [`GlobalEntity`].
    pub(crate) fn insert_global(&mut self, global: GlobalEntity) -> Handle<GlobalEntity> {
        let global = AliasableBox::from_box(Box::new(global));
        let handle = unsafe { Handle::from_unguarded(AliasableBox::as_raw(&global), self.id) };
        self.globals.push(global);
        handle
    }

    /// Inserts the given [`ElemEntity`] into this [`Store`].
    ///
    /// Returns a [`Handle`] to the inserted [`ElemEntity`].
    pub(crate) fn insert_elem(&mut self, elem: ElemEntity) -> Handle<ElemEntity> {
        let elem = AliasableBox::from_box(Box::new(elem));
        let handle = unsafe { Handle::from_unguarded(AliasableBox::as_raw(&elem), self.id) };
        self.elems.push(elem);
        handle
    }

    /// Inserts the given [`DataEntity`] into this [`Store`].
    ///
    /// Returns a [`Handle`] to the inserted [`DataEntity`].
    pub(crate) fn insert_data(&mut self, data: DataEntity) -> Handle<DataEntity> {
        let data = AliasableBox::from_box(Box::new(data));
        let handle = unsafe { Handle::from_unguarded(AliasableBox::as_raw(&data), self.id) };
        self.datas.push(data);
        handle
    }

    /// Inserts the given [`ExternEntity`] into this [`Store`].
    ///
    /// Returns a [`Handle`] to the inserted [`ExternEntity`].
    pub(crate) fn insert_extern(&mut self, extern_: ExternEntity) -> Handle<ExternEntity> {
        let mut extern_ = AliasableBox::from_box(Box::new(extern_));
        let handle =
            unsafe { Handle::from_unguarded(NonNull::new_unchecked(&mut *extern_), self.id) };
        self.externs.push(extern_);
        handle
    }
}

/// A unique identifier for a [`Store`].
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct StoreId(usize);

impl StoreId {
    pub(crate) fn new() -> Self {
        use std::sync::atomic::{AtomicUsize, Ordering};

        static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

        Self(
            NEXT_ID
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |id| id.checked_add(1))
                .unwrap(),
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct InternedFuncType {
    type_: UnguardedInternedFuncType,
    store_id: StoreId,
}

impl InternedFuncType {
    pub(crate) unsafe fn from_unguarded(
        type_: UnguardedInternedFuncType,
        store_id: StoreId,
    ) -> Self {
        Self { type_, store_id }
    }

    pub(crate) fn to_unguarded(self, store_id: StoreId) -> UnguardedInternedFuncType {
        assert_eq!(store_id, self.store_id);
        self.type_
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct UnguardedInternedFuncType(usize);

pub(crate) struct Handle<T> {
    unguarded: UnguardedHandle<T>,
    store_id: StoreId,
}

impl<T> Handle<T> {
    pub(crate) fn as_ref(self, store: &Store) -> &T {
        assert_eq!(store.id, self.store_id, "store mismatch");
        unsafe { self.unguarded.as_ref() }
    }

    pub(crate) fn as_mut(mut self, store: &mut Store) -> &mut T {
        assert_eq!(store.id, self.store_id, "store mismatch");
        unsafe { self.unguarded.as_mut() }
    }

    pub(crate) unsafe fn from_unguarded(unguarded: UnguardedHandle<T>, store_id: StoreId) -> Self {
        Self {
            unguarded,
            store_id,
        }
    }

    pub(crate) fn to_unguarded(self, store_id: StoreId) -> UnguardedHandle<T> {
        assert_eq!(store_id, self.store_id);
        self.unguarded
    }
}

impl<T> Clone for Handle<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for Handle<T> {}

impl<T> fmt::Debug for Handle<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Handle")
            .field("handle", &self.unguarded)
            .field("store_id", &self.store_id)
            .finish()
    }
}

impl<T> Eq for Handle<T> {}

impl<T> Hash for Handle<T> {
    fn hash<H>(&self, state: &mut H)
    where
        H: Hasher,
    {
        self.unguarded.hash(state);
        self.store_id.hash(state);
    }
}

impl<T> PartialEq for Handle<T> {
    fn eq(&self, other: &Self) -> bool {
        if self.unguarded != other.unguarded {
            return false;
        }
        if self.store_id != other.store_id {
            return false;
        }
        true
    }
}

pub(crate) type UnguardedHandle<T> = NonNull<T>;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct HandlePair<T, U>(pub(crate) Handle<T>, pub(crate) Handle<U>);

impl<T, U> HandlePair<T, U> {
    pub(crate) fn as_mut_pair(mut self, store: &Store) -> (&mut T, &mut U) {
        assert_eq!(store.id(), self.0.store_id, "store mismatch");
        assert_eq!(store.id(), self.1.store_id, "store mismatch");
        assert_ne!(
            self.0.unguarded.as_ptr() as usize,
            self.1.unguarded.as_ptr() as usize,
            "overlapping handles"
        );
        unsafe { (self.0.unguarded.as_mut(), self.1.unguarded.as_mut()) }
    }
}

#[derive(Debug)]
struct FuncTypeInterner {
    store_id: StoreId,
    types: Vec<FuncType>,
    interned_types: HashMap<FuncType, UnguardedInternedFuncType>,
}

impl FuncTypeInterner {
    fn new(store_id: StoreId) -> Self {
        Self {
            store_id,
            types: Vec::new(),
            interned_types: HashMap::new(),
        }
    }

    fn resolve(&self, type_: InternedFuncType) -> &FuncType {
        unsafe { self.resolve_unguarded(type_.to_unguarded(self.store_id)) }
    }

    /// An unguarded version of `FuncInterner::resolve`.
    unsafe fn resolve_unguarded(&self, type_: UnguardedInternedFuncType) -> &FuncType {
        &self.types[type_.0]
    }

    fn get_or_intern(&mut self, type_: &FuncType) -> InternedFuncType {
        InternedFuncType {
            type_: self.get_or_intern_unguarded(type_),
            store_id: self.store_id,
        }
    }

    /// An unguarded version of `FuncInterner::get_or_intern`.
    fn get_or_intern_unguarded(&mut self, type_: &FuncType) -> UnguardedInternedFuncType {
        match self.interned_types.get(type_).copied() {
            Some(type_) => type_,
            None => {
                let interned_type = UnguardedInternedFuncType(self.types.len());
                self.types.push(type_.clone());
                self.interned_types.insert(type_.clone(), interned_type);
                interned_type
            }
        }
    }
}
