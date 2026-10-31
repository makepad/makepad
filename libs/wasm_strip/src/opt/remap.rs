//! Renumbering of a module's index spaces. A pass builds old→new maps
//! (`GONE` for removed items) and `Remap::apply` rewrites every reference and
//! moves the items to their new places.

use super::ir::*;

pub const GONE: u32 = u32::MAX;

/// Calls `f` on every index an instruction holds, by kind.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IndexKind {
    Func,
    Type,
    Global,
    Table,
    Memory,
    Data,
    Elem,
}

pub fn for_each_index(instr: &mut Instr, f: &mut impl FnMut(IndexKind, &mut u32)) {
    use IndexKind::*;
    match instr {
        Instr::Block(BlockType::Func(ty))
        | Instr::Loop(BlockType::Func(ty))
        | Instr::If(BlockType::Func(ty)) => f(Type, ty),
        Instr::Call(func) | Instr::ReturnCall(func) | Instr::RefFunc(func) => f(Func, func),
        Instr::CallIndirect { ty, table } | Instr::ReturnCallIndirect { ty, table } => {
            f(Type, ty);
            f(Table, table);
        }
        Instr::GlobalGet(global) | Instr::GlobalSet(global) => f(Global, global),
        Instr::TableGet(table)
        | Instr::TableSet(table)
        | Instr::TableGrow(table)
        | Instr::TableSize(table)
        | Instr::TableFill(table) => f(Table, table),
        Instr::Load(_, arg)
        | Instr::Store(_, arg)
        | Instr::SimdMem(_, arg)
        | Instr::SimdMemLane(_, arg, _)
        | Instr::Atomic(_, arg) => f(Memory, &mut arg.memory),
        Instr::MemorySize(memory) | Instr::MemoryGrow(memory) | Instr::MemoryFill(memory) => {
            f(Memory, memory)
        }
        Instr::MemoryInit { data, memory } => {
            f(Data, data);
            f(Memory, memory);
        }
        Instr::DataDrop(data) => f(Data, data),
        Instr::MemoryCopy { dst, src } => {
            f(Memory, dst);
            f(Memory, src);
        }
        Instr::TableInit { elem, table } => {
            f(Elem, elem);
            f(Table, table);
        }
        Instr::ElemDrop(elem) => f(Elem, elem),
        Instr::TableCopy { dst, src } => {
            f(Table, dst);
            f(Table, src);
        }
        _ => {}
    }
}

/// Read-only counterpart of `for_each_index`.
pub fn visit_indices(instr: &Instr, f: &mut impl FnMut(IndexKind, u32)) {
    let mut copy = instr.clone();
    for_each_index(&mut copy, &mut |kind, index| f(kind, *index));
}

