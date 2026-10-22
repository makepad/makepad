//! Structure sizes for the memory report (MAKEPAD_RUN_MEM): the unit's big arrays, RIR functions and
//! the trees' nested vectors. memstat.rs holds the counting allocator and the tags these numbers
//! are printed with.

fn vbytes<V: HeapBytes>(v: &V) -> u64 {
    v.heap_bytes_u64()
}

/// Heap bytes of an owned array (a mapped Arena holds none).
pub trait HeapBytes {
    fn heap_bytes_u64(&self) -> u64;
}

impl<T> HeapBytes for Vec<T> {
    fn heap_bytes_u64(&self) -> u64 {
        (self.capacity() * std::mem::size_of::<T>()) as u64
    }
}

impl<T: crate::crate_image::Codec> HeapBytes for crate::collected::ImageVec<T> {
    fn heap_bytes_u64(&self) -> u64 {
        self.heap_bytes() as u64
    }
}

impl<T> HeapBytes for crate::arena::Arena<T> {
    fn heap_bytes_u64(&self) -> u64 {
        self.heap_bytes() as u64
    }
}

/// Shallow sizes of the unit's big structures (top-level arrays; nested vectors not followed),
/// for the report: parsed files by crate and part, names, definitions, slots and their RIR.
pub fn unit_sizes(u: &crate::jit::Unit) -> Vec<(String, u64)> {
    let p = &u.prog;
    let mut out: Vec<(String, u64)> = Vec::new();
    let (mut src, mut toks, mut ast, mut side) = (0u64, 0u64, 0u64, 0u64);
    let mut by_crate: Vec<u64> = vec![0; p.crates.len()];
    for (fi, f) in p.files.iter().enumerate() {
        let a = &f.ast;
        let ab = vbytes(&a.exprs) + vbytes(&a.pats) + vbytes(&a.tys) + vbytes(&a.items) + vbytes(&a.blocks) + vbytes(&a.mac_args);
        let sb = (f.exp_map.heap_bytes() as u64) + (f.tok_origin.heap_bytes() as u64) + (f.token_origins.heap_bytes() as u64) + (f.rules_exp.heap_bytes() as u64) + (f.body_origin.heap_bytes() as u64) + f.line_starts.get().map_or(0, |l| vbytes(l));
        src += f.src.heap_bytes() as u64;
        toks += f.toks.heap_bytes() as u64;
        ast += ab;
        side += sb;
        if let Some(c) = p.file_crate.get(fi) {
            if let Some(x) = by_crate.get_mut(*c as usize) {
                *x += (f.src.heap_bytes() as u64) + (f.toks.heap_bytes() as u64) + ab + sb;
            }
        }
    }
    let (mut used, mut cap) = (0u64, 0u64);
    for f in p.files.iter() {
        let a = &f.ast;
        used += (a.exprs.len() * std::mem::size_of::<crate::ast::Expr>() + a.pats.len() * std::mem::size_of::<crate::ast::Pat>() + a.tys.len() * std::mem::size_of::<crate::ast::Ty>() + a.items.len() * std::mem::size_of::<crate::ast::Item>() + a.blocks.len() * std::mem::size_of::<crate::ast::Block>() + f.toks.len() * std::mem::size_of::<crate::lexer::Tok>() + f.src.len()) as u64;
        cap += vbytes(&a.exprs) + vbytes(&a.pats) + vbytes(&a.tys) + vbytes(&a.items) + vbytes(&a.blocks) + (f.toks.heap_bytes() as u64) + (f.src.heap_bytes() as u64);
    }
    let n_exprs: usize = p.files.iter().map(|f| f.ast.exprs.len()).sum();
    // the value is the HEAP share (owned arrays' capacity); `used` counts every array's logical
    // bytes, mapped ones included (those are cache file pages, not malloc)
    out.push((format!("files: arenas+toks+src on the heap (logical {:.0} MB incl. arrays mapped from the cache)", used as f64 / 1048576.0), cap));
    out.push((format!("  node sizes expr {} pat {} ty {} item {} block {} tok {}; {} exprs", std::mem::size_of::<crate::ast::Expr>(), std::mem::size_of::<crate::ast::Pat>(), std::mem::size_of::<crate::ast::Ty>(), std::mem::size_of::<crate::ast::Item>(), std::mem::size_of::<crate::ast::Block>(), std::mem::size_of::<crate::lexer::Tok>(), n_exprs), 0));
    out.push((format!("files: source ({} files)", p.files.len()), src));
    out.push(("files: tokens".to_string(), toks));
    out.push(("files: ast arenas (shallow)".to_string(), ast));
    out.push(("files: position side tables".to_string(), side));
    let mut bc: Vec<(u64, usize)> = by_crate.iter().enumerate().map(|(i, b)| (*b, i)).collect();
    bc.sort_by_key(|x| std::cmp::Reverse(x.0));
    for (b, i) in bc.iter().take(8) {
        out.push((format!("  crate {}", p.syms.str(p.crates[*i].name)), *b));
    }
    out.push(("front: retained crate records".into(), p.front_ahead.iter().map(vbytes).sum()));
    out.push(("front: retained file records".into(), p.front_files.iter().map(|(_, _, b)| vbytes(b)).sum()));
    out.push(("sources: authored registry".into(), p.sources.heap_bytes()));
    let (n_syms, sym_bytes) = p.syms.heap_bytes();
    out.push((format!("interner ({} names)", n_syms), sym_bytes));
    out.push((format!("defs ({})", p.defs.len()), vbytes(&p.defs)));
    let mut mods = vbytes(&p.mods);
    for m in &p.mods {
        mods += ((m.types.capacity() + m.values.capacity() + m.macros.capacity()) * 9 + m.import_pub.capacity() * 10) as u64 + vbytes(&m.globs) + vbytes(&m.anon_traits);
    }
    out.push((format!("modules ({})", p.mods.len()), mods));
    out.push((format!("imports ({})", p.imports.len()), vbytes(&p.imports)));
    out.push((format!("item_defs map ({})", p.item_defs.len()), (p.item_defs.capacity() * 13) as u64));
    out.push((format!("fn sigs ({})", u.tcx.signature_count()), u.tcx.signature_bytes() as u64));
    let (mut n_rir, mut rir, mut spec, mut pcm, mut names) = (0u64, 0u64, 0u64, 0u64, 0u64);
    let mut history = 0u64;
    let mut n_fns = 0u64;
    let (mut rir_by, mut rir_n) = ([0u64; 3], [0u64; 3]);
    let mut parts = [0u64; 4];
    for f in u.fns.iter() {
        n_fns += 1;
        names += f.name.len() as u64;
        pcm += f.pc_map.bytes() as u64;
        if let Some(h) = &f.history {
            history += std::mem::size_of::<crate::jit::SlotHistory>() as u64 + vbytes(&h.prev) + vbytes(&h.retired) + vbytes(&h.inlined_into);
            pcm += h.prev.iter().map(|p| p.2.bytes() as u64).sum::<u64>() + h.retired.iter().map(|p| p.2.bytes() as u64).sum::<u64>();
        }
        if let Some(r) = &f.rir {
            n_rir += 1;
            let b = rir_bytes(r);
            let pp = rir_parts(r);
            for k in 0..4 {
                parts[k] += pp[k];
            }
            rir += b;
            let n = relax_codegen::opt::inst_count(r);
            let k = if n > 600 { 2 } else if n > 120 { 1 } else { 0 };
            rir_by[k] += b;
            rir_n[k] += 1;
        }
        if let Some(s) = &f.spec {
            spec += std::mem::size_of::<crate::jit::SpecCode>() as u64 + vbytes(&s.code) + vbytes(&s.map) + vbytes(&s.relocs);
        }
    }
    out.push((format!("slots ({}; FnEntry)", n_fns), n_fns * std::mem::size_of::<crate::jit::FnEntry>() as u64));
    out.push(("slot names".to_string(), names));
    out.push(("slot histories".to_string(), history));
    out.push(("slot pc maps".to_string(), pcm));
    // the variable tables in the installed bodies' pc maps (makepad inspect `locals`) and the
    // debug tables the retained RIR carries
    let (mut n_vars, mut var_bytes, mut dbg_bytes) = (0u64, 0u64, 0u64);
    for f in u.fns.iter() {
        let e = f.pc_map.entries();
        let k = e.iter().position(|x| x.0 == relax_backend::dbgloc::PC_VARS);
        if let Some(k) = k {
            // its length entry, then its bytes as they are (PcMap::encode_to)
            n_vars += 1;
            var_bytes += relax_backend::dbgloc::varint_bytes(if k > 0 { e[k - 1] } else { (0, 0) }, &e[k..k + 1]) + e[k].1 as u32 as u64;
        }
        if let Some(r) = &f.rir {
            dbg_bytes += vbytes(&r.dbg) + r.dbg.iter().map(|d| vbytes(&d.k) + match &d.loc { relax_backend::rir::DbgLoc::Regs(v) => vbytes(v), _ => 0 }).sum::<u64>();
        }
    }
    out.push((format!("slot pc maps: of which variable tables ({} bodies)", n_vars), var_bytes));
    out.push(("RIR debug tables (Func::dbg)".to_string(), dbg_bytes));
    if relax_backend::dbgloc::stats_on() {
        let st = &relax_backend::dbgloc::STATS;
        let g = |k: usize| st[k].load(std::sync::atomic::Ordering::Relaxed);
        out.push((format!("compiled: {} of {} bodies with a table; table bytes", g(0), g(1)), g(2)));
        out.push(("compiled: the tables' pc-map bytes".to_string(), g(3)));
        out.push(("compiled: the code entries' pc-map bytes".to_string(), g(4)));
    }
    out.push((format!("slot RIR kept ({} fns; inst {} B, block {} B, func {} B)", n_rir, std::mem::size_of::<relax_backend::rir::Inst>(), std::mem::size_of::<relax_backend::rir::Block>(), std::mem::size_of::<relax_backend::rir::Func>()), rir));
    out.push((format!("  RIR insts used (of {:.1} MB capacity)", parts[1] as f64 / 1048576.0), parts[0]));
    out.push((format!("  RIR positions used (of {:.1} MB capacity)", parts[3] as f64 / 1048576.0), parts[2]));
    out.push((format!("  RIR of fns <= 120 insts ({})", rir_n[0]), rir_by[0]));
    out.push((format!("  RIR of fns 121..600 insts ({})", rir_n[1]), rir_by[1]));
    out.push((format!("  RIR of fns > 600 insts ({}; never inlined)", rir_n[2]), rir_by[2]));
    out.push(("speculative code not yet installed".to_string(), spec));
    let bs = std::mem::size_of::<crate::typeck::Body>();
    out.push((format!("checked bodies kept for closures ({}; {} B each + heap)", u.bodies.len(), bs), u.bodies.values().map(|b| bs + b.heap_bytes()).sum::<usize>() as u64));
    out.push((format!("old-version bodies ({})", u.old_bodies.len()), u.old_bodies.values().map(|b| bs + b.heap_bytes()).sum::<usize>() as u64));
    out.extend(file_arrays(&u.prog));
    out.extend(u.front.mem_lines());
    out.extend(u.prog.memo_mem_lines());
    out.extend(u.tcx.mem_lines());
    out.extend(u.lay.mem_lines());
    if let Some(c) = u.cache.as_ref() { out.extend(c.mem_lines()); }
    {
        let t = u.code.read().unwrap_or_else(|e| e.into_inner());
        out.push((format!("code index ({})", t.index.len()), vbytes(&t.index)));
    }
    if std::env::var_os("MAKEPAD_RUN_AST_STATS").is_some() {
        out.extend(ast_nested(&u.prog));
    }
    out
}

