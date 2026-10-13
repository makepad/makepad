//! Field views (plans/relax-borrowck.md section 6; rust-beyond.md 3.1): per fn instance and
//! reference parameter, the places under `*param` its body reads / writes / borrows, with
//! whether any use is mutable; inferred from the body (transitively through calls that reborrow
//! the whole parameter, recursion to a fixed point). A call that reborrows a place P for a
//! parameter then borrows only P's summary parts instead of all of P.
//!
//! Opaque (the whole `*param`): the reference used as a value (copied, stored, returned,
//! passed where the callee's summary is unknown), raw pointers from it, calls through dyn / fn
//! pointers / closures, bodies the checker cannot build. Interior mutability is a shared read
//! of the field (as Rust). A summary over-approximates its body; nothing else is relaxed.

use super::bir::*;
use super::tyx::Tx;
use crate::fxhash::HashMap;
use crate::program::{DefId, Program};
use crate::tcx::Tcx;
use crate::types::{TyId, TyKind};
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Summary {
    /// not a reference parameter / unknown
    Whole,
    /// paths under `*param` (fields / variants), each with its mutability
    Parts(Vec<(Vec<Proj>, bool)>),
}

impl Summary {
    fn add(&mut self, path: Vec<Proj>, m: bool) {
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
                if ext {
                    m2 |= *pm;
                }
                !ext
            });
            v.push((path, m2));
        }
    }

    pub fn describe(&self, tx: &Tx, t: TyId) -> String {
        match self {
            Summary::Whole => "everything".to_string(),
            Summary::Parts(v) => {
                let mut muts = Vec::new();
                let mut refs = Vec::new();
                for (p, m) in v {
                    let s = path_str(tx, t, p);
                    if *m {
                        muts.push(s);
                    } else {
                        refs.push(s);
                    }
                }
                format!("mut {{{}}}, ref {{{}}}", muts.join(", "), refs.join(", "))
            }
        }
    }
}

pub fn path_str(tx: &Tx, mut t: TyId, p: &[Proj]) -> String {
    let mut out = Vec::new();
    let mut vi = 0;
    for pr in p {
        match pr {
            Proj::Field(fi, ft) => {
                out.push(tx.field_name(t, vi, *fi));
                t = *ft;
                vi = 0;
            }
            Proj::Downcast(v) => vi = *v,
            _ => {}
        }
    }
    if out.is_empty() {
        "*".to_string()
    } else {
        out.join(".")
    }
}

type SummaryKey = (crate::tcx::TypingEnv, DefId, Vec<TyId>);

#[derive(Default)]
pub struct SummaryCx {
    cache: RefCell<HashMap<SummaryKey, (Rc<Vec<Summary>>, Rc<Vec<DefId>>)>>,
    inprog: RefCell<Vec<SummaryKey>>,
    /// per in-progress summary: the fns its computation reached so far
    reached: RefCell<Vec<Vec<DefId>>>,
}

