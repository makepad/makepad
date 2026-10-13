// ported from N:rustc_ty_utils/src/assoc.rs @8d1a764
use crate::ast::ItemKind;
use crate::program::{Def, DefId, DefKind, Program, path_segs};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImplTraitInTraitData {
    Trait { fn_def_id: DefId, opaque_def_id: DefId },
    Impl { fn_def_id: DefId },
}

/// assoc.rs:142-208. Synthesis runs after name resolution and before the
/// immutable declaration consumer; impl items follow their trait item links.
pub fn collect(prog: &mut Program) {
    collect_in(prog, None)
}

/// `collect` for the traits and impls of the crates in `visible` (None: every crate). Lead
/// decision 52 (R1): an item edit is seen only by its crate and that crate's dependents
/// (Program::crates_seeing_files). A trait or impl of any other crate names only items of
/// that crate's dependencies, none of which changed, so its synthesized RPITIT items and
/// trait-item links are exactly what a full pass recomputes; they are kept.
pub fn collect_in(prog: &mut Program, visible: Option<&[bool]>) {
    if std::env::var_os("MAKEPAD_RUN_INVALIDATION_TRACE").is_some() { eprintln!("invalidation-trace: assoc whole={} scoped={}", usize::from(visible.is_none()), usize::from(visible.is_some())); }
    let sees = |prog: &Program, d: DefId| visible.map_or(true, |v| v.get(prog.def(d).krate as usize).copied().unwrap_or(true));
    let previous = match visible {
        None => {
            prog.associated_types_for_impl_traits.clear();
            prog.opaque_to_rpitit.clear();
            prog.trait_item_def_id.clear();
            std::mem::take(&mut prog.impl_trait_in_trait_data).iter().map(|(k, v)| (*k, *v)).collect()
        }
        Some(v) => {
            let defs = &prog.defs;
            let seen = |d: &DefId| v.get(defs[d.0 as usize].krate as usize).copied().unwrap_or(true);
            prog.associated_types_for_impl_traits.retain(|d, _| !seen(d));
            prog.opaque_to_rpitit.retain(|d, _| !seen(d));
            prog.trait_item_def_id.retain(|d, _| !seen(d));
            let mut previous = crate::fxhash::HashMap::default();
            prog.impl_trait_in_trait_data.retain(|d, data| if seen(d) { previous.insert(*d, *data); false } else { true });
            previous
        }
    };
    for ti in 0..prog.traits.len() {
        // (a trait's items all belong to its crate; a trait without items synthesizes nothing)
        if visible.is_some() && prog.traits[ti].first().is_none_or(|&m| !sees(prog, m)) { continue; }
        link_trait(prog, ti, &previous);
    }
    for ii in 0..prog.impls.len() {
        if !sees(prog, prog.impls[ii].def) { continue; }
        link_impl(prog, ii, &previous);
    }
}

/// collect_in for a deferred module load (Program::link_deferred_modules): the traits and impls
/// a load can change, with exactly the links and RPITIT items the scoped pass gives them. A load
/// only adds definitions. A trait's synthesized items are a function of its own methods'
/// signatures (assoc.rs:142-208 associated_types_for_impl_traits), so an existing trait keeps
/// them. An existing impl keeps its links when every item is linked: its trait path resolved,
/// the trait's items (and their synthesized ones) are unchanged, and late.rs:3837-3960 matches by
/// name within them. Linked again are: the traits and impls the load added (`first_def` on;
/// a transaction that loads more modules links its earlier ones once more), and the trait impls
/// of the visible crates with an unlinked item -- a trait path that resolved to nothing before
/// (into the module just loaded), or an item without a trait item (its E0407/E0437/E0438 is
/// recorded again; prog.errors holds it once).
pub fn collect_loaded(prog: &mut Program, first_def: usize, visible: &[bool]) {
    collect_loaded_with(prog, first_def, visible, &[], &[])
}

