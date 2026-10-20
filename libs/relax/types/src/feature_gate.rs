//! Feature gates of a checked crate on a stable compiler (rustc 1.97.1): E0658 for unstable
//! syntax and attributes used without `#![feature(..)]` (rustc_ast_passes/src/feature_gate.rs
//! `PostExpansionVisitor`, rustc_feature builtin_attrs.rs, the `cfg` gates of
//! rustc_attr_parsing/src/attributes/cfg.rs), E0557
//! (a removed feature), E0734 (stability attributes outside the standard library).
//!
//! A check-time diagnostic only: `check_crate` runs for the crates whose errors are reported
//! (compiler/src/check.rs) and for no other; std itself uses features and is never walked.
//! The walk follows rustc's: a node whose `cfg` is false is not visited, `cfg_attr` is expanded
//! first, and what a macro expansion wrote is not judged (an expansion's tokens carry no
//! `allow_internal_unstable` information, so only source-written tokens are).

use crate::ast::*;
use crate::cfg::{self, Src};
use crate::features::{Features, GATED_ATTRS, GATED_CFGS};
use crate::lexer::T;
use crate::program::{DefKind, Program};

/// Where an item sits (rustc's `AssocCtxt` / foreign items).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Ctx {
    Free,
    Trait,
    Impl,
    Foreign,
}

pub fn check_crate(prog: &Program, tcx: &crate::tcx::Tcx, krate: u32) -> Vec<String> {
    let info = &prog.crates[krate as usize];
    let feats = info.features(prog);
    let root_file = prog.mods[info.root_mod as usize].file;
    let mut w = Gates::new(prog, tcx, krate, root_file, feats);
    w.crate_features();
    w.cfg_scan();
    w.root_attrs();
    let pf = &prog.files[root_file as usize];
    let items: Vec<ItemId> = pf.ast.root_items.iter().copied().collect();
    w.items(&items, Ctx::Free);
    w.errors
}

struct Gates<'a> {
    prog: &'a Program,
    tcx: &'a crate::tcx::Tcx,
    /// the module whose scope resolves the names of the item being walked
    scope: u32,
    krate: u32,
    file: u32,
    pf: &'a crate::parser::ParsedFile,
    ast: &'a Ast,
    src: Src<'a>,
    cfg: &'a cfg::CfgSet,
    feats: &'a Features,
    /// the innermost position recorded so far (types and patterns keep none of their own)
    cur_pos: u32,
    /// names a body's lexical ribs bind: (name, is a type parameter) -- for `future_proof_import`
    binders: Vec<(String, bool)>,
    errors: Vec<String>,
}

impl<'a> Gates<'a> {
    fn new(prog: &'a Program, tcx: &'a crate::tcx::Tcx, krate: u32, file: u32, feats: &'a Features) -> Gates<'a> {
        let pf = &*prog.files[file as usize];
        Gates { prog, tcx, scope: prog.crates[krate as usize].root_mod, krate, file, pf, ast: &pf.ast, src: prog.src(file), cfg: &prog.crates[krate as usize].cfg, feats, cur_pos: 0, binders: Vec::new(), errors: Vec::new() }
    }

    /// A walker over another file of the crate (an out-of-line module), sharing nothing.
    fn in_file(&self, file: u32) -> Gates<'a> {
        Gates::new(self.prog, self.tcx, self.krate, file, self.feats)
    }

    /// Tokens the crate's author wrote: the file's text, fragments substituted from it, and the
    /// body of a `macro_rules!` defined in this crate (rustc judges those spans). What a
    /// built-in macro or another crate's macro produced is not (it may carry
    /// `allow_internal_unstable`).
    fn is_source(&self, pos: u32) -> bool {
        matches!(self.pf.expansion_kind(pos), crate::parser::ExpnKind::Root | crate::parser::ExpnKind::LocalRules)
    }

    fn tok_pos(&self, i: usize) -> u32 {
        self.src.tok(i).lo
    }

    fn tok_is(&self, i: usize, s: &[u8]) -> bool {
        cfg::tok_str(self.src, i) == s
    }

    /// rustc `gate!`: E0658 unless the crate enabled `feature`.
    fn gate(&mut self, feature: &str, pos: u32, msg: &str) {
        if self.feats.has(feature) || !self.is_source(pos) {
            return;
        }
        self.errors.push(self.prog.record_error(self.file, pos, "typeck", format!("E0658: {}", msg)));
    }

    fn uncoded(&mut self, pos: u32, msg: &str) {
        if !self.is_source(pos) { return; }
        self.errors.push(self.prog.record_uncoded_error(self.file, pos, "typeck", msg.to_string()));
    }

    fn coded(&mut self, code: &str, pos: u32, msg: &str) {
        if !self.is_source(pos) { return; }
        self.errors.push(self.prog.record_error(self.file, pos, "typeck", format!("{}: {}", code, msg)));
    }

    // ---------------------------------------------------------------- the crate

    /// rustc_expand config.rs `features`: E0557 for a removed feature. (E0554, `#![feature]`
    /// on a stable channel, is not reported: the suite's tests run with unstable features
    /// allowed -- RUSTC_BOOTSTRAP -- and Relax has no release channel.)
    fn crate_features(&mut self) {
        let removed: Vec<(u32, String)> = self.feats.declared.iter().filter(|d| d.removed).map(|d| (d.pos, d.name.clone())).collect();
        for (pos, name) in removed {
            self.coded("E0557", pos, &format!("feature has been removed: `{}`", name));
        }
    }

    fn root_attrs(&mut self) {
        let attrs: Vec<Attr> = self.ast.root_attrs.iter().copied().collect();
        self.attr_list(&attrs);
    }

    // ---------------------------------------------------------------- attributes

    /// The attributes of a node: `cfg_attr` expanded (its predicate's cfg gates checked), the
    /// node's `cfg` predicates gated and judged. None: the node is cfg'd out (rustc strips it
    /// before the feature-gate walk, so its other attributes are never looked at).
    fn attrs(&mut self, list: List<Attr>) -> Option<Vec<Attr>> {
        if list.is_empty() { return Some(Vec::new()); }
        let raw: Vec<Attr> = self.ast.list(list).to_vec();
        self.attr_list(&raw)
    }

    fn attr_list(&mut self, raw: &[Attr]) -> Option<Vec<Attr>> {
        let mut flat = Vec::with_capacity(raw.len());
        self.expand(raw, &mut flat);
        let mut active = true;
        for a in &flat {
            if self.attr_is(a, b"cfg") {
                self.cfg_gates(a.toks.lo as usize + 2, a.toks.hi as usize - 1);
                if !cfg::active(self.src, std::slice::from_ref(a), self.cfg) { active = false; }
            }
        }
        if !active { return None; }
        for a in &flat { self.check_attr(a); }
        Some(flat)
    }

    /// `cfg_attr(pred, attrs)` expanded, checking `pred`'s cfg gates on the way.
    fn expand(&mut self, raw: &[Attr], out: &mut Vec<Attr>) {
        for a in raw {
            let lo = a.toks.lo as usize;
            let hi = a.toks.hi as usize;
            if hi > lo + 2 && self.attr_is(a, b"cfg_attr") && self.src.tok(lo + 1).kind == T::OpenParen {
                let end = cfg::cfg_attr_pred_end(self.src, a);
                self.cfg_gates(lo + 2, end);
                if cfg::cfg_attr_holds(self.src, a, self.cfg) {
                    let parts = cfg::cfg_attr_parts(self.src, a, end);
                    self.expand(&parts, out);
                }
            } else {
                out.push(*a);
            }
        }
    }