impl SummaryCx {
    /// A context live edits share (reload): each summary remembers the fns it reached, and
    /// an edit drops the summaries that reached one of its changed fns (`invalidate`); every
    /// other summary is the same before and after the edit, and in the next edit.
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

/// Per argument of `d<args>`: its field-view summary.
pub fn summaries(prog: &Program, tcx: &Tcx, tx: &Tx, d: DefId, args: &[TyId], depth: u32) -> Rc<Vec<Summary>> {
    // Field-view summaries describe places, not outlives relations. Their
    // independent body check cannot inherit the caller's inference variables.
    // ty/erase_regions.rs: retain generic arity and nested signature binders.
    let erased: Vec<_> = args.iter().map(|&ty| tcx.tys.erase_regions(ty)).collect();
    let args = erased.as_slice();
    let key = (tx.typing_env(), d, args.to_vec());
    if let Some((s, reach)) = tx.fv.cache.borrow().get(&key).cloned() {
        tx.fv.reached_by_callers(&reach);
        return s;
    }
    if depth > 6 || tx.fv.inprog.borrow().contains(&key) {
        // recursion: the fixed point starts from nothing (grows monotonically: the outer
        // computation includes everything the recursive call would add through its own body)
        tx.fv.reached_by_callers(&[d]);
        return Rc::new(Vec::new());
    }
    tx.fv.inprog.borrow_mut().push(key.clone());
    tx.fv.reached.borrow_mut().push(vec![d]);
    let r = Rc::new(compute(prog, tcx, tx, d, args, depth));
    let mut reach = tx.fv.reached.borrow_mut().pop().unwrap_or_default();
    reach.sort_unstable();
    reach.dedup();
    let reach = Rc::new(reach);
    tx.fv.reached_by_callers(&reach);
    static DBG: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *DBG.get_or_init(|| std::env::var_os("MAKEPAD_RUN_FV_DEBUG").is_some()) {
        let v: Vec<String> = r.iter().map(|x| match x {
            Summary::Whole => "whole".to_string(),
            Summary::Parts(p) => format!("{:?}", p.iter().map(|(q, m)| (q.len(), *m)).collect::<Vec<_>>()),
        }).collect();
        eprintln!("fv: {}{} depth {}: {:?}", "  ".repeat(depth as usize), prog.def_path(d), depth, v);
    }
    tx.fv.inprog.borrow_mut().retain(|x| *x != key);
    tx.fv.cache.borrow_mut().insert(key, (r.clone(), reach));
    r
}

fn compute(prog: &Program, tcx: &Tcx, tx: &Tx, d: DefId, args: &[TyId], depth: u32) -> Vec<Summary> {
    if !matches!(prog.def(d).kind, crate::program::DefKind::Fn | crate::program::DefKind::AssocFn) {
        return Vec::new();
    }
    // T10 (conform): generic callers can leave projections unnormalizable here.
    // normalize_erasing_regions.rs:172-186: failure means the existing unknown
    // summary, which keeps the whole borrow; definition-body summaries belong to T10.
    let cx = crate::tcx::EvalCx::new(prog, tcx, tx.typing_env(), tx.cx.state);
    let body = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| crate::typeck::try_instantiate_body(&cx, prog, tcx, d, args))) {
        Ok(Ok(b)) if b.errors.is_empty() && b.body.is_some() => b,
        Err(p) if crate::preempt::is_cancelled(&*p) => std::panic::resume_unwind(p),
        _ => return Vec::new(),
    };
    let env = crate::tcx::TypingEnv { limit: crate::tcx::RecursionLimit::for_item(prog, body.def), param_env: body.param_env.unwrap_or_default(), mode: crate::tcx::TypingMode::borrowck(tcx, prog, body.def) };
    let mut callee = Tx::new(prog, tcx, env, tx.cx.state);
    callee.fv = tx.fv.clone();
    // These are caller arguments, not the callee's identity parameters. Its
    // declaration environment would give them the wrong indices/bounds and
    // lose ConstArgHasType clauses for consts nested inside type arguments.
    callee.polymorphic.set(body.args.iter().any(|a| matches!(tcx.tys.kind(*a), TyKind::Param(_))));
    let tx = &callee;
    let mut b = super::build::Build::new(tx, &body, prog.def_path(d));
    let built = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        b.build_fn();
    }));
    let ok = match built {
        Err(p) if crate::preempt::is_cancelled(&*p) => std::panic::resume_unwind(p),
        result => result.is_ok(),
    };
    if !ok || b.out.unsupported.is_some() {
        return Vec::new();
    }
    let bir = std::mem::replace(&mut b.out, super::empty());
    // closures of the callee: their captures are uses (Aggregate operands) in this body
    from_bir(prog, tcx, tx, &bir, depth)
}

/// The summaries of a built body's reference parameters.
pub fn from_bir(prog: &Program, tcx: &Tcx, tx: &Tx, bir: &Bir, depth: u32) -> Vec<Summary> {
    let n = bir.n_args as usize;
    let init: Vec<Summary> = (1..=n)
        .map(|a| match tx.kind(bir.locals.get(a).map_or(tx.unit(), |l| l.ty)) {
            TyKind::Ref(..) => Summary::Parts(Vec::new()),
            _ => Summary::Whole,
        })
        .collect();
    // a parameter local: the reference itself is `_a`, what it points to `*_a`
    let root = |p: &Place| -> Option<(usize, usize)> {
        let l = p.local as usize;
        if l >= 1 && l <= n {
            Some((l - 1, 0))
        } else {
            None
        }
    };
    scan(prog, tcx, tx, bir, depth, init, &root)
}

