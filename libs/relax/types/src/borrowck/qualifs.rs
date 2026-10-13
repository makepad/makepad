//! Owner-scoped promotion queries and const-initializer qualification.
//! rustc_const_eval/check_consts/{qualifs,resolver}.rs. This uses built bodies,
//! never calls borrowck recursively, and never accesses the mutable JIT driver.
use super::bir::*;
use super::body::{BodyKind, CheckedBody};
use super::promote::{Blocked, Qualif, Qualifier, Queries};
use super::tyx::Tx;
use crate::ast::ExprId;
use crate::fxhash::HashMap;
use crate::program::{DefId, DefKind};
use crate::typeck::Body;
use crate::types::{TyId, TyKind};
use std::cell::RefCell;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct Key { owner: DefId, expr: Option<u32> }

/// Confined to one check_body invocation: the program/collection epoch cannot
/// change under the front read lock. Item results use identity substitutions
/// and the item's own environment, like mir_const_qualif(def), then intersect
/// with the instantiated operand type. Inline results belong to this typeck root.
#[derive(Default)]
pub struct Store {
    values: RefCell<HashMap<Key, Option<Result<[bool; 2], Blocked>>>>,
}

pub struct Provider<'a, 'tcx> {
    tx: &'a Tx<'tcx>,
    root: &'a Body,
    store: &'a Store,
    types: RefCell<HashMap<(u8, TyId), bool>>,
}

impl<'a, 'tcx> Provider<'a, 'tcx> {
    pub fn new(tx: &'a Tx<'tcx>, root: &'a Body, store: &'a Store) -> Self {
        Self { tx, root, store, types: RefCell::new(HashMap::default()) }
    }

    fn initializer(&self, owner: DefId, expr: Option<ExprId>) -> Result<[bool; 2], Blocked> {
        let key = Key { owner, expr: expr.map(|e| e.0) };
        if let Some(value) = self.store.values.borrow().get(&key) {
            return value.clone().unwrap_or(Err(Blocked("cycle in constant qualification")));
        }
        self.store.values.borrow_mut().insert(key, None);
        let result = (|| {
            let item;
            let root = if expr.is_some() {
                if owner != self.root.def { return Err(Blocked("inline constant belongs to another typeck root")); }
                self.root
            } else {
                let owner = CheckedBody::item(self.tx.prog, owner).ok_or(Blocked("constant has no initializer body"))?;
                item = self.tx.tcx.typeck(self.tx.prog, owner.owner);
                if !item.errors.is_empty() { return Err(Blocked("constant initializer failed type checking")); }
                &item
            };
            // Separate type-query caches: the initializer's generic environment
            // must not overwrite the parent's Copy/drop memoization.
            // rustc_middle/src/ty/mod.rs:1050: each owner supplies its own predicates.
            let env = crate::tcx::TypingEnv { limit: crate::tcx::RecursionLimit::for_item(self.tx.prog, root.def), param_env: root.param_env.unwrap_or_default(), mode: crate::tcx::TypingMode::borrowck(self.tx.tcx, self.tx.prog, root.def) };
            let tx = Tx::new(self.tx.prog, self.tx.tcx, env, self.tx.cx.state);
            if root.args.iter().any(|a| matches!(tx.kind(*a), TyKind::Param(_))) {
                tx.set_generic_env(root.def, root.args.len());
            }
            let query = Provider::new(&tx, root, self.store);
            let mut builder = super::build::Build::new(&tx, root, tx.prog.def_path(owner));
            if let Some(expr) = expr {
                builder.build_const_child(CheckedBody { owner, kind: BodyKind::InlineConst(expr) });
            } else {
                builder.build_body();
            }
            if builder.out.unsupported.is_some() { return Err(Blocked("constant initializer cannot be represented in BIR")); }
            Ok([return_qualif(&tx, &builder.out, &query, Qualif::NeedsDrop)?,
                return_qualif(&tx, &builder.out, &query, Qualif::HasMutInterior)?])
        })();
        self.store.values.borrow_mut().insert(key, Some(result.clone()));
        result
    }