    fn attr_is(&self, a: &Attr, name: &[u8]) -> bool {
        let lo = a.toks.lo as usize;
        a.toks.hi as usize > lo && self.tok_is(lo, name)
    }

    /// rustc_attr_parsing cfg.rs `try_gate_cfg` over the predicate in tokens [lo, hi): a gated
    /// name (`cfg(version)`, `cfg(ub_checks)`, ..) and the compact `target(..)` form.
    fn cfg_gates(&mut self, lo: usize, hi: usize) {
        // (what the file's text wrote is gated by `cfg_scan`, which also sees the items the
        // parser removed for their `cfg`)
        if lo < hi && self.pf.expansion_kind(self.tok_pos(lo)) == crate::parser::ExpnKind::Root { return; }
        self.cfg_gates_in(lo, hi);
    }

    /// Every `#[cfg(..)]`, `#[cfg_attr(..)]` and `cfg!(..)` the file's text writes, outside
    /// `macro_rules!` bodies: rustc evaluates the `cfg` of every node it configures, including
    /// the ones it then strips, which the parser has already removed from the tree here.
    /// (A `cfg` nested in a stripped item is gated too, where rustc never evaluates it.)
    fn cfg_scan(&mut self) {
        let mut skip: Vec<(u32, u32)> = self.ast.items.iter().filter_map(|it| match it.kind {
            ItemKind::MacroRules(r) => Some((r.lo, r.hi)),
            _ => None,
        }).collect();
        skip.sort();
        let n = self.pf.toks.len();
        let mut i = 1;
        while i + 2 < n {
            let t = self.src.tok(i);
            if t.lo >= self.pf.src_len { break; }
            let name = if t.kind == T::Ident { cfg::tok_str(self.src, i) } else { &[][..] };
            let (is_cfg, is_attr) = (name == b"cfg", name == b"cfg_attr");
            if (is_cfg || is_attr) && !skip.iter().any(|&(lo, hi)| (i as u32) >= lo && (i as u32) < hi) {
                let after = self.src.tok(i + 1).kind;
                let before = self.src.tok(i - 1).kind;
                let attr = before == T::OpenBracket && after == T::OpenParen;
                let mac = is_cfg && after == T::Not && self.src.tok(i + 2).kind == T::OpenParen;
                if attr || mac {
                    let open = if attr { i + 1 } else { i + 2 };
                    let mut depth = 0i32;
                    let mut j = open;
                    let mut end = n;
                    while j < n {
                        match self.src.tok(j).kind {
                            T::OpenParen | T::OpenBracket | T::OpenBrace => depth += 1,
                            T::CloseParen | T::CloseBracket | T::CloseBrace => {
                                depth -= 1;
                                if depth == 0 { end = j; break; }
                            }
                            T::Comma if depth == 1 && is_attr => { end = j; break; }
                            _ => {}
                        }
                        j += 1;
                    }
                    self.cfg_gates_in(open + 1, end);
                    i = end.max(i + 1);
                    continue;
                }
            }
            i += 1;
        }
    }

    fn cfg_gates_in(&mut self, lo: usize, hi: usize) {
        let mut i = lo;
        while i < hi {
            let t = self.src.tok(i);
            if t.kind == T::Ident {
                let name = String::from_utf8_lossy(cfg::tok_str(self.src, i)).into_owned();
                let call = i + 1 < hi && self.src.tok(i + 1).kind == T::OpenParen;
                let pos = t.lo;
                if call && name == "target" {
                    self.gate("cfg_target_compact", pos, "compact `cfg(target(..))` is experimental and subject to change");
                } else if !call || name == "version" {
                    if let Some(&(cfg_name, feature)) = GATED_CFGS.iter().find(|(n, _)| *n == name) {
                        self.gate(feature, pos, &format!("`cfg({})` is experimental and subject to change", cfg_name));
                    }
                }
            }
            i += 1;
        }
    }

    /// feature_gate.rs `visit_attribute` and the stability-attribute check E0734.
    fn check_attr(&mut self, a: &Attr) {
        let (lo, hi) = (a.toks.lo as usize, a.toks.hi as usize);
        if hi <= lo { return; }
        let wrapped = self.tok_is(lo, b"unsafe") && hi > lo + 1 && self.src.tok(lo + 1).kind == T::OpenParen;
        let first = if wrapped { lo + 2 } else { lo };
        if first >= hi || !matches!(self.src.tok(first).kind, T::Ident | T::KwBox | T::KwTry) { return; }
        // the attribute's path
        let mut path = String::new();
        let mut j = first;
        while j < hi && matches!(self.src.tok(j).kind, T::Ident | T::KwBox | T::KwTry | T::PathSep | T::KwCrate | T::KwSuper | T::KwSelfValue) {
            path.push_str(&String::from_utf8_lossy(cfg::tok_str(self.src, j)));
            j += 1;
        }
        let pos = self.tok_pos(lo);
        self.unstable_macro_attr(a, &path, pos);
        // a single-segment name: `name`, `name(..)` or `name = ..`
        if first + 1 < hi && self.src.tok(first + 1).kind == T::PathSep { return; }
        let name = String::from_utf8_lossy(cfg::tok_str(self.src, first)).into_owned();
        let pos = self.tok_pos(lo);
        if let Ok(i) = GATED_ATTRS.binary_search_by(|(n, _, _)| (*n).cmp(name.as_str())) {
            let (_, feature, msg) = GATED_ATTRS[i];
            self.gate(feature, pos, msg);
        } else if name.starts_with("rustc_") && crate::builtin_attrs::is_builtin_attr(&name) {
            // rustc_feature builtin_attrs: every `rustc_*` attribute is gated by `rustc_attrs`
            self.gate("rustc_attrs", pos, "use of an internal attribute");
        }
        match name.as_str() {
            "stable" | "unstable" | "rustc_const_stable" | "rustc_const_unstable" | "rustc_default_body_unstable"
            | "unstable_feature_bound" | "unstable_removed" | "rustc_allow_const_fn_unstable" | "allow_internal_unstable" => {
                if !self.feats.has("staged_api") && name != "allow_internal_unstable" && name != "rustc_allow_const_fn_unstable" {
                    self.coded("E0734", pos, "stability attributes may not be used outside of the standard library");
                }
            }
            "crate_type" if hi > first + 2 && cfg::tok_str(self.src, first + 2) == b"\"sdylib\"" => {
                self.gate("export_stable", pos, "`sdylib` crate type is unstable");
            }
            "link" => {
                for (i, _) in self.list_items(first + 1, hi) {
                    if self.tok_is(i, b"cfg") { self.gate("link_cfg", pos, "link cfg is unstable"); }
                }
            }
            "doc" => self.doc_gates(first + 1, hi, pos),
            "used" => self.used_gates(first + 1, hi, pos),
            _ => {}
        }
    }

