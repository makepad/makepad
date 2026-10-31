//! Field views on MIR (plans/relaxed-rust.md A1, plans/relax-borrowck.md section 6): per fn
//! instance and reference parameter, the places under `*param` its body reads / writes /
//! borrows, with whether any use is mutable; inferred from the analysis MIR of the body
//! (transitively through calls that reborrow the whole parameter, recursion to a fixed point).
//! A call that reborrows a place P for a parameter then borrows only P's summary parts instead
//! of all of P (access.rs applies that to the C-NLL4 conflict check).
//!
//! Opaque (the whole `*param`): the reference used as a value (copied, stored, returned, passed
//! where the callee's summary is unknown), raw pointers from it, calls through dyn / fn
//! pointers / closures, bodies the builder cannot produce. Interior mutability is a shared read
//! of the field (as Rust). A summary over-approximates its body; nothing else is relaxed.
//!
//! Re-port of borrowck/fv.rs (BIR) onto `mir::Body` and the C-NLL2 place model. The verdicts
//! are the BIR checker's: the same summaries, the same narrowing, the same extended two-phase
//! rule, the same export records.

use crate::fxhash::HashMap;
use crate::mir::visit::{MutatingUseContext as M, NonUseContext as U, PlaceContext, Visitor};
use crate::mir::*;
use crate::program::{DefId, DefKind, Program};
use crate::tcx::{EvalCx, Tcx, TypingEnv};
use crate::types::{TyId, TyKind};
use std::cell::RefCell;
use std::rc::Rc;


#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Summary {
    /// not a reference parameter / unknown
    Whole,
    /// paths under `*param` (fields / variants), each with its mutability
    Parts(Vec<(Vec<PlaceElem>, bool)>),
}

impl Summary {
    fn add(&mut self, path: Vec<PlaceElem>, m: bool) {
        if let Summary::Parts(v) = self {
            // an existing prefix absorbs the path; the path absorbs existing extensions
            for (p, pm) in v.iter_mut() {
                if p.len() <= path.len() && path[..p.len()] == p[..] {
                    *pm |= m;
                    return;
                }
            }
            let mut m2 = m;
            v.retain(|(p, pm)| {
                let ext = p.len() > path.len() && p[..path.len()] == path[..];
                if ext { m2 |= *pm; }
                !ext
            });
            v.push((path, m2));
        }
    }
}

/// `a.b.c` for a path under a value of type `t` ("*" for the empty path).
pub fn path_str(types: &FvTypes<'_>, mut t: TyId, p: &[PlaceElem]) -> String {
    let mut out = Vec::new();
    let mut vi = 0;
    for pr in p {
        match pr {
            PlaceElem::Field(fi, ft) => {
                out.push(types.tcx.place_field_name(types.prog, t, vi, fi.0));
                t = *ft;
                vi = 0;
            }
            PlaceElem::Downcast(_, v) => vi = v.0,
            _ => {}
        }
    }
    if out.is_empty() { "*".to_string() } else { out.join(".") }
}

