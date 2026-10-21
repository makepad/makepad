//! Declaration variance (rustc_hir_analysis/src/variance/{terms,constraints,solve}.rs).
//! ADT/constructor inputs are collected types; function variance is inferred on demand
//! against the converged ADT graph. Written signatures cannot contain FnDef types.
//! Bounds do not constrain ADT variance.
use crate::types::AliasTy;
use crate::fxhash::{HashMap, HashSet};
use crate::lang_items::LangItem;
use crate::program::DefId;
use crate::tcx::{GenEnv, Tcx};
use super::{RegionId, RegionKind, TyId, TyKind, Types};
use std::collections::VecDeque;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum V {
    Bi,
    Co,
    Contra,
    Inv,
}

/// rustc variance xform: the variance of a position inside a context.
pub fn xform(ctx: V, v: V) -> V {
    match ctx {
        V::Co => v,
        V::Contra => match v {
            V::Co => V::Contra,
            V::Contra => V::Co,
            x => x,
        },
        // rustc_type_ir/src/lib.rs:318: invariant context stays invariant,
        // including unused arguments of an opaque being generalized.
        V::Inv => V::Inv,
        V::Bi => V::Bi,
    }
}

/// glb in rustc's variance lattice (two uses with different variances).
pub fn join(a: V, b: V) -> V {
    match (a, b) {
        (V::Bi, x) | (x, V::Bi) => x,
        (V::Co, V::Co) => V::Co,
        (V::Contra, V::Contra) => V::Contra,
        _ => V::Inv,
    }
}

/// Maps each of Relax's two argument lists into the full declaration index
/// space. `ReEarlyParam` uses full indices; `Param` uses compact type/const
/// indices. Late-bound lifetimes are not generic arguments.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct GenericIndices {
    pub lifetimes: Vec<u32>,
    pub types_consts: Vec<u32>,
    /// Full indices of const parameters, including inherited parameters.
    pub consts: Vec<u32>,
}

impl GenericIndices {
    pub(crate) fn from_env(env: &GenEnv) -> Self {
        let mut lifetimes: Vec<_> = env.regions.names.iter().filter_map(|(_, r)| {
            if let RegionKind::ReEarlyParam(i) = r { Some(*i) } else { None }
        }).collect();
        lifetimes.extend(&env.regions.anonymous_params);
        lifetimes.sort_unstable();
        let mut types_consts = Vec::with_capacity(env.names.len());
        let mut full = 0;
        while types_consts.len() < env.names.len() {
            if !lifetimes.contains(&full) { types_consts.push(full); }
            full += 1;
        }
        let consts = env.consts.iter().map(|(i, _, _)| types_consts[*i as usize]).collect();
        Self { lifetimes, types_consts, consts }
    }

    fn len(&self) -> usize {
        self.lifetimes.iter().chain(&self.types_consts).max().map_or(0, |i| *i as usize + 1)
    }
}

impl Tcx {
    /// rustc_middle/src/ty/util.rs:954-974: capture variances exist only for
    /// opaques and return-position impl-trait-in-trait projections.
    pub(crate) fn opt_alias_variances(&self, prog: &crate::program::Program, kind: crate::types::AliasTyKind) -> Option<Vec<V>> {
        let def = match kind {
            crate::types::AliasTyKind::Opaque { def_id } => def_id,
            crate::types::AliasTyKind::Projection { def_id } if prog.impl_trait_in_trait_data.contains_key(&def_id) => def_id,
            _ => return None,
        };
        Some(self.variances_of(prog, def))
    }
    /// opt_alias_variances with the alias's generic indices, so callers can map an
    /// alias's split type/region argument lists onto declaration-order variances.
    pub(crate) fn alias_declared_variances(&self, prog: &crate::program::Program, kind: crate::types::AliasTyKind) -> Option<(GenericIndices, Vec<V>)> {
        let variances = self.opt_alias_variances(prog, kind)?;
        Some((self.rpitit_generic_indices(prog, kind.def_id())?, variances))
    }
    /// Variances in full declaration order, cached only after convergence.
    /// Collection publication invalidates the entire dependency graph, including
    /// unchanged declarations which refer to an edited ADT. Session-local only.
    pub fn variances_of(&self, prog: &crate::program::Program, def: DefId) -> Vec<V> {
        crate::type_queries::Providers { tcx: self, prog }.variances_of(def).expect("variances_of query")
    }
    /// The variances of the arguments of a `FnDef` type. A tuple struct shares its DefId with its
    /// constructor, and `variances_of` answers for the ADT; the constructor item (rustc
    /// `DefKind::Ctor`) is a function whose variances come from its signature `fn(fields) -> S<..>`.
    pub fn fn_def_variances(&self, prog: &crate::program::Program, def: DefId) -> Vec<V> {
        let Some(sig) = self.ctor_sigs.get(&def).filter(|_| self.adts.contains_key(&def)) else { return self.variances_of(prog, def) };
        let values = self.variances.get_or_init(|| infer(self));
        let mut walk = Walk { tcx: self, tys: &self.tys, values, indices: &sig.generic_indices,
            uses: vec![V::Bi; sig.generic_indices.len()], deps: HashSet::default() };
        for &p in &sig.params { walk.ty(p, V::Contra); }
        walk.ty(sig.ret, V::Co);
        for &i in &sig.generic_indices.consts { walk.uses[i as usize] = V::Inv; }
        for v in &mut walk.uses { if *v == V::Bi { *v = V::Inv; } }
        walk.uses
    }
    pub(crate) fn variances_of_uncached(&self, prog: &crate::program::Program, def: DefId) -> Vec<V> {
        if let Some(relax_front::assoc::ImplTraitInTraitData::Trait { opaque_def_id, .. }) = prog.impl_trait_in_trait_data.get(&def) {
            return self.variances_of(prog, *opaque_def_id);
        }
        if prog.def(def).kind == crate::program::DefKind::OpaqueTy { return self.variance_of_opaque(prog, def); }
        let values = self.variances.get_or_init(|| infer(self));
        if let Some(v) = values.get(&def) { return v.clone(); }
        let Some(parts) = self.signature_parts(prog, def) else { return Vec::new(); };
        crate::query::init_once(&parts.variances, || {
            let sig = self.fn_sig(prog, def).unwrap();
            let mut walk = Walk { tcx: self, tys: &self.tys, values, indices: &sig.generic_indices,
                uses: vec![V::Bi; sig.generic_indices.len()], deps: HashSet::default() };
            for &p in &sig.params { walk.ty(p, V::Contra); }
            walk.ty(sig.ret, V::Co);
            for &i in &sig.generic_indices.consts { walk.uses[i as usize] = V::Inv; }
            for v in &mut walk.uses { if *v == V::Bi { *v = V::Inv; } }
            walk.uses
        }, |_| {}).clone()
    }
}

