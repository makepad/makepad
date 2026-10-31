use {
    crate::{
        binary,
        code::{self, UncompiledCode},
        config,
        const_expr::ConstExpr,
        data::Data,
        decode::DecodeError,
        elem::{Elem, UnguardedElems},
        engine::Engine,
        error::Error,
        extern_val::{ExternType, ExternVal, ExternValDesc},
        func::{Func, FuncType},
        global::{Global, GlobalType, Mut},
        instance::{Instance, InstanceIniter},
        linker::{InstantiateError, Linker},
        mem::{Mem, MemType},
        ref_::{Ref, RefType},
        store::Store,
        table::{Table, TableType},
        trap::Trap,
        val::ValType,
    },
    std::{
        collections::{hash_map, HashMap},
        slice,
        sync::Arc,
    },
};

/// A Wasm module.
#[derive(Debug)]
pub struct Module {
    types: Arc<[FuncType]>,
    imports: Box<[((Arc<str>, Arc<str>), ImportKind)]>,
    imported_func_count: usize,
    imported_table_count: usize,
    imported_memory_count: usize,
    imported_global_count: usize,
    func_types: Box<[FuncType]>,
    table_types: Box<[TableType]>,
    memory_types: Box<[MemType]>,
    global_types: Box<[GlobalType]>,
    global_vals: Box<[ConstExpr]>,
    exports: HashMap<Arc<str>, ExternValDesc>,
    start: Option<u32>,
    codes: Box<[UncompiledCode]>,
    elems: Box<[ElemDef]>,
    datas: Box<[DataDef]>,
}

impl Module {
    /// Decodes and validates a new [`Module`] from the given byte slice.
    ///
    /// # Errors
    ///
    /// - If the [`Module`] is malformed.
    /// - If the [`Module`] is invalid.
    /// - If the [`Module`] uses features the engine does not run.
    pub fn new(engine: &Engine, bytes: &[u8]) -> Result<Module, DecodeError> {
        let module = binary::Module::decode_with(
            bytes,
            binary::Features {
                multi_memory: false,
                ext_math: engine.extensions().ext_math,
            },
        )?;
        binary::validate(&module)?;
        ModuleBuilder::build(module)
    }