/// Per-array heap and logical bytes, including immutable body segments. Mapped
/// arrays have logical bytes but no heap capacity. Only called by memory reports.
fn file_arrays(p: &crate::program::Program) -> Vec<(String, u64)> {
    let mut totals = std::collections::BTreeMap::<String, (u64, u64)>::new();
    fn add<T>(totals: &mut std::collections::BTreeMap<String, (u64, u64)>, name: String, a: &crate::arena::Arena<T>) {
        let n = totals.entry(name).or_default();
        n.0 += a.heap_bytes() as u64;
        n.1 += std::mem::size_of_val(&**a) as u64;
    }
    fn add_toks(totals: &mut std::collections::BTreeMap<String, (u64, u64)>, name: String, a: &crate::packed::PackedToks) {
        let n = totals.entry(name).or_default();
        n.0 += a.heap_bytes() as u64;
        n.1 += (a.words().len() * 4) as u64;
    }
    fn add_t<K: crate::origin_table::Kind>(totals: &mut std::collections::BTreeMap<String, (u64, u64)>, name: String, t: &crate::origin_table::Table<K>) {
        let n = totals.entry(name).or_default();
        n.0 += t.heap_bytes() as u64;
        n.1 += t.bytes().len() as u64;
    }
    for f in &p.files {
        macro_rules! tables {
            ($prefix:expr, $value:expr, $($field:ident),*) => {
                $(add_t(&mut totals, format!("{}{}", $prefix, stringify!($field)), &$value.$field);)*
            };
        }
        macro_rules! arrays {
            ($prefix:expr, $value:expr, $($field:ident),*) => {
                $(add(&mut totals, format!("{}{}", $prefix, stringify!($field)), &$value.$field);)*
            };
        }
        arrays!("main.", f, src);
        add_toks(&mut totals, "main.toks".into(), &f.toks);
        tables!("main.", f, exp_map, tok_origin, token_origins, rules_exp, body_origin);
        for part in f.parts() {
            arrays!("part.", part, ext);
            add_toks(&mut totals, "part.toks".into(), &part.toks);
            tables!("part.", part, exp_map, tok_origin, token_origins, rules_exp, body_origin);
        }
        let mut a = Some(&f.ast);
        let mut prefix = "main.ast.";
        while let Some(ast) = a {
            arrays!(prefix, ast, exprs, pats, tys, blocks, root_attrs, root_items,
                mac_args, path_segs, stmts, expr_lists, pat_lists, ty_lists, field_exprs,
                arms, closure_params, field_pats, qselfs, generic_args, fn_ptrs,
                generic_arg_lists, bounds, generic_params, ident_lists, attrs,
                asm_ops, items, fn_sigs, params, field_defs, variants, use_trees,
                item_lists, where_preds, method_calls, for_exprs, closure_exprs, struct_exprs, mac_calls, while_exprs, let_stmts, attrs_stmts, use_items, struct_items, union_items, enum_items, alias_items, trait_items, trait_alias_items, impl_items, foreign_mods, vis_paths);
            a = ast.part.get().map(|a| &**a);
            prefix = "part.ast.";
        }
    }
    totals.into_iter().map(|(name, (heap, used))| {
        (format!("files: {name} heap ({} logical bytes)", used), heap)
    }).collect()
}