    fn type_query(&self, q: Qualif, ty: TyId) -> Result<bool, Blocked> {
        let ty = self.tx.normalize(ty);
        let key = (q as u8, ty);
        if let Some(value) = self.types.borrow().get(&key) { return Ok(*value); }
        // Structural auto-trait recursion is coinductive; a recursive ADT can
        // only be sized through indirection. Opaque types are never revealed.
        self.types.borrow_mut().insert(key, false);
        let result = (|| {
            if q == Qualif::HasMutInterior && self.is_unsafe_cell(ty) { return Ok(true); }
            let mut children = Vec::new();
            match self.tx.kind(ty) {
                TyKind::Adt(d, args, _) => {
                    if q == Qualif::HasMutInterior && Some(d) == self.tx.lang("marker::PhantomData") { return Ok(false); }
                    // rustc_ty_utils/needs_drop.rs:353: ManuallyDrop has no
                    // drop glue regardless of its field's type.
                    if q == Qualif::NeedsDrop && Some(d) == self.tx.lang("mem::ManuallyDrop") { return Ok(false); }
                    if q == Qualif::NeedsDrop && self.tx.has_drop_impl(ty) { return Ok(true); }
                    let adt = self.tx.tcx.adts.get(&d).ok_or(Blocked("ADT definition is unavailable for qualification"))?;
                    if q == Qualif::NeedsDrop && adt.is_union { return Ok(false); }
                    for variant in &adt.variants {
                        for field in &variant.fields { children.push(self.tx.tcx.tys.subst(field.ty, self.tx.tcx.tys.list(args))); }
                    }
                }
                TyKind::Tuple(fields) => children = self.tx.tcx.tys.list(fields).to_vec(),
                TyKind::Array(t, n) => {
                    // Freeze applies even to [T; 0]; needs_drop does not.
                    let zero = n.try_to_target_usize(&self.tx.tcx.tys) == Some(0);
                    if q == Qualif::HasMutInterior || !zero { children.push(t); }
                }
                TyKind::Slice(t) => children.push(t),
                TyKind::Closure(_, _, crate::types::ClosureArgs { tupled_upvars_ty: captures, .. }) => {
                    if self.tx.tcx.is_coroutine(ty) { return Err(Blocked("coroutine saved-local types are unavailable for qualification")); }
                    children.push(captures);
                }
                // an escaping bound type is never qualified (rustc: bug!)
                TyKind::Bound(..) => return Err(Blocked("escaping bound type in qualification")),
                TyKind::Param(_) | TyKind::Placeholder(_) | TyKind::Alias(..) | TyKind::Dyn(..) => {
                    let td = if q == Qualif::NeedsDrop { self.tx.copy_trait } else { self.tx.lang("marker::Freeze") };
                    return match td {
                        None => Ok(true),
                        Some(td) => match self.tx.evaluate_trait(td, ty, &[]) {
                            r if r.must_apply_modulo_regions() => Ok(false),
                            crate::next_solver::evaluate_obligation::EvaluationResult::EvaluatedToErr => Ok(true),
                            _ => Err(Blocked("ambiguous trait qualification")),
                        }
                    };
                }
                TyKind::Infer(_) | TyKind::IntVar(_) | TyKind::FloatVar(_) | TyKind::Error => return Err(Blocked("unresolved type in constant qualification")),
                TyKind::Bool | TyKind::Char | TyKind::Str | TyKind::Int(_) | TyKind::Float(_)
                | TyKind::Never | TyKind::Ref(..) | TyKind::Ptr(..) | TyKind::FnPtr(..)
                | TyKind::Foreign(_) | TyKind::FnDef(..) | TyKind::Const(_) => {}
            }
            for child in children { if self.type_query(q, child)? { return Ok(true); } }
            Ok(false)
        })();
        match &result {
            Ok(value) => { self.types.borrow_mut().insert(key, *value); }
            Err(_) => { self.types.borrow_mut().remove(&key); }
        }
        result
    }
}

