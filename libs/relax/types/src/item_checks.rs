//! Item-level checks of `makepad check` that need no body: duplicate definitions (E0428,
//! rustc_resolve build_reduced_graph / report_conflict) and recursive types without
//! indirection (E0072, rustc_ty_utils representability.rs).

use crate::program::{DefId, DefKind, Program, NO_DEF};
use crate::tcx::Tcx;
use crate::types::{TyId, TyKind};

/// The namespaces a definition takes in its module: (type, value).
fn namespaces(prog: &Program, d: DefId) -> (bool, bool) {
    let def = prog.def(d);
    match def.kind {
        DefKind::Mod | DefKind::Enum | DefKind::Union | DefKind::Trait | DefKind::TypeAlias => (true, false),
        // (foreign items are left out: an `extern` block inside a fn body is collected into the
        // enclosing module, not the body's block, so two bodies' declarations would collide)
        DefKind::Fn | DefKind::Const | DefKind::Static => (false, true),
        DefKind::Struct => {
            // tuple and unit structs also live in the value namespace
            let ast = &prog.files[def.file as usize].ast;
            let tuple_or_unit = ast.as_struct(&ast.item(def.item).kind).is_some_and(|s| !matches!(s.data, crate::ast::VariantData::Struct(_)));
            (true, tuple_or_unit)
        }
        _ => (false, false),
    }
}

/// E0428: two items of one module (or block) with the same name in the same namespace. The
/// second is reported, as rustc does ("the name `X` is defined multiple times").
pub fn duplicate_items(prog: &Program, own: &[bool]) -> Vec<String> {
    let mut seen: crate::fxhash::HashMap<(DefId, bool, u32), DefId> = Default::default();
    let mut errors = Vec::new();
    for i in 0..prog.defs.len() {
        let d = DefId(i as u32);
        let def = prog.def(d);
        if !own.get(def.krate as usize).copied().unwrap_or(false) || def.parent == NO_DEF { continue; }
        if prog.def(def.parent).kind != DefKind::Mod { continue; }
        // only items the module's source declares (not the compiler's own definitions)
        if prog.item_defs.get(&(def.file, def.item.0)) != Some(&d) { continue; }
        let name = prog.syms.str(def.name);
        if name == "_" || name.is_empty() { continue; }
        // a module file configured away by its own inner `#![cfg]` defines nothing
        if def.kind == DefKind::Mod {
            let m = &prog.mods[def.sub as usize];
            let attrs: Vec<crate::ast::Attr> = prog.files[m.file as usize].ast.root_attrs.iter().copied().collect();
            if m.file != def.file && !prog.active(m.file, &attrs) { continue; }
        }
        let (ty_ns, value_ns) = namespaces(prog, d);
        for (ns, on) in [(true, ty_ns), (false, value_ns)] {
            if !on { continue; }
            match seen.get(&(def.parent, ns, def.name)) {
                Some(&first) if first != d => {
                    let ast = &prog.files[def.file as usize].ast;
                    let pos = ast.item(def.item).name.lo;
                    let ns_name = if ns { "type" } else { "value" };
                    let module = if prog.block_owner.get(&prog.def(def.parent).sub).is_some() { "this block" } else { "this module" };
                    errors.push(prog.record_error(def.file, pos, "resolve", format!(
                        "E0428: the name `{}` is defined multiple times -- note: `{}` must be defined only once in the {} namespace of {}",
                        name, name, ns_name, module)));
                    break;
                }
                Some(_) => {}
                None => { seen.insert((def.parent, ns, def.name), d); }
            }
        }
    }
    errors
}

/// E0072: ADT `d` contains itself by value (through fields, tuples and arrays, generic
/// arguments substituted), so it has no finite size. References, raw pointers, fn pointers,
/// slices and trait objects end the walk (rustc_ty_utils representability.rs).
pub fn representability(prog: &Program, tcx: &Tcx, d: DefId) -> Option<String> {
    let adt = tcx.adts.get(&d)?;
    let mut stack: Vec<DefId> = vec![d];
    let mut found = false;
    for v in &adt.variants {
        for f in &v.fields {
            if contains(tcx, d, f.ty, &mut stack, 0) { found = true; }
        }
    }
    if !found { return None; }
    let def = prog.def(d);
    let ast = &prog.files[def.file as usize].ast;
    let pos = ast.item(def.item).name.lo;
    Some(prog.record_error(def.file, pos, "typeck", format!(
        "E0072: recursive type `{}` has infinite size -- help: insert some indirection (e.g., a `Box`, `Rc`, or `&`) to break the cycle",
        prog.syms.str(def.name))))
}

