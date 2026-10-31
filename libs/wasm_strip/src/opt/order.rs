//! Code ordering for the compressor. Function order is code order; within
//! each LEB size class of function indices (so no index grows) functions are
//! sorted by type, then by their encoded body, which puts similar bodies next
//! to each other (and makes the function section's type list runs). On a
//! makepad app this beat source order by ~1.9% of the brotli'd code section,
//! and size, type+size, suffix and body-without-locals orders by less.

use super::encode;
use super::ir::*;
use super::remap::Remap;

pub fn run(module: &mut Module) {
    let imported = module.num_imported_funcs() as usize;
    let total = imported + module.funcs.len();
    let bodies: Vec<Vec<u8>> = module
        .funcs
        .iter()
        .map(|func| {
            let mut out = Vec::new();
            encode::func_body(&mut out, func);
            out
        })
        .collect();
    let mut map: Vec<u32> = (0..total as u32).collect();
    // The one-byte indices stay with the functions `compact` put there.
    let mut start = imported.max(128).min(total);
    for end in [1usize << 14, 1 << 21, 1 << 28, usize::MAX] {
        let end = end.min(total);
        if start >= end {
            continue;
        }
        let mut class: Vec<usize> = (start..end).collect();
        class.sort_by(|a, b| {
            let (fa, fb) = (&module.funcs[a - imported], &module.funcs[b - imported]);
            fa.ty
                .cmp(&fb.ty)
                .then_with(|| bodies[a - imported].cmp(&bodies[b - imported]))
                .then(a.cmp(b))
        });
        for (i, f) in class.iter().enumerate() {
            map[*f] = (start + i) as u32;
        }
        start = end;
    }
    Remap {
        funcs: Some(map),
        ..Remap::default()
    }
    .apply(module);
}