impl Queries for Provider<'_, '_> {
    fn type_qualif(&self, q: Qualif, ty: TyId) -> Result<bool, Blocked> {
        self.type_query(q, ty)
    }

    fn constant_qualif(&self, q: Qualif, c: &Constant) -> Result<bool, Blocked> {
        let initializer = match &c.kind {
            ConstantKind::Item { def, .. } => {
                let d = self.tx.prog.def(*def);
                // Function items have no const initializer. Trait associated
                // consts use their type, even when they have a default value.
                if matches!(d.kind, DefKind::Const | DefKind::AssocConst)
                    && self.tx.prog.def(d.parent).kind != DefKind::Trait {
                    Some(self.initializer(*def, None)?)
                } else { None }
            }
            ConstantKind::Inline { owner, expr, .. } => Some(self.initializer(*owner, Some(*expr))?),
            ConstantKind::Unknown => return Err(Blocked("BIR constant has no semantic value")),
            _ => None,
        };
        if initializer.map_or(false, |v| !v[q as usize]) { return Ok(false); }
        self.type_qualif(q, c.ty)
    }

    fn is_unsafe_cell(&self, ty: TyId) -> bool {
        matches!(self.tx.kind(ty), TyKind::Adt(d, ..) if Some(d) == self.tx.lang("cell::UnsafeCell"))
    }

    fn scalar(&self, c: &Constant) -> Result<Option<u128>, Blocked> {
        match &c.kind {
            ConstantKind::Scalar(n) => Ok(Some(*n)),
            ConstantKind::Item { def, args } => {
                let args = self.tx.tcx.complete_args(self.tx.prog, *def, args, crate::types::RegionArgsId::EMPTY);
                let value = self.tx.tcx.const_eval_resolve(self.tx.prog, self.tx.typing_env(), *def, &args)
                    .map_err(|_| Blocked("scalar constant could not be evaluated"))?;
                Ok(value.try_to_leaf().map(|v| v.to_bits(v.size())))
            }
            ConstantKind::Unknown | ConstantKind::Promoted(_) => Err(Blocked("constant has no scalar promotion input")),
            _ => Ok(None),
        }
    }

    fn integer_layout(&self, ty: TyId) -> Result<(u32, bool), Blocked> {
        let TyKind::Int(p) = self.tx.kind(ty) else { return Err(Blocked("integer layout requested for a non-integer")); };
        // Types::int_info is Relax's target integer layout (both supported
        // native backends are 64-bit), including isize/usize.
        let (bytes, signed) = crate::types::Types::int_info(p);
        Ok((bytes * 8, signed))
    }
}

#[derive(Clone, PartialEq, Eq)]
struct State { qualif: Vec<bool>, borrow: Vec<bool> }

impl State {
    fn assign<Q: Queries>(&mut self, tx: &Tx, body: &Bir, queries: &Q, q: Qualif, p: &Place, mut value: bool) -> Result<(), Blocked> {
        if p.proj.contains(&Proj::Deref) { return Ok(()); }
        if !value {
            for i in 0..p.proj.len() {
                let ty = super::moves::place_ty_n(tx, body, p, i);
                if matches!(tx.kind(ty), TyKind::Adt(d, ..) if tx.tcx.adts.get(&d).map_or(false, |a| a.is_union))
                    && queries.type_qualif(q, ty)? { value = true; break; }
            }
        }
        // resolver.rs:69: deliberately do not clear on an unqualified store.
        self.qualif[p.local as usize] |= value;
        Ok(())
    }

    fn operand(&mut self, q: Qualif, o: &Operand) {
        if q == Qualif::NeedsDrop {
            if let Operand::Move(p) = o {
                if p.proj.is_empty() && !self.borrow[p.local as usize] { self.qualif[p.local as usize] = false; }
            }
        }
    }

