//! Resolved name graph checkpoint. No collection, import resolution or body parsing on restore.
use super::*;
use crate::crate_image::{Codec, Decode, FileMeta};
use std::sync::{Arc, OnceLock};

// Same-build data: the definition is Copy and contains only enums and integer IDs.
unsafe impl crate::image::Pod for Def {}

crate::collected_fields!(Def, kind, name, parent, file, item, sub, is_pub, krate, scope);
crate::collected_fields!(Module, def, lexical_parent, normal, parent_mod, krate, file,
    types, values, macros, globs, anon_traits, import_pub);
crate::collected_fields!(ImplData, def, file, item, module, items);
crate::collected_fields!(Import, module, global, segs, bind, glob, is_pub, done, file, pos);

/// Binding tables read in place from the image (ImageMap).
macro_rules! tables { ($op:ident) => { $op!(block_mods, block_owner, item_defs, node_id_to_def_id, legacy_const_args, def_id_to_node_id, impl_trait_in_trait_data, associated_types_for_impl_traits, opaque_to_rpitit, opaque_types_defined_by, trait_item_def_id, def_disambiguators, child_definitions); }; }
/// Record lists read in place from the image (ImageVec).
macro_rules! lists { ($op:ident) => { $op!(mods, impls, imports, variants, traits, file_blocks); }; }

#[derive(Clone, Copy)]
pub(super) struct Start {
    defs: usize, mods: usize, variants: usize, impls: usize, traits: usize,
    imports: usize, files: usize, lines: usize,
}
crate::collected_fields!(Start, defs, mods, variants, impls, traits, imports, files, lines);

/// What a retained prefix loses to its dropped on-demand definitions and gets back when the
/// suffix resolves: the traits and impls (indices) whose RPITIT items are synthesized again,
/// the owners whose legacy const arguments are lowered again, each parent's first dropped
/// AnonConst ordinal, and the retained definitions the declaration tables must collect again.
#[derive(Default)]
pub(crate) struct Relink {
    pub traits: Vec<usize>,
    pub impls: Vec<usize>,
    pub owners: Vec<DefId>,
    ordinals: Vec<(DefId, u32)>,
    fns: Vec<DefId>,
    defs: Vec<DefId>,
}

impl Program {
    pub(super) fn mark_collected_start(&mut self) {
        self.collected_starts.push(Start { defs: self.defs.len(), mods: self.mods.len(),
            variants: self.variants.len(), impls: self.impls.len(), traits: self.traits.len(),
            imports: self.imports.len(), files: self.files.len(), lines: self.lines_loaded });
    }

    /// Verification uses the same symbol numbering, but independently parses/collects every
    /// definition and resolves every binding. Symbols are opaque identities, not queries.
    pub fn seed_collected_symbols(&mut self, saved: &Self) {
        self.syms = Interner::default();
        for s in saved.syms.names() { self.syms.intern(s); }
        for s in &saved.syms.late.lock().unwrap_or_else(|e| e.into_inner()).1 { self.syms.intern_late(s); }
    }

