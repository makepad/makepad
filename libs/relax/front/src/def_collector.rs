// ported from N:rustc_resolve/src/def_collector.rs @8d1a764
use crate::ast::*;
use crate::program::{Def, DefId, DefKind, Program};

/// Relax's arenas have separate node spaces; generated async nodes have their
/// own space too. These are AST identities, never definition identities.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum NodeId {
    AnonConst(ExprId),
    InlineConst(ExprId),
    OpaqueTy(TyId),
    AsyncReturn(ItemId),
    AsyncBody(ItemId),
    Closure(ExprId),
    Field(u32),
}

static NO_NAMES: std::sync::LazyLock<crate::fxhash::HashSet<crate::program::Sym>> = std::sync::LazyLock::new(Default::default);

pub fn collect_definitions(prog: &mut Program, owner: DefId) -> Result<(), String> {
    if !prog.collected_definitions.insert(owner) { return Ok(()); }
    let file = prog.def(owner).file;
    let pf = prog.files[file as usize].clone();
    let scope = prog.def(owner).scope;
    let mut visitor = DefCollector { prog, ast: &pf.ast, file, parent_def: owner, scope, legacy: false, legacy_attrs: None, legacy_names: &NO_NAMES, call_names: Some(Vec::new()), const_ribs: false, bindings: Vec::new(), checking_defs: false, creates_defs: false, legacy_calls: Default::default(), legacy_children: Vec::new() };
    visitor.visit_item(owner)?;
    // an owner with a constant or generic-default rib is walked by the legacy pass (its
    // late.rs const-rib checks report there): no entry
    if visitor.const_ribs { return Ok(()); }
    let mut names = visitor.call_names.take().unwrap_or_default();
    names.sort_unstable();
    names.dedup();
    prog.legacy_call_names.insert(owner, names);
    Ok(())
}

/// rustc_ast_lowering/src/expr.rs:594-679: the split is published before typeck.
#[derive(Clone, Debug)]
pub struct LegacyConstArgs {
    pub callee: DefId,
    pub positions: Vec<u32>,
    pub const_args: Vec<ExprId>,
    pub value_args: Vec<ExprId>,
    pub invalid: bool,
}

/// Owner results have the same boundary as rustc_ast_lowering/src/lib.rs:704-745
/// lower_to_hir. Round-45 lead design: only their scheduling changes; the visitor
/// below still performs expr.rs:594-679 and late.rs:5224-5283 in full.
#[derive(Clone, Debug)]
pub struct LegacyOwner {
    pub file: u32,
    pub ranges: Vec<(u32, u32)>,
    pub calls: crate::fxhash::HashMap<ExprId, LegacyConstArgs>,
    pub children: Vec<(ExprId, DefId)>,
}

pub fn lower_legacy_const_generics(prog: &mut Program) -> usize {
    lower_legacy_const_generics_in(prog, None)
}

/// Lead decision 52 (R1): a declaration edit re-lowers the owners of the crates that can
/// see it (`visible`, the edited crates and their transitive dependents), as rustc
/// recompiles only those crates and reads every other crate from its metadata
/// (rustc_metadata/src/rmeta/decoder/cstore_impl.rs: upstream answers are decoded, never
/// recomputed). An owner of an invisible crate resolves names only in its own crate and
/// its dependencies, all invisible, and calls only callees of those crates, whose
/// `rustc_legacy_const_generics` attributes did not change: its attribute entry and its
/// owner result (expr.rs:594-679, late.rs:5224-5283) stay exactly what a full pass computes.
/// `None` re-lowers every owner.
pub fn lower_legacy_const_generics_in(prog: &mut Program, visible: Option<&[bool]>) -> usize {
    if std::env::var_os("MAKEPAD_RUN_INVALIDATION_TRACE").is_some() { eprintln!("invalidation-trace: legacy whole={} scoped={}", usize::from(visible.is_none()), usize::from(visible.is_some())); }
    let sees = |prog: &Program, d: DefId| visible.map_or(true, |v| v.get(prog.def(d).krate as usize).copied().unwrap_or(true));
    let mut owners: Vec<_> = Vec::new();
    for &owner in &prog.collected_definitions {
        if sees(prog, owner) { owners.push(owner); }
    }
    owners.sort_unstable_by_key(|d| d.0);
    let mut attrs = crate::fxhash::HashMap::default();
    if visible.is_some() {
        for (&owner, indices) in prog.legacy_const_attrs.iter() {
            if !sees(prog, owner) { attrs.insert(owner, indices.clone()); }
        }
    }
    for &owner in &owners {
        if let Some(indices) = legacy_const_generic_args(prog, owner) { attrs.insert(owner, indices); }
    }
    prog.legacy_const_attrs = std::sync::Arc::new(attrs);
    match visible {
        Some(visible) => {
            let defs = &prog.defs;
            prog.legacy_const_args.retain(|d, _| !visible.get(defs[d.0 as usize].krate as usize).copied().unwrap_or(true));
        }
        None => prog.legacy_const_args.clear(),
    }
    lower_legacy_const_owners(prog, &owners)
}

/// A deferred module load (Program::link_deferred_modules) only adds definitions: the
/// loaded files' owners. An existing owner's result (expr.rs:594-679, late.rs:5224-5283)
/// depends on its unchanged syntax, on what its call paths resolve to, and on the callees'
/// `rustc_legacy_const_generics` attributes. A call path the load changes now reaches a new
/// definition (before the load it resolved to nothing, the lookup having missed in the skipped
/// module), and a callee without the attribute lowers as a plain call, exactly as the
/// unresolved path did. So when no added owner carries the attribute, every existing owner's
/// result and attribute entry stay what lower_legacy_const_generics_in computes, and only the
/// added owners are lowered; otherwise the crates seeing the files are lowered again as by
/// lower_legacy_const_generics_in. (A full pass per load re-walked every body of those crates:
/// ~0.6 s per completion load in Calculator, the main thread waiting on the front lock.)
pub fn lower_legacy_const_generics_added(prog: &mut Program, first_def: usize, visible: &[bool]) -> usize {
    lower_legacy_const_generics_relinked(prog, first_def, visible, &[])
}

/// lower_legacy_const_generics_added that also lowers `relinked` (ascending): the owners of a
/// retained collected prefix that lost their on-demand const arguments with the definitions
/// after the suffix start (Program::retain_collected_prefix). They lower first, as the whole
/// pass lowers its owners in definition order, so the definitions come out with the same ids.
pub fn lower_legacy_const_generics_relinked(prog: &mut Program, first_def: usize, visible: &[bool], relinked: &[DefId]) -> usize {
    if std::env::var_os("MAKEPAD_RUN_INVALIDATION_TRACE").is_some() { eprintln!("invalidation-trace: legacy added=1"); }
    let mut owners: Vec<DefId> = Vec::new();
    for &owner in &prog.collected_definitions {
        if owner.0 as usize >= first_def { owners.push(owner); }
    }
    owners.sort_unstable_by_key(|d| d.0);
    for &owner in &owners {
        if legacy_const_generic_args(prog, owner).is_some() {
            // the relinked owners call functions of their own (retained) crates: those attributes stand
            let first = lower_legacy_const_owners(prog, relinked);
            return first + lower_legacy_const_generics_in(prog, Some(visible));
        }
    }
    let mut all = relinked.to_vec();
    all.extend(owners);
    lower_legacy_const_owners(prog, &all)
}

/// The caller has classified the entire transaction as body-only. The cached
/// callable attributes and resolution environment therefore still apply.
pub fn lower_legacy_const_owners(prog: &mut Program, owners: &[DefId]) -> usize {
    let attrs = prog.legacy_const_attrs.clone();
    let names = legacy_callee_names(prog, &attrs);
    let mut results = Vec::new();
    for &owner in owners {
        let def = prog.def(owner);
        let (file, scope, item) = (def.file, def.scope, def.item);
        // an owner none of whose calls names a legacy callee (legacy_call_names, recorded by its
        // definition collection over the same syntax): every lower_legacy_call returns before
        // resolving, the walk splits nothing and creates nothing -- its result is empty
        if prog.legacy_call_names.get(&owner).is_some_and(|c| !c.iter().any(|s| names.contains(s))) {
            results.push((owner, std::sync::Arc::new(LegacyOwner { file, ranges: Vec::new(), calls: Default::default(), children: Vec::new() })));
            continue;
        }
        let pf = prog.files[file as usize].clone();
        let mut visitor = DefCollector { prog, ast: &pf.ast, file, parent_def: owner, scope,
            legacy: true, legacy_attrs: Some(&attrs), legacy_names: &names, call_names: None, const_ribs: false, bindings: Vec::new(), checking_defs: false, creates_defs: false,
            legacy_calls: Default::default(), legacy_children: Vec::new() };
        if let Err(error) = visitor.visit_item(owner) { visitor.prog.errors.push(error); }
        let calls = visitor.legacy_calls;
        let children = visitor.legacy_children;
        let ranges = if calls.is_empty() { Vec::new() } else { prog.item_expr_ranges(file, item) };
        let result = LegacyOwner { file, ranges, calls, children };
        results.push((owner, std::sync::Arc::new(result)));
    }
    // No preemption point exists in declaration collection. The front writer
    // publishes completed owner results together before releasing its lock.
    prog.legacy_const_args.extend(results);
    owners.len()
}

