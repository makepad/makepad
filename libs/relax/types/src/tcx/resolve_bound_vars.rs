// ported from N:rustc_hir_analysis/src/collect/resolve_bound_vars.rs @8d1a764
use super::*;
use super::lifetime_resolve::{LifetimeDecl, LifetimeRes, LifetimeResolution};
use crate::types::{BoundVar, BoundRegionKind};
use crate::next_solver::BoundVariableKind;
use crate::types::variance::GenericIndices;

pub use crate::types::LifetimeId;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum BinderId { Item, FnPointer(u32), Trait(u32), Where(u32), Closure(u32), GenericBound(u32), ArgumentImplTrait(u32), Opaque(DefId) }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(super) enum LifetimeKey {
    Named(u32, u32),
    Reference(u32),
    SelfReference,
    Path(u32, u32),
    ObjectDefault(u32),
    /// A use inside the where-predicate with binder `Where(pred)`. The derives reuse one type
    /// node under several predicates with different binders, so a use is keyed by the
    /// predicate it is resolved in as well as by its node (the unscoped key stays: last wins).
    Scoped(u32, u32, u32, u32),
}

impl LifetimeKey {
    pub(super) fn scoped(self, pred: Option<u32>) -> LifetimeKey {
        let Some(pred) = pred else { return self };
        match self {
            LifetimeKey::Named(a, b) => LifetimeKey::Scoped(pred, 0, a, b),
            LifetimeKey::Reference(a) => LifetimeKey::Scoped(pred, 1, a, 0),
            LifetimeKey::SelfReference => LifetimeKey::Scoped(pred, 2, 0, 0),
            LifetimeKey::Path(a, b) => LifetimeKey::Scoped(pred, 3, a, b),
            LifetimeKey::ObjectDefault(a) => LifetimeKey::Scoped(pred, 4, a, 0),
            scoped @ LifetimeKey::Scoped(..) => scoped,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum ResolvedArg {
    StaticLifetime,
    EarlyBound(LifetimeId),
    LateBound(u32, BoundVar, LifetimeId),
    Free(DefId, LifetimeId),
    Error,
}
impl ResolvedArg {
    fn shifted(self, depth: u32) -> Self {
        match self { Self::LateBound(d, var, id) => Self::LateBound(d + depth, var, id), _ => self }
    }
}

#[derive(Debug)]
pub struct ResolveBoundVars {
    // shared with the lifetime_names query result; copied only when an opaque adds declarations
    pub(super) names: std::sync::Arc<LifetimeResolution>,
    pub(super) named_variable_map: HashMap<LifetimeKey, ResolvedArg>,
    pub(super) late_bound_vars_map: HashMap<BinderId, Vec<BoundVariableKind>>,
    pub(crate) opaque_captured_lifetimes: HashMap<DefId, Vec<(ResolvedArg, LifetimeId)>>,
    pub(super) early_indices: HashMap<LifetimeId, u32>,
    pub(crate) generic_indices: GenericIndices,
}

static TRIVIAL: relax_session::sync::Mutex<Option<std::collections::HashMap<crate::types::variance::GenericIndices, std::sync::Arc<ResolveBoundVars>>>> = relax_session::sync::Mutex::new(None);

pub(crate) struct ZoneTaken(Option<std::collections::HashMap<GenericIndices, std::sync::Arc<ResolveBoundVars>>>);

pub(crate) fn zone_take() -> ZoneTaken {
    let mut trivial = TRIVIAL.lock().unwrap_or_else(|e| e.into_inner());
    let taken = ZoneTaken(std::mem::take(&mut *trivial));
    #[cfg(test)]
    assert!(trivial.is_none());
    taken
}

pub(crate) fn zone_put_back(taken: ZoneTaken) {
    let mut trivial = TRIVIAL.lock().unwrap_or_else(|e| e.into_inner());
    std::mem::forget(std::mem::replace(&mut *trivial, taken.0));
}

impl ResolveBoundVars {
    /// An item without lifetimes: its answer depends only on its generic indices, so equal
    /// ones are one shared value (most items; each distinct index set is stored once).
    pub(crate) fn share(self) -> std::sync::Arc<ResolveBoundVars> {
        let trivial = self.names.declarations.is_empty() && self.names.inherited.is_empty() && self.names.uses.is_empty() && self.names.binders.is_empty()
            && self.named_variable_map.is_empty() && self.opaque_captured_lifetimes.is_empty() && self.early_indices.is_empty()
            && self.late_bound_vars_map.len() == 1 && self.late_bound_vars_map.get(&BinderId::Item).is_some_and(|v| v.is_empty());
        if !trivial { return std::sync::Arc::new(self); }
        let mut table = TRIVIAL.lock().unwrap_or_else(|e| e.into_inner());
        table.get_or_insert_with(Default::default).entry(self.generic_indices.clone()).or_insert_with(|| std::sync::Arc::new(self)).clone()
    }
    /// memory report: (names owned here, other maps) heap bytes; shared names count as 0
    pub(crate) fn heap_bytes_split(&self) -> (usize, usize) {
        fn hm<K, V, S>(m: &std::collections::HashMap<K, V, S>) -> usize { m.capacity() * (std::mem::size_of::<(K, V)>() + 1) * 8 / 7 }
        let names = if std::sync::Arc::strong_count(&self.names) > 1 { 0 } else { self.names.heap_bytes() };
        let rest = hm(&self.named_variable_map) + hm(&self.late_bound_vars_map) + self.late_bound_vars_map.values().map(|v| v.capacity() * std::mem::size_of::<crate::next_solver::BoundVariableKind>()).sum::<usize>()
            + hm(&self.opaque_captured_lifetimes) + self.opaque_captured_lifetimes.values().map(|v| v.capacity() * std::mem::size_of::<(ResolvedArg, LifetimeId)>()).sum::<usize>()
            + hm(&self.early_indices) + (self.generic_indices.lifetimes.capacity() + self.generic_indices.types_consts.capacity() + self.generic_indices.consts.capacity()) * 4;
        (names, rest)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BinderScopeType { Normal, Concatenating }

enum Scope {
    Root { parent: Vec<LifetimeId> },
    Binder { bound_vars: Vec<(LifetimeId, ResolvedArg)>, scope_type: BinderScopeType, id: BinderId, apit: bool },
    Body(DefId),
    ObjectLifetimeDefault(Option<ResolvedArg>),
    Supertrait(Vec<BoundVariableKind>),
    TraitRefBoundary,
    Opaque(DefId),
    LateBoundary { what: &'static str, deny_late_regions: bool },
}

struct BoundVarContext<'a> {
    tcx: &'a Tcx,
    prog: &'a Program,
    owner: DefId,
    file: u32,
    module: u32,
    rbv: ResolveBoundVars,
    scopes: Vec<Scope>,
    capture_errors: Vec<u32>,
}

impl BoundVarContext<'_> {
    fn cause(&self, span: u32) -> crate::next_solver::fulfill::ObligationCause {
        crate::next_solver::fulfill::ObligationCause { body_id: self.owner, span }
    }

    /// The where-predicate whose binder encloses the current position.
    fn pred(&self) -> Option<u32> {
        self.scopes.iter().rev().find_map(|scope| match scope { Scope::Binder { id: BinderId::Where(n), .. } => Some(*n), _ => None })
    }
    /// The recorded mapping of a use: the one made in the current predicate, else the plain one.
    fn mapped(&self, key: LifetimeKey) -> Option<ResolvedArg> {
        self.rbv.named_variable_map.get(&key.scoped(self.pred())).or_else(|| self.rbv.named_variable_map.get(&key)).copied()
    }
    fn map_use(&mut self, key: LifetimeKey, arg: ResolvedArg) {
        if let Some(pred) = self.pred() { self.rbv.named_variable_map.insert(key.scoped(Some(pred)), arg); }
        self.rbv.named_variable_map.insert(key, arg);
    }

    fn declaration(&self, id: LifetimeId) -> &LifetimeDecl {
        if id.owner == self.owner { &self.rbv.names.declarations[id.local as usize] }
        else { self.rbv.names.inherited.iter().find(|d| d.id == id).expect("inherited lifetime declaration") }
    }

    /// resolve_bound_vars.rs:1560-1668. Captures are remapped from the outermost
    /// opaque inward; an inner opaque captures the outer opaque's fresh identity.
    fn remap_opaque_captures(&mut self, scopes: &[DefId], mut lifetime: ResolvedArg, span: u32) -> ResolvedArg {
        if !scopes.is_empty() {
            if let ResolvedArg::LateBound(_, _, id) = lifetime {
                if self.declaration(id).binder != BinderId::Item {
                    self.capture_errors.push(span);
                    self.tcx.dcx.delayed_bug(self.cause(span), "opaque capture error not emitted".into());
                    lifetime = ResolvedArg::Error;
                }
            }
        }
        for &opaque in scopes.iter().rev() {
            let existing = self.rbv.opaque_captured_lifetimes.entry(opaque).or_default().iter().find(|(arg, _)| *arg == lifetime).map(|(_, id)| *id);
            let id = if let Some(id) = existing { id } else {
                let id = LifetimeId { owner: self.owner, local: self.rbv.names.declarations.len() as u32 };
                let name = match lifetime {
                    ResolvedArg::EarlyBound(id) | ResolvedArg::LateBound(_, _, id) | ResolvedArg::Free(_, id) => self.declaration(id).name.clone(),
                    _ => None,
                };
                std::sync::Arc::make_mut(&mut self.rbv.names).declarations.push(LifetimeDecl { id, binder: BinderId::Opaque(opaque), name, span });
                self.rbv.opaque_captured_lifetimes.get_mut(&opaque).unwrap().push((lifetime, id));
                id
            };
            lifetime = ResolvedArg::EarlyBound(id);
        }
        lifetime
    }

    fn emit_opaque_capture_errors(&mut self) {
        let spans = std::mem::take(&mut self.capture_errors);
        for span in spans {
            self.tcx.dcx.report(self.cause(span), "opaque type captures a higher-ranked lifetime".into());
        }
    }

    fn visit_lifetime(&mut self, key: LifetimeKey, span: u32) {
        let res = self.rbv.names.uses.get(&key.scoped(self.pred())).or_else(|| self.rbv.names.uses.get(&key)).copied().expect("lifetime resolved before bound-variable collection");
        match res {
            LifetimeRes::Static => self.map_use(key, ResolvedArg::StaticLifetime),
            LifetimeRes::Error => self.map_use(key, ResolvedArg::Error),
            LifetimeRes::Infer => {}
            LifetimeRes::Param(id) => self.resolve_lifetime_ref(id, key, span),
        }
    }

    fn named_lifetime(&mut self, name: ast::Ident) { self.visit_lifetime(LifetimeKey::Named(name.lo, name.hi), name.lo); }

    /// resolve_bound_vars.rs:1354-1560: opaque boundaries reset late depth;
    /// only late-bound parameters are liberated when crossing a body scope.
    fn resolve_lifetime_ref(&mut self, id: LifetimeId, key: LifetimeKey, span: u32) {
        let mut depth = 0;
        let mut body = None;
        let mut boundary = None;
        let mut captures = Vec::new();
        let mut result = None;
        for scope in self.scopes.iter().rev() {
            match scope {
                Scope::Root { parent } => {
                    if parent.contains(&id) { result = Some(ResolvedArg::EarlyBound(id)); }
                    break;
                }
                Scope::Binder { bound_vars, scope_type, apit, .. } => {
                    if let Some((_, arg)) = bound_vars.iter().find(|(decl, _)| *decl == id) {
                        result = Some(arg.shifted(depth));
                        break;
                    }
                    if *scope_type == BinderScopeType::Normal { depth += 1; }
                    if *apit && self.declaration(id).name.is_none() {
                        let def = self.prog.def(self.owner);
                        let ast = &self.prog.files[self.file as usize].ast;
                        let async_fn = matches!(ast.item(def.item).kind, ItemKind::Fn(sig, _, _) if ast.fn_sig(sig).asyncness);
                        if !async_fn {
                            self.tcx.dcx.report(self.cause(span), "anonymous lifetimes in impl Trait are unstable".into());
                            return;
                        }
                    }
                }
                Scope::Body(owner) => body = Some(*owner),
                Scope::Opaque(def) => { captures.push(*def); depth = 0; }
                Scope::LateBoundary { what, deny_late_regions: true } => boundary = Some(*what),
                Scope::LateBoundary { deny_late_regions: false, .. } | Scope::ObjectLifetimeDefault(_) | Scope::Supertrait(_) | Scope::TraitRefBoundary => {}
            }
        }
        if let Some(arg) = result {
            let mut arg = self.remap_opaque_captures(&captures, arg, span);
            if let ResolvedArg::LateBound(_, _, id) = arg {
                if let Some(what) = boundary {
                    self.tcx.dcx.report(self.cause(span), format!("cannot capture late-bound lifetime in {what}"));
                    arg = ResolvedArg::Error;
                } else if let Some(body) = body { arg = ResolvedArg::Free(body, id); }
            }
            self.map_use(key, arg);
        } else if self.scopes.iter().any(|scope| matches!(scope, Scope::Binder { apit: true, .. })) {
            self.tcx.dcx.report(self.cause(span), "lifetime is not accessible in this impl Trait bound".into());
        } else {
            self.tcx.dcx.delayed_bug(self.cause(span), "could not resolve lifetime in bound-variable scope".into());
        }
    }

    fn poly_trait_ref_binder_info(&mut self) -> (Vec<BoundVariableKind>, BinderScopeType) {
        let mut supertrait_vars = Vec::new();
        for scope in self.scopes.iter().rev() {
            match scope {
                Scope::Body(_) | Scope::Root { .. } => break,
                Scope::Supertrait(vars) => supertrait_vars = vars.clone(),
                Scope::TraitRefBoundary => {
                    if !supertrait_vars.is_empty() {
                        self.tcx.dcx.delayed_bug(self.cause(0), "supertrait lifetimes without a binder".into());
                    }
                    break;
                }
                Scope::Binder { id, .. } => {
                    let mut vars = self.rbv.late_bound_vars_map.get(id).cloned().unwrap_or_default();
                    vars.extend(supertrait_vars);
                    return (vars, BinderScopeType::Concatenating);
                }
                Scope::Opaque(_) | Scope::ObjectLifetimeDefault(_) | Scope::LateBoundary { .. } => {}
            }
        }
        (Vec::new(), BinderScopeType::Normal)
    }

    fn push_binder(&mut self, id: BinderId, mut binders: Vec<BoundVariableKind>, scope_type: BinderScopeType, apit: bool) {
        let mut bound_vars = Vec::new();
        if let Some(declarations) = self.rbv.names.binders.get(&id) {
            for &decl in declarations {
                let arg = ResolvedArg::LateBound(0, BoundVar(binders.len() as u32), decl);
                bound_vars.push((decl, arg));
                binders.push(BoundVariableKind::Region(BoundRegionKind::Named(decl)));
            }
        }
        self.rbv.late_bound_vars_map.insert(id, binders);
        self.scopes.push(Scope::Binder { bound_vars, scope_type, id, apit });
    }

    fn resolve_object_lifetime_default(&mut self, key: LifetimeKey) {
        let mut depth = 0;
        let mut captures = Vec::new();
        let mut result = ResolvedArg::StaticLifetime;
        for scope in self.scopes.iter().rev() {
            match scope {
                Scope::Binder { scope_type: BinderScopeType::Normal, .. } => depth += 1,
                Scope::Root { .. } => break,
                Scope::Body(_) | Scope::ObjectLifetimeDefault(None) => return,
                Scope::ObjectLifetimeDefault(Some(arg)) => { result = arg.shifted(depth); break; }
                Scope::Opaque(def) => { captures.push(*def); depth = 0; }
                _ => {}
            }
        }
        let result = self.remap_opaque_captures(&captures, result, 0);
        self.map_use(key, result);
    }

    /// rustc_hir_analysis check_opaque_precise_captures: the `use<..>` list names each parameter once, lifetimes
    /// first, and every type and const parameter in scope of the opaque.
    fn check_precise_captures(&mut self, bounds: ast::List<ast::Bound>) {
        let ast = &self.prog.files[self.file as usize].ast;
        let Some(args) = ast.list(bounds).iter().find_map(|b| if let ast::Bound::Use(a) = b { Some(*a) } else { None }) else { return };
        let Some(GenericArgs::Angle(list)) = ast.gargs(args) else { return };
        let text = |i: ast::Ident| self.prog.files[self.file as usize].text(i).to_string();
        let mut seen: Vec<String> = Vec::new();
        let mut first_non_lifetime = false;
        for arg in ast.list(*list) {
            let (name, pos, lifetime) = match *arg {
                GenericArg::Lifetime(lt) => (text(lt), lt.lo, true),
                GenericArg::Type(t) => match ast.ty(t) {
                    ast::Ty::Path(p) if p.no_qself() && ast.segs(p.segs).len() == 1 => { let seg = ast.segs(p.segs)[0]; (text(seg.name), seg.name.lo, false) }
                    _ => continue,
                },
                _ => continue,
            };
            if lifetime && first_non_lifetime {
                self.tcx.dcx.report(self.cause(pos), format!("lifetime parameter `{}` must be listed before non-lifetime parameters", name));
            }
            if !lifetime { first_non_lifetime = true; }
            if seen.contains(&name) {
                self.tcx.dcx.report(self.cause(pos), format!("cannot capture parameter `{}` twice", name));
            }
            seen.push(name);
        }
    }

    fn visit_opaque_ty(&mut self, opaque: DefId, bounds: ast::List<ast::Bound>, async_output: Option<ast::TyId>) {
        if self.tcx.validates(self.prog, self.file) { self.check_precise_captures(bounds); }
        self.rbv.opaque_captured_lifetimes.entry(opaque).or_default();
        let ast = &self.prog.files[self.file as usize].ast;
        let span = ast.item(self.prog.def(self.owner).item).lo;
        if opaque_captures_all_in_scope_lifetimes(self.tcx, self.prog, self.owner, bounds, matches!(self.prog.def_id_to_node_id.get(&opaque), Some((_, relax_front::def_collector::NodeId::AsyncReturn(_))))) {
            let mut depth = 0;
            let mut captures = vec![opaque];
            for i in (0..self.scopes.len()).rev() {
                match &self.scopes[i] {
                    Scope::Binder { bound_vars, scope_type, .. } => {
                        let vars = bound_vars.clone();
                        let normal = *scope_type == BinderScopeType::Normal;
                        for &(decl, arg) in vars.iter().rev() {
                            self.remap_opaque_captures(&captures, arg.shifted(depth), self.declaration(decl).span);
                        }
                        if normal { depth += 1; }
                    }
                    Scope::Root { parent } => {
                        let parent = parent.clone();
                        for &decl in parent.iter().rev() {
                            self.remap_opaque_captures(&captures, ResolvedArg::EarlyBound(decl), self.declaration(decl).span);
                        }
                        break;
                    }
                    Scope::Opaque(def) => { captures.push(*def); depth = 0; }
                    Scope::Body(_) => panic!("return opaque capture inside body scope"),
                    Scope::ObjectLifetimeDefault(_) | Scope::Supertrait(_) | Scope::TraitRefBoundary | Scope::LateBoundary { .. } => {}
                }
            }
            self.rbv.opaque_captured_lifetimes.get_mut(&opaque).unwrap().reverse();
        }
        self.scopes.push(Scope::Opaque(opaque));
        self.scopes.push(Scope::TraitRefBoundary);
        self.scopes.push(Scope::LateBoundary { what: "nested impl Trait", deny_late_regions: false });
        self.visit_bounds(bounds);
        if let Some(output) = async_output { self.visit_ty(output); }
        self.scopes.pop(); self.scopes.pop(); self.scopes.pop();
        let _ = span;
        self.emit_opaque_capture_errors();
    }
}

fn opaque_captures_all_in_scope_lifetimes(_tcx: &Tcx, prog: &Program, owner: DefId, bounds: ast::List<ast::Bound>, is_async: bool) -> bool {
    let def = prog.def(owner);
    let ast = &prog.files[def.file as usize].ast;
    if ast.list(bounds).iter().any(|b| matches!(b, ast::Bound::Use(_))) { return false; }
    let parent = prog.def(def.parent);
    let trait_impl = parent.kind == DefKind::Impl && { let a = &prog.files[parent.file as usize].ast; a.as_impl(&a.item(parent.item).kind).is_some_and(|i| i.trait_.is_some()) };
    is_async || prog.crates[def.krate as usize].edition >= 2024 || parent.kind == DefKind::Trait || trait_impl
}

struct LifetimeCollector<'a> {
    tcx: &'a Tcx,
    prog: &'a Program,
    owner: DefId,
    names: &'a LifetimeResolution,
    constrained: bool,
    regions: Vec<LifetimeId>,
    has_fully_capturing_opaque: bool,
}
impl LifetimeCollector<'_> {
    fn lifetime(&mut self, key: LifetimeKey) {
        if let Some(LifetimeRes::Param(id)) = self.names.uses.get(&key) {
            if !self.regions.contains(id) { self.regions.push(*id); }
        }
    }
    fn named(&mut self, name: ast::Ident) { self.lifetime(LifetimeKey::Named(name.lo, name.hi)); }
    fn generic_arg(&mut self, arg: ast::GenericArg) {
        match arg {
            GenericArg::Lifetime(name) => self.named(name),
            GenericArg::Type(ty) => self.ty(ty),
            GenericArg::Binding(_, args, ty) => { self.args(args); self.ty(ty); }
            GenericArg::Constraint(_, args, bounds) => { self.args(args); self.bounds(bounds); },
            GenericArg::Const(_) => {}
        }
    }
    fn args(&mut self, args: ast::GenericArgsId) {
        let ast = &self.prog.files[self.prog.def(self.owner).file as usize].ast;
        match ast.gargs(args) {
            Some(GenericArgs::Angle(arguments)) => for &arg in ast.list(*arguments) { self.generic_arg(arg); },
            Some(GenericArgs::Paren(inputs, output)) => {
                for &ty in ast.list(*inputs) { self.ty(ty); }
                if let Some(ty) = output { self.ty(*ty); }
            }
            None => {}
        }
    }
    fn path(&mut self, path: &ast::Path) {
        let file = self.prog.def(self.owner).file;
        let ast = &self.prog.files[file as usize].ast;
        if self.constrained {
            if path.has_qself() { return; }
            if path.segs.len() > 1 {
                let root = ast.segs(path.segs)[0];
                let symbol = self.prog.syms.get(self.prog.files[file as usize].text(root.name));
                if root.kind() == SegKind::SelfType || symbol.is_some_and(|name| self.tcx.lifetime_path_is_parameter(self.prog, self.owner, name)) { return; }
            }
            let resolved = self.tcx.lifetime_path_def(self.prog, self.owner, file, self.tcx.item_module(self.prog, self.owner), path);
            if let Some(def) = resolved {
                if self.prog.def(def).kind == DefKind::AssocTy { return; }
                if self.prog.def(def).kind == DefKind::TypeAlias {
                    self.alias_args(def, path);
                    return;
                }
            }
            if let Some(last) = ast.segs(path.segs).last() {
                self.args(last.args());
                let segment = path.segs.lo + path.segs.len() as u32 - 1;
                self.implicit_args(segment);
            }
        } else {
            if let Some(qself) = ast.qself(path) {
                self.ty(qself.ty);
                if let Some(path) = qself.trait_path { self.path(&path); }
            }
            for (i, segment) in ast.segs(path.segs).iter().enumerate() {
                self.args(segment.args());
                self.implicit_args(path.segs.lo + i as u32);
            }
        }
    }
    fn implicit_args(&mut self, segment: u32) {
        let mut i = 0;
        while self.names.uses.contains_key(&LifetimeKey::Path(segment, i)) {
            self.lifetime(LifetimeKey::Path(segment, i));
            i += 1;
        }
    }
    /// resolve_bound_vars.rs:2668-2781: inspect the alias's identity type and
    /// visit only arguments that remain outside projections after expansion.
    fn alias_args(&mut self, alias: DefId, path: &ast::Path) {
        use crate::types::fold::{TypeFolder, super_fold_ty};
        struct ConstrainedCollectorPostHirTyLowering { constrained: Vec<bool>, types: Vec<u32> }
        impl TypeFolder for ConstrainedCollectorPostHirTyLowering {
            fn fold_ty(&mut self, tys: &Types, ty: TyId, depth: u32) -> TyId {
                match tys.kind(ty) {
                    TyKind::Param(index) => {
                        if let Some(&index) = self.types.get(*index as usize) { self.constrained[index as usize] = true; }
                    }
                    TyKind::Alias(_, crate::types::AliasTy { kind: AliasTyKind::Projection { .. } | AliasTyKind::Inherent { .. }, .. }) => return ty,
                    _ => {}
                }
                super_fold_ty(tys, ty, self, depth)
            }
            fn fold_region(&mut self, tys: &Types, region: RegionId, _: u32) -> RegionId {
                if let RegionKind::ReEarlyParam(index) = tys.region_kind(region) {
                    if let Some(slot) = self.constrained.get_mut(index as usize) { *slot = true; }
                }
                region
            }
            fn fold_const(&mut self, _: &Types, value: crate::consts::Const, _: u32) -> crate::consts::Const { value }
        }
        let indices = self.tcx.generic_indices_of(self.prog, alias);
        let mut walker = ConstrainedCollectorPostHirTyLowering {
            constrained: vec![false; indices.lifetimes.len() + indices.types_consts.len()], types: indices.types_consts.clone(),
        };
        if let Ok(ty) = self.tcx.type_of(self.prog, alias) { walker.fold_ty(&self.tcx.tys, ty, 0); }
        let ast = &self.prog.files[self.prog.def(self.owner).file as usize].ast;
        let last = ast.segs(path.segs).last().expect("path segment");
        let segment = path.segs.lo + path.segs.len() as u32 - 1;
        let mut i = 0;
        while self.names.uses.contains_key(&LifetimeKey::Path(segment, i)) {
            if walker.constrained.get(i as usize) == Some(&true) { self.lifetime(LifetimeKey::Path(segment, i)); }
            i += 1;
        }
        if let Some(GenericArgs::Angle(arguments)) = ast.gargs(last.args()) {
            for &arg in ast.list(*arguments) {
                if matches!(arg, GenericArg::Binding(..) | GenericArg::Constraint(..)) { continue; }
                match walker.constrained.get(i as usize) {
                    Some(true) => self.generic_arg(arg),
                    Some(false) => {},
                    None => self.tcx.dcx.delayed_bug(crate::next_solver::fulfill::ObligationCause { body_id: self.owner, span: path.lo }, "incorrect generic argument count for alias".into()),
                }
                i += 1;
            }
        }
    }
    fn bounds(&mut self, bounds: ast::List<ast::Bound>) {
        let ast = &self.prog.files[self.prog.def(self.owner).file as usize].ast;
        for bound in ast.list(bounds) {
            match *bound {
                ast::Bound::Lifetime(name) => self.named(name),
                ast::Bound::Trait { path, .. } => self.path(&path),
                ast::Bound::Use(args) => self.args(args),
            }
        }
    }
    fn ty(&mut self, ty: ast::TyId) {
        let ast = &self.prog.files[self.prog.def(self.owner).file as usize].ast;
        match *ast.ty(ty) {
            ast::Ty::Path(path) => self.path(&path),
            ast::Ty::Ref(name, _, inner) => {
                self.lifetime(name.map_or(LifetimeKey::Reference(ty.0), |lt| LifetimeKey::Named(lt.lo, lt.hi)));
                self.ty(inner);
            }
            ast::Ty::Ptr(_, inner) | ast::Ty::Slice(inner) | ast::Ty::Array(inner, _) | ast::Ty::Paren(inner) => self.ty(inner),
            ast::Ty::Tuple(types) => for &ty in ast.list(types) { self.ty(ty); },
            ast::Ty::Fn(pointer) => {
                let pointer = ast.fn_ptr(pointer);
                for &ty in ast.list(pointer.params) { self.ty(ty); }
                if let Some(ty) = pointer.ret { self.ty(ty); }
            }
            ast::Ty::DynTrait(bounds, _) => self.bounds(bounds),
            ast::Ty::ImplTrait(bounds) => {
                self.has_fully_capturing_opaque |= opaque_captures_all_in_scope_lifetimes(self.tcx, self.prog, self.owner, bounds, false);
                self.bounds(bounds);
            }
            _ => {}
        }
    }
    fn generics(&mut self, generics: &ast::Generics) {
        let ast = &self.prog.files[self.prog.def(self.owner).file as usize].ast;
        for param in ast.list(generics.params) {
            match param.kind {
                GenericParamKind::Lifetime(bounds) => if !bounds.is_empty() {
                    self.named(param.name);
                    for &lt in ast.list(bounds) { self.named(lt); }
                },
                GenericParamKind::Type(bounds, default) => {
                    self.bounds(bounds);
                    if let Some(ty) = default { self.ty(ty); }
                }
                GenericParamKind::Const(ty, _) => self.ty(ty),
            }
        }
        for pred in ast.list(generics.where_) {
            match *pred {
                ast::WherePred::Lifetime(lt, bounds) => { self.named(lt); for &lt in ast.list(bounds) { self.named(lt); } }
                ast::WherePred::Bound { ty, bounds, .. } => { self.ty(ty); self.bounds(bounds); }
            }
        }
    }
}

pub(crate) fn owner_generics(prog: &Program, owner: DefId) -> ast::Generics {
    let def = prog.def(owner);
    let ast = &prog.files[def.file as usize].ast;
    match ast.kind_generics(&ast.item(def.item).kind) {
        Some(g) => *g,
        None => Default::default(),
    }
}

pub(crate) fn resolve_bound_vars(tcx: &Tcx, prog: &Program, owner: DefId, parent: Option<&ResolveBoundVars>) -> ResolveBoundVars {
    let def = prog.def(owner);
    let ast = &prog.files[def.file as usize].ast;
    let names = tcx.resolve_lifetime_names(prog, owner);
    let generics = owner_generics(prog, owner);
    let function = match ast.item(def.item).kind { ItemKind::Fn(sig, _, _) => Some(ast.fn_sig(sig)), _ => None };
    let mut late = Vec::new();
    let mut apits = Vec::new();
    if let Some(sig) = function {
        for param in ast.list(sig.params) { super::collect_argument_impl_traits(ast, param.ty, &mut apits); }
    }
    if let Some(sig) = function {
        let collector = |constrained| LifetimeCollector { tcx, prog, owner, names: &names, constrained, regions: Vec::new(), has_fully_capturing_opaque: false };
        let mut inputs = collector(true);
        for p in ast.list(sig.params) { inputs.ty(p.ty); }
        match sig.self_param {
            Some(ast::SelfParam::Ref(name, _)) => inputs.lifetime(name.map_or(LifetimeKey::SelfReference, |lt| LifetimeKey::Named(lt.lo, lt.hi))),
            Some(ast::SelfParam::Typed(_, ty)) => inputs.ty(ty),
            _ => {}
        }
        let mut output = collector(false);
        if let Some(ty) = sig.ret { output.ty(ty); }
        if sig.asyncness || output.has_fully_capturing_opaque {
            for declaration in &names.declarations {
                if declaration.binder == BinderId::Item { output.regions.push(declaration.id); }
            }
        }
        let mut where_clause = collector(false);
        where_clause.generics(&generics);
        for &ty in &apits {
            if let ast::Ty::ImplTrait(bounds) = ast.ty(ty) { where_clause.bounds(*bounds); }
        }
        for declaration in &names.declarations {
            if declaration.binder == BinderId::Item && !where_clause.regions.contains(&declaration.id) &&
                (inputs.regions.contains(&declaration.id) || !output.regions.contains(&declaration.id)) {
                late.push(declaration.id);
            }
        }
    }
    let mut indices = parent.map_or_else(GenericIndices::default, |p| p.generic_indices.clone());
    let mut early_indices = parent.map_or_else(HashMap::default, |p| p.early_indices.clone());
    let mut index = (indices.lifetimes.len() + indices.types_consts.len()) as u32;
    if def.kind == DefKind::Trait { indices.types_consts.push(index); index += 1; }
    for param in ast.list(generics.params) {
        match param.kind {
            GenericParamKind::Lifetime(_) => {
                let LifetimeRes::Param(id) = names.uses[&LifetimeKey::Named(param.name.lo, param.name.hi)] else { unreachable!() };
                if late.contains(&id) { continue; }
                early_indices.insert(id, index);
                indices.lifetimes.push(index);
            }
            GenericParamKind::Type(..) => indices.types_consts.push(index),
            GenericParamKind::Const(..) => { indices.types_consts.push(index); indices.consts.push(index); }
        }
        index += 1;
    }
    // ast_lowering/item.rs:1803-1819: generated lifetime declarations follow
    // written parameters; synthetic APIT parameters follow those lifetimes.
    for declaration in &names.declarations {
        if declaration.binder == BinderId::Item && declaration.name.is_none() && !late.contains(&declaration.id) {
            early_indices.insert(declaration.id, index);
            indices.lifetimes.push(index);
            index += 1;
        }
    }
    for _ in &apits { indices.types_consts.push(index); index += 1; }
    let mut bound_vars = Vec::new();
    for declaration in &names.declarations {
        if declaration.binder != BinderId::Item { continue; }
        let arg = if let Some(index) = late.iter().position(|id| *id == declaration.id) {
            ResolvedArg::LateBound(0, BoundVar(index as u32), declaration.id)
        } else { ResolvedArg::EarlyBound(declaration.id) };
        bound_vars.push((declaration.id, arg));
    }
    let mut late_bound_vars_map = HashMap::default();
    late_bound_vars_map.insert(BinderId::Item, late.iter().map(|&id| BoundVariableKind::Region(BoundRegionKind::Named(id))).collect());
    let inherited = names.inherited.iter().map(|d| d.id).collect();
    let mut visitor = BoundVarContext {
        tcx, prog, owner, file: def.file, module: tcx.item_module(prog, owner), capture_errors: Vec::new(),
        rbv: ResolveBoundVars { names, named_variable_map: Default::default(), late_bound_vars_map,
            opaque_captured_lifetimes: Default::default(), early_indices, generic_indices: indices },
        scopes: vec![Scope::Root { parent: inherited }, Scope::Binder { bound_vars, scope_type: BinderScopeType::Normal, id: BinderId::Item, apit: false }],
    };
    if function.is_none() { visitor.scopes.push(Scope::TraitRefBoundary); }
    visitor.visit_item(&apits);
    for captures in visitor.rbv.opaque_captured_lifetimes.values() {
        for (i, &(_, captured)) in captures.iter().enumerate() { visitor.rbv.early_indices.insert(captured, index + i as u32); }
    }
    visitor.rbv
}

impl BoundVarContext<'_> {
    fn visit_item(&mut self, apits: &[ast::TyId]) {
        let def = self.prog.def(self.owner);
        let ast = &self.prog.files[self.file as usize].ast;
        let generics = owner_generics(self.prog, self.owner);
        self.visit_generics(&generics);
        self.scopes.push(Scope::TraitRefBoundary);
        for &ty in apits {
            if let ast::Ty::ImplTrait(bounds) = *ast.ty(ty) {
                self.push_binder(BinderId::ArgumentImplTrait(ty.0), Vec::new(), BinderScopeType::Normal, true);
                self.visit_bounds(bounds);
                self.scopes.pop();
            }
        }
        self.scopes.pop();
        match ast.item(def.item).kind {
            ItemKind::Fn(sig, _, _) => {
                let sig = ast.fn_sig(sig);
                self.scopes.push(Scope::ObjectLifetimeDefault(Some(ResolvedArg::StaticLifetime)));
                match sig.self_param {
                    Some(ast::SelfParam::Ref(name, _)) => self.visit_lifetime(name.map_or(LifetimeKey::SelfReference, |lt| LifetimeKey::Named(lt.lo, lt.hi)), ast.item(def.item).lo),
                    Some(ast::SelfParam::Typed(_, ty)) => self.visit_ty(ty),
                    _ => {}
                }
                for param in ast.list(sig.params) { self.visit_ty(param.ty); }
                // (ast_lowering of a foreign fn ignores `async`, rejected by ast validation: no opaque)
                if let Some(&opaque) = sig.asyncness.then(|| self.prog.node_id_to_def_id.get(&(self.file, relax_front::def_collector::NodeId::AsyncReturn(def.item)))).flatten() {
                    self.visit_opaque_ty(opaque, Default::default(), sig.ret);
                } else if let Some(ty) = sig.ret { self.visit_ty(ty); }
                self.scopes.pop();
                if let Some(body) = self.prog.fn_body(self.file, def.item).expect("collected body") {
                    self.scopes.push(Scope::Body(self.owner));
                    for param in ast.list(sig.params) { self.visit_pat(param.pat); }
                    self.visit_block(body);
                    self.scopes.pop();
                }
            }
            ItemKind::Struct(r) => self.visit_variant_data(ast.row(r).data),
            ItemKind::Union(r) => self.visit_fields(ast.row(r).fields),
            ItemKind::Enum(r) => for variant in ast.list(ast.row(r).variants) {
                self.visit_variant_data(variant.data);
                if let Some(expr) = variant.disc { self.visit_anon_const(expr); }
            },
            ItemKind::TypeAlias(r) => {
                let ast::TypeAliasItem { bounds, ty, .. } = *ast.row(r);
                self.visit_bounds(bounds);
                if let Some(ty) = ty { self.visit_ty(ty); }
            }
            ItemKind::Trait(r) => self.visit_bounds(ast.row(r).supers),
            ItemKind::TraitAlias(r) => self.visit_bounds(ast.row(r).bounds),
            ItemKind::Impl(r) => {
                let ast::ImplItem { trait_, self_ty, .. } = *ast.row(r);
                if let Some(path) = trait_ { self.visit_path(&path); }
                self.visit_ty(self_ty);
            }
            ItemKind::Const(ty, _) => {
                if let Some(ty) = ty.get() { self.visit_ty(ty); }
                if let Some(expr) = self.prog.const_body(self.file, def.item).expect("collected constant") {
                    self.scopes.push(Scope::Body(self.owner)); self.visit_expr(expr); self.scopes.pop();
                }
            }
            ItemKind::Static(_, ty, _) => {
                self.visit_ty(ty);
                if let Some(expr) = self.prog.const_body(self.file, def.item).expect("collected static") {
                    self.scopes.push(Scope::Body(self.owner)); self.visit_expr(expr); self.scopes.pop();
                }
            }
            _ => {}
        }
    }

    fn visit_generics(&mut self, generics: &ast::Generics) {
        let ast = &self.prog.files[self.file as usize].ast;
        self.scopes.push(Scope::TraitRefBoundary);
        for (i, param) in ast.list(generics.params).iter().enumerate() {
            match param.kind {
                GenericParamKind::Lifetime(bounds) => {
                    if !bounds.is_empty() { self.named_lifetime(param.name); }
                    for &lt in ast.list(bounds) { self.named_lifetime(lt); }
                }
                GenericParamKind::Type(bounds, default) => {
                    self.push_binder(BinderId::GenericBound(generics.params.lo + i as u32), Vec::new(), BinderScopeType::Normal, false);
                    self.visit_bounds(bounds);
                    self.scopes.pop();
                    if let Some(ty) = default { self.visit_ty(ty); }
                }
                GenericParamKind::Const(ty, default) => {
                    self.visit_ty(ty);
                    if let Some(expr) = default { self.visit_anon_const(expr); }
                }
            }
        }
        for (i, pred) in ast.list(generics.where_).iter().enumerate() {
            match *pred {
                ast::WherePred::Lifetime(lt, bounds) => { self.named_lifetime(lt); for &lt in ast.list(bounds) { self.named_lifetime(lt); } }
                ast::WherePred::Bound { ty, bounds, .. } => {
                    self.push_binder(BinderId::Where(generics.where_.lo + i as u32), Vec::new(), BinderScopeType::Normal, false);
                    self.visit_ty(ty);
                    self.visit_bounds(bounds);
                    self.scopes.pop();
                }
            }
        }
        self.scopes.pop();
    }

    fn visit_bounds(&mut self, bounds: ast::List<ast::Bound>) {
        let ast = &self.prog.files[self.file as usize].ast;
        for bound in ast.list(bounds) {
            match *bound {
                ast::Bound::Lifetime(name) => self.named_lifetime(name),
                ast::Bound::Trait { path, hrtb, .. } => {
                    let (binders, scope_type) = self.poly_trait_ref_binder_info();
                    self.push_binder(BinderId::Trait(path.segs.lo), binders, scope_type, false);
                    for param in ast.list(hrtb) {
                        if !matches!(param.kind, GenericParamKind::Lifetime(_)) {
                            self.tcx.dcx.report(self.cause(param.name.lo), "non-lifetime binders are unstable".into());
                        }
                    }
                    self.visit_path(&path);
                    self.scopes.pop();
                }
                ast::Bound::Use(args) => {
                    if let Some(GenericArgs::Angle(arguments)) = ast.gargs(args) {
                        for arg in ast.list(*arguments) {
                            match *arg {
                                GenericArg::Lifetime(lt) => {
                                    match self.rbv.names.uses[&LifetimeKey::Named(lt.lo, lt.hi)] {
                                        LifetimeRes::Param(_) => self.named_lifetime(lt),
                                        LifetimeRes::Error => {},
                                        _ => self.tcx.dcx.report(self.cause(lt.lo), "precise capture must name a lifetime parameter".into()),
                                    }
                                }
                                GenericArg::Type(ty) => self.visit_precise_capturing_param(ty),
                                _ => self.tcx.dcx.report(self.cause(0), "precise capture must name a type or const parameter".into()),
                            }
                        }
                    }
                }
            }
        }
    }

    // resolve_bound_vars.rs:678-704 and 1671-1798. With non-lifetime HRTBs
    // feature-cut, type/const capture names can only resolve to early parameters;
    // the declaration environment supplies that same lexical parameter lookup.
    fn visit_precise_capturing_param(&self, ty: ast::TyId) {
        let ast = &self.prog.files[self.file as usize].ast;
        let ast::Ty::Path(path) = ast.ty(ty) else {
            self.tcx.dcx.report(self.cause(0), "precise capture must name a type or const parameter".into());
            return;
        };
        let segments = ast.segs(path.segs);
        let Some(segment) = segments.first() else { return; };
        let symbol = self.prog.syms.get(self.prog.files[self.file as usize].text(segment.name));
        let parameter = path.no_qself() && segments.len() == 1 && segment.args().is_none() && symbol.is_some_and(|name| self.tcx.lifetime_path_is_parameter(self.prog, self.owner, name));
        if segment.kind() == SegKind::SelfType && !parameter {
            self.tcx.dcx.report(self.cause(segment.name.lo), "precise capture cannot name the Self alias of an impl".into());
        } else if !parameter {
            self.tcx.dcx.report(self.cause(segment.name.lo), "precise capture must name a type or const parameter".into());
        }
    }

    fn visit_ty(&mut self, ty: ast::TyId) {
        let ast = &self.prog.files[self.file as usize].ast;
        match *ast.ty(ty) {
            ast::Ty::Ref(name, _, inner) => {
                let key = name.map_or(LifetimeKey::Reference(ty.0), |lt| LifetimeKey::Named(lt.lo, lt.hi));
                self.visit_lifetime(key, name.map_or(0, |lt| lt.lo));
                self.scopes.push(Scope::ObjectLifetimeDefault(self.mapped(key)));
                self.visit_ty(inner);
                self.scopes.pop();
            }
            ast::Ty::Ptr(_, inner) | ast::Ty::Slice(inner) | ast::Ty::Paren(inner) => self.visit_ty(inner),
            ast::Ty::Array(inner, length) => { self.visit_ty(inner); self.visit_anon_const(length); }
            ast::Ty::Tuple(types) => for &ty in ast.list(types) { self.visit_ty(ty); },
            ast::Ty::Fn(pointer) => {
                let pointer = ast.fn_ptr(pointer);
                self.push_binder(BinderId::FnPointer(ty.0), Vec::new(), BinderScopeType::Normal, false);
                for param in ast.list(pointer.hrtb) {
                    if !matches!(param.kind, GenericParamKind::Lifetime(_)) { self.tcx.dcx.report(self.cause(param.name.lo), "late-bound type or const parameter not allowed on function pointers".into()); }
                }
                // resolve_bound_vars.rs:720-749: a fn pointer adds only its binder; the
                // enclosing object lifetime default reaches its inputs and output.
                for &ty in ast.list(pointer.params) { self.visit_ty(ty); }
                if let Some(ty) = pointer.ret { self.visit_ty(ty); }
                self.scopes.pop();
            }
            ast::Ty::Path(path) => {
                let trait_object = self.tcx.lifetime_path_def(self.prog, self.owner, self.file, self.module, &path).is_some_and(|def| self.prog.def(def).kind == DefKind::Trait);
                if trait_object {
                    self.scopes.push(Scope::TraitRefBoundary);
                    self.push_binder(BinderId::Trait(path.segs.lo), Vec::new(), BinderScopeType::Normal, false);
                }
                self.visit_path(&path);
                if trait_object {
                    self.scopes.pop(); self.scopes.pop();
                    self.resolve_object_lifetime_default(LifetimeKey::ObjectDefault(ty.0));
                }
            }
            ast::Ty::DynTrait(bounds, _) => {
                self.scopes.push(Scope::TraitRefBoundary);
                self.visit_bounds(bounds);
                self.scopes.pop();
                if !ast.list(bounds).iter().any(|bound| matches!(bound, ast::Bound::Lifetime(_))) {
                    self.resolve_object_lifetime_default(LifetimeKey::ObjectDefault(ty.0));
                }
            }
            ast::Ty::ImplTrait(bounds) => {
                if let Some(&opaque) = self.prog.node_id_to_def_id.get(&(self.file, relax_front::def_collector::NodeId::OpaqueTy(ty))) {
                    self.visit_opaque_ty(opaque, bounds, None);
                }
            }
            _ => {}
        }
    }

    fn visit_variant_data(&mut self, data: VariantData) {
        match data { VariantData::Tuple(fields) | VariantData::Struct(fields) => self.visit_fields(fields), VariantData::Unit => {} }
    }
    fn visit_fields(&mut self, fields: ast::List<ast::FieldDef>) {
        for field in self.prog.files[self.file as usize].ast.list(fields) {
            self.visit_ty(field.ty);
            if let Some(expr) = field.default { self.visit_anon_const(expr); }
        }
    }
    fn visit_anon_const(&mut self, expr: ast::ExprId) {
        self.scopes.push(Scope::LateBoundary { what: "constant", deny_late_regions: true });
        let owner = self.prog.node_id_to_def_id[&(self.file, relax_front::def_collector::NodeId::AnonConst(expr))];
        self.scopes.push(Scope::Body(owner));
        self.visit_expr(expr);
        self.scopes.pop(); self.scopes.pop();
    }
}

impl BoundVarContext<'_> {
    fn visit_block(&mut self, block: ast::BlockId) {
        let outer_module = self.module;
        if let Some(&module) = self.prog.block_mods.get(&(self.file, block.0)) { self.module = module; }
        let ast = &self.prog.files[self.file as usize].ast;
        for stmt in ast.list(ast.block(block).stmts) {
            match *stmt {
                ast::Stmt::Let(r) => {
                    let ast::LetStmt { pat, ty, init, else_, .. } = *ast.row(r);
                    self.visit_pat(pat);
                    if let Some(ty) = ty { self.visit_ty(ty); }
                    if let Some(expr) = init { self.visit_expr(expr); }
                    if let Some(block) = else_ { self.visit_block(block); }
                }
                ast::Stmt::Expr(expr, _) => self.visit_expr(expr),
                ast::Stmt::Attrs(r) => self.visit_expr(ast.row(r).expr),
                ast::Stmt::Item(_) | ast::Stmt::Empty => {}
            }
        }
        self.module = outer_module;
    }

