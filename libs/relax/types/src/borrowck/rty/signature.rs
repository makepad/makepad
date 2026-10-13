//! Typed signature-to-BIR producer adapter, retained until C0b.
//! Declaration identity (including unused parameters) precedes I/O traversal.
use crate::types::{AliasTy, AliasTyKind};
use super::{Rty, SigTmpl, SK_ELIDED, SK_FRESH, SK_NAMED, SK_PARAM, SR};
use crate::borrowck::implied_bounds::ImpliedBounds;
use crate::program::DefId;
use crate::tcx::{
    outlives::OutlivesPredicate,
};
use crate::types::{LateParamKind, RegionId, RegionKind, RegionScope, TyId, TyKind};

pub use crate::borrowck::universal_regions::{SignatureRegions, UniversalRegion, Identity, RegionClassification as Classification};

/// Read-only projection for the pre-A4c body/call consumers. No AST is consulted.
/// The vectors follow the existing body layout; the authoritative contract is
/// the collected typed signature and its predicates.
pub fn signature_template(rty: &Rty<'_>, d: DefId, args: &[TyId]) -> SigTmpl {
    signature_template_for(rty,d,args,false)
}
fn signature_template_for(rty: &Rty<'_>, d: DefId, args: &[TyId], body: bool) -> SigTmpl {
    let tcx = rty.tx.tcx;
    let u = if body { SignatureRegions::function(tcx,rty.tx.prog,d) } else { SignatureRegions::function_call(tcx,rty.tx.prog,d) };
    let (inputs, output, contract, universals) = if let Some(u) = u.clone() {
        (u.inputs, u.output, u.contract, u.regions)
    } else if let Some(t) = tcx.const_ty(rty.tx.prog, d) {
        (
            Vec::new(),
            t,
            tcx.const_region_contract(rty.tx.prog, d)
                .cloned()
                .unwrap_or_default(),
            Vec::new(),
        )
    } else {
        return empty();
    };
    // N:rustc_borrowck/src/implied_bounds.rs:42-62: these are the
    // declaration's uninstantiated inputs/output, so normalize them under that
    // declaration's ParamEnv, including its const-parameter type clauses.
    let result = tcx.param_env_id(rty.tx.prog, d);
    let param_env = result;
    let mut p = Projection {
        rty,
        param_env: param_env.clone(),
        args,
        regions: Vec::new(),
        params: Vec::new(),
        names: Vec::new(),
        kinds: Vec::new(),
    };
    for u in universals {
        if let Identity::Region(_, r) = u.identity {
            p.region(r, Some(&u.name));
        }
    }
    for (name, r) in &contract.regions {
        p.region(*r, Some(name));
    }
    let ins = inputs.iter().map(|t| p.ty(*t)).collect();
    let out = p.ty(output);
    let raw: Vec<_> = inputs.iter().copied().chain([output]).collect();
    let (mut assumed, normalized) = crate::borrowck::implied_bounds::mir_borrowck_implied_outlives_bounds(
        tcx,rty.tx.prog,d,rty.tx.cx.typing_env.mode,&param_env,false,&contract.predicates,&raw,&contract.assumed_wf)
        .unwrap_or_else(|_| crate::borrowck::implied_bounds::query_failed(tcx,rty.tx.prog,d,&contract.predicates,&raw));
    assumed.instantiate_query_bounds(tcx,&[]);
    let implied = assumed
        .outlives
        .iter()
        .map(|(a, b)| (p.region(*a, None), p.region(*b, None)))
        .collect();
    let mut tparam_bounds = Vec::new();
    for (t, r) in &assumed.type_outlives {
        if let TyKind::Param(i) = tcx.tys.kind(*t) {
            tparam_bounds.push((*i, p.region(*r, None)));
        }
    }
    // Caller WF is a separate result; callee assumptions are never copied into
    // it. Both input and output WF and declared caller predicates are required.
    let caller = ImpliedBounds::create(
        tcx, rty.tx.prog,
        &contract.caller_predicates,
        &raw,
        &normalized,
        assumed.obligations.clone(),
    );
    let mut cons: Vec<_> = caller
        .outlives
        .iter()
        .map(|(a, b)| (p.region(*a, None), p.region(*b, None)))
        .collect();
    let mut call_bounds = Vec::new();
    for (t, r) in &caller.type_outlives {
        let sr = p.region(*r, None);
        if let TyKind::Param(i) = tcx.tys.kind(*t) {
            call_bounds.push((*i, sr));
        }
        for s in p.ty(*t) {
            cons.push((s, sr));
        }
    }
    for obligation in caller.obligations {
        match obligation {
            OutlivesPredicate::Region(a, b) => cons.push((p.region(a, None), p.region(b, None))),
            OutlivesPredicate::Type(t, r) => {
                let r = p.region(r, None);
                for s in p.ty(t) {
                    cons.push((s, r));
                }
            }
        }
    }
    // Monomorphic Fn-family predicates relate the closure's external input
    // positions to the type arguments used by the ordinary call inputs.
    {
        let bounds = tcx.callable_signatures(rty.tx.prog, d);
        for bound in bounds {
            let Some(actual) = args.get(bound.param as usize) else {
                continue;
            };
            let TyKind::Closure(file, expr, ..) = tcx.tys.kind(*actual) else {
                continue;
            };
            let Some(layout) = rty.closures.borrow().get(&(*file, *expr)).cloned() else {
                continue;
            };
            let TyKind::FnPtr(ps, ret, _, 0) = tcx.tys.kind(bound.signature) else {
                continue;
            };
            let closure = p.ty(tcx.tys.intern(TyKind::Param(bound.param)));
            let mut offset = layout.caps.iter().map(|t| rty.n_slots(*t)).sum::<usize>();
            for (i, _, slots) in &layout.params {
                let n = slots.len();
                if let Some(t) = tcx.tys.list(*ps).get(*i) {
                    let all = p.callable_ty(*t, &mut cons);
                    let input: Vec<_> = slots.iter().filter_map(|&i| all.get(i).copied()).collect();
                    if input.len() == n && offset + n <= closure.len() {
                        for (a, b) in closure[offset..offset + n].iter().copied().zip(input) {
                            cons.extend([(a, b), (b, a)]);
                        }
                    }
                }
                offset += n;
            }
            let output = p.callable_ty(*ret, &mut cons);
            if offset + output.len() == closure.len() {
                for (a, b) in closure[offset..].iter().copied().zip(output) {
                    cons.extend([(a, b), (b, a)]);
                }
            }
        }
    }
    let self_ty = contract
        .self_ty
        .map(|t| rty.tx.normalize(tcx.tys.subst(t, args)));
    let self_slots = contract.self_ty.map(|t| p.ty(t)).unwrap_or_default();
    let trait_lts = contract
        .trait_regions
        .iter()
        .map(|r| p.region(*r, None))
        .collect();
    let trait_targs = contract
        .trait_types
        .iter()
        .map(|t| {
            let s = p.ty(*t);
            let t = rty.tx.normalize(tcx.tys.subst(*t, args));
            Some((s, t))
        })
        .collect();
    let own_targs = contract
        .own_types
        .iter()
        .map(|i| {
            i.and_then(|i| {
                args.get(i as usize).map(|t| {
                    let s = p.ty(tcx.tys.intern(TyKind::Param(i)));
                    (s, rty.tx.normalize(*t))
                })
            })
        })
        .collect();
    let proj_bounds = contract.predicates.iter().any(|p| matches!(p,OutlivesPredicate::Type(t,_) if matches!(tcx.tys.kind(*t),TyKind::Alias(_, AliasTy { kind: AliasTyKind::Projection { .. }, .. }))));
    SigTmpl {
        assumptions: assumed,
        universal: u,
        typed_regions: p.regions,
        n_syms: p.names.len() as u32,
        names: p.names,
        kinds: p.kinds,
        inputs: ins,
        output: out,
        cons,
        implied,
        ok: true,
        own_targs,
        trait_targs,
        trait_lts,
        tparam_bounds,
        tparam_names: contract.type_names,
        call_bounds,
        own_targ_index: contract.own_types,
        proj_bounds,
        self_ty,
        self_slots,
    }
}

