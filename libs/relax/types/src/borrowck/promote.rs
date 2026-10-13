//! BIR promotion analysis, following rustc_mir_transform::promote_consts.
//!
//! This component requires the unpromoted body and owner-environment queries.
//! Query failures are distinct from a rustc decision not to promote. In particular,
//! a missing constant-qualification query must not silently become a type-only
//! qualification or a decision to reject a candidate.
use super::bir::*;
use super::body::BodyKind;
use super::moves::{place_ty, place_ty_n};
use super::tyx::Tx;
use crate::ast::{BinOp, UnOp};
use crate::types::{TyId, TyKind};

pub type Location = (Bb, u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TempState {
    Undefined,
    Defined { location: Location, uses: usize, valid: bool },
    Unpromotable,
    PromotedOut,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub location: Location,
}

/// The definition/use information is also consumed by extraction: a multiply
/// used temporary is copied into a promoted instead of removed from its parent.
pub struct Collected {
    pub temps: Vec<TempState>,
    pub candidates: Vec<Candidate>,
}

#[derive(Clone, Copy)]
enum Use {
    Define,
    Read,
    Borrow,
    Mutate,
}

struct Collector<'a> {
    body: &'a Bir,
    out: Collected,
}

impl Collector<'_> {
    fn local(&mut self, local: Local, context: Use, location: Location) {
        if !matches!(self.body.locals[local as usize].kind, LocalKind::Temp | LocalKind::Ret) {
            return;
        }
        let state = &mut self.out.temps[local as usize];
        *state = match (*state, context) {
            (TempState::Undefined, Use::Define) => TempState::Defined { location, uses: 0, valid: false },
            (TempState::Defined { location, uses, valid }, Use::Read | Use::Borrow) => {
                TempState::Defined { location, uses: uses + 1, valid }
            }
            _ => TempState::Unpromotable,
        };
    }

    fn place(&mut self, place: &Place, context: Use, location: Location) {
        // rustc_middle/mir/visit.rs:1216: a projected store/borrow is a
        // mutating projection, not an assignment/borrow of the base local.
        let context = if place.proj.is_empty() { context } else {
            match context {
                Use::Read => Use::Read,
                _ => Use::Mutate,
            }
        };
        self.local(place.local, context, location);
        for p in &place.proj {
            if let Proj::Index(l) = p {
                self.local(*l, Use::Read, location);
            }
        }
    }

    fn operand(&mut self, operand: &Operand, location: Location) {
        if let Some(p) = operand.place() {
            self.place(p, Use::Read, location);
        }
    }

    fn rvalue(&mut self, value: &Rvalue, location: Location) {
        match value {
            Rvalue::Use(o) | Rvalue::Repeat(o) | Rvalue::Cast(_, o, _) => self.operand(o, location),
            Rvalue::Aggregate(_, ops) | Rvalue::Op(_, ops) => {
                for o in ops { self.operand(o, location); }
            }
            Rvalue::Ref(_, kind, p) => {
                self.place(p, if kind.is_mut() { Use::Borrow } else { Use::Read }, location);
                self.out.candidates.push(Candidate { location });
            }
            Rvalue::RawPtr(mutable, p) => {
                self.place(p, if *mutable { Use::Mutate } else { Use::Read }, location);
            }
            Rvalue::Discriminant(p) | Rvalue::Len(p) => self.place(p, Use::Read, location),
        }
    }
}