    /// The on-demand definitions of the retained crates that sit after the suffix start
    /// (Program::retain_collected_prefix drops them with the suffix): which are they, and what
    /// must be linked or lowered again so the suffix's resolve recreates them. None: a
    /// definition of a retained crate after the start that is not one of them.
    fn on_demand_tail(&self, crates: usize, start: usize) -> Option<Relink> {
        let mut relink = Relink::default();
        for (i, d) in self.defs.get(start..)?.iter().enumerate() {
            if (d.krate as usize) >= crates { continue; }
            let id = DefId((start + i) as u32);
            match d.kind {
                // a legacy const generic argument (def_collector::lower_legacy_const_owners), created
                // for an owner of the crate once all crates are loaded
                DefKind::AnonConst => {
                    if !matches!(self.def_id_to_node_id.get(&id), Some((_, crate::def_collector::NodeId::AnonConst(_)))) { return None; }
                    let mut owner = d.parent;
                    while owner != NO_DEF && !self.legacy_const_args.contains_key(&owner) { owner = self.def(owner).parent; }
                    if owner == NO_DEF || (owner.0 as usize) >= start
                        || !self.legacy_const_args.get(&owner)?.children.iter().any(|(_, c)| *c == id) { return None; }
                    relink.owners.push(owner);
                    // the parent's ordinal counter goes back to its first dropped child
                    match relink.ordinals.iter_mut().find(|(p, _)| *p == d.parent) {
                        Some((_, first)) => *first = (*first).min(d.sub),
                        None => relink.ordinals.push((d.parent, d.sub)),
                    }
                }
                // a return-position `impl Trait` in a trait or impl method (assoc.rs), synthesized
                // after resolve for the traits and impls of the crate
                DefKind::AssocTy => {
                    let fn_def = match self.impl_trait_in_trait_data.get(&id)? {
                        crate::assoc::ImplTraitInTraitData::Trait { fn_def_id, .. }
                        | crate::assoc::ImplTraitInTraitData::Impl { fn_def_id } => *fn_def_id,
                    };
                    let parent = self.def(d.parent);
                    if (d.parent.0 as usize) >= start || (fn_def.0 as usize) >= start { return None; }
                    match parent.kind {
                        DefKind::Trait => relink.traits.push(parent.sub as usize),
                        DefKind::Impl => relink.impls.push(parent.sub as usize),
                        _ => return None,
                    }
                    relink.fns.push(fn_def);
                    relink.defs.push(d.parent);
                }
                _ => return None,
            }
        }
        for v in [&mut relink.traits, &mut relink.impls] { v.sort_unstable(); v.dedup(); }
        for v in [&mut relink.owners, &mut relink.fns, &mut relink.defs] { v.sort_unstable_by_key(|d| d.0); v.dedup(); }
        Some(relink)
    }