fn empty() -> SigTmpl {
    SigTmpl {
        assumptions: ImpliedBounds::default(),
        universal: None,
        typed_regions: Vec::new(),
        n_syms: 0,
        names: Vec::new(),
        kinds: Vec::new(),
        inputs: Vec::new(),
        output: Vec::new(),
        cons: Vec::new(),
        implied: Vec::new(),
        ok: false,
        own_targs: Vec::new(),
        trait_targs: Vec::new(),
        trait_lts: Vec::new(),
        tparam_bounds: Vec::new(),
        tparam_names: Vec::new(),
        call_bounds: Vec::new(),
        own_targ_index: Vec::new(),
        proj_bounds: false,
        self_ty: None,
        self_slots: Vec::new(),
    }
}

struct Projection<'a, 'b> {
    rty: &'a Rty<'b>,
    param_env: crate::next_solver::ParamEnvId,
    args: &'a [TyId],
    regions: Vec<(RegionId, SR)>,
    params: Vec<(u32, Vec<SR>)>,
    names: Vec<String>,
    kinds: Vec<u8>,
}
impl Projection<'_, '_> {
    fn fresh(&mut self, name: &str, kind: u8) -> SR {
        let s = SR::Sym(self.names.len() as u32);
        self.names.push(name.into());
        self.kinds.push(kind);
        s
    }
    fn region(&mut self, r: RegionId, name: Option<&str>) -> SR {
        let kind = self.rty.tx.tcx.tys.region_kind(r);
        match kind {
            RegionKind::ReStatic => return SR::Static,
            RegionKind::ReErased | RegionKind::ReError | RegionKind::ReBound(..) => {
                return SR::Fresh
            }
            _ => {}
        }
        if let Some((_, s)) = self.regions.iter().find(|(a, _)| *a == r) {
            return *s;
        }
        let n = name.unwrap_or("'_");
        let k = if matches!(kind, RegionKind::ReVar(_)) {
            SK_FRESH
        } else if n != "'_" {
            SK_NAMED
        } else {
            SK_ELIDED
        };
        let s = self.fresh(n, k);
        self.regions.push((r, s));
        s
    }
    /// Instantiate before normalizing a callable predicate, as in rustc_borrowck
    /// type_check/mod.rs (FnDef predicates) and canonical.rs. The call's generic
    /// argument slots must survive normalization of e.g. `Self::Item`; normalizing
    /// the declaration under the callee's environment leaves that alias rigid.
    fn callable_ty(&mut self, ty: TyId, cons: &mut Vec<(SR, SR)>) -> Vec<SR> {
        use crate::types::fold::{TypeFolder, super_fold_ty};
        use crate::next_solver::type_op::{Normalize, ParamEnvAnd, TypeOp};
        let tx = self.rty.tx;
        let tys = &tx.tcx.tys;
        if !tys.may_project(ty) { return self.ty(ty); }
        let infcx = crate::infer::InferCtxt::for_item(tx.tcx, tx.prog, tx.cx.typing_env.mode, tx.obligation_owner.get());
        // Reify the adapter's existing slots into this operation's inference
        // context. Never reuse typeck's region variables or identify two slots
        // merely because their erased/typeck regions happen to match.
        struct Instantiate<'a, 'b, 'c, 'tcx> {
            p: &'a mut Projection<'b, 'c>, infcx: &'a crate::infer::InferCtxt<'tcx>,
            regions: Vec<(RegionId, SR)>, valid: bool,
        }
        impl Instantiate<'_, '_, '_, '_> {
            fn region(&mut self, slot: SR) -> RegionId {
                if slot == SR::Static { return self.infcx.tcx.tys.intern_region(RegionKind::ReStatic); }
                let slot = if slot == SR::Fresh { self.p.fresh("'_", SK_FRESH) } else { slot };
                if let Some((r, _)) = self.regions.iter().find(|(_, s)| *s == slot) { return *r; }
                let r = self.infcx.next_region_var();
                self.regions.push((r, slot)); r
            }
            fn unnumbered_region(&mut self, r: RegionId, depth: u32) -> RegionId {
                match self.infcx.tcx.tys.region_kind(r) {
                    RegionKind::ReBound(d, _) if d < depth => r,
                    RegionKind::ReStatic => r,
                    _ => self.region(SR::Fresh),
                }
            }
            fn argument(&mut self, ty: TyId, slots: &mut std::slice::Iter<'_, SR>, depth: u32) -> TyId {
                let tys = &self.infcx.tcx.tys;
                let mut region = |r| {
                    let Some(&slot) = slots.next() else { self.valid = false; return r; };
                    if matches!(tys.region_kind(r), RegionKind::ReBound(d, _) if d < depth) { r }
                    else { self.region(slot) }
                };
                // Same order as Projection::ty / Rty::skel_into. Nested FnPtr
                // binders have no BIR slots; dyn predicates retain their binders.
                let kind = match tys.kind(ty).clone() {
                    TyKind::Ref(m, t, r) => { let r = region(r); TyKind::Ref(m, self.argument(t, slots, depth), r) }
                    TyKind::Adt(d, ts, rs) => {
                        let rs = tys.intern_region_args(&tys.region_args(rs).iter().map(|r| region(*r)).collect::<Vec<_>>());
                        TyKind::Adt(d, tys.mk_list(&tys.list(ts).iter().map(|t| self.argument(*t, slots, depth)).collect::<Vec<_>>()), rs)
                    }
                    TyKind::Dyn(d, ts, bs, rs, r, n, autos) => {
                        let r = region(r);
                        let ts = tys.list(ts).iter().map(|t| self.argument(*t, slots, depth + 1)).collect::<Vec<_>>();
                        let bs = tys.list(bs).iter().map(|(d, t, n)| (*d, self.argument(*t, slots, depth + 1), *n)).collect::<Vec<_>>();
                        let rs = tys.intern_region_args(&tys.region_args(rs).iter().map(|r| self.unnumbered_region(*r, depth + 1)).collect::<Vec<_>>());
                        TyKind::Dyn(d, tys.mk_list(&ts), tys.mk_list(&bs), rs, r, n, autos)
                    }
                    TyKind::Tuple(ts) => TyKind::Tuple(tys.mk_list(&tys.list(ts).iter().map(|t| self.argument(*t, slots, depth)).collect::<Vec<_>>())),
                    TyKind::Slice(t) => TyKind::Slice(self.argument(t, slots, depth)),
                    TyKind::Array(t, n) => TyKind::Array(self.argument(t, slots, depth), n),
                    TyKind::Ptr(m, t) => TyKind::Ptr(m, self.argument(t, slots, depth)),
                    _ => {
                        // Types without layout slots can still contain free
                        // regions (notably FnPtr). Keep those out of the foreign
                        // typeck context as well, without inventing BIR slots.
                        let this = std::cell::RefCell::new(self);
                        return tys.map_regions(ty, &|r, d| this.borrow_mut().unnumbered_region(r, d), depth);
                    }
                };
                tys.intern(kind)
            }
        }
        impl TypeFolder for Instantiate<'_, '_, '_, '_> {
            fn fold_ty(&mut self, tys: &crate::types::Types, ty: TyId, depth: u32) -> TyId {
                if let TyKind::Param(i) = tys.kind(ty) {
                    if let Some(&actual) = self.p.args.get(*i as usize) {
                        let slots = self.p.ty(ty);
                        let actual = crate::borrowck::universal_regions::with_erased_adt_regions(self.p.rty.tx.tcx, self.p.rty.tx.normalize(actual));
                        let mut slots = slots.iter();
                        let actual = self.argument(actual, &mut slots, 0);
                        self.valid &= slots.next().is_none();
                        return tys.shift_in(actual, depth);
                    }
                }
                super_fold_ty(tys, ty, self, depth)
            }
            fn fold_region(&mut self, tys: &crate::types::Types, r: RegionId, depth: u32) -> RegionId {
                if matches!(tys.region_kind(r), RegionKind::ReBound(d, _) if d < depth) { return r; }
                let slot = self.p.region(r, None); self.region(slot)
            }
        }
        let mut inst = Instantiate { p: self, infcx: &infcx, regions: Vec::new(), valid: true };
        let value = inst.fold_ty(tys, tys.set_aliases_to_non_rigid(ty), 0);
        if !inst.valid { return inst.p.ty(ty); }
        let regions = inst.regions;
        let op = ParamEnvAnd { param_env: tx.param_env_id.get(), value: Normalize { value } };
        let Ok(out) = op.fully_perform(&infcx, tx.prog) else { return self.ty(ty); };
        // The type operation's new regions are existential symbols, and its
        // constraints are caller requirements, never implied assumptions.
        let mut normalized = Projection { rty: self.rty, param_env: tx.param_env_id.get(), args: &[],
            regions, params: Vec::new(), names: std::mem::take(&mut self.names), kinds: std::mem::take(&mut self.kinds) };
        let slots = normalized.ty(out.output);
        if let Some(constraints) = out.constraints {
            for c in constraints.constraints {
                for c in c.constraint.iter_outlives() {
                    match c {
                        OutlivesPredicate::Region(a, b) => cons.push((normalized.region(a, None), normalized.region(b, None))),
                        OutlivesPredicate::Type(t, r) => {
                            let r = normalized.region(r, None);
                            for s in normalized.ty(t) { cons.push((s, r)); }
                        }
                    }
                }
            }
        }
        self.names = normalized.names; self.kinds = normalized.kinds;
        slots
    }
    fn ty(&mut self, t: TyId) -> Vec<SR> {
        let tcx = self.rty.tx.tcx;
        match tcx.tys.kind(t).clone() {
            TyKind::Param(i) => {
                if let Some((_, s)) = self.params.iter().find(|(p, _)| *p == i) {
                    return s.clone();
                }
                let n = self.args.get(i as usize).map_or(0, |t| {
                    self.rty
                        .n_slots(self.rty.tx.normalize(*t))
                });
                let s: Vec<_> = (0..n).map(|_| self.fresh("'_", SK_PARAM)).collect();
                self.params.push((i, s.clone()));
                s
            }
            TyKind::Ref(_, t, r) => {
                let mut s = vec![self.region(r, None)];
                s.extend(self.ty(t));
                s
            }
            TyKind::Ptr(_, t) | TyKind::Array(t, _) | TyKind::Slice(t) => self.ty(t),
            TyKind::Tuple(ts) => tcx.tys.list(ts).iter().flat_map(|t| self.ty(*t)).collect(),
            TyKind::Adt(_, ts, rs) => {
                let mut s = tcx
                    .tys
                    .region_args(rs)
                    .iter()
                    .map(|r| self.region(*r, None))
                    .collect::<Vec<_>>();
                for t in tcx.tys.list(ts).iter().copied() {
                    s.extend(self.ty(t));
                }
                s
            }
            TyKind::Dyn(_, ts, bs, _, r, _, _) => {
                let mut s = vec![self.region(r, None)];
                for t in tcx.tys.list(ts).iter().copied() {
                    s.extend(self.ty(t));
                }
                for (_, t, _) in tcx.tys.list(bs).iter().copied() {
                    s.extend(self.ty(t));
                }
                s
            }
            TyKind::Alias(_, AliasTy { kind: AliasTyKind::Projection { .. }, .. }) => {
                let n = self.rty.tx.normalize_in_env(t, &self.param_env);
                if n != t {
                    self.ty(n)
                } else {
                    Vec::new()
                }
            }
            TyKind::Alias(_, AliasTy { kind: AliasTyKind::Opaque { .. }, args: ts, .. }) => tcx.tys.list(ts).iter().flat_map(|t| self.ty(*t)).collect(),
            // Nested function binders have no slots in the legacy body layout.
            TyKind::FnPtr(..) => Vec::new(),
            _ => Vec::new(),
        }
    }
}