struct Declaration<'a> {
    indices: &'a GenericIndices,
    types: Vec<(TyId, V)>,
    function: bool,
}

fn infer(tcx: &Tcx) -> HashMap<DefId, Vec<V>> {
    let mut decls = HashMap::default();
    for (&d, sig) in tcx.ctor_sigs.iter() {
        let mut types: Vec<_> = sig.params.iter().map(|&t| (t, V::Contra)).collect();
        types.push((sig.ret, V::Co));
        decls.insert(d, Declaration { indices: &sig.generic_indices, types, function: true });
    }
    // A tuple struct shares its DefId with its constructor: ADT variance wins.
    for (&d, adt) in &tcx.adts {
        let types = adt.variants.iter().flat_map(|v| v.fields.iter().map(|f| (f.ty, V::Co))).collect();
        decls.insert(d, Declaration { indices: &adt.generic_indices, types, function: false });
    }
    let mut values = HashMap::default();
    for (&d, decl) in &decls {
        let mut vs = vec![V::Bi; decl.indices.len()];
        if let Some(&i) = decl.indices.types_consts.first() {
            if Some(d) == tcx.lang_items.get(LangItem::PhantomData) { vs[i as usize] = V::Co; }
            if Some(d) == tcx.lang_items.get(LangItem::UnsafeCell) { vs[i as usize] = V::Inv; }
        }
        values.insert(d, vs);
    }
    // Dependencies are declaration identities, including edges inside nested
    // type arguments. Never expand an ADT's fields while walking a field type.
    let mut users: HashMap<DefId, Vec<DefId>> = HashMap::default();
    let mut ids: Vec<_> = decls.keys().copied().collect();
    ids.sort_by_key(|d| d.0);
    for &d in &ids {
        let (_, deps) = evaluate(tcx, &values, &decls[&d]);
        for dep in deps { users.entry(dep).or_default().push(d); }
    }
    let mut queue: VecDeque<_> = ids.iter().copied().collect();
    let mut queued: HashSet<_> = ids.into_iter().collect();
    while let Some(d) = queue.pop_front() {
        queued.remove(&d);
        let (uses, _) = evaluate(tcx, &values, &decls[&d]);
        let vs = values.get_mut(&d).unwrap();
        let mut changed = false;
        for (old, use_) in vs.iter_mut().zip(uses) {
            let new = join(*old, use_);
            changed |= new != *old;
            *old = new;
        }
        if changed {
            for &user in users.get(&d).into_iter().flatten() {
                if queued.insert(user) { queue.push_back(user); }
            }
        }
    }
    // Like rustc's create_map: consts are invariant, as are unused function
    // parameters. Unused ADT parameters retain genuine bivariance.
    for (d, vs) in &mut values {
        let decl = &decls[d];
        for &i in &decl.indices.consts { vs[i as usize] = V::Inv; }
        if decl.function {
            for v in vs { if *v == V::Bi { *v = V::Inv; } }
        }
    }
    values
}

