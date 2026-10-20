//! Privacy of paths (rustc_resolve `PrivacyError`, reported as E0603): a path segment looked
//! up in a module must name a binding the path's module can see. Item bindings carry their
//! written visibility (`pub(crate)`, `pub(super)`, `pub(in path)`); an import binding is `pub`
//! or private to its module.
use super::*;

pub struct PrivacyError {
    pub name: Sym,
    pub def: DefId,
    pub import: bool,
    pub ext: bool,
    /// not private but ambiguous (E0659): two glob imports bring different items
    pub ambiguous: bool,
    /// the ambiguity is inherited through a glob whose own module is ambiguous: the lint
    /// `ambiguous_glob_imports` (deny by default), not a hard error
    pub lint: bool,
}

impl Program {
    /// Visibility of module `m`'s binding `name` -> `d`, the item's written visibility when the
    /// item is a plain item of `m` in the same crate.
    pub(super) fn precise_vis(&self, m: u32, name: Sym, ns_type: bool, d: DefId) -> BindVis {
        let md = &self.mods[m as usize];
        if !md.import_pub.contains_key(&(name, ns_type)) {
            let def = self.def(d);
            if def.krate == md.krate && def.scope == m && def.item.0 != u32::MAX {
                if let Some((f, it)) = self.files.get(def.file as usize).and_then(|f| f.ast.items.get(def.item.0 as usize).map(|it| (f, it))) {
                    // (generated text -- macro output -- declares what its macro says it does)
                    if it.lo >= f.src_len { return None; }
                    return self.vis_module(def.file, def.scope, &it.vis);
                }
            }
        }
        self.binding_vis(m, name, ns_type, d)
    }

    /// The first segment of `segs[1..]` (looked up in a module) that names a binding module
    /// `from` cannot see. `last_ns`: the namespace of the final segment (None: either, as in a
    /// `use`, where an inaccessible binding counts only when no namespace has an accessible one).
    pub fn path_privacy(&self, from: u32, global: bool, segs: &[(SegKind, Sym)], last_ns: Option<bool>, from_root: bool, in_use: bool) -> Option<PrivacyError> {
        if segs.len() < 2 || !self.crates[self.mods[from as usize].krate as usize].validate {
            return None;
        }
        let mut cur = self.resolve_mod_path(from, global, &segs[..1], from_root, in_use)?;
        for i in 1..segs.len() {
            let (kind, name) = segs[i];
            let d = self.def(cur);
            if d.kind != DefKind::Mod {
                return None;
            }
            match kind {
                SegKind::SelfValue => continue,
                SegKind::Super => {
                    let p = self.mods[d.sub as usize].parent_mod?;
                    cur = self.mods[p as usize].def;
                    continue;
                }
                SegKind::Ident => {}
                _ => return None,
            }
            let m = d.sub;
            let last = i + 1 == segs.len();
            let nss: &[bool] = if !last { &[true] } else {
                match last_ns { Some(true) => &[true], Some(false) => &[false], None => &[true, false] }
            };
            let mut next = None;
            let mut hidden = None;
            let mut visible = false;
            for &ns in nss {
                let mut q = GlobQuery { precise: true, ..Default::default() };
                let Some((fd, v)) = self.lookup_in_module_vis(m, name, ns, &mut q) else { continue };
                if self.vis_visible(v, from) {
                    visible = true;
                    if next.is_none() || ns { next = Some(fd); }
                } else if hidden.is_none() {
                    hidden = Some((fd, ns));
                }
            }
            if visible {
                for &ns in nss {
                    let ambig = self.glob_ambiguity(m, name, ns, from, false);
                    if ambig != 0 {
                        return Some(PrivacyError { name, def: next.unwrap_or(cur), import: true, ext: false, ambiguous: true, lint: ambig == 2 });
                    }
                }
            }
            if !visible {
                if let Some((fd, ns)) = hidden {
                    let flags = self.mods[m as usize].import_pub.get(&(name, ns)).copied();
                    return Some(PrivacyError { name, def: fd, import: flags.is_some(), ext: flags.map_or(false, |f| f & IMPORT_EXTERN != 0), ambiguous: false, lint: false });
                }
            }
            cur = next?;
        }
        None
    }