    fn visit_pat(&mut self, p: ast::PatId) {
        match *self.prog.files[self.file as usize].ast.pat(p) {
            ast::Pat::Ident { sub: Some(p), .. } | ast::Pat::Ref(_, p) | ast::Pat::Paren(p) | ast::Pat::Box(p) => self.visit_pat(p),
            ast::Pat::Lit(e) => self.visit_expr(e),
            ast::Pat::Range(a, b, _) => { if let Some(e) = a { self.visit_expr(e); } if let Some(e) = b { self.visit_expr(e); } }
            ast::Pat::Path(p) => self.visit_path(&p),
            ast::Pat::TupleStruct(p, ps) => { self.visit_path(&p); for p in self.prog.files[self.file as usize].ast.list(ps) { self.visit_pat(*p); } }
            ast::Pat::Struct(p, fs, _) => { self.visit_path(&p); for f in self.prog.files[self.file as usize].ast.list(fs) { self.visit_pat(f.pat); } }
            ast::Pat::Tuple(ps) | ast::Pat::Slice(ps) | ast::Pat::Or(ps) => for p in self.prog.files[self.file as usize].ast.list(ps) { self.visit_pat(*p); },
            ast::Pat::Mac(m) => { let m = *self.prog.files[self.file as usize].ast.row(m); self.visit_mac(m) }
            ast::Pat::Wild | ast::Pat::Rest | ast::Pat::Ident { sub: None, .. } => {}
        }
    }
    fn visit_expr(&mut self, e: ast::ExprId) {
        match self.prog.files[self.file as usize].ast.expr(e).kind {
            ast::ExprKind::Repeat(v, n) => { self.visit_expr(v); self.visit_anon_const(n); }
            ast::ExprKind::ConstBlock(b) => {
                let owner = self.prog.node_id_to_def_id[&(self.file, relax_front::def_collector::NodeId::InlineConst(e))];
                self.scopes.push(Scope::Body(owner));
                self.visit_block(b);
                self.scopes.pop();
            }
            ast::ExprKind::Closure(row) => {
                let ast::ClosureRow { params, ret, body, .. } = *self.prog.files[self.file as usize].ast.row(row);
                self.push_binder(BinderId::Closure(e.0), Vec::new(), BinderScopeType::Normal, false);
                self.scopes.push(Scope::ObjectLifetimeDefault(Some(ResolvedArg::StaticLifetime)));
                for p in self.prog.files[self.file as usize].ast.list(params) { if let Some(t) = p.ty { self.visit_ty(t); } }
                self.scopes.pop();
                if let Some(t) = ret { self.visit_ty(t); }
                let owner = self.prog.node_id_to_def_id[&(self.file, relax_front::def_collector::NodeId::Closure(e))];
                self.scopes.push(Scope::Body(owner));
                for p in self.prog.files[self.file as usize].ast.list(params) { self.visit_pat(p.pat); }
                self.visit_expr(body); self.scopes.pop(); self.scopes.pop();
            }
            ast::ExprKind::Async(_, b) => {
                self.push_binder(BinderId::Closure(e.0), Vec::new(), BinderScopeType::Normal, false);
                let owner = self.prog.node_id_to_def_id[&(self.file, relax_front::def_collector::NodeId::Closure(e))];
                self.scopes.push(Scope::Body(owner));
                self.visit_block(b); self.scopes.pop(); self.scopes.pop();
            }
            ast::ExprKind::Path(p) => self.visit_path(&p),
            ast::ExprKind::Unary(_, x) | ast::ExprKind::Field(x, _) | ast::ExprKind::TupleField(x, _) |
            ast::ExprKind::AddrOf(_, _, x) | ast::ExprKind::Try(x) | ast::ExprKind::Await(x) | ast::ExprKind::Paren(x) | ast::ExprKind::Box(x) => self.visit_expr(x),
            ast::ExprKind::Binary(_, a, b) | ast::ExprKind::Assign(a, b) | ast::ExprKind::AssignOp(_, a, b) | ast::ExprKind::Index(a, b) => { self.visit_expr(a); self.visit_expr(b); }
            ast::ExprKind::Call(f, args) => { self.visit_expr(f); for e in self.prog.files[self.file as usize].ast.list(args) { self.visit_expr(*e); } }
            ast::ExprKind::MethodCall(recv, row) => {
                let ast::MethodCallRow { turbofish, args, .. } = *self.prog.files[self.file as usize].ast.row(row);
                self.visit_expr(recv); self.visit_constraint_args(turbofish); for e in self.prog.files[self.file as usize].ast.list(args) { self.visit_expr(*e); }
            }
            ast::ExprKind::Cast(e, t) => { self.visit_expr(e); self.visit_ty(t); }
            ast::ExprKind::Block(b, _) | ast::ExprKind::Unsafe(b) | ast::ExprKind::Loop(b, _) => self.visit_block(b),
            ast::ExprKind::If(e, b, other) => { self.visit_expr(e); self.visit_block(b); if let Some(e) = other.get() { self.visit_expr(e); } }
            ast::ExprKind::Let(p, e) => { self.visit_pat(p); self.visit_expr(e); }
            ast::ExprKind::Match(e, arms) => {
                self.visit_expr(e); for a in self.prog.files[self.file as usize].ast.list(arms) { self.visit_pat(a.pat); if let Some(e) = a.guard { self.visit_expr(e); } self.visit_expr(a.body); }
            }
            ast::ExprKind::While(row) => { let ast::WhileRow { cond: e, body: b, .. } = *self.prog.files[self.file as usize].ast.row(row); self.visit_expr(e); self.visit_block(b); }
            ast::ExprKind::For(row) => { let ast::ForRow { pat: p, iter: e, body: b, .. } = *self.prog.files[self.file as usize].ast.row(row); self.visit_pat(p); self.visit_expr(e); self.visit_block(b); }
            ast::ExprKind::Break(_, v) | ast::ExprKind::Return(v) | ast::ExprKind::Yield(v) => if let Some(e) = v.get() { self.visit_expr(e); },
            ast::ExprKind::Tuple(es) | ast::ExprKind::Array(es) => for e in self.prog.files[self.file as usize].ast.list(es) { self.visit_expr(*e); },
            ast::ExprKind::Struct(row) => {
                let ast::StructRow { path: p, fields: fs, base, .. } = *self.prog.files[self.file as usize].ast.row(row);
                self.visit_path(&p); for f in self.prog.files[self.file as usize].ast.list(fs) { self.visit_expr(f.expr); }
                if let Some(e) = base.get() { self.visit_expr(e); }
            }
            ast::ExprKind::Range(a, b, _) => { if let Some(e) = a.get() { self.visit_expr(e); } if let Some(e) = b.get() { self.visit_expr(e); } }
            ast::ExprKind::Mac(m) => { let m = *self.prog.files[self.file as usize].ast.row(m); self.visit_mac(m) }
            ast::ExprKind::Lit(_) | ast::ExprKind::Continue(_) | ast::ExprKind::Underscore => {}
        }
    }
    fn visit_mac(&mut self, m: ast::MacCall) {
        if m.args == ast::NO_MAC_ARGS { return; }
        let args = *self.prog.files[self.file as usize].ast.mac_arg(m.args);
        for e in self.prog.files[self.file as usize].ast.list(args.exprs) { self.visit_expr(*e); }
        if let Some(p) = args.pat { self.visit_pat(p); } if let Some(e) = args.guard { self.visit_expr(e); }
        if let Some(asm) = args.asm {
            for op in self.prog.files[self.file as usize].ast.list(asm.ops) {
                if let Some(e) = op.input { if op.dir == ast::AsmDir::Const { self.visit_anon_const(e); } else { self.visit_expr(e); } }
                if let Some(e) = op.output { self.visit_expr(e); }
            }
        }
    }
}

