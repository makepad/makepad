//! Persistent declaration state. Query/body caches and inference contexts are deliberately
//! absent. An empty signature OnceLock remains empty; checkpointing never asks for a body.
use super::*;
use crate::crate_image::{Codec, Decode};
use crate::collected::ImageMap;

crate::collected_fields!(FnSigT, region_contract, generic_indices, bound_regions, params, ret,
    self_kind, n_generics, n_parent_generics, is_const, rustc_promotable, rustc_intrinsic, header,
    track_caller, target_features, safe_target_features);
crate::collected_fields!(FieldT, name, ty, is_pub, vis_mod);
crate::collected_fields!(VariantT, def, name, shape, fields, discr);
crate::collected_fields!(AdtT, region_contract, generic_indices, flags, region_params,
    is_enum, is_union, n_generics, variants, repr_c, repr_int, repr_align, repr_packed,
    valid_start, valid_end, no_niche, param_maybe_unsized, ext_from, ext_conv);
crate::collected_id!(AdtFlags);
crate::collected_fields!(Destructor, did);
crate::collected_fields!(OpaqueDecl, region_contract, bounds);
crate::collected_fields!(ConstDecl, ty, region_contract, errors, misses);
// Derived variance is rebuilt after declaration publication.
impl Codec for SigParts {
    fn put(&self, w: &mut Vec<u8>) {
        self.sig.put(w); self.self_sized.put(w); self.trait_bounds.put(w);
        self.ty_bounds.put(w); self.errors.put(w);
    }
    fn get(r: &mut Decode<'_>) -> Option<Self> {
        Some(Self { variances: Default::default(), sig: Codec::get(r)?, self_sized: Codec::get(r)?,
            trait_bounds: Codec::get(r)?, ty_bounds: Codec::get(r)?, errors: Codec::get(r)? })
    }
}
crate::collected_fields!(callable::CallableSignature, param, trait_def, signature);
crate::collected_fields!(outlives::RegionContract, callable_signatures, regions, predicates,
    caller_predicates, assumed_wf, type_names, own_types, self_ty, trait_types, trait_regions);

crate::collected_fields!(crate::types::variance::GenericIndices, lifetimes, types_consts, consts);
crate::collected_enum!(crate::types::variance::V; 0 => Bi, 1 => Co, 2 => Contra, 3 => Inv);
crate::collected_enum!(outlives::OutlivesPredicate; 0 => Region(a,b), 1 => Type(t,r));
crate::collected_enum!(TyKey; 0 => Prim(p), 1 => Adt(d), 2 => Slice, 3 => Array,
    4 => Str, 5 => Ref, 6 => Ptr, 7 => Tuple, 8 => FnPtr, 9 => Dyn(d), 10 => Other);
impl crate::collected::ImageKey for TyKey {
    fn image_key(&self) -> u128 {
        let (tag, payload) = match *self {
            TyKey::Prim(p) => (0u128, p as u128), TyKey::Adt(d) => (1, d.0 as u128),
            TyKey::Slice => (2, 0), TyKey::Array => (3, 0), TyKey::Str => (4, 0), TyKey::Ref => (5, 0),
            TyKey::Ptr => (6, 0), TyKey::Tuple => (7, 0), TyKey::FnPtr => (8, 0),
            TyKey::Dyn(d) => (9, d.0 as u128), TyKey::Other => (10, 0),
        };
        tag << 32 | payload
    }
}
crate::collected_enum!(VariantDiscr; 0 => Explicit(d), 1 => Relative(n));
/// Keep the schema next to the collector. Every field here is declaration state, as opposed
/// to a derived query memo. Adding collection state requires updating these lists.
macro_rules! fields {
    ($op:ident) => { $op!(lang_items, async_fns, future_defs,
        opaques, copy_trait, errors); };
}
/// Declaration tables decoded only when demanded from the collected image.
macro_rules! tables {
    ($op:ident) => { $op!(sigs, ctor_sigs, adts, adt_destructors, destructor_impls,
        nominal_region_contracts, impl_self, impl_trait,
        inherent, trait_impls, trait_impls_of, impls_by_key, impl_trait_args, impl_trait_regions,
        impl_trait_bounds, trait_closure); };
}