/// The names a call path's last segment can have when it resolves to a function with
/// `rustc_legacy_const_generics` (lower_legacy_call's target): the functions' own names and
/// every module binding of one (`use ..::f as g;`, also in block modules). Value lookups
/// (lookup_in_scope / lookup_in_module / lookup_in_container) find a function only under
/// one of these, so a call whose last segment names none of them resolves to no legacy
/// callee and lowers as a plain call (expr.rs:594-679): lower_legacy_call skips its
/// resolution, the walk's cost in every body.
fn legacy_callee_names(prog: &Program, attrs: &crate::fxhash::HashMap<DefId, Vec<u32>>) -> crate::fxhash::HashSet<crate::program::Sym> {
    let mut names = crate::fxhash::HashSet::default();
    if attrs.is_empty() { return names; }
    for &d in attrs.keys() { names.insert(prog.def(d).name); }
    for m in &prog.mods {
        for (&name, d) in &m.values { if attrs.contains_key(d) { names.insert(name); } }
    }
    names
}

/// Rebase an unchanged owner's expression coordinates, not its resolution answers.
/// Program::body_ranges records the parser's contiguous owner arenas (declaration
/// and demanded body); concatenating them also handles eager versus lazy parsing.
/// The classifier has already established unchanged syntax for these owners.
pub fn retarget_legacy_owner(prog: &mut Program, owner: DefId) {
    let Some(previous) = prog.legacy_const_args.get(&owner).cloned() else { return; };
    let def = prog.def(owner);
    if previous.file == def.file { return; }
    let mut next = (*previous).clone();
    next.file = def.file;
    if !next.calls.is_empty() {
        next.ranges = prog.item_expr_ranges(def.file, def.item);
        let mut old_len = 0;
        let mut new_len = 0;
        for &(lo, hi) in &previous.ranges { old_len += hi - lo; }
        for &(lo, hi) in &next.ranges { new_len += hi - lo; }
        assert_eq!(old_len, new_len, "unchanged owner's expression arenas differ");
        let remap = |e: ExprId| {
            let mut ordinal = 0;
            let mut found = false;
            for &(lo, hi) in &previous.ranges {
                if e.0 >= lo && e.0 < hi { ordinal += e.0 - lo; found = true; break; }
                ordinal += hi - lo;
            }
            assert!(found, "legacy expression outside its owner arenas");
            for &(lo, hi) in &next.ranges {
                if ordinal < hi - lo { return ExprId(lo + ordinal); }
                ordinal -= hi - lo;
            }
            unreachable!("legacy expression outside the new owner arenas")
        };
        next.calls.clear();
        for (&call, split) in &previous.calls {
            let mut split = split.clone();
            for e in &mut split.const_args { *e = remap(*e); }
            for e in &mut split.value_args { *e = remap(*e); }
            next.calls.insert(remap(call), split);
        }
        for (e, _) in &mut next.children { *e = remap(*e); }
    }
    restore_legacy_children(prog, &next);
    prog.legacy_const_args.insert(owner, std::sync::Arc::new(next));
}

pub(crate) fn restore_legacy_children(prog: &mut Program, result: &LegacyOwner) {
    for &(e, child) in &result.children {
        let key = (result.file, NodeId::AnonConst(e));
        prog.node_id_to_def_id.insert(key, child);
        prog.def_id_to_node_id.insert(child, key);
        // Restoring a generated child also reserves its parent-owned ordinal
        // (rustc_resolve/src/def_collector.rs:58-84) for later environment edits.
        let def = *prog.def(child);
        let next = prog.def_disambiguators.entry((def.parent, DefKind::AnonConst, None)).or_default();
        *next = (*next).max(def.sub + 1);
    }
}

/// rustc_attr_parsing/src/attributes/rustc_internal.rs:134-214.
fn legacy_const_generic_args(prog: &mut Program, owner: DefId) -> Option<Vec<u32>> {
    let def = prog.def(owner);
    let (file, item, kind) = (def.file, def.item, def.kind);
    let pf = prog.files[file as usize].clone();
    let mut result = None;
    for attr in pf.ast.list(pf.ast.item(item).attrs) {
        if attr.toks.lo == attr.toks.hi || pf.tok_text(attr.toks.lo) != "rustc_legacy_const_generics" { continue; }
        let mut indices = Vec::new();
        let mut error = None;
        if result.is_some() { error = Some("multiple rustc_legacy_const_generics attributes"); }
        let ItemKind::Fn(sig, _, _) = pf.ast.item(item).kind else {
            let error = prog.record_error(file, pf.ast.item(item).lo, "legacy_const_generics", "rustc_legacy_const_generics is only allowed on functions".into());
            if !prog.errors.contains(&error) { prog.errors.push(error); }
            continue;
        };
        let sig = pf.ast.fn_sig(sig);
        if kind != DefKind::Fn { error = Some("rustc_legacy_const_generics is only allowed on functions"); }
        let mut token = attr.toks.lo + 1;
        if token >= attr.toks.hi || pf.tok_text(token) != "(" { error = Some("expected a list of integer argument indices"); }
        token += 1;
        while token < attr.toks.hi && pf.tok_text(token) != ")" {
            let literal = pf.tok_text(token).replace('_', "");
            let (radix, digits) = if let Some(s) = literal.strip_prefix("0x") { (16, s) }
                else if let Some(s) = literal.strip_prefix("0o") { (8, s) }
                else if let Some(s) = literal.strip_prefix("0b") { (2, s) } else { (10, literal.as_str()) };
            match u128::from_str_radix(digits, radix) {
                Ok(index) => indices.push(index as usize),
                Err(_) => { error = Some("expected an unsuffixed integer literal argument index"); break; }
            }
            token += 1;
            if token < attr.toks.hi && pf.tok_text(token) == "," { token += 1; }
            else if token < attr.toks.hi && pf.tok_text(token) != ")" { error = Some("expected a comma between argument indices"); break; }
        }
        if indices.is_empty() { error = Some("expected at least one argument index"); }
        let params = pf.ast.list(sig.generics.params);
        if params.iter().any(|p| !matches!(p.kind, GenericParamKind::Const(..))) {
            error = Some("rustc_legacy_const_generics functions must only have const generic parameters");
        } else if indices.len() != params.len() { error = Some("argument index count must match the number of const generic parameters"); }
        let count = pf.ast.list(sig.params).len() + params.len();
        if indices.iter().any(|&i| i >= count) { error = Some("legacy const generic argument index exceeds the argument count"); }
        if let Some(message) = error {
            let error = prog.record_error(file, pf.ast.item(item).lo, "legacy_const_generics", message.into());
            if !prog.errors.contains(&error) { prog.errors.push(error); }
        } else { result = Some(indices.into_iter().map(|i| i as u32).collect()); }
    }
    result
}

/// `create_def`, `with_parent`, and the anon/opaque visitors follow nightly
/// :58-84, :423-426, :452-468. Disambiguators belong to the parent and kind.
// rustc_resolve/src/late.rs value ribs: a block's items are considered before
// bindings outside that block, but after bindings introduced inside it.
enum ValueRib {
    Binding(Ident), Module(u32),
    Generic(Ident, bool),
    SelfParam,
    Constant { reason: ConstRestriction, repeat: bool },
    Forward(Vec<Ident>),
}

#[derive(Clone, Copy)]
enum ConstRestriction { NonTrivial, Discriminant, ParamTy }

struct DefCollector<'a> {
    prog: &'a mut Program,
    ast: &'a Ast,
    file: u32,
    parent_def: DefId,
    scope: u32,
    legacy: bool,
    legacy_attrs: Option<&'a crate::fxhash::HashMap<DefId, Vec<u32>>>,
    /// legacy_callee_names of legacy_attrs (empty outside the legacy pass)
    legacy_names: &'a crate::fxhash::HashSet<crate::program::Sym>,
    /// definition collection: the last-segment names of the owner's call paths that
    /// lower_legacy_call would resolve (Program::legacy_call_names)
    call_names: Option<Vec<crate::program::Sym>>,
    /// definition collection: the owner has a rib the legacy pass's check_generic_path reports
    /// in (ValueRib::Constant or ValueRib::Forward)
    const_ribs: bool,
    bindings: Vec<ValueRib>,
    checking_defs: bool,
    creates_defs: bool,
    legacy_calls: crate::fxhash::HashMap<ExprId, LegacyConstArgs>,
    legacy_children: Vec<(ExprId, DefId)>,
}

