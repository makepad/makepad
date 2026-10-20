// ported from N:rustc_trait_selection/src/traits/specialize/specialization_graph.rs @8d1a764
use super::fast_reject::{simplify_type, SimplifiedType};
use super::overlap::overlapping_impls;
use crate::fxhash::HashMap;
use crate::program::{DefId, Program};
use crate::tcx::Tcx;

#[derive(Default)]
struct Children { blanket_impls: Vec<DefId>, non_blanket_impls: HashMap<SimplifiedType,Vec<DefId>>, order: Vec<SimplifiedType> }
impl Children {
    fn insert_blindly(&mut self, key: Option<SimplifiedType>, def: DefId) {
        if let Some(key)=key {
            if !self.non_blanket_impls.contains_key(&key) { self.order.push(key); }
            self.non_blanket_impls.entry(key).or_default().push(def);
        } else { self.blanket_impls.push(def); }
    }
    fn possible_siblings(&self, key: Option<SimplifiedType>) -> Vec<DefId> {
        let mut siblings=self.blanket_impls.clone();
        if let Some(key)=key {
            if let Some(defs)=self.non_blanket_impls.get(&key) { siblings.extend_from_slice(defs); }
        } else { for key in &self.order { siblings.extend_from_slice(&self.non_blanket_impls[key]); } }
        siblings
    }
}

/// specialize/mod.rs specialization_graph_provider: upstream impls are inserted
/// blindly before local impls. Specialization and marker overlap are cut, so
/// every accepted local impl is a root sibling (round-34 lead decision).
pub fn specialization_graph(tcx: &Tcx, prog: &Program, krate: u32, trait_def: DefId) -> bool {
    specialization_graph_of(tcx, prog, krate, trait_def, impls_of_trait(tcx, prog, trait_def))
}

/// The impls of `trait_def` in program order (rustc_middle/src/ty/trait_def.rs trait_impls_of,
/// which specialization_graph_provider walks as `all_impls`).
pub fn impls_of_trait<'a>(tcx: &'a Tcx, _prog: &Program, trait_def: DefId) -> &'a [DefId] {
    tcx.trait_impls_of.get(&trait_def).map_or(&[], |impls| impls.as_slice())
}

/// specialization_graph for `trait_def` given its impls in program order (impls_of_trait):
/// the graph reads no other impl, so a caller that indexed every trait's impls in one pass
/// (Tcx::coherence_check) passes its list instead of a scan of the whole program per trait.
pub fn specialization_graph_of(tcx: &Tcx, prog: &Program, krate: u32, trait_def: DefId, impls: &[DefId]) -> bool {
    let mut children=Children::default();
    let mut upstream=vec![false;prog.crates.len()];
    let mut todo=vec![krate];
    while let Some(k)=todo.pop() {
        for &(_,dep) in &prog.crates[k as usize].deps { if !upstream[dep as usize] { upstream[dep as usize]=true; todo.push(dep); } }
    }
    // specialize/mod.rs:388-430: prune foreign non-blanket buckets with no
    // local impl unless a local blanket impl needs every bucket; remote crates
    // precede local definitions, ordered by reverse crate number then DefIndex.
    let mut local_keys=Vec::new(); let mut has_local_blanket=false;
    let mut remote=Vec::new();
    for &imp in impls {
        // Declaration invalidation can remove a head before its index is rebuilt.
        if tcx.impl_trait.get(&imp) != Some(&trait_def) { continue; }
        let owner=prog.def(imp).krate;
        if owner==krate {
            if let Some(key)=simplify_type(&tcx.tys,tcx.impl_self[&imp]) {
                if !local_keys.contains(&key) { local_keys.push(key); }
            } else { has_local_blanket=true; }
        } else if upstream[owner as usize] { remote.push(imp); }
    }
    remote.sort_unstable_by_key(|&def| (std::cmp::Reverse(prog.def(def).krate),def.0));
    for def in remote {
        let key=simplify_type(&tcx.tys,tcx.impl_self[&def]);
        if has_local_blanket || key.is_none() || local_keys.contains(&key.unwrap()) { children.insert_blindly(key,def); }
    }
    let mut ok=true;
    for &imp in impls {
        if tcx.impl_trait.get(&imp) != Some(&trait_def) { continue; }
        if prog.def(imp).krate!=krate { continue; }
        let im=&prog.impls[prog.def(imp).sub as usize];
        let key=simplify_type(&tcx.tys,tcx.impl_self[&im.def]);
        let tr=tcx.impl_trait_ref(prog,im.def,&tcx.generic_indices_of(prog,im.def).identity_args(&tcx.tys));
        let erroneous=tr.args.iter().any(|a| match a {
            crate::consts::GenericArg::Type(t)=>tcx.tys.has_error(*t),
            crate::consts::GenericArg::Const(c)=>match tcx.tys.const_kind(*c) {
                crate::consts::ConstKind::Error=>true,
                // an unevaluated constant of the header that fails to evaluate is a const error as well
                // (rustc normalizes the header: coherence does not report conflicts over [const error])
                crate::consts::ConstKind::Alias(_,alias) if alias.args.is_empty() => {
                    let env=crate::tcx::TypingEnv::fully_monomorphized(prog);
                    tcx.const_eval_resolve(prog,env,alias.kind.def_id(),&[]).is_err()
                }
                _=>false,
            },
            _=>false,
        });
        let mut failed=false;
        if !erroneous {
            for sibling in children.possible_siblings(key) {
                if overlapping_impls(tcx,prog,sibling,im.def,krate,true,true).is_none() { continue; }
                if let Some(overlap)=overlapping_impls(tcx,prog,sibling,im.def,krate,false,true) {
                    let def=prog.def(im.def);
                    let span=prog.files[def.file as usize].ast.item(def.item).lo;
                    // rustc coherence: impls of different polarity that overlap are E0751, not E0119
                    let negative=|d:DefId| { let dd=prog.def(d); let ast=&prog.files[dd.file as usize].ast; ast.as_impl(&ast.item(dd.item).kind).is_some_and(|i| i.negative) };
                    if negative(sibling)!=negative(im.def) {
                        let ty=crate::typeck::ty_to_string(false,prog,tcx,tcx.impl_self[&im.def]);
                        tcx.dcx.report(super::fulfill::ObligationCause { body_id:im.def,span },format!("E0751: found both positive and negative implementation of trait `{}` for type `{}`:",prog.def_path(trait_def),ty));
                        ok=false; failed=true; break;
                    }
                    let mut message=format!("E0119: conflicting implementations of trait `{}`",prog.def_path(trait_def));
                    if !overlap.overflowing_predicates.is_empty() { message.push_str(" (overflow evaluating impl requirements)"); }
                    tcx.dcx.report(super::fulfill::ObligationCause { body_id:im.def,span },message);
                    ok=false; failed=true; break;
                }
                // specialization_graph.rs:130-152: leak-check-only overlap is a
                // future-compat lint, not a rejection.
                let def=prog.def(im.def);
                crate::lint::emit(prog, im.def, &crate::lint::COHERENCE_LEAK_CHECK, def.file, prog.files[def.file as usize].ast.item(def.item).lo,
                    &format!("conflicting implementations of trait `{}`", prog.def_path(trait_def)));
            }
        }
        if !failed { children.insert_blindly(key,im.def); }
    }
    ok
}
