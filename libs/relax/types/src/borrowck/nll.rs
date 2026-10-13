//! NLL region inference and the borrow checks (rustc_borrowck: renumber, type_check
//! (constraints), liveness (use-live / drop-live), region_infer (SCCs, values), borrow_set,
//! dataflow Borrows (borrows_out_of_scope, kill_borrows_on_place), check_access_for_conflict
//! with places_conflict, two-phase borrows, universal region checks, closure requirements).
//!
//! Constraints are recorded with the point where they arise (location-sensitive facts); this
//! solver collapses them NLL-style (a region is a set of points, constraints hold everywhere).

use crate::types::AliasTyKind;
use crate::types::{AliasTy};
use super::bir::*;
use super::diag::{describe_place, Diag};
use super::moves::{bit_get, bit_set, bits_new, bits_union, place_ty, predecessors, successors, Bits};
use super::rty::{Ann, Rty, SigTmpl, NAMED_REQ, SK_ELIDED, SK_FRESH, SK_NAMED, SK_PARAM, SR};
use crate::types::variance::V;
use super::tyx::Tx;
use super::constraints::{OutlivesConstraintSet, OutlivesConstraint, ConstraintCategory};
use super::region_infer::{RegionInferenceContext, RegionDefinition, NllRegionVariableOrigin, TypeTest, VerifyBound, RegionErrorKind};
use super::region_infer::values::LivenessValues;
use super::handle_placeholders::{LoweredConstraints, compute_sccs_applying_placeholder_outlives_constraints};
use super::universal_regions::{UniversalRegions, RegionClassification};
use super::infer_ctxt::BorrowckInferCtxt;
use super::renumber::RegionCtxt;
use super::type_check::{Locations, MirTypeckRegionConstraints};
use super::type_check::canonical::fully_perform_op_raw;
use crate::next_solver::type_op::{Normalize, ParamEnvAnd};
use super::type_check::free_region_relations::UniversalRegionRelations;
use crate::types::UniverseIndex;
use crate::fxhash::HashMap;
use crate::types::{TyId, TyKind};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    /// region variable (existential)
    Var,
    /// universal: 'static, the signature's lifetimes, type params' regions
    Univ,
    /// closure body: regions of the closure's type (captures, return) from the parent
    External,
    /// closure body: parameter / env regions (higher-ranked)
    Placeholder,
}

pub struct Loan {
    pub region: Rv,
    pub kind: BorrowKind,
    pub place: Place,
    pub point: usize,
    pub span: Span,
    /// two-phase: the point of the activating use (None: activated at once)
    pub activation: Option<usize>,
    /// the temp / local that holds the reference
    pub holder: Local,
}

/// What a borrow's temp is only used for (field views narrow such a borrow to the summary).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Feed {
    /// argument `ai` of the call terminating block `bi`
    Call(usize, usize),
    /// capture `k` of closure expression `ce`
    Closure(u32, usize),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Depth {
    Shallow,
    ShallowLen,
    ShallowFake,
    Deep,
    Drop,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Rw {
    ReadCopy,
    ReadBorrow(BorrowKind),
    Reservation(BorrowKind),
    Activation(usize),
    WriteMove,
    WriteMutate,
    WriteBorrow(BorrowKind),
    WriteStorageDead,
}

pub struct Nll<'a> {
    pub tx: &'a Tx<'a>,
    pub rty: &'a Rty<'a>,
    pub bir: &'a Bir,
    promoteds: &'a [Bir],
    is_promoted: bool,
    /// Promoted liveness relocated to its parent use.
    promoted_live: Vec<(Rv, usize)>,
    kinds: Vec<Kind>,
    region_ids: Vec<Option<crate::types::RegionId>>,
    names: Vec<String>,
    /// per local: its region variables
    lr: Vec<Vec<Rv>>,
    /// outlives constraints (sup, sub, point)
    cons: Vec<(Rv, Rv, usize)>,
    /// regions a created closure's requirement makes outlive 'static or an owner's named
    /// lifetime (from the closure body's own bound): rustc reports such escapes as E0597, not E0373
    closure_static: Vec<Rv>,
    user_ty_slots_cache: HashMap<u32, Vec<Option<Rv>>>,
    /// the place a Drop terminator being checked drops (E0713)
    cur_drop: Option<Place>,
    /// Ann::Shared ids -> their region
    shared_ann: HashMap<u64, Rv>,
    /// the fn's named lifetimes (user annotations resolve them)
    named_lts: Vec<(Vec<u8>, u32)>,
    /// a closure body: the enclosing fn's lifetimes its annotations name
    outer_named: Vec<(String, Rv)>,
    /// points that assign the return place
    ret_points: std::collections::HashSet<usize>,
    /// a closure body: the higher-ranked regions of its parameters
    hr_regions: Vec<Rv>,
    /// the fn's known lifetime bounds of its type params (where-clauses, implied by the
    /// signature's types): (TyKind::Param index, region)
    tparam_bounds: Vec<(u32, Rv)>,
    tparam_names: Vec<String>,
    proj_bounds: bool,
    /// blocks reachable from the entry (dead code gets no well-formedness obligations)
    reachable: Vec<bool>,
    /// spans of pseudo points (constraints from annotations, located by their type)
    pseudo_spans: Vec<Span>,
    constraint_categories: HashMap<usize,ConstraintCategory>,
    /// type tests `T: 'r` (rustc TypeTest): (param, region, span)
    type_tests: Vec<(u32, Rv, Span)>,
    /// known outlives between universals
    known: Vec<(Rv, Rv)>,
    /// point numbering
    start: Vec<usize>,
    n_points: usize,
    point_block: Vec<u32>,
    pub loans: Vec<Loan>,
    pub diags: Vec<Diag>,
    /// closure bodies: (closure-type slot x, slot y): x must outlive y (to the parent)
    pub requirements: Vec<ClosureOutlivesRequirement>,
    /// external region -> its slot index in the closure type
    ext_slot: HashMap<Rv, u32>,
    /// requirements of the closures this body creates: closure expr -> pairs
    pub closure_reqs: &'a HashMap<u32, Vec<ClosureOutlivesRequirement>>,
    /// closure kind of this body's own closures (expr -> kind)
    reported: Vec<(Place, Span)>,
    region_map: Vec<usize>,
    core_to_bir: Vec<Rv>,
    lowered: Option<LoweredConstraints>,
    regioncx: Option<RegionInferenceContext>,
    dominators: Vec<u32>,
    rpo_index: Vec<u32>,
    /// field views
    relaxed: bool,
    feeds: HashMap<Local, Feed>,
    loan_feed: Vec<Option<Feed>>,
    loan_parts: Vec<Option<Option<Vec<(Place, BorrowKind)>>>>,
    closure_fv: &'a HashMap<u32, Vec<super::fv::Summary>>,
    /// conflicts only field views resolve: (strict diagnostic, which call / closure)
    pub fv_only: Vec<Diag>,
    pub fv_records: Vec<(Span, String, bool, Span, Option<Vec<Span>>)>,
    prog: &'a crate::program::Program,
    tcx: &'a crate::tcx::Tcx,
    /// The body's inference context: every region variable here (`Rv`) is one of its
    /// region variables, including those type operations create.
    infcx: std::rc::Rc<BorrowckInferCtxt<'a>>,
    /// Constraints from type operations (fully_perform_op), in `Rv` numbering.
    typeck_constraints: MirTypeckRegionConstraints,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClosureOutlivesSubject { Region(u32), Ty(TyId) }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClosureOutlivesRequirement { pub subject: ClosureOutlivesSubject, pub outlived_free_region: u32, pub blame_span: Span, pub category: ConstraintCategory }
pub struct NllOut {
    pub diags: Vec<Diag>,
    pub requirements: Vec<ClosureOutlivesRequirement>,
    pub fv_only: Vec<Diag>,
    /// per field-view-only acceptance: the borrow the other side does not touch (its span,
    /// place text, whether its type is Default) -- the export rewrite moves it out around
    pub fv_records: Vec<(Span, String, bool, Span, Option<Vec<Span>>)>,
}

#[allow(clippy::too_many_arguments)]
pub fn run<'a>(tx: &'a Tx, rty: &'a Rty, bir: &'a Bir, closure_reqs: &'a HashMap<u32, Vec<ClosureOutlivesRequirement>>, md: &super::moves::MoveData, relaxed: bool, closure_fv: &'a HashMap<u32, Vec<super::fv::Summary>>, prog: &'a crate::program::Program, tcx: &'a crate::tcx::Tcx) -> NllOut {
    run_with_promoted(tx, rty, bir, &[], closure_reqs, md, relaxed, closure_fv, prog, tcx)
}

/// Promoteds use the parent's inference variables and universal-region context.
/// They must not be passed to `run` independently.
#[allow(clippy::too_many_arguments)]
pub fn run_with_promoted<'a>(tx: &'a Tx, rty: &'a Rty, bir: &'a Bir, promoteds: &'a [Bir], closure_reqs: &'a HashMap<u32, Vec<ClosureOutlivesRequirement>>, md: &super::moves::MoveData, relaxed: bool, closure_fv: &'a HashMap<u32, Vec<super::fv::Summary>>, prog: &'a crate::program::Program, tcx: &'a crate::tcx::Tcx) -> NllOut {
    let def = bir.def.or(bir.owner).unwrap_or(crate::program::NO_DEF);
    let infcx = BorrowckInferCtxt::new(tcx, prog, def, bir.owner.unwrap_or(def), &bir.args);

    let infcx = std::rc::Rc::new(infcx);
    let mut n = Nll::new(tx, rty, bir, promoteds, closure_reqs, relaxed, closure_fv, prog, tcx, infcx);
    n.find_feeds();
    let t0 = crate::clock::cpu_ns();
    n.points();
    rty.cur_owner.set(bir.owner);
    *rty.cur_self.borrow_mut() = None;
    n.renumber();
    n.constraints();
    n.sync_region_vars();
    let rel = n.prepare_regions();
    let t1 = crate::clock::cpu_ns();
    let (live, drop_live) = n.liveness(md, &rel);
    let t2 = crate::clock::cpu_ns();
    n.infer_regions(&live, &drop_live);
    let t3 = crate::clock::cpu_ns();
    PHASE_NS[0].fetch_add(t1 - t0, std::sync::atomic::Ordering::Relaxed);
    PHASE_NS[1].fetch_add(t2 - t1, std::sync::atomic::Ordering::Relaxed);
    PHASE_NS[2].fetch_add(t3 - t2, std::sync::atomic::Ordering::Relaxed);
    let t4 = crate::clock::cpu_ns();
    n.borrow_checks();
    let t5 = crate::clock::cpu_ns();
    n.region_checks();
    let t6 = crate::clock::cpu_ns();
    PHASE_NS[3].fetch_add(t5 - t4, std::sync::atomic::Ordering::Relaxed);
    PHASE_NS[4].fetch_add(t6 - t5, std::sync::atomic::Ordering::Relaxed);
    NllOut { diags: n.diags, requirements: n.requirements, fv_only: n.fv_only, fv_records: n.fv_records }
}

/// cpu ns per phase: constraints, liveness, solve, loans, region checks
pub static PHASE_NS: [std::sync::atomic::AtomicU64; 5] = [const { std::sync::atomic::AtomicU64::new(0) }; 5];

impl<'a> Nll<'a> {
    #[allow(clippy::too_many_arguments)]
    fn new(tx: &'a Tx, rty: &'a Rty, bir: &'a Bir, promoteds: &'a [Bir], closure_reqs: &'a HashMap<u32, Vec<ClosureOutlivesRequirement>>, relaxed: bool, closure_fv: &'a HashMap<u32, Vec<super::fv::Summary>>, prog: &'a crate::program::Program, tcx: &'a crate::tcx::Tcx, infcx: std::rc::Rc<BorrowckInferCtxt<'a>>) -> Self {
        Self {
            tx,
            rty,
            bir,
            promoteds,
            is_promoted: false,
            promoted_live: Vec::new(),
            kinds: Vec::new(),
            region_ids: Vec::new(),
            names: Vec::new(),
            lr: Vec::new(),
            cons: Vec::new(),
            closure_static: Vec::new(),
            user_ty_slots_cache: HashMap::default(),
            cur_drop: None,
            shared_ann: HashMap::default(),
            named_lts: Vec::new(),
            outer_named: Vec::new(),
            ret_points: Default::default(),
            hr_regions: Vec::new(),
            tparam_bounds: Vec::new(),
            tparam_names: Vec::new(),
            proj_bounds: false,
            reachable: Vec::new(),
            pseudo_spans: Vec::new(),
            constraint_categories: Default::default(),
            type_tests: Vec::new(),
            known: Vec::new(),
            start: Vec::new(),
            n_points: 0,
            point_block: Vec::new(),
            loans: Vec::new(),
            diags: Vec::new(),
            requirements: Vec::new(),
            ext_slot: HashMap::default(),
            closure_reqs,
            reported: Vec::new(),
            region_map: vec![],
            core_to_bir: vec![],
            lowered: None,
            regioncx: None,
            dominators: Vec::new(),
            rpo_index: Vec::new(),
            relaxed,
            feeds: HashMap::default(),
            loan_feed: Vec::new(),
            loan_parts: Vec::new(),
            closure_fv,
            fv_only: Vec::new(),
            fv_records: Vec::new(),
            prog,
            tcx,
            infcx,
            typeck_constraints: MirTypeckRegionConstraints::new(0),
        }
    }

    fn new_rv(&mut self, k: Kind, name: &str) -> Rv {
        self.sync_region_vars();
        let origin = if k == Kind::Var { NllRegionVariableOrigin::Existential } else { NllRegionVariableOrigin::FreeRegion };
        let region = self.infcx.next_nll_region_var(origin, || RegionCtxt::Unknown);
        assert_eq!(self.tcx.tys.region_kind(region), crate::types::RegionKind::ReVar(self.kinds.len() as u32));
        self.kinds.push(k);
        self.region_ids.push(None);
        self.names.push(name.to_string());
        (self.kinds.len() - 1) as Rv
    }

    /// Region variables that type operations created in the body's inference context
    /// become existential variables here (placeholders keep their placeholder).
    fn sync_region_vars(&mut self) {
        for vid in self.kinds.len()..self.infcx.num_region_vars() {
            let placeholder = match self.infcx.region_var_origin(vid as u32) {
                crate::infer::region_constraints::RegionVariableOrigin::Nll(NllRegionVariableOrigin::Placeholder(p)) => Some(p),
                _ => None,
            };
            self.kinds.push(Kind::Var);
            self.region_ids.push(placeholder);
            self.names.push("'_".into());
        }
    }

    /// rustc_borrowck/type_check/mod.rs:510: collect in a separate body, but
    /// allocate in the same inference context and relocate constraints/liveness
    /// to the parent's use. Never solve the promoted in isolation.
    fn check_promoted(&mut self, index: u32, use_point: usize) -> Vec<Rv> {
        let mut body = self.promoteds[index as usize].clone();
        let mut regions = vec![STATIC];
        for _ in 1..body.regions.len() { regions.push(self.new_rv(Kind::Var, "'_")); }
        let remap = |r: &mut Rv| { *r = regions[*r as usize]; };
        for b in &mut body.blocks {
            crate::preempt::point();
            for s in &mut b.stmts {
                crate::preempt::point();
                if let StmtKind::Assign(_, Rvalue::Ref(r, _, _)) = &mut s.kind { remap(r); }
            }
            if let Some(Term { kind: TermKind::Call { sig, .. }, .. }) = &mut b.term {
                for rs in &mut sig.inputs { for r in rs { remap(r); } }
                for r in &mut sig.output { remap(r); }
                for (a, b) in &mut sig.outlives { remap(a); remap(b); }
            }
        }
        for l in &mut body.locals { for r in &mut l.regions { remap(r); } }
        for (a, b) in &mut body.known { remap(a); remap(b); }
        for (a, b, _, _) in &mut body.outlives { remap(a); remap(b); }
        self.sync_region_vars();
        let mut n = Nll::new(self.tx, self.rty, &body, self.promoteds, self.closure_reqs, self.relaxed, self.closure_fv, self.prog, self.tcx, self.infcx.clone());
        n.is_promoted = true;
        n.kinds = std::mem::take(&mut self.kinds);
        n.region_ids = std::mem::take(&mut self.region_ids);
        n.names = std::mem::take(&mut self.names);
        n.named_lts = self.named_lts.clone();
        n.outer_named = self.outer_named.clone();
        n.tparam_bounds = self.tparam_bounds.clone();
        n.tparam_names = self.tparam_names.clone();
        n.proj_bounds = self.proj_bounds;
        n.known = self.known.clone();
        n.shared_ann = std::mem::take(&mut self.shared_ann);
        n.points();
        n.renumber();
        n.constraints();
        n.sync_region_vars();
        let rel = vec![true; n.kinds.len()];
        let md = super::moves::MoveData::new(self.tx, &body);
        let (live, drop_live) = n.liveness(&md, &rel);
        for (l, rs) in n.lr.iter().enumerate() {
            if live[l].as_ref().map_or(false, |b| b.iter().any(|w| *w != 0)) {
                self.promoted_live.extend(rs.iter().map(|r| (*r, use_point)));
            }
            if drop_live[l].as_ref().map_or(false, |b| b.iter().any(|w| *w != 0)) {
                let mut slots = Vec::new();
                self.rty.drop_slots(body.locals[l].ty, &mut slots, 0, 0);
                for slot in slots {
                    if let Some(r) = rs.get(slot) { self.promoted_live.push((*r, use_point)); }
                }
            }
        }
        self.promoted_live.extend(n.promoted_live.into_iter().map(|(r, _)| (r, use_point)));
        self.cons.extend(n.cons.into_iter().map(|(a, b, _)| (a, b, use_point)));
        self.kinds = n.kinds;
        self.region_ids = n.region_ids;
        self.names = n.names;
        self.shared_ann = n.shared_ann;
        self.outer_named = n.outer_named;
        self.closure_static.extend(n.closure_static);
        self.type_tests.extend(n.type_tests);
        self.diags.extend(n.diags);
        n.lr.remove(0)
    }

    fn points(&mut self) {
        let mut p = 0;
        for (bi, b) in self.bir.blocks.iter().enumerate() {
            crate::preempt::point();
            self.start.push(p);
            for _ in 0..=b.stmts.len() {
                crate::preempt::point();
                self.point_block.push(bi as u32);
            }
            p += b.stmts.len() + 1;
        }
        self.n_points = p;
    }

    fn pt(&self, b: usize, i: usize) -> usize {
        self.start[b] + i
    }

    fn record_external_slot(&mut self, r: Rv, slot: u32) {
        if let Some(old) = self.ext_slot.insert(r,slot) {
            if old != slot { self.requirements.extend([(old,slot),(slot,old)].map(|(a,b)| ClosureOutlivesRequirement { subject:ClosureOutlivesSubject::Region(a),outlived_free_region:b,blame_span:Span::default(),category:ConstraintCategory::Internal })); }
        }
    }

    fn fresh_slots(&mut self, t: TyId, k: Kind) -> Vec<Rv> {
        let n = self.rty.n_slots(t);
        (0..n).map(|_| self.new_rv(k, "'_")).collect()
    }