/// collect_loaded that also links again `extra_traits` and `extra_impls` (indices into
/// `prog.traits` / `prog.impls`, ascending): the traits and impls of a retained collected
/// prefix whose synthesized items were dropped with the definitions after the suffix start
/// (Program::retain_collected_prefix). They come first, as the whole pass links them in order.
pub fn collect_loaded_with(prog: &mut Program, first_def: usize, visible: &[bool], extra_traits: &[usize], extra_impls: &[usize]) {
    if std::env::var_os("MAKEPAD_RUN_INVALIDATION_TRACE").is_some() { eprintln!("invalidation-trace: assoc added=1"); }
    let mut traits: Vec<usize> = extra_traits.to_vec();
    let mut impls: Vec<usize> = extra_impls.to_vec();
    for di in first_def..prog.defs.len() {
        let def = &prog.defs[di];
        match def.kind {
            DefKind::Trait => traits.push(def.sub as usize),
            DefKind::Impl => impls.push(def.sub as usize),
            _ => {}
        }
    }
    for ii in 0..prog.impls.len() {
        let imp = prog.impls[ii].def;
        if imp.0 as usize >= first_def || !visible.get(prog.def(imp).krate as usize).copied().unwrap_or(true) { continue; }
        let def = prog.def(imp);
        if !{ let ast = &prog.files[def.file as usize].ast; ast.as_impl(&ast.item(def.item).kind).is_some_and(|i| i.trait_.is_some()) } { continue; }
        let unlinked = prog.impls[ii].items.iter().any(|d| !prog.trait_item_def_id.contains_key(d));
        if unlinked && !extra_impls.contains(&ii) { impls.push(ii); }
    }
    // the synthesized items of the traits and impls linked again (the scoped pass's `previous`)
    let mut previous = crate::fxhash::HashMap::default();
    for &ti in &traits {
        for i in 0..prog.traits[ti].len() {
            let d = prog.traits[ti][i];
            if let Some(data) = prog.impl_trait_in_trait_data.remove(&d) {
                if let ImplTraitInTraitData::Trait { opaque_def_id, .. } = data { prog.opaque_to_rpitit.remove(&opaque_def_id); }
                previous.insert(d, data);
            } else {
                prog.associated_types_for_impl_traits.remove(&d);
            }
        }
    }
    for &ii in &impls {
        for i in 0..prog.impls[ii].items.len() {
            let d = prog.impls[ii].items[i];
            if let Some(data) = prog.impl_trait_in_trait_data.remove(&d) { previous.insert(d, data); }
            else { prog.associated_types_for_impl_traits.remove(&d); }
            prog.trait_item_def_id.remove(&d);
        }
    }
    for &ti in &traits { link_trait(prog, ti, &previous); }
    for &ii in &impls { link_impl(prog, ii, &previous); }
}

fn link_trait(prog: &mut Program, ti: usize, previous: &crate::fxhash::HashMap<DefId, ImplTraitInTraitData>) {
    let mut items = Vec::new();
    for method in prog.traits[ti].clone() {
        if previous.contains_key(&method) { continue; }
        items.push(method);
        if prog.def(method).kind != DefKind::AssocFn { continue; }
        let mut synthetics = Vec::new();
        let opaques = RPITVisitor::collect(prog, method);
        for (index, &opaque) in opaques.iter().enumerate() {
            let assoc = associated_type_for_impl_trait_in_trait(prog, method, opaque, index as u32);
            items.push(assoc);
            synthetics.push(assoc);
        }
        prog.associated_types_for_impl_traits.insert(method, synthetics);
    }
    prog.traits[ti] = items;
}

