//! Index compaction: structurally equal types become one, unused types go,
//! and types, defined globals and defined functions are numbered by how often
//! they are referenced, so the most used ones get one-byte LEB indices.
//! Function order is also code order; outside the one-byte range the
//! original order is kept, since it keeps related code together for the
//! compressor (see `order`).

use super::dce::canonical_types;
use super::ir::*;
use super::remap::{for_each_const_expr, visit_indices, IndexKind, Remap};

pub fn run(module: &mut Module) {
    compact_types(module);
    order_globals(module);
    order_funcs(module);
}

/// How many indices fit a one-byte LEB.
const ONE_BYTE: usize = 128;

#[derive(Default)]
struct Counts {
    types: Vec<usize>,
    funcs: Vec<usize>,
    globals: Vec<usize>,
}

fn count(module: &mut Module) -> Counts {
    let mut counts = Counts {
        types: vec![0; module.types.len()],
        funcs: vec![0; module.func_type_indices().len()],
        globals: vec![0; module.global_types().len()],
    };
    for import in &module.imports {
        if let ImportDesc::Func(ty) = import.desc {
            counts.types[ty as usize] += 1;
        }
    }
    let tally = |instr: &Instr, counts: &mut Counts| {
        visit_indices(instr, &mut |kind, index| match kind {
            IndexKind::Type => counts.types[index as usize] += 1,
            IndexKind::Func => counts.funcs[index as usize] += 1,
            IndexKind::Global => counts.globals[index as usize] += 1,
            _ => {}
        })
    };
    for func in &module.funcs {
        counts.types[func.ty as usize] += 1;
        for instr in &func.body {
            tally(instr, &mut counts);
        }
    }
    let mut exprs: Vec<Vec<Instr>> = Vec::new();
    for_each_const_expr(module, &mut |expr| exprs.push(expr.clone()));
    for expr in &exprs {
        for instr in expr {
            tally(instr, &mut counts);
        }
    }
    for elem in &module.elems {
        if let ElemItems::Funcs(funcs) = &elem.items {
            for func in funcs {
                counts.funcs[*func as usize] += 1;
            }
        }
    }
    for export in &module.exports {
        match export.kind {
            ExternKind::Func => counts.funcs[export.index as usize] += 1,
            ExternKind::Global => counts.globals[export.index as usize] += 1,
            _ => {}
        }
    }
    if let Some(start) = module.start {
        counts.funcs[start as usize] += 1;
    }
    counts
}

/// Merges structurally equal types and drops unused ones, most used first.
pub fn compact_types(module: &mut Module) {
    let canon = canonical_types(&module.types);
    // First fold duplicates onto their canonical index.
    let identity = canon.iter().enumerate().all(|(i, c)| *c == i as u32);
    if !identity {
        let mut next = 0u32;
        let mut new_of_canon = vec![u32::MAX; canon.len()];
        let mut map = vec![0u32; canon.len()];
        for (i, c) in canon.iter().enumerate() {
            if *c == i as u32 {
                new_of_canon[i] = next;
                next += 1;
            }
            map[i] = new_of_canon[*c as usize];
        }
        Remap {
            types: Some(map),
            ..Remap::default()
        }
        .apply(module);
    }
    let counts = count(module);
    let mut order: Vec<usize> = (0..module.types.len()).filter(|t| counts.types[*t] > 0).collect();
    order.sort_by(|a, b| counts.types[*b].cmp(&counts.types[*a]).then(a.cmp(b)));
    let mut map = vec![super::remap::GONE; module.types.len()];
    for (new, old) in order.iter().enumerate() {
        map[*old] = new as u32;
    }
    Remap {
        types: Some(map),
        ..Remap::default()
    }
    .apply(module);
}

fn order_globals(module: &mut Module) {
    let counts = count(module);
    let imported = module.num_imported(ExternKind::Global) as usize;
    let mut order: Vec<usize> = (imported..counts.globals.len()).collect();
    order.sort_by(|a, b| counts.globals[*b].cmp(&counts.globals[*a]).then(a.cmp(b)));
    // A global's initializer may only name globals before it (imported ones,
    // or earlier ones on engines that allow it): keep the order then.
    let names_globals = module
        .globals
        .iter()
        .any(|global| global.init.iter().any(|instr| matches!(instr, Instr::GlobalGet(_))));
    if names_globals {
        return;
    }
    let mut map: Vec<u32> = (0..imported as u32).collect();
    map.resize(counts.globals.len(), 0);
    for (i, old) in order.iter().enumerate() {
        map[*old] = (imported + i) as u32;
    }
    Remap {
        globals: Some(map),
        ..Remap::default()
    }
    .apply(module);
}

/// The most referenced defined functions take the one-byte indices left
/// after the imports, in their original relative order; the rest keep
/// theirs.
fn order_funcs(module: &mut Module) {
    let counts = count(module);
    let imported = module.num_imported_funcs() as usize;
    let total = counts.funcs.len();
    let room = ONE_BYTE.saturating_sub(imported);
    if total - imported <= room {
        return;
    }
    let mut by_count: Vec<usize> = (imported..total).collect();
    by_count.sort_by(|a, b| counts.funcs[*b].cmp(&counts.funcs[*a]).then(a.cmp(b)));
    let mut hot = vec![false; total];
    for f in by_count.iter().take(room) {
        hot[*f] = true;
    }
    let mut map: Vec<u32> = (0..imported as u32).collect();
    map.resize(total, 0);
    let mut next = imported as u32;
    for f in imported..total {
        if hot[f] {
            map[f] = next;
            next += 1;
        }
    }
    for f in imported..total {
        if !hot[f] {
            map[f] = next;
            next += 1;
        }
    }
    Remap {
        funcs: Some(map),
        ..Remap::default()
    }
    .apply(module);
}