/// A body for a fn instance, built for the analysis. The MIR builder (S3) provides it; the
/// environment is the instance's own (its param env), as the BIR path used the callee's.
pub struct FvBody { pub body: Body, pub env: TypingEnv }
pub trait FvBodies {
    /// The analysis MIR of `d<args>` under `cx`'s evaluation state, None when it cannot be
    /// built or has errors (its summary is then unknown, which keeps the whole borrow).
    fn body_of(&self, cx: &EvalCx<'_>, tcx: &Tcx, d: DefId, args: &[TyId]) -> Option<FvBody>;
}

type SummaryKey = (TypingEnv, DefId, Vec<TyId>);

#[derive(Default)]
pub struct SummaryCx {
    cache: RefCell<HashMap<SummaryKey, (Rc<Vec<Summary>>, Rc<Vec<DefId>>)>>,
    inprog: RefCell<Vec<SummaryKey>>,
    /// per in-progress summary: the fns its computation reached so far
    reached: RefCell<Vec<Vec<DefId>>>,
}

impl SummaryCx {
    /// A context live edits share (reload): each summary remembers the fns it reached, and an
    /// edit drops the summaries that reached one of its changed fns (`invalidate`); every other
    /// summary is the same before and after the edit, and in the next edit.
    pub fn new() -> Rc<Self> { Rc::new(Self::default()) }
    /// `changed` fns got new bodies: forget the summaries that read them.
    pub fn invalidate(&self, changed: &[DefId]) {
        if changed.is_empty() { return; }
        self.cache.borrow_mut().retain(|_, (_, reach)| !reach.iter().any(|d| changed.contains(d)));
    }
    /// What the computations in progress (the callers of `reach`) reached.
    fn reached_by_callers(&self, reach: &[DefId]) {
        for frame in self.reached.borrow_mut().iter_mut() { frame.extend_from_slice(reach); }
    }
}

/// Type queries the summaries need, over Tcx + the instance's evaluation context.
pub struct FvTypes<'a> {
    pub tcx: &'a Tcx,
    pub prog: &'a Program,
    pub cx: &'a EvalCx<'a>,
    rawptr_memo: RefCell<HashMap<TyId, bool>>,
}
impl<'a> FvTypes<'a> {
    pub fn new(tcx: &'a Tcx, prog: &'a Program, cx: &'a EvalCx<'a>) -> Self { Self { tcx, prog, cx, rawptr_memo: Default::default() } }

    fn is_std_def(&self, d: DefId) -> bool {
        let name = self.prog.syms.str(self.prog.crates[self.prog.def(d).krate as usize].name);
        matches!(name, "core" | "alloc" | "std")
    }
    fn n_fields(&self, t: TyId, vi: u32) -> usize { self.tcx.place_n_fields(t, vi) }
    fn field_ty(&self, t: TyId, vi: u32, fi: u32) -> Option<TyId> { self.tcx.place_field_ty(self.prog, self.cx, t, vi, fi) }
    fn box_inner(&self, t: TyId) -> Option<TyId> {
        match self.tcx.tys.kind(t) {
            TyKind::Adt(d, a, _) if !a.is_empty() && self.tcx.adts.get(d).is_some_and(|adt| adt.is_box()) => Some(self.tcx.tys.list(*a)[0]),
            _ => None,
        }
    }

    /// Whether a value of `t` holds a raw pointer (`*T`, or a struct around one: NonNull, Vec's
    /// buffer, Rc, ...) inline or inside a Box: memory reached through it is not part of any
    /// place the checker sees, so a field view must treat such a field as the whole value.
    /// References end the walk (what they point to is a place of its own).
    pub fn holds_raw_ptr(&self, t: TyId) -> bool { self.holds_raw_ptr_d(t, 0) }

    /// Whether every raw pointer in a value of `t` sits inside a std type that owns the memory
    /// it points to (Vec<u32>, String, Box<Vec<T>>, Rc<RefCell<T>>, user structs of those): a
    /// `*mut T` field of a user type (pointing anywhere) is not.
    pub fn owning(&self, t: TyId) -> bool { self.owning_d(t, 0) }

    fn owning_d(&self, t: TyId, depth: u32) -> bool {
        if depth > 12 { return false; }
        if !self.holds_raw_ptr(t) { return true; }
        match self.tcx.tys.kind(t) {
            TyKind::Adt(d, args, _) if self.is_std_def(*d) => self.tcx.tys.list(*args).iter().all(|a| self.owning_d(*a, depth + 1)),
            TyKind::Adt(d, _, _) => {
                let nv = self.tcx.adts.get(d).map_or(0, |a| a.variants.len());
                (0..nv as u32).all(|vi| (0..self.n_fields(t, vi) as u32).all(|fi| self.field_ty(t, vi, fi).map_or(true, |ft| self.owning_d(ft, depth + 1))))
            }
            TyKind::Tuple(v) => self.tcx.tys.list(*v).iter().all(|a| self.owning_d(*a, depth + 1)),
            TyKind::Array(e, _) | TyKind::Slice(e) => self.owning_d(*e, depth + 1),
            TyKind::Closure(_, _, crate::types::ClosureArgs { tupled_upvars_ty: up, .. }) => self.owning_d(*up, depth + 1),
            _ => false,
        }
    }