fn link_impl(prog: &mut Program, ii: usize, previous: &crate::fxhash::HashMap<DefId, ImplTraitInTraitData>) {
    prog.impls[ii].items.retain(|d| !previous.contains_key(d));
    let imp = prog.impls[ii].def;
    let def = prog.def(imp);
    let Some(path) = ({ let ast = &prog.files[def.file as usize].ast; ast.as_impl(&ast.item(def.item).kind).and_then(|i| i.trait_) }) else { return; };
    let segs = path_segs(prog, def.file, &path);
    let Some(trait_def) = prog.resolve_mod_path(def.scope, path.global(), &segs, false, false) else { return; };
    if prog.def(trait_def).kind != DefKind::Trait { return; }
    let trait_items = prog.traits[prog.def(trait_def).sub as usize].clone();
    let trait_path = segs.iter().map(|s| prog.syms.str(s.1).to_string()).collect::<Vec<_>>().join("::");
    let mut seen: Vec<(DefId, DefId)> = Vec::new();
    let mut items = Vec::new();
    for item in prog.impls[ii].items.clone() {
        items.push(item);
        let def = prog.def(item);
        // rustc_resolve late.rs:3837-3960 check_trait_item: the item's namespace first,
        // then the other one; missing -> E0407/E0437/E0438, a second impl item for one
        // trait item -> E0201, another kind -> E0323/E0324/E0325.
        let type_ns = |k: DefKind| k == DefKind::AssocTy;
        let lookup = |ty: bool| trait_items.iter().copied().find(|&td| {
            !prog.impl_trait_in_trait_data.contains_key(&td) && prog.def(td).name == def.name && type_ns(prog.def(td).kind) == ty
        });
        let found = lookup(type_ns(def.kind)).or_else(|| lookup(!type_ns(def.kind)));
        let name = prog.syms.str(def.name).to_string();
        let (file, pos) = (def.file, prog.files[def.file as usize].ast.item(def.item).lo);
        let error = match found {
            None => Some(match def.kind {
                DefKind::AssocTy => format!("E0437: type `{name}` is not a member of trait `{trait_path}`"),
                DefKind::AssocConst => format!("E0438: const `{name}` is not a member of trait `{trait_path}`"),
                _ => format!("E0407: method `{name}` is not a member of trait `{trait_path}`"),
            }),
            Some(td) if seen.iter().any(|&(t, _)| t == td) => Some(format!("E0201: duplicate definitions with name `{name}`:")),
            Some(td) if prog.def(td).kind != def.kind => Some(match def.kind {
                DefKind::AssocConst => format!("E0323: item `{name}` is an associated const, which doesn't match its trait `{trait_path}`"),
                DefKind::AssocTy => format!("E0325: item `{name}` is an associated type, which doesn't match its trait `{trait_path}`"),
                _ => format!("E0324: item `{name}` is an associated method, which doesn't match its trait `{trait_path}`"),
            }),
            Some(_) => None,
        };
        if let Some(td) = found { if !seen.iter().any(|&(t, _)| t == td) { seen.push((td, item)); } }
        if let Some(message) = error {
            let e = prog.record_error(file, pos, "resolve", message);
            if !prog.errors.contains(&e) { prog.errors.push(e); }
            continue;
        }
        let Some(trait_item) = found else { continue };
        let kind = def.kind;
        prog.trait_item_def_id.insert(item, trait_item);
        if kind != DefKind::AssocFn { continue; }
        let trait_assocs = prog.associated_types_for_impl_traits.get(&trait_item).cloned().unwrap_or_default();
        let mut synthetics = Vec::new();
        for (index, trait_assoc) in trait_assocs.into_iter().enumerate() {
            let assoc = associated_type_for_impl_trait_in_impl(prog, item, trait_assoc, index as u32);
            items.push(assoc);
            synthetics.push(assoc);
        }
        prog.associated_types_for_impl_traits.insert(item, synthetics);
    }
    prog.impls[ii].items = items;
}