/// rustc promote_consts.rs:111-170. Storage markers, drops and PlaceMention
/// are non-uses. FakeRead, including match-guard inspections, is a real use.
pub fn collect_temps_and_candidates(body: &Bir) -> Collected {
    let mut c = Collector {
        body,
        out: Collected { temps: vec![TempState::Undefined; body.locals.len()], candidates: Vec::new() },
    };
    if body.blocks.is_empty() { return c.out; }
    for bb in super::moves::rpo(body) {
        let block = &body.blocks[bb as usize];
        for (si, statement) in block.stmts.iter().enumerate() {
            crate::preempt::point();
            let loc = (bb, si as u32);
            match &statement.kind {
                StmtKind::Assign(p, v) => {
                    c.place(p, Use::Define, loc);
                    c.rvalue(v, loc);
                    for (_, callee) in body.agg_callee.iter().filter(|(l, _)| *l == loc) {
                        c.place(callee, Use::Read, loc);
                    }
                }
                StmtKind::FakeRead(_, p) => c.place(p, Use::Read, loc),
                StmtKind::StorageLive(_) | StmtKind::StorageDead(_) | StmtKind::PlaceMention(_) | StmtKind::Nop => {}
            }
        }
        let loc = (bb, block.stmts.len() as u32);
        match block.term.as_ref().map(|t| &t.kind) {
            Some(TermKind::Call { args, dest, .. }) => {
                // BIR retains the operand of a function-item value separately.
                for (_, p) in body.fn_values.iter().filter(|(b, _)| *b == bb) {
                    c.place(p, Use::Read, loc);
                }
                // MIR visits the callee and arguments before the destination.
                for a in args { c.operand(a, loc); }
                c.place(dest, Use::Define, loc);
            }
            Some(TermKind::Switch(o, _)) => c.operand(o, loc),
            Some(TermKind::Return) => c.local(0, Use::Read, loc),
            _ => {}
        }
    }
    c.out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Qualif {
    NeedsDrop,
    HasMutInterior,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Blocked(pub &'static str);

/// Queries must be bound to this body's owner, substitutions, typing environment
/// and collection epoch. They must not borrow a mutable compiler driver while
/// the background checker holds the front-end read lock.
pub trait Queries {
    /// qualifs.rs:80,138: Freeze obligation / needs_drop in the owner's environment.
    fn type_qualif(&self, qualif: Qualif, ty: TyId) -> Result<bool, Blocked>;
    /// qualifs.rs:325-378: consult the initializer for a non-trait constant;
    /// trait associated constants use type qualification. Apply the instantiated
    /// type's qualification as well. Inline consts need this query too.
    fn constant_qualif(&self, qualif: Qualif, constant: &Constant) -> Result<bool, Blocked>;
    /// Exactly UnsafeCell has inherent HasMutInterior; a union is handled by
    /// the caller. Use the lang item's identity, not a type's spelling.
    fn is_unsafe_cell(&self, ty: TyId) -> bool;
    /// Evaluate an eligible constant in the owner environment. None means CTFE
    /// could not produce a scalar, not that the query is missing. The validator
    /// excludes parameters and inline consts before calling this method.
    fn scalar(&self, constant: &Constant) -> Result<Option<u128>, Blocked>;
    /// The target integer width and signedness, including target-sized integers.
    fn integer_layout(&self, ty: TyId) -> Result<(u32, bool), Blocked>;
}

/// Validation does not mutate the body, so a blocked query cannot leave a
/// partially rewritten CFG. `candidates` contains only rustc-validated borrows.
pub struct Validated {
    temps: Vec<TempState>,
    candidates: Vec<Candidate>,
}

impl Validated {
    pub fn candidates(&self) -> &[Candidate] { &self.candidates }
    pub fn temps(&self) -> &[TempState] { &self.temps }
}

struct Validator<'a, 'tcx, Q> {
    tx: &'a Tx<'tcx>,
    body: &'a Bir,
    queries: &'a Q,
    temps: Vec<TempState>,
    safe_blocks: Option<Vec<bool>>,
}

/// Extract validated trees into separate CFGs (promote_consts.rs:725-1093).
/// The caller must retain these bodies with the parent and check them in the
/// parent's region-inference context before using the rewritten parent.
/// Move checking must also visit each returned body independently.
pub fn promote_candidates(tx: &Tx, body: &mut Bir, validated: Validated) -> Vec<Bir> {
    let mut temps = validated.temps;
    let mut bodies = Vec::new();
    let mut inserts = Vec::new();
    for c in validated.candidates.into_iter().rev() {
        let statement = &body.blocks[c.location.0 as usize].stmts[c.location.1 as usize];
        let StmtKind::Assign(dest, Rvalue::Ref(region, kind, place)) = &statement.kind else { unreachable!() };
        if dest.proj.is_empty() && temps[dest.local as usize] == TempState::PromotedOut { continue; }
        let (region, kind, place, span) = (*region, *kind, place.clone(), statement.span);
        let ref_ty = tx.ref_ty(kind.is_mut(), body.ty_of_local(place.local));
        let index = bodies.len() as u32;
        let mut promoted = super::empty();
        promoted.name = format!("{}::promoted[{}]", body.name, index);
        promoted.file = body.file;
        promoted.module = body.module;
        promoted.owner = body.owner;
        promoted.args = body.args.clone();
        promoted.regions = body.regions.clone();
        promoted.closures = body.closures.clone();
        promoted.user_tys = body.user_tys.clone();
        promoted.locals.push(LocalDecl { ty: ref_ty, kind: LocalKind::Ret, mutable: true, can_be_made_mutable: false, name: None, span, regions: Vec::new(), is_ref_binding: false });
        promoted.blocks.push(BasicBlock { stmts: Vec::new(), term: Some(Term { kind: TermKind::Return, span }) });
        let mut p = Promoter { source: body, promoted, temps: &mut temps, keep_original: false };
        let local = p.temp(place.local);
        let last = p.promoted.blocks.last_mut().unwrap();
        last.stmts.push(Statement { kind: StmtKind::Assign(Place::local(0), Rvalue::Ref(region, kind, Place::local(local))), span });
        bodies.push(p.promoted);

        // Keep projections at the original borrow. MIR places cannot have a
        // constant as their base, so materialize the promoted reference first.
        let reference = body.locals.len() as Local;
        body.locals.push(LocalDecl { ty: ref_ty, kind: LocalKind::Temp, mutable: true, can_be_made_mutable: false, name: None, span, regions: Vec::new(), is_ref_binding: false });
        temps.push(TempState::Unpromotable);
        let StmtKind::Assign(_, Rvalue::Ref(_, _, place)) = &mut body.blocks[c.location.0 as usize].stmts[c.location.1 as usize].kind else { unreachable!() };
        place.local = reference;
        place.proj.insert(0, Proj::Deref);
        inserts.push((c.location, Statement {
            kind: StmtKind::Assign(Place::local(reference), Rvalue::Use(Operand::Const(Box::new(Constant { ty: ref_ty, kind: ConstantKind::Promoted(index) })))),
            span,
        }));
    }

    // Remove constraints belonging to definitions that will be nopped or calls
    // replaced with gotos. Their copies now belong to the promoted bodies.
    body.outlives.retain(|(_, _, bb, si)| {
        match body.blocks[*bb as usize].stmts.get(*si as usize).map(|s| &s.kind) {
            Some(StmtKind::Assign(p, _)) if p.proj.is_empty() => temps[p.local as usize] != TempState::PromotedOut,
            _ => true,
        }
    });
    // Nops preserve locations during cleanup. Inserting references afterwards
    // relocates all BIR side-table locations as well as ordinary statements.
    for block in &mut body.blocks {
        crate::preempt::point();
        for stmt in &mut block.stmts {
            crate::preempt::point();
            let remove = match &stmt.kind {
                StmtKind::Assign(p, _) => p.proj.is_empty() && temps[p.local as usize] == TempState::PromotedOut,
                StmtKind::StorageLive(l) | StmtKind::StorageDead(l) => temps[*l as usize] == TempState::PromotedOut,
                _ => false,
            };
            if remove { stmt.kind = StmtKind::Nop; }
        }
        if let Some(Term { kind: TermKind::Drop(p, target), span }) = &block.term {
            if p.proj.is_empty() && temps[p.local as usize] == TempState::PromotedOut {
                block.term = Some(Term { kind: TermKind::Goto(*target), span: *span });
            }
        }
    }
    let alive = |l: Local| temps[l as usize] != TempState::PromotedOut;
    body.local_user_tys.retain(|a| alive(a.local));
    body.ascriptions.retain(|a| alive(a.place.local));
    body.static_temps.retain(|(l, _)| alive(*l));
    body.const_uses.retain(|c| alive(c.local));
    // Statement annotations for a removed definition must not survive on a nop.
    let assignment = |(b, s): Location| matches!(body.blocks[b as usize].stmts.get(s as usize).map(|s| &s.kind), Some(StmtKind::Assign(..)));
    body.rvalue_user_tys.retain(|(l, _)| assignment(*l));
    body.agg_callee.retain(|(l, _)| assignment(*l));
    body.fn_anns.retain(|(l, _)| assignment(*l));
    body.move_sites.retain(|(l, _)| assignment(*l));
    body.union_inits.retain(|l| assignment(*l));
    let call = |b: Bb| matches!(body.blocks[b as usize].term.as_ref().map(|t| &t.kind), Some(TermKind::Call { .. }));
    body.call_user_tys.retain(|(b, _)| call(*b));
    body.fn_values.retain(|(b, _)| call(*b));
    body.static_callees.retain(|(b, _)| call(*b));
    inserts.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
    for (at, stmt) in inserts {
        shift_locations(body, at);
        body.blocks[at.0 as usize].stmts.insert(at.1 as usize, stmt);
    }
    bodies
}

fn shift_locations(body: &mut Bir, at: Location) {
    let shift = |l: &mut Location| { if l.0 == at.0 && l.1 >= at.1 { l.1 += 1; } };
    for (l, _) in &mut body.rvalue_user_tys { shift(l); }
    for (l, _) in &mut body.agg_callee { shift(l); }
    for (l, _) in &mut body.fn_anns { shift(l); }
    for (l, _) in &mut body.move_sites { shift(l); }
    for l in &mut body.union_inits { shift(l); }
    for c in &mut body.const_uses { shift(&mut c.at); }
    for (_, _, b, s) in &mut body.outlives { if *b == at.0 && *s >= at.1 { *s += 1; } }
}

struct Promoter<'a> {
    source: &'a mut Bir,
    promoted: Bir,
    temps: &'a mut Vec<TempState>,
    keep_original: bool,
}

impl Promoter<'_> {
    fn location(&self) -> Location {
        let bb = self.promoted.blocks.len() - 1;
        (bb as Bb, self.promoted.blocks[bb].stmts.len() as u32)
    }

    fn temp(&mut self, local: Local) -> Local {
        let TempState::Defined { location, uses, .. } = self.temps[local as usize] else { panic!("extracting an unvalidated temporary") };
        assert!(uses > 0);
        let old_keep = self.keep_original;
        self.keep_original |= uses > 1;
        if !self.keep_original { self.temps[local as usize] = TempState::PromotedOut; }
        let mut decl = self.source.locals[local as usize].clone();
        decl.kind = LocalKind::Temp;
        decl.name = None;
        let new_local = self.promoted.locals.len() as Local;
        self.promoted.locals.push(decl);
        if let Some((_, d)) = self.source.static_temps.iter().find(|(l, _)| *l == local) {
            self.promoted.static_temps.push((new_local, *d));
        }
        // The written user type keeps its owner context; only local identities
        // and statement locations change. Temporary declarations have no pattern
        // projections referring to other locals.
        for a in self.source.local_user_tys.iter().filter(|a| a.local == local) {
            let mut a = a.clone(); a.local = new_local;
            self.promoted.local_user_tys.push(a);
        }
        for a in self.source.ascriptions.iter().filter(|a| a.place.local == local) {
            let mut a = a.clone(); a.place.local = new_local;
            self.promoted.ascriptions.push(a);
        }
        let block = &self.source.blocks[location.0 as usize];
        if let Some(stmt) = block.stmts.get(location.1 as usize).cloned() {
            let StmtKind::Assign(_, mut rv) = stmt.kind else { unreachable!() };
            self.rvalue(&mut rv);
            let at = self.location();
            self.statement_annotations(location, at, local, new_local);
            let at = self.location();
            self.promoted.blocks[at.0 as usize].stmts.push(Statement { kind: StmtKind::Assign(Place::local(new_local), rv), span: stmt.span });
        } else {
            let mut term = block.term.clone().unwrap();
            let TermKind::Call { args, dest, target, .. } = &mut term.kind else { unreachable!() };
            if !self.keep_original {
                self.source.blocks[location.0 as usize].term = Some(Term { kind: TermKind::Goto(target.expect("promoted call has no normal return")), span: term.span });
            }
            for a in args { self.operand(a); }
            *dest = Place::local(new_local);
            let fn_value = self.source.fn_values.iter().find(|(b, _)| *b == location.0).map(|(_, p)| p.clone());
            let fn_value = fn_value.map(|mut p| { self.place(&mut p); p });
            let at = self.location();
            *target = Some(at.0 + 1);
            if let Some((_, ann)) = self.source.call_user_tys.iter().find(|(b, _)| *b == location.0) {
                self.promoted.call_user_tys.push((at.0, ann.clone()));
            }
            if let Some(p) = fn_value { self.promoted.fn_values.push((at.0, p)); }
            if let Some((_, d)) = self.source.static_callees.iter().find(|(b, _)| *b == location.0) {
                self.promoted.static_callees.push((at.0, *d));
            }
            self.copy_outlives(location, at);
            if !self.keep_original {
                self.source.outlives.retain(|(_, _, b, s)| (*b, *s) != location);
            }
            let span = term.span;
            self.promoted.blocks[at.0 as usize].term = Some(term);
            self.promoted.blocks.push(BasicBlock { stmts: Vec::new(), term: Some(Term { kind: TermKind::Return, span }) });
        }
        self.keep_original = old_keep;
        new_local
    }

    fn place(&mut self, p: &mut Place) {
        p.local = self.temp(p.local);
        for proj in &mut p.proj {
            if let Proj::Index(l) = proj { *l = self.temp(*l); }
        }
    }

    fn operand(&mut self, op: &mut Operand) {
        if let Operand::Copy(p) | Operand::Move(p) = op { self.place(p); }
    }

    fn rvalue(&mut self, v: &mut Rvalue) {
        match v {
            Rvalue::Use(o) | Rvalue::Repeat(o) | Rvalue::Cast(_, o, _) => self.operand(o),
            Rvalue::Ref(_, _, p) | Rvalue::RawPtr(_, p) | Rvalue::Discriminant(p) | Rvalue::Len(p) => self.place(p),
            Rvalue::Aggregate(_, ops) | Rvalue::Op(_, ops) => { for o in ops { self.operand(o); } }
        }
    }

    fn copy_outlives(&mut self, old: Location, new: Location) {
        for &(a, b, bb, si) in &self.source.outlives {
            if (bb, si) == old { self.promoted.outlives.push((a, b, new.0, new.1)); }
        }
    }

    fn statement_annotations(&mut self, old: Location, mut new: Location, local: Local, new_local: Local) {
        // Constructor values are real operands retained in a side table by BIR.
        // Extract them before choosing the final annotation location.
        let callee = self.source.agg_callee.iter().find(|(l, _)| *l == old).map(|(_, p)| p.clone());
        let callee = callee.map(|mut p| { self.place(&mut p); p });
        if callee.is_some() { new = self.location(); }
        if let Some(p) = callee { self.promoted.agg_callee.push((new, p)); }
        for (l, a) in &self.source.rvalue_user_tys {
            if *l == old { self.promoted.rvalue_user_tys.push((new, *a)); }
        }
        for (l, a) in &self.source.fn_anns {
            if *l == old { self.promoted.fn_anns.push((new, a.clone())); }
        }
        for (l, span) in &self.source.move_sites {
            if *l == old { self.promoted.move_sites.push((new, *span)); }
        }
        if self.source.union_inits.contains(&old) { self.promoted.union_inits.push(new); }
        for c in &self.source.const_uses {
            if c.at == old && c.local == local {
                assert!(c.matched.is_none(), "a const-pattern inspection is not an extracted definition");
                let mut c = c.clone(); c.at = new; c.local = new_local;
                self.promoted.const_uses.push(c);
            }
        }
        self.copy_outlives(old, new);
    }
}

pub fn validate_candidates<Q: Queries>(tx: &Tx, body: &Bir, queries: &Q, collected: Collected) -> Result<Validated, Blocked> {
    let mut v = Validator { tx, body, queries, temps: collected.temps, safe_blocks: None };
    let mut candidates = Vec::new();
    for candidate in collected.candidates {
        if v.validate_candidate(candidate)? { candidates.push(candidate); }
    }
    Ok(Validated { temps: v.temps, candidates })
}

/// promote_consts.rs:602: follow only unconditional successful-execution edges.
/// Stop at switches, returns, and false edges, rather than guessing whether a
/// branch is taken. A cycle has no additional guaranteed blocks to add.
pub fn promotion_safe_blocks(body: &Bir) -> Vec<bool> {
    let mut safe = vec![false; body.blocks.len()];
    if safe.is_empty() { return safe; }
    let mut bb = 0;
    while !safe[bb as usize] {
        safe[bb as usize] = true;
        bb = match body.blocks[bb as usize].term.as_ref().map(|t| &t.kind) {
            Some(TermKind::Goto(b) | TermKind::Drop(_, b)) => *b,
            Some(TermKind::Call { target: Some(b), .. }) => *b,
            _ => break,
        };
    }
    safe
}

impl<Q: Queries> Validator<'_, '_, Q> {
    fn is_static(&self) -> bool {
        self.body.checked_body.map_or(false, |b| b.kind == BodyKind::Static)
    }

    fn definition(&self, local: Local) -> Option<Location> {
        match self.temps[local as usize] {
            TempState::Defined { location, .. } => Some(location),
            _ => None,
        }
    }

    fn rvalue_at(&self, (bb, si): Location) -> Option<&Rvalue> {
        match self.body.blocks[bb as usize].stmts.get(si as usize).map(|s| &s.kind) {
            Some(StmtKind::Assign(_, v)) => Some(v),
            _ => None,
        }
    }

    fn qualif_local(&self, q: Qualif, local: Local) -> Result<bool, Blocked> {
        let Some(loc) = self.definition(local) else { return Ok(false) };
        match self.rvalue_at(loc) {
            Some(v) => self.qualif_rvalue(q, v),
            None => self.queries.type_qualif(q, self.body.ty_of_local(local)),
        }
    }

    fn qualif_rvalue(&self, q: Qualif, v: &Rvalue) -> Result<bool, Blocked> {
        Qualifier { tx: self.tx, body: self.body, queries: self.queries,
            local: |q, l| self.qualif_local(q, l) }.rvalue(q, v)
    }

    fn reborrow_base(&self, p: &Place) -> Place {
        if p.proj.last() == Some(&Proj::Deref) && matches!(self.tx.kind(place_ty_n(self.tx, self.body, p, p.proj.len() - 1)), TyKind::Ref(..)) {
            Place { local: p.local, proj: p.proj[..p.proj.len() - 1].to_vec() }
        } else { p.clone() }
    }

    fn validate_candidate(&mut self, c: Candidate) -> Result<bool, Blocked> {
        let Some(Rvalue::Ref(_, kind, p)) = self.rvalue_at(c.location).cloned() else { unreachable!() };
        if !self.validate_local(p.local)? || !self.validate_ref(kind, &p)? { return Ok(false); }
        // Projections stay in the parent. Only promote the underlying temporary.
        Ok(!p.proj.contains(&Proj::Deref))
    }

    fn validate_local(&mut self, l: Local) -> Result<bool, Blocked> {
        let TempState::Defined { location, uses, valid } = self.temps[l as usize] else { return Ok(false) };
        if self.qualif_local(Qualif::NeedsDrop, l)? { return Ok(false); }
        if valid { return Ok(true); }
        let valid = if let Some(v) = self.rvalue_at(location).cloned() {
            let mut valid = self.validate_rvalue(&v)?;
            for (_, callee) in self.body.agg_callee.iter().filter(|(l, _)| *l == location) {
                valid &= self.validate_place(callee)?;
            }
            valid
        } else {
            match self.body.blocks[location.0 as usize].term.as_ref().map(|t| &t.kind).cloned() {
                Some(TermKind::Call { callee, args, .. }) => self.validate_call(&callee, &args, location.0)?,
                _ => false,
            }
        };
        self.temps[l as usize] = if valid { TempState::Defined { location, uses, valid } } else { TempState::Unpromotable };
        Ok(valid)
    }

    fn scalar(&self, c: &Constant) -> Result<Option<u128>, Blocked> {
        // promote_consts.rs:691: evaluating an inline const here cycles back to
        // promotion of its typeck root. Parameters cannot be evaluated either.
        match c.kind {
            ConstantKind::Param { .. } | ConstantKind::Inline { .. } => Ok(None),
            ConstantKind::Unknown => Err(Blocked("BIR constant has no semantic value")),
            ConstantKind::Promoted(_) => Err(Blocked("promotion analysis requires an unpromoted BIR body")),
            ConstantKind::Scalar(n) => Ok(Some(n)),
            _ => self.queries.scalar(c),
        }
    }

    fn validate_place(&mut self, p: &Place) -> Result<bool, Blocked> {
        let Some(last) = p.proj.last() else { return self.validate_local(p.local) };
        let base = Place { local: p.local, proj: p.proj[..p.proj.len() - 1].to_vec() };
        match last {
            Proj::Downcast(_) => return Ok(false),
            Proj::Deref => {
                if !base.proj.is_empty() || !self.is_static() { return Ok(false); }
                let Some(loc) = self.definition(base.local) else { return Ok(false) };
                if !matches!(self.rvalue_at(loc), Some(Rvalue::Use(Operand::Const(c))) if matches!(c.kind, ConstantKind::StaticPointer { thread_local: false, .. })) {
                    return Ok(false);
                }
            }
            Proj::Index(l) => {
                let Some(loc) = self.definition(*l) else { return Ok(false) };
                let Some(Rvalue::Use(Operand::Const(c))) = self.rvalue_at(loc) else { return Ok(false) };
                let Some(idx) = self.scalar(c)? else { return Ok(false) };
                let TyKind::Array(_, len) = self.tx.kind(place_ty(self.tx, self.body, &base)) else { return Ok(false) };
                let Some(len) = len.try_to_target_usize(&self.tx.tcx.tys) else { return Ok(false) };
                if idx >= len as u128 || !self.validate_local(*l)? { return Ok(false); }
            }
            Proj::Field(..) => {
                if let TyKind::Adt(d, ..) = self.tx.kind(place_ty(self.tx, self.body, &base)) {
                    if self.tx.tcx.adts.get(&d).map_or(false, |a| a.is_union) { return Ok(false); }
                }
            }
            Proj::ConstIndex(..) | Proj::Subslice(..) => {}
        }
        self.validate_place(&base)
    }

    fn validate_operand(&mut self, o: &Operand) -> Result<bool, Blocked> {
        match o {
            Operand::Copy(p) | Operand::Move(p) => self.validate_place(p),
            Operand::Const(c) => match c.kind {
                ConstantKind::StaticPointer { thread_local, .. } => Ok(self.is_static() && !thread_local),
                ConstantKind::Unknown => Err(Blocked("BIR constant has no semantic value")),
                ConstantKind::Promoted(_) => Err(Blocked("promotion analysis requires an unpromoted BIR body")),
                _ => Ok(true),
            },
        }
    }

    fn validate_ref(&self, kind: BorrowKind, p: &Place) -> Result<bool, Blocked> {
        match kind {
            BorrowKind::Shared => Ok(!self.qualif_local(Qualif::HasMutInterior, p.local)?),
            BorrowKind::Mut { .. } => Ok(matches!(self.tx.kind(place_ty(self.tx, self.body, p)), TyKind::Array(_, n) if n.try_to_target_usize(&self.tx.tcx.tys) == Some(0))),
            BorrowKind::Fake | BorrowKind::Shallow | BorrowKind::Unique => Ok(false),
        }
    }

    fn operand_ty(&self, o: &Operand) -> TyId {
        match o {
            Operand::Copy(p) | Operand::Move(p) => place_ty(self.tx, self.body, p),
            Operand::Const(c) => c.ty,
        }
    }

    fn validate_rvalue(&mut self, v: &Rvalue) -> Result<bool, Blocked> {
        match v {
            Rvalue::Use(o) | Rvalue::Repeat(o) => self.validate_operand(o),
            Rvalue::Cast(CastKind::PointerExposeProvenance, _, _) => Ok(false),
            Rvalue::Cast(_, o, _) => self.validate_operand(o),
            Rvalue::Discriminant(p) | Rvalue::Len(p) => self.validate_place(p),
            Rvalue::Ref(_, kind, p) => {
                if !self.validate_place(&self.reborrow_base(p))? { return Ok(false); }
                self.validate_ref(*kind, p)
            }
            Rvalue::RawPtr(_, p) => {
                let base = self.reborrow_base(p);
                if base == *p { return Ok(false); }
                self.validate_place(&base)
            }
            Rvalue::Aggregate(_, ops) => {
                for o in ops { if !self.validate_operand(o)? { return Ok(false); } }
                Ok(true)
            }
            Rvalue::Op(op, ops) => {
                match op {
                    Operation::TraitComparison(_) | Operation::PatternTest => {
                        return Err(Blocked("BIR abstract operation has no MIR promotion semantics"));
                    }
                    Operation::Unary(UnOp::Neg | UnOp::Not) => {}
                    Operation::Unary(_) => return Err(Blocked("non-primitive BIR unary operation")),
                    Operation::Binary(op) => {
                        let lhs = self.operand_ty(&ops[0]);
                        if matches!(self.tx.kind(lhs), TyKind::Ptr(..) | TyKind::FnPtr(..)) { return Ok(false); }
                        if matches!(op, BinOp::And | BinOp::Or) {
                            return Err(Blocked("short-circuit operation must be lowered to a CFG"));
                        }
                        if matches!(op, BinOp::Div | BinOp::Rem) && matches!(self.tx.kind(lhs), TyKind::Int(_)) {
                            let Operand::Const(rhs) = &ops[1] else { return Ok(false) };
                            let Some(rhs) = self.scalar(rhs)? else { return Ok(false) };
                            let (bits, signed) = self.queries.integer_layout(lhs)?;
                            assert!((1..=128).contains(&bits));
                            let mask = u128::MAX >> (128 - bits);
                            let rhs = rhs & mask;
                            if rhs == 0 { return Ok(false); }
                            if signed && rhs == mask {
                                let Operand::Const(lhs) = &ops[0] else { return Ok(false) };
                                let Some(lhs) = self.scalar(lhs)? else { return Ok(false) };
                                if lhs & mask == 1u128 << (bits - 1) { return Ok(false); }
                            }
                        }
                    }
                }
                for o in ops { if !self.validate_operand(o)? { return Ok(false); } }
                Ok(true)
            }
        }
    }

    fn validate_call(&mut self, callee: &Callee, args: &[Operand], bb: Bb) -> Result<bool, Blocked> {
        // promote_consts.rs:639: explicit metadata first; the compatibility
        // permission for ordinary const fns is restricted to const/static roots.
        for (_, p) in self.body.fn_values.iter().filter(|(b, _)| *b == bb) {
            if !self.validate_place(p)? { return Ok(false); }
        }
        for a in args { if !self.validate_operand(a)? { return Ok(false); } }
        let d = match callee { Callee::Fn(d, _) | Callee::Overloaded(d, _) => *d, _ => return Ok(false) };
        let Some(sig) = self.tx.tcx.fn_sig(self.tx.prog, d) else { return Ok(false) };
        if sig.rustc_promotable { return Ok(true); }
        if !sig.is_const || !self.body.checked_body.map_or(false, |b| matches!(b.kind, BodyKind::Const | BodyKind::Static | BodyKind::AssocConst | BodyKind::AnonConst(_))) {
            return Ok(false);
        }
        let safe = self.safe_blocks.get_or_insert_with(|| promotion_safe_blocks(self.body));
        Ok(safe[bb as usize])
    }
}

// Shared rustc_const_eval/check_consts/qualifs.rs traversal: promotion supplies
// definition-tree facts, initializer dataflow supplies facts at a CFG location.
pub(super) struct Qualifier<'a, 'tcx, Q, F> {
    pub tx: &'a Tx<'tcx>,
    pub body: &'a Bir,
    pub queries: &'a Q,
    pub local: F,
}

impl<Q: Queries, F: Fn(Qualif, Local) -> Result<bool, Blocked>> Qualifier<'_, '_, Q, F> {
    pub(super) fn place(&self, q: Qualif, place: &Place) -> Result<bool, Blocked> {
        // qualifs.rs:289: projection types can remove a qualif. Dereference
        // cannot use the value of the pointer to refine the pointee's qualifs.
        for (i, projection) in place.proj.iter().enumerate().rev() {
            if let Proj::Index(l) = projection {
                if (self.local)(q, *l)? { return Ok(true); }
            }
            if !self.queries.type_qualif(q, place_ty_n(self.tx, self.body, place, i + 1))? {
                return Ok(false);
            }
            if *projection == Proj::Deref { return Ok(true); }
        }
        (self.local)(q, place.local)
    }

    fn qualif_operand(&self, q: Qualif, o: &Operand) -> Result<bool, Blocked> {
        match o {
            Operand::Copy(p) | Operand::Move(p) => self.place(q, p),
            Operand::Const(c) => self.queries.constant_qualif(q, c),
        }
    }

    pub(super) fn rvalue(&self, q: Qualif, v: &Rvalue) -> Result<bool, Blocked> {
        // qualifs.rs:219: qualifications propagate through values, not merely
        // their types. An empty enum variant need not drop even when its type can.
        match v {
            Rvalue::Use(o) | Rvalue::Repeat(o) | Rvalue::Cast(_, o, _) => self.qualif_operand(q, o),
            Rvalue::Discriminant(p) | Rvalue::Len(p) => self.place(q, p),
            Rvalue::Ref(_, _, p) | Rvalue::RawPtr(_, p) => self.place(q, &reborrow_base(self.tx, self.body, p)),
            Rvalue::Aggregate(k, ops) => {
                if let AggKind::Adt(t, _) = k {
                    let union = match self.tx.kind(*t) {
                        TyKind::Adt(d, ..) => self.tx.tcx.adts.get(&d).map_or(false, |a| a.is_union),
                        _ => false,
                    };
                    let inherent = match q {
                        Qualif::NeedsDrop => self.tx.has_drop_impl(*t),
                        Qualif::HasMutInterior => self.queries.is_unsafe_cell(*t),
                    };
                    if union || inherent { return self.queries.type_qualif(q, *t); }
                }
                for o in ops { if self.qualif_operand(q, o)? { return Ok(true); } }
                Ok(false)
            }
            Rvalue::Op(_, ops) => {
                for o in ops { if self.qualif_operand(q, o)? { return Ok(true); } }
                Ok(false)
            }
        }
    }

}

fn reborrow_base(tx: &Tx, body: &Bir, p: &Place) -> Place {
    if p.proj.last() == Some(&Proj::Deref) && matches!(tx.kind(place_ty_n(tx, body, p, p.proj.len() - 1)), TyKind::Ref(..)) {
        Place { local: p.local, proj: p.proj[..p.proj.len() - 1].to_vec() }
    } else { p.clone() }
}