    fn renumber(&mut self) {
        // rv 0 = 'static; then the builder's borrow regions (same numbers)
        if !self.is_promoted {
            self.new_rv(Kind::Univ, "'static");
            self.region_ids[0] = Some(self.tcx.tys.intern_region(crate::types::RegionKind::ReStatic));
            for _ in 1..self.bir.regions.len() {
                self.new_rv(Kind::Var, "'_");
            }
        }
        let bir = self.bir;
        let is_closure = bir.closure_kind.is_some();
        // the fn's own signature: universal regions
        let mut tmpl_map: Vec<Rv> = Vec::new();
        let tmpl: Option<std::rc::Rc<SigTmpl>> = if self.is_promoted { None } else if is_closure {
            super::rty::signature::closure_template(self.rty, bir).map(std::rc::Rc::new)
        } else { super::rty::signature::body_template(self.rty, bir) };
        if let Some(t) = &tmpl {
            for s in 0..t.n_syms as usize {
                // a type parameter's regions in an instance are the caller's: their relations
                // are unknown here (rustc checked the generic body with the param opaque), so
                // they are existential, not universal
                let mut k = match t.kinds[s] {
                    SK_NAMED | SK_ELIDED => Kind::Univ,
                    SK_PARAM => Kind::Var,
                    SK_FRESH => Kind::Var,
                    _ => Kind::Var,
                };
                if is_closure {
                    use super::rty::signature::{Classification, Identity};
                    let class = t.typed_regions.iter().find(|(_,r)| *r == SR::Sym(s as u32)).and_then(|(r,_)|
                        t.universal.as_ref()?.regions.iter().find(|u| matches!(u.identity,Identity::Region(_,ur) if ur == *r)).map(|u|u.class));
                    k = match class {
                        Some(Classification::Global) => Kind::Univ,
                        // The old requirement exporter encodes named external
                        // regions as Univ; the typed partition remains External.
                        Some(Classification::External) if t.names[s] != "'_" => Kind::Univ,
                        Some(Classification::External) => Kind::External,
                        Some(Classification::Local) => Kind::Placeholder,
                        None => Kind::Var,
                    };
                }
                let name = t.names[s].clone();
                let r = self.new_rv(k, &name);
                self.region_ids[r as usize] = t.typed_regions.iter().find(|(_,sr)| *sr == SR::Sym(s as u32)).map(|(r,_)| *r)
                    .filter(|r| !t.universal.as_ref().is_some_and(|u| u.query_placeholders.contains(r)));
                tmpl_map.push(r);
            }
        }
        let map_sr = |m: &Vec<Rv>, sr: SR| -> Option<Rv> {
            match sr {
                SR::Static => Some(STATIC),
                SR::Sym(s) => m.get(s as usize).copied(),
                SR::Fresh => None,
            }
        };
        if let Some(t) = &tmpl {
            // only when every self region is the fn's own (named / impl-header elided /
            // 'static); a projection's fresh slots fall back to the written type
            let universal = t.self_slots.iter().all(|sr| match sr {
                SR::Static => true,
                SR::Sym(x) => matches!(t.kinds.get(*x as usize).copied(), Some(SK_NAMED) | Some(SK_ELIDED)),
                SR::Fresh => false,
            });
            if !t.self_slots.is_empty() && universal {
                let v: Option<Vec<Rv>> = t.self_slots.iter().map(|sr| map_sr(&tmpl_map, *sr)).collect();
                *self.rty.cur_self.borrow_mut() = v;
            }
        }
        for (li, l) in bir.locals.iter().enumerate() {
            let n = self.rty.n_slots(l.ty);
            let mut v: Vec<Rv> = Vec::new();
            if let Some(t) = &tmpl {
                if li == 0 && t.output.len() == n {
                    for sr in &t.output {
                        match map_sr(&tmpl_map, *sr) {
                            Some(r) => v.push(r),
                            None => {
                                let r = self.new_rv(if is_closure { Kind::External } else { Kind::Var }, "'_");
                                v.push(r);
                            }
                        }
                    }
                } else if let Some(input) = li.checked_sub(if is_closure { 2 } else { 1 })
                    .filter(|_| l.kind == LocalKind::Arg).and_then(|i| t.inputs.get(i)).filter(|input| input.len() == n) {
                    // A closure parameter's unnumbered regions are the closure type's (external)
                    // only at its external coordinates (external_closure_input_slots): a position
                    // bound by a binder inside the parameter's type (`&dyn for<'a> Fn(&'a T)`) is
                    // local to that type, never a free region of the closure, so it is existential,
                    // whether the parameter is annotated or inferred (the signature's free regions
                    // are numbered by the template).
                    let external_slots = if is_closure {
                        bir.closure_expr.and_then(|ce| self.rty.closures.borrow().get(&(bir.file, ce))
                            .and_then(|cs| cs.params.iter().find(|(k, _, _)| k + 2 == li).map(|(_, _, slots)| slots.clone())))
                            .unwrap_or_default()
                    } else { Vec::new() };
                    for (slot, sr) in input.iter().enumerate() {
                        match map_sr(&tmpl_map, *sr) {
                            Some(r) => v.push(r),
                            None => {
                                let kind = if is_closure && external_slots.contains(&slot) { Kind::External } else { Kind::Var };
                                let r = self.new_rv(kind, "'_");
                                v.push(r);
                            }
                        }
                    }
                }
            }
            if is_closure && v.is_empty() && n > 0 {
                if li == 0 {
                    // the closure's return: its type's return slots (external)
                    v = self.fresh_slots(l.ty, Kind::External);
                } else if li == 1 {
                    // the env: [env ref (placeholder)] + the closure type's slots (external)
                    let t = l.ty;
                    match self.tx.kind(t) {
                        TyKind::Ref(_, ct, _) => {
                            let env = tmpl.as_ref().and_then(|t| t.typed_regions.iter().find(|(r,_)| matches!(self.tcx.tys.region_kind(*r),crate::types::RegionKind::ReLateParam(_,crate::types::LateParamKind::ClosureEnv))).and_then(|(_,r)|map_sr(&tmpl_map,*r)));
                            v.push(env.unwrap_or_else(||self.new_rv(Kind::Placeholder, "'env")));
                            v.extend(self.fresh_slots(ct, Kind::External));
                        }
                        _ => v = self.fresh_slots(t, Kind::External),
                    }
                } else if l.kind == LocalKind::Arg {
                    v = self.fresh_slots(l.ty, Kind::Placeholder);
                    if bir.hr_args.contains(&(li as Local)) {
                        if let Some(r) = v.first() {
                            self.hr_regions.push(*r);
                        }
                    }
                }
            }
            if v.len() != n {
                v = self.fresh_slots(l.ty, Kind::Var);
            }
            self.lr.push(v);
        }
        // closure bodies: the external slots and the return slots are the closure type's
        if is_closure {
            if let Some(env) = self.lr.get(1).cloned() {
                let off = if matches!(self.tx.kind(bir.locals[1].ty), TyKind::Ref(..)) { 1 } else { 0 };
                for (i, r) in env[off..].iter().enumerate() {
                    self.ext_slot.insert(*r, i as u32);
                }
                // `_0`'s slots are the closure type's trailing return slots
                let nret = self.lr[0].len();
                let ncl = env.len() - off;
                if nret <= ncl {
                    let ret_slots: Vec<Rv> = env[off + ncl - nret..].to_vec();
                    if tmpl.is_some() {
                        for (i,r) in self.lr[0].clone().into_iter().enumerate() {
                            if self.kinds[r as usize] == Kind::External { self.record_external_slot(r,(ncl-nret+i) as u32); }
                        }
                    } else { self.lr[0] = ret_slots; }
                }
                // parameters that are not higher-ranked: their regions are the closure type's
                // (after the captures' slots), as rustc infers them in the closure's substs
                if let Some(ce) = bir.closure_expr {
                    let cs = self.rty.closures.borrow().get(&(bir.file, ce)).cloned();
                    if let Some(cs) = cs {
                        let mut o = off + cs.caps.iter().map(|c| self.rty.n_slots(*c)).sum::<usize>();
                        for (k, _, slots) in &cs.params {
                            let n = slots.len();
                            let l = k + 2;
                            if o + n <= env.len() {
                                for (i,&slot) in slots.iter().enumerate() {
                                    if let Some(r) = self.lr.get(l).and_then(|rs|rs.get(slot)).copied() {
                                        if tmpl.is_some() { self.record_external_slot(r,(o-off+i) as u32); }
                                        else { self.lr[l][slot] = env[o+i]; }
                                    }
                                }
                            }
                            o += n;
                        }
                    }
                }
                // the bound's elided output is the higher-ranked parameter's region
                if let Some(a) = bir.hr_ret.filter(|_| tmpl.is_none()) {
                    let hr = self.lr.get(a as usize).and_then(|v| v.first().copied());
                    match hr {
                        Some(r) if self.lr[0].len() == 1 && self.hr_regions.contains(&r) => self.lr[0] = vec![r],
                        _ => {}
                    }
                }
            }
        }
        if is_closure {
            if let Some(t) = &tmpl {
                for r in &tmpl_map { if self.kinds[*r as usize] == Kind::Placeholder && !self.hr_regions.contains(r) { self.hr_regions.push(*r); } }
                for (a,b) in &t.cons {
                    if let (Some(a),Some(b)) = (map_sr(&tmpl_map,*a),map_sr(&tmpl_map,*b)) { self.outlives(a,b,0); }
                }
            }
        }
        // a closure: its owner's `T: 'static` bounds (the only ones a closure's type test can
        // need without the owner's other regions)
        if is_closure {
            if let Some(o) = bir.owner {
                if self.tcx.fn_sig(self.prog, o).is_some() || self.tcx.const_region_contract(self.prog, o).is_some() {
                    let args = self.tcx.generic_indices_of(self.prog, o).identity_types_consts(&self.tcx.tys);
                    let t = self.rty.sig_template(o, &args);
                    if t.ok {
                        // the owner's named lifetimes are universal in the closure too (free in
                        // its signature), with the owner's known relations among them
                        let mut omap: Vec<Option<Rv>> = vec![None; t.n_syms as usize];
                        for s0 in 0..t.n_syms as usize {
                            if t.kinds[s0] == SK_NAMED && t.names[s0] != "'_" && !self.outer_named.iter().any(|x| x.0 == t.names[s0]) {
                                let typed = t.typed_regions.iter().find(|(_,r)| *r == SR::Sym(s0 as u32)).and_then(|(region,_)|
                                    tmpl.as_ref()?.typed_regions.iter().find(|(r,_)|r == region).and_then(|(_,r)|map_sr(&tmpl_map,*r)));
                                let r = typed.unwrap_or_else(|| self.new_rv(Kind::Univ, &t.names[s0]));
                                self.outer_named.push((t.names[s0].clone(), r));
                                omap[s0] = Some(r);
                            }
                        }
                        let ms = |sr: &SR| -> Option<Rv> {
                            match sr {
                                SR::Static => Some(STATIC),
                                SR::Sym(x) => omap.get(*x as usize).copied().flatten(),
                                SR::Fresh => None,
                            }
                        };
                        let rel: Vec<(Rv, Rv)> = t.implied.iter().filter_map(|(a, b)| Some((ms(a)?, ms(b)?))).collect();
                        self.known.extend(rel);
                        for (pi, sr) in &t.tparam_bounds {
                            if *sr == SR::Static {
                                self.tparam_bounds.push((*pi, STATIC));
                            }
                        }
                        self.tparam_names = t.tparam_names.clone();
                        self.proj_bounds = t.proj_bounds;
                    }
                }
            }
        }
        // user type annotations: the local's regions equal the annotation's ('static, the
        // fn's named lifetimes; elided / `'_` ones are unconstrained)
        let mut lts: Vec<(Vec<u8>, u32)> = if self.is_promoted { self.named_lts.clone() } else { Vec::new() };
        if let Some(t) = &tmpl {
            for s in 0..t.n_syms as usize {
                if t.kinds[s] == SK_NAMED && t.names[s] != "'_" {
                    lts.push((t.names[s].as_bytes().to_vec(), tmpl_map[s]));
                }
            }
        }
        self.named_lts = lts.clone();
        // a static's place: its declared type's regions ('static when elided)
        for (l, d) in &bir.static_temps {
            let pd = self.prog.def(*d);
            let sty = match &self.prog.files[pd.file as usize].ast.item(pd.item).kind {
                crate::ast::ItemKind::Static(_, t, _) => *t,
                _ => continue,
            };
            let lt = bir.locals[*l as usize].ty;
            let (pointee, off) = match self.tx.kind(lt) {
                TyKind::Ref(_, x, _) => (x, 1),
                TyKind::Ptr(_, x) => (x, 0),
                _ => continue,
            };
            let ann = self.rty.static_annotation(pd.file, pd.scope, sty, pointee);
            let slots = self.lr[*l as usize].clone();
            if ann.len() + off != slots.len() {
                continue;
            }
            for (i, a) in ann.iter().enumerate() {
                if *a == Ann::Static {
                    let r = slots[off + i];
                    self.outlives(r, STATIC, 0);
                    self.outlives(STATIC, r, 0);
                }
            }
        }
        // rustc check_user_type_annotations / relate_type_and_user_type (type_check/mod.rs:398,
        // :454): a binding's declared user type is related invariantly (:931 visit_local_decl),
        // an ascribed place covariantly (AscribeUserType, :695)
        for lu in &bir.local_user_tys {
            let Some((_pt, ps)) = self.user_ty_projected(lu.ann, &lu.projs, &lts) else { continue };
            let lt = bir.locals[lu.local as usize].ty;
            let slots = self.lr[lu.local as usize].clone();
            let cmp: Vec<Rv> = if lu.by_ref {
                match self.tx.kind(lt) {
                    TyKind::Ref(..) => slots.get(1..).map(|x| x.to_vec()).unwrap_or_default(),
                    _ => continue,
                }
            } else {
                slots
            };
            if ps.len() != cmp.len() {
                continue;
            }
            let ap = self.annotation_point(&bir.user_tys[lu.ann as usize].0);
            for (r, u) in cmp.iter().zip(ps.iter()) {
                if let Some(u) = u {
                    self.outlives(*r, *u, ap);
                    self.outlives(*u, *r, ap);
                }
            }
        }
        for asc in &bir.ascriptions {
            let Some((_pt, ps)) = self.user_ty_projected(asc.ann, &asc.projs, &lts) else { continue };
            let (t, pr) = self.place_regions(&asc.place);
            if pr.len() != ps.len() {
                continue;
            }
            let ap = self.annotation_point(&bir.user_tys[asc.ann as usize].0);
            let var = self.rty.skel(t);
            for (i, (r, u)) in pr.iter().zip(ps.iter()).enumerate() {
                let Some(u) = u else { continue };
                match var.get(i) {
                    Some(V::Co) => self.outlives(*r, *u, ap),
                    Some(V::Contra) => self.outlives(*u, *r, ap),
                    _ => {
                        self.outlives(*r, *u, ap);
                        self.outlives(*u, *r, ap);
                    }
                }
            }
        }
        // type params' lifetime bounds: where-clauses, and implied by the signature's types
        // (`&'a T` / `&'a Vec<T>` give `T: 'a`)
        if let Some(t) = &tmpl {
            for (pi, sr) in &t.tparam_bounds {
                if let Some(r) = map_sr(&tmpl_map, *sr) {
                    self.tparam_bounds.push((*pi, r));
                }
            }
            self.tparam_names = t.tparam_names.clone();
            self.proj_bounds = t.proj_bounds;
        }
        // known relations among universals: implied bounds, where-clauses
        if let Some(t) = &tmpl {
            for (a, b) in t.implied.iter() {
                if let (Some(x), Some(y)) = (map_sr(&tmpl_map, *a), map_sr(&tmpl_map, *b)) {
                    self.known.push((x, y));
                }
            }
        }
        if !self.is_promoted {
            let body = self.new_rv(Kind::Placeholder, "'fn_body");
            for r in 0..self.kinds.len() as Rv {
                if self.kinds[r as usize] != Kind::Var {
                    self.known.extend([(r,r),(STATIC,r),(r,body)]);
                }
            }
            if !is_closure { if let Some(t) = &tmpl { self.normalize_signature(t); } }
        }
    }

    /// rustc_borrowck type_check/free_region_relations.rs:405-433 (create): the inputs and
    /// output are normalized through `fully_perform_op`; the region constraints of that
    /// normalization hold at every point of the body (Locations::All, Internal).
    /// BIR adapter: the signature's regions are mapped to this body's universal region
    /// variables first, as nightly's universal regions are; a type with a free region the
    /// template did not number is left to the template.
    fn normalize_signature(&mut self, t: &SigTmpl) {
        use crate::types::RegionKind;
        let Some(u) = &t.universal else { return };
        let mut regions = Vec::new();
        let mut indices = Vec::new();
        for (r, k) in self.kinds.iter().enumerate() {
            if *k == Kind::Var || self.is_placeholder(r) { continue; }
            let class = if r == STATIC as usize { RegionClassification::Global }
                else if *k == Kind::External || self.outer_named.iter().any(|x| x.1 as usize == r) { RegionClassification::External }
                else { RegionClassification::Local };
            regions.push((r, class));
            if let Some(id) = self.region_ids[r] { indices.push((id, r)); }
        }
        let fr_fn_body = self.names.iter().position(|n| n == "'fn_body");
        let mut universal_regions = UniversalRegions::new(STATIC as usize, fr_fn_body, regions);
        universal_regions.indices = indices;
        let known: Vec<_> = self.infcx.param_env.clauses.iter()
            .filter(|p| matches!(p.kind, crate::next_solver::PredicateKind::Clause(crate::next_solver::ClauseKind::TypeOutlives(..)))).cloned().collect();
        // def_span of the defining item.
        let span = self.bir.def.map_or(Span::default(), |d| {
            let pd = self.prog.def(d);
            let item = self.prog.files[pd.file as usize].ast.item(pd.item);
            Span { lo: item.lo, hi: item.hi }
        });
        let tys = &self.tcx.tys;
        for &ty in u.inputs.iter().chain([&u.output]) {
            let unmapped = std::cell::Cell::new(false);
            let ty = tys.map_regions(tys.subst(ty, &self.bir.args), &|r, depth| match tys.region_kind(r) {
                RegionKind::ReBound(debruijn, _) if debruijn < depth => r,
                _ => match universal_regions.indices.iter().find(|(o, _)| *o == r) {
                    Some(&(_, v)) => tys.intern_region(RegionKind::ReVar(v as u32)),
                    None => { unmapped.set(true); r }
                },
            }, 0);
            if unmapped.get() { continue; }
            let op = ParamEnvAnd { param_env: self.infcx.param_env_id(), value: Normalize { value: ty } };
            let _ = fully_perform_op_raw(&self.infcx, &universal_regions, &t.assumptions.type_outlives, &known,
                &mut self.typeck_constraints, Locations::All(crate::mir::Span { file: self.bir.file, lo: span.lo, hi: span.hi }), crate::mir::Span { file: self.bir.file, lo: span.lo, hi: span.hi }, ConstraintCategory::Internal, op);
        }
    }

    /// The region a user annotation names: in a closure body, a lifetime of the enclosing
    /// fn is a placeholder (universal in the closure, its relations lenient)
    /// The region of a lifetime written by name in this body (an enclosing fn's, in a closure).
    fn named_lifetime_rv(&mut self, n: &str) -> Option<Rv> {
        match self.named_lts.iter().find(|x| x.0.as_slice() == n.as_bytes()) {
            Some((_, r)) => Some(*r),
            None => self.ann_rv(&Ann::Named(n.to_string())),
        }
    }

    /// The point carrying the span of the written type `at` (blame for annotation errors).
    fn annotation_point(&mut self, src: &UserAnn) -> usize {
        let at = match src {
            UserAnn::Ty(at) => *at,
            _ => return 0,
        };
        match self.bir.wf_annots.iter().find(|w| w.0 == at) {
            Some(w) => {
                self.pseudo_spans.push(w.2);
                self.n_points + self.pseudo_spans.len() - 1
            }
            None => 0,
        }
    }

