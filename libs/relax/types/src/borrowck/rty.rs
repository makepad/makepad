//! Regions for Relax's erased types (plans/relax-borrowck.md section 3).
//!
//! Every type gets a *skeleton*: its region slots in canonical walk order with the variance of
//! each slot (Ref = [r] + pointee, Adt = its lifetime params + its type args, Dyn = [bound] +
//! args, Closure = captures + return type). Region facts come from the AST, which keeps every
//! lifetime: an AST type is instantiated against the erased TyId typeck produced (the TyId gives
//! the structure and slot count, the AST the lifetime names). ADT variance is computed from the
//! declaration types through Tcx::variances_of.

pub mod signature;
use crate::types::AliasTyKind;
use crate::types::{AliasTy};
use super::tyx::Tx;
use crate::ast::{self, GenericArg, GenericArgs, GenericParamKind, ItemKind, SegKind, Ty as AstTy};
use crate::fxhash::HashMap;
use crate::program::{DefId, DefKind, Sym};
use crate::types::{TyId, TyKind};
use std::cell::RefCell;
use std::rc::Rc;

use crate::types::variance::{V, xform};

/// A region slot of an instantiated type: a symbol of a template / a parent's slot, 'static,
/// or a fresh existential (unconstrained) region.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum SR {
    Sym(u32),
    Static,
    Fresh,
}

pub struct AdtInfo {
    pub lt_names: Vec<Sym>,
    /// variance per lifetime param, per type param (in the ADT's type-arg order)
    pub var_lt: Vec<V>,
    pub var_ty: Vec<V>,
}

/// Captures (types as stored in the closure: T or &T / &mut T) and return type of a closure
/// expression of this body (filled by the builder).
#[derive(Clone)]
pub struct ClosureSkel {
    pub caps: Vec<TyId>,
    /// External positions within each parameter, including mixed free/bound inputs.
    /// (parameter index, type, region-slot indices).
    pub params: Vec<(usize, TyId, Vec<usize>)>,
    pub ret: TyId,
}

pub struct Rty<'a> {
    pub tx: &'a Tx<'a>,
    skel: RefCell<HashMap<TyId, Rc<Vec<V>>>>,
    adts: RefCell<HashMap<DefId, Rc<AdtInfo>>>,
    fields: RefCell<HashMap<(TyId, u32, u32), Rc<Vec<SR>>>>,
    pub closures: RefCell<HashMap<(u32, u32), ClosureSkel>>,
    pub closure_signatures: RefCell<crate::typeck::SideTable<u32, crate::typeck::ClosureSignatures>>,
    pub closure_parents: RefCell<HashMap<u32, Option<u32>>>,
    /// per closure expression: the owner's named lifetimes its body requires its external
    /// regions to outlive (a closure requirement `'c: 'a` whose outlived region is a free region
    /// of the owner, rustc ClosureOutlivesRequirement::outlived_free_region); a requirement's
    /// second index NAMED_REQ + i names entry i
    pub closure_req_names: RefCell<HashMap<u32, Vec<String>>>,
    templates: RefCell<HashMap<(DefId, Vec<TyId>), Rc<SigTmpl>>>,
    ann_calls: std::cell::Cell<u64>,
    copy_static: RefCell<HashMap<TyId, Rc<Vec<usize>>>>,
    trait_static_memo: RefCell<HashMap<DefId, bool>>,
    /// the fn whose body is being checked (lowering written projection types in annotations)
    pub cur_owner: std::cell::Cell<Option<DefId>>,
    /// the checked fn's impl self type regions (`Self` in its body), as region variables
    pub cur_self: RefCell<Option<Vec<u32>>>,
    manually_drop: Option<DefId>,
}

/// The instantiated signature of a fn instance over symbols: per input its slots, the output's
/// slots, outlives requirements (where-clauses, WF of `&'r T`, opaque captures) and the implied
/// bounds of the inputs (known relations inside the fn itself).
#[derive(Clone)]
pub struct SigTmpl {
    pub assumptions: super::implied_bounds::ImpliedBounds,
    pub universal: Option<super::rty::signature::SignatureRegions>,
    pub typed_regions: Vec<(crate::types::RegionId, SR)>,
    pub n_syms: u32,
    /// per symbol: a readable name ('a, '_, T's)
    pub names: Vec<String>,
    /// per symbol: named / elided lifetime of the signature (universal in the fn itself)
    pub kinds: Vec<u8>,
    pub inputs: Vec<Vec<SR>>,
    pub output: Vec<SR>,
    pub cons: Vec<(SR, SR)>,
    pub implied: Vec<(SR, SR)>,
    /// the input index whose first slot is the `&self` reference (output elision)
    pub ok: bool,
    /// the fn's own type parameters in order: their slots and erased type (None: a const
    /// param or not in the signature)
    pub own_targs: Vec<Option<(Vec<SR>, TyId)>>,
    /// the trait's type arguments after Self (a trait method: its own generics; a trait impl's
    /// method: the impl's trait ref arguments)
    pub trait_targs: Vec<Option<(Vec<SR>, TyId)>>,
    /// the trait's lifetime arguments (a trait item: the trait's lifetime params; a trait impl's
    /// item: the lifetimes written on the impl's trait ref)
    pub trait_lts: Vec<SR>,
    /// lifetime bounds of the type params (`T: 'a`, `T: 'static`, `for<'a> T: 'a`, a trait
    /// with a `'static` supertrait): (index in the generics list = TyKind::Param, region)
    pub tparam_bounds: Vec<(u32, SR)>,
    pub tparam_names: Vec<String>,
    /// the bounds a caller must prove (tparam_bounds without a trait impl's own generics)
    pub call_bounds: Vec<(u32, SR)>,
    /// per own_targs entry: its index in the generics list (TyKind::Param)
    pub own_targ_index: Vec<Option<u32>>,
    /// the fn has lifetime where-clauses on projections (`T::Item: 'a`): type tests through
    /// projections are not decided (this checker does not resolve those clauses)
    pub proj_bounds: bool,
    /// an impl method: the impl's self type and its slots (UFCS annotations `A::<'a>::f`)
    pub self_ty: Option<TyId>,
    pub self_slots: Vec<SR>,
}

impl SigTmpl {
    /// The regions a fn item VALUE carries (rustc: the regions of FnDef's generic arguments;
    /// late-bound lifetimes are instantiated per call and are not among them): the self type's
    /// slots, then the own type arguments' slots (the trait's lifetime arguments first).
    pub fn layout(&self) -> Vec<SR> {
        if !self.ok {
            return Vec::new();
        }
        let mut v = self.trait_lts.clone();
        v.extend(self.self_slots.iter().copied());
        for o in self.trait_targs.iter().flatten().chain(self.own_targs.iter().flatten()) {
            v.extend(o.0.iter().copied());
        }
        v
    }
}

/// A region of a user type annotation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ann {
    Static,
    /// one of the given named lifetimes
    Rv(u32),
    /// a named lifetime not given (a closure's parent's)
    Named(String),
    Fresh,
    /// an unnamed lifetime that several slots share (an alias's elided parameter: `P<u32>` for
    /// `type P<'a, T> = (&'a T, &'a T)`): one fresh region for all of them (unique id)
    Shared(u64),
}

/// A closure requirement's outlived region is one of the owner's named lifetimes (index base).
pub const NAMED_REQ: u32 = 0x8000_0000;

pub const SK_NAMED: u8 = 0;
pub const SK_ELIDED: u8 = 1;
pub const SK_PARAM: u8 = 2;
pub const SK_FRESH: u8 = 3;

/// Instantiation environment.
struct IEnv<'e> {
    file: u32,
    module: u32,
    lts: Vec<(Vec<u8>, SR)>,
    /// type params by name: their slots (lazily from the erased type) or deferred to an outer
    /// AST type (type aliases)
    tparams: Vec<(Vec<u8>, TP)>,
    self_slots: Option<Vec<SR>>,
    elide: Elide,
    /// syms allocated (counter shared through the template)
    next: &'e RefCell<u32>,
    names: &'e RefCell<Vec<String>>,
    kinds: &'e RefCell<Vec<u8>>,
    /// collected: (inner slot, ref region) for every `&'r T` (T: 'r)
    wf: Vec<(SR, SR)>,
    /// regions seen (input positions, for output elision)
    seen: Vec<SR>,
    /// opaque captures (`impl Trait` in return position): (captured, hidden slot)
    caps: Vec<(SR, SR)>,
    /// APIT counter (`impl Trait` params are anonymous generic params)
    apit_base: usize,
    apit_n: usize,
    /// a type written in a fn body (let / cast / turbofish annotation): an object lifetime
    /// with no default from its context is inferred, not 'static (RFC 1156)
    expr_mode: bool,
    /// the impl whose `Self::X` assoc types resolve in this env
    impl_def: Option<DefId>,
    /// all erased generic args of the instance (APIT lookups)
    edition: u16,
}

