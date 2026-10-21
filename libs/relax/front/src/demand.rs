//! Module-qualified demand lookup. A glob is a lookup edge, never enumeration.
//! Expansion uncertainty belongs to the invocation scope (rustc's reduced graph),
//! not to every crate that happens to contain an identifier with the same spelling.
use super::*;
use std::borrow::Borrow;

pub fn within(path: &str, module: &str) -> bool {
    path == module || path.strip_prefix(module).is_some_and(|tail| tail.starts_with("::"))
}

pub fn close(toks: &[Tok], start: usize) -> usize {
    let mut depth = 0usize;
    for (i, t) in toks.iter().enumerate().skip(start) {
        match t.kind {
            T::OpenBrace | T::OpenBracket | T::OpenParen => depth += 1,
            T::CloseBrace | T::CloseBracket | T::CloseParen => {
                depth = depth.saturating_sub(1);
                if depth == 0 { return i; }
            }
            _ => {}
        }
    }
    toks.len().saturating_sub(1)
}
fn word(src: &[u8], t: &Tok) -> String {
    String::from_utf8_lossy(&src[t.lo as usize..t.hi as usize]).trim_start_matches("r#").into()
}

fn is_word(src: &[u8], token: &Tok, expected: &str) -> bool {
    // the spelling is the word or `r#` and the word: any other length cannot match
    let len = (token.hi - token.lo) as usize;
    if len != expected.len() && len != expected.len() + 2 { return false; }
    let text = &src[token.lo as usize..token.hi as usize];
    text.strip_prefix(b"r#").unwrap_or(text) == expected.as_bytes()
}

/// Apply cfg before extracting any demand, including cfg inside functions and
/// inline modules. Use the same evaluator and cfg_attr expansion as the parser.
pub fn active_tokens(src: &[u8], toks: &[Tok], cfg: &CfgSet) -> Vec<Tok> {
    let _profile = crate::load_profile::span(cfg.session.runtime.profile, "rust-demand-cfg", "tokens");
    let mut out = Vec::with_capacity(toks.len());
    let groups = Groups::of(toks);
    active_tokens_inner(src, toks, 0, toks.len(), cfg, &groups, &mut out);
    // a file with disabled items keeps the slack of the unfiltered length for as long as the scan lives
    if out.capacity() - out.len() > out.len() / 32 { out.shrink_to_fit(); }
    out
}

/// Where each bracket group of a token slice closes, found in one pass. `close` walks the group
/// from its opener, so asking it for every nested group re-walks the inner tokens once per
/// nesting level. The pass counts depth exactly as `close` does (any bracket kind), so for a
/// slice whose brackets balance the answers agree; any other slice keeps asking `close`.
struct Groups(Option<Vec<u32>>);
impl Groups {
    fn of(toks: &[Tok]) -> Self {
        let mut matching = vec![0u32; toks.len()];
        let mut open = Vec::<u32>::new();
        for (i, t) in toks.iter().enumerate() {
            match t.kind {
                T::OpenBrace | T::OpenBracket | T::OpenParen => open.push(i as u32),
                T::CloseBrace | T::CloseBracket | T::CloseParen => match open.pop() {
                    Some(o) => matching[o as usize] = i as u32,
                    None => return Self(None),
                },
                _ => {}
            }
        }
        Self(open.is_empty().then_some(matching))
    }
    /// `close(&toks[..hi], start)`.
    fn close(&self, toks: &[Tok], hi: usize, start: usize) -> usize {
        match &self.0 {
            Some(matching) if matches!(toks[start].kind, T::OpenBrace | T::OpenBracket | T::OpenParen) => matching[start] as usize,
            _ => close(&toks[..hi], start),
        }
    }
}

/// `toks[lo..hi]` configured into `out`. Group ends are looked up in `groups` of the whole slice; a
/// group of the range always closes inside it (the range is a balanced interior).
fn active_tokens_inner(src: &[u8], toks: &[Tok], lo: usize, hi: usize, cfg: &CfgSet, groups: &Groups, out: &mut Vec<Tok>) {
    let output_start = out.len();
    let mut i = lo;
    while i < hi {
        // Macro arguments/transcribers are token trees, not configured AST yet.
        if is_word(src, &toks[i], "macro_rules") && i + 3 < hi {
            let end = groups.close(toks, hi, i + 3);
            out.extend_from_slice(&toks[i..=end]);
            i = end + 1;
            continue;
        }
        if toks[i].kind.is_ident_like() && toks.get(i + 1).filter(|_| i + 1 < hi).is_some_and(|t| t.kind == T::Not)
            && toks.get(i + 2).filter(|_| i + 2 < hi).is_some_and(|t| matches!(t.kind, T::OpenBrace | T::OpenParen | T::OpenBracket)) {
            let end = groups.close(toks, hi, i + 2);
            out.extend_from_slice(&toks[i..=end]);
            i = end + 1;
            continue;
        }
        if toks[i].kind == T::Pound {
            let mut attrs = Vec::new();
            let start = i;
            while i < hi && toks[i].kind == T::Pound {
                let inner = toks.get(i + 1).filter(|_| i + 1 < hi).is_some_and(|t| t.kind == T::Not);
                // Inner attributes configure this scope; outer attributes configure
                // the following item. A disabled first item must not erase the file
                // (or inline module) that carries an unrelated inner attribute.
                if attrs.first().is_some_and(|a: &Attr| a.inner != inner) { break; }
                let open = i + 1 + usize::from(inner);
                if !toks.get(open).filter(|_| open < hi).is_some_and(|t| t.kind == T::OpenBracket) { break; }
                let end = groups.close(toks, hi, open);
                attrs.push(Attr { inner, toks: TokRange { lo: (open + 1) as u32, hi: end as u32 } });
                i = end + 1;
            }
            if attrs.is_empty() { out.push(toks[i]); i += 1; continue; }
            let view = Src { src, plain: toks, ext: &[], parts: None };
            if !crate::cfg::active(view, &attrs, cfg) {
                if attrs.iter().any(|a| a.inner) { out.truncate(output_start); return; }
                // The attributed item/statement/field is removed as one unit.
                while i < hi {
                    match toks[i].kind {
                        T::OpenBrace => { i = groups.close(toks, hi, i) + 1; break; }
                        T::OpenParen | T::OpenBracket => i = groups.close(toks, hi, i) + 1,
                        T::Semi | T::Comma => { i += 1; break; }
                        _ => i += 1,
                    }
                }
                continue;
            }
            for a in crate::cfg::expand_cfg_attr_list(view, &attrs, cfg) {
                let alo = a.toks.lo as usize;
                let ahi = a.toks.hi as usize;
                if alo >= ahi || is_word(src, &toks[alo], "cfg") { continue; }
                out.push(toks[start]);
                if a.inner { out.push(Tok { kind: T::Not, ..toks[start] }); }
                out.push(Tok { kind: T::OpenBracket, ..toks[start] });
                out.extend_from_slice(&toks[alo..ahi]);
                out.push(Tok { kind: T::CloseBracket, ..toks[ahi] });
            }
            continue;
        }
        if matches!(toks[i].kind, T::OpenBrace | T::OpenParen | T::OpenBracket) {
            let end = groups.close(toks, hi, i);
            out.push(toks[i]);
            active_tokens_inner(src, toks, i + 1, end, cfg, groups, out);
            out.push(toks[end]);
            i = end + 1;
        } else {
            out.push(toks[i]);
            i += 1;
        }
    }
}

#[path = "demand_bindings.rs"]
mod bindings;
use bindings::{Binding, Bindings};
#[path = "demand_view.rs"]
mod view;
use view::{ModuleCtx, PathSeg, Scratch, Sym, prim_index, sort_dedup, FLAG_ALL_MODULES, FLAG_EMPTY, FLAG_MACRO, FLAG_MODULE, FLAG_SHADOWED, FLAG_TYPE, FLAG_VALUE};
pub(crate) use view::{Sym as NameSym, SymCache};

#[derive(Default)]
pub struct Names {
    crates: Arc<[PreCrate]>,
    symbols: BTreeSet<String>,
    type_symbols: HashSet<String>,
    trait_symbols: HashSet<String>,
    value_symbols: HashSet<String>,
    public: HashSet<String>,
    modules: HashSet<String>,
    uses: BTreeMap<String, Vec<crate::macros::UseItem>>,
    bindings: Bindings,
    module_macros: HashSet<String>,
    macros: HashSet<String>,
    macro_uses: Vec<(String, String, bool)>,
    externs: Vec<(String, String, String, bool)>,
    empty_rules: BTreeMap<String, (usize, String, Vec<Vec<String>>)>,
    empty_macros: HashSet<String>,
    prepared: Prepared,
    view: std::sync::OnceLock<view::View>,
}
/// Namespace state carried between crates prepared in load order.
#[derive(Default)]
struct Prepared {
    crates: usize,
    strong: HashSet<(usize, usize)>,
    modules: HashSet<usize>,
    types: HashSet<usize>,
    values: HashSet<usize>,
    macros: HashSet<usize>,
    type_names: HashSet<(usize, usize)>,
}
impl Names {
    pub fn new(crates: &[PreCrate]) -> Self { Self { crates: crates.into(), ..Self::default() } }

