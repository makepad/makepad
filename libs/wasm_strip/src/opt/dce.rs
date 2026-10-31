//! Dead function, global, import and segment elimination from the real roots.
//!
//! Roots: exports, the start function, and everything reachable from them.
//! A table entry is reachable when the table can be observed: an imported or
//! exported table, or one a live instruction reads, writes or copies, keeps
//! every entry. A table only used by `call_indirect` keeps just the entries
//! whose type one of those live `call_indirect`s names: calling any other
//! entry traps with a signature mismatch whatever its body. Those entries are
//! pointed at one `unreachable` stub (of a type no live `call_indirect` on
//! that table names, so calling it still traps), and stubs at the ends of a
//! segment become empty slots, which trap too.

use super::ir::*;
use super::remap::{visit_indices, IndexKind, Remap, GONE};
use std::collections::{HashMap, HashSet};

struct Live<'a> {
    module: &'a Module,
    canon: Vec<u32>,
    func_types: Vec<u32>,
    funcs: Vec<bool>,
    globals: Vec<bool>,
    datas: Vec<bool>,
    elems: Vec<bool>,
    tables_full: Vec<bool>,
    called: HashSet<(u32, u32)>,
    /// Active segment entries by (table, canonical type): (segment, item).
    entries: HashMap<(u32, u32), Vec<(usize, usize)>>,
    func_work: Vec<u32>,
}

/// Maps every type index to the first index of a structurally equal type.
pub fn canonical_types(types: &[FuncType]) -> Vec<u32> {
    let mut first: HashMap<&FuncType, u32> = HashMap::new();
    types
        .iter()
        .enumerate()
        .map(|(i, ty)| *first.entry(ty).or_insert(i as u32))
        .collect()
}

fn elem_item_func(items: &ElemItems, item: usize) -> Option<u32> {
    match items {
        ElemItems::Funcs(funcs) => Some(funcs[item]),
        ElemItems::Exprs(_, exprs) => match exprs[item].first() {
            Some(Instr::RefFunc(func)) => Some(*func),
            _ => None,
        },
    }
}

impl<'a> Live<'a> {
    fn func(&mut self, func: u32) {
        if !self.funcs[func as usize] {
            self.funcs[func as usize] = true;
            self.func_work.push(func);
        }
    }

    fn global(&mut self, global: u32) {
        if !self.globals[global as usize] {
            self.globals[global as usize] = true;
            let imported = self.module.num_imported(ExternKind::Global) as usize;
            if let Some(defined) = (global as usize).checked_sub(imported) {
                let init = &self.module.globals[defined].init;
                self.expr(init);
            }
        }
    }

    fn expr(&mut self, expr: &[Instr]) {
        for instr in expr {
            self.instr(instr);
        }
    }

    fn elem_all(&mut self, elem: usize) {
        if !self.elems[elem] {
            self.elems[elem] = true;
            let module = self.module;
            match &module.elems[elem].items {
                ElemItems::Funcs(funcs) => {
                    for func in funcs {
                        self.func(*func);
                    }
                }
                ElemItems::Exprs(_, exprs) => {
                    for expr in exprs {
                        self.expr(expr);
                    }
                }
            }
        }
    }

    fn table_full(&mut self, table: u32) {
        if self.tables_full[table as usize] {
            return;
        }
        self.tables_full[table as usize] = true;
        let module = self.module;
        for (i, elem) in module.elems.iter().enumerate() {
            if let ElemMode::Active { table: t, .. } = elem.mode {
                if t == table {
                    self.elem_all(i);
                }
            }
        }
    }

    fn called(&mut self, ty: u32, table: u32) {
        let key = (table, self.canon[ty as usize]);
        if !self.called.insert(key) {
            return;
        }
        let Some(entries) = self.entries.get(&key) else {
            return;
        };
        let module = self.module;
        for (elem, item) in entries.clone() {
            match &module.elems[elem].items {
                ElemItems::Funcs(funcs) => self.func(funcs[item]),
                ElemItems::Exprs(_, exprs) => self.expr(&exprs[item]),
            }
        }
    }

    fn instr(&mut self, instr: &Instr) {
        match instr {
            Instr::CallIndirect { ty, table } | Instr::ReturnCallIndirect { ty, table } => {
                self.called(*ty, *table)
            }
            Instr::TableInit { elem, table } => {
                self.elem_all(*elem as usize);
                self.table_full(*table);
            }
            Instr::ElemDrop(elem) => self.elem_all(*elem as usize),
            Instr::TableGet(_)
            | Instr::TableSet(_)
            | Instr::TableGrow(_)
            | Instr::TableSize(_)
            | Instr::TableFill(_)
            | Instr::TableCopy { .. } => visit_indices(instr, &mut |kind, index| {
                if kind == IndexKind::Table {
                    self.table_full(index);
                }
            }),
            _ => visit_indices(instr, &mut |kind, index| match kind {
                IndexKind::Func => self.func(index),
                IndexKind::Global => self.global(index),
                IndexKind::Data => self.datas[index as usize] = true,
                _ => {}
            }),
        }
    }