    /// Returns an iterator over the imports in this [`Module`].
    pub fn imports(&self) -> ModuleImports<'_> {
        ModuleImports {
            imports: self.imports.iter(),
            imported_func_types: self.func_types[..self.imported_func_count].iter(),
            imported_table_types: self.table_types[..self.imported_table_count].iter(),
            imported_memory_types: self.memory_types[..self.imported_memory_count].iter(),
            imported_global_types: self.global_types[..self.imported_global_count].iter(),
        }
    }

    /// Returns the [`ExternType`] of the export with the given name in this [`Module`], if it exists.
    pub fn export<'a>(&'a self, name: &'a str) -> Option<ExternType> {
        self.exports.get(name).map(|&desc| self.extern_type(desc))
    }

    /// Returns an iterator over the exports in this [`Module`].
    pub fn exports(&self) -> ModuleExports<'_> {
        ModuleExports {
            module: self,
            iter: self.exports.iter(),
        }
    }

    pub(crate) fn instantiate(
        &self,
        store: &mut Store,
        linker: &Linker,
    ) -> Result<Instance, Error> {
        let instance = Instance::uninited(store.id());
        let mut initer = InstanceIniter::new(store.id());
        for type_ in self.types.iter() {
            initer.push_type(store.get_or_intern_type(type_));
        }
        for ((module, name), type_) in self.imports() {
            match type_ {
                ExternType::Func(type_) => {
                    let val = linker
                        .lookup(module, name)
                        .ok_or(InstantiateError::DefNotFound)?;
                    let func = val.to_func().ok_or(InstantiateError::ImportKindMismatch)?;
                    if func.type_(store) != &type_ {
                        return Err(InstantiateError::FuncTypeMismatch)?;
                    }
                    initer.push_func(func);
                }
                ExternType::Global(type_) => {
                    let val = linker
                        .lookup(module, name)
                        .ok_or(InstantiateError::DefNotFound)?;
                    let global = val
                        .to_global()
                        .ok_or(InstantiateError::ImportKindMismatch)?;
                    if global.type_(store) != type_ {
                        return Err(InstantiateError::GlobalTypeMismatch)?;
                    }
                    initer.push_global(global);
                }
                _ => {}
            }
        }
        for (type_, code) in self.internal_funcs() {
            let type_ = store.get_or_intern_type(type_);
            initer.push_func(Func::new_wasm(store, type_, instance.clone(), code.clone()));
        }
        let global_init_vals: Vec<_> = self
            .internal_globals()
            .map(|(_, val)| val.evaluate(store, &initer))
            .collect();
        let elems: Vec<UnguardedElems> = self
            .elems
            .iter()
            .map(|elem| match elem.type_ {
                RefType::FuncRef => UnguardedElems::FuncRef(
                    elem.elems
                        .iter()
                        .map(|elem| {
                            elem.evaluate(store, &initer)
                                .to_func_ref()
                                .unwrap()
                                .to_unguarded(store.id())
                        })
                        .collect(),
                ),
                RefType::ExternRef => UnguardedElems::ExternRef(
                    elem.elems
                        .iter()
                        .map(|elem| {
                            elem.evaluate(store, &initer)
                                .to_extern_ref()
                                .unwrap()
                                .to_unguarded(store.id())
                        })
                        .collect(),
                ),
            })
            .collect();
        for ((module, name), type_) in self.imports() {
            match type_ {
                ExternType::Table(type_) => {
                    let val = linker
                        .lookup(module, name)
                        .ok_or(InstantiateError::DefNotFound)?;
                    let table = val.to_table().ok_or(InstantiateError::ImportKindMismatch)?;
                    if !table.type_(store).is_subtype_of(type_) {
                        return Err(InstantiateError::TableTypeMismatch)?;
                    }
                    initer.push_table(table);
                }
                ExternType::Mem(type_) => {
                    let val = linker
                        .lookup(module, name)
                        .ok_or(InstantiateError::DefNotFound)?;
                    let mem = val.to_mem().ok_or(InstantiateError::ImportKindMismatch)?;
                    if !mem.type_(store).is_subtype_of(type_) {
                        return Err(InstantiateError::MemTypeMismatch)?;
                    }
                    initer.push_mem(mem);
                }
                _ => {}
            }
        }
        for type_ in self.internal_tables() {
            initer.push_table(Table::new(store, type_, Ref::null(type_.elem)).unwrap());
        }
        for type_ in self.internal_memories() {
            initer.push_mem(Mem::new(store, type_));
        }
        for ((type_, _), init_val) in self.internal_globals().zip(global_init_vals) {
            initer.push_global(Global::new(store, type_, init_val).unwrap());
        }
        for (name, &desc) in self.exports.iter() {
            initer.push_export(
                name.clone(),
                match desc {
                    ExternValDesc::Func(idx) => ExternVal::Func(initer.func(idx).unwrap()),
                    ExternValDesc::Table(idx) => ExternVal::Table(initer.table(idx).unwrap()),
                    ExternValDesc::Memory(idx) => ExternVal::Memory(initer.mem(idx).unwrap()),
                    ExternValDesc::Global(idx) => ExternVal::Global(initer.global(idx).unwrap()),
                },
            );
        }
        for elems in elems {
            initer.push_elem(unsafe { Elem::new_unguarded(store, elems) });
        }
        for data in self.datas.iter() {
            initer.push_data(Data::new(store, data.bytes.clone()));
        }
        instance.init(initer);
        for (elem_idx, elem) in (0u32..).zip(self.elems.iter()) {
            let ElemKind::Active {
                table_idx,
                ref offset,
            } = elem.kind
            else {
                continue;
            };
            instance.table(table_idx).unwrap().init(
                store,
                offset.evaluate(store, &instance).to_i32().unwrap() as u32,
                instance.elem(elem_idx).unwrap(),
                0,
                elem.elems.len().try_into().unwrap(),
            )?;
            instance.elem(elem_idx).unwrap().drop_elems(store);
        }
        for (elem_idx, elem) in (0u32..).zip(self.elems.iter()) {
            let ElemKind::Declarative = elem.kind else {
                continue;
            };
            instance.elem(elem_idx).unwrap().drop_elems(store);
        }
        for (data_idx, data) in (0u32..).zip(self.datas.iter()) {
            let DataKind::Active {
                mem_idx,
                ref offset,
            } = data.kind
            else {
                continue;
            };
            instance.mem(mem_idx).unwrap().init(
                store,
                offset
                    .evaluate(store, &instance)
                    .to_i32()
                    .ok_or(Trap::Unreachable)? as u32,
                instance.data(data_idx).unwrap(),
                0,
                data.bytes.len().try_into().unwrap(),
            )?;
            instance.data(data_idx).unwrap().drop_bytes(store);
        }
        if let Some(start) = self.start {
            instance.func(start).unwrap().call(store, &[], &mut [])?;
        }
        Ok(instance)
    }

    fn func(&self, idx: u32) -> Option<&FuncType> {
        let idx = usize::try_from(idx).unwrap();
        self.func_types.get(idx)
    }

    fn table(&self, idx: u32) -> Option<TableType> {
        let idx = usize::try_from(idx).unwrap();
        self.table_types.get(idx).copied()
    }

    fn memory(&self, idx: u32) -> Option<MemType> {
        let idx = usize::try_from(idx).unwrap();
        self.memory_types.get(idx).copied()
    }

    fn global(&self, idx: u32) -> Option<GlobalType> {
        let idx = usize::try_from(idx).unwrap();
        self.global_types.get(idx).copied()
    }

    fn extern_type(&self, desc: ExternValDesc) -> ExternType {
        match desc {
            ExternValDesc::Func(idx) => self.func(idx).cloned().unwrap().into(),
            ExternValDesc::Table(idx) => self.table(idx).unwrap().into(),
            ExternValDesc::Memory(idx) => self.memory(idx).unwrap().into(),
            ExternValDesc::Global(idx) => self.global(idx).unwrap().into(),
        }
    }

    fn internal_funcs(&self) -> impl Iterator<Item = (&FuncType, &UncompiledCode)> {
        self.func_types[self.imported_func_count..]
            .iter()
            .zip(self.codes.iter())
    }

    fn internal_tables(&self) -> impl Iterator<Item = TableType> + '_ {
        self.table_types[self.imported_table_count..]
            .iter()
            .copied()
    }

    fn internal_memories(&self) -> impl Iterator<Item = MemType> + '_ {
        self.memory_types[self.imported_memory_count..]
            .iter()
            .copied()
    }

    fn internal_globals(&self) -> impl Iterator<Item = (GlobalType, &ConstExpr)> {
        self.global_types[self.imported_global_count..]
            .iter()
            .copied()
            .zip(self.global_vals.iter())
    }
}