    fn holds_raw_ptr_d(&self, t: TyId, depth: u32) -> bool {
        if depth > 12 { return true; }
        if let Some(b) = self.rawptr_memo.borrow().get(&t) { return *b; }
        // cycles (a recursive type through Box) answer false while being computed
        self.rawptr_memo.borrow_mut().insert(t, false);
        let r = match self.tcx.tys.kind(t) {
            TyKind::Ptr(..) => true,
            TyKind::Tuple(v) => self.tcx.tys.list(*v).iter().any(|x| self.holds_raw_ptr_d(*x, depth + 1)),
            TyKind::Array(e, _) | TyKind::Slice(e) => self.holds_raw_ptr_d(*e, depth + 1),
            TyKind::Closure(_, _, crate::types::ClosureArgs { tupled_upvars_ty: up, .. }) => self.holds_raw_ptr_d(*up, depth + 1),
            TyKind::Adt(d, _, _) => {
                if let Some(inner) = self.box_inner(t) {
                    self.holds_raw_ptr_d(inner, depth + 1)
                } else {
                    let nv = self.tcx.adts.get(d).map_or(0, |a| a.variants.len());
                    (0..nv as u32).any(|vi| (0..self.n_fields(t, vi) as u32).any(|fi| self.field_ty(t, vi, fi).map_or(false, |ft| self.holds_raw_ptr_d(ft, depth + 1))))
                }
            }
            _ => false,
        };
        self.rawptr_memo.borrow_mut().insert(t, r);
        r
    }
}

/// What summaries are computed in: the program, tables, the evaluation context of the
/// instance being summarized, the shared cache and the MIR source.
pub struct FvEnv<'a> {
    pub prog: &'a Program,
    pub tcx: &'a Tcx,
    pub cx: &'a EvalCx<'a>,
    pub cache: &'a SummaryCx,
    pub bodies: &'a dyn FvBodies,
}

/// Per argument of `d<args>`: its field-view summary.
pub fn summaries(env: &FvEnv<'_>, d: DefId, args: &[TyId], depth: u32) -> Rc<Vec<Summary>> {
    // Field-view summaries describe places, not outlives relations. Their independent body
    // check cannot inherit the caller's inference variables (erase_regions: retain generic
    // arity and nested signature binders).
    let erased: Vec<_> = args.iter().map(|&ty| env.tcx.tys.erase_regions(ty)).collect();
    let args = erased.as_slice();
    let key = (env.cx.typing_env, d, args.to_vec());
    if let Some((s, reach)) = env.cache.cache.borrow().get(&key).cloned() {
        env.cache.reached_by_callers(&reach);
        return s;
    }
    if depth > 6 || env.cache.inprog.borrow().contains(&key) {
        // recursion: the fixed point starts from nothing (grows monotonically: the outer
        // computation includes everything the recursive call would add through its own body)
        env.cache.reached_by_callers(&[d]);
        return Rc::new(Vec::new());
    }
    env.cache.inprog.borrow_mut().push(key.clone());
    env.cache.reached.borrow_mut().push(vec![d]);
    let r = Rc::new(compute(env, d, args, depth));
    let mut reach = env.cache.reached.borrow_mut().pop().unwrap_or_default();
    reach.sort_unstable();
    reach.dedup();
    let reach = Rc::new(reach);
    env.cache.reached_by_callers(&reach);
    env.cache.inprog.borrow_mut().retain(|x| *x != key);
    env.cache.cache.borrow_mut().insert(key, (r.clone(), reach));
    r
}

fn compute(env: &FvEnv<'_>, d: DefId, args: &[TyId], depth: u32) -> Vec<Summary> {
    if !matches!(env.prog.def(d).kind, DefKind::Fn | DefKind::AssocFn) { return Vec::new(); }
    // Failure to build is the unknown summary, which keeps the whole borrow.
    let built = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| env.bodies.body_of(env.cx, env.tcx, d, args))) {
        Ok(Some(b)) => b,
        Err(p) if crate::preempt::is_cancelled(&*p) => std::panic::resume_unwind(p),
        _ => return Vec::new(),
    };
    let cx = EvalCx::new(env.prog, env.tcx, built.env, env.cx.state);
    let callee = FvEnv { prog: env.prog, tcx: env.tcx, cx: &cx, cache: env.cache, bodies: env.bodies };
    from_body(&callee, &built.body, depth)
}