    fn drain(&mut self) {
        let imported = self.module.num_imported_funcs();
        let module = self.module;
        while let Some(func) = self.func_work.pop() {
            if let Some(defined) = func.checked_sub(imported) {
                for instr in &module.funcs[defined as usize].body {
                    self.instr(instr);
                }
            }
        }
    }
}

pub fn run(module: &mut Module) {
    let num_funcs = module.func_type_indices().len();
    let num_globals = module.global_types().len();
    let num_tables = module.table_types().len();
    let canon = canonical_types(&module.types);
    let func_types = module.func_type_indices();

    let mut entries: HashMap<(u32, u32), Vec<(usize, usize)>> = HashMap::new();
    for (i, elem) in module.elems.iter().enumerate() {
        if let ElemMode::Active { table, .. } = elem.mode {
            for item in 0..elem.items.len() {
                // Null entries and other expressions need nothing.
                if let Some(func) = elem_item_func(&elem.items, item) {
                    let ty = canon[func_types[func as usize] as usize];
                    entries.entry((table, ty)).or_default().push((i, item));
                }
            }
        }
    }

    let mut live = Live {
        module,
        canon,
        func_types,
        funcs: vec![false; num_funcs],
        globals: vec![false; num_globals],
        datas: vec![false; module.datas.len()],
        elems: vec![false; module.elems.len()],
        tables_full: vec![false; num_tables],
        called: HashSet::new(),
        entries,
        func_work: Vec::new(),
    };

    // Roots.
    for (i, data) in module.datas.iter().enumerate() {
        if let DataMode::Active { offset, .. } = &data.mode {
            live.datas[i] = true;
            live.expr(offset);
        }
    }
    for elem in &module.elems {
        // Active segments always stay (`elems` marks whose entries were
        // all taken live, so it is set below).
        if let ElemMode::Active { offset, .. } = &elem.mode {
            live.expr(offset);
            // Entries that are not a plain function (a `global.get`) are
            // kept as they are, so what they name stays.
            if let ElemItems::Exprs(_, exprs) = &elem.items {
                for expr in exprs {
                    if !matches!(expr.first(), Some(Instr::RefFunc(_))) {
                        live.expr(expr);
                    }
                }
            }
        }
    }
    let mut table_index = 0;
    for import in &module.imports {
        if let ImportDesc::Table(_) = import.desc {
            live.table_full(table_index);
            table_index += 1;
        }
    }
    for export in &module.exports {
        match export.kind {
            ExternKind::Func => live.func(export.index),
            ExternKind::Global => live.global(export.index),
            ExternKind::Table => live.table_full(export.index),
            ExternKind::Memory => {}
        }
    }
    if let Some(start) = module.start {
        live.func(start);
    }
    live.drain();

    let Live {
        funcs: mut live_funcs,
        globals: live_globals,
        datas: live_datas,
        elems: live_elems,
        tables_full,
        func_types,
        called,
        canon,
        ..
    } = live;

    // Entries of call_indirect-only tables that nothing can call.
    let mut stubs: HashMap<u32, u32> = HashMap::new();
    let mut new_funcs: Vec<Func> = Vec::new();
    for elem in &mut module.elems {
        let ElemMode::Active { table, .. } = elem.mode else {
            continue;
        };
        if tables_full[table as usize] {
            continue;
        }
        for item in 0..elem.items.len() {
            let Some(func) = elem_item_func(&elem.items, item) else {
                continue;
            };
            if live_funcs[func as usize] {
                continue;
            }
            let ty = func_types[func as usize];
            debug_assert!(!called.contains(&(table, canon[ty as usize])));
            let stub = *stubs.entry(table).or_insert_with(|| {
                new_funcs.push(Func {
                    ty,
                    locals: Vec::new(),
                    body: vec![Instr::Unreachable, Instr::End],
                });
                (num_funcs + new_funcs.len() - 1) as u32
            });
            match &mut elem.items {
                ElemItems::Funcs(funcs) => funcs[item] = stub,
                ElemItems::Exprs(_, exprs) => exprs[item] = vec![Instr::RefFunc(stub), Instr::End],
            }
        }
    }
    let stub_set: HashSet<u32> = stubs.values().copied().collect();
    module.funcs.extend(new_funcs);
    live_funcs.resize(num_funcs + stub_set.len(), true);
    trim_stub_ends(module, &stub_set, &tables_full);
    for stub in &stub_set {
        let used = module.elems.iter().any(|elem| {
            (0..elem.items.len()).any(|item| elem_item_func(&elem.items, item) == Some(*stub))
        });
        live_funcs[*stub as usize] = used;
    }

    // Declarative segments keep only the live functions they declare.
    let mut elem_live = live_elems;
    for (i, elem) in module.elems.iter().enumerate() {
        if matches!(elem.mode, ElemMode::Active { .. }) {
            elem_live[i] = true;
        }
    }
    for (i, elem) in module.elems.iter_mut().enumerate() {
        if elem.mode == ElemMode::Declarative {
            match &mut elem.items {
                ElemItems::Funcs(funcs) => funcs.retain(|func| live_funcs[*func as usize]),
                ElemItems::Exprs(_, exprs) => exprs.retain(|expr| match expr.first() {
                    Some(Instr::RefFunc(func)) => live_funcs[*func as usize],
                    _ => true,
                }),
            }
            if elem.items.len() == 0 && !elem_live[i] {
                continue;
            }
            elem_live[i] = true;
        }
    }

    let build = |live: &[bool]| -> Option<Vec<u32>> {
        if live.iter().all(|live| *live) {
            return None;
        }
        let mut next = 0;
        Some(
            live.iter()
                .map(|live| {
                    if *live {
                        next += 1;
                        next - 1
                    } else {
                        GONE
                    }
                })
                .collect(),
        )
    };
    // A function or global import nothing uses goes; imported tables and
    // memories stay (they define the instance's shape).
    let remap = Remap {
        funcs: build(&live_funcs),
        globals: build(&live_globals),
        datas: build(&live_datas),
        elems: build(&elem_live),
        ..Remap::default()
    };
    remap.apply(module);
}