    /// The regions of user type annotation `ann`, instantiated once per annotation (rustc
    /// instantiate_canonical): the projections of one annotation share its region variables.
    fn user_ty_regions(&mut self, ann: u32, lts: &[(Vec<u8>, u32)]) -> Option<Vec<Option<Rv>>> {
        if let Some(v) = self.user_ty_slots_cache.get(&ann) {
            return Some(v.clone());
        }
        let bir = self.bir;
        let (src, t) = bir.user_tys[ann as usize].clone();
        let v: Vec<Option<Rv>> = match src {
            UserAnn::Ty(at) => {
                let a = self.rty.annotation(bir.file, bir.module, at, t, lts);
                a.iter().map(|x| self.ann_rv(x)).collect()
            }
            UserAnn::Path(pargs) => self.path_annotation(t, &pargs)?,
            UserAnn::SelfTy => {
                // `Self(..)`: the impl's self type (its regions in this fn when known)
                if let Some(v) = self.rty.cur_self.borrow().clone() {
                    if v.len() == self.rty.n_slots(t) {
                        v.into_iter().map(Some).collect()
                    } else {
                        return None;
                    }
                } else {
                    let (f, m, at) = self.rty.owner_impl_self()?;
                    let a = self.rty.annotation(f, m, at, t, lts);
                    a.iter().map(|x| self.ann_rv(x)).collect()
                }
            }
        };
        if v.len() != self.rty.n_slots(t) {
            return None;
        }
        self.user_ty_slots_cache.insert(ann, v.clone());
        Some(v)
    }

    /// The regions of user type annotation `ann`, projected by `projs` (rustc
    /// UserTypeProjection; relate_type_and_user_type walks `projection_ty_core`): None when the
    /// shapes do not line up.
    fn user_ty_projected(&mut self, ann: u32, projs: &[Proj], lts: &[(Vec<u8>, u32)]) -> Option<(TyId, Vec<Option<Rv>>)> {
        let bir = self.bir;
        let (_src, mut t) = bir.user_tys[ann as usize].clone();
        let mut rs = self.user_ty_regions(ann, lts)?;
        let mut vi = 0u32;
        for pr in projs {
            match pr {
                Proj::Deref => match self.tx.kind(t) {
                    TyKind::Ref(_, inner, _) => {
                        rs = rs.get(1..)?.to_vec();
                        t = inner;
                    }
                    TyKind::Ptr(_, inner) => t = inner,
                    _ => return None,
                },
                Proj::Field(fi, ft) => {
                    let fm = self.rty.field_slots(t, vi, *fi);
                    let out: Vec<Option<Rv>> = fm.iter().map(|sr| match sr {
                        SR::Sym(i) => rs.get(*i as usize).copied().flatten(),
                        SR::Static => Some(STATIC),
                        SR::Fresh => None,
                    }).collect();
                    rs = out;
                    t = *ft;
                    vi = 0;
                    continue;
                }
                Proj::Downcast(v) => {
                    vi = *v;
                    continue;
                }
                _ => return None,
            }
            vi = 0;
        }
        if rs.len() != self.rty.n_slots(t) {
            return None;
        }
        Some((t, rs))
    }

    fn ann_rv(&mut self, a: &Ann) -> Option<Rv> {
        match a {
            Ann::Static => Some(STATIC),
            Ann::Rv(x) => Some(*x as Rv),
            Ann::Shared(id) => {
                if let Some(r) = self.shared_ann.get(id) {
                    return Some(*r);
                }
                let r = self.new_rv(Kind::Var, "'_");
                self.shared_ann.insert(*id, r);
                Some(r)
            }
            Ann::Named(n) if self.bir.closure_kind.is_some() || self.is_promoted => {
                if let Some((_, r)) = self.outer_named.iter().find(|x| x.0 == *n) {
                    return Some(*r);
                }
                let r = self.new_rv(Kind::Placeholder, n);
                self.outer_named.push((n.clone(), r));
                Some(r)
            }
            _ => None,
        }
    }

    // ------------------------------------------------------------------ constraints

    fn outlives(&mut self, sup: Rv, sub: Rv, p: usize) {
        // 'static: x always holds
        if sup != sub && sup != STATIC {
            self.cons.push((sup, sub, p));
        }
    }

    /// value of type `src` flows into a place of type `dst` (src <: dst) over equal skeletons.
    fn relate(&mut self, src: &[Rv], dst: &[Rv], var: &[V], p: usize) {
        if src.len() != dst.len() || var.len() != dst.len() {
            return;
        }
        for i in 0..dst.len() {
            match var[i] {
                V::Co => self.outlives(src[i], dst[i], p),
                V::Contra => self.outlives(dst[i], src[i], p),
                V::Inv => {
                    self.outlives(src[i], dst[i], p);
                    self.outlives(dst[i], src[i], p);
                }
                V::Bi => {}
            }
        }
    }

    /// Region variables of a place's type (projections from its local's).
    fn place_regions(&mut self, p: &Place) -> (TyId, Vec<Rv>) {
        let mut t = self.bir.locals[p.local as usize].ty;
        let mut rs = self.lr[p.local as usize].clone();
        let mut vi = 0u32;
        for pr in &p.proj {
            match pr {
                Proj::Deref => match self.tx.kind(t) {
                    TyKind::Ref(_, inner, _) => {
                        rs = rs.get(1..).map(|x| x.to_vec()).unwrap_or_default();
                        t = inner;
                    }
                    TyKind::Ptr(_, inner) => t = inner,
                    _ => {
                        if let Some(inner) = self.tx.box_inner(t) {
                            let n = self.rty.n_slots(inner);
                            rs.truncate(n);
                            t = inner;
                        }
                    }
                },
                Proj::Field(fi, ft) => {
                    let fm = self.rty.field_slots(t, vi, *fi);
                    let mut out = Vec::with_capacity(fm.len());
                    for sr in fm.iter() {
                        out.push(match sr {
                            SR::Sym(i) => rs.get(*i as usize).copied().unwrap_or(STATIC),
                            SR::Static => STATIC,
                            SR::Fresh => self.new_rv(Kind::Var, "'_"),
                        });
                    }
                    rs = out;
                    t = *ft;
                    vi = 0;
                    continue;
                }
                Proj::Index(_) | Proj::ConstIndex(..) => {
                    t = match self.tx.kind(t) {
                        TyKind::Array(e, _) | TyKind::Slice(e) => e,
                        _ => t,
                    };
                }
                Proj::Subslice(..) => {}
                Proj::Downcast(v) => {
                    vi = *v;
                    continue;
                }
            }
            vi = 0;
        }
        let n = self.rty.n_slots(t);
        if rs.len() != n {
            rs = (0..n).map(|_| self.new_rv(Kind::Var, "'_")).collect();
        }
        (t, rs)
    }

    fn op_regions(&mut self, o: &Operand) -> Option<(TyId, Vec<Rv>)> {
        o.place().map(|p| {
            let p = p.clone();
            self.place_regions(&p)
        })
    }

    /// rustc: a constant's type equals its path's user type (invariant, type_check/mod.rs:454
    /// relate_type_and_user_type via Constant.user_ty). The const's declared type over its impl /
    /// trait generics, with the user type written on the path related to their regions.
    fn const_use_constraints(&mut self) {
        let bir = self.bir;
        for cu in &bir.const_uses {
            if !self.reachable.get(cu.at.0 as usize).copied().unwrap_or(false) {
                continue;
            }
            let t = self.rty.sig_template(cu.def, &cu.args);
            let dr = self.lr[cu.local as usize].clone();
            if !t.ok || t.output.len() != dr.len() || t.output.iter().any(|x| *x == SR::Fresh) {
                continue;
            }
            let p = self.pt(cu.at.0 as usize, cu.at.1 as usize);
            let map: Vec<Rv> = (0..t.n_syms).map(|_| self.new_rv(Kind::Var, "'_")).collect();
            let m = |sr: &SR| -> Rv {
                match sr {
                    SR::Sym(s) => map.get(*s as usize).copied().unwrap_or(STATIC),
                    _ => STATIC,
                }
            };
            for (x, y) in t.output.iter().map(|x| m(x)).zip(dr.iter().copied()) {
                self.outlives(x, y, p);
                self.outlives(y, x, p);
            }
            for (a, b) in &t.cons {
                self.outlives(m(a), m(b), p);
            }
            let lay: Vec<Option<Rv>> = t.layout().iter().map(|sr| if *sr == SR::Fresh { None } else { Some(m(sr)) }).collect();
            let ann = self.user_ty_slots(&t, &cu.user, true);
            self.equate_slots(&lay, &ann, p);
            // a const pattern's type is a subtype of the matched place's (rustc checks match
            // arms contravariantly: issue-57280-1-flipped)
            if let Some(mp) = &cu.matched {
                let (pt, pr) = self.place_regions(mp);
                if pr.len() == dr.len() {
                    let var = self.rty.skel(pt);
                    self.relate(&dr, &pr, &var, p);
                }
            }
        }
    }

    fn constraints(&mut self) {
        let bir = self.bir;
        // annotations' types are well-formed (also in dead code, as rustc)
        for (at, t, sp) in &bir.wf_annots {
            let lts = self.named_lts.clone();
            let ann = self.rty.annotation(bir.file, bir.module, *at, *t, &lts);
            let ann: Vec<Option<Rv>> = ann.iter().map(|a| self.ann_rv(a)).collect();
            // a pseudo point carrying the annotation's span (blame only)
            let p = self.n_points + self.pseudo_spans.len();
            self.pseudo_spans.push(*sp);
            self.wf_constraints(*t, &ann, p, 0);
        }
        let order = super::moves::rpo(bir);
        self.reachable = vec![false; bir.blocks.len()];
        for b in order {
            crate::preempt::point();
            self.reachable[b as usize] = true;
        }
        self.const_use_constraints();
        for (bi, b) in bir.blocks.iter().enumerate() {
            crate::preempt::point();
            for (si, s) in b.stmts.iter().enumerate() {
                crate::preempt::point();
                let p = self.pt(bi, si);
                if let StmtKind::Assign(dest, rv) = &s.kind {
                    if dest.local == 0 && bir.closure_kind.is_none() && !self.is_promoted {
                        self.ret_points.insert(p);
                    }
                    self.assign_constraints(dest, rv, p, s.span, bi, si);
                }
            }
            if let Some(t) = &b.term {
                let p = self.pt(bi, b.stmts.len());
                if let TermKind::Call { callee, args, dest, .. } = &t.kind {
                    if dest.local == 0 && dest.proj.is_empty() && bir.closure_kind.is_none() && !self.is_promoted {
                        self.ret_points.insert(p);
                    }
                    self.call_constraints(callee, args, dest, p, bi as Bb);
                }
            }
        }
    }

    /// The regions an ADT path's explicit generic arguments give each slot of `t` (None:
    /// not annotated / elided). Lifetime arguments are the ADT's lifetime params in order
    /// (all or none), type arguments the type params.
    fn path_annotation(&mut self, t: TyId, pargs: &[PathArg]) -> Option<Vec<Option<Rv>>> {
        let (d, args) = match self.tx.kind(t) {
            TyKind::Adt(d, a, _) => (d, a),
            _ => return None,
        };
        if self.tcx.tys.list(args).iter().any(|a| matches!(self.tx.kind(*a), TyKind::Const(..))) {
            return None;
        }
        let lts: Vec<crate::ast::Ident> = pargs.iter().filter_map(|a| match a { PathArg::Lifetime(l) => Some(*l), _ => None }).collect();
        let tys: Vec<crate::ast::TyId> = pargs.iter().filter_map(|a| match a { PathArg::Type(x) => Some(*x), _ => None }).collect();
        let info = self.rty.adt_info(d);
        let n_lt = info.var_lt.len();
        if (!lts.is_empty() && lts.len() != n_lt) || (!tys.is_empty() && tys.len() != self.tcx.tys.list(args).len()) {
            return None;
        }
        let n = self.rty.n_slots(t);
        let mut out: Vec<Option<Rv>> = vec![None; n];
        let file = self.bir.file;
        for (i, l) in lts.iter().enumerate() {
            let name = self.prog.files[file as usize].text(*l).to_string();
            let a = if name == "'static" {
                Ann::Static
            } else if name == "'_" {
                Ann::Fresh
            } else {
                match self.named_lts.iter().find(|x| x.0.as_slice() == name.as_bytes()) {
                    Some((_, r)) => Ann::Rv(*r),
                    None => Ann::Named(name),
                }
            };
            if i < n {
                out[i] = self.ann_rv(&a);
            }
        }
        if !tys.is_empty() {
            let mut off = n_lt;
            for (k, a) in self.tcx.tys.list(args).iter().enumerate() {
                let m = self.rty.n_slots(*a);
                let ann = self.rty.annotation(file, self.bir.module, tys[k], *a, &self.named_lts);
                if ann.len() == m && off + m <= n {
                    for (i, x) in ann.iter().enumerate() {
                        out[off + i] = self.ann_rv(x);
                    }
                }
                off += m;
            }
        }
        Some(out)
    }

    /// Slots for an ADT aggregate whose path has explicit generic arguments, equal to them
    /// (rustc relates the aggregate's type to the user type invariantly).
    fn agg_annotation(&mut self, t: TyId, bb: usize, si: usize) -> Option<Vec<Rv>> {
        let ann = self.bir.rvalue_user_tys.iter().find(|x| x.0 == (bb as Bb, si as u32))?.1;
        let lts = self.named_lts.clone();
        let us = self.user_ty_regions(ann, &lts)?;
        let ag = self.fresh_slots(t, Kind::Var);
        for (r, u) in ag.iter().zip(us.iter()) {
            if let Some(u) = u {
                self.outlives(*r, *u, 0);
                self.outlives(*u, *r, 0);
            }
        }
        Some(ag)
    }

    /// Well-formedness of a type at known regions (`ann`, one per slot of `t`): a reference's
    /// pointee outlives its region, an ADT's own lifetime predicates hold (rustc WF).
    fn wf_constraints(&mut self, t: TyId, ann: &[Option<Rv>], p: usize, depth: u32) {
        if depth > 20 || ann.len() != self.rty.n_slots(t) {
            return;
        }
        match self.tx.kind(t) {
            TyKind::Ref(_, inner, _) => {
                if let Some(outer) = ann[0] {
                    for a in ann[1..].iter().flatten() {
                        self.outlives(*a, outer, p);
                    }
                }
                self.wf_constraints(inner, &ann[1..], p, depth + 1);
            }
            TyKind::Array(e, _) | TyKind::Slice(e) => self.wf_constraints(e, ann, p, depth + 1),
            TyKind::Tuple(v) => {
                let mut off = 0;
                for x in self.tcx.tys.list(v).iter().copied() {
                    let n = self.rty.n_slots(x);
                    if off + n > ann.len() {
                        return;
                    }
                    self.wf_constraints(x, &ann[off..off + n], p, depth + 1);
                    off += n;
                }
            }
            TyKind::Adt(d, args, _) => {
                for (a, b) in self.rty.adt_lifetime_bounds(d) {
                    if let Ok(ai) = a {
                        if let (Some(Some(x)), Some(Some(y))) = (ann.get(ai), ann.get(b)) {
                            self.outlives(*x, *y, p);
                        }
                    }
                }
                let mut off = self.rty.adt_info(d).var_lt.len();
                for x in self.tcx.tys.list(args).iter().copied() {
                    let n = self.rty.n_slots(x);
                    if off + n > ann.len() {
                        return;
                    }
                    self.wf_constraints(x, &ann[off..off + n], p, depth + 1);
                    off += n;
                }
            }
            _ => {}
        }
    }

    /// A copy of a value whose `Copy` impl names 'static regions (`impl Copy for Foo<'static>`):
    /// those regions of the copied place outlive 'static.
    fn copy_constraints(&mut self, o: &Operand, p: usize) {
        if let Operand::Copy(pl) = o {
            let (t, rs) = self.place_regions(pl);
            if !matches!(self.tx.kind(t), TyKind::Adt(..)) {
                return;
            }
            let ss = self.rty.copy_static_slots(t);
            for i in ss.iter() {
                if let Some(&r) = rs.get(*i) {
                    self.outlives(r, STATIC, p);
                }
            }
        }
    }