    /// Attribute macros of core that are `#[unstable]` (library/core/src/macros/mod.rs,
    /// library/core/src/contracts.rs), and inner attributes naming a macro
    /// (rustc_resolve/src/macros.rs: `custom_inner_attributes`).
    fn unstable_macro_attr(&mut self, a: &Attr, path: &str, pos: u32) {
        const UNSTABLE: &[(&str, &str, &str)] = &[
            ("bench", "test", "`bench` is a part of custom test frameworks which are unstable"),
            ("test_case", "custom_test_frameworks", "custom test frameworks are an unstable feature"),
            ("derive_const", "derive_const", ""),
            ("cfg_accessible", "cfg_accessible", "`cfg_accessible` is not fully implemented"),
            ("eii", "extern_item_impls", ""),
            ("unsafe_eii", "extern_item_impls", ""),
            ("alloc_error_handler", "alloc_error_handler", ""),
        ];
        // (the prelude's macros, or the same through `core::prelude::v1` / `std::prelude::v1`)
        let last = path.rsplit("::").next().unwrap_or("");
        let in_prelude = !path.contains("::") || {
            let head = path.strip_suffix(last).unwrap_or("");
            ["core", "std"].iter().any(|c| head.strip_prefix(*c).and_then(|r| r.strip_prefix("::prelude::")).is_some_and(|r| {
                matches!(r.trim_end_matches("::"), "v1" | "rust_2015" | "rust_2018" | "rust_2021" | "rust_2024")
            }))
        };
        // a macro of the program with that name (a user `macro_rules!`, a proc-macro crate's
        // attribute) is what the attribute names, not core's
        let shadowed = !path.contains("::") && (crate::macro_reg::has_attr(last) || self.prog.macros.defs.contains_key(last));
        if in_prelude && !shadowed {
            if let Some(&(_, feature, reason)) = UNSTABLE.iter().find(|(n, _, _)| *n == last) {
                let msg = if reason.is_empty() { format!("use of unstable library feature `{}`", feature) } else { format!("use of unstable library feature `{}`: {}", feature, reason) };
                self.gate(feature, pos, &msg);
            }
            if a.inner && matches!(last, "test" | "bench" | "test_case" | "derive" | "global_allocator" | "cfg_accessible" | "cfg_eval" | "derive_const" | "alloc_error_handler") {
                self.gate("custom_inner_attributes", pos, "inner macro attributes are unstable");
            }
        }
        if matches!(path, "core::contracts::requires" | "core::contracts::ensures" | "std::contracts::requires" | "std::contracts::ensures") {
            self.gate("contracts", pos, "use of unstable library feature `contracts`");
        }
    }