/// Stub entries at either end of an active segment become empty table slots,
/// when that cannot change what the table holds: the offset is a constant,
/// the segment fits the table's initial size (so instantiation cannot trap on
/// it), and no other segment writes the same slots.
fn trim_stub_ends(module: &mut Module, stubs: &HashSet<u32>, tables_full: &[bool]) {
    if stubs.is_empty() {
        return;
    }
    let table_types = module.table_types();
    let ranges: Vec<Option<(u32, u64, u64)>> = module
        .elems
        .iter()
        .map(|elem| match &elem.mode {
            ElemMode::Active { table, offset } => match offset.as_slice() {
                [Instr::I32Const(start), Instr::End] => Some((
                    *table,
                    *start as u32 as u64,
                    *start as u32 as u64 + elem.items.len() as u64,
                )),
                _ => None,
            },
            _ => None,
        })
        .collect();
    for i in 0..module.elems.len() {
        let Some((table, start, end)) = ranges[i] else {
            continue;
        };
        if tables_full[table as usize] || end > table_types[table as usize].limits.min as u64 {
            continue;
        }
        let overlaps = module.elems.iter().enumerate().any(|(j, elem)| {
            if j == i {
                return false;
            }
            match (&elem.mode, ranges[j]) {
                (ElemMode::Active { table: t, .. }, Some((_, s, e))) => {
                    *t == table && s < end && start < e
                }
                // A segment at a computed offset could land anywhere.
                (ElemMode::Active { table: t, .. }, None) => *t == table,
                _ => false,
            }
        });
        if overlaps {
            continue;
        }
        let elem = &mut module.elems[i];
        let is_stub = |items: &ElemItems, item: usize| {
            elem_item_func(items, item).map_or(false, |func| stubs.contains(&func))
        };
        let len = elem.items.len();
        let mut lead = 0;
        while lead < len && is_stub(&elem.items, lead) {
            lead += 1;
        }
        let mut tail = len;
        while tail > lead && is_stub(&elem.items, tail - 1) {
            tail -= 1;
        }
        if lead == 0 && tail == len {
            continue;
        }
        match &mut elem.items {
            ElemItems::Funcs(funcs) => {
                funcs.truncate(tail);
                funcs.drain(..lead);
            }
            ElemItems::Exprs(_, exprs) => {
                exprs.truncate(tail);
                exprs.drain(..lead);
            }
        }
        if let ElemMode::Active { offset, .. } = &mut elem.mode {
            *offset = vec![Instr::I32Const((start + lead as u64) as i32), Instr::End];
        }
    }
}
