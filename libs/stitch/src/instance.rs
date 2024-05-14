use {
    crate::{
        const_expr::EvaluationContext,
        data::{Data, UnguardedData},
        elem::{Elem, UnguardedElem},
        extern_val::{ExternVal, UnguardedExternVal},
        func::{Func, UnguardedFunc},
        global::{Global, UnguardedGlobal},
        mem::{Mem, UnguardedMem},
        store::{InternedFuncType, StoreId, UnguardedInternedFuncType},
        table::{Table, UnguardedTable},
    },
    std::{
        cell::OnceCell,
        collections::{hash_map, HashMap},
        sync::Arc,
    },
};

/// A [`Module`](`crate::Module`) instance.
#[derive(Clone, Debug)]
pub struct Instance {
    store_id: StoreId,
    inner: Arc<OnceCell<InstanceInner>>,
}

impl Instance {
    /// Returns the [`ExternVal`] of the export with the given name in this [`Instance`], if it exists.
    pub fn exported_val(&self, name: &str) -> Option<ExternVal> {
        self.inner()
            .exports
            .get(name)
            .map(|val| unsafe { ExternVal::from_unguarded(*val, self.store_id) })
    }

    /// Returns the exported [`Func`] with the given name in this [`Instance`], if it exists.
    pub fn exported_func(&self, name: &str) -> Option<Func> {
        self.exported_val(name).and_then(|val| val.to_func())
    }

    /// Returns the exported [`Table`] with the given name in this [`Instance`], if it exists.
    pub fn exported_table(&self, name: &str) -> Option<Table> {
        self.exported_val(name).and_then(|val| val.to_table())
    }

    /// Returns the exported [`Mem`] with the given name in this [`Instance`], if it exists.
    pub fn exported_mem(&self, name: &str) -> Option<Mem> {
        self.exported_val(name).and_then(|val| val.to_mem())
    }

    /// Returns the exported [`Global`] with the given name in this [`Instance`], if it exists.
    pub fn exported_global(&self, name: &str) -> Option<Global> {
        self.exported_val(name).and_then(|val| val.to_global())
    }