    /// The top-level items of the parenthesized list opening at token `open` (as ranges).
    fn list_items(&self, open: usize, hi: usize) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        if open >= hi || self.src.tok(open).kind != T::OpenParen { return out; }
        let mut depth = 0i32;
        let mut start = open + 1;
        let mut i = open;
        while i < hi {
            match self.src.tok(i).kind {
                T::OpenParen | T::OpenBracket | T::OpenBrace => depth += 1,
                T::CloseParen | T::CloseBracket | T::CloseBrace => {
                    depth -= 1;
                    if depth == 0 {
                        if i > start { out.push((start, i)); }
                        break;
                    }
                }
                T::Comma if depth == 1 => {
                    if i > start { out.push((start, i)); }
                    start = i + 1;
                }
                _ => {}
            }
            i += 1;
        }
        out
    }

    /// The unstable flavors of `#[doc(..)]` (feature_gate.rs `gate_doc`).
    fn doc_gates(&mut self, open: usize, hi: usize, pos: u32) {
        for (lo, _) in self.list_items(open, hi) {
            let name = String::from_utf8_lossy(cfg::tok_str(self.src, lo)).into_owned();
            let (feature, how) = match name.as_str() {
                "cfg" | "auto_cfg" => ("doc_cfg", "experimental"),
                "masked" => ("doc_masked", "experimental"),
                "notable_trait" => ("doc_notable_trait", "experimental"),
                "attribute" | "keyword" | "fake_variadic" | "search_unbox" => ("rustdoc_internals", "meant for internal use only"),
                "rust_logo" => {
                    self.gate("rustdoc_internals", pos, "the `#[doc(rust_logo)]` attribute is used for Rust branding");
                    continue;
                }
                _ => continue,
            };
            self.gate(feature, pos, &format!("`#[doc({})]` is {}", name, how));
        }
    }

    /// `#[used(linker)]` / `#[used(compiler)]` (rustc_attr_parsing attributes/codegen_attrs.rs).
    fn used_gates(&mut self, open: usize, hi: usize, pos: u32) {
        for (lo, _) in self.list_items(open, hi) {
            let name = String::from_utf8_lossy(cfg::tok_str(self.src, lo)).into_owned();
            if matches!(name.as_str(), "linker" | "compiler") {
                self.gate("used_with_arg", pos, &format!("`#[used({})]` is currently unstable", name));
            }
        }
    }

    // ---------------------------------------------------------------- items

    fn items(&mut self, items: &[ItemId], ctx: Ctx) {
        for &it in items { self.item(it, ctx); }
    }

    fn item(&mut self, id: ItemId, ctx: Ctx) {
        let ast = self.ast;
        let item = *ast.item(id);
        let Some(attrs) = self.attrs(item.attrs) else { return };
        let pos = item.lo;
        let outer = std::mem::replace(&mut self.cur_pos, pos);
        let outer_scope = self.scope;
        if let Some(&d) = self.prog.item_defs.get(&(self.file, id.0)) { self.scope = self.prog.def(d).scope; }
        self.item_kind(id, &item, &attrs, ctx);
        self.cur_pos = outer;
        self.scope = outer_scope;
    }

    fn item_kind(&mut self, id: ItemId, item: &Item, attrs: &[Attr], ctx: Ctx) {
        let ast = self.ast;
        let pos = item.lo;
        match item.kind {
            ItemKind::Use(_) | ItemKind::ExternCrate(_) | ItemKind::Mac(_) => {}
            ItemKind::MacroRules(r) => self.metavar_exprs(r),
            ItemKind::Fn(sig, _, _) => {
                let sig = *ast.fn_sig(sig);
                if sig.variadic && ctx != Ctx::Foreign {
                    self.gate("c_variadic", pos, "C-variadic functions are unstable");
                }
                if ctx == Ctx::Foreign { self.llvm_link_name(attrs, pos); }
                self.fn_sig(&sig);
            }
            ItemKind::Struct(r) => {
                let s = *ast.row(r);
                self.repr_simd(attrs);
                self.generics(&s.generics);
                self.variant_data(&s.data);
            }
            ItemKind::Union(r) => {
                let u = *ast.row(r);
                self.repr_simd(attrs);
                if self.has_repr(attrs, b"transparent") {
                    self.gate("transparent_unions", pos, "transparent unions are unstable");
                }
                self.generics(&u.generics);
                self.fields(u.fields);
            }
            ItemKind::Enum(r) => {
                let e = *ast.row(r);
                self.repr_simd(attrs);
                self.generics(&e.generics);
                for v in ast.list(e.variants).to_vec() {
                    let Some(_) = self.attrs(v.attrs) else { continue };
                    self.variant_data(&v.data);
                    if let Some(d) = v.disc { self.expr(d); }
                }
            }
            ItemKind::TypeAlias(r) => {
                let t = *ast.row(r);
                self.generics(&t.generics);
                self.bounds(t.bounds);
                if ctx == Ctx::Foreign {
                    self.gate("extern_types", pos, "extern types are experimental");
                }
                if let Some(ty) = t.ty {
                    if ctx == Ctx::Trait {
                        self.gate("associated_type_defaults", pos, "associated type defaults are unstable");
                    }
                    match ctx {
                        Ctx::Free => self.impl_trait_in_alias(ty, false),
                        Ctx::Trait | Ctx::Impl => self.impl_trait_in_alias(ty, true),
                        Ctx::Foreign => {}
                    }
                    self.ty(ty);
                }
            }
            ItemKind::Const(ty, _) => {
                // (the initializer is a body owner: walked with the bodies)
                if let Some(t) = ty.get() { self.ty(t); }
            }
            ItemKind::Static(_, ty, _) => self.ty(ty),
            ItemKind::Trait(r) => {
                let t = *ast.row(r);
                if t.auto { self.gate("auto_traits", pos, "auto traits are experimental and possibly buggy"); }
                self.generics(&t.generics);
                self.bounds(t.supers);
                let items = ast.list(t.items).to_vec();
                self.items(&items, Ctx::Trait);
            }
            ItemKind::TraitAlias(r) => {
                let t = *ast.row(r);
                self.gate("trait_alias", pos, "trait aliases are experimental");
                self.generics(&t.generics);
                self.bounds(t.bounds);
            }
            ItemKind::Impl(r) => {
                let im = *ast.row(r);
                if im.negative { self.gate("negative_impls", pos, "negative impls are experimental"); }
                if im.trait_.is_none() {
                    // type_of.rs check_feature_inherent_assoc_ty: `impl Ty { type A = ..; }`
                    for &i in ast.list(im.items) {
                        let c = ast.item(i);
                        if matches!(c.kind, ItemKind::TypeAlias(_)) && self.prog.item_defs.get(&(self.file, i.0)).is_some() {
                            self.gate("inherent_associated_types", c.lo, "inherent associated types are unstable");
                        }
                    }
                }
                self.generics(&im.generics);
                if let Some(p) = im.trait_ {
                    self.trait_path_sugar(&p);
                    self.path(&p);
                }
                self.ty(im.self_ty);
                let items = ast.list(im.items).to_vec();
                self.items(&items, Ctx::Impl);
            }
            ItemKind::Mod(Some(list)) => {
                let items = ast.list(list).to_vec();
                self.items(&items, Ctx::Free);
            }
            ItemKind::Mod(None) => {
                let Some(&d) = self.prog.item_defs.get(&(self.file, id.0)) else { return };
                let def = self.prog.def(d);
                if def.kind != DefKind::Mod { return; }
                let file = self.prog.mods[def.sub as usize].file;
                if file == self.file { return; }
                let mut sub = self.in_file(file);
                sub.cfg_scan();
                sub.root_attrs();
                let items: Vec<ItemId> = sub.ast.root_items.iter().copied().collect();
                sub.items(&items, Ctx::Free);
                self.errors.append(&mut sub.errors);
            }
            ItemKind::ForeignMod(r) => {
                let f = *ast.row(r);
                let items = ast.list(f.items).to_vec();
                self.items(&items, Ctx::Foreign);
            }
        }
    }

    fn has_repr(&self, attrs: &[Attr], word: &[u8]) -> bool {
        attrs.iter().any(|a| {
            if !self.attr_is(a, b"repr") { return false; }
            let (lo, hi) = (a.toks.lo as usize, a.toks.hi as usize);
            self.list_items(lo + 1, hi).iter().any(|&(i, _)| self.tok_is(i, word))
        })
    }

    /// rustc_expand/src/mbe/quoted.rs: `${..}` meta-variable expressions (and `$$`) in a
    /// `macro_rules!` definition, `${concat(..)}` additionally.
    fn metavar_exprs(&mut self, r: TokRange) {
        let mut i = r.lo as usize;
        while i + 1 < r.hi as usize {
            if self.src.tok(i).kind == T::Dollar {
                let next = self.src.tok(i + 1).kind;
                if next == T::OpenBrace || next == T::Dollar {
                    let pos = self.tok_pos(i);
                    self.gate("macro_metavar_expr", pos, "meta-variable expressions are unstable");
                    if next == T::OpenBrace && i + 2 < r.hi as usize && self.tok_is(i + 2, b"concat") {
                        self.gate("macro_metavar_expr_concat", pos, "the `concat` meta-variable expression is unstable");
                    }
                    if next == T::Dollar { i += 1; }
                }
            }
            i += 1;
        }
    }

    /// `#[repr(simd)]` on a struct, enum or union.
    fn repr_simd(&mut self, attrs: &[Attr]) {
        for a in attrs {
            if !self.attr_is(a, b"repr") { continue; }
            let (lo, hi) = (a.toks.lo as usize, a.toks.hi as usize);
            for (i, _) in self.list_items(lo + 1, hi) {
                if self.tok_is(i, b"simd") {
                    let pos = self.tok_pos(lo);
                    self.gate("repr_simd", pos, "SIMD types are experimental and possibly buggy");
                }
            }
        }
    }

    /// A foreign fn or static linking to `llvm.*` (`#[link_name = "llvm.x"]`).
    fn llvm_link_name(&mut self, attrs: &[Attr], pos: u32) {
        for a in attrs {
            if !self.attr_is(a, b"link_name") { continue; }
            let lo = a.toks.lo as usize;
            if a.toks.hi as usize > lo + 2 && self.src.tok(lo + 1).kind == T::Eq {
                let v = cfg::tok_str(self.src, lo + 2);
                if v.starts_with(b"\"llvm.") {
                    self.gate("link_llvm_intrinsics", pos, "linking to LLVM intrinsics is experimental");
                }
            }
        }
    }

    /// feature_gate.rs `check_impl_trait`: `impl Trait` inside `type Alias = ..;`.
    fn impl_trait_in_alias(&mut self, ty: TyId, assoc: bool) {
        let mut found = Vec::new();
        self.impl_traits(ty, &mut found);
        for pos in found {
            if assoc {
                self.gate("impl_trait_in_assoc_type", pos, "`impl Trait` in associated types is unstable");
            } else {
                self.gate("type_alias_impl_trait", pos, "`impl Trait` in type aliases is unstable");
            }
        }
    }

    /// The `impl Trait` types inside `ty` (not through anonymous constants). Types keep no
    /// position: the position is the item's first mention of the keyword.
    fn impl_traits(&self, ty: TyId, out: &mut Vec<u32>) {
        let ast = self.ast;
        match *ast.ty(ty) {
            Ty::ImplTrait(_) => out.push(self.ty_pos(ty)),
            Ty::Ref(_, _, t) | Ty::Ptr(_, t) | Ty::Slice(t) | Ty::Paren(t) | Ty::Array(t, _) => self.impl_traits(t, out),
            Ty::Tuple(l) => for &t in ast.list(l) { self.impl_traits(t, out) },
            Ty::Path(p) => {
                for seg in ast.segs(p.segs) {
                    if let Some(GenericArgs::Angle(l)) = ast.gargs(seg.args()) {
                        for ga in ast.list(*l) {
                            match *ga {
                                GenericArg::Type(t) | GenericArg::Binding(_, _, t) => self.impl_traits(t, out),
                                _ => {}
                            }
                        }
                    }
                }
            }
            Ty::Fn(f) => {
                let f = *ast.fn_ptr(f);
                for &t in ast.list(f.params) { self.impl_traits(t, out) }
                if let Some(r) = f.ret { self.impl_traits(r, out) }
            }
            _ => {}
        }
    }

    /// Types keep no position of their own: a gate on a type is reported at the innermost
    /// item or expression that holds it.
    fn ty_pos(&self, _ty: TyId) -> u32 {
        self.cur_pos
    }

    // ---------------------------------------------------------------- signatures

    fn fn_sig(&mut self, sig: &FnSig) {
        self.generics(&sig.generics);
        if let SelfParam::Typed(_, t) = sig.self_param.unwrap_or(SelfParam::Value(false)) { self.ty(t); }
        for p in self.ast.list(sig.params).to_vec() {
            if self.attrs(p.attrs).is_none() { continue; }
            self.pat(p.pat);
            self.ty(p.ty);
        }
        if let Some(r) = sig.ret { self.ret_ty(r); }
    }

    /// A return type (`-> !` needs no gate).
    fn ret_ty(&mut self, t: TyId) {
        if !matches!(self.ast.ty(t), Ty::Never) { self.ty(t); }
    }

    fn generics(&mut self, g: &Generics) {
        for p in self.ast.list(g.params).to_vec() {
            if self.attrs(p.attrs).is_none() { continue; }
            match p.kind {
                GenericParamKind::Lifetime(_) => {}
                GenericParamKind::Type(b, d) => {
                    self.bounds(b);
                    if let Some(t) = d { self.ty(t); }
                }
                GenericParamKind::Const(t, d) => {
                    self.ty(t);
                    if let Some(e) = d { self.expr(e); }
                }
            }
        }
        for w in self.ast.list(g.where_).to_vec() {
            match w {
                WherePred::Bound { hrtb, ty, bounds } => {
                    self.late_bound_defs(hrtb);
                    self.ty(ty);
                    self.bounds(bounds);
                }
                WherePred::Lifetime(..) => {}
            }
        }
    }

    /// feature_gate.rs `check_late_bound_lifetime_defs`: `for<..>` binders hold lifetimes only.
    fn late_bound_defs(&mut self, params: List<GenericParam>) {
        let pos = self.ast.list(params).iter().find(|p| !matches!(p.kind, GenericParamKind::Lifetime(_))).map(|p| p.name.lo);
        if let Some(pos) = pos {
            self.gate("non_lifetime_binders", pos, "only lifetime parameters can be used in this context");
        }
    }

    fn bounds(&mut self, b: List<Bound>) {
        for bound in self.ast.list(b).to_vec() {
            if let Bound::Trait { hrtb, path, async_, konst, .. } = bound {
                if async_ { self.gate("async_trait_bounds", path.lo, "`async` trait bounds are unstable"); }
                if konst { self.gate("const_trait_impl", path.lo, "const trait impls are experimental"); }
                self.late_bound_defs(hrtb);
                self.trait_path_sugar(&path);
                self.path(&path);
            }
        }
    }

    /// hir_ty_lowering `lower_generic_args_of_path`: `Foo(A) -> B` is `Fn`-family sugar, and
    /// the `Fn`-family's own `<Args>` form is unstable (`unboxed_closures`).
    fn trait_path_sugar(&mut self, path: &Path) {
        let ast = self.ast;
        let Some(last) = ast.segs(path.segs).last().copied() else { return };
        let paren = match ast.gargs(last.args()) {
            Some(GenericArgs::Paren(..)) => true,
            Some(GenericArgs::Angle(_)) => false,
            None => return,
        };
        let Some(td) = self.tcx.resolve_type_path_def(self.prog, self.file, self.scope, path) else { return };
        if self.prog.def(td).kind != DefKind::Trait { return; }
        let callable = self.tcx.callable_output(td).is_some();
        if paren && !callable {
            self.gate("unboxed_closures", path.lo, "parenthetical notation is only stable when used with `Fn`-family traits");
        } else if !paren && callable {
            self.gate("unboxed_closures", path.lo, "the precise format of `Fn`-family traits' type parameters is subject to change");
        }
    }

    fn fields(&mut self, l: List<FieldDef>) {
        for f in self.ast.list(l).to_vec() {
            if self.attrs(f.attrs).is_none() { continue; }
            self.ty(f.ty);
            if let Some(d) = f.default { self.expr(d); }
        }
    }

    fn variant_data(&mut self, d: &VariantData) {
        match *d {
            VariantData::Unit => {}
            VariantData::Tuple(l) | VariantData::Struct(l) => self.fields(l),
        }
    }

    // ---------------------------------------------------------------- types and paths

    /// rustc_passes stability `check_path` for a path as written: `std::intrinsics::copy` names
    /// the library's unstable definitions by the written path (a re-export has its own stability).
    fn path_stability(&mut self, p: &Path) {
        if p.has_qself() || !matches!(self.pf.expansion_kind(p.lo), crate::parser::ExpnKind::Root | crate::parser::ExpnKind::LocalRules) { return; }
        let segs = self.ast.segs(p.segs);
        let names: Vec<&str> = segs.iter().map(|sg| self.pf.text(sg.name)).collect();
        if !matches!(names.first(), Some(&"std" | &"core" | &"alloc")) { return; }
        // (a local item or crate of that name shadows the library's)
        if !p.global() {
            if let Some(s0) = self.prog.syms.get(names[0]) {
                if let Some(d) = self.prog.lookup_in_scope(self.scope, s0, true) {
                    if self.prog.def(d).krate == self.krate { return; }
                }
            }
        }
        if let Some((feature, reason)) = table_entry(&names) {
            if self.feats.has(&feature) { return; }
            let msg = match reason { Some(r) => format!("E0658: use of unstable library feature `{}`: {}", feature, r), None => format!("E0658: use of unstable library feature `{}`", feature) };
            self.errors.push(self.prog.record_error(self.file, p.lo, "typeck", msg));
        }
    }

    fn path(&mut self, p: &Path) {
        self.path_stability(p);
        if let Some(q) = self.ast.qself(p).copied() {
            self.ty(q.ty);
            if let Some(tp) = q.trait_path {
                self.trait_path_sugar(&tp);
                self.path(&tp);
            }
        }
        for seg in self.ast.segs(p.segs).to_vec() {
            self.generic_args(seg.args());
        }
    }

    fn generic_args(&mut self, id: GenericArgsId) {
        let Some(ga) = self.ast.gargs(id).copied() else { return };
        match ga {
            GenericArgs::Angle(l) => {
                for a in self.ast.list(l).to_vec() {
                    match a {
                        GenericArg::Lifetime(_) => {}
                        GenericArg::Type(t) => self.ty(t),
                        GenericArg::Const(e) => self.expr(e),
                        GenericArg::Binding(_, ga, t) => {
                            self.generic_args(ga);
                            self.ty(t);
                        }
                        GenericArg::Constraint(_, _, b) => self.bounds(b),
                    }
                }
            }
            GenericArgs::Paren(tys, ret) => {
                for &t in self.ast.list(tys).to_vec().iter() { self.ty(t); }
                // `Fn() -> !`: the one return position that gates the never type
                if let Some(r) = ret { self.ty(r); }
            }
        }
    }

    fn ty(&mut self, t: TyId) {
        let ast = self.ast;
        match *ast.ty(t) {
            Ty::Path(p) => self.path(&p),
            Ty::Ref(_, _, x) | Ty::Ptr(_, x) | Ty::Slice(x) | Ty::Paren(x) => self.ty(x),
            Ty::Array(x, e) => {
                self.ty(x);
                self.expr(e);
            }
            Ty::Tuple(l) => for &x in ast.list(l) { self.ty(x) },
            Ty::Fn(f) => {
                let f = *ast.fn_ptr(f);
                self.late_bound_defs(f.hrtb);
                for &x in ast.list(f.params) { self.ty(x) }
                if let Some(r) = f.ret { self.ret_ty(r); }
            }
            Ty::Never => {
                let pos = self.ty_pos(t);
                self.gate("never_type", pos, "the `!` type is experimental");
            }
            Ty::ImplTrait(b) | Ty::DynTrait(b, _) => self.bounds(b),
            Ty::Infer | Ty::Mac(_) | Ty::ImplicitSelf | Ty::CVarArgs => {}
        }
    }

    // ---------------------------------------------------------------- patterns

    fn pat(&mut self, p: PatId) {
        let ast = self.ast;
        match *ast.pat(p) {
            Pat::Wild | Pat::Rest | Pat::Mac(_) => {}
            Pat::Ident { sub, .. } => if let Some(s) = sub { self.pat(s) },
            Pat::Lit(e) => self.expr(e),
            Pat::Range(a, b, _) => {
                if let Some(e) = a { self.expr(e) }
                if let Some(e) = b { self.expr(e) }
            }
            Pat::Path(path) => self.path(&path),
            Pat::TupleStruct(path, l) => {
                self.path(&path);
                for &x in ast.list(l) { self.pat(x) }
            }
            Pat::Struct(path, l, _) => {
                self.path(&path);
                for f in ast.list(l).to_vec() {
                    if self.attrs(f.attrs).is_none() { continue; }
                    self.pat(f.pat);
                }
            }
            Pat::Tuple(l) | Pat::Or(l) => for &x in ast.list(l) { self.pat(x) },
            Pat::Slice(l) => {
                for &x in ast.list(l) {
                    // `[a, X.., b]`: a half-open range in a slice
                    let inner = match *ast.pat(x) {
                        Pat::Ident { sub: Some(s), .. } => s,
                        _ => x,
                    };
                    if matches!(*ast.pat(inner), Pat::Range(Some(_), None, _)) {
                        let pos = self.cur_pos;
                        self.gate("half_open_range_patterns_in_slices", pos, "`X..` patterns in slices are experimental");
                    }
                    self.pat(x);
                }
            }
            Pat::Ref(_, x) | Pat::Paren(x) => self.pat(x),
            Pat::Box(x) => {
                let pos = self.cur_pos;
                self.gate("box_patterns", pos, "box pattern syntax is experimental");
                self.pat(x);
            }
        }
    }

    // ---------------------------------------------------------------- bodies

    /// A body owner's body: a fn's block or a const / static initializer.
    pub fn body(&mut self, owner: crate::program::DefId) {
        let def = self.prog.def(owner);
        let item = *self.ast.item(def.item);
        let outer = std::mem::replace(&mut self.cur_pos, item.lo);
        self.binders.clear();
        if self.pf.edition >= 2018 { self.body_binders(owner); }
        match item.kind {
            ItemKind::Fn(_, Some(b), _) => self.block(b),
            ItemKind::Const(_, e) | ItemKind::Static(_, _, e) => if let Some(e) = e.get() { self.expr(e) },
            _ => {}
        }
        self.cur_pos = outer;
    }

    /// The generic parameters and fn parameters a body's outermost ribs bind.
    fn body_binders(&mut self, owner: crate::program::DefId) {
        let ast = self.ast;
        let def = self.prog.def(owner);
        let mut owners = vec![owner];
        let parent = self.prog.def(def.parent);
        if matches!(parent.kind, DefKind::Impl | DefKind::Trait) && parent.file == def.file { owners.push(def.parent); }
        for o in owners {
            let it = ast.item(self.prog.def(o).item);
            if let Some(g) = ast.kind_generics(&it.kind) {
                for p in ast.list(g.params) {
                    match p.kind {
                        GenericParamKind::Type(..) => self.binders.push((self.pf.text(p.name).to_string(), true)),
                        GenericParamKind::Const(..) => self.binders.push((self.pf.text(p.name).to_string(), false)),
                        GenericParamKind::Lifetime(_) => {}
                    }
                }
            }
            if o == owner {
                if let ItemKind::Fn(sig, _, _) = it.kind {
                    for p in ast.list(ast.fn_sig(sig).params).to_vec() { self.pat_binders(p.pat); }
                }
            }
        }
    }

    /// Names a pattern binds (an identifier that names a constant or unit struct in scope is
    /// not a binding).
    fn pat_binders(&mut self, p: PatId) {
        let ast = self.ast;
        match *ast.pat(p) {
            Pat::Ident { name, sub, .. } => {
                let n = self.pf.text(name).to_string();
                let is_const = self.prog.syms.get(&n).and_then(|s| self.prog.lookup_in_scope(self.scope, s, false))
                    .is_some_and(|d| matches!(self.prog.def(d).kind, DefKind::Const | DefKind::Static | DefKind::Struct | DefKind::Variant | DefKind::AssocConst));
                if !is_const { self.binders.push((n, false)); }
                if let Some(s) = sub { self.pat_binders(s); }
            }
            Pat::TupleStruct(_, l) | Pat::Tuple(l) | Pat::Slice(l) | Pat::Or(l) => for &x in ast.list(l).to_vec().iter() { self.pat_binders(x) },
            Pat::Struct(_, l, _) => for f in ast.list(l).to_vec() { self.pat_binders(f.pat) },
            Pat::Ref(_, x) | Pat::Paren(x) | Pat::Box(x) => self.pat_binders(x),
            _ => {}
        }
    }

    /// rustc_resolve late.rs `future_proof_import`: from edition 2018 a `use` whose first
    /// segment names a type parameter or a local variable of the enclosing rib is an error.
    fn future_proof_import(&mut self, tree: &UseTree, items: &[ItemId]) {
        let ast = self.ast;
        let (seg, simple_only) = match *tree {
            UseTree::Path(seg, _) => (seg, false),
            UseTree::Name(seg, _) => (seg, true),
            UseTree::Glob => return,
            UseTree::Group(l) => {
                for t in ast.list(l).to_vec() { self.future_proof_import(&t, items); }
                return;
            }
        };
        if seg.kind() != SegKind::Ident { return; }
        let name = self.pf.text(seg.name).to_string();
        // (an item of the enclosing blocks with that name wins over an outer rib)
        let declared = |ns_type: bool| items.iter().any(|&i| {
            let it = ast.item(i);
            if self.pf.text(it.name) != name { return false; }
            let ty_ns = matches!(it.kind, ItemKind::Struct(_) | ItemKind::Enum(_) | ItemKind::Union(_) | ItemKind::Trait(_) | ItemKind::TypeAlias(_) | ItemKind::Mod(_) | ItemKind::TraitAlias(_));
            let val_ns = matches!(it.kind, ItemKind::Fn(..) | ItemKind::Const(..) | ItemKind::Static(..));
            if ns_type { ty_ns } else { val_ns }
        });
        let nss: &[bool] = if simple_only { &[true, false] } else { &[true] };
        for &ns_type in nss {
            if declared(ns_type) { continue; }
            // the innermost rib binding the name in that namespace
            let hit = self.binders.iter().rev().find(|(n, tp)| *n == name && (*tp == ns_type || (!ns_type && !*tp)));
            if let Some(&(_, tp)) = hit {
                if tp != ns_type { continue; }
                let what = if ns_type { "type parameters" } else { "local variables" };
                self.uncoded(seg.name.lo, &format!("imports cannot refer to {}", what));
            }
        }
    }

    fn block(&mut self, b: BlockId) {
        let ast = self.ast;
        if !ast.has_block(b) { return; }
        let blk = *ast.block(b);
        let _ = self.attrs(blk.attrs);
        let mark = self.binders.len();
        let stmts = ast.list(blk.stmts).to_vec();
        let block_items: Vec<ItemId> = stmts.iter().filter_map(|s| if let Stmt::Item(i) = s { Some(*i) } else { None }).collect();
        for s in stmts {
            match s {
                Stmt::Let(r) => {
                    let l = *ast.row(r);
                    if self.attrs(l.attrs).is_none() { continue; }
                    self.pat(l.pat);
                    if let Some(t) = l.ty { self.ty(t); }
                    if let Some(e) = l.init { self.expr(e); }
                    if let Some(eb) = l.else_ { self.block(eb); }
                    if self.pf.edition >= 2018 { self.pat_binders(l.pat); }
                }
                Stmt::Item(i) => {
                    if self.pf.edition >= 2018 {
                        if let ItemKind::Use(r) = ast.item(i).kind {
                            let u = *ast.row(r);
                            if !u.global { self.future_proof_import(&u.tree, &block_items); }
                        }
                    }
                    self.item(i, Ctx::Free)
                }
                Stmt::Expr(e, _) => self.expr(e),
                Stmt::Attrs(r) => {
                    let a = *ast.row(r);
                    if self.attrs(a.attrs).is_none() { continue; }
                    self.expr(a.expr);
                }
                Stmt::Empty => {}
            }
        }
        self.binders.truncate(mark);
    }

    fn expr(&mut self, e: ExprId) {
        let ast = self.ast;
        if !ast.has_expr(e) { return; }
        let ex = *ast.expr(e);
        let outer = std::mem::replace(&mut self.cur_pos, ex.lo);
        match ex.kind {
            ExprKind::Lit(k) => {
                if matches!(k, LitKind::Int | LitKind::Float) {
                    let text = self.pf.bytes(ex.lo, ex.hi);
                    // (`0x1f16` is hex digits: a hex literal has no float suffix)
                    if !(text.starts_with(b"0x") || text.starts_with(b"0X")) {
                        if text.ends_with(b"f16") { self.gate("f16", ex.lo, "the type `f16` is unstable"); }
                        if text.ends_with(b"f128") { self.gate("f128", ex.lo, "the type `f128` is unstable"); }
                    }
                }
            }
            ExprKind::Path(p) => self.path(&p),
            ExprKind::Unary(_, a) | ExprKind::Field(a, _) | ExprKind::TupleField(a, _) | ExprKind::AddrOf(_, _, a)
            | ExprKind::Try(a) | ExprKind::Await(a) | ExprKind::Paren(a) | ExprKind::Box(a) | ExprKind::Let(_, a) => {
                if let ExprKind::Let(p, _) = ex.kind { self.pat(p); }
                self.expr(a);
            }
            ExprKind::Cast(a, t) => {
                self.expr(a);
                self.ty(t);
            }
            ExprKind::Binary(_, a, b) | ExprKind::Assign(a, b) | ExprKind::AssignOp(_, a, b) | ExprKind::Index(a, b) | ExprKind::Repeat(a, b) => {
                self.expr(a);
                self.expr(b);
            }
            ExprKind::Call(f, args) => {
                self.expr(f);
                for &a in ast.list(args).to_vec().iter() { self.expr(a) }
            }
            ExprKind::MethodCall(recv, r) => {
                self.expr(recv);
                let m = *ast.row(r);
                self.generic_args(m.turbofish);
                for &a in ast.list(m.args).to_vec().iter() { self.expr(a) }
            }
            ExprKind::Block(b, _) | ExprKind::Unsafe(b) | ExprKind::Async(_, b) | ExprKind::ConstBlock(b) | ExprKind::Loop(b, _) => self.block(b),
            ExprKind::If(c, b, el) => {
                self.expr(c);
                self.block(b);
                if let Some(x) = el.get() { self.expr(x) }
            }
            ExprKind::Match(s, arms) => {
                self.expr(s);
                for arm in ast.list(arms).to_vec() {
                    if self.attrs(arm.attrs).is_none() { continue; }
                    self.pat(arm.pat);
                    let mark = self.binders.len();
                    if self.pf.edition >= 2018 { self.pat_binders(arm.pat); }
                    if let Some(g) = arm.guard { self.expr(g) }
                    self.expr(arm.body);
                    self.binders.truncate(mark);
                }
            }
            ExprKind::While(r) => {
                let w = *ast.row(r);
                self.expr(w.cond);
                self.block(w.body);
            }
            ExprKind::For(r) => {
                let f = *ast.row(r);
                self.pat(f.pat);
                self.expr(f.iter);
                self.block(f.body);
            }
            ExprKind::Break(_, v) => if let Some(x) = v.get() { self.expr(x) },
            ExprKind::Return(v) | ExprKind::Yield(v) => if let Some(x) = v.get() { self.expr(x) },
            ExprKind::Mac(r) => {
                let m = *ast.row(r);
                // `cfg!(pred)`: the predicate's cfg gates
                let segs = ast.segs(m.path.segs);
                if segs.len() == 1 && self.pf.text(segs[0].name) == "cfg" {
                    self.cfg_gates(m.toks.lo as usize, m.toks.hi as usize);
                }
            }
            ExprKind::Continue(_) | ExprKind::Underscore => {}
            ExprKind::Closure(r) => {
                let c = *ast.row(r);
                let mark = self.binders.len();
                if self.pf.edition >= 2018 { for p in ast.list(c.params).to_vec() { self.pat_binders(p.pat); } }
                if c.is_static { self.gate("coroutines", ex.lo, "coroutine syntax is experimental"); }
                for p in ast.list(c.params).to_vec() {
                    if self.attrs(p.attrs).is_none() { continue; }
                    self.pat(p.pat);
                    if let Some(t) = p.ty { self.ty(t) }
                }
                if let Some(t) = c.ret { self.ret_ty(t) }
                self.expr(c.body);
                self.binders.truncate(mark);
            }
            ExprKind::Tuple(l) | ExprKind::Array(l) => for &x in ast.list(l).to_vec().iter() { self.expr(x) },
            ExprKind::Struct(r) => {
                let s = *ast.row(r);
                self.path(&s.path);
                for f in ast.list(s.fields).to_vec() {
                    if self.attrs(f.attrs).is_none() { continue; }
                    self.expr(f.expr);
                }
                if let Some(b) = s.base.get() { self.expr(b) }
            }
            ExprKind::Range(a, b, _) => {
                if let Some(x) = a.get() { self.expr(x) }
                if let Some(x) = b.get() { self.expr(x) }
            }
        }
        self.cur_pos = outer;
    }
}