fn contains(tcx: &Tcx, root: DefId, t: TyId, stack: &mut Vec<DefId>, depth: u32) -> bool {
    if depth > 64 { return false; }
    match tcx.tys.kind(t).clone() {
        TyKind::Adt(d, args, _) => {
            if d == root { return true; }
            // a cycle that does not pass through `root` is reported at its own ADTs
            if stack.contains(&d) { return false; }
            let Some(adt) = tcx.adts.get(&d) else { return false };
            let args = tcx.tys.list(args).to_vec();
            stack.push(d);
            let mut hit = false;
            'outer: for v in &adt.variants {
                for f in &v.fields {
                    let ft = tcx.tys.subst(f.ty, &args);
                    if contains(tcx, root, ft, stack, depth + 1) { hit = true; break 'outer; }
                }
            }
            stack.pop();
            hit
        }
        TyKind::Tuple(ts) => tcx.tys.list(ts).to_vec().into_iter().any(|x| contains(tcx, root, x, stack, depth + 1)),
        TyKind::Array(e, _) => contains(tcx, root, e, stack, depth + 1),
        _ => false,
    }
}

/// rustc_hir_analysis wfcheck.rs check_impl: a trait impl's trait reference is well formed --
/// the trait's own where-clauses and supertraits hold for it, in the impl's environment
/// (`trait Foo: Send {}` + `impl Foo for Rc<i8> {}`: E0277 "`Rc<i8>` cannot be sent between
/// threads safely"). Ambiguous goals are left to the rest of the check.
pub fn impl_trait_wf(prog: &Program, tcx: &Tcx, imp: DefId) -> Vec<String> {
    use crate::consts::GenericArg;
    use crate::next_solver::{ClauseKind, Goal, PredicateKind};
    let Some(&td) = tcx.impl_trait.get(&imp) else { return Vec::new() };
    let def = prog.def(imp);
    let ast = &prog.files[def.file as usize].ast;
    if ast.as_impl(&ast.item(def.item).kind).is_some_and(|i| i.negative) { return Vec::new(); }
    let indices = tcx.generic_indices_of(prog, imp);
    let trait_ref = tcx.impl_trait_ref(prog, imp, &indices.identity_args(&tcx.tys));
    if trait_ref.args.iter().any(|a| matches!(*a, GenericArg::Type(t) if tcx.tys.has_error(t))) { return Vec::new(); }
    let env = tcx.param_env_id(prog, imp);
    let infcx = crate::infer::InferCtxt::for_item(tcx, prog, crate::tcx::TypingMode::non_body_analysis(), imp);
    let mut errors = Vec::new();
    for p in tcx.clauses_of(prog, td) {
        let p = tcx.instantiate_predicate(prog, td, &trait_ref.args, &p);
        let PredicateKind::Clause(ClauseKind::Trait(t)) = &p.kind else { continue };
        // the trait's own `Self: Trait` predicate holds through this impl
        if t.trait_ref.def_id == td { continue; }
        let tr = t.trait_ref.clone();
        let r = infcx.evaluate_obligation(prog, Goal { param_env: env, predicate: p.clone() });
        if r.may_apply() { continue; }
        let args: Vec<String> = tr.args.iter().map(|a| match a {
            GenericArg::Type(t) => crate::typeck::ty_to_string(false, prog, tcx, *t),
            GenericArg::Const(c) => tcx.tys.const_to_string(*c),
            GenericArg::Lifetime(_) => "'_".to_string(),
        }).collect();
        let name = prog.name(tr.def_id).to_string();
        let self_ty = args.first().cloned().unwrap_or_default();
        let msg = match name.as_str() {
            "Sized" => format!("the size for values of type `{}` cannot be known at compilation time", self_ty),
            "Send" => format!("`{}` cannot be sent between threads safely", self_ty),
            "Sync" => format!("`{}` cannot be shared between threads safely", self_ty),
            _ if args.len() > 1 => format!("the trait bound `{}: {}<{}>` is not satisfied", self_ty, name, args[1..].join(", ")),
            _ => format!("the trait bound `{}: {}` is not satisfied", self_ty, name),
        };
        errors.push(prog.record_error(def.file, ast.item(def.item).lo, "typeck", format!("E0277: {}", msg)));
    }
    errors
}