fn evaluate(tcx: &Tcx, values: &HashMap<DefId, Vec<V>>, decl: &Declaration<'_>) -> (Vec<V>, HashSet<DefId>) {
    let mut walk = Walk { tcx, tys: &tcx.tys, values, indices: decl.indices,
        uses: vec![V::Bi; decl.indices.len()], deps: HashSet::default() };
    for &(ty, variance) in &decl.types { walk.ty(ty, variance); }
    (walk.uses, walk.deps)
}

struct Walk<'a> {
    tys: &'a Types,
    tcx: &'a Tcx,
    values: &'a HashMap<DefId, Vec<V>>,
    indices: &'a GenericIndices,
    uses: Vec<V>,
    deps: HashSet<DefId>,
}

impl Walk<'_> {
    fn constrain(&mut self, full: u32, v: V) {
        self.uses[full as usize] = join(self.uses[full as usize], v);
    }

    fn region(&mut self, r: RegionId, v: V) {
        if let RegionKind::ReEarlyParam(i) = self.tys.region_kind(r) {
            self.constrain(i, v);
        }
        // ReBound belongs to a signature/predicate binder, not the declaration.
    }

    fn ct(&mut self, ct: crate::consts::Const, v: V) {
        use crate::consts::{ConstKind, GenericArg};
        match self.tys.const_kind(ct) {
            ConstKind::Alias(_, alias) => for arg in &*alias.args {
                match *arg {
                    GenericArg::Type(t) => self.ty(t, v),
                    GenericArg::Const(ct) => self.ct(ct, v),
                    GenericArg::Lifetime(r) => self.region(r, v),
                }
            },
            ConstKind::Value(value) => self.ty(value.ty, v),
            _ => {}
        }
    }

    fn ty(&mut self, t: TyId, v: V) {
        match self.tys.kind(t).clone() {
            // (a declaration whose generics failed to lower has no indices to constrain)
            TyKind::Param(i) => if let Some(&full) = self.indices.types_consts.get(i as usize) { self.constrain(full, v) },
            TyKind::Ref(m, t, r) => {
                self.region(r, v);
                self.ty(t, xform(v, if m { V::Inv } else { V::Co }));
            }
            TyKind::Ptr(m, t) => self.ty(t, xform(v, if m { V::Inv } else { V::Co })),
            TyKind::Slice(t) => self.ty(t, v),
            TyKind::Array(t, len) => {
                self.ty(t, v);
                self.ct(len, xform(v, V::Inv));
            }
            TyKind::Const(ct) => self.ct(ct, V::Inv),
            TyKind::Tuple(ts) => { for &t in self.tys.list(ts) { self.ty(t, v); } }
            TyKind::FnPtr(ps, ret, _, _) => {
                for &p in self.tys.list(ps) { self.ty(p, xform(v, V::Contra)); }
                self.ty(ret, v);
            }
            TyKind::Adt(d, ts, rs) | TyKind::FnDef(d, ts, rs) => {
                self.deps.insert(d);
                let target = self.tcx.adts.get(&d).map(|a| &a.generic_indices)
                    .or_else(|| self.tcx.ctor_sigs.get(&d).map(|s| &s.generic_indices));
                for (i, &t) in self.tys.list(ts).iter().enumerate() {
                    let arg_v = target.and_then(|decl| decl.types_consts.get(i))
                        .map_or(V::Inv, |&full| self.values[&d][full as usize]);
                    self.ty(t, xform(v, arg_v));
                }
                for (i, &r) in self.tys.region_args(rs).iter().enumerate() {
                    let arg_v = target.and_then(|decl| decl.lifetimes.get(i))
                        .map_or(V::Inv, |&full| self.values[&d][full as usize]);
                    self.region(r, xform(v, arg_v));
                }
            }
            TyKind::Alias(_, AliasTy { args: ts, regions: rs, .. }) => {
                let inv = xform(v, V::Inv);
                for &t in self.tys.list(ts) { self.ty(t, inv); }
                for &r in self.tys.region_args(rs).iter() { self.region(r, inv); }
            }
            TyKind::Dyn(_, ts, bindings, rs, r, _, _) => {
                self.region(r, v);
                let inv = xform(v, V::Inv);
                for &t in self.tys.list(ts) { self.ty(t, inv); }
                for &r in self.tys.region_args(rs).iter() { self.region(r, inv); }
                for &(_, t, _) in self.tys.list(bindings) { self.ty(t, V::Inv); }
            }
            TyKind::Closure(_, _, crate::types::ClosureArgs { parent_args: owner, closure_kind_ty, closure_sig_as_fn_ptr_ty: sig, tupled_upvars_ty: captures }) => {
                for t in [sig, captures, owner, closure_kind_ty] { self.ty(t, xform(v, V::Inv)); }
            }
            TyKind::Bool | TyKind::Char | TyKind::Str | TyKind::Int(_) |
            TyKind::Foreign(_) | TyKind::Float(_) | TyKind::Never | TyKind::Error |
            TyKind::Infer(_) | TyKind::IntVar(_) | TyKind::FloatVar(_) | TyKind::Bound(..) | TyKind::Placeholder(_) => {}
        }
    }
}

#[cfg(test)]
#[path = "variance/tests.rs"]
mod tests;