impl BoundVarContext<'_> {
    /// HIR's QPath::TypeRelative is a path whose resolution stops at a type (`<T>::A`, `T::A`,
    /// `Self::A`, `S::new`): its unresolved segments' arguments are walked without a scope of
    /// their own (rustc's visit_segment_args only runs for resolved paths), so they see the
    /// enclosing object lifetime default; the resolved prefix's own containers still apply.
    fn resolved_prefix_len(&self, path: &ast::Path) -> usize {
        let ast = &self.prog.files[self.file as usize].ast;
        let segs = ast.segs(path.segs);
        if let Some(qself) = ast.qself(path) { return if qself.trait_path.is_none() { 0 } else { segs.len() }; }
        if segs.len() < 2 || path.global() { return segs.len(); }
        let first = &segs[0];
        let first_is_param = first.kind() == SegKind::SelfType || (first.kind() == SegKind::Ident
            && self.prog.syms.get(self.prog.files[self.file as usize].text(first.name)).is_some_and(|name| self.tcx.lifetime_path_is_parameter(self.prog, self.owner, name)));
        if first_is_param { return 1; }
        if self.tcx.lifetime_path_def(self.prog, self.owner, self.file, self.module, path).is_some() { return segs.len(); }
        (1..segs.len()).rev().find(|&k| {
            let prefix = ast::Path { segs: path.segs.prefix(k), ..*path };
            self.tcx.lifetime_path_def(self.prog, self.owner, self.file, self.module, &prefix)
                .is_some_and(|def| matches!(self.prog.def(def).kind, DefKind::Struct | DefKind::Union | DefKind::Enum | DefKind::TypeAlias))
        }).unwrap_or(segs.len())
    }

    fn visit_path(&mut self, path: &ast::Path) {
        let ast = &self.prog.files[self.file as usize].ast;
        let resolved_len = self.resolved_prefix_len(path);
        let resolved = ast::Path { segs: path.segs.prefix(resolved_len), ..*path };
        if let Some(qself) = ast.qself(path) {
            if let Some(trait_path) = qself.trait_path { self.visit_path(&trait_path); }
        }
        for (i, segment) in ast.segs(path.segs).iter().enumerate() {
            if let Some(GenericArgs::Paren(inputs, output)) = ast.gargs(segment.args()) {
                self.scopes.push(Scope::ObjectLifetimeDefault(Some(ResolvedArg::StaticLifetime)));
                for &ty in ast.list(*inputs) { self.visit_ty(ty); }
                if let Some(ty) = output { self.visit_ty(*ty); }
                self.scopes.pop();
                continue;
            }
            let segment_id = path.segs.lo + i as u32;
            let arguments = match ast.gargs(segment.args()) { Some(GenericArgs::Angle(arguments)) => ast.list(*arguments), _ => &[] };
            let mut lifetimes = Vec::new();
            for argument in arguments {
                if let GenericArg::Lifetime(name) = *argument {
                    self.named_lifetime(name);
                    lifetimes.push(self.mapped(LifetimeKey::Named(name.lo, name.hi)));
                }
            }
            let mut j = 0;
            while self.rbv.names.uses.contains_key(&LifetimeKey::Path(segment_id, j)) {
                let key = LifetimeKey::Path(segment_id, j);
                self.visit_lifetime(key, segment.name.lo);
                lifetimes.push(self.mapped(key));
                j += 1;
            }
            let prefix = ast::Path { segs: path.segs.prefix(i + 1), ..*path };
            let container = if i < resolved_len { self.eligible_container(&resolved, i) } else { None };
            let defaults = container.map(|def| self.compute_object_lifetime_defaults(def, &prefix)).unwrap_or_default();
            let mut index = usize::from(container.is_some_and(|def| self.prog.def(def).kind == DefKind::Trait));
            for argument in arguments {
                match *argument {
                    GenericArg::Lifetime(_) => {}
                    GenericArg::Type(ty) => {
                        if let Some(default) = defaults.get(index) {
                            self.scopes.push(Scope::ObjectLifetimeDefault(*default));
                            self.visit_ty(ty);
                            self.scopes.pop();
                        } else { self.visit_ty(ty); }
                        index += 1;
                    }
                    GenericArg::Const(expr) => { self.visit_anon_const(expr); index += 1; }
                    GenericArg::Binding(..) | GenericArg::Constraint(..) => {}
                }
            }
            for argument in arguments {
                let (name, own_args, ty, bounds) = match *argument {
                    GenericArg::Binding(name, args, ty) => (name, args, Some(ty), None),
                    GenericArg::Constraint(name, args, bounds) => (name, args, None, Some(bounds)),
                    _ => continue,
                };
                let own_has_lifetimes = matches!(ast.gargs(own_args), Some(GenericArgs::Angle(args)) if ast.list(*args).iter().any(|a| matches!(a, GenericArg::Lifetime(_))));
                self.scopes.push(Scope::ObjectLifetimeDefault(if !lifetimes.is_empty() || own_has_lifetimes { None } else { Some(ResolvedArg::StaticLifetime) }));
                let supertrait_vars = container.map(|def| self.supertrait_hrtb_vars(def, name)).unwrap_or_default();
                self.scopes.push(Scope::Supertrait(supertrait_vars));
                self.visit_constraint_args(own_args);
                if let Some(ty) = ty { self.visit_ty(ty); }
                if let Some(bounds) = bounds { self.visit_bounds(bounds); }
                self.scopes.pop(); self.scopes.pop();
            }
        }
        if let Some(qself) = ast.qself(path) {
            let default = if let Some(trait_path) = qself.trait_path {
                let td = self.tcx.resolve_type_path_def(self.prog, self.file, self.module, &trait_path);
                td.and_then(|td| self.compute_object_lifetime_defaults(td, &trait_path).first().copied()).flatten()
            } else { None };
            self.scopes.push(Scope::ObjectLifetimeDefault(default));
            self.visit_ty(qself.ty);
            self.scopes.pop();
        }
    }

    /// resolve_bound_vars.rs:2000-2094: only the resolved declaration's
    /// eligible segments supply object defaults. Variant parameters belong to
    /// their enum even when written on the variant segment.
    fn eligible_container(&self, path: &ast::Path, segment: usize) -> Option<DefId> {
        let def = self.tcx.lifetime_path_def(self.prog, self.owner, self.file, self.module, path)?;
        let reverse = path.segs.len() - segment - 1;
        match self.prog.def(def).kind {
            DefKind::AssocTy => match reverse {
                0 => Some(def),
                1 => Some(self.prog.def(def).parent),
                _ => None,
            },
            DefKind::Variant => match reverse {
                0 | 1 => Some(self.prog.def(def).parent),
                _ => None,
            },
            DefKind::Enum | DefKind::Struct | DefKind::Trait | DefKind::TypeAlias | DefKind::Union if reverse == 0 => Some(def),
            _ => None,
        }
    }

    fn visit_constraint_args(&mut self, args: ast::GenericArgsId) {
        let ast = &self.prog.files[self.file as usize].ast;
        if let Some(GenericArgs::Angle(arguments)) = ast.gargs(args) {
            for argument in ast.list(*arguments) {
                match *argument {
                    GenericArg::Lifetime(name) => self.named_lifetime(name),
                    GenericArg::Type(ty) => self.visit_ty(ty),
                    GenericArg::Const(expr) => self.visit_anon_const(expr),
                    GenericArg::Binding(_, args, ty) => { self.visit_constraint_args(args); self.visit_ty(ty); }
                    GenericArg::Constraint(_, args, bounds) => { self.visit_constraint_args(args); self.visit_bounds(bounds); }
                }
            }
        }
    }

    /// resolve_bound_vars.rs:1181-1234,2094-2190: defaults are declaration
    /// bounds, not trait selection. Multiple different lifetime bounds remain
    /// indeterminate; an empty set is static outside a body and inferred in one.
    fn compute_object_lifetime_defaults(&self, container: DefId, path: &ast::Path) -> Vec<Option<ResolvedArg>> {
        let def = self.prog.def(container);
        let file = &self.prog.files[def.file as usize];
        let generics = owner_generics(self.prog, container);
        let in_body = self.scopes.iter().any(|scope| matches!(scope, Scope::Body(_)));
        let mut parameters = Vec::new();
        if def.kind == DefKind::Trait {
            let bounds = match file.ast.as_trait(&file.ast.item(def.item).kind) { Some(t) => t.supers, None => Default::default() };
            parameters.push(("Self".to_owned(), bounds));
        }
        for param in file.ast.list(generics.params) {
            match param.kind {
                GenericParamKind::Type(bounds, _) => parameters.push((file.text(param.name).to_owned(), bounds)),
                GenericParamKind::Const(..) => parameters.push((String::new(), Default::default())),
                _ => {}
            }
        }
        let names = self.tcx.resolve_lifetime_names(self.prog, container);
        let mut result = Vec::new();
        for (name, bounds) in parameters {
            let mut lifetimes = Vec::new();
            let mut add = |bounds| {
                for bound in file.ast.list(bounds) {
                    if let ast::Bound::Lifetime(lt) = bound {
                        let res = names.uses[&LifetimeKey::Named(lt.lo, lt.hi)];
                        if !lifetimes.contains(&res) { lifetimes.push(res); }
                    }
                }
            };
            add(bounds);
            for pred in file.ast.list(generics.where_) {
                if let ast::WherePred::Bound { hrtb, ty, bounds } = *pred {
                    if !hrtb.is_empty() { continue; }
                    if let ast::Ty::Path(path) = file.ast.ty(ty) {
                        if path.no_qself() && path.segs.len() == 1 && file.text(file.ast.segs(path.segs)[0].name) == name { add(bounds); }
                    }
                }
            }
            let default = match lifetimes.as_slice() {
                [] => if in_body { None } else { Some(ResolvedArg::StaticLifetime) },
                [LifetimeRes::Static] => Some(ResolvedArg::StaticLifetime),
                [LifetimeRes::Param(id)] => self.object_lifetime_argument(container, *id, path),
                _ => None,
            };
            result.push(default);
        }
        result
    }

    // resolve_bound_vars.rs:2136-2169: a parameter belonging to an ancestor
    // declaration uses the matching earlier segment's argument list.
    fn object_lifetime_argument(&self, mut container: DefId, id: LifetimeId, path: &ast::Path) -> Option<ResolvedArg> {
        let mut reverse_segment = 0;
        loop {
            let names = self.tcx.resolve_lifetime_names(self.prog, container);
            let generics = owner_generics(self.prog, container);
            let def = self.prog.def(container);
            let file = &self.prog.files[def.file as usize];
            let mut lifetime_index = 0;
            for param in file.ast.list(generics.params) {
                if !matches!(param.kind, GenericParamKind::Lifetime(_)) { continue; }
                if names.uses.get(&LifetimeKey::Named(param.name.lo, param.name.hi)) == Some(&LifetimeRes::Param(id)) {
                    let ast = &self.prog.files[self.file as usize].ast;
                    let (segments, reverse_index) = if reverse_segment < path.segs.len() {
                        (path.segs, reverse_segment)
                    } else {
                        (ast.qself(path)?.trait_path?.segs, reverse_segment - path.segs.len())
                    };
                    let index = segments.len().checked_sub(reverse_index + 1)?;
                    let segment = ast.segs(segments)[index];
                    if let Some(GenericArgs::Angle(args)) = ast.gargs(segment.args()) {
                        let mut ordinal = 0;
                        for arg in ast.list(*args) {
                            if let GenericArg::Lifetime(lt) = arg {
                                if ordinal == lifetime_index { return self.mapped(LifetimeKey::Named(lt.lo, lt.hi)); }
                                ordinal += 1;
                            }
                        }
                    }
                    return self.mapped(LifetimeKey::Path(segments.lo + index as u32, lifetime_index));
                }
                lifetime_index += 1;
            }
            if !matches!(def.kind, DefKind::AssocTy | DefKind::AssocFn | DefKind::AssocConst) { return None; }
            container = def.parent;
            reverse_segment += 1;
        }
    }

    /// resolve_bound_vars.rs:2204-2260. The explicit-supertrait traversal retains
    /// each predicate's named binder inputs from the immutable owner table.
    fn supertrait_hrtb_vars(&self, container: DefId, name: ast::Ident) -> Vec<BoundVariableKind> {
        let symbol = self.prog.syms.get(self.prog.files[self.file as usize].text(name));
        let Some(symbol) = symbol else { return Vec::new(); };
        let mut stack = vec![(container, Vec::new())];
        let mut visited = crate::fxhash::HashSet::default();
        while let Some((def, vars)) = stack.pop() {
            let d = self.prog.def(def);
            if !matches!(d.kind, DefKind::Trait | DefKind::Impl) { break; }
            if d.kind == DefKind::Trait {
                for &item in &self.prog.traits[d.sub as usize] {
                    let item = self.prog.def(item);
                    if item.kind == DefKind::AssocTy && item.name == symbol { return vars; }
                }
            }
            for predicate in self.tcx.explicit_supertraits_containing_assoc_item(self.prog, def, symbol) {
                if let crate::next_solver::PredicateKind::Clause(crate::next_solver::ClauseKind::Trait(clause)) = predicate.kind {
                    let super_def = clause.trait_ref.def_id;
                    if visited.insert(super_def) {
                        let mut next = vars.clone();
                        next.extend_from_slice(&predicate.bound_vars);
                        stack.push((super_def, next));
                    }
                }
            }
        }
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn source(text: &str, edition: u16) -> (Program, Tcx) {
        let path = std::env::temp_dir().join(format!("relax-r32-lifetimes-{}-{}.rs", std::process::id(), crate::conc::thread_id()));
        std::fs::write(&path, text).unwrap();
        let mut prog = Program::new(&std::sync::Arc::new(crate::Session::default()));
        let cfg = crate::cfg::CfgSet::host(&prog.session, &[]);
        prog.load_crate("probe", path.to_str().unwrap(), edition, cfg, vec![]).unwrap();
        let mut tcx = Tcx::new(); tcx.collect(&prog);  tcx.publish_collected();
        std::fs::remove_file(path).unwrap();
        (prog, tcx)
    }
    fn named(prog: &Program, name: &str) -> DefId {
        (0..prog.defs.len()).map(|i| DefId(i as u32)).find(|d| prog.name(*d) == name).unwrap()
    }
    #[test]
    fn round49_file_body_inputs_recompute_siblings_and_preserve_other_files() {
        // Whole-owner query results contain file-relative node ids, so growing
        // f must invalidate g as well (resolve_bound_vars.rs:253-285 @8d1a764).
        let text = "#![no_std] fn f() { let _x = 1u8; } fn g() { let _y = 2u8; }";
        let (mut prog, _) = source(text, 2021);
        let path = std::env::temp_dir().join(format!("relax-r49-other-{}.rs", std::process::id()));
        std::fs::write(&path, "#![no_std] fn h() { let _z = 3u8; }").unwrap();
        let cfg = crate::cfg::CfgSet::host(&prog.session, &[]);
        prog.load_crate("other", path.to_str().unwrap(), 2021, cfg, vec![]).unwrap();
        std::fs::remove_file(path).unwrap();
        let mut tcx = Tcx::new(); tcx.collect(&prog);
        let owners = [named(&prog, "f"), named(&prog, "g"), named(&prog, "h")];
        let before = owners.map(|d| tcx.typeck(&prog, d));
        for b in &before { assert!(b.errors.is_empty(), "{:?}", b.errors); }
        let declarations = tcx.queries.read_declarations().unwrap();
        let old = prog.def(owners[0]).file;
        let edited = text.replace("let _x = 1u8;", "let _longer_name = 1u8; let _more = 4u8;");
        let version = prog.parse_version(old, "round49.rs", edited.into_bytes()).unwrap();
        let pairs: Vec<_> = (0..version.pf.ast.items.len() as u32).map(|i| (i, i)).collect();
        let new = prog.commit_body_version(version, &pairs).unwrap();
        prog.lower_body_declarations(&[owners[0]], &[new]);
        tcx.publish_file_bodies(&[old, new]);
        let after = owners.map(|d| tcx.typeck(&prog, d));
        let mut recomputed = 0;
        for i in 0..owners.len() {
            assert!(after[i].errors.is_empty(), "{:?}", after[i].errors);
            if !std::sync::Arc::ptr_eq(&before[i], &after[i]) { recomputed += 1; }
        }
        assert_eq!(recomputed, 2);
        assert_eq!(after[0].file, new);
        assert_eq!(after[1].file, new);
        assert!(std::sync::Arc::ptr_eq(&before[2], &after[2]));
        assert_eq!(tcx.queries.read_declarations().unwrap(), declarations);
        prog.revert_version(old, new, &pairs);
        tcx.publish_file_bodies(&[old, new]);
        for i in 0..2 {
            let restored = tcx.typeck(&prog, owners[i]);
            assert_eq!(restored.file, old);
            assert!(restored.errors.is_empty(), "{:?}", restored.errors);
        }
        assert!(std::sync::Arc::ptr_eq(&before[2], &tcx.typeck(&prog, owners[2])));
        eprintln!("round49 file inputs: 2 edited-file owners recomputed; 1 other-file owner reused; revert correct");
    }

    #[test]
    fn round46_body_edit_changes_bound_variable_resolution() {
        // rustc_hir_analysis/src/collect/resolve_bound_vars.rs:430-442:
        // owner resolution includes the body, including local type binders.
        let text = "#![no_std] fn f() { let _: fn(&u8); }";
        let (mut prog, mut tcx) = source(text, 2021);
        let owner = named(&prog, "f");
        let old = prog.def(owner).file;
        let before = tcx.resolve_bound_vars(&prog, owner);
        assert!(!before.names.declarations.iter().any(|d| d.name.as_deref() == Some("'fresh")));
        let edited = text.replace("fn(&u8)", "for<'fresh> fn(&'fresh u8)");
        let version = prog.parse_version(old, "round46.rs", edited.into_bytes()).unwrap();
        let pairs: Vec<_> = (0..version.pf.ast.items.len() as u32).map(|i| (i, i)).collect();
        let new = prog.commit_body_version(version, &pairs).unwrap();
        prog.lower_body_declarations(&[owner], &[new]);
        tcx.publish_file_bodies(&[old, new]);
        let after = tcx.resolve_bound_vars(&prog, owner);
        assert!(after.names.declarations.iter().any(|d| d.name.as_deref() == Some("'fresh")),
            "body-only edit reused stale lifetime resolution");
        let fresh = after.names.declarations.iter().find(|d| d.name.as_deref() == Some("'fresh")).unwrap();
        assert!(after.late_bound_vars_map[&fresh.binder].iter().any(|v|
            matches!(v, BoundVariableKind::Region(BoundRegionKind::Named(id)) if *id == fresh.id)));
        prog.revert_version(old, new, &pairs);
        tcx.publish_file_bodies(&[old, new]);
        let restored = tcx.resolve_bound_vars(&prog, owner);
        assert!(!restored.names.declarations.iter().any(|d| d.name.as_deref() == Some("'fresh")));
    }

    #[test]
    fn round32_lifetime_queries_precede_signature_and_invalidate() {
        let (prog, mut tcx) = source("#![no_std] trait Bound<'a> {} fn f<'a: 'a>(x: &'a u8) -> impl Bound<'a> { loop {} }", 2021);
        let owner = named(&prog, "f");
        assert!(tcx.sigs[&owner].get().is_none());
        let opaque = prog.opaque_types_defined_by[&owner][0];
        let before = tcx.resolve_bound_vars(&prog, owner);
        let generics = tcx.opaque_generics(&prog, opaque);
        let bounds = tcx.explicit_item_bounds(&prog, opaque).to_vec();
        let variances = tcx.variances_of(&prog, opaque);
        assert!(tcx.sigs[&owner].get().is_none());
        assert_eq!(generics.captured_lifetimes.len(), 1);
        assert_eq!(variances, vec![crate::types::variance::V::Bi, crate::types::variance::V::Inv]);
        tcx.fn_sig(&prog, owner).unwrap();
        assert_eq!(generics.captured_lifetimes, tcx.opaque_generics(&prog, opaque).captured_lifetimes);
        assert_eq!(bounds, tcx.explicit_item_bounds(&prog, opaque));
        assert_eq!(variances, tcx.variances_of(&prog, opaque));
        assert!(std::sync::Arc::ptr_eq(&before, &tcx.resolve_bound_vars(&prog, owner)));
        tcx.publish_collected();
        let after = tcx.resolve_bound_vars(&prog, owner);
        assert!(!std::sync::Arc::ptr_eq(&before, &after));
        assert_eq!(before.opaque_captured_lifetimes, after.opaque_captured_lifetimes);
        assert_eq!(bounds, tcx.explicit_item_bounds(&prog, opaque));
        assert_eq!(tcx.dcx.err_count(), 0);
    }
    #[test]
    fn round32_lifetime_async_and_precise_capture_editions() {
        for edition in [2021, 2024] {
            let (prog, tcx) = source("#![no_std] trait Bound {} #[lang = \"future_trait\"] trait Future { #[lang = \"future_output\"] type Output; } async fn asynchronous(x: &u8) {} fn precise<'a>(x: &'a u8) -> impl Bound + use<> { loop {} } fn implicit<'a>(x: &'a u8) -> impl Bound { loop {} }", edition);
            for (name, count) in [("asynchronous", 1), ("precise", 0), ("implicit", usize::from(edition >= 2024))] {
                let owner = named(&prog, name);
                let opaque = prog.opaque_types_defined_by[&owner][0];
                assert_eq!(tcx.opaque_generics(&prog, opaque).captured_lifetimes.len(), count, "{name} {edition}");
                tcx.explicit_item_bounds(&prog, opaque);
                tcx.fn_sig(&prog, owner).unwrap();
            }
            assert_eq!(tcx.dcx.err_count(), 0);
        }
    }
    #[test]
    fn round32_lifetime_supertrait_binder_order() {
        let (prog, tcx) = source("#![no_std] trait Sup<'a> { type Assoc; } trait Mid: for<'b> Sup<'b> {} trait Sub: Mid {} trait Inner<'a> {} fn f() -> impl Sub<Assoc: for<'c> Inner<'c>> { loop {} }", 2021);
        let owner = named(&prog, "f");
        let table = tcx.resolve_bound_vars(&prog, owner);
        let mut found = false;
        for (binder, vars) in &table.late_bound_vars_map {
            if !matches!(binder, BinderId::Trait(_)) || vars.len() != 2 { continue; }
            let BoundVariableKind::Region(BoundRegionKind::Named(inherited)) = vars[0] else { panic!(); };
            let BoundVariableKind::Region(BoundRegionKind::Named(own)) = vars[1] else { panic!(); };
            assert_eq!(inherited.owner, named(&prog, "Mid"));
            assert_eq!(own.owner, owner);
            found = true;
        }
        assert!(found);
        assert_eq!(tcx.dcx.err_count(), 0);
    }
    #[test]
    fn round32_lifetime_qualified_object_default_uses_trait_argument() {
        let (prog, tcx) = source("#![no_std] trait Bound {} trait Tr<'a>: 'a { type Out; } type Alias<'a> = <dyn Bound as Tr<'a>>::Out;", 2021);
        let owner = named(&prog, "Alias");
        let table = tcx.resolve_bound_vars(&prog, owner);
        let mut found = false;
        for (key, arg) in &table.named_variable_map {
            if matches!(key, LifetimeKey::ObjectDefault(_)) {
                assert_eq!(table.region_for_arg(*arg), RegionKind::ReEarlyParam(0));
                found = true;
            }
        }
        assert!(found);
        assert_eq!(tcx.dcx.err_count(), 0);
    }
    #[test]
    fn round32_lifetime_qualified_gat_default_uses_parent_argument() {
        let (prog, tcx) = source("#![no_std] trait Bound {} trait Tr<'a> { type Out<T: ?Sized + 'a>; } type Alias<'a, T: Tr<'a>> = <T as Tr<'a>>::Out<dyn Bound>;", 2021);
        let table = tcx.resolve_bound_vars(&prog, named(&prog, "Alias"));
        assert!(table.named_variable_map.iter().any(|(key, arg)| matches!(key, LifetimeKey::ObjectDefault(_)) && table.region_for_arg(*arg) == RegionKind::ReEarlyParam(0)));
        assert_eq!(tcx.dcx.err_count(), 0);
    }
    #[test]
    fn round32_lifetime_default_rpitit_environment_has_no_inference_regions() {
        let (prog, tcx) = source("#![no_std] trait Copy {} trait Source<'a> { fn get<'b, T: 'b>(&'b self, x: &'b T) -> impl Copy + use<'a, 'b, Self, T> { x } }", 2021);
        let owner = named(&prog, "get");
        let env = tcx.param_env(&prog, owner);
        struct NoRegionVars;
        impl crate::types::fold::TypeFolder for NoRegionVars {
            fn fold_region(&mut self, tys: &Types, region: RegionId, _: u32) -> RegionId {
                assert!(!matches!(tys.region_kind(region), RegionKind::ReVar(_)), "declaration contains {:?}", tys.region_kind(region));
                region
            }
        }
        for predicate in &*env.clauses { crate::next_solver::fold::fold_predicate(&tcx.tys, predicate, &mut NoRegionVars); }
        assert_eq!(tcx.dcx.err_count(), 0);
    }
    #[test]
    fn round32_lifetime_variant_segment_defaults() {
        let (prog, tcx) = source("#![no_std] trait Bound {} enum Enum<'a, T: ?Sized + 'a> { Variant { value: &'a T } } fn f<'a>() { let _ = Enum::Variant::<'a, dyn Bound> { value: loop {} }; let _ = Enum::<'a, dyn Bound>::Variant { value: loop {} }; }", 2021);
        let owner = named(&prog, "f");
        let table = tcx.resolve_bound_vars(&prog, owner);
        let defaults: Vec<_> = table.named_variable_map.iter().filter(|(key, _)| matches!(key, LifetimeKey::ObjectDefault(_))).collect();
        assert_eq!(defaults.len(), 2);
        assert!(defaults.iter().all(|(_, arg)| matches!(arg, ResolvedArg::Free(def, _) if *def == owner)));
        assert_eq!(tcx.dcx.err_count(), 0);
    }
    #[test]
    fn round32_lifetime_type_parameter_shadows_nominal_type() {
        let (prog, tcx) = source("#![no_std] struct Shadow<'a>(&'a u8); fn f<Shadow>(x: Shadow) {}", 2021);
        let owner = named(&prog, "f");
        assert_eq!(tcx.fn_sig(&prog, owner).unwrap().bound_regions, 0);
        assert!(tcx.resolve_bound_vars(&prog, owner).names.declarations.is_empty());
        assert_eq!(tcx.dcx.err_count(), 0);
    }
    #[test]
    fn round32_lifetime_associated_const_path_elision_error() {
        let (prog, tcx) = source("#![no_std] struct Wrap<'a>(&'a u8); trait Tr { const VALUE: Wrap; } static STANDALONE: Wrap = loop {};", 2021);
        tcx.resolve_bound_vars(&prog, named(&prog, "VALUE"));
        let table = tcx.resolve_bound_vars(&prog, named(&prog, "STANDALONE"));
        assert!(tcx.dcx.err_count() > 0);
        assert!(table.named_variable_map.values().any(|r| *r == ResolvedArg::StaticLifetime));
        tcx.dcx.flush_delayed();
    }
    #[test]
    fn round32_lifetime_identity_and_elision() {
        let (prog, tcx) = source("#![no_std] fn named<'a>(x: &'a u8) -> &'a u8 { x } fn anonymous(x: &u8) -> &u8 { x }", 2021);
        for name in ["named", "anonymous"] {
            let owner = named(&prog, name);
            let table = resolve_bound_vars(&tcx, &prog, owner, None);
            assert_eq!(table.late_bound_vars_map[&BinderId::Item].len(), 1);
            assert!(table.generic_indices.lifetimes.is_empty());
            let BoundVariableKind::Region(BoundRegionKind::Named(id)) = table.late_bound_vars_map[&BinderId::Item][0] else { panic!("named lifetime binder"); };
            assert_eq!(id.owner, owner);
            assert_eq!(table.named_variable_map.values().filter(|arg| **arg == ResolvedArg::LateBound(0, BoundVar(0), id)).count(), 2);
        }
        assert_eq!(tcx.dcx.err_count(), 0);
    }
    #[test]
    fn round32_lifetime_nested_capture_remapping() {
        let (prog, tcx) = source("#![no_std] trait Outer { type Assoc; } trait Inner<'a> {} fn nested<'a>(x: &'a u8) -> impl Outer<Assoc = impl Inner<'a>> { loop {} }", 2021);
        let owner = named(&prog, "nested");
        let table = resolve_bound_vars(&tcx, &prog, owner, None);
        let opaques = &prog.opaque_types_defined_by[&owner];
        assert_eq!(opaques.len(), 2);
        let outer = table.opaque_captured_lifetimes[&opaques[0]][0];
        let inner = table.opaque_captured_lifetimes[&opaques[1]][0];
        assert!(matches!(outer.0, ResolvedArg::LateBound(0, BoundVar(0), _)));
        assert_eq!(inner.0, ResolvedArg::EarlyBound(outer.1));
        assert_ne!(inner.1, outer.1);
        assert_eq!(tcx.dcx.err_count(), 0);
    }
    #[test]
    fn round33_lifetime_trait_default_and_impl_rpitit_capture_all() {
        // resolve_bound_vars.rs:527-617 + opaque_captures_all_in_scope_lifetimes: RPITITs (trait
        // default, trait required, impl) capture every in-scope lifetime incl. the parent item's
        // (Scope::Root walk :563-575) in every edition; an inherent RPIT only from 2024 on.
        for edition in [2021, 2024] {
            let (prog, tcx) = source("#![no_std] trait Copy {} impl<'x> Copy for &'x u8 {} trait Tr<'t> { fn default_body<'a>(&'a self, x: &'a u8) -> impl Copy { x } fn required<'a>(&'a self, x: &'a u8) -> impl Copy; } struct S; impl<'i> Tr<'i> for S { fn required<'a>(&'a self, x: &'a u8) -> impl Copy { x } } fn inherent<'a>(x: &'a u8) -> impl Copy { loop {} }", edition);
            let methods: Vec<DefId> = (0..prog.defs.len() as u32).map(DefId).filter(|&d| matches!(prog.name(d), "default_body" | "required" | "inherent") && prog.def(d).kind != crate::program::DefKind::OpaqueTy).collect();
            assert_eq!(methods.len(), 4);
            for owner in methods {
                let opaque = prog.opaque_types_defined_by[&owner][0];
                let expected = if prog.name(owner) == "inherent" { usize::from(edition >= 2024) } else { 2 };
                let before = tcx.opaque_generics(&prog, opaque);
                assert_eq!(before.captured_lifetimes.len(), expected, "{} {edition}", prog.name(owner));
                let bounds = tcx.explicit_item_bounds(&prog, opaque).to_vec();
                tcx.fn_sig(&prog, owner).unwrap();
                assert_eq!(before.captured_lifetimes, tcx.opaque_generics(&prog, opaque).captured_lifetimes);
                assert_eq!(bounds, tcx.explicit_item_bounds(&prog, opaque));
            }
            // The impl's RPITIT value is the impl opaque, remapped onto the trait's synthesized
            // associated type (compare_impl_item.rs:717-756) without errors.
            let impl_required = (0..prog.defs.len() as u32).map(DefId).find(|&d| prog.name(d) == "required" && prog.def(prog.def(d).parent).kind == crate::program::DefKind::Impl).unwrap();
            let values = tcx.collect_return_position_impl_trait_in_trait_tys(&prog, impl_required).unwrap();
            assert_eq!(values.len(), 1);
            // The value is the impl's own opaque, instantiated with the impl method's identity args.
            let TyKind::Alias(_, alias) = tcx.tys.kind(values[0].1) else { panic!("RPITIT value is not an alias") };
            let impl_opaque = prog.opaque_types_defined_by[&impl_required][0];
            assert_eq!(alias.kind, AliasTyKind::Opaque { def_id: impl_opaque });
            let generics = tcx.opaque_generics(&prog, impl_opaque);
            assert_eq!(tcx.tys.region_args(alias.regions).len(), generics.parent.lifetimes.len() + generics.captured_lifetimes.len());
            assert_eq!(tcx.dcx.err_count(), 0, "{edition}");
        }
    }
    fn sig_errors(tcx: &Tcx, prog: &Program, name: &str) -> Vec<String> {
        tcx.signature_parts(prog, named(prog, name)).map_or(Vec::new(), |parts| parts.errors.clone())
    }
    fn object_default(tcx: &Tcx, prog: &Program, owner: DefId) -> Vec<RegionKind> {
        let table = tcx.resolve_bound_vars(prog, owner);
        let mut defaults: Vec<_> = table.named_variable_map.iter().filter(|(key, _)| matches!(key, LifetimeKey::ObjectDefault(_))).map(|(key, arg)| (*key, table.region_for_arg(*arg))).collect();
        defaults.sort_by_key(|(key, _)| match key { LifetimeKey::ObjectDefault(t) => *t, _ => 0 });
        defaults.into_iter().map(|(_, r)| r).collect()
    }
    #[test]
    fn round33_lifetime_fn_pointer_inherits_object_default() {
        // resolve_bound_vars.rs:720-749: a fn pointer only adds a binder, so `dyn Tr` inside
        // `&'a fn(*const dyn Tr)` defaults to 'a (rustc accepts g below).
        let (prog, tcx) = source("#![no_std] trait Tr {} struct S<'a>(&'a fn(*const dyn Tr)); fn g<'a>(f: &'a fn(*const dyn Tr), p: *const (dyn Tr + 'a)) { f(p) }", 2021);
        assert_eq!(object_default(&tcx, &prog, named(&prog, "S")), [RegionKind::ReEarlyParam(0)]);
        assert!(tcx.typeck(&prog, named(&prog, "g")).errors.is_empty());
        assert_eq!(tcx.dcx.err_count(), 0);
    }
    #[test]
    fn round33_lifetime_type_relative_paths_take_the_enclosing_object_default() {
        // resolve_bound_vars.rs:1686-1809 (visit_segment_args runs for resolved paths only):
        // `X::A<dyn Tr>` (type-relative) pushes no object default scope, so a signature's
        // `'static` applies (no E0228); `<X as G>::A<dyn Tr>` is resolved and takes A's
        // declared default.
        let (prog, tcx) = source("#![no_std] trait Tr {} trait G { type A<T: ?Sized>; } fn relative<X: G>(_: X::A<dyn Tr>) {} fn qualified<X: G>(_: <X as G>::A<dyn Tr>) {}", 2021);
        for name in ["relative", "qualified"] {
            tcx.fn_sig(&prog, named(&prog, name));
            assert!(sig_errors(&tcx, &prog, name).is_empty(), "{name}: {:?}", sig_errors(&tcx, &prog, name));
        }
        assert_eq!(object_default(&tcx, &prog, named(&prog, "qualified")), [RegionKind::ReStatic]);
    }
    #[test]
    fn round33_lifetime_type_relative_prefix_keeps_its_container() {
        // resolve_bound_vars.rs:904-944: in `S::<'a, dyn Tr>::new()` the resolved prefix `S`
        // still supplies S's `T: 'x` default ('a) to its own arguments.
        let (prog, tcx) = source("#![no_std] trait Tr {} struct S<'x, T: ?Sized + 'x>(&'x T); impl<'x, T: ?Sized + 'x> S<'x, T> { fn new() {} } fn body<'a>() { S::<'a, dyn Tr>::new(); }", 2021);
        let owner = named(&prog, "body");
        let table = tcx.resolve_bound_vars(&prog, owner);
        let defaults: Vec<_> = table.named_variable_map.iter().filter(|(key, _)| matches!(key, LifetimeKey::ObjectDefault(_))).map(|(_, arg)| *arg).collect();
        assert_eq!(defaults.len(), 1);
        assert!(matches!(defaults[0], ResolvedArg::LateBound(..) | ResolvedArg::EarlyBound(_) | ResolvedArg::Free(..)), "{defaults:?}");
        assert_eq!(tcx.dcx.err_count(), 0);
    }
    #[test]
    fn round33_lifetime_impl_header_rejects_implicit_path_lifetime() {
        // late.rs:3549-3552: AnonymousCreateParameter { report_in_path: true } (E0726).
        for (text, errors) in [("#![no_std] struct S<'a>(&'a u8); impl S {}", 1), ("#![no_std] struct S<'a>(&'a u8); impl S<'_> {}", 0)] {
            let (prog, tcx) = source(text, 2021);
            let imp = (0..prog.defs.len() as u32).map(DefId).find(|&d| prog.def(d).kind == DefKind::Impl).unwrap();
            tcx.resolve_bound_vars(&prog, imp);
            assert_eq!(tcx.dcx.err_count(), errors, "{text}");
            tcx.dcx.flush_delayed();
        }
    }
    #[test]
    fn round33_lifetime_expression_and_pattern_paths_infer() {
        // late.rs:2271-2296 (PathSource) + resolve_params :4067-4077: implicit lifetimes of
        // expression/pattern paths are inferred, also in closure parameter patterns; only the
        // written `'_` of the parameter type creates a closure lifetime.
        let (prog, tcx) = source("#![no_std] struct W<'a>(&'a u8); fn f() { let c = |W(x): W<'_>| *x; }", 2021);
        let owner = named(&prog, "f");
        let names = tcx.resolve_lifetime_names(&prog, owner);
        assert!(names.uses.iter().any(|(key, res)| matches!(key, LifetimeKey::Path(..)) && *res == LifetimeRes::Infer));
        let closure_params: usize = names.binders.iter().filter(|(binder, _)| matches!(binder, BinderId::Closure(_))).map(|(_, ids)| ids.len()).sum();
        assert_eq!(closure_params, 1);
        assert_eq!(tcx.dcx.err_count(), 0);
    }
    #[test]
    fn round33_lifetime_self_elision_requires_a_type_constructor() {
        // late.rs:2730-2745 + resolve_fn_params (Set1::Empty => Elision::None): a type alias
        // impl self type is not `Self` for elision, so `-> &u8` takes x's lifetime (rustc
        // accepts returning x); for the struct itself the output takes self's lifetime.
        for (text, from_self) in [
            ("#![no_std] struct S; type Alias = S; impl Alias { fn f(self: &Alias, x: &u8) -> &u8 { x } }", false),
            ("#![no_std] struct S; impl S { fn f(self: &S, x: &u8) -> &u8 { loop {} } }", true),
        ] {
            let (prog, tcx) = source(text, 2021);
            let sig = tcx.fn_sig(&prog, named(&prog, "f")).unwrap();
            let region = |t: TyId| match tcx.tys.kind(t) { TyKind::Ref(_, _, r) => *r, _ => panic!("reference") };
            assert_eq!(region(sig.ret) == region(sig.params[0]), from_self, "{text}");
            assert_eq!(region(sig.ret) == region(sig.params[1]), !from_self, "{text}");
            assert_eq!(tcx.dcx.err_count(), 0, "{text}");
        }
    }
    #[test]
    fn round33_lifetime_constraint_gat_arguments_forbid_anonymous() {
        // late.rs:1341-1347: `I<'_> = ()` in a constraint is AnonymousReportError (E0637).
        let (prog, tcx) = source("#![no_std] trait L { type I<'a>; } fn f(x: &u8) -> impl L<I<'_> = ()> { loop {} }", 2021);
        tcx.resolve_bound_vars(&prog, named(&prog, "f"));
        assert_eq!(tcx.dcx.err_count(), 1);
        tcx.dcx.flush_delayed();
    }
    #[test]
    fn round33_lifetime_paren_sugar_outside_a_poly_trait_walks_normally() {
        // late.rs:1366-1396: `FnOnce(&u8)` in a qualified path has no poly-trait binder; its
        // `&u8` elides like any input type of `f` instead of binding on a missing binder.
        let (prog, tcx) = source("#![no_std] #[lang = \"fn_once\"] trait FnOnce<Args> { #[lang = \"fn_once_output\"] type Output; } fn f<F: FnOnce(&u8)>(x: <F as FnOnce(&u8)>::Output) {}", 2021);
        let owner = named(&prog, "f");
        let names = tcx.resolve_lifetime_names(&prog, owner);
        assert_eq!(names.binders.get(&BinderId::Item).map_or(0, |ids| ids.len()), 1, "the qualified path's `&u8` is a fresh parameter of f");
        tcx.resolve_bound_vars(&prog, owner);
        assert_eq!(tcx.dcx.err_count(), 0);
        tcx.dcx.flush_delayed();
    }
    #[test]
    fn round33_lifetime_associated_type_segment_has_no_elided_lifetimes() {
        // late.rs:2229-2258: only the trait segment of `<T as L>::I` gets elided lifetimes;
        // the associated type's own (GAT) lifetimes are not fabricated.
        let (prog, tcx) = source("#![no_std] trait L<'t> { type I<'a>; } fn g<'b, T: L<'b>>(x: &<T as L>::I<'b>) {} fn h<'b, T: L<'b>>(x: <T as L<'b>>::I) {}", 2021);
        let path_uses = |name| tcx.resolve_lifetime_names(&prog, named(&prog, name)).uses.keys().filter(|key| matches!(key, LifetimeKey::Path(..))).count();
        assert_eq!(path_uses("g"), 1, "only L's lifetime on the trait segment");
        assert_eq!(path_uses("h"), 0, "I's own lifetime is not elided");
    }
    #[test]
    fn round33_param_assoc_type_ambiguity_is_e0221() {
        // hir_ty_lowering/mod.rs:1351-1397: two bounds defining `X` are ambiguous (E0221); the
        // same trait reached twice (directly and as a supertrait) is one candidate.
        let (prog, tcx) = source("#![no_std] trait A { type X; } trait B { type X; } trait C: A {} fn two<T: A + B>(_: T::X) {} fn one<T: A>(_: T::X) {} fn dedup<T: C + A>(_: T::X) {}", 2021);
        for (name, ambiguous) in [("two", true), ("one", false), ("dedup", false)] {
            tcx.fn_sig(&prog, named(&prog, name));
            assert_eq!(sig_errors(&tcx, &prog, name).iter().any(|e| e.contains("E0221")), ambiguous, "{name}: {:?}", sig_errors(&tcx, &prog, name));
        }
    }
    #[test]
    fn round32_lifetime_higher_ranked_capture_is_rejected() {
        let (prog, tcx) = source("#![no_std] trait Outer<'a> { type Assoc; } trait Inner<'a> {} fn higher() -> impl for<'a> Outer<'a, Assoc = impl Inner<'a>> { loop {} }", 2021);
        let owner = named(&prog, "higher");
        let table = resolve_bound_vars(&tcx, &prog, owner, None);
        assert_eq!(tcx.dcx.err_count(), 1);
        assert!(table.opaque_captured_lifetimes.values().any(|captures| captures.iter().any(|(arg, _)| *arg == ResolvedArg::Error)));
        tcx.dcx.flush_delayed();
    }
}