#[derive(Clone)]
enum TP {
    Slots(Vec<SR>),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Elide {
    /// the output's elided lifetimes: this region
    Fixed(SR),
    /// unconstrained (bodies, unknown)
    Fresh,
}

impl<'a> Rty<'a> {
    pub fn new(tx: &'a Tx<'a>) -> Rty<'a> {
        Rty {
            tx,
            skel: RefCell::new(HashMap::default()),
            adts: RefCell::new(HashMap::default()),
            fields: RefCell::new(HashMap::default()),
            closures: RefCell::new(HashMap::default()),
            closure_signatures: RefCell::new(crate::typeck::SideTable::new()),
            closure_parents: RefCell::new(HashMap::default()),
            closure_req_names: RefCell::new(HashMap::default()),
            templates: RefCell::new(HashMap::default()),
            ann_calls: std::cell::Cell::new(0),
            copy_static: RefCell::new(HashMap::default()),
            trait_static_memo: RefCell::new(HashMap::default()),
            cur_owner: std::cell::Cell::new(None),
            cur_self: RefCell::new(None),
            manually_drop: tx.lang("mem::ManuallyDrop"),
        }
    }

    pub fn is_manually_drop(&self, d: DefId) -> bool {
        Some(d) == self.manually_drop
    }

    // ------------------------------------------------------------------ skeletons

    /// Variance per region slot of `t` (its length is the slot count).
    pub fn skel(&self, t: TyId) -> Rc<Vec<V>> {
        if let Some(s) = self.skel.borrow().get(&t) {
            return s.clone();
        }
        let mut v = Vec::new();
        self.skel_into(t, V::Co, &mut v, 0);
        let r = Rc::new(v);
        self.skel.borrow_mut().insert(t, r.clone());
        r
    }

    pub fn n_slots(&self, t: TyId) -> usize {
        self.skel(t).len()
    }

    fn skel_into(&self, t: TyId, ctx: V, out: &mut Vec<V>, depth: u32) {
        if depth > 40 {
            return;
        }
        match self.tx.kind(t) {
            TyKind::Ref(m, inner, _) => {
                out.push(xform(ctx, V::Co));
                self.pointee_skel(inner, ctx, m, out, depth);
            }
            TyKind::Ptr(m, inner) => self.pointee_skel(inner, ctx, m, out, depth),
            TyKind::Array(e, _) | TyKind::Slice(e) => self.skel_into(e, ctx, out, depth + 1),
            TyKind::Tuple(v) => {
                for x in self.tx.tcx.tys.list(v).iter().copied() {
                    self.skel_into(x, ctx, out, depth + 1);
                }
            }
            TyKind::Adt(d, args, _) => {
                let info = self.adt_info(d);
                for v in &info.var_lt {
                    out.push(xform(ctx, *v));
                }
                for (i, a) in self.tx.tcx.tys.list(args).iter().enumerate() {
                    let v = info.var_ty.get(i).copied().unwrap_or(V::Inv);
                    self.skel_into(*a, xform(ctx, v), out, depth + 1);
                }
            }
            TyKind::Dyn(_, args, binds, _, _, _, _) => {
                out.push(xform(ctx, V::Co));
                for a in self.tx.tcx.tys.list(args).iter().copied() {
                    self.skel_into(a, xform(ctx, V::Inv), out, depth + 1);
                }
                for (_, b, _) in self.tx.tcx.tys.list(binds).iter().copied() {
                    self.skel_into(b, xform(ctx, V::Inv), out, depth + 1);
                }
            }
            TyKind::Closure(file, ce, crate::types::ClosureArgs { tupled_upvars_ty: up, .. }) => {
                let cs = self.closures.borrow().get(&(file, ce)).cloned();
                match cs {
                    Some(cs) => {
                        for c in &cs.caps {
                            self.skel_into(*c, xform(ctx, V::Inv), out, depth + 1);
                        }
                        for (_, p, slots) in &cs.params {
                            let mut all = vec![];
                            self.skel_into(*p, xform(ctx, V::Inv), &mut all, depth + 1);
                            out.extend(slots.iter().map(|&i| all[i]));
                        }
                        self.skel_into(cs.ret, xform(ctx, V::Inv), out, depth + 1);
                    }
                    None => self.skel_into(up, xform(ctx, V::Inv), out, depth + 1),
                }
            }
            TyKind::Alias(_, AliasTy { kind: AliasTyKind::Opaque { .. }, args, regions: _ }) => {
                for a in self.tx.tcx.tys.list(args).iter().copied() {
                    self.skel_into(a, xform(ctx, V::Inv), out, depth + 1);
                }
            }
            // a tuple constructor's fn item: the slots of the ADT it builds, invariant (its
            // args are both parameter and return types; rustc: the user annotation's regions)
            TyKind::FnDef(d, args, _) if matches!(self.tx.prog.def(d).kind, crate::program::DefKind::Struct | crate::program::DefKind::Variant) => {
                let ad = if self.tx.prog.def(d).kind == crate::program::DefKind::Variant { self.tx.prog.def(d).parent } else { d };
                let at = self.tx.tcx.tys.intern(TyKind::Adt(ad, args, crate::types::RegionArgsId::EMPTY));
                self.skel_into(at, xform(ctx, V::Inv), out, depth + 1);
            }
            // a fn item: the regions of its generic arguments (SigTmpl::layout), invariant
            TyKind::FnDef(d, args, _) if matches!(self.tx.prog.def(d).kind, crate::program::DefKind::Fn | crate::program::DefKind::AssocFn) => {
                let n = self.fn_value_slots(d, self.tx.tcx.tys.list(args));
                for _ in 0..n {
                    out.push(xform(ctx, V::Inv));
                }
            }
            _ => {}
        }
    }

    /// The number of regions a fn item value carries: those of its generic arguments, in the
    /// order of SigTmpl::layout (self type, trait arguments, own type arguments). Computed from
    /// the def's generics alone, never from its signature (rustc: FnDef(def, args) folds over
    /// its GenericArgs only, rustc_borrowck/src/renumber.rs).
    pub fn fn_value_slots(&self, d: DefId, args: &[TyId]) -> usize {
        self.sig_template(d, args).layout().len()
    }

    /// A pointee: `&mut T` is invariant in T, except that a trait object's lifetime bound may
    /// shrink (rustc's unsizing coercion `dyn Tr + 'long` -> `dyn Tr + 'short` behind any
    /// pointer, applied at every coercion site).
    fn pointee_skel(&self, inner: TyId, ctx: V, m: bool, out: &mut Vec<V>, depth: u32) {
        if let TyKind::Dyn(_, args, binds, _, _, _, _) = self.tx.kind(inner) {
            out.push(xform(ctx, V::Co));
            for a in self.tx.tcx.tys.list(args).iter().copied() {
                self.skel_into(a, xform(ctx, V::Inv), out, depth + 1);
            }
            for (_, b, _) in self.tx.tcx.tys.list(binds).iter().copied() {
                self.skel_into(b, xform(ctx, V::Inv), out, depth + 1);
            }
            return;
        }
        let s0 = out.len();
        self.skel_into(inner, xform(ctx, if m { V::Inv } else { V::Co }), out, depth + 1);
        // an unsized struct ending in a trait object: the object's bound may shrink too
        // (struct unsizing `S<dyn Tr + 'long>` -> `S<dyn Tr + 'short>`)
        if matches!(self.tx.kind(inner), TyKind::Adt(..)) && self.tx.is_unsized(inner) {
            let mut idx = Vec::new();
            self.dyn_obj_slots(inner, 0, &mut idx, 0);
            for i in idx {
                if let Some(v) = out.get_mut(s0 + i) {
                    *v = xform(ctx, V::Co);
                }
            }
        }
    }