    fn assign_constraints(&mut self, dest: &Place, rv: &Rvalue, p: usize, span: Span, bb: usize, si: usize) {
        // a fn item path with a written user type: the value's regions equal it
        if let Rvalue::Use(Operand::Const(_)) = rv {
            if let Some((_, u)) = self.bir.fn_anns.iter().find(|x| x.0 == (bb as Bb, si as u32)).cloned() {
                let (dt0, dr0) = self.place_regions(dest);
                if let TyKind::FnDef(fd, fargs, _) = self.tx.kind(dt0) {
                    let t = self.rty.sig_template(fd, self.tcx.tys.list(fargs));
                    let live = self.reachable.get(bb).copied().unwrap_or(true);
                    let ann = self.user_ty_slots(&t, &u, live);
                    if ann.len() == dr0.len() {
                        let dv: Vec<Option<Rv>> = dr0.into_iter().map(Some).collect();
                        self.equate_slots(&dv, &ann, p);
                    }
                }
            }
        }
        match rv {
            Rvalue::Use(o) | Rvalue::Repeat(o) | Rvalue::Cast(_, o, _) => self.copy_constraints(o, p),
            Rvalue::Aggregate(_, v) | Rvalue::Op(_, v) => {
                for o in v {
                    self.copy_constraints(o, p);
                }
            }
            _ => {}
        }
        let (dt, dr) = self.place_regions(dest);
        let dvar = self.rty.skel(dt);
        match rv {
            Rvalue::Use(Operand::Const(c)) if matches!(c.kind, ConstantKind::Promoted(_)) => {
                let ConstantKind::Promoted(index) = c.kind else { unreachable!() };
                let sr = self.check_promoted(index, p);
                self.relate(&sr, &dr, &dvar, p);
            }
            Rvalue::Use(o) | Rvalue::Repeat(o) => {
                if let Some((st, sr)) = self.op_regions(o) {
                    // an implicit `&T` -> `*const T` coercion: the pointee's regions carry over
                    let sr = match (self.tx.kind(st), self.tx.kind(dt)) {
                        (TyKind::Ref(_, a, _), TyKind::Ptr(_, b)) if a == b && sr.len() == dr.len() + 1 => sr[1..].to_vec(),
                        _ => sr,
                    };
                    self.relate(&sr, &dr, &dvar, p);
                }
            }
            Rvalue::Ref(r, bk, bp) => {
                // into a reference (a raw pointer destination keeps no region: `let p: *mut T =
                // &mut x`)
                let dest_ref = matches!(self.tx.kind(dt), TyKind::Ref(..));
                if let (Some(&rd), true) = (dr.first(), dest_ref) {
                    self.outlives(*r, rd, p);
                }
                let (_bt, br) = self.place_regions(bp);
                if dest_ref && dr.len() == br.len() + 1 {
                    let var: Vec<V> = dvar[1..].to_vec();
                    self.relate(&br, &dr[1..], &var, p);
                }
                self.reborrow_constraints(*r, bp, p);
                if !self.is_promoted { self.new_loan(*r, *bk, bp.clone(), dest.local, p, span); }
            }
            Rvalue::RawPtr(_, bp) => {
                // the rvalue is `*const PlaceTy` / `*mut PlaceTy`, flowing into the destination
                let (_bt, br) = self.place_regions(bp);
                self.relate(&br, &dr, &dvar, p);
            }
            Rvalue::Aggregate(k, ops) => match k {
                AggKind::Adt(t, vi) => {
                    // the constructed type is well-formed: the ADT's lifetime predicates
                    if let TyKind::Adt(ad, _, _) = self.tx.kind(*t) {
                        if dr.len() == self.rty.n_slots(*t) {
                            for (a, b) in self.rty.adt_lifetime_bounds(ad) {
                                if let Ok(ai) = a {
                                    if let (Some(x), Some(y)) = (dr.get(ai), dr.get(b)) {
                                        self.outlives(*x, *y, p);
                                    }
                                }
                            }
                        }
                    }
                    // explicit type arguments on the path: the value's type is the annotation
                    // (fresh slots equal to it, flowing into the destination)
                    let dr = match self.agg_annotation(*t, bb, si) {
                        Some(ag) => {
                            self.relate(&ag, &dr, &dvar, p);
                            ag
                        }
                        None => dr,
                    };
                    // built by calling a constructor value: the value's slots (its fn item
                    // type's args, the ADT's slots) are the aggregate's
                    if let Some(cp) = self.bir.agg_callee.iter().find(|x| x.0 == (bb as Bb, si as u32)).map(|x| x.1.clone()) {
                        let (_ct, cr) = self.place_regions(&cp);
                        if cr.len() == dr.len() {
                            for (a, b) in cr.iter().zip(dr.iter()) {
                                self.outlives(*a, *b, p);
                                self.outlives(*b, *a, p);
                            }
                        }
                    }
                    for (fi, o) in ops.iter().enumerate() {
                        if let Some((ot, or)) = self.op_regions(o) {
                            let fm = self.rty.field_slots(*t, *vi, fi as u32);
                            let mut target: Vec<Rv> = Vec::with_capacity(fm.len());
                            for sr in fm.iter() {
                                target.push(match sr {
                                    SR::Sym(i) => dr.get(*i as usize).copied().unwrap_or(STATIC),
                                    SR::Static => STATIC,
                                    SR::Fresh => self.new_rv(Kind::Var, "'_"),
                                });
                            }
                            let var = self.rty.skel(ot);
                            self.relate(&or, &target, &var, p);
                        }
                    }
                }
                AggKind::Tuple => {
                    for (fi, o) in ops.iter().enumerate() {
                        if let Some((ot, or)) = self.op_regions(o) {
                            let fm = self.rty.field_slots(dt, 0, fi as u32);
                            let mut target: Vec<Rv> = Vec::with_capacity(fm.len());
                            for sr in fm.iter() {
                                target.push(match sr {
                                    SR::Sym(i) => dr.get(*i as usize).copied().unwrap_or(STATIC),
                                    SR::Static => STATIC,
                                    SR::Fresh => self.new_rv(Kind::Var, "'_"),
                                });
                            }
                            let var = self.rty.skel(ot);
                            self.relate(&or, &target, &var, p);
                        }
                    }
                }
                AggKind::Array(_) => {
                    for o in ops {
                        if let Some((_ot, or)) = self.op_regions(o) {
                            self.relate(&or, &dr, &dvar, p);
                        }
                    }
                }
                AggKind::Closure(ci) => {
                    // captures in order: each capture's slots, then the return slots
                    let mut off = 0;
                    for o in ops {
                        if let Some((ot, or)) = self.op_regions(o) {
                            let n = or.len();
                            if off + n <= dr.len() {
                                let target = dr[off..off + n].to_vec();
                                let var = vec![V::Inv; n];
                                let _ = ot;
                                self.relate(&or, &target, &var, p);
                            }
                            off += n;
                        }
                    }
                    // the closure body's requirements between its type's slots
                    let ce = self.bir.closures.get(*ci as usize).copied();
                    if let Some(reqs) = ce.and_then(|ce| self.closure_reqs.get(&ce)).cloned() {
                        for req in reqs {
                            let (x,y) = (req.subject,req.outlived_free_region);
                            let b = if y == u32::MAX {
                                Some(STATIC)
                            } else if y >= NAMED_REQ {
                                // an owner's free region the closure's external region must outlive
                                let nm = ce.and_then(|ce| self.rty.closure_req_names.borrow().get(&ce).and_then(|v| v.get((y - NAMED_REQ) as usize).cloned()));
                                nm.and_then(|n| self.named_lifetime_rv(&n))
                            } else {
                                dr.get(y as usize).copied()
                            };
                            let ClosureOutlivesSubject::Region(x) = x else {
                                if let (ClosureOutlivesSubject::Ty(t),Some(b)) = (x,b) { self.type_test(t,b,req.blame_span); }
                                continue;
                            };
                            let a = if x == u32::MAX { Some(STATIC) } else if x >= NAMED_REQ {
                                let name = ce.and_then(|ce| self.rty.closure_req_names.borrow().get(&ce).and_then(|v| v.get((x - NAMED_REQ) as usize).cloned()));
                                name.and_then(|n| self.named_lifetime_rv(&n))
                            } else { dr.get(x as usize).copied() };
                            if let (Some(a), Some(b)) = (a, b) {
                                if y == u32::MAX || y >= NAMED_REQ {
                                    self.closure_static.push(a);
                                }
                                let point = self.n_points + self.pseudo_spans.len();
                                self.pseudo_spans.push(req.blame_span);
                                self.constraint_categories.insert(point,req.category);
                                self.outlives(a, b, point);
                            }
                        }
                    }
                }
            },
            Rvalue::Cast(k, o, _) => {
                if let Some((st, sr)) = self.op_regions(o) {
                    match k {
                        CastKind::Same | CastKind::Unsize => self.relate(&sr, &dr, &dvar, p),
                        CastKind::ToDyn => {
                            // pointer region carries over; the source pointee's regions outlive
                            // the object bound
                            let src_ref = matches!(self.tx.kind(st), TyKind::Ref(..));
                            let dst_ref = matches!(self.tx.kind(dt), TyKind::Ref(..));
                            let (s_inner, d_obj) = if src_ref && dst_ref {
                                if let (Some(a), Some(b)) = (sr.first(), dr.first()) {
                                    self.outlives(*a, *b, p);
                                }
                                (sr.get(1..).map(|x| x.to_vec()).unwrap_or_default(), dr.get(1).copied())
                            } else {
                                (sr.clone(), dr.first().copied())
                            };
                            if let Some(obj) = d_obj {
                                for r in s_inner {
                                    self.outlives(r, obj, p);
                                }
                                // the source's type params must outlive the object bound
                                let pointee = if src_ref || matches!(self.tx.kind(st), TyKind::Ptr(..)) { self.tx.builtin_deref(st).unwrap_or(st) } else { self.tx.box_inner(st).unwrap_or(st) };
                                self.type_test(pointee, obj, span);
                            }
                        }
                        CastKind::PtrToPtr => {
                            // a cast that is a coercion (rustc CoercionCast / ArrayToPointer):
                            // `&T`/`*mut T` -> `*const T`, `&mut T` -> `*mut T`, `&[T; N]` ->
                            // `*const T`: the pointee flows into the destination's pointee;
                            // other pointer casts carry nothing
                            let (sm, sp, soff) = match self.tx.kind(st) {
                                TyKind::Ref(m, x, _) => (m, x, 1usize),
                                TyKind::Ptr(m, x) => (m, x, 0usize),
                                _ => return,
                            };
                            if let TyKind::Ptr(dm, dp) = self.tx.kind(dt) {
                                let coercible = !dm || sm;
                                let same = sp == dp;
                                let arr = soff == 1 && matches!(self.tx.kind(sp), TyKind::Array(e, _) if e == dp);
                                if coercible && (same || arr) && sr.len() >= soff {
                                    let src = sr[soff..].to_vec();
                                    if src.len() == dr.len() {
                                        self.relate(&src, &dr, &dvar, p);
                                    }
                                }
                            }
                        }
                        CastKind::Plain | CastKind::PointerExposeProvenance | CastKind::PointerWithExposedProvenance => {}
                    }
                }
            }
            Rvalue::Op(_, ops) => {
                // a comparison of raw pointers: rustc relates the operand types both ways
                // (they must be equal); fn pointers are left out (their elided lifetimes are
                // higher-ranked, which erased types do not show)
                if ops.len() == 2 {
                    if let (Some((ta, ra)), Some((tb, rb))) = (self.op_regions(&ops[0]), self.op_regions(&ops[1])) {
                        if ta == tb && matches!(self.tx.kind(ta), TyKind::Ptr(..)) && !self.has_fn_ptr(ta, 0) && ra.len() == rb.len() && !ra.is_empty() {
                            let var = self.rty.skel(ta);
                            self.relate(&ra, &rb, &var, p);
                            self.relate(&rb, &ra, &var, p);
                        }
                    }
                }
            }
            Rvalue::Discriminant(_) | Rvalue::Len(_) => {}
        }
    }

    /// rustc add_reborrow_constraint: a reborrow through `*r` (r: &'x T) needs 'x: 'new.
    fn reborrow_constraints(&mut self, new: Rv, bp: &Place, p: usize) {
        for i in (0..bp.proj.len()).rev() {
            if bp.proj[i] != Proj::Deref {
                continue;
            }
            let base = Place { local: bp.local, proj: bp.proj[..i].to_vec() };
            let (bt, br) = self.place_regions(&base);
            match self.tx.kind(bt) {
                TyKind::Ref(m, _, _) => {
                    if let Some(&rr) = br.first() {
                        self.outlives(rr, new, p);
                    }
                    if !m {
                        break;
                    }
                }
                TyKind::Ptr(..) => break,
                _ => {}
            }
        }
    }

    /// The regions a path's written user type gives each slot of the callee's value layout
    /// (SigTmpl::layout; None: not written). rustc: the canonical user type of
    /// UserType::TypeOf(def, UserArgs) instantiated for relate_type_and_user_type.
    fn user_ty_slots(&mut self, t: &SigTmpl, u: &PathUserTy, with_self: bool) -> Vec<Option<Rv>> {
        let n_self = t.self_slots.len();
        let n_tl = t.trait_lts.len();
        let mut out: Vec<Option<Rv>> = vec![None; t.layout().len()];
        // the trait's written lifetime arguments (all or none)
        if !u.trait_lts.is_empty() && u.trait_lts.len() == n_tl {
            for (i, l) in u.trait_lts.iter().enumerate() {
                let name = self.prog.files[self.bir.file as usize].text(*l).to_string();
                let a = if name == "'static" {
                    Ann::Static
                } else if name == "'_" {
                    Ann::Fresh
                } else {
                    match self.named_lts.iter().find(|x| x.0.as_slice() == name.as_bytes()) {
                        Some((_, r)) => Ann::Rv(*r),
                        None => Ann::Named(name),
                    }
                };
                out[i] = self.ann_rv(&a);
            }
        }
        if let (true, Some(sa), Some(st)) = (with_self, &u.self_ann, t.self_ty) {
            if n_self == self.rty.n_slots(st) && n_self > 0 {
                let ann: Vec<Option<Rv>> = match sa {
                    SelfAnn::Ty(at) => {
                        let lts = self.named_lts.clone();
                        let a = self.rty.annotation(self.bir.file, self.bir.module, *at, st, &lts);
                        a.iter().map(|x| self.ann_rv(x)).collect()
                    }
                    SelfAnn::Args(pa) => self.path_annotation(st, pa).unwrap_or_default(),
                };
                for (i, a) in ann.into_iter().enumerate().take(n_self) {
                    out[n_tl + i] = a;
                }
            }
        }
        // (the user's type arguments and the template's, in layout order: trait ones, own ones)
        let mut off = n_tl + n_self;
        for (written, tmpl) in [(&u.trait_targs, &t.trait_targs), (&u.targs, &t.own_targs)] {
            if !written.is_empty() && written.len() == tmpl.len() {
                for (at, ot) in written.iter().zip(tmpl.iter()) {
                    if let Some((slots, ty)) = ot {
                        let ann = self.rty.annotation(self.bir.file, self.bir.module, *at, *ty, &self.named_lts);
                        if ann.len() == slots.len() {
                            for (i, a) in ann.iter().enumerate() {
                                out[off + i] = self.ann_rv(a);
                            }
                        }
                        off += slots.len();
                    }
                }
            } else {
                off += tmpl.iter().flatten().map(|x| x.0.len()).sum::<usize>();
            }
        }
        out
    }

    /// Two views of the same regions are equal (rustc relates a FnDef's regions to its user type
    /// invariantly).
    fn equate_slots(&mut self, a: &[Option<Rv>], b: &[Option<Rv>], p: usize) {
        for (x, y) in a.iter().zip(b.iter()) {
            if let (Some(x), Some(y)) = (x, y) {
                self.outlives(*x, *y, p);
                self.outlives(*y, *x, p);
            }
        }
    }

    fn call_constraints(&mut self, callee: &Callee, args: &[Operand], dest: &Place, p: usize, bb: Bb) {
        for o in args {
            self.copy_constraints(o, p);
        }
        let (_dt, dr) = self.place_regions(dest);
        match callee {
            Callee::Fn(d, a) | Callee::Overloaded(d, a) | Callee::Virtual(d, a) => {
                let t = self.rty.sig_template(*d, a);
                if !t.ok {
                    return;
                }
                let mut map: Vec<Rv> = (0..t.n_syms).map(|_| self.new_rv(Kind::Var, "'_")).collect();
                // unknown slots: fresh unconstrained variables (never 'static)
                let fresh0 = map.len();
                let n_fresh = t.inputs.iter().flatten().chain(t.output.iter()).chain(t.cons.iter().flat_map(|(a, b)| [a, b])).filter(|x| **x == SR::Fresh).count();
                for _ in 0..n_fresh {
                    let r = self.new_rv(Kind::Var, "'_");
                    map.push(r);
                }
                let fi = std::cell::Cell::new(fresh0);
                let m = |sr: &SR| -> Rv {
                    match sr {
                        SR::Static => STATIC,
                        SR::Sym(s) => map[*s as usize],
                        SR::Fresh => {
                            let i = fi.get();
                            fi.set(i + 1);
                            map.get(i).copied().unwrap_or(STATIC)
                        }
                    }
                };
                for (i, o) in args.iter().enumerate() {
                    if let (Some((ot, or)), Some(formal)) = (self.op_regions(o), t.inputs.get(i)) {
                        let fr: Vec<Rv> = formal.iter().map(|x| m(x)).collect();
                        let var = self.rty.skel(ot);
                        self.relate(&or, &fr, &var, p);
                    }
                }
                let out: Vec<Rv> = t.output.iter().map(|x| m(x)).collect();
                let dt = place_ty(self.tx, self.bir, dest);
                let dvar = self.rty.skel(dt);
                self.relate(&out, &dr, &dvar, p);
                for (a, b) in &t.cons {
                    let (x, y) = (m(a), m(b));
                    self.outlives(x, y, p);
                }
                // the callee's `T: 'r` bounds on what this body passes for T (its own params)
                let tb: Vec<(u32, Rv)> = t.call_bounds.iter().map(|(pi, sr)| (*pi, m(sr))).collect();
                let mut regs: Vec<(Rv, Rv)> = Vec::new();
                for (pi, r) in tb {
                    if let Some(at) = a.get(pi as usize) {
                        let sp = self.span_at(p);
                        self.type_test(*at, r, sp);
                    }
                    // the regions in what is passed for T outlive the bound (outlives
                    // components): `foo::<&'a u32>()` with `T: 'static`
                    let concrete = a.get(pi as usize).map_or(false, |at| !self.has_unknown(*at, 0));
                    if let (true, Some(k)) = (concrete, t.own_targ_index.iter().position(|x| *x == Some(pi))) {
                        if let Some(Some((slots, _))) = t.own_targs.get(k) {
                            for sl in slots {
                                if let SR::Sym(_) = sl {
                                    regs.push((m(sl), r));
                                }
                            }
                        }
                    }
                }
                for (x, r) in regs {
                    self.outlives(x, r, p);
                }
                // the callee's impl self type is well-formed: its ADT lifetime predicates hold for
                // the instance (`Foo::<'a, 'b>::f` with `struct Foo<'a, 'b: 'a>`)
                if let Some(st) = t.self_ty {
                    if let TyKind::Adt(ad, _, _) = self.tx.kind(st) {
                        if t.self_slots.len() == self.rty.n_slots(st) {
                            let pairs: Vec<(Rv, Rv)> = self.rty.adt_lifetime_bounds(ad).into_iter().filter_map(|(a, b)| match a {
                                Ok(ai) => match (t.self_slots.get(ai), t.self_slots.get(b)) {
                                    (Some(x @ SR::Sym(_)), Some(y @ SR::Sym(_))) => Some((m(x), m(y))),
                                    _ => None,
                                },
                                Err(_) => None,
                            }).collect();
                            for (x, y) in pairs {
                                self.outlives(x, y, p);
                            }
                        }
                    }
                }
                // the callee's value regions (rustc: the FnDef's generic arguments' regions,
                // SigTmpl::layout), related to the user type written on its path (the self type
                // `A::<'a>::new` / `<A<'a>>::new` / `<T as Tr>::f`, explicit type arguments) and
                // to the fn item value the call goes through
                // (the self type not in dead code, where rustc's typeck leaves such a call's
                // types open)
                let live_call = self.reachable.get(bb as usize).copied().unwrap_or(true);
                let lay: Vec<Option<Rv>> = t.layout().iter().map(|sr| if *sr == SR::Fresh { None } else { Some(m(sr)) }).collect();
                if let Some((_, u)) = self.bir.call_user_tys.iter().find(|x| x.0 == bb).cloned() {
                    let ann = self.user_ty_slots(&t, &u, live_call);
                    self.equate_slots(&lay, &ann, p);
                }
                if let Some((_, vp)) = self.bir.fn_values.iter().find(|x| x.0 == bb).cloned() {
                    let (_vt, vr) = self.place_regions(&vp);
                    if vr.len() == lay.len() {
                        let vr: Vec<Option<Rv>> = vr.into_iter().map(Some).collect();
                        self.equate_slots(&lay, &vr, p);
                    }
                }
            }
            Callee::Indirect => {
                // through a static: its declared fn-pointer / `dyn Fn*` parameters' 'static
                // regions (elided ones are higher-ranked and say nothing)
                if let Some((_, d)) = self.bir.static_callees.iter().find(|x| x.0 == bb).copied() {
                    self.static_callee_constraints(d, args, p);
                }
                // a fn-pointer field (`(self.func)(arg)` with `func: fn(&'a mut T)`): the struct's
                // lifetime params named in the parameters
                if let Some(Operand::Copy(pl) | Operand::Move(pl)) = args.first() {
                    // (the callee operand is usually a temp copied from the field just before)
                    let mut src = pl.clone();
                    if src.proj.is_empty() {
                        for st in self.bir.blocks[bb as usize].stmts.iter().rev() {
                            crate::preempt::point();
                            if let StmtKind::Assign(d, Rvalue::Use(Operand::Copy(x) | Operand::Move(x))) = &st.kind {
                                if *d == src {
                                    src = x.clone();
                                    break;
                                }
                            }
                        }
                    }
                    self.field_fnptr_constraints(&src, args, p);
                }
                // a closure called through its env: the result's regions are the closure
                // type's return slots
                if let Some(o) = args.first() {
                    if let Some((rt, rr)) = self.op_regions(o) {
                        let ct = match self.tx.kind(rt) {
                            TyKind::Ref(_, c, _) => c,
                            _ => rt,
                        };
                        if let TyKind::Closure(cf, ce, ..) = self.tx.kind(ct) {
                            let off = if matches!(self.tx.kind(rt), TyKind::Ref(..)) { 1 } else { 0 };
                            // the arguments flow into the closure type's parameter regions
                            let cs = self.rty.closures.borrow().get(&(cf, ce)).cloned();
                            if let Some(cs) = cs {
                                let mut o = off + cs.caps.iter().map(|c| self.rty.n_slots(*c)).sum::<usize>();
                                for (k, _, slots) in &cs.params {
                                    let n = slots.len();
                                    if let Some((at, ar)) = args.get(k + 1).and_then(|a| self.op_regions(a)) {
                                        if o + n <= rr.len() && slots.iter().all(|&i| i < ar.len()) {
                                            let target = rr[o..o+n].to_vec();
                                            let source: Vec<_> = slots.iter().map(|&i|ar[i]).collect();
                                            let variance = self.rty.skel(at);
                                            let var: Vec<_> = slots.iter().map(|&i|variance[i]).collect();
                                            self.relate(&source, &target, &var, p);
                                        }
                                    }
                                    o += n;
                                }
                            }
                            let nret = dr.len();
                            if rr.len() >= off + nret {
                                let ret = rr[rr.len() - nret..].to_vec();
                                let dt = place_ty(self.tx, self.bir, dest);
                                let dvar = self.rty.skel(dt);
                                self.relate(&ret, &dr, &dvar, p);
                            }
                        }
                    }
                }
            }
        }
    }