/// Shallow bytes of one RIR function (blocks, insts, positions, vregs, slots).
/// (insts bytes used, insts capacity, pos used, pos capacity) of one RIR function
pub fn rir_parts(f: &relax_backend::rir::Func) -> [u64; 4] {
    let mut r = [0u64; 4];
    for blk in &f.blocks {
        r[0] += (blk.insts.len() * std::mem::size_of::<relax_backend::rir::Inst>()) as u64;
        r[1] += vbytes(&blk.insts);
        r[2] += (blk.pos.len() * 8) as u64;
        r[3] += vbytes(&blk.pos);
    }
    r
}

pub fn rir_bytes(f: &relax_backend::rir::Func) -> u64 {
    let mut b = vbytes(&f.blocks) + vbytes(&f.vregs) + vbytes(&f.slots) + vbytes(&f.params) + f.name.capacity() as u64;
    for blk in &f.blocks {
        b += vbytes(&blk.insts) + vbytes(&blk.pos);
    }
    b
}

/// MAKEPAD_RUN_AST_STATS=1: heap bytes the trees' nested vectors and boxes hold, by node kind
/// (capacity x element size, + 16 bytes malloc overhead per block): what a flat tree saves.
pub fn ast_nested(p: &crate::program::Program) -> Vec<(String, u64)> {
    use crate::ast::*;
    fn path(p: &Path) -> u64 {
        if p.has_qself() {
            64
        } else {
            0
        }
    }
    let mut k: std::collections::BTreeMap<&'static str, (u64, u64)> = Default::default();
    let mut add = |name: &'static str, b: u64| {
        let e = k.entry(name).or_insert((0, 0));
        e.0 += b;
        e.1 += 1;
    };
    let visit = |a: &Ast, add: &mut dyn FnMut(&'static str, u64)| {
        for e in &a.exprs {
            match &e.kind {
                ExprKind::Path(p) => add("expr Path/Struct qself", path(p)),
                ExprKind::Struct(r) => add("expr Path/Struct qself", path(&a.row(*r).path)),
                ExprKind::Mac(m) => add("expr Mac qself", path(&a.row(*m).path)),
                _ => add("expr (no nested)", 0),
            }
        }
        for p in &a.pats {
            match p {
                Pat::Path(x) | Pat::TupleStruct(x, _) | Pat::Struct(x, ..) => add("pat Path qself", path(x)),
                _ => add("pat (no nested)", 0),
            }
        }
        for t in &a.tys {
            match t {
                Ty::Path(x) => add("ty Path qself", path(x)),
                _ => add("ty (no nested)", 0),
            }
        }
        fn cap<V: HeapBytes>(v: &V) -> usize {
            v.heap_bytes_u64() as usize
        }
        add(
            "flat arrays (segments, statements, lists, generic args, bounds, params, attrs, ..)",
            (cap(&a.path_segs) + cap(&a.stmts) + cap(&a.expr_lists) + cap(&a.pat_lists) + cap(&a.ty_lists) + cap(&a.field_exprs) + cap(&a.arms) + cap(&a.closure_params) + cap(&a.field_pats) + cap(&a.qselfs) + cap(&a.generic_args) + cap(&a.fn_ptrs) + cap(&a.generic_arg_lists) + cap(&a.bounds) + cap(&a.generic_params) + cap(&a.ident_lists) + cap(&a.attrs) + cap(&a.asm_ops) + cap(&a.mac_args) + cap(&a.fn_sigs) + cap(&a.params) + cap(&a.field_defs) + cap(&a.variants) + cap(&a.use_trees) + cap(&a.item_lists) + cap(&a.where_preds)) as u64,
        );
    };
    for f in &p.files {
        visit(&f.ast, &mut add);
        if let Some(part) = f.ast.part.get() {
            visit(part, &mut add);
        }
    }
    k.into_iter().map(|(n, (b, c))| (format!("nested {} ({} nodes)", n, c), b)).collect()
}