    fn rvalue<Q: Queries>(&mut self, tx: &Tx, body: &Bir, queries: &Q, q: Qualif, v: &Rvalue) -> Result<(), Blocked> {
        match v {
            Rvalue::Use(o) | Rvalue::Repeat(o) | Rvalue::Cast(_, o, _) => self.operand(q, o),
            Rvalue::Op(_, ops) | Rvalue::Aggregate(_, ops) => { for o in ops { self.operand(q, o); } }
            Rvalue::Ref(_, _, p) | Rvalue::RawPtr(_, p) if !p.proj.contains(&Proj::Deref) => {
                let ty = super::moves::place_ty(tx, body, p);
                let mutable = match v {
                    Rvalue::Ref(_, kind, _) => kind.is_mut() || queries.type_qualif(Qualif::HasMutInterior, ty)?,
                    _ => true,
                };
                if mutable && queries.type_qualif(q, ty)? {
                    self.qualif[p.local as usize] = true;
                    self.borrow[p.local as usize] = true;
                }
            }
            _ => {}
        }
        Ok(())
    }
}

/// Forward may-analysis with union joins and rustc's move/borrow transfer
/// effects. Return places are read after convergence, never from syntax.
fn return_qualif<Q: Queries>(tx: &Tx, body: &Bir, queries: &Q, q: Qualif) -> Result<bool, Blocked> {
    let ret_ty = body.ty_of_local(0);
    if !queries.type_qualif(q, ret_ty)? { return Ok(false); }
    let empty = State { qualif: vec![false; body.locals.len()], borrow: vec![false; body.locals.len()] };
    let mut states = vec![None::<State>; body.blocks.len()];
    if states.is_empty() { return Err(Blocked("constant initializer has no CFG")); }
    let mut initial = empty;
    for l in 1..=body.n_args as usize { initial.qualif[l] = queries.type_qualif(q, body.locals[l].ty)?; }
    states[0] = Some(initial);
    let mut pending = std::collections::VecDeque::from([0u32]);
    let mut queued = vec![false; body.blocks.len()];
    queued[0] = true;
    let mut returns = HashMap::default();
    while let Some(bb) = pending.pop_front() {
        queued[bb as usize] = false;
        let mut state = states[bb as usize].clone().unwrap();
        let block = &body.blocks[bb as usize];
        for s in &block.stmts {
            match &s.kind {
                StmtKind::Assign(p, v) => {
                    let value = Qualifier { tx, body, queries, local: |_, l: Local| Ok(state.qualif[l as usize]) }.rvalue(q, v)?;
                    state.assign(tx, body, queries, q, p, value)?;
                    state.rvalue(tx, body, queries, q, v)?;
                }
                StmtKind::StorageDead(l) => { state.qualif[*l as usize] = false; state.borrow[*l as usize] = false; }
                _ => {}
            }
        }
        match block.term.as_ref().map(|t| &t.kind) {
            Some(TermKind::Call { args, dest, target, .. }) => {
                for o in args { state.operand(q, o); }
                if target.is_some() {
                    let value = queries.type_qualif(q, super::moves::place_ty(tx, body, dest))?;
                    state.assign(tx, body, queries, q, dest, value)?;
                }
            }
            Some(TermKind::Switch(o, _)) => state.operand(q, o),
            Some(TermKind::Return) => { returns.insert(bb, state.qualif[0]); }
            _ => {}
        }
        for succ in super::moves::successors(body, bb) {
            let changed = match &mut states[succ as usize] {
                None => { states[succ as usize] = Some(state.clone()); true }
                Some(old) => {
                    let mut changed = false;
                    for (a, b) in old.qualif.iter_mut().zip(&state.qualif).chain(old.borrow.iter_mut().zip(&state.borrow)) {
                        if *b && !*a { *a = true; changed = true; }
                    }
                    changed
                }
            };
            if changed && !queued[succ as usize] { pending.push_back(succ); queued[succ as usize] = true; }
        }
    }
    // Diverging initializer: rustc uses type qualification.
    Ok(returns.is_empty() || returns.values().any(|v| *v))
}