/// The bodies of a checked crate's owners (fn bodies and const / static initializers) and the
/// items nested in them, once the bodies are loaded.
pub fn check_bodies(prog: &Program, tcx: &crate::tcx::Tcx, krate: u32) -> Vec<String> {
    let info = &prog.crates[krate as usize];
    let feats = info.features(prog);
    let mut out = Vec::new();
    for i in 0..prog.defs.len() {
        let d = crate::program::DefId(i as u32);
        let def = prog.def(d);
        if def.krate != krate || !matches!(def.kind, DefKind::Fn | DefKind::AssocFn | DefKind::Const | DefKind::Static | DefKind::AssocConst) { continue; }
        if prog.item_defs.get(&(def.file, def.item.0)) != Some(&d) { continue; }
        let mut w = Gates::new(prog, tcx, krate, def.file, feats);
        w.scope = def.scope;
        w.body(d);
        out.append(&mut w.errors);
    }
    out
}

/// rustc_passes/src/stability.rs `check_path` / `check_method_call`: a std item marked
/// `#[unstable(feature = "..")]` that a checked body names (a path or a method call written in
/// the crate's source) without enabling the feature. Relax's std marks the items its tests
/// reach (`fn_traits`): the table of rustc's library is not mirrored.
pub fn check_stability(prog: &Program, body: &crate::typeck::Body) -> Vec<String> {
    let krate = prog.def(body.def).krate;
    let feats = prog.crates[krate as usize].features(prog);
    let pf = &*prog.files[body.file as usize];
    let mut out = Vec::new();
    let mut visit = |e: u32, d: crate::program::DefId| {
        let def = prog.def(d);
        if def.krate == krate { return; }
        let Some((feature, reason)) = unstable_of(prog, d) else { return };
        let ex = *pf.ast.expr(ExprId(e));
        if feats.has(&feature) || !matches!(pf.expansion_kind(ex.lo), crate::parser::ExpnKind::Root | crate::parser::ExpnKind::LocalRules) { return; }
        let msg = match reason { Some(r) => format!("E0658: use of unstable library feature `{}`: {}", feature, r), None => format!("E0658: use of unstable library feature `{}`", feature) };
        out.push(prog.record_error(body.file, ex.lo, "typeck", msg));
    };
    for (&e, r) in body.res.iter() {
        if let crate::typeck::Res::Def(d) = *r { visit(e, d); }
    }
    for (&e, m) in body.methods.iter() { visit(e, m.def); }
    out
}

