use {
    crate::{
        code::UncompiledCode,
        config,
        const_expr::ConstExpr,
        data::Data,
        decode::{Decode, DecodeError, Decoder},
        elem::{Elem, UnguardedElems},
        engine::Engine,
        error::Error,
        extern_val::{ExternType, ExternTypeDesc, ExternVal, ExternValDesc},
        func::{Func, FuncType},
        global::{Global, GlobalType},
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
        collections::{hash_map, HashMap, HashSet},
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
    pub fn new(engine: &Engine, bytes: &[u8]) -> Result<Module, DecodeError> {
        const MAGIC: [u8; 4] = [0x00, 0x61, 0x73, 0x6D];
        const VERSION: [u8; 4] = [0x01, 0x00, 0x00, 0x00];
        const EXPECTED_SECTION_IDS: &[u8] = &[1, 2, 3, 4, 5, 6, 7, 8, 9, 12, 10, 11];

        let mut decoder = Decoder::new(bytes);
        let magic = decoder.read_bytes(4)?;
        if magic != MAGIC {
            return Err(DecodeError::new(""))?;
        }
        let version = decoder.read_bytes(4)?;
        if version != VERSION {
            return Err(DecodeError::new(""))?;
        }
        let mut builder = ModuleBuilder::new();
        let mut expected_section_ids = EXPECTED_SECTION_IDS.iter().copied();
        while !decoder.is_at_end() {
            let section_id = decoder.read_byte()?;
            if section_id != 0 {
                if !expected_section_ids
                    .any(|expected_section_id| expected_section_id == section_id)
                {
                    return Err(DecodeError::new("section id mismatch"))?;
                }
            }
            let mut section_decoder = decoder.decode_decoder()?;
            match section_id {
                // Custom section
                0 => {
                    section_decoder.decode_string()?;
                    section_decoder.read_bytes_until_end();
                }
                // Type section
                1 => {
                    for type_ in section_decoder.decode_iter()? {
                        builder.push_type(type_?)?;
                    }
                }
                // Import section
                2 => {
                    for import in section_decoder.decode_iter()? {
                        builder.push_import(import?)?;
                    }
                }
                // Function section
                3 => {
                    for type_idx in section_decoder.decode_iter()? {
                        builder.push_func(type_idx?)?;
                    }
                }
                // Table section
                4 => {
                    for table in section_decoder.decode_iter()? {
                        builder.push_table(table?)?;
                    }
                }
                // Memory section
                5 => {
                    for mem in section_decoder.decode_iter()? {
                        builder.push_memory(mem?)?;
                    }
                }
                // Global section
                6 => {
                    for global in section_decoder.decode_iter()? {
                        builder.push_global(global?)?;
                    }
                }
                // Export section
                7 => {
                    for export in section_decoder.decode_iter()? {
                        builder.push_export(export?)?;
                    }
                }
                // Start section
                8 => {
                    builder.set_start(section_decoder.decode()?)?;
                }
                // Element section
                9 => {
                    for elem in section_decoder.decode_iter()? {
                        builder.push_elem(elem?)?;
                    }
                }
                // Code section
                10 => {
                    for code in section_decoder.decode_iter()? {
                        builder.push_code(code?)?;
                    }
                }
                // Data section
                11 => {
                    for data in section_decoder.decode_iter()? {
                        builder.push_data(data?)?;
                    }
                }
                // Data count section
                12 => {
                    let data_count = section_decoder.decode()?;
                    builder.set_data_count(data_count);
                }
                _ => unreachable!(),
            }
            if !section_decoder.is_at_end() {
                return Err(DecodeError::new(""))?;
            }
        }
        builder.finish(engine)
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

/// A builder for a [`Module`].
#[derive(Debug)]
pub(crate) struct ModuleBuilder {
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
    start: Option<u32>,
    codes: Vec<UncompiledCode>,
    elems: Vec<ElemDef>,
    datas: Vec<DataDef>,
    data_count: Option<u32>,
    refs: HashSet<u32>,
}

impl ModuleBuilder {
    fn new() -> Self {
        Self {
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
            start: None,
            codes: Vec::new(),
            elems: Vec::new(),
            datas: Vec::new(),
            data_count: None,
            refs: HashSet::new(),
        }
    }

    pub(crate) fn type_(&self, idx: u32) -> Result<&FuncType, DecodeError> {
        let idx = usize::try_from(idx).unwrap();
        self.types
            .get(idx)
            .ok_or_else(|| DecodeError::new("unknown type"))
    }

    pub(crate) fn func(&self, idx: u32) -> Result<&FuncType, DecodeError> {
        let idx = usize::try_from(idx).unwrap();
        self.func_types
            .get(idx)
            .ok_or_else(|| DecodeError::new("unknown function"))
    }

    pub(crate) fn table(&self, idx: u32) -> Result<TableType, DecodeError> {
        let idx = usize::try_from(idx).unwrap();
        self.table_types
            .get(idx)
            .copied()
            .ok_or_else(|| DecodeError::new("unknown table"))
    }

    pub(crate) fn memory(&self, idx: u32) -> Result<MemType, DecodeError> {
        let idx = usize::try_from(idx).unwrap();
        self.memory_types
            .get(idx)
            .copied()
            .ok_or_else(|| DecodeError::new("unknown memory"))
    }

    pub(crate) fn imported_global(&self, idx: u32) -> Result<GlobalType, DecodeError> {
        let idx = usize::try_from(idx).unwrap();
        self.global_types[..self.imported_global_count]
            .get(idx)
            .copied()
            .ok_or_else(|| DecodeError::new("unknown global"))
    }

    pub(crate) fn global(&self, idx: u32) -> Result<GlobalType, DecodeError> {
        let idx = usize::try_from(idx).unwrap();
        self.global_types
            .get(idx)
            .copied()
            .ok_or_else(|| DecodeError::new("unknown global"))
    }

    pub(crate) fn elem(&self, idx: u32) -> Result<RefType, DecodeError> {
        let idx = usize::try_from(idx).unwrap();
        self.elems
            .get(idx)
            .map(|elem| elem.type_)
            .ok_or_else(|| DecodeError::new("unknown element segment"))
    }

    pub(crate) fn data(&self, idx: u32) -> Result<(), DecodeError> {
        if let Some(data_count) = self.data_count {
            if idx >= data_count {
                return Err(DecodeError::new("unknown data segment"));
            }
            Ok(())
        } else {
            Err(DecodeError::new("missing data count section"))
        }
    }

    pub(crate) fn ref_(&self, func_idx: u32) -> Result<(), DecodeError> {
        if !self.refs.contains(&func_idx) {
            return Err(DecodeError::new("undeclared reference"));
        }
        Ok(())
    }

    fn push_type(&mut self, type_: FuncType) -> Result<(), DecodeError> {
        if self.types.len() == config::MAX_TYPE_COUNT {
            return Err(DecodeError::new("too many types"));
        }
        self.types.push(type_);
        Ok(())
    }

    fn push_import(&mut self, import: ImportDef) -> Result<(), DecodeError> {
        if self.imports.len() == config::MAX_IMPORT_COUNT {
            return Err(DecodeError::new("too many imports"));
        }
        let key = (import.module, import.name);
        match import.desc {
            ExternTypeDesc::Func(type_idx) => {
                if self.func_types.len() == config::MAX_FUNC_COUNT {
                    return Err(DecodeError::new("too many functions"));
                }
                self.imports.push((key, ImportKind::Func));
                self.imported_func_count += 1;
                self.func_types.push(self.type_(type_idx).cloned()?);
            }
            ExternTypeDesc::Table(type_) => {
                if self.table_types.len() == config::MAX_TABLE_COUNT {
                    return Err(DecodeError::new("too many tables"));
                }
                if !type_.is_valid() {
                    return Err(DecodeError::new("invalid table type"))?;
                }
                self.imports.push((key, ImportKind::Table));
                self.imported_table_count += 1;
                self.table_types.push(type_);
            }
            ExternTypeDesc::Memory(type_) => {
                if self.memory_types.len() == config::MAX_MEMORY_COUNT {
                    return Err(DecodeError::new("too many memories"));
                }
                if !type_.is_valid() {
                    return Err(DecodeError::new("invalid memory type"));
                }
                self.imports.push((key, ImportKind::Mem));
                self.imported_memory_count += 1;
                self.memory_types.push(type_);
            }
            ExternTypeDesc::Global(type_) => {
                if self.global_types.len() == config::MAX_GLOBAL_COUNT {
                    return Err(DecodeError::new("too many globals"));
                }
                self.imports.push((key, ImportKind::Global));
                self.imported_global_count += 1;
                self.global_types.push(type_);
            }
        }
        Ok(())
    }

    fn push_func(&mut self, type_idx: u32) -> Result<(), DecodeError> {
        if self.func_types.len() == config::MAX_FUNC_COUNT {
            return Err(DecodeError::new("too many functions"));
        }
        let type_ = self.type_(type_idx).cloned()?;
        if type_.params().len() > config::MAX_FUNC_PARAM_COUNT {
            return Err(DecodeError::new("too many function parameters"));
        }
        if type_.results().len() > config::MAX_FUNC_RESULT_COUNT {
            return Err(DecodeError::new("too many function results"));
        }
        self.func_types.push(type_);
        Ok(())
    }

    fn push_table(&mut self, table: TableDef) -> Result<(), DecodeError> {
        if self.table_types.len() == config::MAX_TABLE_COUNT {
            return Err(DecodeError::new("too many tables"));
        }
        if !table.type_.is_valid() {
            return Err(DecodeError::new("invalid table type"))?;
        }
        self.table_types.push(table.type_);
        Ok(())
    }

    fn push_memory(&mut self, memory: MemDef) -> Result<(), DecodeError> {
        if self.memory_types.len() == config::MAX_MEMORY_COUNT {
            return Err(DecodeError::new("too many memories"));
        }
        if !memory.type_.is_valid() {
            return Err(DecodeError::new("invalid memory type"));
        }
        self.memory_types.push(memory.type_);
        Ok(())
    }

    fn push_global(&mut self, global: GlobalDef) -> Result<(), DecodeError> {
        if self.global_types.len() == config::MAX_GLOBAL_COUNT {
            return Err(DecodeError::new("too many globals"));
        }
        if global.val.validate(self)? != global.type_.val {
            return Err(DecodeError::new("type mismatch"));
        }
        if let Some(func_idx) = global.val.func_idx() {
            self.refs.insert(func_idx);
        }
        self.global_types.push(global.type_);
        self.global_vals.push(global.val);
        Ok(())
    }

    fn push_export(&mut self, export: ExportDef) -> Result<(), DecodeError> {
        if self.exports.len() == config::MAX_EXPORT_COUNT {
            return Err(DecodeError::new("too many exports"));
        }
        match export.desc {
            ExternValDesc::Func(idx) => {
                self.refs.insert(idx);
                self.func(idx)?;
            }
            ExternValDesc::Table(idx) => {
                self.table(idx)?;
            }
            ExternValDesc::Memory(idx) => {
                self.memory(idx)?;
            }
            ExternValDesc::Global(idx) => {
                self.global(idx)?;
            }
        }
        if self.exports.contains_key(&export.name) {
            return Err(DecodeError::new("duplicate export name"));
        }
        self.exports.insert(export.name, export.desc);
        Ok(())
    }

    fn set_start(&mut self, start: u32) -> Result<(), DecodeError> {
        let type_ = self.func(start)?;
        if type_ != &FuncType::from_val_type(None) {
            return Err(DecodeError::new("type mismatch"));
        }
        self.start = Some(start);
        Ok(())
    }

    fn push_code(&mut self, code: UncompiledCode) -> Result<(), DecodeError> {
        if self.codes.len() == self.func_types.len() - self.imported_func_count {
            return Err(DecodeError::new(
                "function and code section have inconsistent sizes",
            ))?;
        }
        if code.locals.len() > config::MAX_FUNC_LOCAL_COUNT {
            return Err(DecodeError::new("too many function locals"));
        }
        if code.expr.len() > config::MAX_FUNC_BODY_SIZE {
            return Err(DecodeError::new("function body too large"));
        }
        self.codes.push(code);
        Ok(())
    }

    fn push_elem(&mut self, elem: ElemDef) -> Result<(), DecodeError> {
        if self.elems.len() == config::MAX_ELEM_COUNT {
            return Err(DecodeError::new("too many element segments"));
        }
        if elem.elems.len() > config::MAX_ELEM_SIZE {
            return Err(DecodeError::new("element segment too large"));
        }
        if let ElemKind::Active {
            table_idx,
            ref offset,
        } = elem.kind
        {
            let table = self.table(table_idx)?;
            if elem.type_ != table.elem {
                return Err(DecodeError::new("type mismatch"));
            }
            if offset.validate(self)? != ValType::I32 {
                return Err(DecodeError::new("type mismatch"));
            }
        }
        for expr in &*elem.elems {
            if expr.validate(self)? != elem.type_.into() {
                return Err(DecodeError::new("type mismatch"));
            }
        }
        for elem in elem.elems.iter() {
            if let Some(func_idx) = elem.func_idx() {
                self.refs.insert(func_idx);
            }
        }
        self.elems.push(elem);
        Ok(())
    }

    fn push_data(&mut self, data: DataDef) -> Result<(), DecodeError> {
        if self.datas.len() == config::MAX_DATA_COUNT {
            return Err(DecodeError::new("too many data segments"));
        }
        if data.bytes.len() > config::MAX_DATA_SIZE {
            return Err(DecodeError::new("data segment too large"));
        }
        if let DataKind::Active {
            mem_idx,
            ref offset,
        } = data.kind
        {
            self.memory(mem_idx)?;
            if offset.validate(self)? != ValType::I32 {
                return Err(DecodeError::new("type mismatch"));
            }
        }
        self.datas.push(data);
        Ok(())
    }

    fn set_data_count(&mut self, data_count: u32) {
        self.data_count = Some(data_count);
    }

    fn finish(self, engine: &Engine) -> Result<Module, DecodeError> {
        if self.func_types.len() - self.imported_func_count > self.codes.len() {
            return Err(DecodeError::new(
                "function and code section have inconsistent sizes",
            ))?;
        }
        for (type_, code) in self.func_types[self.imported_func_count..]
            .iter()
            .zip(self.codes.iter())
        {
            engine.validate(type_, &self, code)?;
        }
        if let Some(data_count) = self.data_count {
            if data_count != u32::try_from(self.datas.len()).unwrap() {
                return Err(DecodeError::new(
                    "data count and data section have inconsistent sizes",
                ))?;
            }
        }
        Ok(Module {
            types: self.types.into(),
            imports: self.imports.into(),
            imported_func_count: self.imported_func_count,
            imported_table_count: self.imported_table_count,
            imported_memory_count: self.imported_memory_count,
            imported_global_count: self.imported_global_count,
            func_types: self.func_types.into(),
            table_types: self.table_types.into(),
            memory_types: self.memory_types.into(),
            global_types: self.global_types.into(),
            global_vals: self.global_vals.into(),
            exports: self.exports,
            start: self.start,
            codes: self.codes.into(),
            elems: self.elems.into(),
            datas: self.datas.into(),
        })
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

/// A definition for an import.
#[derive(Clone, Debug)]
struct ImportDef {
    module: Arc<str>,
    name: Arc<str>,
    desc: ExternTypeDesc,
}

impl Decode for ImportDef {
    fn decode(decoder: &mut Decoder<'_>) -> Result<Self, DecodeError> {
        Ok(ImportDef {
            module: decoder.decode()?,
            name: decoder.decode()?,
            desc: decoder.decode()?,
        })
    }
}

/// A definition for a [`Table`].
#[derive(Debug)]
struct TableDef {
    type_: TableType,
}

impl Decode for TableDef {
    fn decode(decoder: &mut Decoder) -> Result<Self, DecodeError> {
        Ok(Self {
            type_: decoder.decode()?,
        })
    }
}

/// A definition for a [`Mem`].
#[derive(Debug)]
struct MemDef {
    type_: MemType,
}

impl Decode for MemDef {
    fn decode(decoder: &mut Decoder) -> Result<Self, DecodeError> {
        Ok(Self {
            type_: decoder.decode()?,
        })
    }
}

/// A definition for a [`Global`].
#[derive(Clone, Debug)]
struct GlobalDef {
    type_: GlobalType,
    val: ConstExpr,
}

impl Decode for GlobalDef {
    fn decode(decoder: &mut Decoder) -> Result<Self, DecodeError> {
        Ok(Self {
            type_: decoder.decode()?,
            val: decoder.decode()?,
        })
    }
}

/// A definition for an [`Export`].
#[derive(Clone, Debug)]
struct ExportDef {
    name: Arc<str>,
    desc: ExternValDesc,
}

impl Decode for ExportDef {
    fn decode(decoder: &mut Decoder<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            name: decoder.decode()?,
            desc: decoder.decode()?,
        })
    }
}

/// A definition for an [`Elem`].
#[derive(Clone, Debug)]
struct ElemDef {
    kind: ElemKind,
    type_: RefType,
    elems: Arc<[ConstExpr]>,
}

impl Decode for ElemDef {
    fn decode(decoder: &mut Decoder<'_>) -> Result<Self, DecodeError> {
        let flags: u32 = decoder.decode()?;
        Ok(Self {
            kind: if flags & 0x01 != 0 {
                if flags & 0x02 != 0 {
                    ElemKind::Declarative
                } else {
                    ElemKind::Passive
                }
            } else {
                ElemKind::Active {
                    table_idx: if flags & 0x02 != 0 {
                        decoder.decode()?
                    } else {
                        0
                    },
                    offset: decoder.decode()?,
                }
            },
            type_: if flags & 0x03 != 0 {
                if flags & 0x04 != 0 {
                    decoder.decode()?
                } else {
                    match decoder.decode()? {
                        0x00 => RefType::FuncRef,
                        _ => {
                            return Err(DecodeError::new("malformed element kind"));
                        }
                    }
                }
            } else {
                RefType::FuncRef
            },
            elems: if flags & 0x04 != 0 {
                decoder.decode_iter()?.collect::<Result<_, _>>()?
            } else {
                decoder
                    .decode_iter::<u32>()?
                    .map(|func_idx| func_idx.map(|func_idx| ConstExpr::new_ref_func(func_idx)))
                    .collect::<Result<_, _>>()?
            },
        })
    }
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

impl Decode for DataDef {
    fn decode(decoder: &mut Decoder<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            kind: {
                let flags: u32 = decoder.decode()?;
                if flags & 0x1 != 0 {
                    DataKind::Passive
                } else {
                    DataKind::Active {
                        mem_idx: if flags & 0x02 != 0 {
                            decoder.decode()?
                        } else {
                            0
                        },
                        offset: decoder.decode()?,
                    }
                }
            },
            bytes: decoder.decode_decoder()?.read_bytes_until_end().into(),
        })
    }
}

/// The kind of a [`Data`].
#[derive(Clone, Debug)]
enum DataKind {
    /// Passive [`Data`]s are used to initialize a [`Mem`] during execution.
    Passive,
    /// Active [`Data`]s are used to initialize a [`Mem`] during instantiation.
    Active { mem_idx: u32, offset: ConstExpr },
}
