// ported from N:rustc_const_eval/src/check_consts/{check,ops,mod,qualifs,resolver}.rs @8d1a764
//! Const checking of BIR bodies (rustc_const_eval::check_consts::Checker): the operations a
//! const context (const, static, array length / inline const, `const fn` body) may not
//! perform, found by visiting the body with the flow-sensitive NeedsDrop / HasMutInterior
//! qualifs at each location.
//!
//! Callees of Relax's own std are assumed const-correct: its `const` markings are not a
//! mirror of the real library's, so only trait-method calls (never const on stable) and
//! callees of the user's crates are judged.
use super::bir::*;
use super::body::BodyKind;
use super::diag::Diag;
use super::moves::{place_ty, rpo};
use super::promote::{Blocked, Qualif, Qualifier, Queries};
use super::qualifs::{entry_states, transfer_stmt, transfer_term, State};
use super::tyx::Tx;
use crate::program::DefKind;
use crate::types::{TyId, TyKind};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ConstContext {
    ConstFn,
    Static(bool),
    Const,
}

impl ConstContext {
    /// hir::ConstContext's Display (`constants`, `statics`, `constant functions`).
    fn plural(self) -> &'static str {
        match self {
            ConstContext::ConstFn => "constant functions",
            ConstContext::Static(_) => "statics",
            ConstContext::Const => "constants",
        }
    }
}

/// rustc hir body_const_context: the const context a body is checked in, if any. Closure
/// bodies are not const contexts (a closure is never `const`).
fn const_context(tx: &Tx, bir: &Bir) -> Option<ConstContext> {
    let cb = bir.checked_body?;
    if bir.closure_kind.is_some() { return None; }
    // `#[rust(expr)]` field initial values are generated consts holding arbitrary run-time
    // expressions (front derive.rs default_consts, marked `#[relax_live_default]`): not
    // constants of the program
    if is_live_default(tx, cb.owner) { return None; }
    match cb.kind {
        BodyKind::Const | BodyKind::AssocConst | BodyKind::InlineConst(_) | BodyKind::AnonConst(_) => Some(ConstContext::Const),
        BodyKind::Static => {
            let d = tx.prog.def(cb.owner);
            let ast = &tx.prog.files[d.file as usize].ast;
            match &ast.item(d.item).kind {
                crate::ast::ItemKind::Static(mutable, _, _) => Some(ConstContext::Static(*mutable)),
                _ => Some(ConstContext::Static(false)),
            }
        }
        BodyKind::Function => {
            let sig = tx.tcx.fn_sig(tx.prog, cb.owner)?;
            if sig.is_const { Some(ConstContext::ConstFn) } else { None }
        }
        BodyKind::Closure(_) => None,
    }
}

struct Checker<'a, 'tcx, Q> {
    tx: &'a Tx<'tcx>,
    body: &'a Bir,
    queries: &'a Q,
    kind: ConstContext,
    /// `feature(const_trait_impl)`: trait calls may be const
    const_traits: bool,
    /// `feature(const_destruct)` / `const_precise_live_drops`: drops may be const
    const_drops: bool,
    needs_drop: Vec<Option<State>>,
    has_mut_interior: Vec<Option<State>>,
    transient: Option<Vec<bool>>,
    primary: Vec<Diag>,
    secondary: Vec<Diag>,
}

/// Const-checks every const context among `bodies` (a body, its const children; closures
/// are skipped). Errors are rustc's primary ones; the secondary ones (escaping borrows) only
/// when no primary error was found in the same body.
pub fn check<Q: Queries>(tx: &Tx, queries: &Q, bodies: &[Bir]) -> Result<Vec<Diag>, Blocked> {
    let mut out = Vec::new();
    for bir in bodies {
        crate::preempt::point();
        if bir.unsupported.is_some() || bir.blocks.is_empty() { continue; }
        let Some(kind) = const_context(tx, bir) else { continue };
        let needs_drop = entry_states(tx, bir, queries, Qualif::NeedsDrop)?;
        let has_mut_interior = entry_states(tx, bir, queries, Qualif::HasMutInterior)?;
        let krate = &tx.prog.crates[tx.prog.def(bir.checked_body.map_or(crate::program::NO_DEF, |b| b.owner)).krate as usize];
        let const_traits = krate.features(tx.prog).has("const_trait_impl");
        let const_drops = krate.features(tx.prog).has("const_destruct") || krate.features(tx.prog).has("const_precise_live_drops");
        let mut c = Checker { tx, body: bir, queries, kind, const_traits, const_drops, needs_drop, has_mut_interior, transient: None, primary: Vec::new(), secondary: Vec::new() };
        c.run()?;
        if c.primary.is_empty() { out.extend(c.secondary); } else { out.extend(c.primary); }
    }
    Ok(out)
}