/// `#[unstable(feature = "name", reason = "..")]` on a definition's item: (feature, reason).
fn unstable_of(prog: &Program, d: crate::program::DefId) -> Option<(String, Option<String>)> {
    let def = prog.def(d);
    // (the other definitions -- primitives, locals -- have no item of their own)
    if !matches!(def.kind, DefKind::Mod | DefKind::Fn | DefKind::AssocFn | DefKind::Const | DefKind::Static | DefKind::AssocConst | DefKind::Struct | DefKind::Enum | DefKind::Union | DefKind::Trait | DefKind::TypeAlias | DefKind::AssocTy) { return None; }
    unstable_in_attrs(prog, d)
}

/// The library's table entry for a path as written (`std::intrinsics::copy`): a re-export has
/// its own stability, so the key is the written path, not the defining one.
fn table_entry(names: &[&str]) -> Option<(String, Option<String>)> {
    // a stable re-export of a module or item is the definition it names (rustc checks the
    // stability of the path's final definition): rewrite the longest re-exported prefix
    let mut key: Vec<String> = names.iter().map(|s| s.to_string()).collect();
    for _ in 0..4 {
        let joined = key.join("::");
        if let Some(&(f, r)) = stability_table().get(joined.as_str()) {
            if !f.is_empty() { return Some((f.to_string(), if r.is_empty() { None } else { Some(r.to_string()) })); }
        }
        let mut rewritten = false;
        for k in (1..=key.len()).rev() {
            let prefix = key[..k].join("::");
            if let Some(&("", r)) = stability_table().get(prefix.as_str()) {
                if let Some(target) = r.strip_prefix("reexport ") {
                    let mut nk: Vec<String> = target.split("::").map(|s| s.to_string()).collect();
                    nk.extend(key[k..].iter().cloned());
                    key = nk;
                    rewritten = true;
                    break;
                }
            }
        }
        if !rewritten { return None; }
    }
    None
}