/// The summaries of a body's reference parameters.
pub fn from_body(env: &FvEnv<'_>, body: &Body, depth: u32) -> Vec<Summary> {
    let n = body.arg_count as usize;
    let init: Vec<Summary> = (1..=n)
        .map(|a| match body.local_decls.get(a).map(|l| env.tcx.tys.kind(l.ty)) {
            Some(TyKind::Ref(..)) => Summary::Parts(Vec::new()),
            _ => Summary::Whole,
        })
        .collect();
    // a parameter local: the reference itself is `_a`, what it points to `*_a`
    let root = |p: &Place| -> Option<(usize, usize)> {
        let l = p.local.0 as usize;
        if l >= 1 && l <= n { Some((l - 1, 0)) } else { None }
    };
    scan(env, body, depth, init, &root)
}

/// The summaries of a closure body's by-reference captures (`*(*env).k`): what the closure
/// does through each captured reference.
pub fn closure_captures(env: &FvEnv<'_>, body: &Body) -> Vec<Summary> {
    let Some(env_ty) = body.local_decls.get(1).map(|l| l.ty) else { return Vec::new() };
    let tys = &env.tcx.tys;
    let by_ref_env = matches!(tys.kind(env_ty), TyKind::Ref(..));
    let closure_ty = match tys.kind(env_ty) { TyKind::Ref(_, t, _) => *t, _ => env_ty };
    let init: Vec<Summary> = match tys.kind(closure_ty) {
        TyKind::Closure(_, _, crate::types::ClosureArgs { tupled_upvars_ty: up, .. }) => match tys.kind(*up) {
            TyKind::Tuple(v) => tys.list(*v).iter().map(|&c| if matches!(tys.kind(c), TyKind::Ref(..)) { Summary::Parts(Vec::new()) } else { Summary::Whole }).collect(),
            _ => Vec::new(),
        },
        _ => Vec::new(),
    };
    let root = |p: &Place| -> Option<(usize, usize)> {
        if p.local != Local(1) { return None; }
        let i = if by_ref_env {
            if p.projection.first() != Some(&PlaceElem::Deref) { return None; }
            1
        } else { 0 };
        match p.projection.get(i) {
            Some(PlaceElem::Field(k, _)) => Some((k.0 as usize, i + 1)),
            _ => None,
        }
    };
    scan(env, body, 0, init, &root)
}

/// The callee instance of a call's `func` operand: a fn item, resolved to its impl method when
/// it is a trait item with a known impl (rustc_ty_utils instance.rs resolve_associated_item; an
/// unresolvable call stays on the trait item).
pub fn callee_of(env: &FvEnv<'_>, func: &Operand) -> Option<(DefId, Vec<TyId>)> {
    let Operand::Constant(c) = func else { return None };
    let Const::Val(_, ty) = &c.const_ else { return None };
    let TyKind::FnDef(d, args, _) = env.tcx.tys.kind(*ty) else { return None };
    let args = env.tcx.tys.list(*args).to_vec();
    match env.tcx.resolve_instance_raw(env.prog, env.cx.typing_env, *d, &args) {
        Ok(instance) => match instance.def {
            crate::instance::InstanceKind::Item(r) if env.prog.def(r).kind != DefKind::Closure => Some((r, instance.compact_args(env.tcx))),
            _ => Some((*d, args)),
        },
        Err(_) => Some((*d, args)),
    }
}

/// Where a single-use temp holding a borrow goes: a call argument, or a closure capture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Feed {
    /// (block, argument index)
    Call(usize, usize),
    /// (closure expr, capture index)
    Closure(u32, usize),
}