impl Tcx {
    /// Invalidate declaration owners in the changed suffix, preserving the independent
    /// dependency graph and its type IDs. Derived query caches were never in the image.
    pub fn retain_collected_prefix(&mut self, first: usize) {
        self.declarations.retain_prefix(first);
        let keep = |d: &DefId| (d.0 as usize) < first;
        let impls: Vec<_> = self.impl_self.keys().filter(|d| !keep(d)).copied().collect();
        self.uncollect_impls(&impls);
        self.inherent.retain(|_, v| !v.is_empty()); self.trait_impls.retain(|_, v| !v.is_empty());
        let mut gone: Vec<_> = self.sigs.keys().chain(self.adts.keys())
            .chain(self.impl_self.keys()).chain(self.trait_closure.keys()).filter(|d| !keep(d)).copied().collect();
        gone.extend(self.const_tys.keys().filter(|d| !keep(d)));
        self.invalidate_check(&gone);
        self.impl_trait_args.retain(|d, _| keep(d)); self.impl_trait_regions.retain(|d, _| keep(d));
        self.impl_trait_bounds.retain(|d, _| keep(d));
        self.nominal_region_contracts.retain(|d, _| keep(d)); self.contracts_waiting.retain(|d| keep(d));
        self.ctor_sigs.retain(|d, _| keep(d)); self.opaques.retain(|d, _| keep(d));
        self.adt_destructors.retain(|d, v| keep(d) && keep(&v.did));
        self.destructor_impls.retain(|d, v| keep(d) && keep(v));
        self.async_fns = self.async_fns.snapshot().into_iter().filter(|(d, _)| keep(d))
            .fold(Default::default(), |out, (d, v)| { out.insert(d, v); out });
        for d in &mut self.lang_items.items { if d.as_ref().is_some_and(|d| !keep(d)) { *d = None; } }
        self.lang_items.by_def.retain(|d, _| keep(d));
    }

    pub fn collect_collected_suffix(&mut self, prog: &Program, first: usize) {
        // The retained methods and traits that lost a dropped item come first: their signature
        // cells are empty again and their region contracts follow the recreated items.
        let mut defs: Vec<_> = prog.collected_recollect.iter().copied()
            .filter(|d| matches!(prog.def(*d).kind, DefKind::AssocFn | DefKind::Trait)).collect();
        defs.extend((first..prog.defs.len()).map(|d| DefId(d as u32)));
        self.collect_added(prog, &defs, true);
        for d in defs {
            if prog.def(d).kind == DefKind::Trait {
                let mut all = Vec::new(); let mut todo = vec![d];
                while let Some(x) = todo.pop() { if !all.contains(&x) { all.push(x); todo.extend(self.trait_supers(prog, x)); } }
                self.trait_closure.insert(d, all);
            }
        }
        self.flush_errs();
        self.seal_collected_diagnostics();
        self.publish_collected();
    }