    pub fn fork(&self) -> Self { Self { crates: self.crates.clone(), ..Self::default() } }
    pub fn merge(&mut self, other: Self, keep_bindings: bool) {
        self.reset_view();
        // prepare() discards these namespace surfaces for unrelated crates.
        // Do that before merging thousands of names into the global tables.
        if keep_bindings {
            self.symbols.extend(other.symbols);
            self.modules.extend(other.modules);
            for (module, uses) in other.uses { self.uses.entry(module).or_default().extend(uses); }
        }
        self.type_symbols.extend(other.type_symbols);
        self.trait_symbols.extend(other.trait_symbols);
        self.value_symbols.extend(other.value_symbols);
        self.public.extend(other.public);
        self.module_macros.extend(other.module_macros);
        self.macros.extend(other.macros);
        self.macro_uses.extend(other.macro_uses);
        self.externs.extend(other.externs);
        self.empty_rules.extend(other.empty_rules);
    }
    pub fn add(&mut self, ci: usize, module: &str, src: &[u8], toks: &[Tok], _uses: &[crate::macros::UseItem], mods: &[String]) {
        self.reset_view();
        self.modules.extend(mods.iter().cloned());
        self.modules.insert(module.into());
        self.scan(ci, module, src, toks);
        for name in fixed_function_macros(src, toks).values().flatten() {
            self.symbols.insert(format!("{module}::{name}"));
            self.public.insert(format!("{module}::{name}"));
        }
    }
    fn scan(&mut self, ci: usize, module: &str, src: &[u8], toks: &[Tok]) {
        let mut i = 0;
        let mut exported = false;
        let mut macro_use = false;
        while i < toks.len() {
            // only an identifier is compared as text below
            let text = if matches!(toks[i].kind, T::Ident | T::RawIdent) {
                std::str::from_utf8(&src[toks[i].lo as usize..toks[i].hi as usize]).map_or("\u{fffd}", |s| s.trim_start_matches("r#"))
            } else { "" };
            let public = i > 0 && matches!(toks[i - 1].kind, T::KwPub | T::CloseParen);
            if toks[i].kind == T::Pound && toks.get(i + 1).is_some_and(|t| t.kind == T::OpenBracket) {
                let end = close(toks, i + 1);
                exported |= toks[i + 2..end].iter().any(|t| is_word(src, t, "macro_export"));
                macro_use |= toks[i + 2..end].iter().any(|t| is_word(src, t, "macro_use"));
                i = end + 1;
                continue;
            }
            if toks[i].kind == T::KwExtern && i + 2 < toks.len() && toks[i + 1].kind == T::KwCrate {
                let name = word(src, &toks[i + 2]);
                let alias = if toks.get(i + 3).is_some_and(|t| t.kind == T::KwAs) {
                    toks.get(i + 4).map(|t| word(src, t)).unwrap_or_else(|| name.clone())
                } else { name.clone() };
                let target = if name == "self" { Some(self.crates[ci].name.clone()) }
                    else { self.crates[ci].deps.iter().find(|(alias, _)| alias == &name).map(|(_, root)| root.clone()) };
                if let Some(target) = target { self.externs.push((module.into(), alias, target, public)); }
            }
            if toks[i].kind == T::KwExtern && macro_use && i + 2 < toks.len() && toks[i + 1].kind == T::KwCrate {
                let name = word(src, &toks[i + 2]);
                if let Some((_, target)) = self.crates[ci].deps.iter().find(|(alias, _)| alias == &name) {
                    self.macro_uses.push((module.into(), target.clone(), true));
                }
                macro_use = false;
            }
            if toks[i].kind == T::KwUse {
                let end = i + toks[i..].iter().position(|t| t.kind == T::Semi).unwrap_or(toks.len() - i);
                let mut uses = Vec::new();
                let tx = |k: usize| word(src, &toks[k]);
                super::super::use_tree(&tx, toks, i + 1, end, &[], module, public, &mut uses);
                self.uses.entry(module.into()).or_default().extend(uses);
                i = end + usize::from(end < toks.len());
                continue;
            }
            if toks[i].kind == T::KwMod && i + 2 < toks.len() {
                let child = format!("{module}::{}", word(src, &toks[i + 1]));
                self.modules.insert(child.clone());
                if macro_use { self.macro_uses.push((module.into(), child.clone(), false)); }
                macro_use = false;
                if public { self.public.insert(child.clone()); }
                if toks[i + 2].kind == T::OpenBrace {
                    let end = close(toks, i + 2);
                    self.scan(ci, &child, src, &toks[i + 3..end]);
                    i = end + 1;
                    continue;
                }
            }
            if matches!(toks[i].kind, T::KwFn | T::KwStruct | T::KwEnum | T::KwTrait | T::KwType | T::KwConst | T::KwStatic)
                || matches!(text, "union" | "style" | "widget" | "app" | "theme" | "sheet") {
                if let Some(t) = toks.get(i + 1).filter(|t| matches!(t.kind, T::Ident | T::RawIdent)) {
                    let name = format!("{module}::{}", word(src, t));
                    if matches!(toks[i].kind, T::KwStruct | T::KwEnum | T::KwTrait | T::KwType) || text == "union" {
                        self.type_symbols.insert(name.clone());
                    }
                    if toks[i].kind == T::KwTrait { self.trait_symbols.insert(name.clone()); }
                    if !matches!(toks[i].kind, T::KwEnum | T::KwTrait | T::KwType) && text != "union" {
                        self.value_symbols.insert(name.clone());
                    }
                    self.symbols.insert(name.clone());
                    if public { self.public.insert(name); }
                }
            }
            // `macro_rules ! name { .. }` (a bare `macro_rules`, `macro_rules! {}` and `tests!(macro_rules)` define nothing)
            if text == "macro_rules" && i + 3 < toks.len() && toks[i + 1].kind == T::Not
                && matches!(toks[i + 2].kind, T::Ident | T::RawIdent)
                && matches!(toks[i + 3].kind, T::OpenBrace | T::OpenParen | T::OpenBracket) {
                let name = word(src, &toks[i + 2]);
                self.symbols.insert(format!("{module}::{name}"));
                let end = close(toks, i + 3);
                if toks[i + 4..end].iter().any(|t| t.kind == T::KwMod) {
                    self.module_macros.insert(format!("{module}::{name}"));
                }
                let qualified = format!("{module}::{name}");
                if let Some(calls) = empty_rule_calls(src, &toks[i + 4..end]) {
                    self.empty_rules.insert(qualified.clone(), (ci, module.into(), calls));
                }
                self.macros.insert(qualified);
                if exported {
                    let root = &self.crates[ci].name;
                    self.uses.entry(root.clone()).or_default().push(crate::macros::UseItem {
                        module: root.clone(), path: module.split("::").chain(std::iter::once(name.as_str())).map(str::to_owned).collect(), bind: Some(name), is_pub: true,
                    });
                }
                exported = false;
            }
            if matches!(toks[i].kind, T::OpenBrace | T::OpenParen | T::OpenBracket) {
                let end = close(toks, i);
                if toks[i].kind == T::OpenBrace { exported = false; macro_use = false; }
                i = end + 1;
            } else { i += 1; }
        }
    }