    /// Whether `name` is found in module `m` only through glob imports that bring different
    /// items (rustc_resolve: a glob binding that conflicts with another glob's is ambiguous
    /// when used; E0659). `from`: the module the name is used from.
    pub fn glob_ambiguous(&self, m: u32, name: Sym, ns_type: bool, from: u32, skip_direct: bool) -> bool {
        self.glob_ambiguity(m, name, ns_type, from, skip_direct) != 0
    }

    /// 0: not ambiguous; 1: two globs of `m` bring different items; 2: only inherited through a
    /// glob whose module is itself ambiguous for `name`.
    pub fn glob_ambiguity(&self, m: u32, name: Sym, ns_type: bool, _from: u32, skip_direct: bool) -> u8 {
        // a checked module: what the replay of rustc's import resolution found
        if let Some(t) = &self.sim_ambiguity {
            if self.own_crates.get(self.mods[m as usize].krate as usize).copied().unwrap_or(false) {
                let md = &self.mods[m as usize];
                if !skip_direct && (if ns_type { md.types.contains_key(&name) } else { md.values.contains_key(&name) }) { return 0; }
                let v = t.get(&(m, name, ns_type)).copied().unwrap_or(0);
                return if skip_direct { v >> 2 } else { v & 3 };
            }
        }
        let key = (m, name, ns_type, skip_direct);
        if let Some(r) = self.glob_amb_cache.get(&key) { return r; }
        let r = self.glob_ambiguity_in(m, name, ns_type, skip_direct, &mut crate::fxhash::HashMap::default());
        if self.imports_done { self.glob_amb_cache.insert(key, r); }
        r
    }

    /// What a glob binds `name` to, for comparison with the module's other globs.
    fn glob_binding(&self, sub: u32, name: Sym, ns_type: bool) -> Option<(DefId, BindVis)> {
        if let Some(r) = self.glob_prec_cache.get(&(sub, name, ns_type)) { return r; }
        let mut q = GlobQuery { precise: true, ..Default::default() };
        let r = self.lookup_in_module_vis(sub, name, ns_type, &mut q);
        if self.imports_done { self.glob_prec_cache.insert((sub, name, ns_type), r); }
        r
    }

    /// Ambiguity is a property of the module's glob imports (rustc records it on the binding when
    /// two globs meet), so the answer is per (module, name, namespace) and cached.
    fn glob_ambiguity_in(&self, m: u32, name: Sym, ns_type: bool, skip_direct: bool, seen: &mut crate::fxhash::HashMap<u32, u8>) -> u8 {
        let md = &self.mods[m as usize];
        if md.globs.is_empty() || (!skip_direct && (if ns_type { md.types.contains_key(&name) } else { md.values.contains_key(&name) })) {
            return 0;
        }
        if let Some(r) = self.glob_amb_cache.get(&(m, name, ns_type, skip_direct)) { return r; }
        // a module still being judged answers 0 (glob cycles)
        if let Some(&r) = seen.get(&m) { return r; }
        seen.insert(m, 0);
        let mut found: Option<DefId> = None;
        let mut out = 0u8;
        for &(g, _) in &md.globs {
            let gd = self.def(g);
            let r = match gd.kind {
                DefKind::Mod => {
                    let r = self.glob_binding(gd.sub, name, ns_type);
                    if r.is_some() && self.glob_ambiguity_in(gd.sub, name, ns_type, false, seen) != 0 { out = out.max(2); }
                    r
                }
                DefKind::Enum => self.lookup_in_container(g, name, false).map(|d| (d, None)),
                _ => None,
            };
            if let Some((d, v)) = r {
                if !self.vis_visible(v, m) {
                    continue;
                }
                match found {
                    None => found = Some(d),
                    Some(f) if f != d => out = 1,
                    _ => {}
                }
            }
        }
        seen.insert(m, out);
        out
    }