/// Use counts per local and the moves into call arguments / closure aggregates: temps used
/// exactly once whose single use feeds a call or a closure are the borrow's feeds.
pub fn find_feeds(body: &Body) -> HashMap<Local, Feed> {
    struct Uses(HashMap<Local, u32>);
    impl Visitor for Uses {
        fn visit_local(&mut self, local: &Local, context: PlaceContext, _: Location) {
            match context {
                PlaceContext::NonUse(U::StorageLive | U::StorageDead | U::VarDebugInfo) => {}
                // a plain definition of the whole local is not a use of it
                PlaceContext::MutatingUse(M::Store | M::Call | M::Yield | M::AsmOutput) => {}
                _ => *self.0.entry(*local).or_insert(0) += 1,
            }
        }
    }
    let mut uses = Uses(HashMap::default());
    for (b, block) in body.basic_blocks.iter().enumerate() {
        uses.visit_basic_block_data(BasicBlock(b as u32), block);
    }
    let mut cand: HashMap<Local, Feed> = HashMap::default();
    for (bi, block) in body.basic_blocks.iter().enumerate() {
        for s in &block.statements {
            if let StatementKind::Assign(_, Rvalue::Aggregate(kind, ops)) = &s.kind {
                if let AggregateKind::Closure(id, _) = &**kind {
                    for (k, o) in ops.iter().enumerate() {
                        if let Operand::Move(p) = o {
                            if p.projection.is_empty() { cand.insert(p.local, Feed::Closure(id.expr, k)); }
                        }
                    }
                }
            }
        }
        if let Some(Terminator { kind: TerminatorKind::Call { args, .. }, .. }) = &block.terminator {
            for (ai, o) in args.iter().enumerate() {
                if let Operand::Move(p) = o {
                    if p.projection.is_empty() { cand.insert(p.local, Feed::Call(bi, ai)); }
                }
            }
        }
    }
    cand.into_iter()
        .filter(|(l, _)| uses.0.get(l).copied() == Some(1) && body.local_decls[l.0 as usize].kind == LocalKind::Temp)
        .collect()
}

fn is_mut_borrow(bk: BorrowKind) -> bool { matches!(bk, BorrowKind::Mut { .. }) }