/// An iterator over the imports in a [`Module`].
#[derive(Clone, Debug)]
pub struct ModuleImports<'a> {
    imports: slice::Iter<'a, ((Arc<str>, Arc<str>), ImportKind)>,
    imported_func_types: slice::Iter<'a, FuncType>,
    imported_table_types: slice::Iter<'a, TableType>,
    imported_memory_types: slice::Iter<'a, MemType>,
    imported_global_types: slice::Iter<'a, GlobalType>,
}

impl<'a> Iterator for ModuleImports<'a> {
    type Item = ((&'a str, &'a str), ExternType);

    fn next(&mut self) -> Option<Self::Item> {
        self.imports.next().map(|((module, name), imported)| {
            (
                (&**module, &**name),
                match imported {
                    ImportKind::Func => self.imported_func_types.next().cloned().unwrap().into(),
                    ImportKind::Table => self.imported_table_types.next().copied().unwrap().into(),
                    ImportKind::Mem => self.imported_memory_types.next().copied().unwrap().into(),
                    ImportKind::Global => {
                        self.imported_global_types.next().copied().unwrap().into()
                    }
                },
            )
        })
    }
}

/// An iterator over the exports in a [`Module`].
#[derive(Clone, Debug)]
pub struct ModuleExports<'a> {
    module: &'a Module,
    iter: hash_map::Iter<'a, Arc<str>, ExternValDesc>,
}