/// Const children have no argument locals, but inherit their owner's assumptions.
/// Their return type is the existing typeck/builder output until A4c's MIR handoff.
pub fn body_template(rty: &Rty<'_>, bir: &crate::borrowck::bir::Bir) -> Option<std::rc::Rc<SigTmpl>> {
    if let Some(child) = bir.checked_body {
        if matches!(
            child.kind,
            crate::borrowck::body::BodyKind::InlineConst(_) | crate::borrowck::body::BodyKind::AnonConst(_)
        ) {
            let mut t = (*rty.sig_template(child.owner, &bir.args)).clone();
            t.inputs.clear();
            t.output.clear();
            if matches!(child.kind, crate::borrowck::body::BodyKind::InlineConst(_)) {
                if let Some(ret) = bir.locals.first() {
                    for _ in 0..rty.n_slots(ret.ty) {
                        t.output.push(SR::Sym(t.n_syms));
                        t.n_syms += 1;
                        t.names.push("'_".into());
                        t.kinds.push(SK_ELIDED);
                    }
                }
            }
            return t.ok.then(|| std::rc::Rc::new(t));
        }
    }
    let def = bir.def?;
    let t = if rty.tx.tcx.fn_item_sig(rty.tx.prog, def).is_some_and(|sig| sig.header.c_variadic) {
        std::rc::Rc::new(signature_template_for(rty,def,&bir.args,true))
    } else { rty.sig_template(def, &bir.args) };
    t.ok.then_some(t)
}