    /// Returns an iterator over the exports in this [`Instance`].
    pub fn exports(&self) -> InstanceExports<'_> {
        InstanceExports {
            store_id: self.store_id,
            iter: self.inner().exports.iter(),
        }
    }

    /// Creates an uninitialized [`Instance`].
    pub(crate) fn uninited(store_id: StoreId) -> Instance {
        Instance {
            store_id,
            inner: Arc::new(OnceCell::new()),
        }
    }

    /// Initialize this [`Instance`] with the given [`InstanceIniter`].
    pub(crate) fn init(&self, initer: InstanceIniter) {
        assert_eq!(initer.store_id, self.store_id);
        self.inner
            .set(InstanceInner {
                types: initer.types.into(),
                funcs: initer.funcs.into(),
                tables: initer.tables.into(),
                mems: initer.mems.into(),
                globals: initer.globals.into(),
                elems: initer.elems.into(),
                datas: initer.datas.into(),
                exports: initer.exports,
            })
            .expect("instance already initialized");
    }

    /// Returns the [`InternedFuncType`] at the given index in this [`Instance`], if it exists.
    pub(crate) fn type_(&self, idx: u32) -> Option<InternedFuncType> {
        self.inner()
            .types
            .get(idx as usize)
            .map(|type_| unsafe { InternedFuncType::from_unguarded(*type_, self.store_id) })
    }

    /// Returns the [`Func`] at the given index in this [`Instance`], if it exists.
    pub(crate) fn func(&self, idx: u32) -> Option<Func> {
        self.unguarded_func(idx)
            .map(|func| unsafe { Func::from_unguarded(func, self.store_id) })
    }

    /// An unguarded version of [`Instance::func`].
    fn unguarded_func(&self, idx: u32) -> Option<UnguardedFunc> {
        self.inner().funcs.get(idx as usize).copied()
    }

    /// Returns the [`Table`] at the given index in this [`Instance`], if it exists.
    pub(crate) fn table(&self, idx: u32) -> Option<Table> {
        self.unguarded_table(idx)
            .map(|table| unsafe { Table::from_unguarded(table, self.store_id) })
    }

    /// An unguarded version of [`Instance::table`].
    fn unguarded_table(&self, idx: u32) -> Option<UnguardedTable> {
        self.inner().tables.get(idx as usize).copied()
    }

    /// Returns the [`Mem`] at the given index in this [`Instance`], if it exists.
    pub(crate) fn mem(&self, idx: u32) -> Option<Mem> {
        self.unguarded_mem(idx)
            .map(|mem| unsafe { Mem::from_unguarded(mem, self.store_id) })
    }

    /// An unguarded version of [`Instance::mem`].
    fn unguarded_mem(&self, idx: u32) -> Option<UnguardedMem> {
        self.inner().mems.get(idx as usize).copied()
    }

    /// Returns the [`Global`] at the given index in this [`Instance`], if it exists.
    pub(crate) fn global(&self, idx: u32) -> Option<Global> {
        self.unguarded_global(idx)
            .map(|global| unsafe { Global::from_unguarded(global, self.store_id) })
    }

    /// An unguarded version of [`Instance::global`].
    fn unguarded_global(&self, idx: u32) -> Option<UnguardedGlobal> {
        self.inner().globals.get(idx as usize).copied()
    }

    /// Returns the [`Elem`] at the given index in this [`Instance`], if it exists.
    pub(crate) fn elem(&self, idx: u32) -> Option<Elem> {
        self.unguarded_elem(idx)
            .map(|elem| unsafe { Elem::from_unguarded(elem, self.store_id) })
    }

    /// An unguarded version of [`Instance::elem`].
    fn unguarded_elem(&self, idx: u32) -> Option<UnguardedElem> {
        self.inner().elems.get(idx as usize).copied()
    }

    /// Returns the [`Data`] at the given index in this [`Instance`], if it exists.
    pub(crate) fn data(&self, idx: u32) -> Option<Data> {
        self.unguarded_data(idx)
            .map(|data| unsafe { Data::from_unguarded(data, self.store_id) })
    }

    /// An unguarded version of [`Instance::data`].
    fn unguarded_data(&self, idx: u32) -> Option<UnguardedData> {
        self.inner().datas.get(idx as usize).copied()
    }

    fn inner(&self) -> &InstanceInner {
        self.inner.get().expect("instance not yet initialized")
    }
}

impl EvaluationContext for Instance {
    fn func(&self, idx: u32) -> Option<Func> {
        self.func(idx)
    }

    fn global(&self, idx: u32) -> Option<Global> {
        self.global(idx)
    }
}

/// An iterator over the exports in an [`Instance`].
#[derive(Clone, Debug)]
pub struct InstanceExports<'a> {
    store_id: StoreId,
    iter: hash_map::Iter<'a, Arc<str>, UnguardedExternVal>,
}

impl<'a> Iterator for InstanceExports<'a> {
    type Item = (&'a str, ExternVal);

    fn next(&mut self) -> Option<Self::Item> {
        self.iter.next().map(|(name, val)| {
            (&**name, unsafe {
                ExternVal::from_unguarded(*val, self.store_id)
            })
        })
    }
}

#[derive(Debug)]
struct InstanceInner {
    types: Box<[UnguardedInternedFuncType]>,
    funcs: Box<[UnguardedFunc]>,
    tables: Box<[UnguardedTable]>,
    mems: Box<[UnguardedMem]>,
    globals: Box<[UnguardedGlobal]>,
    elems: Box<[UnguardedElem]>,
    datas: Box<[UnguardedData]>,
    exports: HashMap<Arc<str>, UnguardedExternVal>,
}

/// An initializer for an [`Instance`].
#[derive(Debug)]
pub(crate) struct InstanceIniter {
    store_id: StoreId,
    types: Vec<UnguardedInternedFuncType>,
    funcs: Vec<UnguardedFunc>,
    tables: Vec<UnguardedTable>,
    mems: Vec<UnguardedMem>,
    globals: Vec<UnguardedGlobal>,
    elems: Vec<UnguardedElem>,
    datas: Vec<UnguardedData>,
    exports: HashMap<Arc<str>, UnguardedExternVal>,
}

impl InstanceIniter {
    /// Creates a new [`InstanceIniter`].
    pub(crate) fn new(store_id: StoreId) -> InstanceIniter {
        InstanceIniter {
            types: Vec::new(),
            funcs: Vec::new(),
            tables: Vec::new(),
            mems: Vec::new(),
            globals: Vec::new(),
            elems: Vec::new(),
            datas: Vec::new(),
            exports: HashMap::new(),
            store_id,
        }
    }

