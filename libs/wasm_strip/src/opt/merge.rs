//! Identical function merging. Functions with the same type, locals and body
//! become one; every call, reference, table entry and export of a duplicate
//! moves to the first copy. Merging can make callers identical in turn, so
//! it repeats until nothing merges.

use super::compact::compact_types;
use super::ir::*;
use super::remap::Remap;
use std::collections::HashMap;

pub fn run(module: &mut Module) {
    run_mapped(module);
}

/// `run`, saying where every function went: old index to new index (a
/// merged function goes where its copy went).
pub fn run_mapped(module: &mut Module) -> Vec<u32> {
    // Equal types under different indices would hide equal bodies.
    compact_types(module);
    let mut moved: Vec<u32> = (0..module.num_imported_funcs() + module.funcs.len() as u32).collect();
    loop {
        let imported = module.num_imported_funcs() as usize;
        let total = imported + module.funcs.len();
        let mut first: HashMap<&Func, usize> = HashMap::new();
        let mut keep_of: Vec<usize> = (0..total).collect();
        let mut merged = 0;
        for (i, func) in module.funcs.iter().enumerate() {
            let index = imported + i;
            let kept = *first.entry(func).or_insert(index);
            if kept != index {
                keep_of[index] = kept;
                merged += 1;
            }
        }
        drop(first);
        if merged == 0 {
            return moved;
        }
        let mut new_index = vec![0u32; total];
        let mut next = 0u32;
        for f in 0..total {
            if keep_of[f] == f {
                new_index[f] = next;
                next += 1;
            }
        }
        let map: Vec<u32> = (0..total).map(|f| new_index[keep_of[f]]).collect();
        for to in &mut moved {
            *to = map[*to as usize];
        }
        // Exports of two merged functions would now share a name's target;
        // that is fine, but the name map must keep the first copy's name.
        Remap {
            funcs: Some(map),
            ..Remap::default()
        }
        .apply(module);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_through_callers() {
        let body = |callee: u32| vec![Instr::Call(callee), Instr::End];
        let mut module = Module {
            types: vec![FuncType::default()],
            funcs: vec![
                Func { ty: 0, locals: vec![], body: vec![Instr::Nop, Instr::End] },
                Func { ty: 0, locals: vec![], body: vec![Instr::Nop, Instr::End] },
                Func { ty: 0, locals: vec![], body: body(0) },
                Func { ty: 0, locals: vec![], body: body(1) },
            ],
            exports: vec![
                Export { name: "a".into(), kind: ExternKind::Func, index: 2 },
                Export { name: "b".into(), kind: ExternKind::Func, index: 3 },
            ],
            ..Module::default()
        };
        run(&mut module);
        assert_eq!(module.funcs.len(), 2);
        assert_eq!(module.exports[0].index, 1);
        assert_eq!(module.exports[1].index, 1);
        assert_eq!(module.funcs[1].body, body(0));
    }
}