pub fn closure_template(rty: &Rty<'_>, bir: &crate::borrowck::bir::Bir) -> Option<SigTmpl> {
    let ce = bir.closure_expr?;
    let mut u = SignatureRegions::closure(rty, bir.owner?, ce)?;
    // Fn/FnMut borrow their environment; FnOnce receives it by value.
    if let Some(env) = bir.locals.get(1) {
        let tcx = rty.tx.tcx;
        if let TyKind::Ref(m, ct, _) = tcx.tys.kind(env.ty) {
            let scope = RegionScope::Closure {
                owner: bir.owner?,
                expr: ce,
            };
            let r = tcx
                .tys
                .intern_region(RegionKind::ReLateParam(scope, LateParamKind::ClosureEnv));
            u.regions.insert(
                u.fr_fn_body,
                UniversalRegion {
                    identity: Identity::Region(scope, r),
                    class: Classification::Local,
                    name: "'env".into(),
                },
            );
            u.fr_fn_body += 1;
            u.implicit_input = Some(tcx.tys.intern(TyKind::Ref(*m, *ct, r)));
        } else {
            u.implicit_input = Some(env.ty);
        }
    }
    let sig = *rty.closure_signatures.borrow().get(&ce)?;
    let mut p = Projection {
        rty,
        param_env: rty.tx.param_env_id.get(),
        args: &bir.args,
        regions: Vec::new(),
        params: Vec::new(),
        names: Vec::new(),
        kinds: Vec::new(),
    };
    for region in &u.regions {
        if let Identity::Region(_, r) = region.identity {
            p.region(r, Some(&region.name));
        }
    }
    let inputs = u.inputs.iter().map(|t| p.ty(*t)).collect();
    let output = p.ty(u.output);
    // The annotation ascribed onto the closure's signature (rustc
    // type_check/input_output.rs check_signature_annotation): S', its own
    // binder instantiated with fresh variables, equals E', the inputs above.
    let mut equality = Vec::new();
    if let Some((a, b)) = sig.annotation_eq {
        let tys = &rty.tx.tcx.tys;
        if let (TyKind::FnPtr(ap, ar, _, _), TyKind::FnPtr(bp, br, _, _)) =
            (tys.kind(a), tys.kind(b))
        {
            for (a, b) in tys.list(*ap).iter().chain([ar]).zip(tys.list(*bp).iter().chain([br])) {
                let a = p.ty(*a);
                let b = p.ty(*b);
                if a.len() == b.len() {
                    for (a, b) in a.into_iter().zip(b) {
                        equality.push((a, b));
                        equality.push((b, a));
                    }
                }
            }
        }
    }
    let mut t = empty();
    t.ok = true;
    t.inputs = inputs;
    t.output = output;
    t.cons = equality;
    t.tparam_names = u.contract.type_names.clone();
    let raw: Vec<_> = u.inputs.iter().copied().chain([u.output]).chain(u.implicit_input).collect();
    let owner = bir.owner?;
    let (mut relations, _) = crate::borrowck::implied_bounds::mir_borrowck_implied_outlives_bounds(
        rty.tx.tcx,rty.tx.prog,rty.tx.obligation_owner.get(),rty.tx.cx.typing_env.mode,&rty.tx.param_env_id.get(),true,&u.contract.predicates,&raw,&u.contract.assumed_wf)
        .unwrap_or_else(|_| crate::borrowck::implied_bounds::query_failed(rty.tx.tcx,rty.tx.prog,owner,&u.contract.predicates,&raw));
    // External ROOT query variables already have SR identities in this adapter.
    for &r in &u.query_placeholders { p.region(r,None); }
    relations.retain_instantiated_query_bounds(rty.tx.tcx,&u.query_placeholders);
    t.implied = relations
        .outlives
        .iter()
        .map(|(a, b)| (p.region(*a, None), p.region(*b, None)))
        .collect();
    t.tparam_bounds = relations
        .type_outlives
        .iter()
        .filter_map(|(t, r)| {
            if let TyKind::Param(i) = rty.tx.tcx.tys.kind(*t) {
                Some((*i, p.region(*r, None)))
            } else {
                None
            }
        })
        .collect();
    t.assumptions = relations;
    t.n_syms = p.names.len() as u32;
    t.names = p.names;
    t.kinds = p.kinds;
    t.typed_regions = p.regions;
    t.universal = Some(u);
    Some(t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        program::Program,
        tcx::{Tcx, EvalState, TypingEnv},
    };

    #[test]
    fn declaration_universals_and_typed_assumptions() {
        let source = br#"
            #![no_std]
            struct S<T: 'static>(T);
            struct Host<'outer>(&'outer u32);
            impl<'outer> Host<'outer> { fn method<'unused>(&self) {} }
            fn signature<'unused, 'a>(x: &'a u32, f: for<'b> fn(&'a u32, &'b u32)) -> &'a u32 { x }
            fn early<'unused, 'a, T: 'a>(x: &'a T) where 'a: 'static {}
            fn output<'a,T>(x: &'a T) -> S<&'a T> { S(x) }
            fn parent<'unused, 'a>(x: &'a u32) { let c = |y: &u32| {}; let inferred = |z: &u32| { z }; }
        "#;
        let dir = std::env::temp_dir().join(format!("relax-a4b-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("lib.rs");
        std::fs::write(&file, source).unwrap();
        let session = std::sync::Arc::new(crate::Session::default());
        let mut prog = Program::new(&session);
        prog.load_crate(
            "a4b",
            file.to_str().unwrap(),
            2021,
            crate::cfg::CfgSet::host(&session, &[]),
            Vec::new(),
        )
        .unwrap();
        let mut tcx = Tcx::new();
        tcx.collect(&prog);
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(tcx.errors.is_empty(), "{:?}", tcx.errors);
        let def = |n| (0..prog.defs.len() as u32).map(DefId).find(|&d| prog.name(d) == n).unwrap();
        let u = SignatureRegions::function(&tcx,&prog, def("signature")).unwrap();
        assert!(u.regions.iter().any(|u| u.name == "'unused"));
        assert_eq!(u.regions[u.fr_static].class, Classification::Global);
        assert!(matches!(
            u.regions[u.fr_fn_body].identity,
            Identity::FnBody(_)
        ));
        let TyKind::Ref(_, _, a) = tcx.tys.kind(u.inputs[0]) else {
            panic!()
        };
        assert!(matches!(
            tcx.tys.region_kind(*a),
            RegionKind::ReLateParam(RegionScope::Item(_), _)
        ));
        assert!(matches!(tcx.tys.kind(u.output),TyKind::Ref(_,_,r) if r==a));
        let TyKind::FnPtr(ps, _, _, 1) = tcx.tys.kind(u.inputs[1]) else {
            panic!()
        };
        let ps = tcx.tys.list(*ps);
        assert!(matches!(tcx.tys.kind(ps[0]),TyKind::Ref(_,_,r) if r==a));
        assert!(
            matches!(tcx.tys.kind(ps[1]),TyKind::Ref(_,_,r) if matches!(tcx.tys.region_kind(*r),RegionKind::ReBound(0,_)))
        );
        let method = SignatureRegions::function(&tcx,&prog, def("method")).unwrap();
        assert!(method.regions.iter().any(|u| u.name == "'outer"));
        assert!(method.regions.iter().any(|u| u.name == "'unused"));
        let early = SignatureRegions::function(&tcx,&prog, def("early")).unwrap();
        assert!(early.contract.predicates.iter().any(|p|matches!(p,OutlivesPredicate::Region(_,r) if tcx.tys.region_kind(*r)==RegionKind::ReStatic)));
        let out = SignatureRegions::function(&tcx,&prog, def("output")).unwrap();
        let raw = out
            .inputs
            .iter()
            .copied()
            .chain([out.output])
            .collect::<Vec<_>>();
        let rel =
            ImpliedBounds::create(&tcx,&prog, &out.contract.predicates, &raw, &raw, Vec::new());
        let TyKind::Ref(_, _, a) = tcx.tys.kind(out.inputs[0]) else {
            panic!()
        };
        let st = tcx.tys.intern_region(RegionKind::ReStatic);
        assert!(
            rel.outlives.contains(&(*a, st)),
            "return type WF must be an assumption"
        );
        assert!(rel
            .type_outlives
            .iter()
            .any(|(t, r)| matches!(tcx.tys.kind(*t), TyKind::Param(0)) && *r == st));
        let obligations = vec![OutlivesPredicate::Region(st, *a)];
        let isolated = ImpliedBounds::create(&tcx,&prog, &[], &[], &[], obligations.clone());
        assert_eq!(isolated.obligations, obligations);
        assert!(isolated.outlives.is_empty());
        let alias = tcx.tys.intern(TyKind::Alias(crate::consts::IsRigid::No, AliasTy { kind: AliasTyKind::Projection { def_id: def("output") }, args: tcx.tys.mk_list(&[tcx.tys.intern(TyKind::Param(0))]), regions: crate::types::RegionArgsId::EMPTY }));
        let alias_rel = ImpliedBounds::create(
            &tcx, &prog,
            &[OutlivesPredicate::Type(alias, *a)],
            &[],
            &[],
            Vec::new(),
        );
        assert_eq!(
            alias_rel.type_outlives,
            vec![(alias, *a)],
            "alias subjects must survive until constraint conversion"
        );
        let normalized_only = ImpliedBounds::create(&tcx,&prog, &[], &[], &raw, Vec::new());
        assert!(normalized_only.outlives.contains(&(*a, st)));
        let state = EvalState::new();
        let owner = def("parent");
        // Borrowck consumes the definition-owned result; codegen instantiation
        // erases the very closure regions this assertion inspects.
        let body = tcx.typeck(&prog, owner);
        assert!(body.errors.is_empty(), "{:?}", body.errors);
        let tx = crate::borrowck::tyx::Tx::new(&prog, &tcx, TypingEnv::fully_monomorphized(&prog), &state);
        let rty = Rty::new(&tx);
        *rty.closure_signatures.borrow_mut() = body.closure_sigs.clone();
        let inferred = body.closure_sigs[&body.closures[1]];
        let TyKind::FnPtr(_, ret, _, _) = tcx.tys.kind(inferred.liberated) else {
            panic!()
        };
        assert!(
            matches!(tcx.tys.kind(*ret),TyKind::Ref(_,_,r) if matches!(tcx.tys.region_kind(*r),RegionKind::ReVar(_))),
            "inferred output must not capture an input-only late lifetime"
        );
        let closure = SignatureRegions::closure(&rty, owner, body.closures[0]).unwrap();
        assert!(closure
            .regions
            .iter()
            .any(|r| r.name == "'unused" && r.class == Classification::External));
        assert!(closure.regions.iter().any(|r| matches!(
            r.identity,
            Identity::Region(RegionScope::Closure { .. }, _)
        ) && r.class == Classification::Local));
    }
}

#[cfg(test)]
mod round33_tests {
    use super::*;
    use crate::{program::Program, tcx::Tcx};

    /// D7 / N:rustc_borrowck/src/implied_bounds.rs:59,95 + free_region_relations.rs:523-529:
    /// a signature type that fails to normalize fails the whole query; the caller keeps the
    /// declared predicates and the unnormalized signature and records a delayed bug.
    #[test]
    fn round33_implied_bounds_failure_fails_query() {
        let source = br#"
            #![no_std]
            trait Tr { type A; }
            fn good<'a, T: Tr>(x: &'a T, y: <T as Tr>::A) where T: 'a {}
            fn bad<'a, T: 'a>(x: &'a T, y: <T as Tr>::A) {}
        "#;
        let dir = std::env::temp_dir().join(format!("relax-r33-ib-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("lib.rs");
        std::fs::write(&file, source).unwrap();
        let session = std::sync::Arc::new(crate::Session::default());
        let mut prog = Program::new(&session);
        prog.load_crate("r33ib", file.to_str().unwrap(), 2021, crate::cfg::CfgSet::host(&session, &[]), Vec::new()).unwrap();
        let mut tcx = Tcx::new();
        tcx.collect(&prog);
        std::fs::remove_dir_all(&dir).unwrap();
        let def = |n| (0..prog.defs.len() as u32).map(DefId).find(|&d| prog.name(d) == n).unwrap();
        let query = |name| {
            let d = def(name);
            let u = SignatureRegions::function(&tcx, &prog, d).unwrap();
            let raw: Vec<_> = u.inputs.iter().copied().chain([u.output]).collect();
            let env = tcx.param_env_id(&prog, d);
            (d, raw.clone(), u.contract.predicates.clone(), crate::borrowck::implied_bounds::mir_borrowck_implied_outlives_bounds(
                &tcx, &prog, d, crate::tcx::TypingMode::non_body_analysis(), &env, false, &u.contract.predicates, &raw, &u.contract.assumed_wf))
        };
        let (_, _, _, good) = query("good");
        let (bounds, normalized) = good.expect("a normalizable signature");
        assert_eq!(normalized.len(), 3);
        assert!(!bounds.outlives.is_empty() || !bounds.type_outlives.is_empty());
        let (d, raw, predicates, bad) = query("bad");
        assert!(bad.is_err(), "an unprovable projection fails the query");
        let (fallback, signature) = crate::borrowck::implied_bounds::query_failed(&tcx, &prog, d, &predicates, &raw);
        assert_eq!(signature, raw, "the unnormalized signature");
        let declared = ImpliedBounds::create(&tcx, &prog, &predicates, &[], &[], Vec::new());
        assert_eq!(fallback.outlives, declared.outlives);
        assert_eq!(fallback.type_outlives, declared.type_outlives, "only the declared predicates, no implied bounds");
        // rustc_errors::DiagCtxt: delayed bugs flush at session end, and only
        // become an ICE when the session has not reported an error.
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| tcx.dcx.flush_delayed())).is_err());
        let _ = crate::borrowck::implied_bounds::query_failed(&tcx, &prog, d, &predicates, &raw);
        tcx.dcx.report(Default::default(), "E0277".into());
        tcx.dcx.flush_delayed();
    }
}