impl<Q: Queries> Checker<'_, '_, Q> {
    fn run(&mut self) -> Result<(), Blocked> {
        for bb in rpo(self.body) {
            let block = &self.body.blocks[bb as usize];
            let mut nd = self.needs_drop[bb as usize].clone().unwrap();
            let mut hmi = self.has_mut_interior[bb as usize].clone().unwrap();
            for st in &block.stmts {
                crate::preempt::point();
                if let StmtKind::Assign(_, v) = &st.kind {
                    self.rvalue(v, st.span, &hmi)?;
                }
                transfer_stmt(&mut nd, self.tx, self.body, self.queries, Qualif::NeedsDrop, &st.kind)?;
                transfer_stmt(&mut hmi, self.tx, self.body, self.queries, Qualif::HasMutInterior, &st.kind)?;
            }
            if let Some(t) = &block.term {
                self.terminator(t, &nd)?;
                transfer_term(&mut nd, self.tx, self.body, self.queries, Qualif::NeedsDrop, &t.kind)?;
                transfer_term(&mut hmi, self.tx, self.body, self.queries, Qualif::HasMutInterior, &t.kind)?;
            }
        }
        Ok(())
    }

    fn error(&mut self, code: &'static str, msg: String, span: Span) {
        self.primary.push(Diag::new(code, msg, span));
    }

    fn secondary(&mut self, code: &'static str, msg: String, span: Span) {
        self.secondary.push(Diag::new(code, msg, span));
    }

    fn operand_ty(&self, o: &Operand) -> TyId {
        match o {
            Operand::Copy(p) | Operand::Move(p) => place_ty(self.tx, self.body, p),
            Operand::Const(c) => c.ty,
        }
    }

    /// check.rs local_is_transient: a local that is storage-dead on every path to a return
    /// cannot outlive the evaluation. Locals without storage markers are always live.
    fn local_is_transient(&mut self, local: Local) -> bool {
        if self.transient.is_none() {
            let n = self.body.locals.len();
            let mut marked = vec![false; n];
            for b in &self.body.blocks {
                for s in &b.stmts {
                    if let StmtKind::StorageLive(l) | StmtKind::StorageDead(l) = &s.kind { marked[*l as usize] = true; }
                }
            }
            // maybe-storage-live: always-live locals start live; StorageLive sets, StorageDead clears
            let mut entry = vec![None::<Vec<bool>>; self.body.blocks.len()];
            entry[0] = Some((0..n).map(|l| !marked[l]).collect());
            let mut pending = vec![0u32];
            let mut live_at_return = vec![false; n];
            while let Some(bb) = pending.pop() {
                let mut st = entry[bb as usize].clone().unwrap();
                let block = &self.body.blocks[bb as usize];
                for s in &block.stmts {
                    match &s.kind {
                        StmtKind::StorageLive(l) => st[*l as usize] = true,
                        StmtKind::StorageDead(l) => st[*l as usize] = false,
                        _ => {}
                    }
                }
                if matches!(block.term.as_ref().map(|t| &t.kind), Some(TermKind::Return)) {
                    for (a, b) in live_at_return.iter_mut().zip(&st) { *a |= *b; }
                }
                for succ in super::moves::successors(self.body, bb) {
                    let changed = match &mut entry[succ as usize] {
                        None => { entry[succ as usize] = Some(st.clone()); true }
                        Some(old) => {
                            let mut c = false;
                            for (a, b) in old.iter_mut().zip(&st) { if *b && !*a { *a = true; c = true; } }
                            c
                        }
                    };
                    if changed { pending.push(succ); }
                }
            }
            self.transient = Some(live_at_return.into_iter().map(|l| !l).collect());
        }
        self.transient.as_ref().unwrap()[local as usize]
    }

    /// check.rs place_may_escape: whether the place can outlive the evaluation of the
    /// initializer. All borrows in a const fn are transient.
    fn place_may_escape(&mut self, p: &Place) -> bool {
        let is_transient = match self.kind {
            ConstContext::ConstFn => true,
            _ => p.proj.contains(&Proj::Deref) || self.local_is_transient(p.local),
        };
        !is_transient
    }