    /// Reuse a certified dependency prefix after a leaf (or dependent suffix) changes.
    /// Its IDs remain stable; only new crates pass through the ordinary loader/collector.
    /// The definitions of the retained crates that the whole build creates after all crates
    /// (legacy const generic arguments, RPITIT items) go with the suffix and are created
    /// again by its resolve in the same order (Relink), with the ids a fresh build gives them.
    pub fn retain_collected_prefix(&mut self, crates: usize) -> Option<usize> {
        let s = *self.collected_starts.get(crates)?;
        if s.mods > self.mods.len() || self.mods.iter().skip(s.mods).any(|m| (m.krate as usize) < crates)
            || self.file_crate.get(s.files..)?.iter().any(|k| (*k as usize) < crates) { return None; }
        let relink = self.on_demand_tail(crates, s.defs)?;
        let names: crate::fxhash::HashSet<String> = self.crates[..crates].iter().map(|c| self.syms.str(c.name).to_string()).collect();
        // A library and its binary can share a name; their macro-table shadowing requires
        // the ordinary loader if either changes.
        if self.crates[crates..].iter().any(|c| names.contains(self.syms.str(c.name))) { return None; }
        self.node_id_to_def_id.retain(|(f, _), d| (*f as usize) < s.files && (d.0 as usize) < s.defs);
        self.legacy_const_args.retain(|d, _| (d.0 as usize) < s.defs);
        for owner in &relink.owners { self.legacy_const_args.remove(owner); }
        Arc::make_mut(&mut self.legacy_const_attrs).retain(|d, _| (d.0 as usize) < s.defs);
        self.def_id_to_node_id.retain(|d, _| (d.0 as usize) < s.defs);
        let kept = |d: &DefId| (d.0 as usize) < s.defs;
        self.impl_trait_in_trait_data.retain(|d, _| kept(d));
        self.associated_types_for_impl_traits.retain(|d, v| kept(d) && v.iter().all(kept));
        self.opaque_to_rpitit.retain(|d, v| kept(d) && kept(v));
        self.opaque_types_defined_by.retain(|d, v| { v.retain(kept); kept(d) });
        self.trait_item_def_id.retain(|d, v| kept(d) && kept(v));
        // the dropped items leave their trait's and impl's lists; the resolve of the suffix
        // synthesizes them again
        for &ti in &relink.traits { self.traits.get_mut(ti)?.retain(kept); }
        for &ii in &relink.impls { self.impls.get_mut(ii)?.items.retain(kept); }
        self.repeat_expr_counts.retain(|d| (d.0 as usize) < s.defs);
        self.collected_definitions.retain(|d| (d.0 as usize) < s.defs);
        self.legacy_call_names.retain(|d, _| (d.0 as usize) < s.defs);
        self.def_disambiguators.retain(|(d, _, _), _| (d.0 as usize) < s.defs);
        for &(parent, first) in &relink.ordinals {
            let key = (parent, DefKind::AnonConst, None);
            if first == 0 { self.def_disambiguators.remove(&key); } else { self.def_disambiguators.insert(key, first); }
        }
        self.child_definitions.retain(|(d, _, _, _), child| (d.0 as usize) < s.defs && (child.0 as usize) < s.defs);
        self.crates.truncate(crates);
        self.load_recipe.truncate(crates);
        self.collected_starts.truncate(crates);
        self.defs = match std::mem::take(&mut self.defs) {
            crate::arena::Arena::Mapped(a) => crate::arena::Arena::Mapped(&a[..s.defs]),
            crate::arena::Arena::Owned(mut a) => { a.truncate(s.defs); a.into() },
        };
        self.mods.truncate(s.mods); self.variants.truncate(s.variants);
        self.impls.truncate(s.impls); self.traits.truncate(s.traits); self.imports.truncate(s.imports);
        self.files.truncate(s.files); self.file_paths.truncate(s.files);
        self.file_crate.truncate(s.files); self.file_blocks.truncate(s.files);
        self.lines_loaded = s.lines;
        self.reload = OnceLock::new();
        self.block_mods.retain(|(f, _), m| (*f as usize) < s.files && (*m as usize) < s.mods);
        self.block_owner.retain(|m, _| (*m as usize) < s.mods);
        self.item_defs.retain(|(f, _), _| (*f as usize) < s.files);
        self.imports_of_module.retain(|m, _| (*m as usize) < s.mods);
        self.part_interned.get_mut().ok()?.retain(|(f, _)| (*f as usize) < s.files);
        self.front_ahead.truncate(crates); self.front_restored.truncate(crates);
        self.front_dep_names.truncate(crates); self.front_probes.truncate(crates);
        self.front_macro_fragments.retain(|f| *f < s.files);
        // the lazy selection's state of the dropped crates (a restored image may carry it)
        self.skipped_files.retain(|m, _| (*m as usize) < s.mods);
        self.deferred_imports.retain(|(i, _)| (*i as usize) < s.imports);
        self.deferred_waits.retain(|m, w| { w.retain(|x| (*x as usize) < s.mods); (*m as usize) < s.mods });
        self.deferred_binds.retain(|(m, _), w| { w.retain(|x| (*x as usize) < s.mods); (*m as usize) < s.mods });
        let table = Arc::make_mut(&mut self.macros);
        table.clear_lookup_cache();
        table.defs.retain(|_, defs| { defs.retain(|d| d.krate.is_empty() || names.contains(&d.krate)); !defs.is_empty() });
        table.aliases.retain(|k, _| names.contains(k));
        table.macro_use.retain(|k, _| names.contains(k));
        table.crates.retain(|k, _| names.contains(k.split('\u{1}').next().unwrap_or(k)));
        table.exp_uses.retain(|k, _| names.contains(k)); table.sealed.retain(|k| names.contains(k));
        self.record_front = true;
        // the suffix crates load into an unresolved program, as at a cold load (run.rs resolves
        // after the last one): with imports_done left set, every define() of the suffix cleared
        // scope_cache, each clear retiring a 512 KB table until a quiescent point -- an app-edit
        // relaunch (48 crates collected again under lazy) grew to 16 GB and was OOM-killed
        self.imports_done = false;
        self.collected_relink = Some(relink);
        Some(s.defs)
    }