/// `path -> (feature, reason)` of the library's unstable definitions (std/stability.tsv).
fn stability_table() -> &'static std::collections::HashMap<&'static str, (&'static str, &'static str)> {
    static TABLE: std::sync::OnceLock<std::collections::HashMap<&'static str, (&'static str, &'static str)>> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| {
        include_str!("../../compiler/std/stability.tsv").lines().filter_map(|l| {
            let mut it = l.splitn(3, '\t');
            Some((it.next()?, (it.next()?, it.next().unwrap_or(""))))
        }).collect()
    })
}

/// rustc_passes stability.rs `check_path` on `use` declarations: every module and item a checked
/// crate's import names must be stable or have its feature enabled.
pub fn check_imports(prog: &Program, krate: u32) -> Vec<String> {
    let feats = prog.crates[krate as usize].features(prog);
    let mut out = Vec::new();
    for imp in prog.imports.iter() {
        if imp.ext || prog.file_crate.get(imp.file as usize) != Some(&krate) { continue; }
        let pf = &*prog.files[imp.file as usize];
        if !matches!(pf.expansion_kind(imp.pos), crate::parser::ExpnKind::Root | crate::parser::ExpnKind::LocalRules) { continue; }
        let names: Vec<&str> = imp.segs.iter().map(|sg| prog.syms.str(sg.1)).collect();
        if !imp.global && !matches!(names.first(), Some(&"std" | &"core" | &"alloc")) { continue; }
        if let Some((feature, reason)) = table_entry(&names) {
            if feats.has(&feature) { continue; }
            let msg = match reason { Some(r) => format!("E0658: use of unstable library feature `{}`: {}", feature, r), None => format!("E0658: use of unstable library feature `{}`", feature) };
            out.push(prog.record_error(imp.file, imp.pos, "typeck", msg));
        }
    }
    out
}

fn unstable_in_attrs(prog: &Program, d: crate::program::DefId) -> Option<(String, Option<String>)> {
    let def = prog.def(d);
    let pf = &*prog.files[def.file as usize];
    let src = prog.src(def.file);
    let item = pf.ast.item(def.item);
    for a in pf.ast.list(item.attrs) {
        let (lo, hi) = (a.toks.lo as usize, a.toks.hi as usize);
        if hi <= lo + 3 || cfg::tok_str(src, lo) != b"unstable" { continue; }
        let (mut feature, mut reason) = (None, None);
        let mut i = lo + 2;
        while i + 2 < hi {
            if src.tok(i + 1).kind == T::Eq && src.tok(i + 2).kind == T::Str {
                let v = String::from_utf8_lossy(cfg::tok_str(src, i + 2)).trim_matches('"').to_string();
                match cfg::tok_str(src, i) {
                    b"feature" => feature = Some(v),
                    b"reason" => reason = Some(v),
                    _ => {}
                }
                i += 3;
            } else {
                i += 1;
            }
        }
        return feature.map(|f| (f, reason));
    }
    None
}