    /// Returns the [`Func`] at the given index in this [`InstanceIniter`], if it exists.
    pub(crate) fn func(&self, idx: u32) -> Option<Func> {
        self.unguarded_func(idx)
            .map(|func| unsafe { Func::from_unguarded(func, self.store_id) })
    }

    /// An unguarded version of [`InstanceIniter::func`].
    fn unguarded_func(&self, idx: u32) -> Option<UnguardedFunc> {
        self.funcs.get(idx as usize).copied()
    }

    /// Returns the [`Table`] at the given index in this [`InstanceIniter`], if it exists.
    pub(crate) fn table(&self, idx: u32) -> Option<Table> {
        self.unguarded_table(idx)
            .map(|table| unsafe { Table::from_unguarded(table, self.store_id) })
    }

    /// An unguarded version of [`InstanceIniter::table`].
    fn unguarded_table(&self, idx: u32) -> Option<UnguardedTable> {
        self.tables.get(idx as usize).copied()
    }

    /// Returns the [`Mem`] at the given index in this [`InstanceIniter`], if it exists.
    pub(crate) fn mem(&self, idx: u32) -> Option<Mem> {
        self.unguarded_mem(idx)
            .map(|mem| unsafe { Mem::from_unguarded(mem, self.store_id) })
    }

    /// An unguarded version of [`InstanceIniter::mem`].
    fn unguarded_mem(&self, idx: u32) -> Option<UnguardedMem> {
        self.mems.get(idx as usize).copied()
    }

    /// Returns the [`Global`] at the given index in this [`InstanceIniter`], if it exists.
    pub(crate) fn global(&self, idx: u32) -> Option<Global> {
        self.unguarded_global(idx)
            .map(|global| unsafe { Global::from_unguarded(global, self.store_id) })
    }

    fn unguarded_global(&self, idx: u32) -> Option<UnguardedGlobal> {
        self.globals.get(idx as usize).copied()
    }

    /// Appends the given [`InternedFuncType`] to this [`InstanceIniter`].
    pub(crate) fn push_type(&mut self, type_: InternedFuncType) {
        self.types.push(type_.to_unguarded(self.store_id));
    }

    /// Appends the given [`Func`] to this [`InstanceIniter`].
    pub(crate) fn push_func(&mut self, func: Func) {
        self.funcs.push(func.to_unguarded(self.store_id));
    }

    /// Appends the given [`Table`] to this [`InstanceIniter`].
    pub(crate) fn push_table(&mut self, table: Table) {
        self.tables.push(table.to_unguarded(self.store_id));
    }

    /// Appends the given [`Mem`] to this [`InstanceIniter`].
    pub(crate) fn push_mem(&mut self, mem: Mem) {
        self.mems.push(mem.to_unguarded(self.store_id));
    }

    /// Appends the given [`Global`] to this [`InstanceIniter`].
    pub(crate) fn push_global(&mut self, global: Global) {
        self.globals.push(global.to_unguarded(self.store_id));
    }

    /// Appends the given [`Elem`] to this [`InstanceIniter`].
    pub(crate) fn push_elem(&mut self, elem: Elem) {
        self.elems.push(elem.to_unguarded(self.store_id));
    }

    /// Appends the given [`Data`] to this [`InstanceIniter`].
    pub(crate) fn push_data(&mut self, data: Data) {
        self.datas.push(data.to_unguarded(self.store_id));
    }

    /// Appends an export with the given name and [`ExternVal`] to this [`InstanceIniter`].
    pub(crate) fn push_export(&mut self, name: Arc<str>, val: ExternVal) {
        self.exports.insert(name, val.to_unguarded(self.store_id));
    }
}

impl EvaluationContext for InstanceIniter {
    fn func(&self, idx: u32) -> Option<Func> {
        self.func(idx)
    }

    fn global(&self, idx: u32) -> Option<Global> {
        self.global(idx)
    }
}