/// `root(p)` = Some((index, n)): p.projection[n..] continue from the reference `index`
/// (p.projection[n] must be its Deref to reach the pointee; anything else uses the reference
/// itself).
fn scan(env: &FvEnv<'_>, body: &Body, depth: u32, init: Vec<Summary>, root: &dyn Fn(&Place) -> Option<(usize, usize)>) -> Vec<Summary> {
    let types = FvTypes::new(env.tcx, env.prog, env.cx);
    let mut out = init;
    let mut use_count: HashMap<Local, u32> = HashMap::default();
    {
        struct Uses<'a>(&'a mut HashMap<Local, u32>);
        impl Visitor for Uses<'_> {
            fn visit_local(&mut self, local: &Local, context: PlaceContext, _: Location) {
                match context {
                    PlaceContext::NonUse(U::StorageLive | U::StorageDead | U::VarDebugInfo) => {}
                    PlaceContext::MutatingUse(M::Store | M::Call | M::Yield | M::AsmOutput) => {}
                    _ => *self.0.entry(*local).or_insert(0) += 1,
                }
            }
        }
        let mut uses = Uses(&mut use_count);
        for (b, block) in body.basic_blocks.iter().enumerate() {
            crate::preempt::point();
            uses.visit_basic_block_data(BasicBlock(b as u32), block);
        }
    }
    // temps holding a reference that only feeds one call argument
    let feeds: HashMap<Local, (usize, usize)> = find_feeds(body).into_iter().filter_map(|(l, f)| match f { Feed::Call(b, a) => Some((l, (b, a))), _ => None }).collect();
    let callee_summary = |bi: usize, ai: usize| -> Option<Summary> {
        let Some(Terminator { kind: TerminatorKind::Call { func, .. }, .. }) = &body.basic_blocks[bi].terminator else { return None };
        let (d, a) = callee_of(env, func)?;
        summaries(env, d, &a, depth + 1).get(ai).cloned()
    };
    let note = |out: &mut Vec<Summary>, p: &Place, m: bool| {
        let (ai, n0) = match root(p) { Some(x) => x, None => return };
        if ai >= out.len() { return; }
        if p.projection.get(n0) != Some(&PlaceElem::Deref) {
            // the reference itself used as a value (or reborrowed whole elsewhere): it escapes
            out[ai] = Summary::Whole;
            return;
        }
        let mut path = Vec::new();
        for pr in &p.projection[n0 + 1..] {
            match pr {
                PlaceElem::Field(_, ft) => {
                    if types.holds_raw_ptr(*ft) {
                        // a std container owning memory through a raw pointer (Vec, Box, Rc,
                        // String, ...): writes through it are not places the checker sees, so
                        // any use of the field counts as a mutable use of it (`v[0]` then
                        // `v.pop()` overlap on `v`)
                        let std_adt = matches!(env.tcx.tys.kind(*ft), TyKind::Adt(fd, _, _) if types.is_std_def(*fd));
                        if types.owning(*ft) && std_adt {
                            path.push(pr.clone());
                            out[ai].add(path, true);
                            return;
                        }
                        if types.owning(*ft) {
                            // a user struct of owning containers: its fields are places
                            path.push(pr.clone());
                            continue;
                        }
                        // a raw pointer (or a user type around one): what it points to is not
                        // tracked -- the whole value
                        out[ai] = Summary::Whole;
                        return;
                    }
                    path.push(pr.clone())
                }
                PlaceElem::Downcast(..) => path.push(pr.clone()),
                // through a reference field / an index: the field itself is the unit
                _ => break,
            }
        }
        if path.is_empty() && m {
            out[ai] = Summary::Whole;
            return;
        }
        out[ai].add(path, m);
    };
    // the pointee of a root reference exactly (`*param`)?
    let is_root_pointee = |p: &Place| -> bool {
        match root(p) { Some((_, n0)) => p.projection.len() == n0 + 1 && p.projection[n0] == PlaceElem::Deref, None => false }
    };
    let is_root_ref = |p: &Place| -> bool { match root(p) { Some((_, n0)) => p.projection.len() == n0, None => false } };
    for block in &body.basic_blocks {
        crate::preempt::point();
        for s in &block.statements {
            crate::preempt::point();
            if let StatementKind::Assign(dp, rv) = &s.kind {
                note(&mut out, dp, true);
                match rv {
                    Rvalue::Ref(_, bk, p) => {
                        // a reborrow of the whole pointee that only feeds a call: the callee's
                        // summary under it
                        let fed = if is_root_pointee(p) && dp.projection.is_empty() && use_count.get(&dp.local).copied() == Some(1) { feeds.get(&dp.local).copied() } else { None };
                        match fed.and_then(|(bi, ai)| callee_summary(bi, ai)) {
                            Some(Summary::Parts(v)) => {
                                for (sub, m) in v {
                                    let mut q = p.clone();
                                    q.projection.extend(sub);
                                    note(&mut out, &q, m && is_mut_borrow(*bk));
                                }
                            }
                            _ => note(&mut out, p, is_mut_borrow(*bk)),
                        }
                    }
                    Rvalue::RawPtr(_, p) => {
                        if let Some((ai, _)) = root(p) { if ai < out.len() { out[ai] = Summary::Whole; } }
                    }
                    Rvalue::Use(o) | Rvalue::Cast(_, o, _) | Rvalue::Repeat(o, _) | Rvalue::UnaryOp(_, o) => op_note(&note, &mut out, o),
                    Rvalue::Aggregate(_, ops) => for o in ops { op_note(&note, &mut out, o); },
                    Rvalue::BinaryOp(_, ops) => { op_note(&note, &mut out, &ops.0); op_note(&note, &mut out, &ops.1); }
                    Rvalue::Discriminant(p) | Rvalue::CopyForDeref(p) => note(&mut out, p, false),
                    Rvalue::ThreadLocalRef(_) => {}
                }
            }
        }
        if let Some(t) = &block.terminator {
            match &t.kind {
                TerminatorKind::Call { func, args, destination, .. } => {
                    note(&mut out, destination, true);
                    let callee = callee_of(env, func);
                    for (ai, o) in args.iter().enumerate() {
                        // a root reference passed on as it is (a `&self` param handed to a
                        // callee): the callee's summary
                        if let Operand::Copy(p) | Operand::Move(p) = o {
                            if is_root_ref(p) {
                                let cs = callee.as_ref().and_then(|(d, a)| summaries(env, *d, a, depth + 1).get(ai).cloned());
                                match cs {
                                    Some(Summary::Parts(v)) => {
                                        for (sub, m) in v {
                                            let mut q = Place { local: p.local, projection: p.projection.clone() };
                                            q.projection.push(PlaceElem::Deref);
                                            q.projection.extend(sub);
                                            note(&mut out, &q, m);
                                        }
                                    }
                                    _ => note(&mut out, p, true),
                                }
                                continue;
                            }
                        }
                        op_note(&note, &mut out, o);
                    }
                }
                TerminatorKind::SwitchInt { discr, .. } => op_note(&note, &mut out, discr),
                _ => {}
            }
        }
    }
    out
}