    pub fn verify_collected(&self, saved_prog: &Program, prog: &Program, rebuilt: &Self) {
        let mut a_keys: Vec<_> = self.sigs.keys().copied().collect(); a_keys.sort_by_key(|d| d.0);
        let mut b_keys: Vec<_> = rebuilt.sigs.keys().copied().collect(); b_keys.sort_by_key(|d| d.0);
        assert_eq!(a_keys, b_keys, "collected signature definitions differ");
        for d in a_keys {
            // Demand the same declarations on both sides, never their bodies.
            if self.sigs[&d].get().is_some() || rebuilt.sigs[&d].get().is_some() {
                let a = self.signature_parts(saved_prog, d);
                let b = rebuilt.signature_parts(prog, d);
                let encode = |p: Option<&SigParts>| {
                    let mut w = Vec::new();
                    p.is_some().put(&mut w);
                    if let Some(p) = p {
                        // Variance is a derived memo; collection leaves it lazy.
                        p.sig.put(&mut w); p.self_sized.put(&mut w);
                        p.trait_bounds.put(&mut w);
                        p.ty_bounds.put(&mut w); p.errors.put(&mut w);
                    }
                    w
                };
                assert_eq!(self.tys.with_semantic_ids(|| encode(a)), rebuilt.tys.with_semantic_ids(|| encode(b)),
                    "collected signature differs: {}", prog.def_path(d));
            }
        }
        macro_rules! compare { ($($f:ident),+) => { $(
            assert!(self.tys.with_semantic_ids(|| crate::crate_image::encode(&self.$f)) ==
                rebuilt.tys.with_semantic_ids(|| crate::crate_image::encode(&rebuilt.$f)),
                "collected Tcx {} differs", stringify!($f));
        )+ }; }
        let mut a_keys: Vec<_> = self.const_tys.declaration_keys().collect(); a_keys.sort_by_key(|d| d.0);
        let mut b_keys: Vec<_> = rebuilt.const_tys.declaration_keys().collect(); b_keys.sort_by_key(|d| d.0);
        assert_eq!(a_keys, b_keys, "collected constant definitions differ");
        for d in a_keys {
            // as for signatures: the same declarations on both sides, only those demanded
            if self.const_tys.declaration(&d).unwrap().get().is_some() || rebuilt.const_tys.declaration(&d).unwrap().get().is_some() {
                // Lower before entering semantic encoding: lowering may intern new types.
                let a = self.const_decl(saved_prog, d, self.const_tys.declaration(&d).unwrap());
                let b = rebuilt.const_decl(prog, d, rebuilt.const_tys.declaration(&d).unwrap());
                let encode = |decl: &ConstDecl| {
                    let mut w = Vec::new();
                    decl.ty.put(&mut w); decl.region_contract.put(&mut w); decl.errors.put(&mut w);
                    w
                };
                assert_eq!(self.tys.with_semantic_ids(|| encode(a)), rebuilt.tys.with_semantic_ids(|| encode(b)),
                    "collected constant differs: {}", prog.def_path(d));
            }
        }
        {
            // A fed type is a query side effect (lowering an anonymous constant's expected type
            // feeds it on demand), so the set depends on what was demanded before the image was
            // recorded: the rebuild at boot has the entries its boot demanded, the image any the
            // recording run demanded later (also without any edit: the complete restore of an
            // unchanged program has them). What must hold: every entry of the rebuild is in the
            // image with the same type, and every other entry of the image names the same
            // definition on both sides (a prefix restore keeps no entry of a dropped definition).
            let (fa, fb) = (self.const_tys.fed_types(), rebuilt.const_tys.fed_types());
            let mut extra: Vec<DefId> = fa.keys().filter(|d| !fb.contains_key(d)).copied().collect();
            extra.sort_by_key(|d| d.0);
            let identity = |p: &Program, d: DefId| (p.def(d).kind, p.def(d).sub, p.def_path(d));
            let moved: Vec<_> = extra.iter().filter(|d| {
                (d.0 as usize) >= prog.defs.len() || identity(saved_prog, **d) != identity(prog, **d)
            }).take(8).map(|d| format!("{}={}", d.0, saved_prog.def_path(*d))).collect();
            let missing: Vec<_> = fb.keys().filter(|d| !fa.contains_key(d)).take(8).map(|d| format!("{}={}", d.0, prog.def_path(*d))).collect();
            assert!(moved.is_empty() && missing.is_empty(),
                "collected anonymous constant types differ: image has {} entries, rebuild {}; not the same definition in the rebuild {:?}; missing from the image {:?}",
                fa.len(), fb.len(), moved, missing);
            let common = |m: &crate::fxhash::HashMap<DefId, TyId>| {
                let mut v: Vec<_> = m.iter().filter(|(d, _)| fb.contains_key(d)).map(|(d, t)| (*d, *t)).collect();
                v.sort_by_key(|(d, _)| d.0); v
            };
            let (ca, cb) = (common(&fa), common(&fb));
            assert_eq!(self.tys.with_semantic_ids(|| crate::crate_image::encode(&ca)),
                rebuilt.tys.with_semantic_ids(|| crate::crate_image::encode(&cb)),
                "collected anonymous constant types differ");
        }
        compare!(ctor_sigs, adts, lang_items, adt_destructors, destructor_impls,
            nominal_region_contracts, contracts_waiting, impl_self, impl_trait,
            inherent, trait_impls, impls_by_key, impl_trait_args,
            impl_trait_regions, impl_trait_bounds, trait_closure, copy_trait);
        assert_eq!(self.collected_errors, rebuilt.collected_errors, "collected declaration diagnostics differ");
    }