    /// The suffix's resolve is about to link and lower the dropped definitions again: what is
    /// left is what the declaration tables must collect again (`collected_recollect`).
    pub(crate) fn take_collected_relink(&mut self) -> Relink {
        let relink = self.collected_relink.take().unwrap_or_default();
        self.collected_recollect = relink.fns.iter().chain(&relink.defs).copied().collect();
        self.collected_recollect.sort_unstable_by_key(|d| d.0);
        relink
    }

    pub fn verify_collected(&self, rebuilt: &Self) {
        macro_rules! compare { ($($f:ident),+) => { $(
            assert!(crate::crate_image::encode(&self.$f) == crate::crate_image::encode(&rebuilt.$f),
                "collected Program {} differs", stringify!($f));
        )+ }; }
        assert!(self.syms.names().eq(rebuilt.syms.names()), "collected symbols differ");
        compare!(file_paths, file_crate, defs, mods, variants, impls, traits, imports,
            block_mods, block_owner, item_defs, prim_defs, prelude_crate, imports_of_module, file_blocks, node_id_to_def_id, legacy_const_args, legacy_const_attrs, def_id_to_node_id, impl_trait_in_trait_data, associated_types_for_impl_traits, opaque_to_rpitit, opaque_types_defined_by, trait_item_def_id, repeat_expr_counts, collected_definitions, def_disambiguators, child_definitions);
        assert_eq!(self.crates.len(), rebuilt.crates.len(), "collected crate count");
        for (a, b) in self.crates.iter().zip(&rebuilt.crates) {
            assert_eq!((a.name, a.root_mod, a.edition, &a.deps, &a.cfg.set, a.proc_macro, &a.root_path),
                (b.name, b.root_mod, b.edition, &b.deps, &b.cfg.set, b.proc_macro, &b.root_path), "collected crate");
        }
        for (i, (a, b)) in self.files.iter().zip(&rebuilt.files).enumerate() {
            assert_eq!(a.source_hash(&self.file_paths[i]), b.source_hash(&rebuilt.file_paths[i]), "collected file identity");
        }
    }

    /// Checkpoints only a completed immutable boot. Runtime body/lookup caches stay lazy.
    /// The lazy selection's state goes with it: the modules whose files are still skipped and
    /// the imports waiting on them, so a restored program loads them later (post-boot loader,
    /// completion race) exactly as the boot that recorded it would have.
    pub fn collected_image(&self) -> Option<Vec<u8>> {
        Some(self.collected_writer()?.finish())
    }