    /// Slot indices (within `t`'s skeleton) of trait objects' bounds.
    fn dyn_obj_slots(&self, t: TyId, base: usize, out: &mut Vec<usize>, depth: u32) {
        if depth > 20 {
            return;
        }
        match self.tx.kind(t) {
            TyKind::Dyn(..) => out.push(base),
            TyKind::Adt(d, args, _) => {
                let mut off = base + self.adt_info(d).var_lt.len();
                for a in self.tx.tcx.tys.list(args).iter().copied() {
                    self.dyn_obj_slots(a, off, out, depth + 1);
                    off += self.n_slots(a);
                }
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------------ ADTs

    fn adt_generics(&self, d: DefId) -> Option<(u32, &'a ast::Generics)> {
        let def = self.tx.prog.def(d);
        let ast_ = &self.tx.prog.files[def.file as usize].ast;
        let it = ast_.item(def.item);
        match &it.kind {
            ItemKind::Struct(r) => Some((def.file, &ast_.row(*r).generics)),
            ItemKind::Union(r) => Some((def.file, &ast_.row(*r).generics)),
            ItemKind::Enum(r) => Some((def.file, &ast_.row(*r).generics)),
            ItemKind::TypeAlias(r) if ast_.row(*r).ty.is_some() => Some((def.file, &ast_.row(*r).generics)),
            _ => None,
        }
    }

    fn text(&self, file: u32, id: ast::Ident) -> &'a str {
        self.tx.prog.files[file as usize].text(id)
    }

    pub fn adt_info(&self, d: DefId) -> Rc<AdtInfo> {
        if let Some(a) = self.adts.borrow().get(&d) {
            return a.clone();
        }
        // Names serve the transitional slot adapter; variance comes exclusively
        // from the collected declaration types and their index metadata.
        let lt_names = self.adt_generics(d).map_or_else(Vec::new, |(file, g)| {
            self.tx.prog.files[file as usize].ast.list(g.params).iter()
                .filter(|p| matches!(p.kind, GenericParamKind::Lifetime(_)))
                .map(|p| self.tx.prog.syms.get(self.text(file, p.name)).unwrap_or(u32::MAX))
                .collect()
        });
        let (var_lt, var_ty) = self.tx.tcx.adts.get(&d).map_or_else(
            || (Vec::new(), Vec::new()),
            |adt| {
                let vs = self.tx.tcx.variances_of(self.tx.prog, d);
                let indices = &adt.generic_indices;
                (indices.lifetimes.iter().map(|&i| vs[i as usize]).collect(),
                 indices.types_consts.iter().map(|&i| vs[i as usize]).collect())
            },
        );
        let a = Rc::new(AdtInfo { lt_names, var_lt, var_ty });
        self.adts.borrow_mut().insert(d, a.clone());
        a
    }

    // ------------------------------------------------------------------ fields

    /// The slots of field `fi` of variant `vi` of ADT type `t`, as references to `t`'s slots
    /// (SR::Sym(i) = t's slot i).
    pub fn field_slots(&self, t: TyId, vi: u32, fi: u32) -> Rc<Vec<SR>> {
        if let Some(r) = self.fields.borrow().get(&(t, vi, fi)) {
            return r.clone();
        }
        let r = Rc::new(self.field_slots_uncached(t, vi, fi));
        self.fields.borrow_mut().insert((t, vi, fi), r.clone());
        r
    }

    fn field_slots_uncached(&self, t: TyId, vi: u32, fi: u32) -> Vec<SR> {
        let ft = match self.tx.field_ty(t, vi, fi) {
            Some(x) => x,
            None => return Vec::new(),
        };
        let n = self.n_slots(ft);
        match self.tx.kind(t) {
            TyKind::Tuple(v) => {
                let mut off = 0;
                for x in self.tx.tcx.tys.list(v).iter().take(fi as usize) {
                    off += self.n_slots(*x);
                }
                (0..n).map(|i| SR::Sym((off + i) as u32)).collect()
            }
            TyKind::Adt(d, args, _) => {
                let info = self.adt_info(d);
                let def = self.tx.prog.def(d);
                let ast_ = &self.tx.prog.files[def.file as usize].ast;
                let it = ast_.item(def.item);
                let fast: Option<ast::TyId> = match &it.kind {
                    ItemKind::Struct(r) => vd_fields(ast_, &ast_.row(*r).data).get(fi as usize).copied(),
                    ItemKind::Union(r) => ast_.list(ast_.row(*r).fields).get(fi as usize).map(|f| f.ty),
                    ItemKind::Enum(r) => ast_.list(ast_.row(*r).variants).get(vi as usize).and_then(|v| vd_fields(ast_, &v.data).get(fi as usize).copied()),
                    _ => None,
                };
                let fast = match fast {
                    Some(x) => x,
                    None => return vec![SR::Fresh; n],
                };
                // env: lifetime params -> t's first slots, type params -> the args' ranges
                let (gfile, g) = match self.adt_generics(d) {
                    Some(x) => x,
                    None => return vec![SR::Fresh; n],
                };
                let mut lts = Vec::new();
                let mut li = 0u32;
                let mut tparams = Vec::new();
                let mut off = info.var_lt.len();
                let mut ai = 0usize;
                for p in self.tx.prog.files[gfile as usize].ast.list(g.params) {
                    match &p.kind {
                        GenericParamKind::Lifetime(_) => {
                            lts.push((self.text(gfile, p.name).as_bytes().to_vec(), SR::Sym(li)));
                            li += 1;
                        }
                        _ => {
                            let a = self.tx.tcx.tys.list(args).get(ai).copied();
                            ai += 1;
                            let k = a.map_or(0, |a| self.n_slots(a));
                            tparams.push((self.text(gfile, p.name).as_bytes().to_vec(), TP::Slots((0..k).map(|i| SR::Sym((off + i) as u32)).collect())));
                            off += k;
                        }
                    }
                }
                let next = RefCell::new(0u32);
                let names = RefCell::new(Vec::new());
                let kinds = RefCell::new(Vec::new());
                let mut env = IEnv {
                    file: def.file,
                    module: def.scope,
                    lts,
                    tparams,
                    self_slots: None,
                    elide: Elide::Fresh,
                    next: &next,
                    names: &names,
                    kinds: &kinds,
                    wf: Vec::new(),
                    seen: Vec::new(),
                    caps: Vec::new(),
                    apit_base: usize::MAX,
                    apit_n: 0,
            expr_mode: false,
                    impl_def: None,
                    edition: self.tx.prog.files[def.file as usize].edition,
                };
                let mut out = Vec::new();
                self.inst(&mut env, fast, ft, SR::Static, &mut out, 0);
                if out.len() != n {
                    return vec![SR::Fresh; n];
                }
                out
            }
            TyKind::Closure(..) => {
                // closure captures: consecutive ranges
                let mut off = 0usize;
                let cs = match self.tx.kind(t) {
                    TyKind::Closure(file, ce, ..) => self.closures.borrow().get(&(file, ce)).cloned(),
                    _ => None,
                };
                if let Some(cs) = cs {
                    for c in cs.caps.iter().take(fi as usize) {
                        off += self.n_slots(*c);
                    }
                }
                (0..n).map(|i| SR::Sym((off + i) as u32)).collect()
            }
            _ => vec![SR::Fresh; n],
        }
    }

    // ------------------------------------------------------------------ instantiation

    fn new_sym(&self, env: &IEnv, name: String, kind: u8) -> SR {
        let mut n = env.next.borrow_mut();
        let s = *n;
        *n += 1;
        env.names.borrow_mut().push(name);
        env.kinds.borrow_mut().push(kind);
        SR::Sym(s)
    }

    fn fill_fresh(&self, env: &IEnv, t: TyId, out: &mut Vec<SR>) {
        let n = self.n_slots(t);
        for _ in 0..n {
            let s = self.new_sym(env, "'_".to_string(), SK_FRESH);
            out.push(s);
        }
    }

    fn lifetime(&self, env: &mut IEnv, l: Option<ast::Ident>) -> SR {
        let name = l.map(|l| self.text(env.file, l));
        match name {
            Some("'static") => SR::Static,
            Some(n) if n != "'_" => {
                let nb = n.as_bytes();
                match env.lts.iter().find(|x| x.0.as_slice() == nb) {
                    Some((_, r)) => *r,
                    None => {
                        // an undeclared (late-bound from an in-band context / for<'a>) lifetime:
                        // its own symbol
                        let r = self.new_sym(env, n.to_string(), SK_NAMED);
                        env.lts.push((nb.to_vec(), r));
                        r
                    }
                }
            }
            _ => match env.elide {
                Elide::Fixed(r) => r,
                Elide::Fresh => self.new_sym(env, "'_".to_string(), SK_FRESH),
            },
        }
    }

    fn inst(&self, env: &mut IEnv, at: ast::TyId, t: TyId, obj: SR, out: &mut Vec<SR>, depth: u32) {
        let start = out.len();
        let want = self.n_slots(t);
        if depth > 30 {
            self.fill_fresh(env, t, out);
            return;
        }
        self.inst_inner(env, at, t, obj, out, depth);
        if out.len() - start != want {
            out.truncate(start);
            self.fill_fresh(env, t, out);
        }
        for r in &out[start..] {
            if !env.seen.contains(r) {
                env.seen.push(*r);
            }
        }
    }

    fn inst_inner(&self, env: &mut IEnv, at: ast::TyId, t: TyId, obj: SR, out: &mut Vec<SR>, depth: u32) {
        let file = env.file;
        let ast_ = &self.tx.prog.files[file as usize].ast;
        let k = self.tx.kind(t);
        match (ast_.ty(at).clone(), k) {
            (AstTy::Paren(x), _) => self.inst(env, x, t, obj, out, depth + 1),
            (AstTy::Ref(lt, _, x), TyKind::Ref(_, inner, _)) => {
                let r = self.lifetime(env, lt);
                out.push(r);
                let s = out.len();
                // In a body, an inferred reference lifetime supplies no object default:
                // `&dyn Trait` infers two regions, not one shared region. Named lifetimes
                // still default the object bound (resolve_bound_vars.rs:811-817).
                let obj = if env.expr_mode && matches!(env.elide, Elide::Fresh) && lt.map_or(true, |l| self.text(file, l) == "'_") {
                    SR::Fresh
                } else { r };
                self.inst(env, x, inner, obj, out, depth + 1);
                for i in s..out.len() {
                    env.wf.push((out[i], r));
                }
            }
            (AstTy::Ptr(_, x), TyKind::Ptr(_, inner)) => self.inst(env, x, inner, SR::Static, out, depth + 1),
            (AstTy::Slice(x), TyKind::Slice(e)) | (AstTy::Array(x, _), TyKind::Array(e, _)) => self.inst(env, x, e, obj, out, depth + 1),
            (AstTy::Tuple(v), TyKind::Tuple(es)) if v.len() == self.tx.tcx.tys.list(es).len() => {
                for (x, e) in ast_.list(v).iter().zip(self.tx.tcx.tys.list(es).iter()) {
                    self.inst(env, *x, *e, obj, out, depth + 1);
                }
            }
            (AstTy::DynTrait(bounds, _), TyKind::Dyn(td, args, binds, _, _, _, _)) => {
                let mut bound = None;
                for b in ast_.list(bounds) {
                    if let ast::Bound::Lifetime(l) = b {
                        bound = Some(self.lifetime(env, Some(*l)));
                    }
                }
                // the default object lifetime: the trait's own lifetime bound (`trait Any:
                // 'static`), else the enclosing reference's
                let bound = bound.or_else(|| if self.trait_static_bound(td) { Some(SR::Static) } else { None });
                let obj = if env.expr_mode && obj == SR::Static { self.new_sym(env, "'_".to_string(), SK_FRESH) } else { obj };
                out.push(bound.unwrap_or(obj));
                for a in self.tx.tcx.tys.list(args).iter().copied() {
                    self.fill_fresh(env, a, out);
                }
                for (_, b, _) in self.tx.tcx.tys.list(binds).iter().copied() {
                    self.fill_fresh(env, b, out);
                }
            }
            (AstTy::ImplTrait(bounds), _) => {
                if env.apit_base != usize::MAX {
                    // argument position: an anonymous generic param of the fn
                    let idx = env.apit_base + env.apit_n;
                    env.apit_n += 1;
                    let _ = idx;
                    self.fill_fresh_kind(env, t, out, SK_PARAM);
                } else {
                    // return position: the hidden type's regions are the captured lifetimes
                    let s = out.len();
                    self.fill_fresh(env, t, out);
                    let mut captured: Vec<SR> = Vec::new();
                    for b in ast_.list(bounds) {
                        if let ast::Bound::Lifetime(l) = b {
                            captured.push(self.lifetime(env, Some(*l)));
                        }
                    }
                    if env.edition >= 2024 {
                        for (_, r) in &env.lts {
                            captured.push(*r);
                        }
                    }
                    // type params' regions are always captured
                    let tps: Vec<SR> = env.tparams.iter().flat_map(|(_, tp)| match tp {
                        TP::Slots(v) => v.clone(),
                    }).collect();
                    captured.extend(tps);
                    if let Some(ss) = &env.self_slots {
                        captured.extend(ss.iter().copied());
                    }
                    // elided `'_` in the bounds (`+ '_`) was resolved by `lifetime` above
                    for i in s..out.len() {
                        for c in &captured {
                            env.caps.push((*c, out[i]));
                        }
                    }
                }
            }
            (AstTy::Path(p), _) => self.inst_path(env, &p, t, obj, out, depth),
            (AstTy::Infer, _) => self.fill_fresh(env, t, out),
            _ => self.fill_fresh(env, t, out),
        }
    }

    fn fill_fresh_kind(&self, env: &IEnv, t: TyId, out: &mut Vec<SR>, kind: u8) {
        let n = self.n_slots(t);
        for _ in 0..n {
            let s = self.new_sym(env, "'_".to_string(), kind);
            out.push(s);
        }
    }

    fn inst_path(&self, env: &mut IEnv, p: &ast::Path, t: TyId, obj: SR, out: &mut Vec<SR>, depth: u32) {
        let file = env.file;
        // `<Q as Tr>::X`: the selected impl's `type X = ..`, its lifetimes bound by matching the
        // impl's self type against the written Q
        // (body annotations only: the owner's generics lower the written types)
        if let (true, Some(q), Some(owner), Some(last)) = (env.expr_mode, self.tx.prog.files[file as usize].ast.qself(p), self.cur_owner.get(), self.tx.prog.files[file as usize].ast.segs(p.segs).last()) {
            if let Some(tp) = &q.trait_path {
                let name = self.text(file, last.name).to_string();
                let s0 = out.len();
                if self.inst_projection(env, q.ty, tp, &name, t, obj, out, depth, owner) {
                    return;
                }
                out.truncate(s0);
            }
        }
        // a type param / Self
        if p.no_qself() && p.segs.len() == 1 && self.tx.prog.files[file as usize].ast.segs(p.segs)[0].args().is_none() {
            let seg = &self.tx.prog.files[file as usize].ast.segs(p.segs)[0];
            if seg.kind() == SegKind::SelfType {
                match env.self_slots.clone() {
                    Some(v) => out.extend(v),
                    None => {
                        // `Self` written in a body: the impl's self type -- its regions in the
                        // checked fn (incl. the impl header's `'_`), else from its AST
                        if env.expr_mode {
                            if let Some(v) = self.cur_self.borrow().clone() {
                                if v.len() == self.n_slots(t) {
                                    out.extend(v.iter().map(|r| SR::Sym(*r | 0x8000_0000)));
                                    return;
                                }
                            }
                        }
                        if let (true, Some((ifile, imod, sty))) = (env.expr_mode, self.owner_impl_self()) {
                            let saved = (env.file, env.module);
                            env.file = ifile;
                            env.module = imod;
                            let s0 = out.len();
                            self.inst(env, sty, t, obj, out, depth + 1);
                            env.file = saved.0;
                            env.module = saved.1;
                            if out.len() - s0 == self.n_slots(t) {
                                return;
                            }
                            out.truncate(s0);
                        }
                        self.fill_fresh(env, t, out)
                    }
                }
                return;
            }
            let n = self.text(file, seg.name).as_bytes().to_vec();
            if let Some(i) = env.tparams.iter().position(|x| x.0 == n) {
                match env.tparams[i].1.clone() {
                    TP::Slots(v) => out.extend(v),

                }
                return;
            }
        }
        // `Self::X` in an impl: the impl's assoc type
        if p.no_qself() && p.segs.len() == 2 && self.tx.prog.files[file as usize].ast.segs(p.segs)[0].kind() == SegKind::SelfType {
            if let Some(imp) = env.impl_def {
                let name = self.text(file, self.tx.prog.files[file as usize].ast.segs(p.segs)[1].name);
                if let Some(x) = self.impl_assoc_ty(imp, name) {
                    let (ifile, aty) = x;
                    // a generic associated type (`type Iter<'a> = X<'a, P> where Self: 'a`):
                    // its lifetime params are the written arguments (`Self::Iter<'_>` in an
                    // output is the elided input region), not lifetimes of their own
                    let gat_lts = self.impl_assoc_lifetimes(imp, name);
                    let fast = &self.tx.prog.files[file as usize].ast;
                    let written: Vec<ast::Ident> = match fast.gargs(fast.segs(p.segs)[1].args()) {
                        Some(GenericArgs::Angle(v)) => fast.list(*v).iter().filter_map(|g| match g {
                            GenericArg::Lifetime(l) => Some(*l),
                            _ => None,
                        }).collect(),
                        _ => Vec::new(),
                    };
                    let mut bound: Vec<(Vec<u8>, SR)> = Vec::new();
                    for (i, n) in gat_lts.iter().enumerate() {
                        let r = self.lifetime(env, written.get(i).copied());
                        bound.push((n.as_bytes().to_vec(), r));
                    }
                    let n0 = env.lts.len();
                    for b in bound.into_iter().rev() {
                        env.lts.insert(0, b);
                    }
                    let added = env.lts.len() - n0;
                    let saved = env.file;
                    env.file = ifile;
                    self.inst(env, aty, t, obj, out, depth + 1);
                    env.file = saved;
                    env.lts.drain(0..added);
                    return;
                }
            }
        }
        let k = self.tx.kind(t);
        let def = if p.no_qself() { self.tx.tcx.resolve_type_path_def(self.tx.prog, file, env.module, p) } else { None };
        match (def.map(|d| (d, self.tx.prog.def(d).kind)), k) {
            (Some((d, DefKind::Struct | DefKind::Enum | DefKind::Union)), TyKind::Adt(d2, args, _)) if d == d2 => {
                let info = self.adt_info(d);
                let last = self.tx.prog.files[file as usize].ast.segs(p.segs).last();
                let mut lt_args: Vec<ast::Ident> = Vec::new();
                let mut ty_args: Vec<Option<ast::TyId>> = Vec::new();
                if let Some(GenericArgs::Angle(v)) = last.and_then(|s| self.tx.prog.files[file as usize].ast.gargs(s.args())) {
                    for g in self.tx.prog.files[file as usize].ast.list(*v) {
                        match g {
                            GenericArg::Lifetime(l) => lt_args.push(*l),
                            GenericArg::Type(x) => ty_args.push(Some(*x)),
                            GenericArg::Const(_) => ty_args.push(None),
                            _ => {}
                        }
                    }
                }
                let lt0 = out.len();
                for i in 0..info.var_lt.len() {
                    let r = match lt_args.get(i) {
                        Some(l) => self.lifetime(env, Some(*l)),
                        None => self.lifetime(env, None),
                    };
                    out.push(r);
                }
                let mut arg_ranges = Vec::new();
                for (i, a) in self.tx.tcx.tys.list(args).iter().enumerate() {
                    let s0 = out.len();
                    match ty_args.get(i).copied().flatten() {
                        Some(x) => {
                            // `Box<dyn Tr>`: the object default is 'static
                            self.inst(env, x, *a, SR::Static, out, depth + 1)
                        }
                        None => self.fill_fresh(env, *a, out),
                    }
                    arg_ranges.push((s0, out.len()));
                }
                // the ADT's own predicates (`struct S<'a, 'b: 'a, T: 'a>`): well-formedness of
                // this type -- implied bounds in inputs, requirements elsewhere
                if out.len() == lt0 + self.n_slots(t) {
                    for (a, b) in self.adt_lifetime_bounds(d) {
                        match (a, b) {
                            (Ok(ai), bi) => {
                                if let (Some(x), Some(y)) = (out.get(lt0 + ai), out.get(lt0 + bi)) {
                                    env.wf.push((*x, *y));
                                }
                            }
                            (Err(ti), bi) => {
                                if let (Some((s0, s1)), Some(y)) = (arg_ranges.get(ti), out.get(lt0 + bi).copied()) {
                                    for k in *s0..*s1 {
                                        env.wf.push((out[k], y));
                                    }
                                }
                            }
                        }
                    }
                }
            }
            (Some((d, DefKind::TypeAlias)), _) => self.inst_alias(env, d, p, t, obj, out, depth),
            (Some((_, DefKind::Trait)), TyKind::Dyn(td, args, binds, _, _, _, _)) => {
                // bare trait object (2015)
                out.push(if self.trait_static_bound(td) { SR::Static } else { obj });
                for a in self.tx.tcx.tys.list(args).iter().copied() {
                    self.fill_fresh(env, a, out);
                }
                for (_, b, _) in self.tx.tcx.tys.list(binds).iter().copied() {
                    self.fill_fresh(env, b, out);
                }
            }
            _ => self.fill_fresh(env, t, out),
        }
    }

    /// `trait T: 'static` (also through supertraits): its objects default to `+ 'static`.
    pub fn trait_static_bound(&self, td: DefId) -> bool {
        let mut todo = vec![td];
        let mut seen = Vec::new();
        while let Some(t) = todo.pop() {
            if seen.contains(&t) || seen.len() > 16 {
                continue;
            }
            seen.push(t);
            let pd = self.tx.prog.def(t);
            let tast = &self.tx.prog.files[pd.file as usize].ast;
            if let Some(crate::ast::TraitItem { supers, .. }) = tast.as_trait(&tast.item(pd.item).kind) {
                for b in tast.list(*supers) {
                    if let ast::Bound::Lifetime(l) = b {
                        if self.text(pd.file, *l) == "'static" {
                            return true;
                        }
                    }
                }
            }
            todo.extend(self.tx.tcx.trait_supers(self.tx.prog, t));
        }
        false
    }

    /// An ADT's declared outlives predicates: (lifetime index | Err(type param index), lifetime
    /// index) meaning the first outlives the second.
    /// Per type argument of ADT `d`: whether its declaration bounds it `'static` (`struct
    /// S<T: 'static>`, `where T: Any`): an ADT in a signature implies it (implied bounds).
    pub fn adt_tparam_bounds(&self, d: DefId) -> Vec<Vec<Option<usize>>> {
        let (file, g) = match self.adt_generics(d) {
            Some(x) => x,
            None => return Vec::new(),
        };
        let module = self.tx.prog.def(d).scope;
        let ast_ = &self.tx.prog.files[file as usize].ast;
        let tys: Vec<&str> = ast_.list(g.params).iter().filter(|p| !matches!(p.kind, GenericParamKind::Lifetime(_))).map(|p| self.text(file, p.name)).collect();
        let lts: Vec<&str> = ast_.list(g.params).iter().filter(|p| matches!(p.kind, GenericParamKind::Lifetime(_))).map(|p| self.text(file, p.name)).collect();
        let mut out: Vec<Vec<Option<usize>>> = vec![Vec::new(); tys.len()];
        // None: 'static; Some(i): the ADT's lifetime param i
        let bound_of = |b: &ast::Bound| -> Option<Option<usize>> {
            match b {
                ast::Bound::Lifetime(l) => {
                    let t = self.text(file, *l);
                    if t == "'static" { Some(None) } else { lts.iter().position(|x| *x == t).map(Some) }
                }
                ast::Bound::Trait { path, maybe: false, .. } => if self.trait_static(file, module, path, 0) { Some(None) } else { None },
                _ => None,
            }
        };
        for p in ast_.list(g.params) {
            if let GenericParamKind::Type(bs, _) = &p.kind {
                if let Some(i) = tys.iter().position(|x| *x == self.text(file, p.name)) {
                    for b in ast_.list(*bs) {
                        if let Some(x) = bound_of(b) {
                            out[i].push(x);
                        }
                    }
                }
            }
        }
        for wp in ast_.list(g.where_) {
            if let ast::WherePred::Bound { ty, bounds, .. } = wp {
                if let AstTy::Path(pp) = ast_.ty(*ty) {
                    if pp.segs.len() == 1 {
                        if let Some(i) = tys.iter().position(|x| *x == self.text(file, self.tx.prog.files[file as usize].ast.segs(pp.segs)[0].name)) {
                            for b in ast_.list(*bounds) {
                                if let Some(x) = bound_of(b) {
                                    out[i].push(x);
                                }
                            }
                        }
                    }
                }
            }
        }
        out
    }

    pub fn adt_lifetime_bounds(&self, d: DefId) -> Vec<(Result<usize, usize>, usize)> {
        let (file, g) = match self.adt_generics(d) {
            Some(x) => x,
            None => return Vec::new(),
        };
        let ast_ = &self.tx.prog.files[file as usize].ast;
        let mut lts: Vec<&str> = Vec::new();
        let mut tys: Vec<&str> = Vec::new();
        for p in ast_.list(g.params) {
            match &p.kind {
                GenericParamKind::Lifetime(_) => lts.push(self.text(file, p.name)),
                _ => tys.push(self.text(file, p.name)),
            }
        }
        let li = |n: &str| lts.iter().position(|x| *x == n);
        let mut out = Vec::new();
        for p in ast_.list(g.params) {
            match &p.kind {
                GenericParamKind::Lifetime(bs) => {
                    if let Some(a) = li(self.text(file, p.name)) {
                        for b in ast_.list(*bs) {
                            if let Some(bi) = li(self.text(file, *b)) {
                                out.push((Ok(a), bi));
                            }
                        }
                    }
                }
                GenericParamKind::Type(bs, _) => {
                    if let Some(ti) = tys.iter().position(|x| *x == self.text(file, p.name)) {
                        for b in ast_.list(*bs) {
                            if let ast::Bound::Lifetime(l) = b {
                                if let Some(bi) = li(self.text(file, *l)) {
                                    out.push((Err(ti), bi));
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        for wp in ast_.list(g.where_) {
            if let ast::WherePred::Lifetime(a, bs) = wp {
                if let Some(ai) = li(self.text(file, *a)) {
                    for b in ast_.list(*bs) {
                        if let Some(bi) = li(self.text(file, *b)) {
                            out.push((Ok(ai), bi));
                        }
                    }
                }
            }
        }
        out
    }

    /// The impl of the fn being checked: (file, module, self type AST).
    pub fn owner_impl_self(&self) -> Option<(u32, u32, ast::TyId)> {
        let owner = self.cur_owner.get()?;
        let prog = self.tx.prog;
        let parent = prog.def(owner).parent;
        if parent == crate::program::NO_DEF || prog.def(parent).kind != DefKind::Impl {
            return None;
        }
        let pd = prog.def(parent);
        match &prog.files[pd.file as usize].ast.item(pd.item).kind {
            ItemKind::Impl(r) => Some((pd.file, pd.scope, prog.files[pd.file as usize].ast.row(*r).self_ty)),
            _ => None,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn inst_projection(&self, env: &mut IEnv, qty: ast::TyId, tp: &ast::Path, name: &str, t: TyId, obj: SR, out: &mut Vec<SR>, depth: u32, owner: DefId) -> bool {
        if depth > 20 {
            return false;
        }
        let prog = self.tx.prog;
        let tcx = self.tx.tcx;
        if self.tx.prog.files[env.file as usize].ast.segs(tp.segs).last().map_or(true, |s| s.args().is_some()) {
            return false;
        }
        let ge = tcx.item_env(prog, owner);
        let qt = self.tx.normalize(tcx.lower_ty(&self.tx.cx, prog, env.file, env.module, &ge, qty));
        if matches!(self.tx.kind(qt), TyKind::Error | TyKind::Infer(_) | TyKind::Param(_) | TyKind::Alias(_, AliasTy { kind: AliasTyKind::Projection { .. }, .. })) {
            return false;
        }
        let td = match tcx.resolve_type_path_def(prog, env.file, env.module, tp) {
            Some(d) => d,
            None => return false,
        };
        let Some(imp) = self.tx.select_impl(td, qt, &[]) else { return false };
        let (ifile, aty) = match self.impl_assoc_ty(imp, name) {
            Some(x) => x,
            None => return false,
        };
        let ipd = prog.def(imp);
        let self_ast = match &prog.files[ipd.file as usize].ast.item(ipd.item).kind {
            ItemKind::Impl(r) => prog.files[ipd.file as usize].ast.row(*r).self_ty,
            _ => return false,
        };
        // the written self type's regions
        let mut qs = Vec::new();
        self.inst(env, qty, qt, SR::Static, &mut qs, depth + 1);
        // the impl's self type with its own lifetimes as fresh symbols
        let mk = |file: u32, module: u32, lts: Vec<(Vec<u8>, SR)>| IEnv {
            file,
            module,
            lts,
            tparams: Vec::new(),
            self_slots: None,
            elide: Elide::Fresh,
            next: env.next,
            names: env.names,
            kinds: env.kinds,
            wf: Vec::new(),
            seen: Vec::new(),
            caps: Vec::new(),
            apit_base: usize::MAX,
            apit_n: 0,
            expr_mode: false,
            impl_def: Some(imp),
            edition: env.edition,
        };
        let mut sub = mk(ipd.file, ipd.scope, Vec::new());
        let mut is = Vec::new();
        self.inst(&mut sub, self_ast, qt, SR::Static, &mut is, depth + 1);
        if is.len() != qs.len() {
            return false;
        }
        let mut map: Vec<(Vec<u8>, SR)> = Vec::new();
        for (n, sr) in &sub.lts {
            if let Some(k) = is.iter().position(|x| x == sr) {
                map.push((n.clone(), qs[k]));
            }
        }
        let mapped: Vec<Vec<u8>> = map.iter().map(|x| x.0.clone()).collect();
        let mut sub2 = mk(ifile, ipd.scope, map);
        let s0 = out.len();
        self.inst(&mut sub2, aty, t, obj, out, depth + 1);
        if out.len() - s0 != self.n_slots(t) {
            return false;
        }
        // impl lifetimes the self type does not bind: unconstrained here
        let unbound: Vec<SR> = sub2.lts.iter().filter(|(n, _)| !mapped.contains(n)).map(|(_, r)| *r).collect();
        for x in out[s0..].iter_mut() {
            if unbound.contains(x) {
                *x = SR::Fresh;
            }
        }
        true
    }

    /// The slots of ADT `t` that its `Copy` impl requires to be 'static (`impl Copy for
    /// Foo<'static>`): a copy of a value of `t` needs them (rustc proves `Foo<'a>: Copy`).
    pub fn copy_static_slots(&self, t: TyId) -> Rc<Vec<usize>> {
        if let Some(v) = self.copy_static.borrow().get(&t) {
            return v.clone();
        }
        let mut out = Vec::new();
        let prog = self.tx.prog;
        if let (Some(ct), TyKind::Adt(..)) = (self.tx.copy_trait, self.tx.kind(t)) {
            if self.n_slots(t) > 0 {
                if let Some(imp) = self.tx.select_impl(ct, t, &[]) {
                    let pd = prog.def(imp);
                    if let Some(crate::ast::ImplItem { self_ty, .. }) = { let a = &prog.files[pd.file as usize].ast; a.as_impl(&a.item(pd.item).kind) } {
                        let ann = self.annotation(pd.file, pd.scope, *self_ty, t, &[]);
                        if ann.len() == self.n_slots(t) {
                            for (i, a) in ann.iter().enumerate() {
                                if *a == Ann::Static {
                                    out.push(i);
                                }
                            }
                        }
                    }
                }
            }
        }
        let r = Rc::new(out);
        self.copy_static.borrow_mut().insert(t, r.clone());
        r
    }

    /// The lifetime params of an impl's associated type (a GAT's), by name.
    fn impl_assoc_lifetimes(&self, imp: DefId, name: &str) -> Vec<String> {
        let pd = self.tx.prog.def(imp);
        let ast_ = &self.tx.prog.files[pd.file as usize].ast;
        if let Some(crate::ast::ImplItem { items, .. }) = ast_.as_impl(&ast_.item(pd.item).kind) {
            for it in ast_.list(*items) {
                let item = ast_.item(*it);
                if self.text(pd.file, item.name) == name {
                    if let Some(crate::ast::TypeAliasItem { generics, .. }) = ast_.as_alias(&item.kind) {
                        return ast_.list(generics.params).iter().filter(|g| matches!(g.kind, GenericParamKind::Lifetime(_))).map(|g| self.text(pd.file, g.name).to_string()).collect();
                    }
                }
            }
        }
        Vec::new()
    }

    fn impl_assoc_ty(&self, imp: DefId, name: &str) -> Option<(u32, ast::TyId)> {
        let pd = self.tx.prog.def(imp);
        let ast_ = &self.tx.prog.files[pd.file as usize].ast;
        if let Some(crate::ast::ImplItem { items, .. }) = ast_.as_impl(&ast_.item(pd.item).kind) {
            for it in ast_.list(*items) {
                let item = ast_.item(*it);
                if self.text(pd.file, item.name) == name {
                    if let Some(crate::ast::TypeAliasItem { ty: Some(t), .. }) = ast_.as_alias(&item.kind) {
                        return Some((pd.file, *t));
                    }
                }
            }
        }
        None
    }

    #[allow(clippy::too_many_arguments)]
    fn inst_alias(&self, env: &mut IEnv, d: DefId, p: &ast::Path, t: TyId, obj: SR, out: &mut Vec<SR>, depth: u32) {
        let pd = self.tx.prog.def(d);
        let ast_ = &self.tx.prog.files[pd.file as usize].ast;
        let (generics, aty) = match ast_.as_alias(&ast_.item(pd.item).kind) {
            Some(crate::ast::TypeAliasItem { generics, ty: Some(x), .. }) => (generics, *x),
            _ => return self.fill_fresh(env, t, out),
        };
        let mut lt_args: Vec<ast::Ident> = Vec::new();
        let mut ty_args: Vec<ast::TyId> = Vec::new();
        let uast = &self.tx.prog.files[env.file as usize].ast;
        if let Some(GenericArgs::Angle(v)) = uast.segs(p.segs).last().and_then(|s| uast.gargs(s.args())) {
            for g in uast.list(*v) {
                match g {
                    GenericArg::Lifetime(l) => lt_args.push(*l),
                    GenericArg::Type(x) => ty_args.push(*x),
                    _ => return self.fill_fresh(env, t, out),
                }
            }
        }
        // type params: each written argument instantiated (in the user's scope) against the
        // erased type at the param's first use in the alias body
        let tnames: Vec<ast::Ident> = ast_.list(generics.params).iter().filter(|g| matches!(g.kind, GenericParamKind::Type(..))).map(|g| g.name).collect();
        if ast_.list(generics.params).iter().any(|g| !matches!(g.kind, GenericParamKind::Lifetime(_) | GenericParamKind::Type(..))) || tnames.len() != ty_args.len() {
            return self.fill_fresh(env, t, out);
        }
        let mut tps: Vec<(Vec<u8>, TP)> = Vec::new();
        for (nm, at) in tnames.iter().zip(ty_args.iter()) {
            let name = self.text(pd.file, *nm).as_bytes().to_vec();
            let Some(pt) = self.alias_param_ty(pd.file, aty, t, &name, 0) else { return self.fill_fresh(env, t, out) };
            let mut v = Vec::new();
            self.inst(env, *at, pt, obj, &mut v, depth + 1);
            if v.len() != self.n_slots(pt) {
                return self.fill_fresh(env, t, out);
            }
            // an inferred argument (`P<_>`) is one type: its regions shared by every use
            for x in v.iter_mut() {
                if *x == SR::Fresh {
                    *x = self.new_sym(env, "'_".to_string(), SK_FRESH);
                }
            }
            tps.push((name, TP::Slots(v)));
        }
        let mut lts = Vec::new();
        let mut li = 0;
        for g in ast_.list(generics.params) {
            if let GenericParamKind::Lifetime(_) = g.kind {
                let r = match lt_args.get(li) {
                    Some(l) => self.lifetime(env, Some(*l)),
                    None => self.lifetime(env, None),
                };
                lts.push((self.text(pd.file, g.name).as_bytes().to_vec(), r));
                li += 1;
            }
        }
        let saved_file = env.file;
        let saved_mod = env.module;
        let saved_lts = std::mem::replace(&mut env.lts, lts);
        let saved_tps = if tps.is_empty() { None } else { Some(std::mem::replace(&mut env.tparams, tps)) };
        env.file = pd.file;
        env.module = pd.scope;
        self.inst(env, aty, t, obj, out, depth + 1);
        env.file = saved_file;
        env.module = saved_mod;
        env.lts = saved_lts;
        if let Some(v) = saved_tps {
            env.tparams = v;
        }
    }

    /// The erased type at the first use of type param `name` in alias body `at` (matched
    /// against the alias's erased type `t`).
    fn alias_param_ty(&self, file: u32, at: ast::TyId, t: TyId, name: &[u8], depth: u32) -> Option<TyId> {
        if depth > 20 {
            return None;
        }
        let ast_ = &self.tx.prog.files[file as usize].ast;
        match (ast_.ty(at), self.tx.kind(t)) {
            (AstTy::Paren(x), _) => self.alias_param_ty(file, *x, t, name, depth + 1),
            (AstTy::Path(p), _) if p.no_qself() && p.segs.len() == 1 && self.tx.prog.files[file as usize].ast.segs(p.segs)[0].args().is_none() && self.text(file, self.tx.prog.files[file as usize].ast.segs(p.segs)[0].name).as_bytes() == name => Some(t),
            (AstTy::Ref(_, _, x), TyKind::Ref(_, i, _)) | (AstTy::Ptr(_, x), TyKind::Ptr(_, i)) | (AstTy::Slice(x), TyKind::Slice(i)) | (AstTy::Array(x, _), TyKind::Array(i, _)) => self.alias_param_ty(file, *x, i, name, depth + 1),
            (AstTy::Tuple(v), TyKind::Tuple(ts)) if v.len() == self.tx.tcx.tys.list(ts).len() => ast_.list(*v).iter().zip(self.tx.tcx.tys.list(ts).iter()).find_map(|(x, y)| self.alias_param_ty(file, *x, *y, name, depth + 1)),
            (AstTy::Path(p), TyKind::Adt(_, args, _)) => {
                let tys: Vec<ast::TyId> = match ast_.segs(p.segs).last().and_then(|s| ast_.gargs(s.args())) {
                    Some(GenericArgs::Angle(v)) => ast_.list(*v).iter().filter_map(|g| match g {
                        GenericArg::Type(x) => Some(*x),
                        _ => None,
                    }).collect(),
                    _ => Vec::new(),
                };
                if tys.len() != self.tx.tcx.tys.list(args).len() {
                    return None;
                }
                tys.iter().zip(self.tx.tcx.tys.list(args).iter()).find_map(|(x, y)| self.alias_param_ty(file, *x, *y, name, depth + 1))
            }
            _ => None,
        }
    }

    // ------------------------------------------------------------------ signatures

    /// The signature template of fn instance `d<args>` (args: all generic args, impl / trait
    /// params first).
    pub fn sig_template(&self, d: DefId, args: &[TyId]) -> Rc<SigTmpl> {
        let key = (d, args.to_vec());
        if let Some(t) = self.templates.borrow().get(&key) {
            return t.clone();
        }
        let t = Rc::new(self.sig_template_uncached(d, args));
        self.templates.borrow_mut().insert(key, t.clone());
        t
    }

    fn sig_template_uncached(&self, d: DefId, args: &[TyId]) -> SigTmpl {
        super::rty::signature::signature_template(self, d, args)
    }

    /// Whether a trait bound implies `'static` (a `'static` supertrait, transitively: `Any`).
    pub fn trait_static(&self, file: u32, module: u32, path: &ast::Path, depth: u32) -> bool {
        if depth > 8 {
            return false;
        }
        match self.tx.tcx.resolve_type_path_def(self.tx.prog, file, module, path) {
            Some(d) => self.trait_static_d(d, depth),
            None => false,
        }
    }

    /// An associated type whose declaration bounds it by a lifetime (`type Output: 'a`).
    pub fn assoc_has_lifetime_bound(&self, ad: DefId) -> bool {
        let pd = self.tx.prog.def(ad);
        let ast_ = &self.tx.prog.files[pd.file as usize].ast;
        match &ast_.item(pd.item).kind {
            ItemKind::TypeAlias(r) => ast_.list(ast_.row(*r).bounds).iter().any(|b| match b {
                ast::Bound::Lifetime(_) => true,
                ast::Bound::Trait { path, maybe: false, .. } => self.trait_static(pd.file, pd.scope, path, 0),
                _ => false,
            }),
            _ => true,
        }
    }

    pub fn trait_static_def(&self, d: DefId) -> bool {
        self.trait_static_d(d, 0)
    }

    fn trait_static_d(&self, d: DefId, depth: u32) -> bool {
        if depth > 8 {
            return false;
        }
        if let Some(b) = self.trait_static_memo.borrow().get(&d) {
            return *b;
        }
        self.trait_static_memo.borrow_mut().insert(d, false);
        let pd = self.tx.prog.def(d);
        let ast_ = &self.tx.prog.files[pd.file as usize].ast;
        let (supers, gens): (&[ast::Bound], Option<&ast::Generics>) = match &ast_.item(pd.item).kind {
            ItemKind::Trait(r) => (ast_.list(ast_.row(*r).supers), Some(&ast_.row(*r).generics)),
            ItemKind::TraitAlias(r) => (ast_.list(ast_.row(*r).bounds), Some(&ast_.row(*r).generics)),
            _ => (&[], None),
        };
        let mut r = false;
        // `where Self: 'static` on the trait
        if let Some(g) = gens {
            for wp in ast_.list(g.where_) {
                if let ast::WherePred::Bound { ty, bounds, .. } = wp {
                    if let AstTy::Path(p) = ast_.ty(*ty) {
                        if p.segs.len() == 1 && self.text(pd.file, ast_.segs(p.segs)[0].name) == "Self" {
                            for b in ast_.list(*bounds) {
                                match b {
                                    ast::Bound::Lifetime(l) if self.text(pd.file, *l) == "'static" => r = true,
                                    ast::Bound::Trait { path, maybe: false, .. } => {
                                        if self.trait_static(pd.file, pd.scope, path, depth + 1) {
                                            r = true;
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                }
            }
        }
        for b in supers {
            match b {
                ast::Bound::Lifetime(l) if self.text(pd.file, *l) == "'static" => r = true,
                ast::Bound::Trait { path, maybe: false, .. } => {
                    if self.trait_static(pd.file, pd.scope, path, depth + 1) {
                        r = true;
                    }
                }
                _ => {}
            }
        }
        self.trait_static_memo.borrow_mut().insert(d, r);
        r
    }

    /// A `let` annotation's slots for a local of erased type `t` (named lifetimes of the fn
    /// in `lts`, elided ones fresh).
    pub fn annotation(&self, file: u32, module: u32, at: ast::TyId, t: TyId, lts: &[(Vec<u8>, u32)]) -> Vec<Ann> {
        self.annotation_with(file, module, at, t, lts, false)
    }

    /// A static's / const's declared type: its elided lifetimes are 'static.
    pub fn static_annotation(&self, file: u32, module: u32, at: ast::TyId, t: TyId) -> Vec<Ann> {
        self.annotation_with(file, module, at, t, &[], true)
    }

    fn annotation_with(&self, file: u32, module: u32, at: ast::TyId, t: TyId, lts: &[(Vec<u8>, u32)], elided_static: bool) -> Vec<Ann> {
        let next = RefCell::new(0u32);
        let names = RefCell::new(Vec::new());
        let kinds = RefCell::new(Vec::new());
        let mut env = IEnv {
            file,
            module,
            lts: lts.iter().map(|(n, r)| (n.clone(), SR::Sym(*r | 0x8000_0000))).collect(),
            tparams: Vec::new(),
            self_slots: None,
            elide: Elide::Fresh,
            next: &next,
            names: &names,
            kinds: &kinds,
            wf: Vec::new(),
            seen: Vec::new(),
            caps: Vec::new(),
            apit_base: usize::MAX,
            apit_n: 0,
            expr_mode: false,
            impl_def: None,
            edition: self.tx.prog.files[file as usize].edition,
        };
        env.expr_mode = true;
        if elided_static {
            env.elide = Elide::Fixed(SR::Static);
        }
        let mut out = Vec::new();
        self.inst(&mut env, at, t, SR::Static, &mut out, 0);
        // only 'static and the fn's named lifetimes are meaningful; fresh ones are unconstrained
        let nm = names.borrow();
        let call = self.ann_calls.get() + 1;
        self.ann_calls.set(call);
        let count = |s: u32| out.iter().filter(|x| **x == SR::Sym(s)).count();
        let counts: Vec<usize> = out.iter().map(|r| match r {
            SR::Sym(s) => count(*s),
            _ => 0,
        }).collect();
        out.iter().zip(counts).map(|(r, c)| match *r {
            SR::Sym(s) if s & 0x8000_0000 != 0 => Ann::Rv(s & 0x7fff_ffff),
            SR::Sym(s) => match nm.get(s as usize) {
                Some(n) if n != "'_" => Ann::Named(n.clone()),
                _ if c > 1 => Ann::Shared((call << 32) | s as u64),
                _ => Ann::Fresh,
            },
            SR::Static => Ann::Static,
            SR::Fresh => Ann::Fresh,
        }).collect()
    }

    // ------------------------------------------------------------------ dropck

    /// Which slots of `t` must be live where a value of `t` is dropped (rustc dropck_outlives):
    /// all of a type with a Drop impl (lifetime params; type params only through what their own
    /// drop needs for std's `#[may_dangle]` containers), fields' needs for types without one.
    pub fn drop_slots(&self, t: TyId, out: &mut Vec<usize>, base: usize, depth: u32) {
        if depth > 20 {
            return;
        }
        let n = self.n_slots(t);
        if n == 0 || !self.tx.needs_drop(t) {
            return;
        }
        match self.tx.kind(t) {
            TyKind::Adt(d, args, _) => {
                if self.is_manually_drop(d) || self.tx.is_union(t) {
                    return;
                }
                if self.tx.has_drop_impl(t) {
                    let info = self.adt_info(d);
                    if self.tx.is_std_def(d) {
                        // std containers: `#[may_dangle] T` (rustc's Vec, Box, Rc, Arc,
                        // collections, ...): the lifetime params must be live, the type args
                        // only as their own drop needs
                        for i in 0..info.var_lt.len() {
                            out.push(base + i);
                        }
                        let mut off = base + info.var_lt.len();
                        for a in self.tx.tcx.tys.list(args).iter().copied() {
                            self.drop_slots(a, out, off, depth + 1);
                            off += self.n_slots(a);
                        }
                    } else {
                        for i in 0..n {
                            out.push(base + i);
                        }
                    }
                    return;
                }
                // structural: the fields' needs, mapped back to t's slots
                let nv = self.tcx_variants(t);
                for vi in 0..nv {
                    for fi in 0..self.tx.n_fields(t, vi as u32) {
                        let ft = match self.tx.field_ty(t, vi as u32, fi as u32) {
                            Some(x) => x,
                            None => continue,
                        };
                        let fm = self.field_slots(t, vi as u32, fi as u32);
                        let mut sub = Vec::new();
                        self.drop_slots(ft, &mut sub, 0, depth + 1);
                        for s in sub {
                            if let Some(SR::Sym(i)) = fm.get(s) {
                                out.push(base + *i as usize);
                            }
                        }
                    }
                }
            }
            TyKind::Tuple(v) => {
                let mut off = base;
                for x in self.tx.tcx.tys.list(v).iter().copied() {
                    self.drop_slots(x, out, off, depth + 1);
                    off += self.n_slots(x);
                }
            }
            TyKind::Array(e, _) | TyKind::Slice(e) => self.drop_slots(e, out, base, depth + 1),
            TyKind::Closure(..) | TyKind::Dyn(..) => {
                for i in 0..n {
                    out.push(base + i);
                }
            }
            _ => {}
        }
    }

    fn tcx_variants(&self, t: TyId) -> usize {
        match self.tx.kind(t) {
            TyKind::Adt(d, _, _) => self.tx.tcx.adts.get(&d).map_or(0, |a| a.variants.len()),
            _ => 0,
        }
    }
}

/// A struct's field `fi`: its file, module and AST type.
pub fn struct_field_ast(prog: &crate::program::Program, d: DefId, fi: u32) -> Option<(u32, u32, ast::TyId)> {
    let def = prog.def(d);
    let ast_ = &prog.files[def.file as usize].ast;
    let it = ast_.item(def.item);
    match &it.kind {
        ItemKind::Struct(r) => vd_fields(ast_, &ast_.row(*r).data).get(fi as usize).map(|t| (def.file, def.scope, *t)),
        _ => None,
    }
}

fn vd_fields(ast_: &ast::Ast, vd: &ast::VariantData) -> Vec<ast::TyId> {
    match vd {
        ast::VariantData::Unit => Vec::new(),
        ast::VariantData::Tuple(fs) | ast::VariantData::Struct(fs) => ast_.list(*fs).iter().map(|f| f.ty).collect(),
    }
}