    /// A name used on its own: ambiguous when the scope it is found in binds it through globs only.
    pub fn name_ambiguity(&self, from: u32, name: Sym, ns_type: bool, skip_direct: bool) -> Option<PrivacyError> {
        if !self.crates[self.mods[from as usize].krate as usize].validate {
            return None;
        }
        let mut m = from;
        loop {
            let md = &self.mods[m as usize];
            if !skip_direct && if ns_type { md.types.contains_key(&name) } else { md.values.contains_key(&name) } {
                return None;
            }
            let ambig = self.glob_ambiguity(m, name, ns_type, from, skip_direct);
            if ambig != 0 {
                let def = self.lookup_in_module(m, name, ns_type, &mut Vec::new())?;
                return Some(PrivacyError { name, def, import: true, ext: false, ambiguous: true, lint: ambig == 2 });
            }
            // the first scope that binds the name settles it (late resolution reports no ambiguity
            // between a scope's glob and an outer scope)
            if self.sim_ambiguity.as_ref().is_some_and(|t| self.own_crates.get(md.krate as usize).copied().unwrap_or(false) && t.contains_key(&(m, name, ns_type))) {
                return None;
            }
            match md.lexical_parent {
                Some(p) => m = p,
                None => return None,
            }
        }
    }

    /// rustc's `Res::descr` of what a path names.
    pub fn def_descr(&self, d: DefId) -> &'static str {
        match self.def(d).kind {
            DefKind::Mod => "module",
            DefKind::Fn | DefKind::AssocFn | DefKind::ForeignFn => "function",
            DefKind::Struct => "struct",
            DefKind::Union => "union",
            DefKind::Enum => "enum",
            DefKind::Variant => "variant",
            DefKind::Const | DefKind::AssocConst => "constant",
            DefKind::Static | DefKind::ForeignStatic => "static",
            DefKind::Trait => "trait",
            DefKind::TypeAlias | DefKind::AssocTy | DefKind::ForeignTy => "type alias",
            DefKind::MacroRules => "macro",
            _ => "item",
        }
    }

    /// "E0603: function `f` is private" / "module import `n` is private".
    pub fn privacy_message(&self, e: &PrivacyError) -> String {
        let n = self.syms.str(e.name);
        if e.ambiguous {
            format!("E0659: `{n}` is ambiguous")
        } else if e.ext {
            format!("E0603: crate import `{n}` is private")
        } else if e.import {
            format!("E0603: {} import `{n}` is private", self.def_descr(e.def))
        } else {
            format!("E0603: {} `{n}` is private", self.def_descr(e.def))
        }
    }

    /// E0603 for the `use` declarations of the own crates (`own[krate]`).
    pub fn import_privacy_errors(&self, own: &[bool]) -> Vec<String> {
        let mut out = Vec::new();
        for im in &self.imports {
            let krate = self.mods[im.module as usize].krate;
            if !own.get(krate as usize).copied().unwrap_or(false) || im.bound & BOUND_INJECTED != 0 {
                continue;
            }
            let edition = self.crates[krate as usize].edition;
            let mut segs: &[(SegKind, Sym)] = &im.segs;
            let mut ns = if im.glob { Some(true) } else { None };
            if segs.last().map_or(false, |s| s.0 == SegKind::SelfValue) && segs.len() > 1 {
                segs = &segs[..segs.len() - 1];
                ns = Some(true);
            }
            let single = if segs.len() == 1 && segs[0].0 == SegKind::Ident && !im.global && im.bind.is_some() {
                // `use V;`: the name in this module's scope (the crate root before 2018)
                let m = if edition < 2018 { self.crates[krate as usize].root_mod } else { im.module };
                [true, false].into_iter().find_map(|t| self.name_ambiguity(m, segs[0].1, t, true))
            } else {
                None
            };
            if let Some(e) = single.or_else(|| self.path_privacy(im.module, im.global, segs, ns, edition < 2018, true)).filter(|e| !e.lint) {
                out.push(self.record_error(im.file, im.pos, "resolve", self.privacy_message(&e)));
            } else if im.glob && im.bound & BOUND_PREFIX_ONLY == 0 && self.glob_of_own_module(im, segs, edition) {
                out.push(self.record_error(im.file, im.pos, "resolve", "E0432: unresolved import -- cannot glob-import a module into itself".to_string()));
            } else if !im.glob && im.is_pub && !im.ext {
                if let Some(msg) = self.cannot_reexport(im, segs, edition) {
                    out.push(self.record_error(im.file, im.pos, "resolve", msg));
                }
            }
        }
        out
    }

    /// A tuple struct whose constructor module `from` cannot call: a field of it is invisible there
    /// (an import brings only the struct's type namespace then).
    pub fn tuple_ctor_hidden(&self, d: DefId, from: u32) -> bool {
        let def = self.def(d);
        if def.kind != DefKind::Struct || def.item.0 == u32::MAX {
            return false;
        }
        let Some(f) = self.files.get(def.file as usize) else { return false };
        let Some(it) = f.ast.items.get(def.item.0 as usize) else { return false };
        let Some(st) = f.ast.as_struct(&it.kind) else { return false };
        let VariantData::Tuple(l) = st.data else { return false };
        f.ast.list(l).iter().any(|fd| !self.vis_visible(self.vis_module(def.file, def.scope, &fd.vis), from))
    }

    /// `use self::*;` / `use crate::m::*;` inside `m`: a module cannot glob-import itself.
    fn glob_of_own_module(&self, im: &Import, segs: &[(SegKind, Sym)], edition: u16) -> bool {
        match self.resolve_mod_path(im.module, im.global, segs, edition < 2018, true) {
            Some(t) => self.mods[im.module as usize].lexical_parent.is_none() && self.def(t).kind == DefKind::Mod && self.def(t).sub == self.mods[im.module as usize].normal,
            None => false,
        }
    }

    /// The visibility of an item definition as written (None: `pub`).
    pub fn def_item_vis(&self, d: DefId) -> BindVis {
        let def = self.def(d);
        if def.item.0 != u32::MAX {
            if let Some((f, it)) = self.files.get(def.file as usize).and_then(|f| f.ast.items.get(def.item.0 as usize).map(|it| (f, it))) {
                if it.lo >= f.src_len { return None; }
                return self.vis_module(def.file, def.scope, &it.vis);
            }
        }
        if def.is_pub { None } else { Some(self.mods[def.scope as usize].normal) }
    }

    /// E0364 / E0365 (rustc_resolve imports.rs finalize_import): a `pub use` whose every
    /// resolved binding is less visible than the import itself.
    fn cannot_reexport(&self, im: &Import, segs: &[(SegKind, Sym)], edition: u16) -> Option<String> {
        let (_, name) = *segs.last()?;
        if segs.is_empty() || im.segs.last().map_or(false, |s| s.0 != SegKind::Ident) {
            return None;
        }
        let parent = if segs.len() == 1 {
            // `pub use foo as bar;`: an item of this module (of the crate root before 2018)
            let m = if edition < 2018 { self.crates[self.mods[im.module as usize].krate as usize].root_mod } else { im.module };
            self.mods[m as usize].def
        } else {
            self.resolve_mod_path(im.module, im.global, &segs[..segs.len() - 1], edition < 2018, true)?
        };
        // the import's nominal visibility: the `use` item's
        let f = self.files.get(im.file as usize)?;
        let item = f.ast.items.iter().find(|it| it.lo == im.pos && matches!(it.kind, ItemKind::Use(_)))?;
        let import_vis = self.vis_module(im.file, im.module, &item.vis);
        let pd = self.def(parent);
        let mut any_ok = false;
        let mut failed: Option<(bool, BindVis)> = None;
        for ns in [true, false] {
            let found = match pd.kind {
                DefKind::Mod => {
                    let mut q = GlobQuery { precise: true, ..Default::default() };
                    self.lookup_in_module_vis(pd.sub, name, ns, &mut q)
                }
                // a variant is as visible as its enum
                DefKind::Enum => self.lookup_in_container(parent, name, ns).map(|d| (d, self.def_item_vis(parent))),
                _ => None,
            };
            let Some((_, v)) = found else { continue };
            if self.vis_wider(import_vis, v) {
                failed = Some((ns, v));
            } else {
                any_ok = true;
            }
        }
        if any_ok {
            return None;
        }
        let (ns_type, bv) = failed?;
        let n = self.syms.str(name);
        let krate_root = self.crates[self.mods[im.module as usize].krate as usize].root_mod;
        let crate_private = bv == Some(self.mods[krate_root as usize].normal);
        let code = if ns_type { "E0365" } else { "E0364" };
        Some(if crate_private {
            format!("{code}: `{n}` is only public within the crate, and cannot be re-exported outside")
        } else {
            format!("{code}: `{n}` is private, and cannot be re-exported")
        })
    }
}