    fn absolute<P: Borrow<str>>(&self, ci: usize, module: &str, path: &[P]) -> Vec<String> {
        let mut at = 0;
        if path.first().is_some_and(|p| p.borrow().is_empty()) { at += 1; }
        let Some(first) = path.get(at) else { return Vec::new(); };
        at += 1;
        let base = match first.borrow() {
            "crate" => self.crates[ci].name.clone(),
            "self" => {
                let mut base = module;
                while path.get(at).is_some_and(|x| x.borrow() == "super") { at += 1; base = base.rsplit_once("::").map_or(base, |x| x.0); }
                base.to_owned()
            }
            "super" => {
                let mut base = module.rsplit_once("::").map_or(module, |x| x.0);
                while path.get(at).is_some_and(|x| x.borrow() == "super") { at += 1; base = base.rsplit_once("::").map_or(base, |x| x.0); }
                base.to_owned()
            }
            name => {
                // A child module of this module shadows the extern prelude, as in rustc:
                // `pub use core::{EngineCore, ..}` in a module with a child `mod core` names that
                // child, not the core crate.
                let local = format!("{module}::{name}");
                if self.modules.contains(&local) { local }
                // The loader also supplies the implicit core/alloc/std graph.
                // Those names need their real root in Rust file-selection mode;
                // treating std::arch as a child of this module loses its import.
                else if let Some((_, root)) = self.crates[ci].deps.iter()
                    .chain(self.crates[ci].image_dep_names.iter().filter(|_| self.crates[ci].cfg.session.options.rs_lazy))
                    .find(|(alias, _)| alias == name) { root.clone() }
                else if self.crates[ci].name == name { name.into() }
                else { format!("{module}::{name}") }
            }
        };
        let tail = path[at..].join("::");
        vec![if tail.is_empty() { base } else { format!("{base}::{tail}") }]
    }
    /// Bind one crate's declarations and evaluate its imports to their fixed point.
    /// Imports only add bindings to scopes of their own crate and read crates
    /// earlier in load order, so a crate is final once its dependencies are; the
    /// cold planner prepares each crate as soon as its outline expansion is sealed.
    pub fn prepare_crate(&mut self, ci: usize) {
        self.prepare_crate_with(ci, None);
    }
    fn prepare_crate_with(&mut self, ci: usize, saved: Option<BindingImage>) {
        self.reset_view();
        let mut prepared = std::mem::take(&mut self.prepared);
        if prepared.crates == 0 {
            for krate in self.crates.iter() { self.bindings.intern(&krate.name); }
        }
        prepared.crates = prepared.crates.max(ci + 1);
        let krate = self.crates[ci].name.clone();
        let mine = |path: &str| within(path, &krate);
        // `krate;` sorts after every `krate::` path and before `krate_suffix`.
        let range = (std::ops::Bound::Included(krate.clone()), std::ops::Bound::Excluded(format!("{krate};")));
        let symbols = self.symbols.range::<String, _>(range.clone()).filter(|path| mine(path)).cloned().collect::<Vec<_>>();
        let modules = self.modules.iter().filter(|path| mine(path)).cloned().collect::<Vec<_>>();
        let strong = &mut prepared.strong;
        for path in symbols.iter().chain(&modules) {
            if let Some((module, name)) = path.rsplit_once("::") {
                let module = self.bindings.intern(module);
                let name = self.bindings.intern(name);
                let id = self.bindings.entry(module, name);
                self.bindings.values[id].targets.insert(id);
                self.bindings.values[id].public |= self.public.contains(path);
                strong.insert((module, name));
            }
        }
        for (module, uses) in self.uses.range::<String, _>(range.clone()).filter(|(path, _)| mine(path)) {
            let module = self.bindings.intern(module);
            for u in uses {
                if let Some(name) = &u.bind { strong.insert((module, self.bindings.intern(name))); }
            }
        }
        for (module, name, target, public) in self.externs.iter().filter(|e| mine(&e.0)) {
            let module = self.bindings.intern(module);
            let name = self.bindings.intern(name);
            let target = self.bindings.intern(target);
            let id = self.bindings.entry(module, name);
            self.bindings.values[id].targets.insert(target);
            self.bindings.values[id].public |= public;
            strong.insert((module, name));
        }
        prepared.modules.extend(modules.iter().filter_map(|name| self.bindings.id(name)));
        for path in &symbols {
            let Some(id) = self.bindings.id(path) else { continue; };
            if self.value_symbols.contains(path) { prepared.values.insert(id); }
            if self.macros.contains(path) { prepared.macros.insert(id); }
            if self.type_symbols.contains(path) {
                prepared.types.insert(id);
                // Only seeded declarations can become an import target.
                if let Some((module, name)) = path.rsplit_once("::") {
                    prepared.type_names.insert((self.bindings.intern(module), self.bindings.intern(name)));
                }
            }
        }
        if let Some(saved) = saved {
            // The strings are sorted by spelling, so neighbours share their module: intern
            // it once per run. A set is the sorted union of its members' ids in this table.
            let mut last = (String::new(), usize::MAX);
            let ids = saved.strings.iter().map(|name| self.bindings.intern_after(name, &mut last)).collect::<Vec<_>>();
            let sets = saved.sets.iter().map(|set| {
                let mut set = set.iter().map(|&id| ids[id]).collect::<Vec<_>>();
                set.sort_unstable();
                set.dedup();
                bindings::Names::from_sorted(set)
            }).collect::<Vec<_>>();
            for (name, public, targets, via) in saved.entries {
                // ids[name] is the interned `module::local`: its parts are the pair.
                let (module, local) = self.bindings.parts(ids[name]).expect("validated binding image");
                let id = self.bindings.entry(module, local);
                let binding = &mut self.bindings.values[id];
                binding.public = public;
                binding.targets.extend(&sets[targets]);
                binding.via.extend(&sets[via]);
            }
            self.prepared = prepared;
            return;
        }
        let (strong, modules, types, values, macros, type_names) = (&prepared.strong, &prepared.modules, &prepared.types, &prepared.values, &prepared.macros, &prepared.type_names);
        enum Import {
            Use { scope: usize, target: Vec<usize>, name: Option<usize>, public: bool },
            Macros { scope: usize, target: usize, external: bool },
        }
        let mut imports = Vec::new();
        for (scope, uses) in self.uses.range::<String, _>(range).filter(|(path, _)| mine(path)) {
            let scope_id = self.bindings.intern(scope);
            for u in uses {
                for target in self.absolute(ci, scope, &u.path) {
                    let target = self.bindings.path(&target);
                    let name = u.bind.as_ref().map(|name| self.bindings.intern(name));
                    imports.push(Import::Use { scope: scope_id, target, name, public: u.is_pub });
                }
            }
        }
        for (scope, target, external) in self.macro_uses.iter().filter(|m| mine(&m.0)) {
            imports.push(Import::Macros { scope: self.bindings.intern(scope), target: self.bindings.intern(target), external: *external });
        }
        // Subscribe to missing segments as well as existing bindings. Interned
        // (module, name) pairs keep failed lookups observable without allocating
        // their qualified spellings again on each fixed-point visit.
        let profile = self.crates.first().is_some_and(|c| c.cfg.session.runtime.profile);
        let imports_profile = crate::load_profile::span(profile, "rust-demand-imports", &krate);
        let mut watchers = HashMap::<(usize, Option<usize>), BTreeSet<usize>>::default();
        // A glob subscribes to changes in its target module. After its first
        // visit it only reads bindings changed since that visit, rather than
        // enumerating the entire target to test a revision on every binding.
        // These are worklist cursors, discarded when this fixed point finishes.
        let mut changes = HashMap::<usize, Vec<usize>>::default();
        let mut forwarded = vec![HashMap::<usize, usize>::default(); imports.len()];
        let mut routes = vec![None; imports.len()];
        let mut evaluated = 0;
        let mut visits = 0usize;
        let mut full_visits = 0usize;
        let mut contributions = 0usize;
        let mut updates = 0usize;
        let mut pending = (0..imports.len()).collect::<std::collections::VecDeque<_>>();
        let mut queued = vec![true; imports.len()];
        while let Some(id) = pending.pop_front() {
            queued[id] = false;
            evaluated += 1;
            let mut watched = BTreeSet::new();
            let mut additions = Vec::new();
            let scope = match &imports[id] {
                Import::Use { scope, target, name, public } => {
                    let mut binding = self.bindings.watched(target, |module, name| { watched.insert((module, Some(name))); });
                    binding.public = *public;
                    if !binding.targets.is_empty() {
                        if let Some(name) = name {
                            additions.push((*name, binding));
                        } else {
                            // A changed route contributes new provenance to
                            // every forwarded binding, including old names.
                            if routes[id].as_ref() != Some(&binding) {
                                forwarded[id].clear();
                                routes[id] = Some(binding.clone());
                            }
                            for module in &binding.targets {
                                watched.insert((module, None));
                                // A child can glob its parent's private imports
                                // (`use super::*`); outsiders see public names.
                                let private = within(self.bindings.spelling(*scope), self.bindings.spelling(module));
                                full_visits += self.bindings.modules.get(&module).map_or(0, HashMap::len);
                                let journal = changes.get(&module).map_or(&[][..], Vec::as_slice);
                                let cursor = forwarded[id].insert(module, journal.len());
                                let sources = if let Some(cursor) = cursor {
                                    let mut names = journal[cursor..].to_vec();
                                    names.sort_unstable();
                                    names.dedup();
                                    names.into_iter().filter_map(|name| self.bindings.modules.get(&module)
                                        .and_then(|entries| entries.get(&name)).map(|&source| (name, source))).collect::<Vec<_>>()
                                } else {
                                    self.bindings.modules.get(&module).into_iter().flatten()
                                        .map(|(&name, &source)| (name, source)).collect()
                                };
                                visits += sources.len();
                                for (name, source) in sources {
                                    let value = &self.bindings.values[source];
                                    if !value.public && !private { continue; }
                                    if strong.contains(&(*scope, name)) && value.targets.iter().all(|target| modules.contains(&target)) { continue; }
                                    if type_names.contains(&(*scope, name)) && value.targets.iter().all(|target|
                                        types.contains(&target) && !values.contains(&target) && !macros.contains(&target)) { continue; }
                                    // A new wave revisits existing globs with fresh revision
                                    // counters. Most destinations already contain this exact
                                    // contribution: avoid cloning its target/provenance sets
                                    // only when extend below would be a no-op, including both
                                    // the route and destination provenance.
                                    if let Some(&destination) = self.bindings.modules.get(scope).and_then(|names| names.get(&name)) {
                                        let current = &self.bindings.values[destination];
                                        if (!*public || current.public) && current.targets.contains_all(&value.targets)
                                            && current.via.contains_all(&value.via) && current.via.contains_all(&binding.via)
                                            && current.via.contains(source) && current.via.contains(destination) { continue; }
                                    }
                                    let mut value = value.clone();
                                    value.public = *public;
                                    value.via.extend(&binding.via);
                                    value.via.insert(source);
                                    additions.push((name, value));
                                }
                            }
                        }
                    }
                    *scope
                }
                Import::Macros { scope, target, external } => {
                    watched.insert((*target, None));
                    for (&name, &source) in self.bindings.modules.get(target).into_iter().flatten() {
                        visits += 1;
                        full_visits += 1;
                        let value = &self.bindings.values[source];
                        if (*external && !value.public) || !value.targets.iter().any(|target| macros.contains(&target)) { continue; }
                        let mut value = value.clone();
                        value.public = false;
                        additions.push((name, value));
                    }
                    *scope
                }
            };
            for module in watched { watchers.entry(module).or_default().insert(id); }
            let mut changed = Vec::new();
            contributions += additions.len();
            for (name, mut value) in additions {
                let destination = self.bindings.entry(scope, name);
                value.via.insert(destination);
                if self.bindings.values[destination].extend(&value) {
                    changes.entry(scope).or_default().push(name);
                    updates += 1;
                    changed.push(Some(name));
                }
            }
            if !changed.is_empty() {
                changed.push(None);
                for name in changed {
                    for &dependent in watchers.get(&(scope, name)).into_iter().flatten() {
                        if !queued[dependent] { queued[dependent] = true; pending.push_back(dependent); }
                    }
                }
            }
        }
        let _ = (strong, modules, types, values, macros, type_names);
        self.prepared = prepared;
        if profile {
            eprintln!("rust-demand-imports: crate={krate:?} crate_imports={} evaluated={evaluated} visits={visits} full_visits={full_visits} contributions={contributions} updates={updates} bindings={}",
                imports.len(), self.bindings.modules.values().map(|names| names.len()).sum::<usize>());
        }
        drop(imports_profile);
    }
    pub fn finish_prepare(&mut self) {
        let profile = self.crates.first().is_some_and(|c| c.cfg.session.runtime.profile);
        let _profile = crate::load_profile::span(profile, "rust-demand-empty-macros", "load");
        // Every finite expansion of a closed set of empty/forwarding rules is
        // empty, including recursive convenience arms. Prune the candidates
        // that can reach an unknown or token-producing rule. This proves only
        // the output surface, not termination: selected invocations still go
        // through the real expander and retain its matching/recursion errors.
        self.empty_macros = self.empty_rules.keys().cloned().collect();
        loop {
            let rejected = self.empty_rules.iter().filter(|(name, (ci, scope, calls))| {
                self.empty_macros.contains(*name) && calls.iter().any(|path| !self.empty_macro_strs(*ci, scope, path, true))
            }).map(|(name, _)| name.clone()).collect::<Vec<_>>();
            if rejected.is_empty() { break; }
            for name in rejected { self.empty_macros.remove(&name); }
        }
        self.reset_view();
    }
    pub fn entry(&self, ci: usize, name: &str) -> BTreeSet<String> {
        self.resolve(ci, &self.crates[ci].name, &[name])
    }
    pub fn empty_macro<P: Borrow<str>>(&self, ci: usize, module: &str, path: &[P]) -> bool { self.empty_macro_strs(ci, module, path, false) }
    /// Outline a local declaration macro with name-only arguments. Matching
    /// types/identifiers and transcribing tokens does not parse emitted bodies.
    /// An impl owned by a type declared here cannot require this file before
    /// that type does; empty helper invocations contribute no surface either.
    pub fn local_impl_macro(&self, ci: usize, module: &str, src: &[u8], toks: &[Tok], call: usize, open: usize) -> bool {
        self.local_impl_output(ci, module, src, toks, call, open).is_some()
    }
    pub fn local_impl_output(&self, ci: usize, module: &str, src: &[u8], toks: &[Tok], call: usize, open: usize) -> Option<(String, Vec<Tok>)> {
        let end = close(toks, open);
        if toks[open + 1..end].iter().any(|t| !matches!(t.kind,
            T::Ident | T::RawIdent | T::PathSep | T::Comma | T::Semi | T::FatArrow)) { return None; }
        let name = word(src, &toks[call]);
        let mut definition = None;
        let mut at = 0;
        while at + 3 < call {
            if is_word(src, &toks[at], "macro_rules") {
                let last = close(toks, at + 3);
                if word(src, &toks[at + 2]) == name { definition = Some(&toks[at + 4..last]); }
                at = last + 1;
            } else if matches!(toks[at].kind, T::OpenBrace | T::OpenParen | T::OpenBracket) { at = close(toks, at) + 1; }
            else { at += 1; }
        }
        let Some(definition) = definition else { return None; };
        let mtoks = definition.iter().map(|t| crate::macros::MTok {
            kind: t.kind, text: String::from_utf8_lossy(&src[t.lo as usize..t.hi as usize]).into_owned(), pos: t.lo,
        }).collect::<Vec<_>>();
        let c = &self.crates[ci];
        let Ok(def) = crate::macros::parse_def(&name, &c.name, c.edition, &mtoks) else { return None; };
        fn names_only(nodes: &[crate::macros::MNode]) -> bool {
            use crate::macros::{Frag, MNode};
            nodes.iter().all(|n| match n {
                MNode::Var(_, frag) => matches!(frag, Frag::Ident | Frag::Ty | Frag::Path),
                MNode::Rep(nodes, _, _) | MNode::Group(_, _, nodes) => names_only(nodes),
                MNode::Tok(_) => true,
            })
        }
        if !def.arms.iter().all(|(matcher, _)| names_only(matcher)) { return None; }
        let mut parser = crate::parser::Parser::new(&c.cfg.session, src, toks, c.edition);
        parser.krate = c.name.clone();
        let Ok((expanded, mut tokens, _, _, _)) = parser.expand_rules(&def, open + 1, end) else { return None; };
        tokens.push(Tok { kind: T::Eof, lo: expanded.len() as u32, hi: expanded.len() as u32 });
        let bytes = expanded.as_bytes();
        let mut at = 0;
        while at < tokens.len() && tokens[at].kind != T::Eof {
            if tokens[at].kind == T::Semi { at += 1; continue; }
            if tokens[at].kind == T::KwImpl {
                let Some(open) = (at + 1..tokens.len()).find(|&i| tokens[i].kind == T::OpenBrace) else { return None; };
                let end = close(&tokens, open);
                let Ok(stream) = crate::pm_tokens::from_toks(bytes, &tokens[at..=end], &|_, _| crate::pm_tokens::Span::DUMMY) else { return None; };
                let index = crate::rs_index::of_tokens(&stream);
                if index.impls.len() != 1 || !self.type_symbols.contains(&format!("{module}::{}", index.impls[0].self_path)) { return None; }
                at = end + 1;
                continue;
            }
            let mut path = Vec::new();
            loop {
                if !tokens.get(at).is_some_and(|t| matches!(t.kind, T::Ident | T::RawIdent | T::KwCrate | T::KwSelfValue | T::KwSuper)) { return None; }
                path.push(word(bytes, &tokens[at])); at += 1;
                if !tokens.get(at).is_some_and(|t| t.kind == T::PathSep) { break; }
                at += 1;
            }
            if !tokens.get(at).is_some_and(|t| t.kind == T::Not)
                || !tokens.get(at + 1).is_some_and(|t| matches!(t.kind, T::OpenBrace | T::OpenParen | T::OpenBracket))
                || !self.empty_macro(ci, module, &path) { return None; }
            at = close(&tokens, at + 1) + 1;
        }
        Some((expanded, tokens))
    }
    fn resolve<P: Borrow<str>>(&self, ci: usize, module: &str, path: &[P]) -> BTreeSet<String> { self.resolve_strs(ci, module, path) }
    fn resolve_macro<P: Borrow<str>>(&self, ci: usize, module: &str, path: &[P]) -> BTreeSet<String> { self.resolve_macro_strs(ci, module, path) }
    pub fn resolve_path(&self, ci: usize, module: &str, path: &[String]) -> BTreeSet<String> { self.resolve(ci, module, path) }
    /// The symbol of a spelling (see `demand_view.rs`).
    pub fn sym(&self, cache: &mut SymCache, text: &str) -> Sym { self.view().sym(&self.bindings, cache, text) }
    pub fn spelling_of(&self, s: Sym) -> String { self.view().spelling(&self.bindings, s) }
    /// The module a qualified name is in.
    pub fn scope_of(&self, s: Sym) -> Option<Sym> { self.view().parts(&self.bindings, s).map(|(m, _)| m) }
    pub fn is_type_sym(&self, s: Sym) -> bool { self.flag(self.view(), s, FLAG_TYPE) }
    pub fn is_trait(&self, name: &str) -> bool { self.trait_symbols.contains(name) }
    pub fn is_type(&self, name: &str) -> bool { self.type_symbols.contains(name) }
    /// Names the compiler itself looks up in the std family (typeck's
    /// `lang_item` core paths, builtin macro output, the runtime entry), and
    /// every prelude binding: prelude traits are in scope for method calls in
    /// every module without being named. These replace a forced core crate.
    pub fn implicit_demands(&self) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        let Some(ci) = self.crates.iter().position(|c| c.name == "core") else { return out; };
        for path in ["prelude", "ops", "cmp", "marker", "clone", "convert", "default", "iter", "future", "fmt", "panicking",
            "result", "option", "boxed", "string", "vec", "ffi::CStr", "cell::UnsafeCell", "hint", "intrinsics", "mem", "ptr", "slice", "str"] {
            out.extend(self.resolve(ci, "core", &path.split("::").map(str::to_owned).collect::<Vec<_>>()));
        }
        for edition in ["rust_2015", "rust_2018", "rust_2021", "rust_2024"] {
            if let Some(prelude) = self.bindings.id(&format!("core::prelude::{edition}")) {
                for (_, &id) in self.bindings.modules.get(&prelude).into_iter().flatten() {
                    let binding = &self.bindings.values[id];
                    out.extend(binding.targets.iter().chain(binding.via.iter()).map(|id| self.bindings.spelling(id).to_string()));
                }
            }
        }
        // Operators, comparisons, formatting, iteration, conversions, hashing
        // and markers reach their traits through syntax and builtin expansion,
        // not through a name in the source.
        for module in ["ops", "cmp", "fmt", "iter", "marker", "convert", "hash", "borrow", "default", "clone", "future", "any", "str", "error"] {
            let scope = format!("core::{module}");
            out.extend(self.trait_symbols.iter().filter(|name| within(name, &scope)).cloned());
        }
        if let Some(ci) = self.crates.iter().position(|c| c.name == "std") {
            for path in ["process", "relax", "panic", "rt"] {
                out.extend(self.resolve(ci, "std", &[path.to_owned()]));
            }
        }
        out
    }
    pub fn qualify_index(&mut self, file: &mut FileIndex) {
        self.reset_view();
        for name in &file.glob_names { self.public.insert(format!("{}::{name}", file.module)); }
        for name in std::mem::take(&mut file.names) {
            let name = format!("{}::{name}", file.module);
            self.symbols.insert(name.clone());
            file.names.insert(name);
        }
    }
    pub fn qualify_impls(&self, file: &mut FileIndex) {
        file.impls = std::mem::take(&mut file.impls).into_iter().flat_map(|(tr, ty)| {
            let own = format!("{}::{ty}", file.module);
            let mut names = if self.type_symbols.contains(&own) { [own].into() }
                else { self.resolve(file.ci, &file.module, &ty.split("::").map(str::to_owned).collect::<Vec<_>>()) };
            if names.is_empty() { names.insert(format!("{}::{ty}", file.module)); }
            names.into_iter().map(move |name| (tr.clone(), name))
        }).collect();
        file.blankets = std::mem::take(&mut file.blankets).into_iter().flat_map(|tr| self.resolve(file.ci, &file.module, &tr.split("::").map(str::to_owned).collect::<Vec<_>>())).collect();
    }
    pub fn qualify_rust_impls(&self, file: &mut FileIndex, index: &crate::rs_index::NameIndex, parameters: &BTreeSet<String>, cfg: &CfgSet) {
        fn walk(names: &Names, out: &mut FileIndex, index: &crate::rs_index::NameIndex, module: &str, parameters: &BTreeSet<String>, cfg: &CfgSet) {
            let mut scoped = FileIndex { ci: out.ci, path: out.path.clone(), module: module.into(), names: BTreeSet::new(), glob_names: BTreeSet::new(), impls: Vec::new(), blankets: BTreeSet::new(), force: false };
            let direct = crate::rs_index::NameIndex { impls: index.impls.clone(), ..Default::default() };
            super::index_names(&direct, parameters, &mut scoped, cfg);
            // A generic spelling can also name a concrete type in another impl
            // in this file. Retain both candidates instead of erasing that head.
            for imp in &direct.impls {
                let head = imp.self_path.split('<').next().unwrap_or("").trim();
                if parameters.contains(head) { scoped.impls.push((imp.trait_path.clone(), head.into())); }
                if imp.trait_path.as_ref().is_some_and(|tr| tr.starts_with('!')) { scoped.force = true; }
            }
            let had_blankets = !scoped.blankets.is_empty();
            names.qualify_impls(&mut scoped);
            out.force |= scoped.force || had_blankets && scoped.blankets.is_empty();
            out.impls.extend(scoped.impls);
            out.blankets.extend(scoped.blankets);
            for child in &index.modules {
                if let Some(inner) = &child.inline {
                    walk(names, out, inner, &format!("{module}::{}", child.name), parameters, cfg);
                }
            }
        }
        file.impls.clear();
        file.blankets.clear();
        let module = file.module.clone();
        walk(self, file, index, &module, parameters, cfg);
    }
    pub fn opaque_scopes(&self, ci: usize, src: &[u8], toks: &[Tok], module: &str) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        let mut attrs = false;
        let mut i = 0;
        while i < toks.len() {
            if is_word(src, &toks[i], "macro_rules") && i + 3 < toks.len() {
                i = close(toks, i + 3) + 1;
                continue;
            }
            if toks[i].kind == T::Pound && toks.get(i + 1).is_some_and(|t| t.kind == T::OpenBracket) {
                let end = close(toks, i + 1);
                attrs |= toks[i + 2..end].iter().any(|t| crate::macro_reg::has_attr(&word(src, t)));
                i = end + 1;
                continue;
            }
            if toks[i].kind == T::KwMod && i + 2 < toks.len() {
                // An attribute replaces this item in its enclosing module.
                if attrs { out.insert(module.into()); }
                attrs = false;
                if toks[i + 2].kind == T::OpenBrace {
                    let end = close(toks, i + 2);
        
                    out.extend(self.opaque_scopes(ci, src, &toks[i + 3..end], &format!("{module}::{}", word(src, &toks[i + 1]))));
                    i = end + 1;
                    continue;
                }
            }
            if matches!(toks[i].kind, T::KwFn | T::KwStruct | T::KwEnum | T::KwImpl | T::KwTrait | T::Semi) { attrs = false; }
            if matches!(toks[i].kind, T::Ident | T::RawIdent | T::KwCrate | T::KwSelfValue | T::KwSuper) {
                let mut path = vec![word(src, &toks[i])];
                let mut end = i + 1;
                while end + 1 < toks.len() && toks[end].kind == T::PathSep {
                    path.push(word(src, &toks[end + 1])); end += 2;
                }
                if toks.get(end).is_some_and(|t| t.kind == T::Not) {
                    if self.empty_macro(ci, module, &path)
                        && toks.get(end + 1).is_some_and(|t| matches!(t.kind, T::OpenBrace | T::OpenParen | T::OpenBracket)) {
                        i = close(toks, end + 1) + 1;
                        continue;
                    }
                    let resolved = self.resolve_macro(ci, module, &path);
                    if path.last().is_some_and(|name| name == "include")
                        || resolved.iter().any(|name| self.module_macros.contains(name)) {
                        out.insert(module.into());
                    }
                    // A proc macro's argument may itself supply module items.
                    if toks.get(end + 1).is_some_and(|t| matches!(t.kind, T::OpenBrace | T::OpenParen | T::OpenBracket)) {
                        let close = close(toks, end + 1);
                        if toks[end + 2..close].iter().any(|t| t.kind == T::KwMod) { out.insert(module.into()); }
                        i = close + 1;
                        continue;
                    }
                }
            }
            // Function/impl/struct bodies cannot declare sibling modules.
            if matches!(toks[i].kind, T::OpenBrace | T::OpenParen | T::OpenBracket) { i = close(toks, i) + 1; }
            else { i += 1; }
        }
        out
    }
    pub fn references(&self, ci: usize, module: &str, src: &[u8], toks: &[Tok]) -> BTreeSet<String> {
        let v = self.view();
        let mut out = Vec::new();
        self.references_shared(ci, module, src, toks, &mut Lookups::default(), &mut out);
        out.into_iter().map(|s| v.spelling(&self.bindings, s)).collect()
    }
    /// The names one token range demands, as sorted symbols appended to `out`. A path the caller's
    /// earlier ranges already resolved (outside any block import of this range, which could change
    /// it) is not resolved again.
    pub fn references_shared(&self, ci: usize, module: &str, src: &[u8], toks: &[Tok], shared: &mut Lookups, out: &mut Vec<Sym>) {
        let v = self.view();
        let b = &self.bindings;
        let start = out.len();
        let Lookups { scope, scope_gen, ctx, resolved: shared_resolved, arena, trie, seen, call, cache, scratch, hits, misses, glob_queries, glob_bindings } = shared;
        if scope.as_ref().map_or(true, |(c, m)| *c != ci || m != module) {
            *scope = Some((ci, module.to_owned()));
            *ctx = Some(self.module_ctx(v, module));
            *scope_gen += 1;
            arena.clear();
        }
        let m = ctx.as_ref().unwrap();
        *call += 1;
        let rs_lazy = self.crates[ci].cfg.session.options.rs_lazy;
        let seg_of = |t: &Tok| -> PathSeg<'_> {
            let text = match std::str::from_utf8(&src[t.lo as usize..t.hi as usize]) {
                Ok(text) => text.trim_start_matches("r#"),
                Err(_) => "\u{fffd}",
            };
            PathSeg { text, id: b.segment_id(text) }
        };
        // Variant declarations are not value lookups through a glob. Keep
        // their payload/discriminant tokens, which may name external types.
        // Generic/attributed shapes we cannot outline here stay conservative.
        let mut variants = Vec::<usize>::new();
        for at in 0..toks.len().saturating_sub(2) {
            if toks[at].kind != T::KwEnum || toks[at + 2].kind != T::OpenBrace { continue; }
            let end = close(toks, at + 2);
            let mut next = at + 3;
            let mut head = true;
            let mut declared = Vec::new();
            while next < end {
                // A discriminant can contain top-level generic commas. Leave
                // that enum to the conservative reference walk.
                if toks[next].kind == T::Eq { declared.clear(); break; }
                if head && matches!(toks[next].kind, T::Ident | T::RawIdent) { declared.push(next); head = false; }
                if toks[next].kind == T::Comma { head = true; }
                if matches!(toks[next].kind, T::OpenBrace | T::OpenParen | T::OpenBracket) { next = close(toks, next) + 1; }
                else { next += 1; }
            }
            variants.extend(declared);
        }
        variants.sort_unstable();
        variants.dedup();
        // Block imports are lexical to this source. A conservative union of
        // their candidates may overretain within this file, but never exports
        // those private bindings into another module's glob lookup.
        let mut local = HashMap::<String, Vec<Sym>>::default();
        let mut glob_names: Option<Vec<Sym>> = None;
        let mut local_types = Vec::<(usize, usize, String)>::new();
        let mut blocks = Vec::new();
        let mut at = 0;
        let mut depth = 0usize;
        while at < toks.len() {
            if toks[at].kind == T::KwMod && toks.get(at + 2).is_some_and(|t| t.kind == T::OpenBrace) {
                at = close(toks, at + 2) + 1;
                continue;
            }
            if is_word(src, &toks[at], "macro_rules") && at + 3 < toks.len() {
                at = close(toks, at + 3) + 1;
                continue;
            }
            if toks[at].kind == T::KwUse {
                let end = at + toks[at..].iter().position(|t| t.kind == T::Semi).unwrap_or(toks.len() - at);
                // Module imports are already resolved by prepare(), including
                // local declarations shadowing globs. Re-adding them as block
                // imports would resurrect the shadowed candidates.
                if depth == 0 { at = end + usize::from(end < toks.len()); continue; }
                let mut uses = Vec::new();
                super::super::use_tree(&|k| word(src, &toks[k]), toks, at + 1, end, &[], module, false, &mut uses);
                for u in uses {
                    let segs = self.strs_path(&u.path);
                    let mut candidates = Vec::new();
                    self.resolve_core(v, cache, ci, m, &segs, &mut candidates, scratch);
                    sort_dedup(&mut candidates);
                    if let Some(name) = u.bind {
                        // `resolve` carries the modules on the reexport route
                        // as well as the imported declaration. Only terminal
                        // declarations determine the imported namespace.
                        let mut targets = Vec::new();
                        for &candidate in &candidates {
                            let mut segs = std::mem::take(&mut scratch.segs);
                            segs.clear();
                            v.segs_of(b, candidate, &mut segs);
                            self.walk(&segs, scratch);
                            scratch.segs = segs;
                            targets.extend_from_slice(&scratch.t);
                        }
                        sort_dedup(&mut targets);
                        targets.retain(|&t| !self.flag(v, t, FLAG_MODULE));
                        if !targets.is_empty() && targets.iter().all(|&t| self.flag(v, t, FLAG_TYPE)
                            && !self.flag(v, t, FLAG_VALUE) && !self.flag(v, t, FLAG_MACRO)) {
                            if let Some(&(lo, hi)) = blocks.last() { local_types.push((lo, hi, name.clone())); }
                        }
                        let entry = local.entry(name).or_default();
                        entry.extend_from_slice(&candidates);
                        sort_dedup(entry);
                    }
                    else {
                        for &candidate in &candidates {
                            let private = m.is_within(candidate);
                            // only an interned name is a module of the table
                            if candidate >= v.base() { continue; }
                            let module = candidate;
                            // Block globs are private to this range. Only a
                            // path's first token can consult `local` below; do
                            // not materialize every name of a large facade.
                            let names = glob_names.get_or_insert_with(|| {
                                let mut names = toks.iter().filter(|t| matches!(t.kind,
                                    T::Ident | T::RawIdent | T::KwCrate | T::KwSelfValue | T::KwSuper))
                                    .filter_map(|t| seg_of(t).id).collect::<Vec<_>>();
                                names.sort_unstable(); names.dedup(); names
                            });
                            *glob_queries += names.len();
                            let Some(entries) = b.modules.get(&(module as usize)) else { continue; };
                            for &name in names.iter() {
                                let Some(&id) = entries.get(&(name as usize)) else { continue; };
                                let binding = &b.values[id];
                                if !binding.public && !private { continue; }
                                *glob_bindings += 1;
                                let targets = local.entry(b.spelling(name as usize).to_string()).or_default();
                                targets.extend_from_slice(binding.targets.ids());
                                targets.extend_from_slice(binding.via.ids());
                                sort_dedup(targets);
                            }
                        }
                    }
                }
                at = end;
            }
            match toks.get(at).map(|t| t.kind) {
                Some(T::OpenBrace) => { depth += 1; blocks.push((at, close(toks, at))); },
                Some(T::CloseBrace) => { depth = depth.saturating_sub(1); blocks.pop(); },
                _ => {},
            }
            at += 1;
        }
        let mut implicit = 0u32;
        let mut path: Vec<PathSeg<'_>> = Vec::new();
        let mut resolved: Vec<Sym> = Vec::new();
        let mut i = 0;
        while i < toks.len() {
            // Unknown transcribers may emit references absent from the call's
            // arguments. Retain those conservatively; only a certified empty
            // rule can have its transcriber ignored altogether.
            if is_word(src, &toks[i], "macro_rules") && i + 3 < toks.len() {
                let name = seg_of(&toks[i + 2]);
                let empty = m.qid().zip(name.id).and_then(|(q, n)| v.pair_find(b, q, n)).is_some_and(|q| self.flag(v, q, FLAG_EMPTY));
                if empty {
                    i = close(toks, i + 3) + 1;
                    continue;
                }
            }
            implicit |= match toks[i].kind {
                T::Int => 0x1ffe0, T::Float => 0x18, T::Str | T::RawStr => 0x4, T::Char => 0x2,
                T::Byte => 0x800, T::KwTrue | T::KwFalse => 0x1, _ => 0,
            };
            if toks[i].kind == T::KwMod && i + 2 < toks.len() {
                if toks[i + 2].kind == T::OpenBrace {
                    let end = close(toks, i + 2);
                    let inner = format!("{module}::{}", word(src, &toks[i + 1]));
                    self.references_shared(ci, &inner, src, &toks[i + 3..end], &mut Lookups::default(), out);
                    i = end + 1;
                } else { i += 3; }
                continue;
            }
            if toks[i].kind == T::KwUse {
                let end = i + toks[i..].iter().position(|t| t.kind == T::Semi).unwrap_or(toks.len() - i);
                let mut uses = Vec::new();
                super::super::use_tree(&|k| word(src, &toks[k]), toks, i + 1, end, &[], module, false, &mut uses);
                for u in &uses {
                    let segs = self.strs_path(&u.path);
                    if rs_lazy {
                        self.import_demands_core(v, cache, ci, m, &segs, u.bind.is_none(), out, scratch);
                    } else if u.bind.is_some() {
                        self.resolve_core(v, cache, ci, m, &segs, out, scratch);
                    }
                }
                i = end + usize::from(end < toks.len());
                continue;
            }
            if toks[i].kind == T::Pound && toks.get(i + 1).is_some_and(|t| t.kind == T::OpenBracket) {
                i = close(toks, i + 1) + 1;
                continue;
            }
            if matches!(toks[i].kind, T::Ident | T::RawIdent | T::KwCrate | T::KwSelfValue | T::KwSuper) {
                if i > 0 && matches!(toks[i - 1].kind, T::KwStruct | T::KwEnum | T::KwTrait | T::KwType) {
                    // A declared type/trait reaches its external impls, but
                    // never a glob-imported homonym of that declaration.
                    let name = seg_of(&toks[i]);
                    let own = m.qid().zip(name.id).and_then(|(q, n)| v.pair_find(b, q, n));
                    if let Some(own) = own.filter(|&own| self.flag(v, own, FLAG_TYPE)) { out.push(own); }
                    i += 1; continue;
                }
                // Member names, field/parameter labels and function declarations
                // are not lookups in the surrounding module's namespace.
                if variants.binary_search(&i).is_ok() || i > 0 && matches!(toks[i - 1].kind, T::Dot | T::KwFn)
                    || toks.get(i + 1).is_some_and(|t| t.kind == T::Colon) {
                    i += 1; continue;
                }
                let lookup_at = i;
                path.clear();
                path.push(seg_of(&toks[i]));
                i += 1;
                while i + 1 < toks.len() && toks[i].kind == T::PathSep && matches!(toks[i + 1].kind, T::Ident | T::RawIdent | T::KwSuper | T::KwSelfValue | T::KwCrate) {
                    path.push(seg_of(&toks[i + 1])); i += 2;
                }
                let invocation = toks.get(i).is_some_and(|token| token.kind == T::Not);
                // How much of the path can name anything: a segment the table never saw resolves
                // nothing, and every shorter prefix is tried below, so the rest is irrelevant.
                let first = path[0];
                let block_local = !local.is_empty() && local.contains_key(first.text);
                let first_ok = first.id.is_some() || matches!(first.text, "crate" | "self" | "super") || block_local
                    || prim_index(first.text).is_some() || v.dep_root(self, ci, first.text).is_some();
                let known = if first_ok {
                    // `self::super::..` and `super::super::..` consume their `super` run without a lookup
                    let mut k = 1;
                    if matches!(first.text, "self" | "super") { while path.get(k).is_some_and(|p| p.text == "super") { k += 1; } }
                    k + path[k..].iter().take_while(|p| p.id.is_some()).count()
                } else { 0 };
                if invocation && known == path.len() && toks.get(i + 1).is_some_and(|t| matches!(t.kind, T::OpenBrace | T::OpenParen | T::OpenBracket))
                    && self.empty_macro_core(v, cache, ci, m, &path, false, scratch) {
                    i = close(toks, i + 1) + 1;
                    continue;
                }
                if known == 0 { continue; }
                let shadowed = !invocation && local_types.iter().any(|(lo, hi, name)| *lo < lookup_at && lookup_at < *hi && *name == path[0].text);
                let mut node = 0u32;
                for p in &path[..known] {
                    let id = match p.id { Some(id) => id, None => v.seg(b, cache, p.text) };
                    let next = trie.len() as u32 + 1;
                    node = *trie.entry((node, id)).or_insert(next);
                }
                let shared_key = (node as u64) << 1 | invocation as u64;
                let key = (node as u64) << 2 | (invocation as u64) | (shadowed as u64) << 1;
                if seen.insert(key, *call) == Some(*call) { continue; }
                if !block_local {
                    if let Some(&(generation, a, len)) = shared_resolved.get(&shared_key) {
                        if generation == *scope_gen {
                            *hits += 1;
                            out.extend_from_slice(&arena[a as usize..(a + len) as usize]);
                            continue;
                        }
                    }
                }
                *misses += 1;
                let mut stored = false;
                // A type's associated item still demands the defining type.
                let mut k = known;
                while k > 0 {
                    resolved.clear();
                    if invocation { self.resolve_macro_core(v, cache, ci, m, &path[..k], &mut resolved, scratch); }
                    else { self.resolve_core(v, cache, ci, m, &path[..k], &mut resolved, scratch); }
                    sort_dedup(&mut resolved);
                    if !invocation { resolved.retain(|&name| !self.flag(v, name, FLAG_MACRO)); }
                    if shadowed {
                        // A named block import shadows outer type bindings only
                        // inside its lexical block. Other namespaces survive.
                        resolved.retain(|&name| !self.binding_flag(v, name, FLAG_SHADOWED, scratch));
                    }
                    if let Some(targets) = local.get(path[0].text) {
                        if k == 1 { resolved.extend_from_slice(targets); }
                        else {
                            for &target in targets {
                                let mut segs = std::mem::take(&mut scratch.segs);
                                segs.clear();
                                v.segs_of(b, target, &mut segs);
                                segs.extend(path[1..k].iter().map(|p| p.id.unwrap()));
                                self.walk(&segs, scratch);
                                scratch.segs = segs;
                                if !scratch.t.is_empty() {
                                    resolved.extend_from_slice(&scratch.t);
                                    resolved.extend_from_slice(&scratch.v);
                                    resolved.push(target);
                                }
                            }
                        }
                        sort_dedup(&mut resolved);
                    }
                    // A module cannot be a bare value/type expression. A path
                    // such as module::Type still demands its complete binding.
                    if k == 1 && !invocation {
                        resolved.retain(|&name| !self.binding_flag(v, name, FLAG_ALL_MODULES, scratch));
                    }
                    if !resolved.is_empty() {
                        if !block_local {
                            let a = arena.len() as u32;
                            arena.extend_from_slice(&resolved);
                            shared_resolved.insert(shared_key, (*scope_gen, a, resolved.len() as u32));
                            stored = true;
                        }
                        out.extend_from_slice(&resolved);
                        break;
                    }
                    k -= 1;
                }
                if !block_local && !stored { shared_resolved.insert(shared_key, (*scope_gen, 0, 0)); }
            } else { i += 1; }
        }
        for index in 0..17 {
            if implicit >> index & 1 != 0 { out.push(v.prim(index)); }
        }
        dedup_tail(out, start);
    }
}