    /// Only declaration diagnostics decide whether collection completed cleanly. Later
    /// lazy/speculative queries can report errors for unused items; these are not a failure
    /// of the collected graph. SigParts keeps its own diagnostics for subsequent readers.
    pub(super) fn seal_collected_diagnostics(&mut self) {
        let mut errors = self.errors.clone();
        errors.extend(self.err_sink.get_mut().unwrap_or_else(|e| e.into_inner()).iter().cloned());
        self.collected_errors = Some(errors);
    }

    pub fn collected_image(&self) -> Option<Vec<u8>> {
        self.collected_image_with(&|| false)
    }

    /// `stop`: asked between the tables; yes drops the image (None) -- the recorder yields the
    /// unit lock to a waiting main thread (relax-compiler cache/collected.rs).
    pub fn collected_image_with(&self, stop: &dyn Fn() -> bool) -> Option<Vec<u8>> {
        Some(self.collected_writer_with(stop)?.finish())
    }

    pub fn collected_writer_with(&self, stop: &dyn Fn() -> bool) -> Option<crate::image::Writer<'_>> {
        let errors = self.collected_errors.as_ref()?;
        if !errors.is_empty() { return None; }
        let mut w = Vec::new();
        7u32.put(&mut w);
        macro_rules! put_one {
            (errors) => { errors.put(&mut w) };
            ($f:ident) => { self.$f.put(&mut w) };
        }
        macro_rules! put { ($($f:ident),+) => { $(put_one!($f);)+ }; }
        fields!(put);
        let mut image = crate::image::Writer::default();
        image.add_bytes(0, w);
        if stop() { return None; }
        image.add_image(1, std::sync::Arc::new(self.tys.collected_writer()?));
        let mut id = 2;
        macro_rules! table { ($($f:ident),+) => { $(if stop() { return None; } self.$f.write_image(&mut image, &mut id);)+ }; }
        tables!(table);
        self.const_tys.write_image(&mut image, &mut id);
        self.contracts_waiting.write_image(&mut image, &mut id);
        Some(image)
    }

    /// As Program::adopt_collected, for the declaration tables and type interner of the Tcx image
    /// `bytes` (`collected_writer_with`).
    pub fn adopt_collected(&mut self, bytes: &'static [u8], retired: &mut Vec<Box<dyn Send>>, mut note: impl FnMut(&'static str, crate::collected::Adopt)) -> Option<()> {
        let image = crate::image::Reader::new(bytes)?;
        // (the interner's mapped rows are the image's section 1; its arena chunks below them are freed)
        note("types", self.tys.adopt_collected(image.bytes(1, 1)?));
        let (mut k, mut id) = (2, 2);
        macro_rules! table { ($($f:ident),+) => { $({
            let t = ImageMap::read_image(&image, &mut k, &mut id)?;
            note(stringify!($f), self.$f.adopt(t, retired));
        })+ }; }
        tables!(table);
        let t = ConstTypes::read_image(&image, &mut k, &mut id)?;
        note("const_tys", self.const_tys.adopt(t, retired));
        let t = crate::collected::ImageSet::read_image(&image, &mut k, &mut id)?;
        note("contracts_waiting", self.contracts_waiting.adopt(t, retired));
        (k == image.len()).then_some(())
    }

    pub fn restore_collected(bytes: &'static [u8], files: usize, crates: usize) -> Option<Self> {
        let image = crate::image::Reader::new(bytes)?;
        let mut r = Decode::new(image.bytes(0, 0)?);
        if u32::get(&mut r)? != 7 { return None; }
        let mut t = Self::with_types(Types::restore_collected(image.bytes(1, 1)?)?);
        macro_rules! get { ($($f:ident),+) => { $(t.$f = Codec::get(&mut r)?;)+ }; }
        fields!(get);
        if !r.bytes.is_empty() { return None; }
        let (mut k, mut id) = (2, 2);
        macro_rules! table { ($($f:ident),+) => { $(t.$f = ImageMap::read_image(&image, &mut k, &mut id)?;)+ }; }
        tables!(table);
        t.const_tys = ConstTypes::read_image(&image, &mut k, &mut id)?;
        t.contracts_waiting = crate::collected::ImageSet::read_image(&image, &mut k, &mut id)?;
        if k != image.len() { return None; }
        // Epochs and derived variance/query memos belong to this process. Re-publish the
        // declaration input once, just as collect does, rather than persisting boot counts.
        t.queries.register_files(files);
        t.queries.register_crates(crates);
        t.collected_errors = Some(t.errors.clone());
        t.publish_collected();
        Some(t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round38_collected_native_declarations_restore() {
        let session = std::sync::Arc::new(crate::Session::default());
        let root = std::env::temp_dir().join(format!("n01-collected-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join("lib.rs");
        std::fs::write(&source, r#"#![no_std]
            #[lang = "sized"] pub trait Sized {}
            pub struct Array<'a, const N: usize> { pub value: &'a [u8; N] }
            pub trait View { fn view<'a>(&'a self) -> impl Sized; }
            pub fn borrow<'a, const N: usize>(x: &'a Array<'a, N>) -> &'a [u8; N] { x.value }
            pub fn closure() { let _f = |x: u8| x; }
        "#).unwrap();
        let mut prog = Program::new(&session);
        prog.load_crate("fixture", source.to_str().unwrap(), 2021, crate::cfg::CfgSet::host(&session, &[]), vec![]).unwrap();
        prog.resolve_imports();
        assert!(prog.errors.is_empty(), "{:?}", prog.errors);
        let mut tcx = Tcx::new();
        tcx.collect(&prog);
        let def = DefId(prog.defs.iter().position(|d| prog.syms.str(d.name) == "borrow").unwrap() as u32);
        let _ = tcx.fn_item_sig(&prog, def).unwrap();
        let program_image = prog.collected_image().unwrap();
        let types_image = tcx.collected_image().unwrap();
        let pre = [crate::preload::PreCrate { name: "fixture".into(),
            root: source.to_str().unwrap().into(), edition: 2021, cfg: crate::cfg::CfgSet::host(&session, &[]),
            deps: vec![], proc_macro: false, image_deps: vec![], image_dep_names: vec![] }];
        let restored_prog = Program::restore_collected(&session, Box::leak(program_image.into_boxed_slice()), &pre,
            |_, _, _, meta| prog.file_paths.iter().position(|path| *path == meta.path).and_then(|i| crate::parser::parse_owned(&session, prog.files[i].src.to_vec(), 2021).ok())).unwrap();
        let restored_tcx = Tcx::restore_collected(Box::leak(types_image.into_boxed_slice()), restored_prog.files.len(), restored_prog.crates.len()).unwrap();
        prog.verify_collected(&restored_prog);
        tcx.verify_collected(&prog, &restored_prog, &restored_tcx);
        // the running tables adopt the images written from them: same declarations, read in place
        {
            let mut retired: Vec<Box<dyn Send>> = Vec::new();
            let mut outcomes = Vec::new();
            let (program_bytes, types_bytes): (&'static [u8], &'static [u8]) = (
                Box::leak(prog.collected_image().unwrap().into_boxed_slice()), Box::leak(tcx.collected_image().unwrap().into_boxed_slice()));
            let before = tcx.fn_item_sig(&prog, def).unwrap().bound_regions.clone();
            prog.adopt_collected(program_bytes, &mut retired, |n, how| outcomes.push((n, how))).unwrap();
            tcx.adopt_collected(types_bytes, &mut retired, |n, how| outcomes.push((n, how))).unwrap();
            assert!(outcomes.iter().any(|(n, how)| *n == "sigs" && matches!(how, crate::collected::Adopt::Adopted(_))), "{outcomes:?}");
            assert!(outcomes.iter().any(|(n, how)| *n == "item_defs" && matches!(how, crate::collected::Adopt::Adopted(_))), "{outcomes:?}");
            assert!(outcomes.iter().all(|(_, how)| !matches!(how, crate::collected::Adopt::Dirty | crate::collected::Adopt::Mismatch)), "{outcomes:?}");
            prog.verify_collected(&restored_prog);
            tcx.verify_collected(&prog, &restored_prog, &restored_tcx);
            assert_eq!(tcx.fn_item_sig(&prog, def).unwrap().bound_regions, before);
        }
        assert!(!restored_prog.node_id_to_def_id.is_empty());
        assert!(!restored_prog.impl_trait_in_trait_data.is_empty());
        assert_eq!(restored_tcx.fn_item_sig(&restored_prog, def).unwrap().bound_regions,
            tcx.fn_item_sig(&prog, def).unwrap().bound_regions);
        std::fs::remove_dir_all(root).unwrap();
    }
}

/// Const/static declarations and anonymous types share one mapped definition table.
/// The overlay contains only anonymous types fed by shared lowering; checkpointing folds
/// it into that table. Undemanded declaration cells stay empty across an image round trip.
#[derive(Default)]
pub(super) struct ConstTypes {
    image: ImageMap<DefId, ConstType>,
    added: crate::conc::ShardMap<DefId, TyId>,
}

#[derive(Clone)]
enum ConstType {
    Declaration(std::sync::OnceLock<Box<ConstDecl>>),
    Fed(TyId),
}
crate::collected_enum!(ConstType; 0 => Declaration(cell), 1 => Fed(ty));

impl ConstTypes {
    pub(super) fn declaration(&self, d: &DefId) -> Option<&std::sync::OnceLock<Box<ConstDecl>>> {
        match self.image.get(d)? { ConstType::Declaration(cell) => Some(cell), ConstType::Fed(_) => None }
    }
    pub(super) fn insert_declaration(&mut self, d: DefId) {
        self.image.insert(d, ConstType::Declaration(Default::default()));
    }
    pub(super) fn fed_type(&self, d: &DefId) -> Option<TyId> {
        self.added.get(d).or_else(|| match self.image.get(d)? {
            ConstType::Fed(ty) => Some(*ty), ConstType::Declaration(_) => None,
        })
    }
    pub(super) fn feed(&self, d: DefId, ty: TyId) { self.added.insert(d, ty); }
    fn declaration_keys(&self) -> impl Iterator<Item = DefId> + '_ {
        self.image.iter().filter_map(|(d, v)| matches!(v, ConstType::Declaration(_)).then_some(*d))
    }
    fn fed_types(&self) -> crate::fxhash::HashMap<DefId, TyId> {
        let mut out: crate::fxhash::HashMap<_, _> = self.image.iter().filter_map(|(d, v)| match v {
            ConstType::Fed(ty) => Some((*d, *ty)), ConstType::Declaration(_) => None,
        }).collect();
        out.extend(self.added.snapshot()); out
    }
    fn keys(&self) -> impl Iterator<Item = DefId> + '_ {
        self.image.keys().copied().chain(self.added.snapshot().into_keys())
    }
    pub(super) fn retain(&mut self, mut keep: impl FnMut(&DefId) -> bool) {
        self.image.retain(|d, _| keep(d)); self.added.retain(|d, _| keep(d));
    }
    pub(super) fn reset_declarations_with_misses(&mut self) {
        // Include persisted misses in entries not yet decoded in this process.
        for value in self.image.values_mut() {
            if let ConstType::Declaration(cell) = value {
                if cell.get().is_some_and(|decl| !decl.misses.is_empty()) { *cell = Default::default(); }
            }
        }
    }
    fn write_image(&self, image: &mut crate::image::Writer<'_>, id: &mut u32) {
        let added = self.added.snapshot();
        if added.is_empty() { self.image.write_image(image, id); return; }
        let mut table = self.image.clone();
        table.extend(added.into_iter().map(|(d, ty)| (d, ConstType::Fed(ty))));
        table.write_image(image, id);
    }
    fn read_image(image: &crate::image::Reader, k: &mut usize, id: &mut u32) -> Option<Self> {
        Some(Self { image: ImageMap::read_image(image, k, id)?, added: Default::default() })
    }
    /// As ImageMap::adopt; fed types the image folded in make the written table a copy, so
    /// a table with any stays as it is.
    fn adopt(&mut self, image: Self, retired: &mut Vec<Box<dyn Send>>) -> crate::collected::Adopt {
        if !self.added.snapshot().is_empty() { return crate::collected::Adopt::Dirty; }
        self.image.adopt(image.image, retired)
    }
}