fn op_note(note: &dyn Fn(&mut Vec<Summary>, &Place, bool), out: &mut Vec<Summary>, o: &Operand) {
    match o {
        Operand::Copy(p) => note(out, p, false),
        Operand::Move(p) => note(out, p, true),
        Operand::Constant(_) | Operand::RuntimeChecks(_) => {}
    }
}

/// Relaxed Rust (field views on) for the crate of `d`: `[package.metadata.makepad-run] relaxed =
/// true` in its Cargo.toml, or MAKEPAD_RUN_RELAXED=1 / `--relaxed` for every crate.
pub fn relaxed(prog: &Program, d: DefId) -> bool {
    // the crate's relaxed-Rust setting (relaxed.rs): field views and extended two-phase borrows
    let k = prog.def(d).krate as usize;
    prog.crates[k].relaxed(prog).iter().any(|f| f == "field-views" || f == "two-phase")
}

/// Field-view summaries of fn instances (for live patches: before / after an edit), in a
/// caller's summary context (the live edit session's).
pub fn summaries_with(prog: &Program, tcx: &Tcx, bodies: &dyn FvBodies, insts: &[(DefId, Vec<TyId>)], cache: Option<&Rc<SummaryCx>>) -> Vec<Vec<Summary>> {
    let state = crate::tcx::EvalState::new();
    let cx = EvalCx::new(prog, tcx, TypingEnv::fully_monomorphized(prog), &state);
    let own = SummaryCx::new();
    let env = FvEnv { prog, tcx, cx: &cx, cache: cache.unwrap_or(&own), bodies };
    insts.iter().map(|(d, a)| summaries(&env, *d, a, 0).as_ref().clone()).collect()
}

/// "mut {self.n}, ref {self.items}" for a summary of param `k` of `d`.
pub fn describe_summary(prog: &Program, tcx: &Tcx, d: DefId, k: usize, s: &Summary) -> String {
    let pname = if k == 0 && tcx.fn_sig(prog, d).map_or(false, |x| x.self_kind != 0) { "self".to_string() } else { format!("arg{}", k) };
    let pt = tcx.fn_sig(prog, d).and_then(|x| x.params.get(k).copied());
    let state = crate::tcx::EvalState::new();
    let cx = EvalCx::new(prog, tcx, TypingEnv::fully_monomorphized(prog), &state);
    let types = FvTypes::new(tcx, prog, &cx);
    let t = match pt.map(|p| tcx.tys.kind(p).clone()) {
        Some(TyKind::Ref(_, i, _)) => i,
        _ => return "everything".to_string(),
    };
    match s {
        Summary::Whole => format!("all of `*{}`", pname),
        Summary::Parts(v) => {
            let parts: Vec<String> = v.iter().map(|(p, m)| format!("{}`{}.{}`", if *m { "mutably " } else { "" }, pname, path_str(&types, t, p))).collect();
            if parts.is_empty() { "nothing".to_string() } else { parts.join(", ") }
        }
    }
}

pub mod edit;

/// One conflict accepted only with field views / extended two-phase borrows, as the checker
/// found it. `render_records` (diagnostics) turns these into `export::FvRecord`s (source text).
#[derive(Clone, Debug)]
pub struct FvRecordRaw {
    /// the field-level borrow (or access) that is moved out / borrowed again
    pub span: Span,
    /// its place (None for extended two-phase borrows)
    pub place: Option<Place>,
    /// its type implements Default
    pub default: bool,
    /// the conflicting access
    pub access: Span,
    /// extended two-phase borrows: `span` is the call argument holding the access, these are
    /// the arguments before it (the reserved one among them)
    pub hoist: Option<Vec<Span>>,
}