    pub fn collected_writer(&self) -> Option<crate::image::Writer<'_>> {
        if !self.imports_done || !self.errors.is_empty() || !self.pending_macs.is_empty() {
            if crate::preload::trace() { eprintln!("makepad-run collected: Program not recordable: imports {}, errors {}, macros {}", self.imports_done, self.errors.len(), self.pending_macs.len()); if let Some(e) = self.errors.first() { eprintln!("makepad-run collected: first program error: {e}"); } }
            return None;
        }
        // Certify record relationships once, when publishing. The reader verifies the
        // immutable image checksum and its input certificate, not every definition again.
        if self.crates.iter().any(|c| c.root_mod as usize >= self.mods.len())
            || self.defs.iter().any(|d| d.krate as usize >= self.crates.len()
                || d.scope as usize >= self.mods.len() || d.file as usize >= self.files.len()
                || (d.parent != NO_DEF && d.parent.0 as usize >= self.defs.len())) { return None; }
        let mut w = Vec::new();
        4u32.put(&mut w);
        let late = self.syms.late.lock().unwrap_or_else(|e| e.into_inner());
        late.1.len().put(&mut w);
        for s in &late.1 { s.to_string().put(&mut w); }
        drop(late);
        self.crates.len().put(&mut w);
        for c in &self.crates {
            c.name.put(&mut w); c.root_mod.put(&mut w); c.edition.put(&mut w);
            c.deps.put(&mut w); c.cfg.set.put(&mut w); c.proc_macro.put(&mut w);
            c.root_path.put(&mut w); c.context.fingerprint().put(&mut w);
        }
        macro_rules! put { ($($f:ident),+) => { $(self.$f.put(&mut w);)+ }; }
        put!(collected_starts, lines_loaded);
        // Standalone loaders leave the interface prefix at usize::MAX (all files).
        // Persist its concrete length; the image's counts are bounded u32 values.
        self.boot_files.min(self.files.len()).put(&mut w);
        put!(file_paths, file_crate, prim_defs, prelude_crate,
            imports_of_module, front_dep_names, front_probes, front_macro_fragments,
            part_interned, deferred_imports, deferred_waits, deferred_binds, legacy_const_attrs);
        let skipped: Vec<(u32, (String, (String, bool)))> = self.skipped_files.iter()
            .map(|(m, (path, dir, mod_rs))| (*m, (path.clone(), (dir.clone(), *mod_rs)))).collect();
        skipped.put(&mut w);
        self.files.len().put(&mut w);
        for (i, f) in self.files.iter().enumerate() {
            f.source_hash(&self.file_paths[i]).put(&mut w);
            f.edition.put(&mut w); f.src_len.put(&mut w); f.token_input.put(&mut w);
            // length-prefixed: a restore keeps it encoded until a body parse needs it
            let lazy = crate::crate_image::encode(&f.lazy);
            lazy.len().put(&mut w); w.extend_from_slice(&lazy);
        }
        let mut image = crate::image::Writer::default();
        image.add_bytes(0, w);
        match &self.defs {
            crate::arena::Arena::Mapped(defs) => image.add_mapped(1, defs),
            crate::arena::Arena::Owned(defs) => image.add(1, defs),
        }
        self.macros.write_image(&mut image, 2);
        self.syms.write_image(&mut image, 3);
        let mut id = 6;
        macro_rules! table { ($($f:ident),+) => { $(self.$f.write_image(&mut image, &mut id);)+ }; }
        tables!(table);
        macro_rules! list { ($($f:ident),+) => { $(self.$f.write_image(&mut image, &mut id);)+ }; }
        lists!(list);
        self.repeat_expr_counts.write_image(&mut image, &mut id);
        self.collected_definitions.write_image(&mut image, &mut id);
        Some(image)
    }

    /// The running program's heap tables become views of `bytes`, the collected Program image
    /// written from them (`collected_writer`), as a warm restore's are: the heap copies are
    /// moved to `retired` (the caller drops them outside its locks) and an entry decodes on first use. A table that changed since it was written (or
    /// whose entry count differs) stays as it is. `note` hears each table's outcome. Needs
    /// exclusive access to the program, like any mutation.
    pub fn adopt_collected(&mut self, bytes: &'static [u8], retired: &mut Vec<Box<dyn Send>>, mut note: impl FnMut(&'static str, crate::collected::Adopt)) -> Option<()> {
        let image = crate::image::Reader::new(bytes)?;
        // the definition array: the image's rows, if they are the same definitions
        let mapped: &'static [Def] = image.slice(1, 1)?;
        let same = |a: &Def, b: &Def| a.kind == b.kind && a.name == b.name && a.parent == b.parent && a.file == b.file
            && a.item == b.item && a.sub == b.sub && a.is_pub == b.is_pub && a.krate == b.krate && a.scope == b.scope;
        note("defs", if self.defs.is_mapped() { crate::collected::Adopt::Mapped }
            else if self.defs.len() != mapped.len() || !self.defs.iter().zip(mapped).all(|(a, b)| same(a, b)) { crate::collected::Adopt::Mismatch }
            else {
                let n = mapped.len();
                if let crate::arena::Arena::Owned(old) = std::mem::replace(&mut self.defs, crate::arena::Arena::Mapped(mapped)) { retired.push(Box::new(old)); }
                crate::collected::Adopt::Adopted(n)
            });
        let (mut k, mut id) = (6, 6);
        macro_rules! table { ($($f:ident),+) => { $({
            let t = crate::collected::ImageMap::read_image(&image, &mut k, &mut id)?;
            note(stringify!($f), self.$f.adopt(t, retired));
        })+ }; }
        tables!(table);
        macro_rules! list { ($($f:ident),+) => { $({
            let t = crate::collected::ImageVec::read_image(&image, &mut k, &mut id)?;
            note(stringify!($f), self.$f.adopt(t, retired));
        })+ }; }
        lists!(list);
        let t = crate::collected::ImageSet::read_image(&image, &mut k, &mut id)?;
        note("repeat_expr_counts", self.repeat_expr_counts.adopt(t, retired));
        let t = crate::collected::ImageSet::read_image(&image, &mut k, &mut id)?;
        note("collected_definitions", self.collected_definitions.adopt(t, retired));
        (k == image.len()).then_some(())
    }

    pub fn restore_collected(
        session: &Arc<crate::Session>, bytes: &'static [u8], pre: &[crate::preload::PreCrate],
        mut file: impl FnMut(usize, u32, &CfgSet, &mut FileMeta) -> Option<ParsedFile>,
    ) -> Option<Self> {
        let started = std::time::Instant::now();
        let phase = |name: &str| if crate::preload::trace() {
            eprintln!("makepad-run collected: Program {} at {:.1} ms", name, started.elapsed().as_secs_f64() * 1e3);
        };
        let image = crate::image::Reader::new(bytes)?;
        let mut r = Decode::new(image.bytes(0, 0)?);
        if u32::get(&mut r)? != 4 { return None; }
        let mut p = Program::new(session);
        p.syms = Interner::from_image(&image, 3, 3)?;
        for s in Vec::<String>::get(&mut r)? { p.syms.intern_late(&s); }
        phase("symbols");
        let n = usize::get(&mut r)?;
        if n > r.bytes.len() || n != pre.len() { return None; }
        let contexts = crate::source_context::Context::for_crates(pre, p.sources.clone(), session);
        p.load_recipe = pre.to_vec();
        for context in contexts {
            let name = Codec::get(&mut r)?;
            let root_mod = Codec::get(&mut r)?;
            let edition = Codec::get(&mut r)?;
            let deps = Codec::get(&mut r)?;
            let set = Codec::get(&mut r)?;
            let proc_macro = Codec::get(&mut r)?;
            let root_path: String = Codec::get(&mut r)?;
            let selection_hash = u128::get(&mut r)?;
            context.restore_fingerprint(selection_hash);
            p.crates.push(CrateInfo {
                name, root_mod, edition, recursion_limit: 128, deps, cfg: CfgSet { set, session: session.clone() },
                proc_macro, relaxed: OnceLock::new(), root_path, context,
            });
        }
        macro_rules! get { ($($f:ident),+) => { $(p.$f = Codec::get(&mut r)?;)+ }; }
        p.defs = crate::arena::Arena::Mapped(image.slice(1, 1)?);
        p.macros = crate::collected::Deferred::mapped(image.bytes(2, 2)?);
        get!(collected_starts, lines_loaded, boot_files, file_paths, file_crate, prim_defs, prelude_crate,
            imports_of_module, front_dep_names, front_probes, front_macro_fragments,
            part_interned, deferred_imports, deferred_waits, deferred_binds, legacy_const_attrs);
        p.skipped_files = Vec::<(u32, (String, (String, bool)))>::get(&mut r)?.into_iter()
            .map(|(m, (path, (dir, mod_rs)))| (m, (path, dir, mod_rs))).collect();
        let (mut k, mut id) = (6, 6);
        macro_rules! table { ($($f:ident),+) => { $(p.$f = crate::collected::ImageMap::read_image(&image, &mut k, &mut id)?;)+ }; }
        tables!(table);
        macro_rules! list { ($($f:ident),+) => { $(p.$f = crate::collected::ImageVec::read_image(&image, &mut k, &mut id)?;)+ }; }
        lists!(list);
        p.repeat_expr_counts = crate::collected::ImageSet::read_image(&image, &mut k, &mut id)?;
        p.collected_definitions = crate::collected::ImageSet::read_image(&image, &mut k, &mut id)?;
        if k != image.len() { return None; }
        phase("bindings");
        let n = usize::get(&mut r)?;
        if n != p.file_paths.len() || n != p.file_crate.len() || n != p.file_blocks.len() { return None; }
        p.files.reserve(n);
        for i in 0..n {
            let mut meta = FileMeta {
                path: p.file_paths[i].clone(), module: String::new(), hash: Codec::get(&mut r)?,
                edition: Codec::get(&mut r)?, src_len: Codec::get(&mut r)?, token_input: Codec::get(&mut r)?,
                lazy: { let n = usize::get(&mut r)?; crate::parser::FileLazy::mapped(r.take(n)?) }, shared: false,
                used: crate::macros::MacroUse::from_image(Default::default(), Default::default()),
                prep: FilePrep { idents: Vec::new(), blocks: Default::default(), lines: 0 },
            };
            let k = p.file_crate[i];
            let c = p.crates.get(k as usize)?;
            p.files.push(Arc::new(file(i, k, &c.cfg, &mut meta)?));
        }
        phase("files");
        for krate in &mut p.crates {
            krate.recursion_limit = recursion_limit(&p.files[p.mods[krate.root_mod as usize].file as usize]);
        }
        if !r.bytes.is_empty() { return None; }
        p.front_restored = vec![true; p.crates.len()];
        p.front_ahead.resize_with(p.crates.len(), Vec::new);
        p.imports_done = true;
        // the reload index is built from the restored tables by its first user
        p.reload = OnceLock::new();
        Some(p)
    }
}