fn dedup_tail(out: &mut Vec<Sym>, start: usize) {
    out[start..].sort_unstable();
    let mut w = start;
    for r in start..out.len() {
        if w == start || out[r] != out[w - 1] { out[w] = out[r]; w += 1; }
    }
    out.truncate(w);
}

// A local macro whose every arm emits only functions with literal names has
// a complete item outline without evaluating its bodies. Tokens substituted
// into a function body cannot add items to the containing module.
/// Resolutions of one crate/module's paths, shared by the ranges a caller
/// evaluates in it (`Names::references_shared`), and a worker's scratch buffers.
#[derive(Default)]
pub struct Lookups {
    scope: Option<(usize, String)>,
    scope_gen: u32,
    ctx: Option<ModuleCtx>,
    /// (path prefix, invocation) -> the scope generation it was resolved in, its range of `arena`
    resolved: HashMap<u64, (u32, u32, u32)>,
    arena: Vec<Sym>,
    /// path prefixes as nodes: (parent node, segment) -> node
    trie: HashMap<(u32, Sym), u32>,
    /// the call that last resolved a (path prefix, flags)
    seen: HashMap<u64, u32>,
    call: u32,
    cache: SymCache,
    scratch: Scratch,
    pub hits: usize,
    pub misses: usize,
    pub glob_queries: usize,
    pub glob_bindings: usize,
}
impl Lookups {
    /// The symbol caches of this worker, for naming things outside `references_shared`.
    pub fn cache(&mut self) -> &mut SymCache { &mut self.cache }
}