impl<'a> Iterator for ModuleExports<'a> {
    type Item = (&'a str, ExternType);

    fn next(&mut self) -> Option<Self::Item> {
        self.iter
            .next()
            .map(|(name, &desc)| (&**name, self.module.extern_type(desc)))
    }
}

/// Builds the engine's [`Module`] from a decoded, validated module,
/// refusing what the engine does not run and modules beyond its limits.
#[derive(Debug)]
struct ModuleBuilder {
    types: Vec<FuncType>,
    imports: Vec<((Arc<str>, Arc<str>), ImportKind)>,
    imported_func_count: usize,
    imported_table_count: usize,
    imported_memory_count: usize,
    imported_global_count: usize,
    func_types: Vec<FuncType>,
    table_types: Vec<TableType>,
    memory_types: Vec<MemType>,
    global_types: Vec<GlobalType>,
    global_vals: Vec<ConstExpr>,
    exports: HashMap<Arc<str>, ExternValDesc>,
    codes: Vec<UncompiledCode>,
    elems: Vec<ElemDef>,
    datas: Vec<DataDef>,
}

fn table_type(type_: binary::TableType) -> TableType {
    TableType {
        limits: type_.limits,
        elem: type_.elem.to_ref().unwrap(),
    }
}

fn mem_type(type_: binary::MemType) -> Result<MemType, DecodeError> {
    if type_.shared {
        return Err(DecodeError::new("shared memories are not supported"));
    }
    Ok(MemType {
        limits: type_.limits,
    })
}

fn global_type(type_: binary::GlobalType) -> Result<GlobalType, DecodeError> {
    // v128 globals are not supported: a v128 value cannot flow through
    // the global entity storage or the global.get/set register paths.
    if type_.ty == ValType::V128 {
        return Err(DecodeError::new("v128 globals are not supported"));
    }
    Ok(GlobalType {
        val: type_.ty,
        mut_: if type_.mutable { Mut::Var } else { Mut::Const },
    })
}

impl ModuleBuilder {
    fn build(module: binary::Module) -> Result<Module, DecodeError> {
        let mut builder = ModuleBuilder {
            types: Vec::new(),
            imports: Vec::new(),
            imported_func_count: 0,
            imported_table_count: 0,
            imported_memory_count: 0,
            imported_global_count: 0,
            func_types: Vec::new(),
            table_types: Vec::new(),
            memory_types: Vec::new(),
            global_types: Vec::new(),
            global_vals: Vec::new(),
            exports: HashMap::new(),
            codes: Vec::new(),
            elems: Vec::new(),
            datas: Vec::new(),
        };
        if module.types.len() > config::MAX_TYPE_COUNT {
            return Err(DecodeError::new("too many types"));
        }
        for type_ in &module.types {
            builder.types.push(FuncType::new(
                type_.params.iter().copied(),
                type_.results.iter().copied(),
            ));
        }
        if module.imports.len() > config::MAX_IMPORT_COUNT {
            return Err(DecodeError::new("too many imports"));
        }
        for import in &module.imports {
            let key = (import.module.as_str().into(), import.name.as_str().into());
            match import.desc {
                binary::ImportDesc::Func(type_idx) => {
                    builder.imports.push((key, ImportKind::Func));
                    builder.imported_func_count += 1;
                    builder.push_func(type_idx)?;
                }
                binary::ImportDesc::Table(type_) => {
                    builder.imports.push((key, ImportKind::Table));
                    builder.imported_table_count += 1;
                    builder.push_table(table_type(type_))?;
                }
                binary::ImportDesc::Memory(type_) => {
                    builder.imports.push((key, ImportKind::Mem));
                    builder.imported_memory_count += 1;
                    builder.push_memory(mem_type(type_)?)?;
                }
                binary::ImportDesc::Global(type_) => {
                    builder.imports.push((key, ImportKind::Global));
                    builder.imported_global_count += 1;
                    builder.push_global_type(global_type(type_)?)?;
                }
            }
        }
        for func in &module.funcs {
            builder.push_func(func.ty)?;
        }
        for type_ in &module.tables {
            builder.push_table(table_type(*type_))?;
        }
        for type_ in &module.memories {
            builder.push_memory(mem_type(*type_)?)?;
        }
        for global in &module.globals {
            builder.push_global_type(global_type(global.ty)?)?;
            builder.global_vals.push(ConstExpr::from_expr(&global.init)?);
        }
        if module.exports.len() > config::MAX_EXPORT_COUNT {
            return Err(DecodeError::new("too many exports"));
        }
        for export in &module.exports {
            let desc = match export.kind {
                binary::ExternKind::Func => ExternValDesc::Func(export.index),
                binary::ExternKind::Table => ExternValDesc::Table(export.index),
                binary::ExternKind::Memory => ExternValDesc::Memory(export.index),
                binary::ExternKind::Global => ExternValDesc::Global(export.index),
            };
            builder.exports.insert(export.name.as_str().into(), desc);
        }
        if module.elems.len() > config::MAX_ELEM_COUNT {
            return Err(DecodeError::new("too many element segments"));
        }
        for elem in &module.elems {
            if elem.items.len() > config::MAX_ELEM_SIZE {
                return Err(DecodeError::new("element segment too large"));
            }
            builder.elems.push(ElemDef {
                kind: match &elem.mode {
                    binary::ElemMode::Passive => ElemKind::Passive,
                    binary::ElemMode::Declarative => ElemKind::Declarative,
                    binary::ElemMode::Active { table, offset } => ElemKind::Active {
                        table_idx: *table,
                        offset: ConstExpr::from_expr(offset)?,
                    },
                },
                type_: elem.items.elem_type().to_ref().unwrap(),
                elems: match &elem.items {
                    binary::ElemItems::Funcs(funcs) => funcs
                        .iter()
                        .map(|func_idx| ConstExpr::new_ref_func(*func_idx))
                        .collect(),
                    binary::ElemItems::Exprs(_, exprs) => exprs
                        .iter()
                        .map(|expr| ConstExpr::from_expr(expr))
                        .collect::<Result<_, _>>()?,
                },
            });
        }
        for func in module.funcs {
            if func.locals.len() > config::MAX_FUNC_LOCAL_COUNT {
                return Err(DecodeError::new("too many function locals"));
            }
            if func.body.len() > config::MAX_FUNC_BODY_SIZE {
                return Err(DecodeError::new("function body too large"));
            }
            for instr in &func.body {
                code::check_supported(instr)?;
            }
            builder.codes.push(UncompiledCode {
                locals: func.locals.into(),
                body: func.body.into(),
            });
        }
        if module.datas.len() > config::MAX_DATA_COUNT {
            return Err(DecodeError::new("too many data segments"));
        }
        for data in module.datas {
            if data.bytes.len() > config::MAX_DATA_SIZE {
                return Err(DecodeError::new("data segment too large"));
            }
            builder.datas.push(DataDef {
                kind: match &data.mode {
                    binary::DataMode::Passive => DataKind::Passive,
                    binary::DataMode::Active { memory, offset } => DataKind::Active {
                        mem_idx: *memory,
                        offset: ConstExpr::from_expr(offset)?,
                    },
                },
                bytes: data.bytes.into(),
            });
        }
        Ok(Module {
            types: builder.types.into(),
            imports: builder.imports.into(),
            imported_func_count: builder.imported_func_count,
            imported_table_count: builder.imported_table_count,
            imported_memory_count: builder.imported_memory_count,
            imported_global_count: builder.imported_global_count,
            func_types: builder.func_types.into(),
            table_types: builder.table_types.into(),
            memory_types: builder.memory_types.into(),
            global_types: builder.global_types.into(),
            global_vals: builder.global_vals.into(),
            exports: builder.exports,
            start: module.start,
            codes: builder.codes.into(),
            elems: builder.elems.into(),
            datas: builder.datas.into(),
        })
    }