/// The summaries of a closure body's by-reference captures (`*(*env).k`): what the closure
/// does through each captured reference.
pub fn closure_captures(prog: &Program, tcx: &Tcx, tx: &Tx, bir: &Bir) -> Vec<Summary> {
    let caps = &bir.captures;
    let init: Vec<Summary> = caps.iter().map(|c| if c.kind != 0 { Summary::Parts(Vec::new()) } else { Summary::Whole }).collect();
    let by_ref_env = bir.closure_kind != Some(ClosureKind::FnOnce);
    let root = |p: &Place| -> Option<(usize, usize)> {
        if p.local != 1 {
            return None;
        }
        let i = if by_ref_env {
            if p.proj.first() != Some(&Proj::Deref) {
                return None;
            }
            1
        } else {
            0
        };
        match p.proj.get(i) {
            Some(Proj::Field(k, _)) => Some((*k as usize, i + 1)),
            _ => None,
        }
    };
    scan(prog, tcx, tx, bir, 0, init, &root)
}

/// `root(p)` = Some((index, n)): p.proj[n..] continue from the reference `index` (p.proj[n]
/// must be its Deref to reach the pointee; anything else uses the reference itself).
fn scan(prog: &Program, tcx: &Tcx, tx: &Tx, bir: &Bir, depth: u32, init: Vec<Summary>, root: &dyn Fn(&Place) -> Option<(usize, usize)>) -> Vec<Summary> {
    let mut out = init;
    // temps holding a reference that only feeds one call argument
    let mut uses: HashMap<Local, u32> = HashMap::default();
    let mut feeds: HashMap<Local, (usize, usize)> = HashMap::default();
    for (bi, blk) in bir.blocks.iter().enumerate() {
        crate::preempt::point();
        for s in &blk.stmts {
            crate::preempt::point();
            if let StmtKind::Assign(dp, rv) = &s.kind {
                for l in super::nll::rvalue_locals(rv) {
                    *uses.entry(l).or_insert(0) += 1;
                }
                if !dp.proj.is_empty() {
                    *uses.entry(dp.local).or_insert(0) += 1;
                }
            }
        }
        if let Some(t) = &blk.term {
            match &t.kind {
                TermKind::Call { args, .. } => {
                    for (ai, o) in args.iter().enumerate() {
                        if let Operand::Move(p) = o {
                            if p.proj.is_empty() {
                                feeds.insert(p.local, (bi, ai));
                            }
                        }
                        if let Some(p) = o.place() {
                            *uses.entry(p.local).or_insert(0) += 1;
                        }
                    }
                }
                TermKind::Switch(o, _) => {
                    if let Some(p) = o.place() {
                        *uses.entry(p.local).or_insert(0) += 1;
                    }
                }
                _ => {}
            }
        }
    }
    let callee_summary = |bi: usize, ai: usize| -> Option<Summary> {
        match &bir.blocks[bi].term.as_ref()?.kind {
            TermKind::Call { callee: Callee::Fn(d, a) | Callee::Overloaded(d, a), .. } => {
                let s = summaries(prog, tcx, tx, *d, a, depth + 1);
                s.get(ai).cloned()
            }
            _ => None,
        }
    };
    let note = |out: &mut Vec<Summary>, p: &Place, m: bool| {
        let (ai, n0) = match root(p) {
            Some(x) => x,
            None => return,
        };
        if ai >= out.len() {
            return;
        }
        if p.proj.get(n0) != Some(&Proj::Deref) {
            // the reference itself used as a value (or reborrowed whole elsewhere): it escapes
            out[ai] = Summary::Whole;
            return;
        }
        let mut path = Vec::new();
        for pr in &p.proj[n0 + 1..] {
            match pr {
                Proj::Field(_, ft) => {
                    if tx.holds_raw_ptr(*ft) {
                        // a std container owning memory through a raw pointer (Vec, Box, Rc,
                        // String, ...): writes through it are not places the checker sees, so
                        // any use of the field counts as a mutable use of it (`v[0]` then
                        // `v.pop()` overlap on `v`)
                        let std_adt = matches!(tx.kind(*ft), TyKind::Adt(fd, _, _) if tx.is_std_def(fd));
                        if tx.owning(*ft) && std_adt {
                            path.push(*pr);
                            out[ai].add(path, true);
                            return;
                        }
                        if tx.owning(*ft) {
                            // a user struct of owning containers: its fields are places
                            path.push(*pr);
                            continue;
                        }
                        // a raw pointer (or a user type around one): what it points to is not
                        // tracked -- the whole value
                        out[ai] = Summary::Whole;
                        return;
                    }
                    path.push(*pr)
                }
                Proj::Downcast(_) => path.push(*pr),
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
        match root(p) {
            Some((_, n0)) => p.proj.len() == n0 + 1 && p.proj[n0] == Proj::Deref,
            None => false,
        }
    };
    let is_root_ref = |p: &Place| -> bool {
        match root(p) {
            Some((_, n0)) => p.proj.len() == n0,
            None => false,
        }
    };
    for blk in &bir.blocks {
        crate::preempt::point();
        for s in &blk.stmts {
            crate::preempt::point();
            if let StmtKind::Assign(dp, rv) = &s.kind {
                note(&mut out, dp, true);
                match rv {
                    Rvalue::Ref(_, bk, p) => {
                        // a reborrow of the whole pointee that only feeds a call: the callee's
                        // summary under it
                        let fed = if is_root_pointee(p) && dp.proj.is_empty() && uses.get(&dp.local).copied() == Some(1) { feeds.get(&dp.local).copied() } else { None };
                        match fed.and_then(|(bi, ai)| callee_summary(bi, ai)) {
                            Some(Summary::Parts(v)) => {
                                for (sub, m) in v {
                                    let mut q = p.clone();
                                    q.proj.extend(sub);
                                    note(&mut out, &q, m && bk.is_mut());
                                }
                            }
                            _ => note(&mut out, p, bk.is_mut()),
                        }
                    }
                    Rvalue::RawPtr(_, p) => {
                        if let Some((ai, _)) = root(p) {
                            if ai < out.len() {
                                out[ai] = Summary::Whole;
                            }
                        }
                    }
                    Rvalue::Use(o) | Rvalue::Cast(_, o, _) | Rvalue::Repeat(o) => op_note(&note, &mut out, o),
                    Rvalue::Aggregate(_, ops) | Rvalue::Op(_, ops) => {
                        for o in ops {
                            op_note(&note, &mut out, o);
                        }
                    }
                    Rvalue::Discriminant(p) | Rvalue::Len(p) => note(&mut out, p, false),
                }
            }
        }
        if let Some(t) = &blk.term {
            match &t.kind {
                TermKind::Call { callee, args, dest, .. } => {
                    note(&mut out, dest, true);
                    for (ai, o) in args.iter().enumerate() {
                        // a root reference passed on as it is (a `&self` param handed to a
                        // callee): the callee's summary
                        if let Some(p) = o.place() {
                            if is_root_ref(p) {
                                let cs = match callee {
                                    Callee::Fn(d, a) | Callee::Overloaded(d, a) => summaries(prog, tcx, tx, *d, a, depth + 1).get(ai).cloned(),
                                    _ => None,
                                };
                                match cs {
                                    Some(Summary::Parts(v)) => {
                                        for (sub, m) in v {
                                            let mut q = p.deref();
                                            q.proj.extend(sub);
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
                TermKind::Switch(o, _) => op_note(&note, &mut out, o),
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
        Operand::Const(_) => {}
    }
}

/// Relaxed Rust (field views on) for the crate of `d`: `[package.metadata.makepad-run] relaxed =
/// true` in its Cargo.toml, or MAKEPAD_RUN_RELAXED=1 / `--relaxed` for every crate.
pub fn relaxed(prog: &Program, d: DefId) -> bool {
    // the crate's relaxed-Rust setting (relaxed.rs: `[package.metadata.makepad-run] relaxed = ..`,
    // MAKEPAD_RUN_RELAXED, --relaxed): field views and extended two-phase borrows
    let k = prog.def(d).krate as usize;
    prog.crates[k].relaxed(prog).iter().any(|f| f == "field-views" || f == "two-phase")
}