pub fn fixed_function_macros(src: &[u8], toks: &[Tok]) -> BTreeMap<String, Vec<String>> {
    let mut out = BTreeMap::new();
    let mut i = 0;
    while i + 3 < toks.len() {
        if !is_word(src, &toks[i], "macro_rules") { i += 1; continue; }
        let end = close(toks, i + 3);
        let mut names = Vec::new();
        let mut valid = true;
        let mut arm = i + 4;
        while arm < end {
            if matches!(toks[arm].kind, T::Semi | T::Comma) { arm += 1; continue; }
            let matcher = close(toks, arm);
            if !toks.get(matcher + 1).is_some_and(|t| t.kind == T::FatArrow) { valid = false; break; }
            let body = matcher + 2;
            let body_end = close(toks, body);
            let mut at = body + 1;
            while at < body_end {
                if toks[at].kind == T::KwPub { at += 1; }
                if !toks.get(at).is_some_and(|t| t.kind == T::KwFn)
                    || !toks.get(at + 1).is_some_and(|t| matches!(t.kind, T::Ident | T::RawIdent)) { valid = false; break; }
                names.push(word(src, &toks[at + 1]));
                at += 2;
                while at < body_end && toks[at].kind != T::OpenBrace {
                    if matches!(toks[at].kind, T::OpenParen | T::OpenBracket) { at = close(toks, at) + 1; }
                    else { at += 1; }
                }
                if at >= body_end { valid = false; break; }
                at = close(toks, at) + 1;
            }
            if !valid { break; }
            arm = body_end + 1;
        }
        if valid && !names.is_empty() { out.insert(word(src, &toks[i + 2]), names); }
        i = end + 1;
    }
    out
}