impl ResolveBoundVars {
    pub(super) fn declaration_region(&self, id: LifetimeId) -> RegionKind {
        if let Some(&index) = self.early_indices.get(&id) { return RegionKind::ReEarlyParam(index); }
        let vars = &self.late_bound_vars_map[&BinderId::Item];
        let index = vars.iter().position(|var| *var == BoundVariableKind::Region(BoundRegionKind::Named(id))).expect("late lifetime declaration");
        RegionKind::ReBound(0, BoundVar(index as u32))
    }
    pub(super) fn region_for_use(&self, key: LifetimeKey, pred: Option<u32>) -> Option<RegionKind> {
        let arg = match self.named_variable_map.get(&key.scoped(pred)).or_else(|| self.named_variable_map.get(&key)).copied() {
            Some(arg) => arg,
            None => {
                if let LifetimeKey::Named(lo, hi) = key {
                    if let Some(LifetimeRes::Param(id)) = self.names.uses.get(&key) {
                        let declaration = self.names.declarations.iter().chain(&self.names.inherited).find(|d| d.id == *id).expect("named lifetime declaration");
                        if declaration.binder == BinderId::Item && declaration.span == lo && hi > lo {
                            return Some(self.declaration_region(*id));
                        }
                    }
                }
                return None;
            }
        };
        Some(self.region_for_arg(arg))
    }
    /// `map_opaque_lifetime_to_parent_lifetime` for each lifetime the opaque captures: a lifetime
    /// that maps to a parameter of an enclosing opaque keeps mapping until it reaches the item's own.
    pub(crate) fn opaque_captured_parent_lifetimes(&self, opaque: DefId, owner: DefId) -> Vec<RegionKind> {
        self.opaque_captured_lifetimes[&opaque].iter().map(|&(mut arg, _)| {
            while let ResolvedArg::EarlyBound(id) = arg {
                let decl = self.names.declarations.iter().chain(self.names.inherited.iter()).find(|d| d.id == id).expect("lifetime declaration");
                let BinderId::Opaque(outer) = decl.binder else { break };
                arg = self.opaque_captured_lifetimes[&outer].iter().find(|(_, own)| *own == id).expect("captured lifetime of the enclosing opaque").0;
            }
            match arg {
                // a late-bound parameter of the item, as its signature's liberated regions name it
                ResolvedArg::LateBound(_, var, _) => RegionKind::ReLateParam(crate::types::RegionScope::Item(owner), crate::types::LateParamKind::Anon(var.0)),
                _ => self.region_for_arg(arg),
            }
        }).collect()
    }
    pub(crate) fn region_for_arg(&self, arg: ResolvedArg) -> RegionKind {
        match arg {
            ResolvedArg::StaticLifetime => RegionKind::ReStatic,
            ResolvedArg::EarlyBound(id) => RegionKind::ReEarlyParam(self.early_indices[&id]),
            ResolvedArg::LateBound(depth, var, _) => RegionKind::ReBound(depth, var),
            ResolvedArg::Free(owner, id) => {
                let declaration = self.names.declarations.iter().find(|d| d.id == id).expect("liberated lifetime declaration");
                let vars = &self.late_bound_vars_map[&declaration.binder];
                let index = vars.iter().position(|var| *var == BoundVariableKind::Region(BoundRegionKind::Named(id))).unwrap();
                let scope = match declaration.binder {
                    BinderId::Closure(expr) => crate::types::RegionScope::Closure { owner: id.owner, expr },
                    _ => crate::types::RegionScope::Item(owner),
                };
                RegionKind::ReLateParam(scope, crate::types::LateParamKind::Anon(index as u32))
            }
            ResolvedArg::Error => RegionKind::ReError,
        }
    }
}

crate::collected_enum!(BinderId; 0 => Item, 1 => FnPointer(n), 2 => Trait(n), 3 => Where(n), 4 => Closure(n), 5 => GenericBound(n), 6 => ArgumentImplTrait(n), 7 => Opaque(d));
crate::collected_enum!(LifetimeKey; 0 => Named(a,b), 1 => Reference(n), 2 => SelfReference, 3 => Path(a,b), 4 => ObjectDefault(n), 5 => Scoped(p,t,a,b));
crate::collected_enum!(ResolvedArg; 0 => StaticLifetime, 1 => EarlyBound(id), 2 => LateBound(d,v,id), 3 => Free(d,id), 4 => Error);
crate::collected_fields!(ResolveBoundVars, names, named_variable_map, late_bound_vars_map, opaque_captured_lifetimes, early_indices, generic_indices);
crate::collected_enum!(BoundVariableKind; 0 => Ty, 1 => Region(r), 2 => Const);