    fn new_loan(&mut self, r: Rv, bk: BorrowKind, bp: Place, holder: Local, p: usize, span: Span) {
        // rustc ignore_borrow: borrows through a shared reference or raw pointer are not tracked
        let mut t = self.bir.locals[bp.local as usize].ty;
        for pr in &bp.proj {
            if *pr == Proj::Deref {
                match self.tx.kind(t) {
                    TyKind::Ref(false, _, _) | TyKind::Ptr(..) => return,
                    _ => {}
                }
            }
            t = super::moves::proj_ty(self.tx, t, pr);
        }
        self.loans.push(Loan { region: r, kind: bk, place: bp, point: p, span, activation: None, holder });
        let f = self.feeds.get(&holder).copied();
        self.loan_feed.push(f);
        self.loan_parts.push(None);
    }

    /// Temps used exactly once, moved into a call argument or a closure's captures.
    fn find_feeds(&mut self) {
        let bir = self.bir;
        let mut uses: HashMap<Local, u32> = HashMap::default();
        let mut cand: HashMap<Local, Feed> = HashMap::default();
        for (bi, blk) in bir.blocks.iter().enumerate() {
            crate::preempt::point();
            for s in &blk.stmts {
                crate::preempt::point();
                if let StmtKind::Assign(dp, rv) = &s.kind {
                    for l in rvalue_locals(rv) {
                        *uses.entry(l).or_insert(0) += 1;
                    }
                    if !dp.proj.is_empty() {
                        *uses.entry(dp.local).or_insert(0) += 1;
                    }
                    if let Rvalue::Aggregate(AggKind::Closure(ci), ops) = rv {
                        if let Some(&ce) = bir.closures.get(*ci as usize) {
                            for (k, o) in ops.iter().enumerate() {
                                if let Operand::Move(p) = o {
                                    if p.proj.is_empty() {
                                        cand.insert(p.local, Feed::Closure(ce, k));
                                    }
                                }
                            }
                        }
                    }
                }
            }
            if let Some(t) = &blk.term {
                match &t.kind {
                    TermKind::Call { args, .. } => {
                        for (ai, o) in args.iter().enumerate() {
                            if let Some(p) = o.place() {
                                *uses.entry(p.local).or_insert(0) += 1;
                                if let Operand::Move(p) = o {
                                    if p.proj.is_empty() {
                                        cand.insert(p.local, Feed::Call(bi, ai));
                                    }
                                }
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
        for (l, f) in cand {
            if uses.get(&l).copied() == Some(1) && self.bir.locals[l as usize].kind == LocalKind::Temp {
                self.feeds.insert(l, f);
            }
        }
    }

    /// Field views: the parts of a borrow fed to a call / closure whose summary is known (None:
    /// the whole place).
    fn parts(&mut self, li: usize) -> Option<Vec<(Place, BorrowKind)>> {
        if let Some(p) = &self.loan_parts[li] {
            return p.clone();
        }
        let r = self.parts_uncached(li);
        self.loan_parts[li] = Some(r.clone());
        r
    }

    fn parts_uncached(&self, li: usize) -> Option<Vec<(Place, BorrowKind)>> {
        let feed = self.loan_feed[li]?;
        let l = &self.loans[li];
        // a borrow of a value reached through derefs and fields (`*self`, `*cx`, `cx.vm`,
        // a local) is narrowed; element / variant places stay whole
        if l.place.proj.iter().any(|p| matches!(p, Proj::Index(_) | Proj::ConstIndex(..) | Proj::Subslice(..) | Proj::Downcast(_))) {
            return None;
        }
        let summ = match feed {
            Feed::Call(bi, ai) => match &self.bir.blocks[bi].term.as_ref()?.kind {
                TermKind::Call { callee: Callee::Fn(d, a) | Callee::Overloaded(d, a), .. } => super::fv::summaries(self.prog, self.tcx, self.tx, *d, a, 0).get(ai).cloned()?,
                _ => return None,
            },
            Feed::Closure(ce, k) => self.closure_fv.get(&ce)?.get(k).cloned()?,
        };
        match summ {
            super::fv::Summary::Whole => None,
            super::fv::Summary::Parts(v) => Some(
                v.into_iter()
                    .map(|(sub, m)| {
                        let mut q = l.place.clone();
                        q.proj.extend(sub);
                        (q, if m && l.kind.is_mut() { l.kind } else { BorrowKind::Shared })
                    })
                    .collect(),
            ),
        }
    }

    // ------------------------------------------------------------------ liveness

    /// (use-live, drop-live) point sets per local (only locals with regions).
    fn liveness(&mut self, md: &super::moves::MoveData, rel: &[bool]) -> (Vec<Option<Bits>>, Vec<Option<Bits>>) {
        let bir = self.bir;
        let nl0 = bir.locals.len();
        if self.loans.is_empty() && !self.is_promoted {
            return (vec![None; nl0], vec![None; nl0]);
        }
        // drops of places that may be initialized only (rustc liveness trace: drop-live where
        // the local is maybe-initialized)
        let order = super::moves::rpo(bir);
        let minit = super::moves::maybe_init_flow(md, bir, &order);
        let nl = bir.locals.len();
        let np = self.n_points;
        // defs / uses / drops per point
        let mut uses: Vec<Vec<usize>> = vec![Vec::new(); nl];
        let mut drops: Vec<Vec<usize>> = vec![Vec::new(); nl];
        let mut defs: Vec<Bits> = Vec::new();
        // only locals with a region in a scc some loan's region reaches
        let relevant: Vec<bool> = (0..nl).map(|l| self.is_promoted || self.lr[l].iter().any(|r| rel[*r as usize])).collect();
        for l in 0..nl {
            defs.push(if relevant[l] { bits_new(np) } else { Vec::new() });
        }
        let use_at = |l: Local, p: usize, uses: &mut Vec<Vec<usize>>| {
            if relevant[l as usize] {
                uses[l as usize].push(p);
            }
        };
        for (bi, b) in bir.blocks.iter().enumerate() {
            crate::preempt::point();
            for (si, s) in b.stmts.iter().enumerate() {
                crate::preempt::point();
                let p = self.start[bi] + si;
                match &s.kind {
                    StmtKind::Assign(dest, rv) => {
                        for l in rvalue_locals(rv) {
                            use_at(l, p, &mut uses);
                        }
                        if dest.proj.is_empty() {
                            if relevant[dest.local as usize] {
                                bit_set(&mut defs[dest.local as usize], p);
                            }
                        } else {
                            for l in place_locals(dest) {
                                use_at(l, p, &mut uses);
                            }
                        }
                    }
                    StmtKind::StorageLive(l) | StmtKind::StorageDead(l) => {
                        if relevant[*l as usize] {
                            bit_set(&mut defs[*l as usize], p);
                        }
                    }
                    StmtKind::FakeRead(_, pl) | StmtKind::PlaceMention(pl) => {
                        for l in place_locals(pl) {
                            use_at(l, p, &mut uses);
                        }
                    }
                    StmtKind::Nop => {}
                }
            }
            if let Some(t) = &b.term {
                let p = self.start[bi] + b.stmts.len();
                match &t.kind {
                    TermKind::Switch(o, _) => {
                        if let Some(pl) = o.place() {
                            for l in place_locals(pl) {
                                use_at(l, p, &mut uses);
                            }
                        }
                    }
                    TermKind::Call { args, dest, .. } => {
                        for o in args {
                            if let Some(pl) = o.place() {
                                for l in place_locals(pl) {
                                    use_at(l, p, &mut uses);
                                }
                            }
                        }
                        // the destination is defined on the return edge: a def at the target's
                        // first point would be more precise; the call point is rustc's choice
                        if !dest.proj.is_empty() {
                            for l in place_locals(dest) {
                                use_at(l, p, &mut uses);
                            }
                        }
                    }
                    TermKind::Drop(pl, _) => {
                        if relevant[pl.local as usize] {
                            let live_here = match &minit[bi] {
                                Some(e) => {
                                    let mut st = e.clone();
                                    for s2 in &b.stmts {
                                        crate::preempt::point();
                                        super::moves::init_stmt_effect(md, &s2.kind, &mut st);
                                    }
                                    super::moves::maybe_init(md, pl, &st)
                                }
                                None => false,
                            };
                            if live_here {
                                drops[pl.local as usize].push(p);
                            }
                        }
                    }
                    TermKind::Return => use_at(0, p, &mut uses),
                    _ => {}
                }
            }
        }
        // call destinations: def at the call (after its uses)
        for (bi, b) in bir.blocks.iter().enumerate() {
            crate::preempt::point();
            if let Some(Term { kind: TermKind::Call { dest, .. }, .. }) = &b.term {
                if dest.proj.is_empty() && relevant[dest.local as usize] {
                    let p = self.start[bi] + b.stmts.len();
                    // a use of the same local in the args keeps it live up to here; the def
                    // ends liveness before (backwards)
                    bit_set(&mut defs[dest.local as usize], p);
                }
            }
        }
        let preds = predecessors(bir);
        let mut live = vec![None; nl];
        let mut drop_live = vec![None; nl];
        // drop-live only where the local may be initialized (rustc liveness trace): per drop
        // local, its maybe-init points
        let drop_locals: Vec<usize> = (0..nl).filter(|l| relevant[*l] && !drops[*l].is_empty()).collect();
        let mut minit_pts: HashMap<usize, Bits> = HashMap::default();
        if !drop_locals.is_empty() {
            for l in &drop_locals {
                minit_pts.insert(*l, bits_new(self.n_points));
            }
            for (bi, b) in bir.blocks.iter().enumerate() {
                crate::preempt::point();
                let mut st = match &minit[bi] {
                    Some(e) => e.clone(),
                    None => continue,
                };
                for si in 0..=b.stmts.len() {
                    crate::preempt::point();
                    let p = self.start[bi] + si;
                    for l in &drop_locals {
                        if super::moves::maybe_init(md, &Place::local(*l as Local), &st) {
                            bit_set(minit_pts.get_mut(l).unwrap(), p);
                        }
                    }
                    if si < b.stmts.len() {
                        super::moves::init_stmt_effect(md, &b.stmts[si].kind, &mut st);
                    }
                }
            }
        }
        for l in 0..nl {
            if !relevant[l] {
                continue;
            }
            if !uses[l].is_empty() {
                live[l] = Some(self.trace(&uses[l], &defs[l], &preds, None));
            }
            if !drops[l].is_empty() {
                drop_live[l] = Some(self.trace(&drops[l], &defs[l], &preds, minit_pts.get(&l)));
            }
        }
        (live, drop_live)
    }

    /// Points from which a use is reachable without passing a def (backwards from the uses).
    /// The use point itself is live; a def at the use point (`x = f(x)`) does not stop it.
    fn trace(&self, uses: &[usize], defs: &Bits, preds: &[Vec<u32>], only: Option<&Bits>) -> Bits {
        // `only`: points outside it (the local definitely uninitialized) end the trace
        let ok = |q: usize| only.map_or(true, |b| bit_get(b, q));
        let mut live = bits_new(self.n_points);
        let mut stack: Vec<usize> = Vec::new();
        for &u in uses {
            if !bit_get(&live, u) {
                bit_set(&mut live, u);
                stack.push(u);
            }
        }
        while let Some(p) = stack.pop() {
            crate::preempt::point();
            // predecessor points
            let b = self.point_block[p] as usize;
            let first = self.start[b];
            if p > first {
                let q = p - 1;
                if !bit_get(defs, q) && ok(q) {
                    if !bit_get(&live, q) {
                        bit_set(&mut live, q);
                        stack.push(q);
                    }
                } else {
                    // the def point itself: the value is (re)defined there; rustc marks the def
                    // point live only for uses at it
                }
            } else {
                for &pb in &preds[b] {
                    let q = self.start[pb as usize] + self.bir.blocks[pb as usize].stmts.len();
                    if !bit_get(defs, q) && ok(q) {
                        if !bit_get(&live, q) {
                            bit_set(&mut live, q);
                            stack.push(q);
                        }
                    }
                }
            }
        }
        live
    }

    // Constraint-generation adapter. The ported core owns all graph/value inference.
    fn prepare_regions(&mut self) -> Vec<bool> {
        // Match nightly's allocation order: global, external, local, existential.
        let mut core_to_bir: Vec<_> = (0..self.kinds.len() as Rv).collect();
        let rank = |r: Rv| if r == STATIC { 0 } else if self.is_placeholder(r as usize) || self.kinds[r as usize] == Kind::Var { 3 }
            else if self.kinds[r as usize] == Kind::External || self.outer_named.iter().any(|x| x.1 == r) { 1 } else { 2 };
        core_to_bir.sort_by_key(|&r| (rank(r),r));
        self.core_to_bir = core_to_bir;
        self.region_map = vec![0; self.kinds.len()];
        for (vid,&r) in self.core_to_bir.iter().enumerate() { self.region_map[r as usize] = vid; }
        let definitions = self.core_to_bir.iter().map(|&r| {
            let external_name = self.region_ids[r as usize];
            let (origin,universe) = match external_name.map(|r| self.tcx.tys.region_kind(r)) {
                Some(crate::types::RegionKind::RePlaceholder { universe,.. }) => (NllRegionVariableOrigin::Placeholder(external_name.unwrap()),universe),
                _ => (if self.kinds[r as usize] == Kind::Var { NllRegionVariableOrigin::Existential } else { NllRegionVariableOrigin::FreeRegion },UniverseIndex::ROOT),
            };
            RegionDefinition { origin,universe,external_name }
        }).collect();
        let mut constraints = OutlivesConstraintSet::default();
        for &(sup,sub,p) in &self.cons {
            constraints.push(OutlivesConstraint { sup: self.region_map[sup as usize], sub: self.region_map[sub as usize], location: Some(p), span: { let s = self.span_at(p); crate::mir::Span { file: self.bir.file, lo: s.lo, hi: s.hi } }, from_closure: false,
                category: self.constraint_categories.get(&p).copied().unwrap_or_else(|| if self.ret_points.contains(&p) { ConstraintCategory::Return(super::constraints::ReturnConstraint::Normal) } else { ConstraintCategory::Assignment }) });
        }
        for c in &self.typeck_constraints.outlives_constraints.outlives {
            constraints.push(OutlivesConstraint { sup: self.region_map[c.sup], sub: self.region_map[c.sub], ..*c });
        }
        let lowered = compute_sccs_applying_placeholder_outlives_constraints(definitions, constraints, STATIC as usize);
        // Only loan-reachable points are needed by today's loan-scope consumer.
        let sccs = &lowered.constraint_sccs;
        let mut relevant = vec![false; sccs.num_sccs()];
        let mut stack: Vec<_> = self.loans.iter().map(|l| sccs.scc(self.region_map[l.region as usize])).collect();
        stack.extend(self.type_tests.iter().map(|(_,r,_)| sccs.scc(self.region_map[*r as usize])));
        stack.extend(self.typeck_constraints.type_tests.iter().map(|t| sccs.scc(self.region_map[t.lower_bound])));
        while let Some(s) = stack.pop() {
            crate::preempt::point();
            if relevant[s] { continue; } relevant[s] = true;
            stack.extend_from_slice(sccs.successors(s));
        }
        let regions = (0..self.kinds.len()).map(|r| relevant[sccs.scc(self.region_map[r])]).collect();
        self.lowered = Some(lowered);
        regions
    }
    fn infer_regions(&mut self, live: &[Option<Bits>], drop_live: &[Option<Bits>]) {
        let mut liveness = LivenessValues::new(self.kinds.len(), self.n_points);
        let mut add = |r: Rv, words: &Bits| {
            for (wi, &word) in words.iter().enumerate() {
                let mut word = word;
                while word != 0 {
                    let p = wi * 64 + word.trailing_zeros() as usize;
                    if p < self.n_points { liveness.add_point(self.region_map[r as usize],p); }
                    word &= word - 1;
                }
            }
        };
        for (l, rs) in self.lr.iter().enumerate() {
            if let Some(points) = &live[l] { for &r in rs { add(r, points); } }
            if let Some(points) = &drop_live[l] {
                let mut slots = vec![];
                self.rty.drop_slots(self.bir.locals[l].ty, &mut slots, 0, 0);
                for slot in slots { if let Some(&r) = rs.get(slot) { add(r, points); } }
            }
        }
        for &(r,p) in &self.promoted_live { liveness.add_point(self.region_map[r as usize],p); }
        let mut universals = vec![];
        for (r,k) in self.kinds.iter().enumerate() {
            if *k == Kind::Var || self.is_placeholder(r) { continue; }
            liveness.add_all_points(self.region_map[r]);
            let class = if r == STATIC as usize { RegionClassification::Global }
                else if *k == Kind::External || self.outer_named.iter().any(|x| x.1 as usize == r) { RegionClassification::External }
                else { RegionClassification::Local };
            universals.push((self.region_map[r],class));
        }
        universals.sort_by_key(|r| r.0);
        let fr_fn_body = self.names.iter().position(|n| n == "'fn_body").map(|r| self.region_map[r]);
        let universal_regions = UniversalRegions::new(STATIC as usize, fr_fn_body, universals);
        let known: Vec<_> = self.known.iter().map(|&(a,b)| (self.region_map[a as usize],self.region_map[b as usize])).collect();
        let relations = UniversalRegionRelations::create(universal_regions, self.kinds.len(), &known);
        let mut tests: Vec<_> = self.type_tests.iter().map(|&(pi,r,span)| TypeTest {
            generic_kind: self.tcx.tys.intern(TyKind::Param(pi)), span,
            lower_bound: self.region_map[r as usize],
            verify_bound: VerifyBound::AnyBound(std::iter::once(VerifyBound::IsEmpty)
                .chain(fr_fn_body.map(VerifyBound::OutlivedBy))
                .chain(self.tparam_bounds.iter().filter(|(p,_)| *p == pi)
                    .map(|(_,b)| VerifyBound::OutlivedBy(self.region_map[*b as usize]))).collect()),
        }).collect();
        for t in &self.typeck_constraints.type_tests {
            tests.push(TypeTest { generic_kind: self.remap_vids(t.generic_kind), lower_bound: self.region_map[t.lower_bound], span: t.span,
                verify_bound: self.remap_verify_bound(&t.verify_bound) });
        }
        self.regioncx = Some(RegionInferenceContext::solve(self.tcx, self.prog, self.lowered.take().unwrap(), relations,
            liveness, self.n_points, &tests, self.bir.closure_kind.is_some()));
    }
    /// Type-operation results use `Rv` numbering; the core numbers regions in nightly's
    /// allocation order (`region_map`).
    fn remap_vids(&self, ty: TyId) -> TyId {
        let tys = &self.tcx.tys;
        tys.map_regions(ty, &|r, _| match tys.region_kind(r) {
            crate::types::RegionKind::ReVar(v) => tys.intern_region(crate::types::RegionKind::ReVar(self.region_map[v as usize] as u32)),
            _ => r,
        }, 0)
    }
    fn remap_verify_bound(&self, bound: &VerifyBound) -> VerifyBound {
        match bound {
            VerifyBound::IfEq { ty, bound } => VerifyBound::IfEq { ty: self.remap_vids(*ty), bound: match self.tcx.tys.region_kind(*bound) {
                crate::types::RegionKind::ReVar(v) => self.tcx.tys.intern_region(crate::types::RegionKind::ReVar(self.region_map[v as usize] as u32)),
                _ => *bound,
            } },
            VerifyBound::IsEmpty => VerifyBound::IsEmpty,
            VerifyBound::OutlivedBy(r) => VerifyBound::OutlivedBy(self.region_map[*r]),
            VerifyBound::AnyBound(bs) => VerifyBound::AnyBound(bs.iter().map(|b| self.remap_verify_bound(b)).collect()),
            VerifyBound::AllBounds(bs) => VerifyBound::AllBounds(bs.iter().map(|b| self.remap_verify_bound(b)).collect()),
        }
    }
    fn is_placeholder(&self, r: usize) -> bool {
        self.region_ids[r].is_some_and(|r| matches!(self.tcx.tys.region_kind(r),crate::types::RegionKind::RePlaceholder { .. }))
    }
    fn regioncx(&self) -> &RegionInferenceContext { self.regioncx.as_ref().unwrap() }
    fn region_contains(&self, r: Rv, p: usize) -> bool { self.regioncx().region_contains_point(self.region_map[r as usize],p) }
    fn region_contains_univ_end(&self, r: Rv) -> Option<Rv> {
        self.regioncx().universal_regions_outlived_by(self.region_map[r as usize]).map(|r| self.core_to_bir[r])
            .find(|u| matches!(self.kinds[*u as usize], Kind::Univ | Kind::External))
    }

    // ------------------------------------------------------------------ loans

    fn borrow_checks(&mut self) {
        if self.loans.is_empty() {
            return;
        }
        let bir = self.bir;
        let np = self.n_points;
        let nl = self.loans.len();
        // two-phase activations: the first use of the holder temp after the reservation
        for li in 0..nl {
            if let BorrowKind::Mut { two_phase: true } = self.loans[li].kind {
                let h = self.loans[li].holder;
                self.loans[li].activation = self.find_use_after(h, self.loans[li].point);
            }
        }
        // dominators for is_active
        self.compute_dominators();
        // out-of-scope kills: from each loan's point, the first points (along every path) that
        // are not in its region
        let mut kills: Vec<Vec<u32>> = vec![Vec::new(); np];
        for li in 0..nl {
            let r = self.loans[li].region;
            let start = self.loans[li].point;
            let mut seen = bits_new(np);
            let mut stack: Vec<usize> = self.next_points(start);
            while let Some(q) = stack.pop() {
                crate::preempt::point();
                if bit_get(&seen, q) {
                    continue;
                }
                bit_set(&mut seen, q);
                if !self.region_contains(r, q) {
                    kills[q].push(li as u32);
                    continue;
                }
                stack.extend(self.next_points(q));
            }
        }
        // loans by local
        let mut by_local: HashMap<Local, Vec<u32>> = HashMap::default();
        for (i, l) in self.loans.iter().enumerate() {
            by_local.entry(l.place.local).or_default().push(i as u32);
        }
        // gen points
        let mut gens: HashMap<usize, Vec<u32>> = HashMap::default();
        for (i, l) in self.loans.iter().enumerate() {
            gens.entry(l.point).or_default().push(i as u32);
        }
        // forward dataflow over blocks
        let order = super::moves::rpo(bir);
        let nb = bir.blocks.len();
        let mut entry: Vec<Option<Bits>> = vec![None; nb];
        entry[0] = Some(bits_new(nl));
        let mut changed = true;
        let mut iters = 0;
        while changed && iters < 100 {
            changed = false;
            iters += 1;
            for &b in &order {
                crate::preempt::point();
                let mut st = match &entry[b as usize] {
                    Some(x) => x.clone(),
                    None => continue,
                };
                self.block_transfer(b as usize, &mut st, &kills, &gens, &by_local);
                for s in successors(bir, b) {
                    match &mut entry[s as usize] {
                        Some(e) => {
                            if bits_union(e, &st) {
                                changed = true;
                            }
                        }
                        slot @ None => {
                            *slot = Some(st.clone());
                            changed = true;
                        }
                    }
                }
            }
        }
        // checks
        for &b in &order {
            crate::preempt::point();
            let mut st = match &entry[b as usize] {
                Some(x) => x.clone(),
                None => continue,
            };
            let blk = &bir.blocks[b as usize];
            for si in 0..=blk.stmts.len() {
                crate::preempt::point();
                let p = self.start[b as usize] + si;
                for k in &kills[p] {
                    super::moves::bit_clear(&mut st, *k as usize);
                }
                self.check_point(b as usize, si, &st, &by_local);
                self.point_effect(b as usize, si, &mut st, &gens, &by_local);
            }
        }
    }

    fn next_points(&self, p: usize) -> Vec<usize> {
        let b = self.point_block[p] as usize;
        let last = self.start[b] + self.bir.blocks[b].stmts.len();
        if p < last {
            vec![p + 1]
        } else {
            successors(self.bir, b as u32).into_iter().map(|s| self.start[s as usize]).collect()
        }
    }

    fn find_use_after(&self, h: Local, p: usize) -> Option<usize> {
        let mut seen = vec![false; self.n_points];
        let mut stack = self.next_points(p);
        while let Some(q) = stack.pop() {
            crate::preempt::point();
            if seen[q] {
                continue;
            }
            seen[q] = true;
            if self.point_uses_local(q, h) {
                return Some(q);
            }
            stack.extend(self.next_points(q));
        }
        None
    }

    fn point_uses_local(&self, p: usize, l: Local) -> bool {
        let b = self.point_block[p] as usize;
        let i = p - self.start[b];
        let blk = &self.bir.blocks[b];
        if i < blk.stmts.len() {
            match &blk.stmts[i].kind {
                StmtKind::Assign(d, rv) => rvalue_locals(rv).contains(&l) || (!d.proj.is_empty() && place_locals(d).contains(&l)),
                StmtKind::FakeRead(_, pl) | StmtKind::PlaceMention(pl) => place_locals(pl).contains(&l),
                _ => false,
            }
        } else {
            match blk.term.as_ref().map(|t| &t.kind) {
                Some(TermKind::Call { args, dest, .. }) => args.iter().any(|o| o.place().map_or(false, |pl| place_locals(pl).contains(&l))) || (!dest.proj.is_empty() && place_locals(dest).contains(&l)),
                Some(TermKind::Switch(o, _)) => o.place().map_or(false, |pl| place_locals(pl).contains(&l)),
                Some(TermKind::Drop(pl, _)) => pl.local == l,
                Some(TermKind::Return) => l == 0,
                _ => false,
            }
        }
    }

    fn block_transfer(&self, b: usize, st: &mut Bits, kills: &[Vec<u32>], gens: &HashMap<usize, Vec<u32>>, by_local: &HashMap<Local, Vec<u32>>) {
        let blk = &self.bir.blocks[b];
        for si in 0..=blk.stmts.len() {
            crate::preempt::point();
            let p = self.start[b] + si;
            for k in &kills[p] {
                super::moves::bit_clear(st, *k as usize);
            }
            self.point_effect(b, si, st, gens, by_local);
        }
    }

    fn point_effect(&self, b: usize, si: usize, st: &mut Bits, gens: &HashMap<usize, Vec<u32>>, by_local: &HashMap<Local, Vec<u32>>) {
        let blk = &self.bir.blocks[b];
        let p = self.start[b] + si;
        if si < blk.stmts.len() {
            match &blk.stmts[si].kind {
                StmtKind::Assign(d, _) => {
                    self.kill_on_place(d, st, by_local);
                    if let Some(g) = gens.get(&p) {
                        for x in g {
                            bit_set(st, *x as usize);
                        }
                    }
                }
                StmtKind::StorageDead(l) => {
                    if let Some(v) = by_local.get(l) {
                        for x in v {
                            super::moves::bit_clear(st, *x as usize);
                        }
                    }
                }
                _ => {}
            }
        } else if let Some(Term { kind: TermKind::Call { dest, .. }, .. }) = &blk.term {
            self.kill_on_place(dest, st, by_local);
        }
    }

    /// rustc kill_borrows_on_place: overwriting a local kills all its loans; a projection kills
    /// the loans of places that definitely conflict with it.
    fn kill_on_place(&self, p: &Place, st: &mut Bits, by_local: &HashMap<Local, Vec<u32>>) {
        let v = match by_local.get(&p.local) {
            Some(v) => v,
            None => return,
        };
        for x in v {
            let l = &self.loans[*x as usize];
            if p.proj.is_empty() || self.places_conflict(&l.place, l.kind, p, Depth::Deep, false) {
                super::moves::bit_clear(st, *x as usize);
            }
        }
    }

    // ------------------------------------------------------------------ accesses

    fn check_point(&mut self, b: usize, si: usize, st: &Bits, by_local: &HashMap<Local, Vec<u32>>) {
        let blk = &self.bir.blocks[b];
        let p = self.start[b] + si;
        // activations of two-phase borrows at this point
        let acts: Vec<usize> = (0..self.loans.len()).filter(|i| self.loans[*i].activation == Some(p)).collect();
        for li in acts {
            let place = self.loans[li].place.clone();
            let span = if si < blk.stmts.len() { blk.stmts[si].span } else { blk.term.as_ref().map_or(Span::default(), |t| t.span) };
            self.access_fv(p, &place, span, Depth::Deep, Rw::Activation(li), st, by_local, Some(li));
        }
        if si < blk.stmts.len() {
            let s = &blk.stmts[si];
            match &s.kind {
                StmtKind::Assign(d, rv) => {
                    self.consume_rvalue(p, rv, s.span, st, by_local);
                    self.access(p, d, s.span, Depth::Shallow, Rw::WriteMutate, st, by_local);
                }
                StmtKind::StorageDead(l) => {
                    self.access(p, &Place::local(*l), s.span, Depth::Shallow, Rw::WriteStorageDead, st, by_local);
                }
                _ => {}
            }
        } else if let Some(t) = &blk.term {
            match &t.kind {
                TermKind::Switch(o, _) => self.consume_op(p, o, t.span, st, by_local),
                TermKind::Call { args, dest, arg_spans, target, .. } => {
                    for (i, o) in args.iter().enumerate() {
                        let sp = arg_spans.get(i).copied().unwrap_or(t.span);
                        self.consume_op(p, o, sp, st, by_local);
                    }
                    self.access(p, dest, t.span, Depth::Deep, Rw::WriteMutate, st, by_local);
                    if target.is_none() {
                        // a diverging call leaves the fn by unwinding: the locals die there
                        // (rustc check_for_invalidation_at_exit at UnwindResume); a borrow of a
                        // local that must outlive the fn (its region reaches a universal
                        // region's end) is invalidated
                        self.exit_check(p, t.span, st);
                    }
                }
                TermKind::Drop(pl, _) => {
                    self.cur_drop = Some(pl.clone());
                    self.access(p, pl, t.span, Depth::Drop, Rw::WriteStorageDead, st, by_local);
                    self.cur_drop = None;
                }
                // a loop's false unwind edge (rustc: to the cleanup that ends the fn)
                TermKind::FalseUnwind(..) => self.exit_check(p, t.span, st),
                _ => {}
            }
        }
    }

    fn exit_check(&mut self, p: usize, span: Span, st: &Bits) {
        for li in 0..self.loans.len() {
            if !bit_get(st, li) {
                continue;
            }
            let l = &self.loans[li];
            // through a reference / raw pointer the place is not the fn's own storage
            if l.place.proj.iter().enumerate().any(|(i, pr)| *pr == Proj::Deref && {
                let base = Place { local: l.place.local, proj: l.place.proj[..i].to_vec() };
                self.tx.box_inner(place_ty(self.tx, self.bir, &base)).is_none()
            }) {
                continue;
            }
            if self.region_contains_univ_end(l.region).is_none() {
                continue;
            }
            let pl = Place::local(l.place.local);
            self.report(p, &pl, span, Rw::WriteStorageDead, li);
        }
    }

    fn consume_rvalue(&mut self, p: usize, rv: &Rvalue, span: Span, st: &Bits, by_local: &HashMap<Local, Vec<u32>>) {
        match rv {
            Rvalue::Use(o) | Rvalue::Cast(_, o, _) | Rvalue::Repeat(o) => self.consume_op(p, o, span, st, by_local),
            Rvalue::Aggregate(_, ops) | Rvalue::Op(_, ops) => {
                for o in ops {
                    self.consume_op(p, o, span, st, by_local);
                }
            }
            Rvalue::Ref(_, bk, pl) => {
                let (d, rw) = match bk {
                    BorrowKind::Shallow => (Depth::ShallowFake, Rw::ReadBorrow(*bk)),
                    BorrowKind::Shared | BorrowKind::Fake => (Depth::Deep, Rw::ReadBorrow(*bk)),
                    BorrowKind::Mut { two_phase: true } => (Depth::Deep, Rw::Reservation(*bk)),
                    _ => (Depth::Deep, Rw::WriteBorrow(*bk)),
                };
                let own = self.loans.iter().position(|l| l.point == p);
                self.access_fv(p, pl, span, d, rw, st, by_local, own);
            }
            Rvalue::RawPtr(m, pl) => {
                let rw = if *m { Rw::WriteBorrow(BorrowKind::Mut { two_phase: false }) } else { Rw::ReadBorrow(BorrowKind::Shared) };
                self.access(p, pl, span, Depth::Deep, rw, st, by_local);
            }
            Rvalue::Discriminant(pl) => self.access(p, pl, span, Depth::Shallow, Rw::ReadCopy, st, by_local),
            Rvalue::Len(pl) => self.access(p, pl, span, Depth::ShallowLen, Rw::ReadCopy, st, by_local),
        }
    }

    fn consume_op(&mut self, p: usize, o: &Operand, span: Span, st: &Bits, by_local: &HashMap<Local, Vec<u32>>) {
        match o {
            Operand::Copy(pl) => self.access(p, pl, span, Depth::Deep, Rw::ReadCopy, st, by_local),
            Operand::Move(pl) => self.access(p, pl, span, Depth::Deep, Rw::WriteMove, st, by_local),
            Operand::Const(_) => {}
        }
    }

    fn is_active(&self, li: usize, p: usize) -> bool {
        let l = &self.loans[li];
        let act = match l.kind {
            BorrowKind::Mut { two_phase: true } => l.activation,
            _ => return true,
        };
        let act = match act {
            Some(a) => a,
            None => return false,
        };
        if self.point_dominates(act, p) {
            return true;
        }
        let res = l.point + 1;
        if self.point_dominates(res, p) {
            false
        } else {
            true
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn access(&mut self, p: usize, place: &Place, span: Span, depth: Depth, rw: Rw, st: &Bits, by_local: &HashMap<Local, Vec<u32>>) {
        self.access_fv(p, place, span, depth, rw, st, by_local, None)
    }

    /// One access against the loans in scope. `own`: the access is loan `own`'s own borrow /
    /// activation (field views narrow it to that loan's parts).
    #[allow(clippy::too_many_arguments)]
    fn access_fv(&mut self, p: usize, place: &Place, span: Span, depth: Depth, rw: Rw, st: &Bits, by_local: &HashMap<Local, Vec<u32>>, own: Option<usize>) {
        let cands = match by_local.get(&place.local) {
            Some(v) => v.clone(),
            None => return,
        };
        for li in cands {
            let li = li as usize;
            if !bit_get(st, li) {
                continue;
            }
            let (lplace, lkind) = (self.loans[li].place.clone(), self.loans[li].kind);
            let strict = self.pair_err(p, place, rw, depth, &lplace, lkind, li);
            if !strict {
                continue;
            }
            // a guard's own ref bindings are taken under the match's fake borrows (rustc binds
            // them for the guard before checking it)
            if lkind == BorrowKind::Shallow && own.map_or(false, |o| self.bir.guard_locals.iter().any(|g| g.0 == self.loans[o].holder)) {
                continue;
            }
            // extended two-phase borrows (relaxed Rust): a `&mut` reserved for a call argument
            // and not used until the call may be borrowed again meanwhile (`f(vm, g(vm))`),
            // when that cannot move what the reservation points to: the reserved place is
            // the accessed place or reached from it through fields only. Loans the meanwhile
            // borrow leaves live still conflict at the activation.
            if self.ext_two_phase(p, place, rw, &lplace, lkind, li) {
                let n0 = self.diags.len();
                self.report(p, place, span, rw, li);
                if self.diags.len() > n0 {
                    let mut d = self.diags.pop().unwrap();
                    d.fv_only = true;
                    let at = self.loans[li].span;
                    d.label(at, "reserved here for the call, not used before it".to_string(), false);
                    d.note("accepted with extended two-phase borrows (relaxed Rust): the reserved borrow is only used by the call, after this".to_string());
                    // the export to rustc: the argument holding this access is evaluated into a
                    // local before the statement
                    if let Some(Feed::Call(bi, _)) = self.loan_feed[li] {
                        if let Some(TermKind::Call { arg_spans, .. }) = self.bir.blocks[bi].term.as_ref().map(|t| &t.kind) {
                            if let Some(k) = arg_spans.iter().position(|a| a.lo <= span.lo && span.hi <= a.hi && a.hi > a.lo) {
                                self.fv_records.push((arg_spans[k], String::new(), false, span, Some(arg_spans[..k].to_vec())));
                            }
                        }
                    }
                    if self.relaxed {
                        self.fv_only.push(d);
                    } else {
                        self.diags.push(d);
                    }
                }
                if self.relaxed {
                    continue;
                }
                return;
            }
            // field views: the access's own parts against the loan's parts
            let acc_parts: Option<Vec<(Place, Rw)>> = own.and_then(|o| self.parts(o)).map(|v| {
                v.into_iter()
                    .map(|(q, k)| {
                        let r2 = match rw {
                            Rw::Activation(a) => {
                                if k.is_mut() {
                                    Rw::Activation(a)
                                } else {
                                    Rw::ReadBorrow(BorrowKind::Shared)
                                }
                            }
                            Rw::Reservation(_) | Rw::WriteBorrow(_) | Rw::ReadBorrow(_) => {
                                if k.is_mut() {
                                    rw
                                } else {
                                    Rw::ReadBorrow(BorrowKind::Shared)
                                }
                            }
                            x => x,
                        };
                        (q, r2)
                    })
                    .collect()
            });
            let loan_parts = self.parts(li);
            let fv = if acc_parts.is_none() && loan_parts.is_none() {
                true
            } else {
                let ap = acc_parts.unwrap_or_else(|| vec![(place.clone(), rw)]);
                let lp = loan_parts.unwrap_or_else(|| vec![(lplace.clone(), lkind)]);
                let mut any = false;
                'o: for (aq, arw) in &ap {
                    for (lq, lk) in &lp {
                        if self.pair_err(p, aq, *arw, depth, lq, *lk, li) {
                            any = true;
                            break 'o;
                        }
                    }
                }
                any
            };
            if !fv {
                // accepted only with field views
                let n0 = self.diags.len();
                self.report(p, place, span, rw, li);
                if self.diags.len() > n0 {
                    let mut d = self.diags.pop().unwrap();
                    d.fv_only = true;
                    let why = self.fv_explain(own, li);
                    d.note(format!("accepted with field views (relaxed Rust): {}", why));
                    // the export to rustc: the field-level borrow (the loan, or the access when
                    // the loan is the whole value) is moved out around the statement
                    let (fspan, fplace) = if self.loans[li].place.proj.iter().any(|x| matches!(x, Proj::Field(..))) { (self.loans[li].span, self.loans[li].place.clone()) } else { (span, place.clone()) };
                    let ftxt = describe_place(self.tx, self.bir, &fplace).unwrap_or_default();
                    let fty = super::moves::place_ty(self.tx, self.bir, &fplace);
                    let dflt = self.tx.lang("default::Default").map_or(false, |td| self.tx.evaluate_trait(td, fty, &[]).must_apply_modulo_regions());
                    self.fv_records.push((fspan, ftxt, dflt, span, None));
                    if self.relaxed {
                        self.fv_only.push(d);
                    } else {
                        self.diags.push(d);
                    }
                }
                if self.relaxed {
                    continue;
                }
                return;
            }
            let n0 = self.diags.len();
            self.report(p, place, span, rw, li);
            if (self.loan_parts[li].as_ref().map_or(false, |x| x.is_some()) || own.map_or(false, |o| self.loan_parts[o].as_ref().map_or(false, |x| x.is_some()))) && self.diags.len() > n0 {
                // a conflict that remains through field views: name what the call touches
                let why = self.fv_explain(own, li);
                if let Some(d) = self.diags.last_mut() {
                    d.note(format!("field views: {}", why));
                }
            }
            return;
        }
    }

    /// The type params occurring in `t` (outlives components: through references, ADTs,
    /// tuples, arrays; not through fn pointers / trait objects / closures).
    fn params_in(&self, t: TyId, out: &mut Vec<u32>, depth: u32) {
        if depth > 20 {
            return;
        }
        match self.tx.kind(t) {
            TyKind::Param(i) => {
                if !out.contains(&i) {
                    out.push(i);
                }
            }
            TyKind::Ref(_, x, _) | TyKind::Ptr(_, x) | TyKind::Array(x, _) | TyKind::Slice(x) => self.params_in(x, out, depth + 1),
            TyKind::Tuple(v) => {
                for x in self.tcx.tys.list(v).iter().copied() {
                    self.params_in(x, out, depth + 1);
                }
            }
            TyKind::Adt(_, args, _) => {
                for x in self.tcx.tys.list(args).iter().copied() {
                    self.params_in(x, out, depth + 1);
                }
            }
            // a projection outlives 'r when its trait's input types do (rustc's projection
            // outlives rule without a where-clause on the projection itself)
            TyKind::Alias(_, AliasTy { kind: AliasTyKind::Projection { def_id: ad }, args, regions: _ }) => {
                if self.proj_bounds || self.rty.assoc_has_lifetime_bound(ad) {
                    return;
                }
                for x in self.tcx.tys.list(args).iter().copied() {
                    self.params_in(x, out, depth + 1);
                }
            }
            _ => {}
        }
    }

    /// A type with parts this body cannot see through (type params, projections, opaques).
    fn has_unknown(&self, t: TyId, depth: u32) -> bool {
        if depth > 20 {
            return true;
        }
        match self.tx.kind(t) {
            TyKind::Param(_) | TyKind::Alias(_, AliasTy { kind: AliasTyKind::Projection { .. }, .. }) | TyKind::Alias(_, AliasTy { kind: AliasTyKind::Opaque { .. }, .. }) | TyKind::Infer(_) | TyKind::Error => true,
            TyKind::Ref(_, x, _) | TyKind::Ptr(_, x) | TyKind::Array(x, _) | TyKind::Slice(x) => self.has_unknown(x, depth + 1),
            TyKind::Tuple(v) => self.tcx.tys.list(v).iter().any(|x| self.has_unknown(*x, depth + 1)),
            TyKind::Adt(_, a, _) => self.tcx.tys.list(a).iter().any(|x| self.has_unknown(*x, depth + 1)),
            _ => false,
        }
    }

    fn has_fn_ptr(&self, t: TyId, depth: u32) -> bool {
        if depth > 20 {
            return true;
        }
        match self.tx.kind(t) {
            TyKind::FnPtr(..) | TyKind::Dyn(..) | TyKind::Closure(..) | TyKind::Alias(_, AliasTy { kind: AliasTyKind::Opaque { .. }, .. }) => true,
            TyKind::Ref(_, x, _) | TyKind::Ptr(_, x) | TyKind::Array(x, _) | TyKind::Slice(x) => self.has_fn_ptr(x, depth + 1),
            TyKind::Tuple(v) => self.tcx.tys.list(v).iter().any(|x| self.has_fn_ptr(*x, depth + 1)),
            TyKind::Adt(_, a, _) => self.tcx.tys.list(a).iter().any(|x| self.has_fn_ptr(*x, depth + 1)),
            _ => false,
        }
    }

    fn type_test(&mut self, t: TyId, r: Rv, span: Span) {
        let mut ps = Vec::new();
        self.params_in(t, &mut ps, 0);
        for p in ps {
            self.type_tests.push((p, r, span));
        }
    }

    fn ext_two_phase(&self, p: usize, place: &Place, rw: Rw, lplace: &Place, lkind: BorrowKind, li: usize) -> bool {
        if !matches!(lkind, BorrowKind::Mut { two_phase: true }) || self.is_active(li, p) {
            return false;
        }
        let ok_rw = match rw {
            Rw::WriteBorrow(_) | Rw::Reservation(_) | Rw::ReadBorrow(_) | Rw::ReadCopy => true,
            Rw::Activation(a) => a != li,
            _ => false,
        };
        if !ok_rw {
            return false;
        }
        let k = place.proj.len();
        lplace.proj.len() <= k || lplace.proj[k..].iter().all(|x| matches!(x, Proj::Field(..)))
    }

    /// "`m` only uses `self.counter`" for the field-view parts of the access / the loan.
    fn fv_explain(&mut self, own: Option<usize>, li: usize) -> String {
        let mut out = Vec::new();
        for x in [own, Some(li)].into_iter().flatten() {
            let parts = match self.parts(x) {
                Some(v) => v,
                None => continue,
            };
            let who = match self.loan_feed[x] {
                Some(Feed::Call(bi, _)) => match self.bir.blocks[bi].term.as_ref().map(|t| &t.kind) {
                    Some(TermKind::Call { callee: Callee::Fn(d, _) | Callee::Overloaded(d, _), .. }) => format!("`{}`", self.prog.name(*d)),
                    _ => "the call".to_string(),
                },
                Some(Feed::Closure(..)) => "the closure".to_string(),
                None => continue,
            };
            let mut muts = Vec::new();
            let mut refs = Vec::new();
            for (q, k) in &parts {
                let d = describe_place(self.tx, self.bir, q).unwrap_or_else(|| "value".to_string());
                if k.is_mut() {
                    muts.push(format!("`{}`", d));
                } else {
                    refs.push(format!("`{}`", d));
                }
            }
            let mut s = format!("{} uses", who);
            if !muts.is_empty() {
                s.push_str(&format!(" mutably {}", muts.join(", ")));
            }
            if !refs.is_empty() {
                if !muts.is_empty() {
                    s.push_str(" and");
                }
                s.push_str(&format!(" shared {}", refs.join(", ")));
            }
            if muts.is_empty() && refs.is_empty() {
                s.push_str(" nothing of it");
            }
            out.push(s);
        }
        out.join("; ")
    }

    /// An access of `place` (`rw`, `depth`) against a borrow of `lplace` (`lkind`): an error?
    #[allow(clippy::too_many_arguments)]
    fn pair_err(&self, p: usize, place: &Place, rw: Rw, depth: Depth, lplace: &Place, lkind: BorrowKind, li: usize) -> bool {
        if !self.places_conflict(lplace, lkind, place, depth, true) {
            return false;
        }
        match (rw, lkind) {
            (Rw::Activation(a), _) if a == li => false,
            (Rw::ReadCopy | Rw::ReadBorrow(_), BorrowKind::Shared | BorrowKind::Fake | BorrowKind::Shallow) => false,
            (Rw::ReadBorrow(BorrowKind::Shallow), BorrowKind::Mut { .. } | BorrowKind::Unique) => false,
            (Rw::Reservation(_), BorrowKind::Shared | BorrowKind::Fake | BorrowKind::Shallow) => false,
            (Rw::WriteMove, BorrowKind::Shallow) => false,
            (Rw::ReadCopy | Rw::ReadBorrow(_), BorrowKind::Mut { .. } | BorrowKind::Unique) => self.is_active(li, p),
            _ => true,
        }
    }

    /// rustc borrow_conflicts_with_place (bias Overlap when `overlap`, NoOverlap for kills).
    fn places_conflict(&self, borrow: &Place, bk: BorrowKind, access: &Place, depth: Depth, overlap: bool) -> bool {
        if borrow.local != access.local {
            return false;
        }
        if borrow.proj.is_empty() && access.proj.is_empty() {
            return true;
        }
        let n = borrow.proj.len().min(access.proj.len());
        let mut t = self.bir.locals[borrow.local as usize].ty;
        for i in 0..n {
            let (b, a) = (&borrow.proj[i], &access.proj[i]);
            match (b, a) {
                (Proj::Deref, Proj::Deref) => {}
                (Proj::Field(f1, _), Proj::Field(f2, _)) => {
                    if f1 != f2 {
                        if self.tx.is_union(t) {
                            return true;
                        }
                        return false;
                    }
                }
                (Proj::Downcast(v1), Proj::Downcast(v2)) => {
                    if v1 != v2 {
                        return false;
                    }
                }
                (Proj::Index(_), Proj::Index(_) | Proj::ConstIndex(..) | Proj::Subslice(..)) | (Proj::ConstIndex(..) | Proj::Subslice(..), Proj::Index(_)) => {
                    if !overlap {
                        return false;
                    }
                }
                (Proj::ConstIndex(o1, e1, m1), Proj::ConstIndex(o2, e2, m2)) => {
                    if e1 == e2 {
                        if o1 != o2 {
                            return false;
                        }
                    } else {
                        // from the front vs from the end (rustc: with the longer min length)
                        let (begin, end) = if !*e1 { (*o1, *o2) } else { (*o2, *o1) };
                        let ml = (*m1).max(*m2);
                        if begin < ml.saturating_sub(end) {
                            return false;
                        }
                    }
                }
                (Proj::ConstIndex(o, false, _), Proj::Subslice(f, to, false)) | (Proj::Subslice(f, to, false), Proj::ConstIndex(o, false, _)) => {
                    if !(f <= o && o < to) {
                        return false;
                    }
                }
                (Proj::ConstIndex(o, false, _), Proj::Subslice(f, _, true)) | (Proj::Subslice(f, _, true), Proj::ConstIndex(o, false, _)) => {
                    if o < f {
                        return false;
                    }
                }
                (Proj::ConstIndex(o, true, _), Proj::Subslice(_, to, true)) | (Proj::Subslice(_, to, true), Proj::ConstIndex(o, true, _)) => {
                    if o <= to {
                        return false;
                    }
                }
                (Proj::Subslice(f1, t1, false), Proj::Subslice(f2, t2, false)) => {
                    if f2 >= t1 || f1 >= t2 {
                        return false;
                    }
                }
                (Proj::Subslice(..), Proj::Subslice(..)) => {}
                _ => {
                    // mismatched projections (different types): treat as overlapping
                    return true;
                }
            }
            t = super::moves::proj_ty(self.tx, t, b);
        }
        if borrow.proj.len() > access.proj.len() {
            let mut bt = t;
            for i in access.proj.len()..borrow.proj.len() {
                let e = &borrow.proj[i];
                match (e, depth) {
                    (_, Depth::ShallowLen) | (_, Depth::ShallowFake) => return false,
                    (Proj::Deref, Depth::Shallow) => return false,
                    (Proj::Deref, Depth::Drop) => {
                        if matches!(self.tx.kind(bt), TyKind::Ref(..)) {
                            return false;
                        }
                    }
                    (Proj::Field(..), Depth::Drop) => {
                        if self.tx.has_drop_impl(bt) {
                            return true;
                        }
                    }
                    _ => {}
                }
                bt = super::moves::proj_ty(self.tx, bt, e);
            }
        }
        if bk == BorrowKind::Shallow && borrow.proj.len() < access.proj.len() {
            return false;
        }
        true
    }

    // ------------------------------------------------------------------ errors

    fn report(&mut self, p: usize, place: &Place, span: Span, rw: Rw, li: usize) {
        if self.reported.iter().any(|(q, s)| q == place && *s == span) {
            return;
        }
        self.reported.push((place.clone(), span));
        let loan_place = self.loans[li].place.clone();
        let loan_span = self.loans[li].span;
        let lkind = self.loans[li].kind;
        let what = describe_place(self.tx, self.bir, place).unwrap_or_else(|| "value".to_string());
        let bwhat = describe_place(self.tx, self.bir, &loan_place).unwrap_or_else(|| "value".to_string());
        let lk = |k: BorrowKind| if k.is_mut() { "mutable" } else { "immutable" };
        let later = self.later_use(li, p);
        // a match guard's fake borrow (rustc cannot_mutate_in_immutable_section, E0510)
        if lkind == BorrowKind::Shallow {
            let action = match rw {
                Rw::WriteMutate => Some("assign"),
                Rw::WriteBorrow(bk) | Rw::Reservation(bk) if bk.is_mut() => Some("mutably borrow"),
                _ => None,
            };
            if let Some(action) = action {
                let ctx = if self.bir.index_fakes.contains(&self.loans[li].holder) { "indexing expression" } else { "match guard" };
                let mut d = Diag::new("E0510", format!("cannot {} `{}` in {}", action, what, ctx), span);
                d.label(span, format!("cannot {}", action), true);
                d.label(loan_span, format!("value is immutable in {}", ctx), false);
                self.diags.push(d);
                return;
            }
        }
        let mut d = match rw {
            Rw::ReadCopy => {
                let mut d = Diag::new("E0503", format!("cannot use `{}` because it was mutably borrowed", what), span);
                d.label(loan_span, format!("`{}` is borrowed here", bwhat), false);
                d.label(span, format!("use of borrowed `{}`", bwhat), true);
                d
            }
            Rw::ReadBorrow(bk) | Rw::WriteBorrow(bk) | Rw::Reservation(bk) => {
                let (code, msg) = if bk.is_mut() && lkind.is_mut() {
                    ("E0499", format!("cannot borrow `{}` as mutable more than once at a time", what))
                } else if bk == BorrowKind::Unique || lkind == BorrowKind::Unique {
                    ("E0501", format!("cannot borrow `{}` as {} because previous closure requires unique access", what, lk(bk)))
                } else {
                    ("E0502", format!("cannot borrow `{}` as {} because it is also borrowed as {}", what, lk(bk), lk(lkind)))
                };
                let mut d = Diag::new(code, msg, span);
                d.label(loan_span, format!("{} borrow occurs here", if lkind.is_mut() { if bk.is_mut() { "first mutable" } else { "mutable" } } else { "immutable" }), false);
                d.label(span, format!("{} borrow occurs here", if bk.is_mut() { if lkind.is_mut() { "second mutable" } else { "mutable" } } else { "immutable" }), true);
                d
            }
            Rw::Activation(a) => {
                let bk = self.loans[a].kind;
                let (code, msg) = if lkind.is_mut() {
                    ("E0499", format!("cannot borrow `{}` as mutable more than once at a time", what))
                } else {
                    ("E0502", format!("cannot borrow `{}` as mutable because it is also borrowed as immutable", what))
                };
                let _ = bk;
                let mut d = Diag::new(code, msg, self.loans[a].span);
                d.label(loan_span, format!("{} borrow occurs here", lk(lkind)), false);
                d.label(self.loans[a].span, "mutable borrow occurs here".to_string(), true);
                d
            }
            Rw::WriteMove => {
                let mut d = Diag::new("E0505", format!("cannot move out of `{}` because it is borrowed", what), span);
                d.label(loan_span, format!("borrow of `{}` occurs here", bwhat), false);
                d.label(span, format!("move out of `{}` occurs here", what), true);
                d
            }
            Rw::WriteMutate => {
                let mut d = Diag::new("E0506", format!("cannot assign to `{}` because it is borrowed", what), span);
                d.label(loan_span, format!("`{}` is borrowed here", bwhat), false);
                d.label(span, format!("`{}` is assigned to here but it was already borrowed", what), true);
                d
            }
            Rw::WriteStorageDead if matches!(self.loan_feed[li], Some(Feed::Closure(..))) && (self.region_contains_univ_end(self.loans[li].region).is_some() || later.is_some())
                // a closure escaping as the fn's `impl Trait` result: rustc keeps E0597
                && !self.via_closure_static(self.loans[li].region)
                && !self.bir.def.and_then(|d| self.tcx.fn_sig(self.prog, d)).map_or(false, |sg| matches!(self.tx.kind(sg.ret), TyKind::Alias(_, AliasTy { kind: AliasTyKind::Opaque { .. }, .. }))) => {
                // a capture of a closure that outlives the fn / the block whose local it
                // borrows (rustc report_escaping_closure_capture, E0373)
                let what_scope = if self.region_contains_univ_end(self.loans[li].region).is_some() { "function" } else { "block" };
                let root = describe_place(self.tx, self.bir, &Place::local(loan_place.local)).unwrap_or_else(|| bwhat.clone());
                let mut d = Diag::new("E0373", format!("closure may outlive the current {}, but it borrows `{}`, which is owned by the current {}", what_scope, root, what_scope), loan_span);
                d.label(loan_span, format!("`{}` is borrowed here", bwhat), true);
                d.note("to force the closure to take ownership of the variables it uses, use the `move` keyword".to_string());
                d
            }
            // a borrow through a reference the dropped value owns, whose type runs a destructor
            // (rustc report_borrow_conflicts_with_destructor, E0713)
            Rw::WriteStorageDead if self.drop_through_destructor(&loan_place).is_some() => {
                let dp = self.cur_drop.clone().unwrap();
                let dty = self.drop_through_destructor(&loan_place).unwrap();
                let dwhat = describe_place(self.tx, self.bir, &dp).unwrap_or_else(|| "value".to_string());
                let mut d = Diag::new("E0713", "borrow may still be in use when destructor runs".to_string(), loan_span);
                d.label(loan_span, String::new(), true);
                d.label(span, format!("here, drop of `{}` needs exclusive access to `{}`, because the type `{}` implements the `Drop` trait", dwhat, bwhat, self.tx.ty_str(dty)), false);
                d
            }
            Rw::WriteStorageDead => {
                let escapes = self.region_contains_univ_end(self.loans[li].region);
                let is_temp = self.bir.locals[loan_place.local as usize].name.is_none();
                let ret_escape = escapes.map_or(false, |u| self.lr.get(0).map_or(false, |r0| r0.iter().any(|x| self.regioncx().constraint_sccs.scc(self.region_map[*x as usize]) == self.regioncx().constraint_sccs.scc(self.region_map[u as usize]) || self.univ_flows(*x, u))));
                if ret_escape {
                    // rustc points at the returned value: the fn's tail expression when the
                    // value leaves through it, else the `return`
                    let rs = escapes.map(|u| self.blame_span(self.loans[li].region, u)).unwrap_or_default();
                    let tail = self.fn_tail_span();
                    // the value leaves through the tail when the return place is written inside it
                    let via_tail = tail.map_or(false, |ts| self.ret_points.iter().any(|q| {
                        let sp = self.span_at(*q);
                        sp.lo >= ts.lo && sp.hi <= ts.hi && sp != Span::default()
                    }));
                    let loan_span = match (rs == Span::default(), tail) {
                        (_, Some(ts)) if via_tail && !self.ret_points.iter().any(|q| { let sp = self.span_at(*q); sp != Span::default() && (sp.lo < ts.lo || sp.hi > ts.hi) }) => ts,
                        (false, Some(ts)) if rs.lo >= ts.lo && rs.hi <= ts.hi => ts,
                        (false, _) => rs,
                        (true, Some(ts)) => ts,
                        (true, None) => loan_span,
                    };
                    let msg = if is_temp {
                        "cannot return value referencing temporary value".to_string()
                    } else {
                        format!("cannot return value referencing local variable `{}`", bwhat)
                    };
                    let mut d = Diag::new("E0515", msg, loan_span);
                    d.label(loan_span, if is_temp { "temporary value created here".to_string() } else { format!("`{}` is borrowed here", bwhat) }, true);
                    d
                } else if is_temp {
                    let mut d = Diag::new("E0716", "temporary value dropped while borrowed".to_string(), loan_span);
                    d.label(loan_span, "creates a temporary value which is freed while still in use".to_string(), true);
                    d.label(span, "temporary value is freed at the end of this statement".to_string(), false);
                    d
                } else {
                    // a closure capture: rustc points at the use in the closure
                    let loan_span = self.bir.capture_spans.iter().find(|x| x.0 == self.loans[li].holder).map_or(loan_span, |x| x.1);
                    let mut d = Diag::new("E0597", format!("`{}` does not live long enough", bwhat), loan_span);
                    d.label(loan_span, "borrowed value does not live long enough".to_string(), true);
                    d.label(span, format!("`{}` dropped here while still borrowed", bwhat), false);
                    d
                }
            }
        };
        if let Some(ls) = later {
            d.label(ls, format!("{} borrow later used here", lk(lkind)), false);
        }
        self.diags.push(d);
    }

    /// The span of the fn body's tail expression (not for closures).
    fn fn_tail_span(&self) -> Option<Span> {
        if self.bir.closure_kind.is_some() {
            return None;
        }
        let d = self.bir.def?;
        let pd = self.prog.def(d);
        let ast = &self.prog.files[pd.file as usize].ast;
        let body = match &ast.item(pd.item).kind {
            crate::ast::ItemKind::Fn(_, b, _) => b.or_else(|| self.prog.fn_body(pd.file, pd.item).ok().flatten())?,
            _ => return None,
        };
        match ast.stmts(body).last() {
            Some(crate::ast::Stmt::Expr(e, false)) => {
                let x = ast.expr(*e);
                Some(Span { lo: x.lo, hi: x.hi })
            }
            _ => None,
        }
    }

    fn univ_flows(&self, from: Rv, u: Rv) -> bool {
        self.regioncx().universal_regions_outlived_by(self.region_map[from as usize]).any(|r| r == self.region_map[u as usize])
    }

    fn later_use(&self, li: usize, p: usize) -> Option<Span> {
        let r = self.loans[li].region;
        let rs = self.regioncx().constraint_sccs.scc(self.region_map[r as usize]);
        // sccs reachable from the loan region (sup -> sub): their locals keep it live
        let mut reach = vec![false; self.regioncx().constraint_sccs.num_sccs()];
        let mut stack = vec![rs];
        while let Some(s) = stack.pop() {
            crate::preempt::point();
            if reach[s as usize] {
                continue;
            }
            reach[s as usize] = true;
            stack.extend(self.regioncx().constraint_sccs.successors(s as usize).iter().copied());
        }
        let locals: Vec<Local> = (0..self.lr.len()).filter(|l| self.lr[*l].iter().any(|x| reach[self.regioncx().constraint_sccs.scc(self.region_map[*x as usize]) as usize])).map(|l| l as Local).collect();
        let mut seen = vec![false; self.n_points];
        let mut stack = self.next_points(p);
        while let Some(q) = stack.pop() {
            crate::preempt::point();
            if seen[q] || !self.region_contains(r, q) {
                continue;
            }
            seen[q] = true;
            for l in &locals {
                if *l != self.loans[li].holder && self.point_uses_local(q, *l) || *l == self.loans[li].holder && self.point_uses_local(q, *l) {
                    return Some(self.span_at(q));
                }
            }
            stack.extend(self.next_points(q));
        }
        None
    }

    fn span_at(&self, p: usize) -> Span {
        if p >= self.n_points {
            return self.pseudo_spans.get(p - self.n_points).copied().unwrap_or_default();
        }
        let b = self.point_block[p] as usize;
        let i = p - self.start[b];
        let blk = &self.bir.blocks[b];
        if i < blk.stmts.len() {
            blk.stmts[i].span
        } else {
            blk.term.as_ref().map_or(Span::default(), |t| t.span)
        }
    }

    /// Universal regions that must outlive universals they are not known to (rustc
    /// check_universal_regions): "lifetime may not live long enough"; closure bodies collect
    /// the requirements between their external regions for the parent.
    fn region_checks(&mut self) {
        // Translate only the core's verdict and outward requirements to BIR slots.
        let encode = |r: usize| {
            let r = self.core_to_bir[r] as usize;
            if r == STATIC as usize { return u32::MAX; }
            // Parent generic arguments precede signature slots in closure_mapping.
            if let Some((name,_)) = self.outer_named.iter().find(|x| x.1 as usize == r) {
                let ce = self.bir.closure_expr.unwrap();
                let mut all = self.rty.closure_req_names.borrow_mut();
                let names = all.entry(ce).or_default();
                let k = names.iter().position(|n| n == name).unwrap_or_else(|| { names.push(name.clone()); names.len()-1 });
                NAMED_REQ + k as u32
            } else { *self.ext_slot.get(&(r as Rv)).unwrap_or_else(|| panic!("external region missing its closure coordinate: def {:?}, typeck root {:?} ({}), closure {:?}, BIR region {}, external slots {:?}, outer names {:?}", self.bir.def, self.bir.owner, self.bir.owner.map(|d| self.prog.def_path(d)).unwrap_or_default(), self.bir.closure_expr, r, self.ext_slot, self.outer_named)) }
        };
        let mut requirements: Vec<_> = self.regioncx().propagated_outlives_requirements.iter().map(|c|
            ClosureOutlivesRequirement { subject:ClosureOutlivesSubject::Region(encode(c.sup)),outlived_free_region:encode(c.sub),blame_span:Span { lo: c.span.lo, hi: c.span.hi },category:c.category }).collect();
        for req in &self.regioncx().propagated_type_requirements {
            // Today's producer emits Param subjects; generic/alias decomposition
            // remains with constraint generation until C0b/C2.
            assert!(matches!(self.tcx.tys.kind(req.subject),TyKind::Param(_)));
            requirements.push(ClosureOutlivesRequirement { subject:ClosureOutlivesSubject::Ty(req.subject),outlived_free_region:encode(req.outlived_free_region),blame_span:req.blame_span,category:ConstraintCategory::Boring });
        }
        for req in requirements { if !self.requirements.contains(&req) { self.requirements.push(req); } }
        for error in self.regioncx().errors.clone() {
            let (a,b) = match error {
                RegionErrorKind::RegionError { longer_fr, shorter_fr, is_reported } => {
                    if !is_reported { continue; } (longer_fr,shorter_fr)
                }
                RegionErrorKind::PlaceholderOutlivesIllegalRegion { longer_fr, illegally_outlived_r } => (longer_fr,illegally_outlived_r),
                RegionErrorKind::TypeTest { test } if test >= self.type_tests.len() => {
                    // A type-operation test (`T: 'r` from normalization): nightly's
                    // generic "may not live long enough" report.
                    let t = &self.typeck_constraints.type_tests[test - self.type_tests.len()];
                    let code = if self.regioncx().constraint_sccs.scc(self.region_map[t.lower_bound]) == self.regioncx().constraint_sccs.scc(self.region_map[STATIC as usize]) { "E0310" } else { "E0309" };
                    let what = match self.tcx.tys.kind(t.generic_kind) {
                        TyKind::Param(pi) if (*pi as usize) < self.tparam_names.len() => format!("the parameter type `{}`", self.tparam_names[*pi as usize]),
                        _ => format!("the type `{}`", self.tx.ty_str(self.tcx.tys.erase_all_regions(t.generic_kind))),
                    };
                    let mut d = Diag::new(code, format!("{what} may not live long enough"), t.span);
                    d.label(t.span, "...so that the type will meet its required lifetime bounds".into(), true);
                    self.diags.push(d);
                    continue;
                }
                RegionErrorKind::TypeTest { test } => {
                    let (pi,r,span) = self.type_tests[test];
                    let Some(name) = self.tparam_names.get(pi as usize) else { continue; };
                    let Some(u) = self.regioncx().universal_regions_outlived_by(self.region_map[r as usize]).map(|u| self.core_to_bir[u] as usize).find(|u| {
                        matches!(self.kinds[*u], Kind::Univ) && !self.tparam_bounds.iter().any(|(p,b)| *p == pi && self.regioncx().universal_region_relations.outlives(self.region_map[*b as usize],self.region_map[*u]))
                    }) else { continue; };
                    if let Some((_, bound)) = self.tparam_bounds.iter().find(|(p,b)| *p == pi && *b != STATIC) {
                        let blame = self.blame_span(r,u as Rv);
                        let at = if blame == Span::default() { span } else { blame };
                        let mut d = Diag::new("", "lifetime may not live long enough".into(), at);
                        d.label(at, format!("this requires that `{}` must outlive `{}`",pretty(&self.names[*bound as usize]),pretty(&self.names[u])), true);
                        self.diags.push(d);
                    } else {
                        let code = if u == STATIC as usize { "E0310" } else { "E0309" };
                        let mut d = Diag::new(code, format!("the parameter type `{}` may not live long enough", name),span);
                        d.label(span,format!("requires `{}` to outlive `{}`",name,pretty(&self.names[u])),true);
                        self.diags.push(d);
                    }
                    continue;
                }
            };
            let (a,b) = (self.core_to_bir[a] as usize,self.core_to_bir[b] as usize);
            let span = self.blame_span(a as Rv,b as Rv);
            let env_escape = self.bir.closure_kind.is_some() && self.names[a] == "'env";
            let escapes = self.bir.closure_kind.is_some() && self.hr_regions.contains(&(a as Rv))
                && b != STATIC as usize && !self.lr[0].contains(&(b as Rv));
            let (code,msg) = if env_escape {
                ("", if self.bir.closure_kind == Some(ClosureKind::FnMut) { "captured variable cannot escape `FnMut` closure body" }
                    else { "returns a reference to a captured variable which escapes the closure body" })
            }
                else if escapes { ("E0521", "borrowed data escapes outside of closure") }
                else { ("", "lifetime may not live long enough") };
            let mut d = Diag::new(code,msg.into(),span);
            d.label(span,format!("this requires that `{}` must outlive `{}`",pretty(&self.names[a]),pretty(&self.names[b])),true);
            self.diags.push(d);
        }
    }

    fn field_fnptr_constraints(&mut self, pl: &Place, args: &[Operand], p: usize) {
        let Some((Proj::Field(fi, _), base)) = pl.proj.split_last().map(|(a, b)| (a.clone(), b)) else { return };
        let bp = Place { local: pl.local, proj: base.to_vec() };
        let (bt, br) = self.place_regions(&bp);
        let TyKind::Adt(d, _, _) = self.tx.kind(bt) else { return };
        let Some((file, module, fat)) = super::rty::struct_field_ast(self.prog, d, fi) else { return };
        let ast = &self.prog.files[file as usize].ast;
        let params = match ast.ty(fat) {
            crate::ast::Ty::Fn(fp) => ast.list(ast.fn_ptr(*fp).params).to_vec(),
            _ => return,
        };
        // the struct's lifetime params are its first slots
        let pd = self.prog.def(d);
        let names: Vec<Vec<u8>> = match &ast.item(pd.item).kind {
            crate::ast::ItemKind::Struct(r) => ast.list(ast.row(*r).generics.params).iter().filter(|x| matches!(x.kind, crate::ast::GenericParamKind::Lifetime(_))).map(|x| self.prog.files[file as usize].text(x.name).as_bytes().to_vec()).collect(),
            _ => return,
        };
        let mut lts: Vec<(Vec<u8>, u32)> = Vec::new();
        for (i, n) in names.into_iter().enumerate() {
            if let Some(r) = br.get(i) {
                lts.push((n, *r));
            }
        }
        for (i, pa) in params.iter().enumerate() {
            let Some((at, ar)) = args.get(i + 1).and_then(|a| self.op_regions(a)) else { continue };
            let ann = self.rty.annotation(file, module, *pa, at, &lts);
            if ann.len() != ar.len() {
                continue;
            }
            for (r, a) in ar.iter().zip(ann.iter()) {
                match a {
                    Ann::Static => self.outlives(*r, STATIC, p),
                    Ann::Rv(x) => self.outlives(*r, *x as Rv, p),
                    _ => {}
                }
            }
        }
    }

    fn static_callee_constraints(&mut self, d: crate::program::DefId, args: &[Operand], p: usize) {
        let pd = self.prog.def(d);
        let file = pd.file;
        let ast = &self.prog.files[file as usize].ast;
        let mut t = match &ast.item(pd.item).kind {
            crate::ast::ItemKind::Static(_, t, _) => *t,
            _ => return,
        };
        let params: Vec<crate::ast::TyId> = loop {
            match ast.ty(t) {
                crate::ast::Ty::Ref(_, _, x) | crate::ast::Ty::Paren(x) => t = *x,
                crate::ast::Ty::Fn(fp) => break ast.list(ast.fn_ptr(*fp).params).to_vec(),
                crate::ast::Ty::DynTrait(bs, _) => {
                    let mut ps = None;
                    for b in ast.list(*bs) {
                        if let crate::ast::Bound::Trait { path, .. } = b {
                            if let Some(seg) = ast.segs(path.segs).last() {
                                if let Some(crate::ast::GenericArgs::Paren(v, _)) = ast.gargs(seg.args()) {
                                    ps = Some(ast.list(*v).to_vec());
                                }
                            }
                        }
                    }
                    match ps {
                        Some(v) => break v,
                        None => return,
                    }
                }
                _ => return,
            }
        };
        for (i, pa) in params.iter().enumerate() {
            let Some((at, ar)) = args.get(i + 1).and_then(|a| self.op_regions(a)) else { continue };
            let ann = self.rty.annotation(file, pd.scope, *pa, at, &[]);
            if ann.len() != ar.len() {
                continue;
            }
            for (r, a) in ar.iter().zip(ann.iter()) {
                if *a == Ann::Static {
                    self.outlives(*r, STATIC, p);
                }
            }
        }
    }

    /// The Drop terminator being checked drops a value that owns the reference `loan` borrows
    /// through, inside a type with a destructor (rustc E0713): that type.
    fn drop_through_destructor(&self, loan: &Place) -> Option<TyId> {
        let dp = self.cur_drop.as_ref()?;
        if loan.local != dp.local || loan.proj.len() <= dp.proj.len() || loan.proj[..dp.proj.len()] != dp.proj[..] {
            return None;
        }
        let k = (dp.proj.len()..loan.proj.len()).find(|i| loan.proj[*i] == Proj::Deref)?;
        for n in dp.proj.len()..=k {
            let q = Place { local: loan.local, proj: loan.proj[..n].to_vec() };
            let t = place_ty(self.tx, self.bir, &q);
            if self.tx.has_drop_impl(t) {
                return Some(t);
            }
        }
        None
    }

    /// `a` outlives 'static through a closure requirement (closure_static).
    fn via_closure_static(&self, a: Rv) -> bool {
        if self.closure_static.is_empty() {
            return false;
        }
        let mut seen = vec![false; self.kinds.len()];
        let mut q = vec![a];
        seen[a as usize] = true;
        while let Some(x) = q.pop() {
            if self.closure_static.contains(&x) {
                return true;
            }
            for (sup, sub, _) in &self.cons {
                if *sup == x && !seen[*sub as usize] {
                    seen[*sub as usize] = true;
                    q.push(*sub);
                }
            }
        }
        false
    }

    fn blame_span(&self, a: Rv, b: Rv) -> Span {
        self.regioncx().best_blame_constraint(self.tcx, self.region_map[a as usize],self.region_map[b as usize]).map_or(Span::default(), |c| Span { lo: c.span.lo, hi: c.span.hi })
    }

    // ------------------------------------------------------------------ dominators

    fn compute_dominators(&mut self) {
        let bir = self.bir;
        let order = super::moves::rpo(bir);
        let nb = bir.blocks.len();
        let mut idx = vec![u32::MAX; nb];
        for (i, b) in order.iter().enumerate() {
            idx[*b as usize] = i as u32;
        }
        let preds = predecessors(bir);
        let mut idom = vec![u32::MAX; nb];
        idom[0] = 0;
        let mut changed = true;
        while changed {
            changed = false;
            for &b in order.iter().skip(1) {
                let mut new = u32::MAX;
                for &p in &preds[b as usize] {
                    if idom[p as usize] == u32::MAX {
                        continue;
                    }
                    new = if new == u32::MAX { p } else { intersect(&idom, &idx, p, new) };
                }
                if new != u32::MAX && idom[b as usize] != new {
                    idom[b as usize] = new;
                    changed = true;
                }
            }
        }
        self.dominators = idom;
        self.rpo_index = idx;
    }

    fn block_dominates(&self, a: u32, b: u32) -> bool {
        let mut x = b;
        loop {
            if x == a {
                return true;
            }
            let d = self.dominators.get(x as usize).copied().unwrap_or(u32::MAX);
            if d == u32::MAX || d == x {
                return false;
            }
            x = d;
        }
    }

    fn point_dominates(&self, a: usize, b: usize) -> bool {
        if a >= self.n_points || b >= self.n_points {
            return false;
        }
        let (ba, bb) = (self.point_block[a], self.point_block[b]);
        if ba == bb {
            a <= b
        } else {
            self.block_dominates(ba, bb)
        }
    }
}

fn intersect(idom: &[u32], idx: &[u32], mut a: u32, mut b: u32) -> u32 {
    while a != b {
        while idx[a as usize] > idx[b as usize] {
            a = idom[a as usize];
        }
        while idx[b as usize] > idx[a as usize] {
            b = idom[b as usize];
        }
    }
    a
}

fn pretty(n: &str) -> String {
    if n == "'_" {
        "'1".to_string()
    } else {
        n.to_string()
    }
}

pub fn place_locals(p: &Place) -> Vec<Local> {
    let mut v = vec![p.local];
    for pr in &p.proj {
        if let Proj::Index(l) = pr {
            v.push(*l);
        }
    }
    v
}

pub fn rvalue_locals(rv: &Rvalue) -> Vec<Local> {
    let mut v = Vec::new();
    let op = |o: &Operand, v: &mut Vec<Local>| {
        if let Some(p) = o.place() {
            v.extend(place_locals(p));
        }
    };
    match rv {
        Rvalue::Use(o) | Rvalue::Cast(_, o, _) | Rvalue::Repeat(o) => op(o, &mut v),
        Rvalue::Aggregate(_, ops) | Rvalue::Op(_, ops) => {
            for o in ops {
                op(o, &mut v);
            }
        }
        Rvalue::Ref(_, _, p) | Rvalue::RawPtr(_, p) | Rvalue::Discriminant(p) | Rvalue::Len(p) => v.extend(place_locals(p)),
    }
    v
}