/// A conflict accepted by an analysis stronger than rustc's NLL (relaxed Rust), with the MIR facts
/// its export to rustc needs (`edit::plan_relaxed`).
#[derive(Clone, Debug)]
pub enum RelaxedRecord {
    Polonius(PoloniusRecord),
    Disjoint(DisjointRecord),
}
impl RelaxedRecord {
    /// The same acceptance (a conflict is found again at an activation, or in the Polonius run).
    pub fn same_site(&self, o: &Self) -> bool {
        match (self, o) {
            (Self::Polonius(a), Self::Polonius(b)) => a.span == b.span && a.code == b.code,
            (Self::Disjoint(a), Self::Disjoint(b)) => a.first == b.first && a.second == b.second,
            _ => false,
        }
    }
}

/// Two borrows of elements of one array / slice / `Vec` accepted as disjoint (D3).
#[derive(Clone, Debug)]
pub struct DisjointRecord {
    /// the body's owner (the run-time check is lowered into its code)
    pub owner: DefId,
    /// the index expressions (`v[i]`) of the earlier and the later borrow
    pub first: Span,
    pub second: Span,
    /// the indices are distinct by the checker's proof (else: compared at run time)
    pub proven: bool,
    /// both borrows are `&mut` (the `get_disjoint_mut` rewrite needs that)
    pub both_mut: bool,
    pub code: &'static str,
}

/// An NLL error that Polonius does not have (A2).
#[derive(Clone, Debug)]
pub struct PoloniusRecord {
    pub code: &'static str,
    /// the error's primary span (the conflicting access)
    pub span: Span,
    /// the statement that made the loan NLL kept alive
    pub loan: Span,
    /// the std map lookup the loan is the receiver of, when a second lookup with its key finds the
    /// same entry (the re-lookup rewrite)
    pub lookup: Option<Lookup>,
}

/// `map.get_mut(key)` / `map.get(key)` of std's HashMap / BTreeMap with a std-compared key.
#[derive(Clone, Copy, Debug)]
pub struct Lookup {
    pub mutable: bool,
    /// the call expression
    pub call: Span,
}

/// The field-view context of one body's check (C-NLL4 conflict check hook, check_hook.rs).
pub struct FieldViews<'a> {
    pub prog: &'a Program,
    pub typing_env: TypingEnv,
    pub state: &'a crate::tcx::EvalState,
    pub cache: &'a SummaryCx,
    pub bodies: &'a dyn FvBodies,
    /// relaxed crate: accepted conflicts are reported as such instead of errors
    pub relaxed: bool,
    /// what each closure of the body does through its captured references, by closure expr
    pub closure_fv: HashMap<u32, Vec<Summary>>,
    pub default_trait: Option<DefId>,
}
impl<'a> FieldViews<'a> {
    pub fn new(prog: &'a Program, typing_env: TypingEnv, state: &'a crate::tcx::EvalState, cache: &'a SummaryCx, bodies: &'a dyn FvBodies,
        relaxed: bool, closure_fv: HashMap<u32, Vec<Summary>>) -> Self {
        Self { prog, typing_env, state, cache, bodies, relaxed, closure_fv, default_trait: lang_path(prog, "default::Default") }
    }
    /// Run `f` with the summary environment of this body.
    pub fn with_env<R>(&self, tcx: &Tcx, f: impl FnOnce(&FvEnv<'_>) -> R) -> R {
        let cx = EvalCx::new(self.prog, tcx, self.typing_env, self.state);
        f(&FvEnv { prog: self.prog, tcx, cx: &cx, cache: self.cache, bodies: self.bodies })
    }
}

/// `core::<path>` (also found through core's re-exports), "::"-separated.
pub fn lang_path(prog: &Program, path: &str) -> Option<DefId> {
    let core = prog.syms.get("core")?;
    let mut cur = None;
    for c in 0..prog.crates.len() {
        if prog.crates[c].name == core { cur = Some(prog.mods[prog.crates[c].root_mod as usize].def); }
    }
    let mut cur = cur?;
    for p in path.split("::") {
        let s = prog.syms.get(p)?;
        cur = match prog.lookup_in_container(cur, s, true) {
            Some(d) => d,
            None => prog.lookup_in_container(cur, s, false)?,
        };
    }
    Some(cur)
}

#[cfg(test)]
mod tests;