impl crate::collected::ImageKey for crate::def_collector::NodeId {
    fn image_key(&self) -> u128 {
        use crate::def_collector::NodeId::*;
        let (tag, id) = match *self {
            AnonConst(x) => (0u128, x.0), InlineConst(x) => (1, x.0), OpaqueTy(x) => (2, x.0),
            AsyncReturn(x) => (3, x.0), AsyncBody(x) => (4, x.0), Closure(x) => (5, x.0), Field(x) => (6, x),
        };
        tag << 32 | id as u128
    }
}
impl crate::collected::ImageKey for (DefId, DefKind, Option<Sym>) {
    fn image_key(&self) -> u128 {
        ((self.0.0 as u128) << 64) | (def_kind_key(self.1) << 33) | self.2.map_or(0, |s| s as u128 + 1)
    }
}
impl crate::collected::ImageKey for (DefId, DefKind, Option<Sym>, u32) {
    fn image_key(&self) -> u128 { (self.0, self.1, self.2).image_key() << 32 | self.3 as u128 }
}

fn def_kind_key(kind: DefKind) -> u128 {
    match kind {
        DefKind::Mod => 0,
        DefKind::Fn => 1,
        DefKind::Struct => 2,
        DefKind::Union => 3,
        DefKind::Enum => 4,
        DefKind::Variant => 5,
        DefKind::Const => 6,
        DefKind::Static => 7,
        DefKind::Trait => 8,
        DefKind::Impl => 9,
        DefKind::TypeAlias => 10,
        DefKind::AssocFn => 11,
        DefKind::AssocConst => 12,
        DefKind::AssocTy => 13,
        DefKind::ForeignFn => 14,
        DefKind::ForeignStatic => 15,
        DefKind::ForeignTy => 16,
        DefKind::AnonConst => 17,
        DefKind::OpaqueTy => 18,
        DefKind::Closure => 19,
        DefKind::Field => 20,
        DefKind::MacroRules => 21,
        DefKind::Prim(p) => 22 | ((p as u128) << 8),
    }
}