struct RPITVisitor<'a> { prog: &'a Program, file: u32, synthetics: Vec<DefId> }
impl RPITVisitor<'_> {
    fn collect(prog: &Program, method: DefId) -> Vec<DefId> {
        use crate::def_collector::NodeId;
        let def = prog.def(method);
        let ast = &prog.files[def.file as usize].ast;
        let ItemKind::Fn(sig, _, _) = ast.item(def.item).kind else { unreachable!() };
        let sig = ast.fn_sig(sig);
        let mut visitor = RPITVisitor { prog, file: def.file, synthetics: Vec::new() };
        if sig.asyncness { visitor.synthetics.push(prog.node_id_to_def_id[&(def.file, NodeId::AsyncReturn(def.item))]); }
        if let Some(ty) = sig.ret { visitor.visit_ty(ty); }
        visitor.synthetics
    }
    fn visit_ty(&mut self, ty: crate::ast::TyId) {
        use crate::ast::*;
        let ast = &self.prog.files[self.file as usize].ast;
        match *ast.ty(ty) {
            Ty::ImplTrait(bs) => {
                self.synthetics.push(self.prog.node_id_to_def_id[&(self.file, crate::def_collector::NodeId::OpaqueTy(ty))]);
                self.visit_bounds(bs);
            }
            Ty::Ref(_, _, t) | Ty::Ptr(_, t) | Ty::Slice(t) | Ty::Array(t, _) | Ty::Paren(t) => self.visit_ty(t),
            Ty::Tuple(ts) => for &t in ast.list(ts) { self.visit_ty(t); },
            Ty::Path(p) => {
                if let Some(q) = ast.qself(&p) { self.visit_ty(q.ty); }
                for seg in ast.segs(p.segs) { self.visit_args(seg.args()); }
            }
            Ty::Fn(sig) => {
                let sig = ast.fn_ptr(sig);
                for &t in ast.list(sig.params) { self.visit_ty(t); }
                if let Some(t) = sig.ret { self.visit_ty(t); }
            }
            Ty::DynTrait(bs, _) => self.visit_bounds(bs),
            _ => {}
        }
    }
    fn visit_bounds(&mut self, bounds: crate::ast::List<crate::ast::Bound>) {
        let ast = &self.prog.files[self.file as usize].ast;
        for bound in ast.list(bounds) {
            if let crate::ast::Bound::Trait { path, .. } = bound {
                for seg in ast.segs(path.segs) { self.visit_args(seg.args()); }
            }
        }
    }
    fn visit_args(&mut self, args: crate::ast::GenericArgsId) {
        use crate::ast::*;
        let ast = &self.prog.files[self.file as usize].ast;
        match ast.gargs(args) {
            Some(GenericArgs::Angle(args)) => for arg in ast.list(*args) {
                match *arg {
                    GenericArg::Type(t) => self.visit_ty(t),
                    GenericArg::Binding(_, args, t) => { self.visit_args(args); self.visit_ty(t); }
                    GenericArg::Constraint(_, bs) => self.visit_bounds(bs),
                    _ => {}
                }
            },
            Some(GenericArgs::Paren(ps, ret)) => {
                for &t in ast.list(*ps) { self.visit_ty(t); }
                if let Some(t) = ret { self.visit_ty(*t); }
            }
            None => {}
        }
    }
}

fn create_def(prog: &mut Program, method: DefId, index: u32) -> DefId {
    let def = prog.def(method);
    let (parent, method_name) = (def.parent, def.name);
    let key = (parent, DefKind::AssocTy, Some(method_name), index);
    let name = prog.syms.intern(&format!("{{{}#opaque{index}}}", prog.name(method)));
    let def = prog.def(method);
    let def = Def { kind: DefKind::AssocTy, name, parent, file: def.file, item: def.item,
        sub: index, is_pub: def.is_pub, krate: def.krate, scope: def.scope };
    if let Some(&id) = prog.child_definitions.get(&key) {
        prog.defs[id.0 as usize] = def;
        id
    } else {
        let id = prog.add_def(def);
        prog.child_definitions.insert(key, id);
        id
    }
}

fn associated_type_for_impl_trait_in_trait(prog: &mut Program, method: DefId, opaque: DefId, index: u32) -> DefId {
    let id = create_def(prog, method, index);
    prog.impl_trait_in_trait_data.insert(id, ImplTraitInTraitData::Trait { fn_def_id: method, opaque_def_id: opaque });
    prog.opaque_to_rpitit.insert(opaque, id);
    id
}