impl DefCollector<'_> {
    fn create_def(&mut self, node: NodeId, name: Option<crate::program::Sym>, kind: DefKind) -> DefId {
        if self.checking_defs { self.creates_defs = true; return self.parent_def; }
        let key = (self.file, node);
        if let Some(&def) = self.prog.node_id_to_def_id.get(&key) { return def; }
        let next = self.prog.def_disambiguators.entry((self.parent_def, kind, name)).or_default();
        let disambiguator = *next;
        *next += 1;
        let label = match kind {
            DefKind::AnonConst => "constant", DefKind::OpaqueTy => "opaque",
            DefKind::Closure => "closure", DefKind::Field => "field", _ => unreachable!(),
        };
        let symbol = name.unwrap_or_else(|| self.prog.syms.intern(&format!("{{{}#{}}}", label, disambiguator)));
        let parent = self.prog.def(self.parent_def);
        let def = Def { kind, name: symbol, parent: self.parent_def, file: self.file, item: parent.item,
            sub: disambiguator, is_pub: false, krate: parent.krate, scope: self.scope };
        let path = (self.parent_def, kind, name, disambiguator);
        let id = if let Some(&id) = self.prog.child_definitions.get(&path) {
            self.prog.defs[id.0 as usize] = def;
            id
        } else {
            let id = self.prog.add_def(def);
            self.prog.child_definitions.insert(path, id);
            id
        };
        self.prog.node_id_to_def_id.insert(key, id);
        self.prog.def_id_to_node_id.insert(id, key);
        if kind == DefKind::OpaqueTy {
            // rustc_ty_utils/src/opaque_types.rs:310-318 (RpitAndAsyncFnOnly; TAITs are
            // feature-gated): an RPIT/async-fn opaque is defined by its item, and
            // typeck children (closures, nested opaques, non-type-system inline
            // consts) answer with their typeck root's list.
            let mut owner = self.parent_def;
            loop {
                let typeck_child = match self.prog.def(owner).kind {
                    DefKind::OpaqueTy | DefKind::Closure => true,
                    DefKind::AnonConst => matches!(self.prog.def_id_to_node_id.get(&owner), Some((_, NodeId::InlineConst(_)))),
                    _ => false,
                };
                if !typeck_child { break; }
                owner = self.prog.def(owner).parent;
            }
            let definitions = self.prog.opaque_types_defined_by.entry(owner).or_default();
            if !definitions.contains(&id) { definitions.push(id); }
        }
        id
    }

    fn with_parent(&mut self, parent: DefId) -> DefId {
        std::mem::replace(&mut self.parent_def, parent)
    }

    // rustc_resolve/src/late.rs:5224-5283 and ident.rs:1609-1780.
    // Generic and constant ribs share the lexical stack with legacy-call resolution.
    fn push_item_generics(&mut self, owner: DefId) {
        let item = self.prog.def(owner).item;
        let kind = self.ast.item(item).kind;
        let g = match kind {
            ItemKind::Trait(r) => { self.bindings.push(ValueRib::SelfParam); self.ast.row(r).generics }
            k => match self.ast.kind_generics(&k) { Some(g) => *g, None => return },
        };
        for p in self.ast.list(g.params) {
            match p.kind {
                GenericParamKind::Type(..) => self.bindings.push(ValueRib::Generic(p.name, true)),
                GenericParamKind::Const(..) => self.bindings.push(ValueRib::Generic(p.name, false)),
                _ => {}
            }
        }
    }

    fn check_generic_path(&mut self, p: Path, type_ns: bool) {
        if !self.legacy || self.checking_defs || p.global() || !p.no_qself() { return; }
        let Some(first) = self.ast.segs(p.segs).first() else { return; };
        let type_ns = type_ns || p.segs.len() > 1;
        let pf = &self.prog.files[self.file as usize];
        let name = pf.text(first.name);
        let mut restriction = None;
        let mut forward = false;
        let mut found = false;
        for rib in self.bindings.iter().rev() {
            match rib {
                ValueRib::Constant { reason, repeat } => {
                    if !(*repeat && type_ns) { restriction = Some(*reason); }
                }
                ValueRib::Forward(names) => {
                    if names.iter().any(|n| pf.text(*n) == name) { forward = true; }
                }
                ValueRib::Generic(binding, ty) if *ty == type_ns && pf.text(*binding) == name && pf.hyg_visible(binding.lo, first.name.lo) => { found = true; break; }
                ValueRib::SelfParam if type_ns && name == "Self" => { found = true; break; }
                ValueRib::Binding(binding) if !type_ns && pf.text(*binding) == name && pf.hyg_visible(binding.lo, first.name.lo) => return,
                ValueRib::Module(scope) => {
                    if let Some(sym) = self.prog.syms.get(name) {
                        if self.prog.lookup_in_module(*scope, sym, type_ns, &mut Vec::new()).is_some() { return; }
                    }
                }
                _ => {}
            }
        }
        if !found { return; }
        let message = if forward {
            Some("E0128: generic parameter defaults cannot reference parameters before they are declared".to_string())
        } else { restriction.map(|r| match r {
            ConstRestriction::NonTrivial => format!("generic parameters may not be used in const operations: `{name}`"),
            ConstRestriction::Discriminant => format!("generic parameters may not be used in enum discriminant values: `{name}`"),
            ConstRestriction::ParamTy => format!("E0770: the type of const parameters must not depend on other generic parameters: `{name}`"),
        }) };
        if let Some(message) = message {
            let error = self.prog.record_error(self.file, p.lo, "resolve", message);
            if !self.prog.errors.contains(&error) { self.prog.errors.push(error); }
        }
    }

    fn visit_item(&mut self, owner: DefId) -> Result<(), String> {
        if self.legacy {
            let parent = self.prog.def(owner).parent;
            if parent != crate::program::NO_DEF && matches!(self.prog.def(parent).kind, DefKind::Trait | DefKind::Impl) {
                self.push_item_generics(parent);
            }
            self.push_item_generics(owner);
        }
        let item = self.prog.def(owner).item;
        match self.ast.item(item).kind {
            ItemKind::Fn(sig, _, _) => {
                let sig = *self.ast.fn_sig(sig);
                self.visit_generics(sig.generics);
                if let Some(SelfParam::Typed(_, ty)) = sig.self_param { self.visit_ty(ty, false); }
                for p in self.ast.list(sig.params) { self.visit_pat(p.pat); self.visit_ty(p.ty, false); self.bind_pat(p.pat); }
                if sig.asyncness && self.prog.def(owner).kind != DefKind::ForeignFn {
                    // Nightly :270: the written output is inside the return opaque;
                    // the body is inside the desugared coroutine closure.
                    let ret = self.create_def(NodeId::AsyncReturn(item), None, DefKind::OpaqueTy);
                    let saved = self.with_parent(ret);
                    if let Some(t) = sig.ret { self.visit_ty(t, true); }
                    self.parent_def = saved;
                    if let Some(b) = self.prog.fn_body(self.file, item)? {
                        let closure = self.create_def(NodeId::AsyncBody(item), None, DefKind::Closure);
                        let saved = self.with_parent(closure);
                        self.visit_block(b);
                        self.parent_def = saved;
                    }
                } else {
                    if let Some(t) = sig.ret { self.visit_ty(t, true); }
                    if let Some(b) = self.prog.fn_body(self.file, item)? { self.visit_block(b); }
                }
            }
            ItemKind::Struct(r) => { let StructItem { generics: g, data } = *self.ast.row(r); self.visit_generics(g); self.visit_variant_data(data); }
            ItemKind::Union(r) => { let UnionItem { generics: g, fields } = *self.ast.row(r); self.visit_generics(g); self.visit_fields(fields); }
            ItemKind::Enum(r) => {
                let EnumItem { generics: g, variants } = *self.ast.row(r);
                self.visit_generics(g);
                for (i, v) in self.ast.list(variants).iter().enumerate() {
                    let vd = self.prog.variants[self.prog.def(owner).sub as usize][i];
                    let saved = self.with_parent(vd);
                    self.visit_variant_data(v.data);
                    if let Some(e) = v.disc { self.visit_const(e, Some(ConstRestriction::Discriminant), false); }
                    self.parent_def = saved;
                }
            }
            ItemKind::TypeAlias(r) => {
                let TypeAliasItem { generics, bounds, ty } = *self.ast.row(r);
                self.visit_generics(generics); self.visit_bounds(bounds, true);
                if let Some(t) = ty { self.visit_ty(t, true); }
            }
            ItemKind::Const(ty, _) => {
                if let Some(t) = ty.get() { self.visit_ty(t, false); }
                if let Some(e) = self.prog.const_body(self.file, item)? { self.visit_expr(e); }
            }
            ItemKind::Static(_, ty, _) => {
                self.visit_ty(ty, false);
                if let Some(e) = self.prog.const_body(self.file, item)? { self.visit_expr(e); }
            }
            ItemKind::Trait(r) => {
                let TraitItem { generics, supers, .. } = *self.ast.row(r);
                self.visit_generics(generics); self.visit_bounds(supers, false);
            }
            ItemKind::TraitAlias(r) => {
                let TraitAliasItem { generics, bounds: supers } = *self.ast.row(r);
                self.visit_generics(generics); self.visit_bounds(supers, false);
            }
            ItemKind::Impl(r) => {
                let ImplItem { generics, trait_, self_ty, .. } = *self.ast.row(r);
                self.visit_generics(generics);
                if let Some(p) = trait_ { self.visit_path(p, false); }
                self.visit_ty(self_ty, false);
            }
            _ => {}
        }
        Ok(())
    }

    fn visit_variant_data(&mut self, data: VariantData) {
        match data { VariantData::Tuple(f) | VariantData::Struct(f) => self.visit_fields(f), VariantData::Unit => {} }
    }
    fn visit_fields(&mut self, fields: List<FieldDef>) {
        for (i, f) in self.ast.list(fields).iter().enumerate() {
            let name = match f.name {
                Some(name) => self.prog.ident(self.file, name),
                None => self.prog.syms.intern(&i.to_string()),
            };
            let def = self.create_def(NodeId::Field(fields.lo + i as u32), Some(name), DefKind::Field);
            let saved = self.with_parent(def);
            self.visit_ty(f.ty, false);
            if let Some(e) = f.default { self.visit_anon_const(e); }
            self.parent_def = saved;
        }
    }
    fn visit_generics(&mut self, g: Generics) {
        self.visit_generic_params(g.params);
        for p in self.ast.list(g.where_) {
            if let WherePred::Bound { hrtb, ty, bounds } = *p {
                self.visit_generic_params(hrtb); self.visit_ty(ty, false); self.visit_bounds(bounds, false);
            }
        }
    }
    fn visit_generic_params(&mut self, params: List<GenericParam>) {
        for (index, p) in self.ast.list(params).iter().enumerate() {
            match p.kind {
                GenericParamKind::Type(bounds, ty) => {
                    self.visit_bounds(bounds, false);
                    if let Some(t) = ty {
                        let saved = self.bindings.len();
                        self.const_ribs = true;
                        if self.legacy { self.bindings.push(ValueRib::Forward(self.ast.list(params)[index..].iter().map(|p| p.name).collect())); }
                        self.visit_ty(t, false);
                        self.bindings.truncate(saved);
                    }
                }
                GenericParamKind::Const(ty, val) => {
                    self.const_ribs = true;
                    let saved = self.bindings.len();
                    if self.legacy { self.bindings.push(ValueRib::Constant { reason: ConstRestriction::ParamTy, repeat: false }); }
                    self.visit_ty(ty, false);
                    self.bindings.truncate(saved);
                    if let Some(e) = val {
                        if self.legacy { self.bindings.push(ValueRib::Forward(self.ast.list(params)[index..].iter().map(|p| p.name).collect())); }
                        self.visit_anon_const(e);
                        self.bindings.truncate(saved);
                    }
                }
                GenericParamKind::Lifetime(_) => {}
            }
        }
    }
    fn visit_bounds(&mut self, bounds: List<Bound>, opaque: bool) {
        for b in self.ast.list(bounds) {
            match *b {
                Bound::Trait { hrtb, path, .. } => { self.visit_generic_params(hrtb); self.visit_path(path, opaque); }
                Bound::Use(args) => self.visit_args(args, opaque), Bound::Lifetime(_) => {}
            }
        }
    }
    fn visit_path(&mut self, p: Path, opaque: bool) {
        if let Some(q) = self.ast.qself(&p).copied() {
            self.visit_ty(q.ty, opaque);
            if let Some(path) = q.trait_path { self.visit_path(path, opaque); }
        }
        for s in self.ast.segs(p.segs) { self.visit_args(s.args(), opaque); }
    }
    fn visit_args(&mut self, args: GenericArgsId, opaque: bool) {
        match self.ast.gargs(args) {
            Some(GenericArgs::Angle(args)) => for arg in self.ast.list(*args) {
                match *arg {
                    GenericArg::Type(t) => {
                        // A bare generic argument is syntactically a type until
                        // resolution chooses the const-parameter namespace.
                        if let Ty::Path(p) = *self.ast.ty(t) {
                            if p.segs.len() == 1 { self.check_generic_path(p, false); }
                        }
                        self.visit_ty(t, opaque);
                    }
                    GenericArg::Const(e) => self.visit_anon_const(e),
                    GenericArg::Binding(_, args, t) => { self.visit_args(args, opaque); self.visit_ty(t, opaque); }
                    GenericArg::Constraint(_, bs) => self.visit_bounds(bs, opaque),
                    GenericArg::Lifetime(_) => {}
                }
            },
            Some(GenericArgs::Paren(ps, r)) => {
                for t in self.ast.list(*ps) { self.visit_ty(*t, opaque); }
                if let Some(t) = r { self.visit_ty(*t, opaque); }
            }
            None => {}
        }
    }
    fn visit_ty(&mut self, ty: TyId, opaque: bool) {
        match *self.ast.ty(ty) {
            Ty::Array(t, n) => { self.visit_ty(t, opaque); self.visit_anon_const(n); }
            Ty::ImplTrait(bs) => {
                if opaque {
                    let def = self.create_def(NodeId::OpaqueTy(ty), None, DefKind::OpaqueTy);
                    let saved = self.with_parent(def); self.visit_bounds(bs, true); self.parent_def = saved;
                } else { self.visit_bounds(bs, false); }
            }
            Ty::Path(p) => { self.check_generic_path(p, true); self.visit_path(p, opaque); },
            Ty::Ref(_, _, t) | Ty::Ptr(_, t) | Ty::Slice(t) | Ty::Paren(t) => self.visit_ty(t, opaque),
            Ty::Tuple(ts) => for t in self.ast.list(ts) { self.visit_ty(*t, opaque); },
            Ty::Fn(f) => {
                let f = *self.ast.fn_ptr(f); self.visit_generic_params(f.hrtb);
                for t in self.ast.list(f.params) { self.visit_ty(*t, opaque); }
                if let Some(t) = f.ret { self.visit_ty(t, opaque); }
            }
            Ty::DynTrait(bs, _) => self.visit_bounds(bs, opaque),
            Ty::Mac(m) => { let m = *self.ast.row(m); self.visit_mac(m) }
            Ty::Never | Ty::Infer | Ty::ImplicitSelf | Ty::CVarArgs => {}
        }
    }
    fn is_potential_trivial_const_arg(&self, e: ExprId) -> bool {
        // rustc_ast/src/ast.rs:1420-1442: exactly one brace layer.
        let e = match self.ast.expr(e).kind {
            ExprKind::Block(b, l) if l.is_none() => match self.ast.stmts(b) { [Stmt::Expr(e, false)] => *e, _ => e },
            _ => e,
        };
        match self.ast.expr(e).kind {
            ExprKind::Path(p) => p.no_qself() && !p.global() && p.segs.len() == 1 && self.ast.gargs(self.ast.segs(p.segs)[0].args()).is_none(),
            _ => false,
        }
    }
    fn visit_anon_const(&mut self, e: ExprId) {
        let reason = if self.is_potential_trivial_const_arg(e) { None } else { Some(ConstRestriction::NonTrivial) };
        self.visit_const(e, reason, false);
    }
    fn visit_const(&mut self, e: ExprId, reason: Option<ConstRestriction>, repeat: bool) {
        let def = self.create_def(NodeId::AnonConst(e), None, DefKind::AnonConst);
        if repeat && !self.checking_defs { self.prog.repeat_expr_counts.insert(def); }
        let ribs = self.bindings.len();
        if reason.is_some() { self.const_ribs = true; }
        if self.legacy { if let Some(reason) = reason { self.bindings.push(ValueRib::Constant { reason, repeat }); } }
        let saved = self.with_parent(def); self.visit_expr(e); self.parent_def = saved;
        self.bindings.truncate(ribs);
    }
    fn visit_block(&mut self, b: BlockId) {
        let bindings = self.bindings.len();
        // rustc_resolve/src/late.rs:885: anonymous consts resolve in their
        // lexical block, including its local items, independently of ownership.
        let saved_scope = self.scope;
        if let Some(&scope) = self.prog.block_mods.get(&(self.file, b.0)) {
            self.scope = scope;
            if self.legacy && !self.checking_defs { self.bindings.push(ValueRib::Module(scope)); }
        }
        for s in self.ast.list(self.ast.block(b).stmts) {
            match *s {
                Stmt::Let(r) => {
                    let LetStmt { pat, ty, init, else_, .. } = *self.ast.row(r);
                    self.visit_pat(pat); if let Some(t) = ty { self.visit_ty(t, false); }
                    if let Some(e) = init { self.visit_expr(e); } if let Some(b) = else_ { self.visit_block(b); }
                    self.bind_pat(pat);
                }
                Stmt::Expr(e, _) => self.visit_expr(e),
                Stmt::Attrs(r) => self.visit_expr(self.ast.row(r).expr),
                Stmt::Item(i) => {
                    if self.checking_defs { self.creates_defs = true; continue; }
                    if !self.legacy {
                        if let Some(&d) = self.prog.item_defs.get(&(self.file, i.0)) {
                            self.prog.defs[d.0 as usize].parent = self.parent_def;
                        }
                    }
                }
                Stmt::Empty => {}
            }
        }
        self.scope = saved_scope;
        self.bindings.truncate(bindings);
    }
    fn visit_pat(&mut self, p: PatId) {
        match *self.ast.pat(p) {
            Pat::Ident { sub: Some(p), .. } | Pat::Ref(_, p) | Pat::Paren(p) | Pat::Box(p) => self.visit_pat(p),
            Pat::Lit(e) => self.visit_expr(e),
            Pat::Range(a, b, _) => { if let Some(e) = a { self.visit_expr(e); } if let Some(e) = b { self.visit_expr(e); } }
            Pat::Path(p) => self.visit_path(p, false),
            Pat::TupleStruct(p, ps) => { self.visit_path(p, false); for p in self.ast.list(ps) { self.visit_pat(*p); } }
            Pat::Struct(p, fs, _) => { self.visit_path(p, false); for f in self.ast.list(fs) { self.visit_pat(f.pat); } }
            Pat::Tuple(ps) | Pat::Slice(ps) | Pat::Or(ps) => for p in self.ast.list(ps) { self.visit_pat(*p); },
            Pat::Mac(m) => { let m = *self.ast.row(m); self.visit_mac(m) }
            Pat::Wild | Pat::Rest | Pat::Ident { sub: None, .. } => {}
        }
    }
    fn visit_expr(&mut self, e: ExprId) {
        if self.checking_defs && self.creates_defs { return; }
        match self.ast.expr(e).kind {
            ExprKind::Repeat(v, n) => {
                self.visit_expr(v);
                let reason = if self.is_potential_trivial_const_arg(n) { None } else { Some(ConstRestriction::NonTrivial) };
                self.visit_const(n, reason, true);
            }
            ExprKind::ConstBlock(b) => {
                let def = self.create_def(NodeId::InlineConst(e), None, DefKind::AnonConst);
                let saved = self.with_parent(def); self.visit_block(b); self.parent_def = saved;
            }
            ExprKind::Closure(row) => {
                let ClosureRow { params, ret, body, .. } = *self.ast.row(row);
                let def = self.create_def(NodeId::Closure(e), None, DefKind::Closure);
                let saved = self.with_parent(def);
                let bindings = self.bindings.len();
                for p in self.ast.list(params) { self.visit_pat(p.pat); if let Some(t) = p.ty { self.visit_ty(t, false); } self.bind_pat(p.pat); }
                if let Some(t) = ret { self.visit_ty(t, false); } self.visit_expr(body); self.parent_def = saved;
                self.bindings.truncate(bindings);
            }
            ExprKind::Async(_, b) => {
                let def = self.create_def(NodeId::Closure(e), None, DefKind::Closure);
                let saved = self.with_parent(def);
                self.visit_block(b); self.parent_def = saved;
            }
            ExprKind::Path(p) => { self.check_generic_path(p, false); self.visit_path(p, false); },
            ExprKind::Unary(_, x) | ExprKind::Field(x, _) | ExprKind::TupleField(x, _) |
            ExprKind::AddrOf(_, _, x) | ExprKind::Try(x) | ExprKind::Await(x) | ExprKind::Paren(x) | ExprKind::Box(x) => self.visit_expr(x),
            ExprKind::Binary(_, a, b) | ExprKind::Assign(a, b) | ExprKind::AssignOp(_, a, b) | ExprKind::Index(a, b) => { self.visit_expr(a); self.visit_expr(b); }
            ExprKind::Call(f, args) => {
                if self.legacy && !self.checking_defs { self.lower_legacy_call(e, f, args); }
                if !self.checking_defs && self.call_names.is_some() { self.note_call_name(f); }
                self.visit_expr(f); for e in self.ast.list(args) { self.visit_expr(*e); }
            }
            ExprKind::MethodCall(recv, row) => {
                let MethodCallRow { turbofish, args, .. } = *self.ast.row(row);
                self.visit_expr(recv); self.visit_args(turbofish, false); for e in self.ast.list(args) { self.visit_expr(*e); }
            }
            ExprKind::Cast(e, t) => { self.visit_expr(e); self.visit_ty(t, false); }
            ExprKind::Block(b, _) | ExprKind::Unsafe(b) | ExprKind::Loop(b, _) => self.visit_block(b),
            // rustc_resolve/src/late.rs:4080-4097,4275,5365-5403: initializer
            // before bindings; condition bindings live in the then/body rib only.
            ExprKind::If(e, b, other) => {
                let bindings = self.bindings.len();
                self.visit_expr(e); self.visit_block(b); self.bindings.truncate(bindings);
                if let Some(e) = other.get() { self.visit_expr(e); }
            }
            ExprKind::Let(p, e) if !self.legacy => { self.visit_pat(p); self.visit_expr(e); }
            ExprKind::Let(p, e) => { self.visit_expr(e); self.visit_pat(p); self.bind_pat(p); }
            ExprKind::Match(e, arms) => {
                self.visit_expr(e);
                for a in self.ast.list(arms) {
                    let bindings = self.bindings.len();
                    self.visit_pat(a.pat); self.bind_pat(a.pat);
                    if let Some(e) = a.guard { self.visit_expr(e); } self.visit_expr(a.body);
                    self.bindings.truncate(bindings);
                }
            }
            ExprKind::While(row) => {
                let WhileRow { cond: e, body: b, .. } = *self.ast.row(row);
                let bindings = self.bindings.len(); self.visit_expr(e); self.visit_block(b); self.bindings.truncate(bindings);
            }
            ExprKind::For(row) if !self.legacy => { let ForRow { pat: p, iter: e, body: b, .. } = *self.ast.row(row); self.visit_pat(p); self.visit_expr(e); self.visit_block(b); }
            ExprKind::For(row) => {
                let ForRow { pat: p, iter: e, body: b, .. } = *self.ast.row(row);
                self.visit_expr(e); let bindings = self.bindings.len();
                self.visit_pat(p); self.bind_pat(p); self.visit_block(b); self.bindings.truncate(bindings);
            }
            ExprKind::Break(_, v) | ExprKind::Return(v) | ExprKind::Yield(v) => if let Some(e) = v.get() { self.visit_expr(e); },
            ExprKind::Tuple(es) | ExprKind::Array(es) => for e in self.ast.list(es) { self.visit_expr(*e); },
            ExprKind::Struct(row) => {
                let StructRow { path: p, fields: fs, base, .. } = *self.ast.row(row);
                self.visit_path(p, false); for f in self.ast.list(fs) { self.visit_expr(f.expr); }
                if let Some(e) = base.get() { self.visit_expr(e); }
            }
            ExprKind::Range(a, b, _) => { if let Some(e) = a.get() { self.visit_expr(e); } if let Some(e) = b.get() { self.visit_expr(e); } }
            ExprKind::Mac(m) => { let m = *self.ast.row(m); self.visit_mac(m) }
            ExprKind::Lit(_) | ExprKind::Continue(_) | ExprKind::Underscore => {}
        }
    }
    fn bind_pat(&mut self, p: PatId) {
        if !self.legacy || self.checking_defs { return; }
        match *self.ast.pat(p) {
            Pat::Ident { name, sub, .. } => {
                self.bindings.push(ValueRib::Binding(name));
                if let Some(p) = sub { self.bind_pat(p); }
            }
            Pat::Ref(_, p) | Pat::Paren(p) | Pat::Box(p) => self.bind_pat(p),
            Pat::Tuple(ps) | Pat::Slice(ps) | Pat::Or(ps) | Pat::TupleStruct(_, ps) => {
                for &p in self.ast.list(ps) { self.bind_pat(p); }
            }
            Pat::Struct(_, fs, _) => for f in self.ast.list(fs) { self.bind_pat(f.pat); },
            _ => {}
        }
    }

    /// The name lower_legacy_call looks up for a call of `callee`, if it gets that far.
    fn note_call_name(&mut self, callee: ExprId) {
        let ExprKind::Path(path) = self.ast.expr(callee).kind else { return; };
        if !path.no_qself() { return; }
        let Some(last) = self.ast.segs(path.segs).last() else { return; };
        if self.ast.gargs(last.args()).is_some() { return; }
        let text = self.prog.files[self.file as usize].text(last.name);
        let text = text.strip_prefix("r#").unwrap_or(text).to_string();
        let name = self.prog.syms.intern(&text);
        if let Some(names) = self.call_names.as_mut() { names.push(name); }
    }

    fn lower_legacy_call(&mut self, call: ExprId, callee: ExprId, args: List<ExprId>) {
        if self.legacy_calls.contains_key(&call) { return; }
        let ExprKind::Path(path) = self.ast.expr(callee).kind else { return; };
        if !path.no_qself() { return; }
        let segments = self.ast.segs(path.segs);
        let Some(last) = segments.last() else { return; };
        if self.ast.gargs(last.args()).is_some() { return; }
        {
            let pf = &self.prog.files[self.file as usize];
            let text = pf.text(last.name);
            let text = text.strip_prefix("r#").unwrap_or(text);
            if !self.prog.syms.get(text).is_some_and(|s| self.legacy_names.contains(&s)) { return; }
        }
        let segs = crate::program::path_segs(self.prog, self.file, &path);
        // rustc_resolve/src/lib.rs:2627-2655 consumes a fully resolved value path.
        let target = if segs.len() == 1 && !path.global() {
            let pf = &self.prog.files[self.file as usize];
            let mut item = None;
            for rib in self.bindings.iter().rev() {
                match rib {
                    ValueRib::Binding(binding) => {
                        if pf.text(*binding) == pf.text(last.name) && pf.hyg_visible(binding.lo, last.name.lo) { return; }
                    }
                    ValueRib::Generic(binding, false) if pf.text(*binding) == pf.text(last.name) => return,
                    ValueRib::Generic(..) | ValueRib::SelfParam | ValueRib::Constant { .. } | ValueRib::Forward(_) => {},
                    ValueRib::Module(scope) => {
                        if let Some(d) = self.prog.lookup_in_module(*scope, segs[0].1, false, &mut Vec::new()) {
                            item = Some(d); break;
                        }
                    }
                }
            }
            item.or_else(|| self.prog.lookup_in_scope(self.scope, segs[0].1, false))
        } else {
            let Some(parent) = self.prog.resolve_mod_path(self.scope, path.global(), &segs[..segs.len()-1], false, false) else { return; };
            self.prog.lookup_in_container(parent, segs[segs.len()-1].1, false)
        };
        let Some(target) = target else { return; };
        let def = self.prog.def(target);
        if !matches!(def.kind, DefKind::Fn | DefKind::AssocFn) || def.krate == self.prog.def(self.parent_def).krate { return; }
        let Some(positions) = self.legacy_attrs.and_then(|attrs| attrs.get(&target)).cloned() else { return; };
        let mut split = LegacyConstArgs { callee: target, positions, const_args: Vec::new(), value_args: Vec::new(), invalid: false };
        for (index, &arg) in self.ast.list(args).iter().enumerate() {
            if split.positions.contains(&(index as u32)) {
                let child = self.create_def(NodeId::AnonConst(arg), None, DefKind::AnonConst);
                self.legacy_children.push((arg, child));
                // WillCreateDefIdsVisitor (expr.rs:90-109) uses the same AST walk;
                // the checking mode records definition-producing nodes without installing them.
                self.checking_defs = true; self.creates_defs = false;
                self.visit_expr(arg);
                self.checking_defs = false;
                if self.creates_defs && !split.invalid {
                    let error = self.prog.record_error(self.file, self.ast.expr(arg).lo, "legacy_const_generics",
                        "invalid legacy const generic argument; use a const generic argument in the path instead".into());
                    if !self.prog.errors.contains(&error) { self.prog.errors.push(error); }
                    split.invalid = true;
                }
                self.visit_anon_const(arg);
                split.const_args.push(arg);
            } else { split.value_args.push(arg); }
        }
        self.legacy_calls.insert(call, split);
    }

    fn visit_mac(&mut self, m: MacCall) {
        if m.args == NO_MAC_ARGS { return; }
        let args = *self.ast.mac_arg(m.args);
        for e in self.ast.list(args.exprs) { self.visit_expr(*e); }
        if let Some(p) = args.pat { self.visit_pat(p); } if let Some(e) = args.guard { self.visit_expr(e); }
        if let Some(asm) = args.asm {
            for op in self.ast.list(asm.ops) {
                if let Some(e) = op.input { if op.dir == AsmDir::Const { self.visit_anon_const(e); } else { self.visit_expr(e); } }
                if let Some(e) = op.output { self.visit_expr(e); }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn definitions_have_parent_owned_identities() {
        let dir = std::env::temp_dir().join(format!("relax-def-collector-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("lib.rs");
        std::fs::write(&path, br#"#![no_std]
            trait Tr { type Item; }
            fn f<const N: usize>() -> impl Tr<Item = impl Tr> {
                let _: [u8; N + 1];
                let _ = [0; 3];
                let _ = const { 1 + 2 };
                let _ = || { let _: [u8; 5]; };
                let _ = async { let _: [u8; 6]; };
                loop {}
            }
            async fn g() -> impl Tr { loop {} }
            struct S { a: [u8; 2], b: [u8; 4] }
            enum E { A = 1, B = 2 }
            fn h() { let _: [u8; const { 2 }]; f::<{ 1 + 2 }>(); }
        "#).unwrap();
        let mut prog = Program::new(&std::sync::Arc::new(crate::Session::default()));
        let cfg = crate::cfg::CfgSet::host(&prog.session, &[]);
        prog.load_crate("defs", path.to_str().unwrap(), 2021, cfg, vec![]).unwrap();
        let mut opaque = Vec::new();
        let mut anon = Vec::new();
        for (i, d) in prog.defs.iter().enumerate() {
            if d.kind == DefKind::OpaqueTy { opaque.push(DefId(i as u32)); }
            if d.kind == DefKind::AnonConst { anon.push(DefId(i as u32)); }
        }
        assert_eq!(opaque.len(), 4); // f + nested, g's future + output
        assert_eq!(anon.len(), 12);
        assert_eq!(opaque.iter().filter(|d| prog.def(prog.def(**d).parent).kind == DefKind::OpaqueTy).count(), 2);
        assert_eq!(anon.iter().filter(|d| prog.def(prog.def(**d).parent).kind == DefKind::Closure).count(), 2);
        assert_eq!(anon.iter().filter(|d| prog.def(prog.def(**d).parent).kind == DefKind::Field).count(), 2);
        assert_eq!(anon.iter().filter(|d| prog.def(prog.def(**d).parent).kind == DefKind::Variant).count(), 2);
        let mut paths = crate::fxhash::HashSet::default();
        for d in opaque.iter().chain(&anon) { assert!(paths.insert(prog.def_path(*d))); }
        let mut nested_inline = false;
        for (&(_, node), &inline) in &prog.node_id_to_def_id {
            if let NodeId::InlineConst(_) = node {
                let outer = prog.def(inline).parent;
                if prog.def(outer).kind == DefKind::AnonConst {
                    assert_ne!(outer, inline);
                    nested_inline = true;
                }
            }
        }
        assert!(nested_inline);
        for (&key, &def) in &prog.node_id_to_def_id { assert_eq!(prog.def_id_to_node_id.get(&def), Some(&key)); }
        let before = prog.defs.len();
        for d in 0..before { if prog.def(DefId(d as u32)).kind == DefKind::Fn { collect_definitions(&mut prog, DefId(d as u32)).unwrap(); } }
        assert_eq!(prog.defs.len(), before);
        let files = prog.current_source_files().into_iter().collect();
        let new = prog.recollect_check_crate(0, files).unwrap();
        assert!(!new.is_empty());
        for (&key, &d) in &prog.node_id_to_def_id {
            assert_eq!(prog.def_id_to_node_id.get(&d), Some(&key));
            assert!(matches!(prog.def(d).kind, DefKind::AnonConst | DefKind::OpaqueTy | DefKind::Closure | DefKind::Field));
            assert!(d.0 as usize >= before);
        }
        std::fs::remove_file(path).unwrap(); std::fs::remove_dir(dir).unwrap();
    }
}

#[cfg(test)]
mod legacy_tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn legacy_const_generics_resolve_before_publishing_definitions() {
        let dir = std::env::temp_dir().join(format!("relax-legacy-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dep = dir.join("dep.rs");
        let root = dir.join("lib.rs");
        std::fs::write(&dep, r#"#![no_std]
            #[rustc_legacy_const_generics(1)] pub fn f<const N: i32>(x: i32) -> i32 { x + N }
        "#).unwrap();
        std::fs::write(&root, r#"#![no_std]
            use dep::f as alias;
            fn renamed() { alias(1, 2 + 3); }
            fn qualified() { dep::f(1, -2); }
            fn explicit() { alias::<3>(1); }
            fn local_binding() { let alias = |x, y| x + y; alias(1, 2); }
            fn param(alias: fn(i32, i32)) { alias(1, 2); }
            fn nested() { let _ = || alias(1, 2); }
            fn inner_item() { let alias = |x, y| x + y; { use dep::f as alias; alias(1, 2); } }
            fn let_init() { let alias = alias(1, 2); }
            fn branch() { if let Some(alias) = None { alias(1, 2); } else { alias(1, 2); } }
            fn local_crate() { own(1, 2); }
            #[rustc_legacy_const_generics(1)] fn own<const N: i32>(x: i32) {}
            fn reject_closure() { alias(1, (|| 2)()); }
            fn reject_const() { alias(1, const { 2 }); }
            fn reject_item() { alias(1, { const C: i32 = 2; C }); }
            fn reject_repeat() { alias(1, [2; 1][0]); }
        "#).unwrap();
        let mut prog = Program::new(&Arc::new(crate::Session::default()));
        let cfg = crate::cfg::CfgSet::host(&prog.session, &[]);
        let dependency = prog.load_crate("dep", dep.to_str().unwrap(), 2021, cfg.clone(), vec![]).unwrap();
        let sym = prog.syms.intern("dep");
        let krate = prog.load_crate("test", root.to_str().unwrap(), 2021, cfg, vec![(sym, dependency)]).unwrap();
        prog.resolve_imports();
        let mut names = Vec::new();
        let mut rejected = 0;
        for result in prog.legacy_const_args.values() {
            let file = result.file;
            for split in result.calls.values() {
                assert_eq!(split.positions, vec![1]);
                assert_eq!(split.value_args.len(), 1);
                assert_eq!(split.const_args.len(), 1);
                let d = prog.node_id_to_def_id[&(file, NodeId::AnonConst(split.const_args[0]))];
                assert_eq!(prog.def(d).krate, krate);
                let mut owner = prog.def(d).parent;
                while prog.def(owner).kind == DefKind::Closure { owner = prog.def(owner).parent; }
                names.push(prog.name(owner).to_string());
                if split.invalid { rejected += 1; }
            }
        }
        names.sort();
        assert_eq!(names, ["branch", "inner_item", "let_init", "nested", "qualified", "reject_closure", "reject_const", "reject_item", "reject_repeat", "renamed"]);
        assert_eq!(rejected, 4);
        assert_eq!(prog.errors.len(), 4, "{:?}", prog.errors);
        let count = prog.defs.len();
        lower_legacy_const_generics(&mut prog);
        assert_eq!(prog.defs.len(), count);
        assert_eq!(prog.errors.len(), 4);
        std::fs::remove_file(dep).unwrap();
        std::fs::remove_file(root).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }
}

#[cfg(test)]
mod legacy_attribute_tests {
    use crate::program::Program;
    #[test]
    fn legacy_attribute_indices_and_arity() {
        let dir = std::env::temp_dir().join(format!("relax-legacy-attributes-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("lib.rs");
        std::fs::write(&path, r#"#![no_std]
            #[rustc_legacy_const_generics(2)] fn out_of_bounds<const N: i32>(x: i32) {}
            #[rustc_legacy_const_generics(0)] fn arity<const N: i32, const M: i32>() {}
            #[rustc_legacy_const_generics(0)] fn type_param<T>() {}
            #[rustc_legacy_const_generics(0u8)] fn suffix<const N: i32>() {}
            #[rustc_legacy_const_generics()] fn empty() {}
            #[rustc_legacy_const_generics(0x0)] fn valid<const N: i32>() {}
        "#).unwrap();
        let mut prog = crate::program::Program::new(&std::sync::Arc::new(crate::Session::default()));
        let cfg = crate::cfg::CfgSet::host(&prog.session, &[]);
        prog.load_crate("attrs", path.to_str().unwrap(), 2021, cfg, vec![]).unwrap();
        prog.resolve_imports();
        assert_eq!(prog.errors.len(), 5, "{:?}", prog.errors);
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }
    #[test]
    fn round34_constant_ribs() {
        let dir = std::env::temp_dir().join(format!("relax-constant-ribs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("lib.rs");
        std::fs::write(&path, br#"#![no_std]
            struct A<const N: usize>;
            struct Defaults<const N: usize = 2, const M: usize = N>;
            fn bare<const N: usize>(_: A<{ N }>) {}
            fn inline<const N: usize>() { let _ = const { N + 1 }; }
            fn repeat_type<T>() { let _ = [0; size_of::<T>()]; }
            fn shadow<const N: usize>() { let _ = [0; { const N: usize = 3; N + 1 }]; }
            fn op<const N: usize>() -> [u8; N + 1] { loop {} }
            fn ty<T>() -> [u8; size_of::<T>()] { loop {} }
            fn repeat<const N: usize>() { let _ = [0; N + 1]; }
            struct BadDefault<const N: usize, const M: usize = {N + 1}>;
            struct Forward<const N: usize = M, const M: usize = 2>;
            struct ParamTy<T, const N: T>(T);
            enum E<const N: usize> { A = N as isize }
            const fn id<const N: usize>() -> usize { N }
            fn nested_arg<const N: usize>() -> [u8; id::<N>()] { loop {} }
        "#).unwrap();
        let mut prog = Program::new(&std::sync::Arc::new(crate::Session::default()));
        let cfg = crate::cfg::CfgSet::host(&prog.session, &[]);
        prog.load_crate("ribs", path.to_str().unwrap(), 2021, cfg, vec![]).unwrap();
        prog.resolve_imports();
        assert_eq!(prog.errors.iter().filter(|e| e.contains("generic parameters may not be used in const operations")).count(), 5, "{:?}", prog.errors);
        assert_eq!(prog.errors.iter().filter(|e| e.contains("E0128")).count(), 1, "{:?}", prog.errors);
        assert_eq!(prog.errors.iter().filter(|e| e.contains("E0770")).count(), 1, "{:?}", prog.errors);
        assert_eq!(prog.errors.iter().filter(|e| e.contains("enum discriminant")).count(), 1, "{:?}", prog.errors);
        assert_eq!(prog.repeat_expr_counts.len(), 3);
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_dir(&dir).unwrap();
    }

}

crate::collected_enum!(NodeId; 0 => AnonConst(e), 1 => InlineConst(e), 2 => OpaqueTy(t), 3 => AsyncReturn(i), 4 => AsyncBody(i), 5 => Closure(e), 6 => Field(n));
crate::collected_fields!(LegacyConstArgs, callee, positions, const_args, value_args, invalid);

crate::collected_fields!(LegacyOwner, file, ranges, calls, children);

#[cfg(test)]
mod round45_tests {
    use super::*;
    use std::sync::Arc;

    fn pairs(prog: &Program, old: u32, next: &crate::program::FileVersion) -> Vec<(u32, u32)> {
        assert_eq!(prog.files[old as usize].ast.items.len(), next.pf.ast.items.len());
        (0..next.pf.ast.items.len() as u32).map(|i| (i, i)).collect()
    }
    fn owner(prog: &Program, name: &str) -> DefId {
        *prog.collected_definitions.iter().find(|&&d| prog.name(d) == name).unwrap()
    }
    fn setup(label: &str, source: &str, extra: Option<&str>) -> (Program, std::path::PathBuf, u32) {
        let dir = std::env::temp_dir().join(format!("relax-r45-{label}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dep = dir.join("dep.rs");
        let root = dir.join("lib.rs");
        std::fs::write(&dep, "#![no_std] #[rustc_legacy_const_generics(1)] pub fn f<const N: i32>(x: i32) -> i32 { x + N } pub fn g(x: i32, y: i32) -> i32 { x + y }").unwrap();
        std::fs::write(&root, source).unwrap();
        if let Some(extra) = extra { std::fs::write(dir.join("inner.rs"), extra).unwrap(); }
        let mut prog = Program::new(&Arc::new(crate::Session::default()));
        let cfg = crate::cfg::CfgSet::host(&prog.session, &[]);
        let dep = prog.load_crate("dep", dep.to_str().unwrap(), 2021, cfg.clone(), vec![]).unwrap();
        let sym = prog.syms.intern("dep");
        let krate = prog.load_crate("test", root.to_str().unwrap(), 2021, cfg, vec![(sym, dep)]).unwrap();
        prog.resolve_imports();
        assert!(prog.errors.is_empty(), "{:?}", prog.errors);
        let file = prog.mods[prog.crates[krate as usize].root_mod as usize].file;
        (prog, dir, file)
    }
    fn cleanup(dir: std::path::PathBuf) {
        std::fs::remove_file(dir.join("lib.rs")).unwrap();
        std::fs::remove_file(dir.join("dep.rs")).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }

    #[test]
    fn body_owner_count_coordinates_children_and_rollback() {
        let source = "#![no_std] fn a() { dep::f(1, 2); } fn b() { dep::f(3, 4); } trait Tr { fn get() -> impl Tr; } struct S; impl Tr for S { fn get() -> impl Tr { S } }";
        let (mut prog, dir, old) = setup("body", source, None);
        let a = owner(&prog, "a");
        let b = owner(&prog, "b");
        let untouched = owner(&prog, "f");
        let saved = prog.legacy_const_args.clone();
        let rpitit = crate::crate_image::encode(&prog.impl_trait_in_trait_data);
        let associated = crate::crate_image::encode(&prog.associated_types_for_impl_traits);
        let a_child = saved[&a].children[0].1;
        let b_child = saved[&b].children[0].1;
        let src = source.replace("dep::f(1, 2);", "let x = 5; dep::f(x, 6);");
        let v = prog.parse_version(old, dir.join("lib.rs").to_str().unwrap(), src.into_bytes()).unwrap();
        let pairs = pairs(&prog, old, &v);
        let new = prog.commit_body_version(v, &pairs).unwrap();
        assert_eq!(prog.lower_body_declarations(&[a]), 1);
        assert!(Arc::ptr_eq(&saved[&untouched], &prog.legacy_const_args[&untouched]));
        assert_eq!(prog.legacy_const_args[&a].children[0].1, a_child);
        assert_eq!(prog.legacy_const_args[&b].children[0].1, b_child);
        let b_result = &prog.legacy_const_args[&b];
        assert_eq!(b_result.file, new);
        let (&call, split) = b_result.calls.iter().next().unwrap();
        assert!(prog.legacy_const_call(b, new, call).is_some());
        let e = prog.files[new as usize].ast.expr(split.const_args[0]);
        assert_eq!(prog.files[new as usize].bytes(e.lo, e.hi), b"4");
        assert_eq!(prog.node_id_to_def_id[&(new, NodeId::AnonConst(split.const_args[0]))], b_child);
        assert_eq!(crate::crate_image::encode(&prog.impl_trait_in_trait_data), rpitit);
        assert_eq!(crate::crate_image::encode(&prog.associated_types_for_impl_traits), associated);
        prog.revert_version(old, new, &pairs);
        assert_eq!(crate::crate_image::encode(&prog.legacy_const_args), crate::crate_image::encode(&saved));
        assert!(Arc::ptr_eq(&saved[&a], &prog.legacy_const_args[&a]));
        for d in [a, b] {
            for &(e, child) in &saved[&d].children {
                assert_eq!(prog.node_id_to_def_id[&(old, NodeId::AnonConst(e))], child);
                assert_eq!(prog.def_id_to_node_id[&child], (old, NodeId::AnonConst(e)));
            }
        }
        assert_eq!(crate::crate_image::encode(&prog.impl_trait_in_trait_data), rpitit);
        cleanup(dir);
    }

    #[test]
    fn changed_body_keeps_constant_restrictions_and_refusal_state() {
        let source = "#![no_std] fn a<const N: usize>() { let _ = [0; N]; } fn b() { dep::f(3, 4); }";
        let (mut prog, dir, old) = setup("restrictions", source, None);
        let a = owner(&prog, "a");
        let before = crate::crate_image::encode(&prog.legacy_const_args);
        for text in [source.replace("[0; N]", "[0; N + 1]"), source.replace("[0; N]", "[1; N]")] {
            let v = prog.parse_version(old, dir.join("lib.rs").to_str().unwrap(), text.as_bytes().to_vec()).unwrap();
            let pairs = pairs(&prog, old, &v);
            let new = prog.commit_body_version(v, &pairs).unwrap();
            assert_eq!(prog.lower_body_declarations(&[a]), 1);
            if text.contains("N + 1") {
                assert!(prog.errors.iter().any(|e| e.contains("generic parameters may not be used in const operations")));
            } else { assert!(prog.errors.is_empty(), "{:?}", prog.errors); }
            prog.revert_version(old, new, &pairs);
            assert!(prog.errors.is_empty());
            assert_eq!(crate::crate_image::encode(&prog.legacy_const_args), before);
        }
        cleanup(dir);
    }

    #[test]
    fn restored_children_reserve_ids_for_later_resolution_changes() {
        let source = "#![no_std] use dep::g as alias; mod inner;";
        let inner = "pub fn b() { dep::f(1, 2); crate::alias(3, 4); }";
        let (mut prog, dir, root) = setup("child-counter", source, Some(inner));
        let b = owner(&prog, "b");
        let old = prog.def(b).file;
        let child = prog.legacy_const_args[&b].children[0].1;
        let v = prog.parse_version(old, dir.join("inner.rs").to_str().unwrap(), inner.replace("(1, 2)", "(5, 6)").into_bytes()).unwrap();
        let body_pairs = pairs(&prog, old, &v);
        let new = prog.commit_body_version(v, &body_pairs).unwrap();
        prog.lower_body_declarations(&[b]);
        prog.revert_version(old, new, &body_pairs);
        let v = prog.parse_version(root, dir.join("lib.rs").to_str().unwrap(), source.replace("dep::g as alias", "dep::f as alias").into_bytes()).unwrap();
        let pairs = pairs(&prog, root, &v);
        let use_item = prog.files[root as usize].ast.root_items[0];
        let pos = prog.files[root as usize].ast.item(use_item).lo;
        let module = prog.imports.iter().find(|i| i.file == root && i.pos == pos).unwrap().module;
        prog.unbind_use(root, pos);
        let imports = prog.imports.len();
        let new = prog.commit_version(v, &pairs).unwrap();
        prog.add_items(new, &[(use_item, Ok(module))]).unwrap();
        prog.resolve_imports_from(imports);
        let children = &prog.legacy_const_args[&b].children;
        assert_eq!(children.len(), 2);
        assert_eq!(children[0].1, child);
        assert_ne!(children[0].1, children[1].1, "a newly legacy call reused the restored child's DefId");
        std::fs::remove_file(dir.join("inner.rs")).unwrap();
        cleanup(dir);
    }

    #[test]
    fn alias_edit_recomputes_all_owners_and_restores_results() {
        let source = "#![no_std] use dep::f as alias; fn a() { alias(1, 2); } mod inner;";
        let (mut prog, dir, old) = setup("alias", source, Some("pub fn b() { crate::alias(3, 4); }"));
        let saved = prog.legacy_const_args.clone();
        let a = owner(&prog, "a");
        let b = owner(&prog, "b");
        assert_eq!(saved[&a].calls.len(), 1);
        assert_eq!(saved[&b].calls.len(), 1);
        let v = prog.parse_version(old, dir.join("lib.rs").to_str().unwrap(), source.replace("dep::f as alias", "dep::g as alias").into_bytes()).unwrap();
        let pairs = pairs(&prog, old, &v);
        let use_item = prog.files[old as usize].ast.root_items[0];
        let pos = prog.files[old as usize].ast.item(use_item).lo;
        let uses = prog.unbind_use(old, pos);
        let imports = prog.imports.len();
        let module = prog.def(a).scope;
        let new = prog.commit_version(v, &pairs).unwrap();
        prog.add_items(new, &[(use_item, Ok(module))]).unwrap();
        prog.resolve_imports_from(imports);
        let count = lower_legacy_const_generics(&mut prog);
        assert_eq!(count, prog.collected_definitions.len());
        for (&d, previous) in &saved { assert!(!Arc::ptr_eq(previous, &prog.legacy_const_args[&d])); }
        assert!(prog.legacy_const_args[&a].calls.is_empty());
        assert!(prog.legacy_const_args[&b].calls.is_empty());
        assert_ne!(prog.def(b).file, new);
        prog.unbind_imports_from(imports);
        prog.rebind_use(uses);
        prog.revert_version(old, new, &pairs);
        assert_eq!(crate::crate_image::encode(&prog.legacy_const_args), crate::crate_image::encode(&saved));
        std::fs::remove_file(dir.join("inner.rs")).unwrap();
        cleanup(dir);
    }
}