// A rule with no output, or only invocations of other empty rules, cannot
// contribute a declaration or a reference. This follows configured definitions
// and their resolved bindings; it does not recognize application macro names.
fn empty_rule_calls(src: &[u8], toks: &[Tok]) -> Option<Vec<Vec<String>>> {
    let mut calls = Vec::new();
    let mut arm = 0;
    while arm < toks.len() {
        if matches!(toks[arm].kind, T::Semi | T::Comma) { arm += 1; continue; }
        if !matches!(toks[arm].kind, T::OpenBrace | T::OpenParen | T::OpenBracket) { return None; }
        let matcher = close(toks, arm);
        if !toks.get(matcher + 1).is_some_and(|t| t.kind == T::FatArrow) { return None; }
        let body = matcher + 2;
        if !toks.get(body).is_some_and(|t| matches!(t.kind, T::OpenBrace | T::OpenParen | T::OpenBracket)) { return None; }
        let end = close(toks, body);
        let mut at = body + 1;
        while at < end {
            if toks[at].kind == T::Semi { at += 1; continue; }
            let mut path = Vec::new();
            // Only $crate is definition-site anchored. An ordinary helper
            // name could resolve differently at the invocation site.
            if !is_word(src, &toks[at], "$") || !toks.get(at + 1).is_some_and(|t| t.kind == T::KwCrate) { return None; }
            at += 1;
            if !matches!(toks[at].kind, T::Ident | T::RawIdent | T::KwCrate | T::KwSelfValue | T::KwSuper) { return None; }
            path.push(word(src, &toks[at])); at += 1;
            while at + 1 < end && toks[at].kind == T::PathSep {
                if !matches!(toks[at + 1].kind, T::Ident | T::RawIdent | T::KwCrate | T::KwSelfValue | T::KwSuper) { return None; }
                path.push(word(src, &toks[at + 1])); at += 2;
            }
            if !toks.get(at).is_some_and(|t| t.kind == T::Not)
                || !toks.get(at + 1).is_some_and(|t| matches!(t.kind, T::OpenBrace | T::OpenParen | T::OpenBracket)) { return None; }
            at = close(toks, at + 1) + 1;
            if at > end { return None; }
            calls.push(path);
        }
        arm = end + 1;
    }
    Some(calls)
}

use crate::crate_image::{Codec, Decode};
use crate::crate_image::strings::{self, Pack};

// Both strings and target/provenance sets are local to this image. Reexport
// fanout repeats sets, not their contents, and never emits process binding IDs.
#[derive(Clone, Default)]
struct BindingImage {
    strings: Vec<String>,
    sets: Vec<Vec<usize>>,
    entries: Vec<(usize, bool, usize, usize)>,
}
/// The spellings are in sorted order and a crate's bindings share long prefixes (`a::b::c`,
/// `a::b::d`): each is stored as the length it shares with the one before and its remaining bytes,
/// inline (they are most of a namespace's bytes and nothing else in the payload repeats them).
/// Sets are sorted ascending local ids, entries ascend by spelling (local id): both are stored as
/// varint differences, and an entry's `public` flag rides in the low bit of its targets set id.
impl Pack for BindingImage {
    fn pack(&self, w: &mut Vec<u8>, _: &mut strings::Strings) {
        strings::put_var(w, self.strings.len() as u64);
        let mut previous = "";
        for text in &self.strings {
            let shared = previous.bytes().zip(text.bytes()).take_while(|(a, b)| a == b).count();
            strings::put_var(w, shared as u64);
            strings::put_var(w, (text.len() - shared) as u64);
            w.extend_from_slice(&text.as_bytes()[shared..]);
            previous = text;
        }
        strings::put_var(w, self.sets.len() as u64);
        for set in &self.sets {
            strings::put_var(w, set.len() as u64);
            let mut last = 0usize;
            for &id in set { strings::put_var(w, zigzag(id as i64 - last as i64)); last = id; }
        }
        strings::put_var(w, self.entries.len() as u64);
        let mut last = 0usize;
        for &(name, public, targets, via) in &self.entries {
            strings::put_var(w, zigzag(name as i64 - last as i64)); last = name;
            strings::put_var(w, ((targets as u64) << 1) | u64::from(public));
            strings::put_var(w, via as u64);
        }
    }
    fn unpack(r: &mut Decode<'_>, s: &[String]) -> Option<Self> {
        let n = usize::unpack(r, s)?;
        if n > r.bytes.len() { return None; }
        let mut strings: Vec<String> = Vec::with_capacity(n);
        for _ in 0..n {
            let shared = usize::unpack(r, s)?;
            let rest = usize::unpack(r, s)?;
            let mut bytes = Vec::with_capacity(shared + rest);
            bytes.extend_from_slice(strings.last().map_or(&[][..], |p| p.as_bytes()).get(..shared)?);
            bytes.extend_from_slice(r.take(rest)?);
            strings.push(String::from_utf8(bytes).ok()?);
        }
        let n = usize::unpack(r, s)?;
        if n > r.bytes.len() { return None; }
        let mut sets = Vec::with_capacity(n);
        for _ in 0..n {
            let len = usize::unpack(r, s)?;
            if len > r.bytes.len() { return None; }
            let mut set = Vec::with_capacity(len);
            let mut last = 0i64;
            for _ in 0..len {
                last += unzigzag(strings::get_var(r)?);
                set.push(usize::try_from(last).ok()?);
            }
            sets.push(set);
        }
        let n = usize::unpack(r, s)?;
        if n > r.bytes.len() { return None; }
        let mut entries = Vec::with_capacity(n);
        let mut last = 0i64;
        for _ in 0..n {
            last += unzigzag(strings::get_var(r)?);
            let tagged = strings::get_var(r)?;
            let via = usize::unpack(r, s)?;
            entries.push((usize::try_from(last).ok()?, tagged & 1 == 1, usize::try_from(tagged >> 1).ok()?, via));
        }
        Some(Self { strings, sets, entries })
    }
    fn plain_size(&self) -> usize { self.strings.plain_size() + self.sets.plain_size() + self.entries.plain_size() }
}
fn zigzag(value: i64) -> u64 { ((value << 1) ^ (value >> 63)) as u64 }
fn unzigzag(value: u64) -> i64 { (value >> 1) as i64 ^ -((value & 1) as i64) }
impl BindingImage {
    fn capture(names: &Names, mut entries: Vec<usize>) -> Self {
        let mut referenced = entries.iter().copied().collect::<HashSet<_>>();
        for &id in &entries {
            let value = &names.bindings.values[id];
            referenced.extend(value.targets.iter().chain(value.via.iter()));
        }
        // Spellings rendered for this capture only (not cached in the bindings), in spelling order.
        let mut rendered = referenced.into_iter().map(|id| (names.bindings.spelling_string(id), id)).collect::<Vec<_>>();
        rendered.sort_unstable();
        let strings = rendered.iter().map(|&(_, id)| id).collect::<Vec<_>>();
        let ids = strings.iter().enumerate().map(|(local, &id)| (id, local)).collect::<HashMap<_, _>>();
        // entries in spelling order = in local order (a binding is its own spelling's only owner)
        entries.sort_by_key(|id| ids[id]);
        // each distinct set once, as the key of its id (no second copy in a list)
        let mut set_ids = HashMap::<Vec<usize>, usize>::default();
        let mut intern = |set: &bindings::Names| {
            let mut local = set.iter().map(|id| ids[&id]).collect::<Vec<_>>();
            local.sort_unstable();
            if let Some(&id) = set_ids.get(&local) { return id; }
            let id = set_ids.len();
            set_ids.insert(local, id);
            id
        };
        let entries = entries.into_iter().map(|id| {
            let value = &names.bindings.values[id];
            (ids[&id], value.public, intern(&value.targets), intern(&value.via))
        }).collect();
        let mut sets = vec![Vec::new(); set_ids.len()];
        for (set, id) in set_ids { sets[id] = set; }
        Self { strings: rendered.into_iter().map(|(text, _)| text).collect(), sets, entries }
    }
    fn valid(&self) -> bool {
        self.sets.iter().all(|set| set.iter().all(|&id| id < self.strings.len()) && set.windows(2).all(|w| w[0] < w[1]))
            && self.entries.iter().all(|&(name, _, targets, via)| self.strings.get(name).is_some_and(|s| s.contains("::"))
                && targets < self.sets.len() && via < self.sets.len())
    }
    fn legacy_size(&self, delta: bool) -> usize {
        if delta {
            4 + self.entries.iter().map(|&(name, _, targets, via)| {
                self.strings[name].plain_size() + 1 + [targets, via].into_iter().map(|id|
                    4 + self.sets[id].iter().map(|&id| self.strings[id].plain_size()).sum::<usize>()).sum::<usize>()
            }).sum::<usize>()
        } else {
            self.strings.plain_size() + 4 + self.entries.iter().map(|&(_, _, targets, via)|
                5 + self.sets[targets].plain_size() + self.sets[via].plain_size()).sum::<usize>()
        }
    }
}
impl Names {
    /// References see their crate, its transitive extern/reexport graph and the
    /// implicit standard preludes. Unrelated consumers cannot change a lookup.
    pub fn reference_crates(&self, ci: usize) -> Vec<usize> {
        // crates by name (one pass): looking each visited crate's roots up by scanning every
        // crate made a call O(crates^2), and the demand record calls it for every crate
        let mut by_name = HashMap::<&str, Vec<usize>>::default();
        for (i, c) in self.crates.iter().enumerate() { by_name.entry(c.name.as_str()).or_default().push(i); }
        let named = |name: &str| by_name.get(name).map_or(&[][..], |v| &v[..]);
        let mut todo = vec![ci];
        for std in ["core", "alloc", "std"] { todo.extend_from_slice(named(std)); }
        let mut seen = BTreeSet::new();
        while let Some(ci) = todo.pop() {
            if !seen.insert(ci) { continue; }
            let c = &self.crates[ci];
            todo.extend(c.image_deps.iter().copied());
            todo.extend_from_slice(named(&c.name));
            for (_, root) in c.deps.iter().chain(&c.image_dep_names) { todo.extend_from_slice(named(root)); }
        }
        seen.into_iter().collect()
    }