fn associated_type_for_impl_trait_in_impl(prog: &mut Program, method: DefId, trait_assoc: DefId, index: u32) -> DefId {
    let id = create_def(prog, method, index);
    prog.impl_trait_in_trait_data.insert(id, ImplTraitInTraitData::Impl { fn_def_id: method });
    prog.trait_item_def_id.insert(id, trait_assoc);
    id
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rpitit_synthesis_uses_trait_items_and_return_opaque_order() {
        let dir = std::env::temp_dir().join(format!("relax-rpitit-defs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("lib.rs");
        std::fs::write(&path, "impl Alias for S { async fn second(&self) -> u32 { 2 } fn first(&self) -> u32 { 1 } } use inner::Tr as Alias; struct S; mod inner { pub trait Tr { fn first(&self) -> impl Iterator<Item = impl Copy>; async fn second(&self) -> u32; } }").unwrap();
        let mut prog = Program::new(&std::sync::Arc::new(crate::Session::default()));
        let cfg = crate::cfg::CfgSet::host(&prog.session, &[]);
        prog.load_crate("rpitit", path.to_str().unwrap(), 2021, cfg, vec![]).unwrap();
        prog.resolve_imports();
        assert!(prog.errors.is_empty(), "{:?}", prog.errors);
        let mut trait_assocs = Vec::new();
        let mut impl_assocs = Vec::new();
        for (&d, &info) in &prog.impl_trait_in_trait_data {
            match info {
                ImplTraitInTraitData::Trait { fn_def_id, opaque_def_id } => {
                    assert_eq!(prog.def(d).parent, prog.def(fn_def_id).parent);
                    assert_eq!(prog.opaque_to_rpitit[&opaque_def_id], d);
                    trait_assocs.push(d);
                }
                ImplTraitInTraitData::Impl { fn_def_id } => {
                    let trait_assoc = prog.trait_item_def_id[&d];
                    let ImplTraitInTraitData::Trait { fn_def_id: trait_fn, .. } = prog.impl_trait_in_trait_data[&trait_assoc] else { panic!() };
                    assert_eq!(prog.trait_item_def_id[&fn_def_id], trait_fn);
                    assert_eq!(prog.def(d).parent, prog.def(fn_def_id).parent);
                    impl_assocs.push(d);
                }
            }
        }
        assert_eq!((trait_assocs.len(), impl_assocs.len()), (3, 3));
        let count = prog.defs.len();
        collect(&mut prog);
        assert_eq!(prog.defs.len(), count);
        for d in trait_assocs { assert!(prog.impl_trait_in_trait_data.contains_key(&d)); }
        let files = prog.current_source_files().into_iter().collect();
        prog.recollect_check_crate(0, files).unwrap();
        assert_eq!(prog.impl_trait_in_trait_data.len(), 6);
        for (&d, _) in &prog.impl_trait_in_trait_data { assert!(d.0 as usize >= count); }
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }

    /// collect_loaded (a deferred module load) gives the traits and impls it links again
    /// exactly what the whole-program pass gives them, keeps every other link, and links an
    /// existing impl whose items were left unlinked (its trait path missed before the load).
    #[test]
    fn collect_loaded_links_what_the_full_pass_links() {
        let dir = std::env::temp_dir().join(format!("relax-rpitit-loaded-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("lib.rs");
        std::fs::write(&path, "pub trait Tr { fn first(&self) -> impl Iterator<Item = impl Copy>; async fn second(&self) -> u32; type A; const C: u8; } \
            struct S; struct T; \
            impl Tr for S { type A = u8; const C: u8 = 1; fn first(&self) -> impl Iterator<Item = impl Copy> { [1u8].into_iter() } async fn second(&self) -> u32 { 2 } } \
            impl Tr for T { type A = u16; const C: u8 = 2; fn first(&self) -> impl Iterator<Item = impl Copy> { [1u8].into_iter() } async fn second(&self) -> u32 { 3 } fn extra(&self) {} }").unwrap();
        let mut prog = Program::new(&std::sync::Arc::new(crate::Session::default()));
        let cfg = crate::cfg::CfgSet::host(&prog.session, &[]);
        prog.load_crate("loaded", path.to_str().unwrap(), 2021, cfg, vec![]).unwrap();
        prog.resolve_imports();
        type Snap = (Vec<(DefId, ImplTraitInTraitData)>, Vec<(DefId, DefId)>, Vec<(DefId, Vec<DefId>)>, Vec<(DefId, DefId)>, Vec<Vec<DefId>>, Vec<Vec<DefId>>, usize);
        let snap = |p: &Program| -> Snap {
            let mut a: Vec<_> = p.impl_trait_in_trait_data.iter().map(|(k, v)| (*k, *v)).collect(); a.sort_by_key(|x| x.0);
            let mut b: Vec<_> = p.trait_item_def_id.iter().map(|(k, v)| (*k, *v)).collect(); b.sort_by_key(|x| x.0);
            let mut c: Vec<_> = p.associated_types_for_impl_traits.iter().map(|(k, v)| (*k, v.clone())).collect(); c.sort_by_key(|x| x.0);
            let mut d: Vec<_> = p.opaque_to_rpitit.iter().map(|(k, v)| (*k, *v)).collect(); d.sort_by_key(|x| x.0);
            (a, b, c, d, p.traits.iter().cloned().collect(), p.impls.iter().map(|i| i.items.clone()).collect(), p.defs.len())
        };
        let full = snap(&prog);
        // the E0407 of `extra` stays one error
        let errors = prog.errors.len();
        assert!(errors >= 1, "{:?}", prog.errors);
        let visible = vec![true; prog.crates.len()];
        // nothing added: every link stays (only the impl with the unlinked `extra` is linked again)
        let n = prog.defs.len();
        collect_loaded(&mut prog, n, &visible);
        assert!(snap(&prog) == full);
        assert_eq!(prog.errors.len(), errors);
        // every trait and impl added by the "load": linked again, identically
        collect_loaded(&mut prog, 0, &visible);
        assert!(snap(&prog) == full);
        // an existing impl whose trait path missed: no links, no synthesized items
        let s_impl = prog.impls.iter().position(|i| i.items.iter().any(|d| prog.syms.str(prog.def(*d).name) == "first")).unwrap();
        let items = prog.impls[s_impl].items.clone();
        for d in &items {
            prog.trait_item_def_id.remove(d);
            prog.associated_types_for_impl_traits.remove(d);
            prog.impl_trait_in_trait_data.remove(d);
        }
        let kept: Vec<DefId> = items.iter().copied().filter(|d| prog.def(*d).kind != DefKind::AssocTy || !prog.syms.str(prog.def(*d).name).contains("#opaque")).collect();
        prog.impls[s_impl].items = kept;
        let n = prog.defs.len();
        collect_loaded(&mut prog, n, &visible);
        assert!(snap(&prog) == full);
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }
}

impl crate::crate_image::Codec for ImplTraitInTraitData {
    fn put(&self, w: &mut Vec<u8>) {
        match self {
            Self::Trait { fn_def_id, opaque_def_id } => { 0u8.put(w); fn_def_id.put(w); opaque_def_id.put(w); }
            Self::Impl { fn_def_id } => { 1u8.put(w); fn_def_id.put(w); }
        }
    }
    fn get(r: &mut crate::crate_image::Decode<'_>) -> Option<Self> {
        use crate::crate_image::Codec;
        Some(match u8::get(r)? {
            0 => Self::Trait { fn_def_id: Codec::get(r)?, opaque_def_id: Codec::get(r)? },
            1 => Self::Impl { fn_def_id: Codec::get(r)? }, _ => return None,
        })
    }
}