    fn rvalue(&mut self, v: &Rvalue, span: Span, hmi: &State) -> Result<(), Blocked> {
        match v {
            Rvalue::Use(Operand::Const(c)) => {
                if matches!(c.kind, ConstantKind::StaticPointer { thread_local: true, .. }) {
                    self.error("E0625", "thread-local statics cannot be accessed at compile-time".to_string(), span);
                }
            }
            Rvalue::Ref(_, BorrowKind::Mut { .. } | BorrowKind::Unique, p) | Rvalue::RawPtr(true, p) => {
                // static mut: arbitrary mutable references are allowed
                if self.kind != ConstContext::Static(true) && self.place_may_escape(p) {
                    let msg = match self.kind {
                        ConstContext::Static(_) => "mutable borrows of temporaries that have their lifetime extended until the end of the program are not allowed".to_string(),
                        _ => format!("mutable references are not allowed in the final value of {}", self.kind.plural()),
                    };
                    self.secondary("E0764", msg, span);
                }
            }
            Rvalue::Ref(_, BorrowKind::Shared | BorrowKind::Shallow | BorrowKind::Fake, p) | Rvalue::RawPtr(false, p) => {
                let q = Qualifier { tx: self.tx, body: self.body, queries: self.queries, local: |q, l: Local| {
                    if !self.queries.type_qualif(q, self.body.ty_of_local(l))? { return Ok(false); }
                    Ok(hmi.qualif[l as usize])
                } };
                let interior = q.place(Qualif::HasMutInterior, p)?;
                if interior && self.place_may_escape(p) {
                    self.secondary("E0492", format!("{} cannot refer to interior mutable data", self.kind.plural()), span);
                }
            }
            Rvalue::Cast(CastKind::PointerExposeProvenance, _, _) => {
                self.error("", "pointers cannot be cast to integers during const eval".to_string(), span);
            }
            Rvalue::Op(Operation::Binary(op), ops) if ops.len() == 2 => {
                use crate::ast::BinOp::*;
                let lhs = self.operand_ty(&ops[0]);
                if matches!(op, Eq | Ne | Lt | Le | Gt | Ge) && matches!(self.tx.kind(lhs), TyKind::Ptr(..) | TyKind::FnPtr(..)) {
                    self.error("", "pointers cannot be reliably compared during const eval".to_string(), span);
                }
            }
            Rvalue::Op(Operation::TraitComparison(_), _) if !self.const_traits => {
                // a call of PartialEq / PartialOrd: trait methods are never const on stable
                self.error("E0015", format!("cannot call non-const operator in {}", self.kind.plural()), span);
            }
            _ => {}
        }
        Ok(())
    }

    fn terminator(&mut self, t: &Term, nd: &State) -> Result<(), Blocked> {
        match &t.kind {
            TermKind::Call { callee, args, .. } => self.call(callee, args, t.span),
            TermKind::Drop(place, _) => {
                let ty = place_ty(self.tx, self.body, place);
                let needs_drop = if place.proj.is_empty() {
                    self.queries.type_qualif(Qualif::NeedsDrop, ty)? && nd.qualif[place.local as usize]
                } else {
                    self.queries.type_qualif(Qualif::NeedsDrop, ty)?
                };
                if needs_drop && !self.const_drops {
                    // the span of the dropped local's declaration, like rustc
                    let span = if place.proj.is_empty() { self.body.locals[place.local as usize].span } else { t.span };
                    self.error("E0493", format!("destructor of `{}` cannot be evaluated at compile-time", self.tx.ty_str(ty)), span);
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn call(&mut self, callee: &Callee, args: &[Operand], span: Span) -> Result<(), Blocked> {
        let tx = self.tx;
        match callee {
            Callee::Indirect => {
                let mut ty = args.first().map_or(tx.unit(), |a| self.operand_ty(a));
                while let TyKind::Ref(_, inner, _) = tx.kind(ty) { ty = inner; }
                if matches!(tx.kind(ty), TyKind::FnPtr(..)) {
                    self.error("E0015", format!("function pointer calls are not allowed in {}", self.kind.plural()), span);
                } else {
                    self.error("E0015", format!("cannot call non-const closure in {}", self.kind.plural()), span);
                }
            }
            Callee::Fn(d, _) | Callee::Overloaded(d, _) | Callee::Virtual(d, _) => {
                let def = tx.prog.def(*d);
                if !matches!(def.kind, DefKind::Fn | DefKind::AssocFn | DefKind::ForeignFn) { return Ok(()); }
                let parent = def.parent;
                let through_trait = parent != crate::program::NO_DEF && match tx.prog.def(parent).kind {
                    DefKind::Trait => true,
                    DefKind::Impl => tx.tcx.impl_trait.get(&parent).is_some(),
                    _ => false,
                } || matches!(callee, Callee::Virtual(..));
                let non_const = if through_trait {
                    !self.const_traits
                } else if tx.is_std_def(*d) {
                    false
                } else {
                    def.kind == DefKind::ForeignFn || !tx.tcx.fn_sig(tx.prog, *d).map_or(true, |s| s.is_const)
                };
                if non_const {
                    let descr = if tx.tcx.fn_sig(tx.prog, *d).is_some_and(|s| s.self_kind != 0) { "method" }
                        else if def.kind == DefKind::AssocFn { "associated function" } else { "function" };
                    self.error("E0015", format!("cannot call non-const {} `{}` in {}", descr, tx.prog.def_path(*d), self.kind.plural()), span);
                }
            }
        }
        Ok(())
    }
}

fn is_live_default(tx: &Tx, d: crate::program::DefId) -> bool {
    let def = tx.prog.def(d);
    let ast = &tx.prog.files[def.file as usize].ast;
    let src = tx.prog.src(def.file);
    ast.list(ast.item(def.item).attrs).iter().any(|a| crate::cfg::tok_str(src, a.toks.lo as usize) == b"relax_live_default")
}