    fn push_func(&mut self, type_idx: u32) -> Result<(), DecodeError> {
        if self.func_types.len() == config::MAX_FUNC_COUNT {
            return Err(DecodeError::new("too many functions"));
        }
        let type_ = self.types[type_idx as usize].clone();
        if type_.params().len() > config::MAX_FUNC_PARAM_COUNT {
            return Err(DecodeError::new("too many function parameters"));
        }
        if type_.results().len() > config::MAX_FUNC_RESULT_COUNT {
            return Err(DecodeError::new("too many function results"));
        }
        self.func_types.push(type_);
        Ok(())
    }

    fn push_table(&mut self, type_: TableType) -> Result<(), DecodeError> {
        if self.table_types.len() == config::MAX_TABLE_COUNT {
            return Err(DecodeError::new("too many tables"));
        }
        self.table_types.push(type_);
        Ok(())
    }

    fn push_memory(&mut self, type_: MemType) -> Result<(), DecodeError> {
        if self.memory_types.len() == config::MAX_MEMORY_COUNT {
            return Err(DecodeError::new("too many memories"));
        }
        self.memory_types.push(type_);
        Ok(())
    }

    fn push_global_type(&mut self, type_: GlobalType) -> Result<(), DecodeError> {
        if self.global_types.len() == config::MAX_GLOBAL_COUNT {
            return Err(DecodeError::new("too many globals"));
        }
        self.global_types.push(type_);
        Ok(())
    }
}

/// The kind of an import
#[derive(Clone, Copy, Debug)]
enum ImportKind {
    Func,
    Table,
    Mem,
    Global,
}

/// A definition for an [`Elem`].
#[derive(Clone, Debug)]
struct ElemDef {
    kind: ElemKind,
    type_: RefType,
    elems: Arc<[ConstExpr]>,
}

/// The kind of an [`Elem`].
#[derive(Clone, Debug)]
enum ElemKind {
    /// Passive [`Elem`]s are used to initialize [`Table`]s during execution.
    Passive,
    /// Active [`Elem`]s are used to initialize [`Table`]s during instantiation.
    Active { table_idx: u32, offset: ConstExpr },
    /// Declarative [`Elem`]s are only used during validation.
    Declarative,
}

/// A definition for a [`Data`].
#[derive(Clone, Debug)]
pub(crate) struct DataDef {
    kind: DataKind,
    bytes: Arc<[u8]>,
}

/// The kind of a [`Data`].
#[derive(Clone, Debug)]
enum DataKind {
    /// Passive [`Data`]s are used to initialize a [`Mem`] during execution.
    Passive,
    /// Active [`Data`]s are used to initialize a [`Mem`] during instantiation.
    Active { mem_idx: u32, offset: ConstExpr },
}