/// The free subset of a closure input; bound positions are local to its call.
/// Nightly represents these as captured generic args. BIR carries slot indices.
pub fn external_closure_input_slots(rty: &Rty<'_>, expr: u32, input: usize) -> Vec<usize> {
    let sigs = rty.closure_signatures.borrow();
    let Some(sig) = sigs.get(&expr) else { return vec![]; };
    let TyKind::FnPtr(ps, _, _, _) = rty.tx.tcx.tys.kind(sig.bound) else { return vec![]; };
    let Some(&ty) = rty.tx.tcx.tys.list(*ps).get(input) else { return vec![]; };
    let mut projection = Projection { rty,param_env:rty.tx.param_env_id.get(),args:&[],regions:vec![],params:vec![],names:vec![],kinds:vec![] };
    let tys = &rty.tx.tcx.tys;
    let ty = crate::borrowck::universal_regions::with_erased_adt_regions(rty.tx.tcx, ty);
    let ty = tys.map_regions(ty,&|r,depth| match tys.region_kind(r) {
        RegionKind::ReBound(d,v) if d > depth => tys.intern_region(RegionKind::RePlaceholder { universe:crate::types::UniverseIndex::ROOT,var:v }),
        _ => r,
    },0);
    let ty = crate::borrowck::implied_bounds::replace_erased_regions_with_placeholders(rty.tx.tcx,&[ty])[0];
    projection.ty(ty).iter().enumerate().filter_map(|(i,r)| (!matches!(r, SR::Fresh)).then_some(i)).collect()
}