    pub fn recorded_indexes(&mut self, files: &[FileIndex]) {
        self.reset_view();
        for file in files {
            self.symbols.extend(file.names.iter().cloned());
            self.public.extend(file.glob_names.iter().map(|name| format!("{}::{name}", file.module)));
        }
    }
    /// The crate's namespace and its input identity. The identity names exactly what
    /// `prepare_crate(ci)` consumes from this crate before any import is evaluated:
    /// the declaration/use/extern/macro tables for paths within it (the snapshot's
    /// tables, canonically ordered, without bindings) and the crate's dependency
    /// spellings that `absolute` reads. Everything else it reads is the bindings of
    /// earlier crates, which a part pins by its dependency identities.
    pub fn snapshot(&self, ci: usize) -> (Vec<u8>, u128) {
        let mut image = self.tables(ci);
        let krate = &self.crates[ci].name;
        let mine = |s: &str| within(s, krate);
        let input = self.input_identity(ci, &image);
        image.bindings = BindingImage::capture(self, self.bindings.modules.iter().filter(|(id, _)| mine(self.bindings.spelling(**id)))
            .flat_map(|(_, entries)| entries.values().copied()).collect());
        (strings::encode(&image), input)
    }
    /// See `snapshot`: `tables` is the crate's namespace image with empty bindings.
    fn input_identity(&self, ci: usize, tables: &NamespaceDelta) -> u128 {
        let c = &self.crates[ci];
        let mut hash = crate::sip::Sip128::new();
        hash.str("demand-namespace-input-v1");
        hash.u128(crate::crate_image::file_hash("", &strings::encode(tables)));
        hash.str(&c.name);
        hash.u8(c.cfg.session.options.rs_lazy as u8);
        for (alias, root) in &c.deps { hash.str(alias); hash.str(root); }
        hash.u8(0xff);
        for (alias, root) in &c.image_dep_names { hash.str(alias); hash.str(root); }
        hash.finish128()
    }
    /// The input identity alone, for a crate about to be prepared.
    pub fn input_id(&self, ci: usize) -> u128 { self.input_identity(ci, &self.tables(ci)) }
    /// The crate's tables as the snapshot records them, without bindings.
    fn tables(&self, ci: usize) -> NamespaceDelta {
        let krate = &self.crates[ci].name;
        let mine = |s: &str| within(s, krate);
        NamespaceDelta {
            symbols: self.symbols.iter().filter(|s| mine(s)).cloned().collect(),
            types: self.type_symbols.iter().filter(|s| mine(s)).cloned().collect::<BTreeSet<_>>().into_iter().collect(),
            traits: self.trait_symbols.iter().filter(|s| mine(s)).cloned().collect::<BTreeSet<_>>().into_iter().collect(),
            values: self.value_symbols.iter().filter(|s| mine(s)).cloned().collect::<BTreeSet<_>>().into_iter().collect(),
            public: self.public.iter().filter(|s| mine(s)).cloned().collect::<BTreeSet<_>>().into_iter().collect(),
            modules: self.modules.iter().filter(|s| mine(s)).cloned().collect::<BTreeSet<_>>().into_iter().collect(),
            module_macros: self.module_macros.iter().filter(|s| mine(s)).cloned().collect::<BTreeSet<_>>().into_iter().collect(),
            macros: self.macros.iter().filter(|s| mine(s)).cloned().collect::<BTreeSet<_>>().into_iter().collect(),
            uses: self.uses.iter().filter(|(s, _)| mine(s)).map(|(s, v)| (s.clone(), v.clone())).collect(),
            macro_uses: self.macro_uses.iter().filter(|v| mine(&v.0)).cloned().collect(),
            externs: self.externs.iter().filter(|v| mine(&v.0)).cloned().collect(),
            empty_rules: self.empty_rules.iter().filter(|(s, _)| mine(s)).map(|(s, v)| (s.clone(), v.clone())).collect(),
            bindings: BindingImage::default(),
        }
    }
    /// `snapshot(ci)` of each of `cis` (crates whose names no other crate of the graph has),
    /// from one pass over each table: a snapshot per crate filtered every table, which made
    /// the demand record of a 257-crate graph quadratic (Stage: 11.6 s). An entry belongs to
    /// the crate its first path segment names (`within`; crate names have no `::`).
    pub fn snapshots(&self, cis: &[usize]) -> Vec<(Vec<u8>, u128)> {
        let slot: HashMap<&str, usize> = cis.iter().enumerate().map(|(k, &ci)| (self.crates[ci].name.as_str(), k)).collect();
        let owner = |s: &str| slot.get(s.split("::").next().unwrap_or(s)).copied();
        let mut images: Vec<NamespaceDelta> = (0..cis.len()).map(|_| NamespaceDelta::default()).collect();
        // symbols, uses and empty_rules iterate sorted (BTree); bucketing keeps that order
        for name in &self.symbols { if let Some(k) = owner(name) { images[k].symbols.push(name.clone()); } }
        for (name, uses) in &self.uses { if let Some(k) = owner(name) { images[k].uses.insert(name.clone(), uses.clone()); } }
        for (name, rule) in &self.empty_rules { if let Some(k) = owner(name) { images[k].empty_rules.insert(name.clone(), rule.clone()); } }
        // vectors keep their order
        for entry in &self.macro_uses { if let Some(k) = owner(&entry.0) { images[k].macro_uses.push(entry.clone()); } }
        for entry in &self.externs { if let Some(k) = owner(&entry.0) { images[k].externs.push(entry.clone()); } }
        // hash sets: sorted and deduplicated, as snapshot's BTreeSet collect
        let sets: [(&HashSet<String>, fn(&mut NamespaceDelta) -> &mut Vec<String>); 7] = [
            (&self.type_symbols, |d| &mut d.types), (&self.trait_symbols, |d| &mut d.traits), (&self.value_symbols, |d| &mut d.values),
            (&self.public, |d| &mut d.public), (&self.modules, |d| &mut d.modules), (&self.module_macros, |d| &mut d.module_macros),
            (&self.macros, |d| &mut d.macros)];
        for (set, field) in sets {
            for name in set { if let Some(k) = owner(name) { field(&mut images[k]).push(name.clone()); } }
            for image in &mut images { let v = field(image); v.sort_unstable(); v.dedup(); }
        }
        let mut entries: Vec<Vec<usize>> = vec![Vec::new(); cis.len()];
        for (&module, names) in &self.bindings.modules {
            if let Some(k) = owner(self.bindings.spelling(module)) { entries[k].extend(names.values().copied()); }
        }
        // each crate's bindings and encoding are independent: spread over the loader's workers
        let work: Vec<relax_session::sync::Mutex<Option<(NamespaceDelta, Vec<usize>)>>> = images.into_iter().zip(entries).map(|w| relax_session::sync::Mutex::new(Some(w))).collect();
        let next = std::sync::atomic::AtomicUsize::new(0);
        let out: Vec<relax_session::sync::Mutex<(Vec<u8>, u128)>> = (0..work.len()).map(|_| Default::default()).collect();
        crate::pool::scope(|scope| {
            for _ in 0..crate::preload::jobs().clamp(1, work.len().max(1)) {
                scope.spawn(|| { let _mem = crate::memstat::tag(crate::memstat::Tag::Planner); loop {
                    let k = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let Some(cell) = work.get(k) else { break };
                    let (mut image, entries) = cell.lock().unwrap().take().unwrap();
                    let input = self.input_identity(cis[k], &image);
                    image.bindings = BindingImage::capture(self, entries);
                    *out[k].lock().unwrap() = (strings::encode(&image), input);
                } });
            }
        });
        out.into_iter().map(|o| o.into_inner().unwrap()).collect()
    }
    /// Restore a crate's recorded namespace. `replace`: the tables may already hold entries
    /// for this crate (it was added this run); otherwise only a same-named crate can.
    pub fn restore(&mut self, ci: usize, bytes: &[u8], replace: bool) -> Option<()> {
        self.restore_decoded(ci, decode_namespace(bytes)?, replace)
    }
    pub fn restore_decoded(&mut self, ci: usize, image: NamespaceDelta, replace: bool) -> Option<()> {
        self.reset_view();
        let mut local = self.fork();
        local.symbols = image.symbols.into_iter().collect();
        local.type_symbols = image.types.into_iter().collect();
        local.trait_symbols = image.traits.into_iter().collect();
        local.value_symbols = image.values.into_iter().collect();
        local.public = image.public.into_iter().collect();
        local.modules = image.modules.into_iter().collect();
        local.module_macros = image.module_macros.into_iter().collect();
        local.macros = image.macros.into_iter().collect();
        local.uses = image.uses;
        local.macro_uses = image.macro_uses;
        local.externs = image.externs;
        local.empty_rules = image.empty_rules;
        let bindings = image.bindings;
        // A package library and binary share this namespace. The snapshot is
        // its complete projection at this point, so replace its raw facts rather
        // than appending the library's imports a second time.
        let krate = &self.crates[ci].name;
        // Entries for this crate's paths come from adding or restoring it, or a crate of
        // its name (a package's library and binary). None of those: nothing to retain.
        if replace || self.crates.iter().enumerate().any(|(other, c)| other != ci && c.name == *krate) {
            self.symbols.retain(|name| !within(name, krate));
            self.type_symbols.retain(|name| !within(name, krate));
            self.trait_symbols.retain(|name| !within(name, krate));
            self.value_symbols.retain(|name| !within(name, krate));
            self.public.retain(|name| !within(name, krate));
            self.modules.retain(|name| !within(name, krate));
            self.module_macros.retain(|name| !within(name, krate));
            self.macros.retain(|name| !within(name, krate));
            self.uses.retain(|name, _| !within(name, krate));
            self.macro_uses.retain(|entry| !within(&entry.0, krate));
            self.externs.retain(|entry| !within(&entry.0, krate));
            self.empty_rules.retain(|name, _| !within(name, krate));
        }
        self.merge(local, true);
        self.prepare_crate_with(ci, Some(bindings));
        Some(())
    }
}

fn decode_namespace(bytes: &[u8]) -> Option<NamespaceDelta> {
    let image: NamespaceDelta = strings::decode(bytes)?;
    image.bindings.valid().then_some(image)
}

/// Recorded namespaces decoded ahead on background threads, in crate order, while the
/// planner discovers; `take` decodes a crate itself when no worker has reached it.
pub(super) struct Predecoded {
    parts: Vec<Option<Arc<DemandPart>>>,
    slots: relax_session::sync::Mutex<Vec<Slot>>,
    done: relax_session::sync::Condvar,
}
enum Slot { Free, Busy, Done(Option<NamespaceDelta>), Taken }
impl Predecoded {
    pub(super) fn start(parts: Vec<Option<Arc<DemandPart>>>, workers: usize) -> Arc<Self> {
        let slots = parts.iter().map(|p| if p.as_ref().is_some_and(|p| !p.namespace.is_empty()) { Slot::Free } else { Slot::Taken }).collect();
        let this = Arc::new(Self { parts, slots: relax_session::sync::Mutex::new(slots), done: relax_session::sync::Condvar::new() });
        for _ in 0..workers.min(this.parts.len()) {
            let this = this.clone();
            std::thread::Builder::new().name("run-namespace-decode".into()).spawn(move || { let _z = crate::zone_alloc::compiler_thread();
                while let Some(ci) = this.claim(None) { this.decode(ci); }
            }).expect("namespace decode worker");
        }
        this
    }
    // The next free slot (or `only`, if it is free); None when there is nothing to claim.
    fn claim(&self, only: Option<usize>) -> Option<usize> {
        let mut slots = self.slots.lock().unwrap();
        let ci = match only {
            Some(ci) => matches!(slots[ci], Slot::Free).then_some(ci),
            None => slots.iter().position(|s| matches!(s, Slot::Free)),
        }?;
        slots[ci] = Slot::Busy;
        Some(ci)
    }
    fn decode(&self, ci: usize) {
        let image = decode_namespace(&self.parts[ci].as_ref().unwrap().namespace);
        self.slots.lock().unwrap()[ci] = Slot::Done(image);
        self.done.notify_all();
    }
    /// The decoded recorded namespace of crate `ci`, once.
    pub(super) fn take(&self, ci: usize) -> Option<NamespaceDelta> {
        if self.claim(Some(ci)).is_some() { self.decode(ci); }
        let mut slots = self.slots.lock().unwrap();
        loop {
            match std::mem::replace(&mut slots[ci], Slot::Taken) {
                Slot::Done(image) => return image,
                Slot::Taken => return None,
                busy => { slots[ci] = busy; slots = self.done.wait(slots).unwrap(); }
            }
        }
    }
}

// Only deferred executable outlines change the namespace after discovery. The
// writer records that delta, so an edit restores it without a second full
// namespace decode/import fixed point. All identities on disk are spellings.
#[derive(Clone, Default)]
pub(crate) struct NamespaceDelta {
    symbols: Vec<String>, types: Vec<String>, traits: Vec<String>, values: Vec<String>,
    public: Vec<String>, modules: Vec<String>, module_macros: Vec<String>, macros: Vec<String>,
    uses: BTreeMap<String, Vec<crate::macros::UseItem>>,
    macro_uses: Vec<(String, String, bool)>, externs: Vec<(String, String, String, bool)>,
    empty_rules: BTreeMap<String, (usize, String, Vec<Vec<String>>)>,
    bindings: BindingImage,
}
strings::fields!(NamespaceDelta, symbols, types, traits, values, public, modules, module_macros,
    macros, uses, macro_uses, externs, empty_rules, bindings);