/// Calls `f` on every expression of the module outside function bodies.
pub fn for_each_const_expr(module: &mut Module, f: &mut impl FnMut(&mut Vec<Instr>)) {
    for global in &mut module.globals {
        f(&mut global.init);
    }
    for elem in &mut module.elems {
        if let ElemMode::Active { offset, .. } = &mut elem.mode {
            f(offset);
        }
        if let ElemItems::Exprs(_, exprs) = &mut elem.items {
            for expr in exprs {
                f(expr);
            }
        }
    }
    for data in &mut module.datas {
        if let DataMode::Active { offset, .. } = &mut data.mode {
            f(offset);
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Remap {
    pub funcs: Option<Vec<u32>>,
    pub types: Option<Vec<u32>>,
    pub globals: Option<Vec<u32>>,
    pub tables: Option<Vec<u32>>,
    pub datas: Option<Vec<u32>>,
    pub elems: Option<Vec<u32>>,
}

fn map_one(map: &Option<Vec<u32>>, index: &mut u32) {
    if let Some(map) = map {
        let new = map[*index as usize];
        debug_assert!(new != GONE, "reference to a removed item");
        *index = new;
    }
}

/// Moves the defined items to their new slots and drops the removed ones.
/// `num_imported` is the old count of imported items of this kind, and
/// `new_imported` the new one.
fn permute<T>(items: Vec<T>, map: &[u32], num_imported: usize, new_imported: usize) -> Vec<T> {
    let mut slots: Vec<Option<T>> = Vec::new();
    for (i, item) in items.into_iter().enumerate() {
        let new = map[num_imported + i];
        if new == GONE {
            continue;
        }
        let at = new as usize - new_imported;
        if slots.len() <= at {
            slots.resize_with(at + 1, || None);
        }
        slots[at] = Some(item);
    }
    slots
        .into_iter()
        .map(|slot| slot.expect("remap left a hole"))
        .collect()
}

fn new_imported_count(map: &[u32], num_imported: usize) -> usize {
    map[..num_imported].iter().filter(|new| **new != GONE).count()
}

impl Remap {
    pub fn instr(&self, instr: &mut Instr) {
        for_each_index(instr, &mut |kind, index| match kind {
            IndexKind::Func => map_one(&self.funcs, index),
            IndexKind::Type => map_one(&self.types, index),
            IndexKind::Global => map_one(&self.globals, index),
            IndexKind::Table => map_one(&self.tables, index),
            IndexKind::Data => map_one(&self.datas, index),
            IndexKind::Elem => map_one(&self.elems, index),
            IndexKind::Memory => {}
        });
    }

    pub fn expr(&self, expr: &mut [Instr]) {
        for instr in expr {
            self.instr(instr);
        }
    }

    /// Rewrites every reference, then moves the items. Imported items may
    /// only be removed, never reordered.
    pub fn apply(&self, module: &mut Module) {
        let old_imported_funcs = module.num_imported(ExternKind::Func) as usize;
        let old_imported_globals = module.num_imported(ExternKind::Global) as usize;
        let old_imported_tables = module.num_imported(ExternKind::Table) as usize;

        for func in &mut module.funcs {
            map_one(&self.types, &mut func.ty);
            self.expr(&mut func.body);
        }
        for_each_const_expr(module, &mut |expr| self.expr(expr));
        for elem in &mut module.elems {
            if let ElemMode::Active { table, .. } = &mut elem.mode {
                map_one(&self.tables, table);
            }
            if let ElemItems::Funcs(funcs) = &mut elem.items {
                for func in funcs {
                    map_one(&self.funcs, func);
                }
            }
        }
        for export in &mut module.exports {
            match export.kind {
                ExternKind::Func => map_one(&self.funcs, &mut export.index),
                ExternKind::Global => map_one(&self.globals, &mut export.index),
                ExternKind::Table => map_one(&self.tables, &mut export.index),
                ExternKind::Memory => {}
            }
        }
        if let Some(start) = &mut module.start {
            map_one(&self.funcs, start);
        }
        if let Some(names) = &mut module.names {
            let remap_names = |list: &mut Vec<(u32, String)>, map: &Option<Vec<u32>>| {
                if let Some(map) = map {
                    let mut out: Vec<(u32, String)> = std::mem::take(list)
                        .into_iter()
                        .filter_map(|(index, name)| {
                            let new = *map.get(index as usize)?;
                            (new != GONE).then_some((new, name))
                        })
                        .collect();
                    out.sort_by_key(|(index, _)| *index);
                    out.dedup_by_key(|(index, _)| *index);
                    *list = out;
                }
            };
            remap_names(&mut names.funcs, &self.funcs);
            remap_names(&mut names.globals, &self.globals);
        }

        // Imports: type references, then removal of the dropped ones.
        let mut counters = [0usize; 3];
        let funcs_map = self.funcs.as_deref();
        let globals_map = self.globals.as_deref();
        let tables_map = self.tables.as_deref();
        module.imports.retain_mut(|import| {
            let (slot, map) = match &mut import.desc {
                ImportDesc::Func(ty) => {
                    map_one(&self.types, ty);
                    (0, funcs_map)
                }
                ImportDesc::Global(_) => (1, globals_map),
                ImportDesc::Table(_) => (2, tables_map),
                ImportDesc::Memory(_) => return true,
            };
            let index = counters[slot];
            counters[slot] += 1;
            map.map_or(true, |map| map[index] != GONE)
        });

        if let Some(map) = &self.funcs {
            let new_imported = new_imported_count(map, old_imported_funcs);
            module.funcs = permute(
                std::mem::take(&mut module.funcs),
                map,
                old_imported_funcs,
                new_imported,
            );
        }
        if let Some(map) = &self.globals {
            let new_imported = new_imported_count(map, old_imported_globals);
            module.globals = permute(
                std::mem::take(&mut module.globals),
                map,
                old_imported_globals,
                new_imported,
            );
        }
        if let Some(map) = &self.tables {
            let new_imported = new_imported_count(map, old_imported_tables);
            module.tables = permute(
                std::mem::take(&mut module.tables),
                map,
                old_imported_tables,
                new_imported,
            );
        }
        if let Some(map) = &self.types {
            module.types = permute(std::mem::take(&mut module.types), map, 0, 0);
        }
        if let Some(map) = &self.datas {
            module.datas = permute(std::mem::take(&mut module.datas), map, 0, 0);
            if module.data_count.is_some() {
                module.data_count = Some(module.datas.len() as u32);
            }
        }
        if let Some(map) = &self.elems {
            module.elems = permute(std::mem::take(&mut module.elems), map, 0, 0);
        }
    }
}