impl NamespaceDelta {
    pub(super) fn legacy_size(&self, delta: bool) -> usize {
        self.plain_size() - self.bindings.plain_size() + self.bindings.legacy_size(delta)
    }
}
/// A part's closure delta, always as the bytes it is stored as: the recorder encodes it once, a
/// stored part reads those bytes without decoding them (a decoded delta is one owned spelling per
/// name, 0.4 GB on a cold Stage), and only a load that applies the delta decodes it. The legacy
/// size is MAKEPAD_RUN_PROFILE's report, taken from the delta at the record (0 unprofiled or read).
pub(crate) struct ClosureDelta { image: Vec<u8>, legacy: usize }
impl Codec for ClosureDelta {
    fn put(&self, w: &mut Vec<u8>) {
        self.image.len().put(w);
        w.extend_from_slice(&self.image);
    }
    fn get(r: &mut Decode<'_>) -> Option<Self> {
        let n = usize::get(r)?;
        Some(Self { image: r.take(n)?.to_vec(), legacy: 0 })
    }
}
impl ClosureDelta {
    pub(super) fn new(value: &NamespaceDelta, legacy: usize) -> Self {
        let _mem = crate::memstat::tag(crate::memstat::Tag::FrontRecord);
        let mut image = strings::encode(value);
        image.shrink_to_fit();
        Self { image, legacy }
    }
    pub(super) fn same_image(&self, other: &Self) -> bool { self.image == other.image }
    /// The delta, decoded (None when the stored bytes are not a valid delta).
    pub(super) fn decoded(&self) -> Option<NamespaceDelta> {
        let value: NamespaceDelta = strings::decode(&self.image)?;
        value.bindings.valid().then_some(value)
    }
    pub(super) fn legacy_size(&self) -> usize { self.legacy }
}
pub(super) fn namespace_legacy_size(bytes: &[u8]) -> Option<usize> {
    let image: NamespaceDelta = strings::decode(bytes)?;
    image.bindings.valid().then(|| image.legacy_size(false))
}
impl Names {
    pub fn closure_delta(&self, before: &Self) -> NamespaceDelta {
        let additions = |after: &HashSet<String>, before: &HashSet<String>| {
            let mut out = after.difference(before).cloned().collect::<Vec<_>>(); out.sort(); out
        };
        let t0 = std::time::Instant::now();
        let mut bindings = Vec::new();
        // each id's id in `before` (before.bindings.id(spelling)), without rendering spellings
        let map = self.bindings.ids_in(&before.bindings);
        let t_map = t0.elapsed();
        for entries in self.bindings.modules.values() {
            for &id in entries.values() {
                let value = &self.bindings.values[id];
                let same_set = |new: &bindings::Names, old: &bindings::Names| {
                    new.iter().len() == old.iter().len() && new.iter().all(|id|
                        map[id].is_some_and(|id| old.contains(id)))
                };
                let unchanged = map[id].is_some_and(|id| {
                    let old = &before.bindings.values[id];
                    old.public == value.public && same_set(&value.targets, &old.targets) && same_set(&value.via, &old.via)
                });
                if !unchanged { bindings.push(id); }
            }
        }
        let (t_loop, changed) = (t0.elapsed(), bindings.len());
        let bindings = BindingImage::capture(self, bindings);
        if crate::preload::trace() {
            eprintln!("rx-demand-closure-delta: {} ids mapped in {:.0} ms, bindings compared in {:.0} ms, {} changed captured in {:.0} ms",
                map.len(), t_map.as_secs_f64() * 1e3, (t_loop - t_map).as_secs_f64() * 1e3, changed, (t0.elapsed() - t_loop).as_secs_f64() * 1e3);
        }
        NamespaceDelta {
            symbols: self.symbols.difference(&before.symbols).cloned().collect(),
            types: additions(&self.type_symbols, &before.type_symbols),
            traits: additions(&self.trait_symbols, &before.trait_symbols),
            values: additions(&self.value_symbols, &before.value_symbols),
            public: additions(&self.public, &before.public), modules: additions(&self.modules, &before.modules),
            module_macros: additions(&self.module_macros, &before.module_macros), macros: additions(&self.macros, &before.macros),
            uses: self.uses.iter().filter_map(|(name, uses)| {
                let start = before.uses.get(name).map_or(0, Vec::len);
                (start < uses.len()).then(|| (name.clone(), uses[start..].to_vec()))
            }).collect(),
            macro_uses: self.macro_uses[before.macro_uses.len()..].to_vec(),
            externs: self.externs[before.externs.len()..].to_vec(),
            empty_rules: self.empty_rules.iter().filter(|(k, v)| before.empty_rules.get(*k) != Some(*v)).map(|(k, v)| (k.clone(), v.clone())).collect(),
            bindings,
        }
    }
    pub fn apply_closure_delta(&mut self, delta: &NamespaceDelta) {
        self.reset_view();
        self.symbols.extend(delta.symbols.iter().cloned()); self.type_symbols.extend(delta.types.iter().cloned());
        self.trait_symbols.extend(delta.traits.iter().cloned()); self.value_symbols.extend(delta.values.iter().cloned());
        self.public.extend(delta.public.iter().cloned()); self.modules.extend(delta.modules.iter().cloned());
        self.module_macros.extend(delta.module_macros.iter().cloned()); self.macros.extend(delta.macros.iter().cloned());
        for (name, uses) in &delta.uses { self.uses.entry(name.clone()).or_default().extend(uses.iter().cloned()); }
        self.macro_uses.extend(delta.macro_uses.iter().cloned()); self.externs.extend(delta.externs.iter().cloned());
        self.empty_rules.extend(delta.empty_rules.iter().map(|(k, v)| (k.clone(), v.clone())));
        let saved = &delta.bindings;
        let ids = saved.strings.iter().map(|name| self.bindings.intern(name)).collect::<Vec<_>>();
        for &(name, public, targets, via) in &saved.entries {
            let (module, local) = saved.strings[name].rsplit_once("::").expect("validated binding image");
            let module = self.bindings.intern(module); let local = self.bindings.intern(local);
            let id = self.bindings.entry(module, local);
            let mut value = Binding { public, ..Binding::default() };
            for &target in &saved.sets[targets] { value.targets.insert(ids[target]); }
            for &source in &saved.sets[via] { value.via.insert(ids[source]); }
            self.bindings.values[id] = value;
        }
        self.finish_prepare();
    }
}

#[cfg(test)]
mod image_tests {
    use super::*;
    #[test]
    fn binding_image_round_trips() {
        let image = BindingImage {
            strings: ["a", "a::b", "a::b::c", "core::fmt::Debug", "z", "\u{e9}::x", "\u{e8}::y", "\u{e9}::x::\u{20ac}", "a"].map(String::from).to_vec(),
            sets: vec![vec![], vec![0], vec![1, 2, 4], vec![0, 3, 4]],
            // ascending names plus an out-of-order and a repeated one: the codec must not assume order
            entries: vec![(1, true, 2, 0), (2, false, 3, 1), (3, true, 0, 3), (1, false, 1, 2), (1, true, 3, 3)],
        };
        let bytes = strings::encode(&image);
        let back: BindingImage = strings::decode(&bytes).unwrap();
        assert_eq!(back.strings, image.strings);
        assert_eq!(back.sets, image.sets);
        assert_eq!(back.entries, image.entries);
        assert!(back.valid());
        let mut truncated = bytes.clone();
        truncated.pop();
        assert!(strings::decode::<BindingImage>(&truncated).is_none());
    }
}

#[cfg(test)]
mod active_tests {
    use super::*;
    fn old_active_tokens_inner(src: &[u8], toks: &[Tok], cfg: &CfgSet, out: &mut Vec<Tok>) {
        let output_start = out.len();
        let mut i = 0;
        while i < toks.len() {
            // Macro arguments/transcribers are token trees, not configured AST yet.
            if is_word(src, &toks[i], "macro_rules") && i + 3 < toks.len() {
                let end = close(toks, i + 3);
                out.extend_from_slice(&toks[i..=end]);
                i = end + 1;
                continue;
            }
            if toks[i].kind.is_ident_like() && toks.get(i + 1).is_some_and(|t| t.kind == T::Not)
                && toks.get(i + 2).is_some_and(|t| matches!(t.kind, T::OpenBrace | T::OpenParen | T::OpenBracket)) {
                let end = close(toks, i + 2);
                out.extend_from_slice(&toks[i..=end]);
                i = end + 1;
                continue;
            }
            if toks[i].kind == T::Pound {
                let mut attrs = Vec::new();
                let start = i;
                while i < toks.len() && toks[i].kind == T::Pound {
                    let inner = toks.get(i + 1).is_some_and(|t| t.kind == T::Not);
                    // Inner attributes configure this scope; outer attributes configure
                    // the following item. A disabled first item must not erase the file
                    // (or inline module) that carries an unrelated inner attribute.
                    if attrs.first().is_some_and(|a: &Attr| a.inner != inner) { break; }
                    let open = i + 1 + usize::from(inner);
                    if !toks.get(open).is_some_and(|t| t.kind == T::OpenBracket) { break; }
                    let end = close(toks, open);
                    attrs.push(Attr { inner, toks: TokRange { lo: (open + 1) as u32, hi: end as u32 } });
                    i = end + 1;
                }
                if attrs.is_empty() { out.push(toks[i]); i += 1; continue; }
                let view = Src { src, plain: toks, ext: &[], parts: None };
                if !crate::cfg::active(view, &attrs, cfg) {
                    if attrs.iter().any(|a| a.inner) { out.truncate(output_start); return; }
                    // The attributed item/statement/field is removed as one unit.
                    while i < toks.len() {
                        match toks[i].kind {
                            T::OpenBrace => { i = close(toks, i) + 1; break; }
                            T::OpenParen | T::OpenBracket => i = close(toks, i) + 1,
                            T::Semi | T::Comma => { i += 1; break; }
                            _ => i += 1,
                        }
                    }
                    continue;
                }
                for a in crate::cfg::expand_cfg_attr_list(view, &attrs, cfg) {
                    let lo = a.toks.lo as usize;
                    let hi = a.toks.hi as usize;
                    if lo >= hi || is_word(src, &toks[lo], "cfg") { continue; }
                    out.push(toks[start]);
                    if a.inner { out.push(Tok { kind: T::Not, ..toks[start] }); }
                    out.push(Tok { kind: T::OpenBracket, ..toks[start] });
                    out.extend_from_slice(&toks[lo..hi]);
                    out.push(Tok { kind: T::CloseBracket, ..toks[hi] });
                }
                continue;
            }
            if matches!(toks[i].kind, T::OpenBrace | T::OpenParen | T::OpenBracket) {
                let end = close(toks, i);
                out.push(toks[i]);
                old_active_tokens_inner(src, &toks[i + 1..end], cfg, out);
                out.push(toks[end]);
                i = end + 1;
            } else {
                out.push(toks[i]);
                i += 1;
            }
        }
    }
    
    
    fn lexed(text: &str) -> Vec<Tok> {
        let mut toks = Vec::new();
        let _ = crate::lexer::lex(text.as_bytes(), 2021, &mut toks);
        toks
    }
    /// The group table agrees with the walk it replaces: on balanced, nested, attributed, macro and
    /// malformed token streams.
    #[test]
    fn configured_tokens_match_the_nested_walk() {
        let sess = std::sync::Arc::new(crate::Session::default());
        let cfg = CfgSet::host(&sess, &[]);
        let cases = [
            "fn f() { #[cfg(any())] let x = (1, [2]); #[cfg(all())] g(); } #[cfg(any())] fn h() {} struct S { #[cfg(any())] a: u8, b: u8 }",
            "macro_rules! m { ($a:expr) => { $a + (1) }; } m!(1); vec![1, 2]; mod a { #![cfg(any())] fn x() {} }",
            "mod a { #![cfg(all())] fn x() { if true { ( } } }",
            "fn f() { } } fn g() { }",
            "fn f( { fn g() { #[cfg(any())] x } ",
            "macro_rules foo bar { } fn z() { macro_rules ! }",
            "#[cfg(any())] #[inline] fn a() {} #[cfg_attr(all(), inline)] fn b() {} #[cfg_attr(any(), inline)] fn c() {}",
            "m!{ #[cfg(any())] 1 } #[derive(Debug)] struct T; #",
            "fn f() { #[ } ",
            "",
        ];
        for text in cases {
            let toks = lexed(text);
            let mut want = Vec::new();
            old_active_tokens_inner(text.as_bytes(), &toks, &cfg, &mut want);
            let got = active_tokens(text.as_bytes(), &toks, &cfg);
            let key = |v: &[Tok]| v.iter().map(|t| (t.kind as u32, t.lo, t.hi)).collect::<Vec<_>>();
            assert_eq!(key(&got), key(&want), "{text}");
        }
    }
}
