//! Build-owned queries over immutable O1 snapshots. A slot is an instance key within
//! one Unit; these keys never escape to disk (cache.rs encodes stable function keys).
//! rustc_middle/src/ty/layout.rs:1236,1300 and rustc_ty_utils/src/abi.rs:601 attach
//! unwinding to the callee instance/ABI. Relax additionally analyzes its O1 RIR.

use crate::query::{Fingerprint, QueryEngine, QueryError, QueryId, QueryTable};
use crate::rir::{Callee, Func, Inst, Term};
use std::cell::RefCell;
use std::sync::Arc;

type Build = Result<(Func, Vec<(u32, usize)>), String>;

fn slot_fp(slot: &u32) -> Fingerprint { Fingerprint::from_bytes(&slot.to_le_bytes()) }
fn bool_fp(value: &bool) -> Fingerprint { Fingerprint::from_bytes(&[*value as u8]) }
fn facts_fp(facts: &[(u32, bool)]) -> Fingerprint {
    let mut h = crate::sip::Sip128::new();
    for &(slot, value) in facts { h.u32(slot); h.u8(value as u8); }
    Fingerprint(h.finish128())
}

/// The projection of optimized RIR read by body_may_unwind. Non-call instructions
/// affect this input only through reachability; invoked calls cannot escape their pad.
fn body_fp(f: &Func) -> Fingerprint {
    let reach = crate::regalloc::reachable(f);
    let mut h = crate::sip::Sip128::new();
    for (bi, b) in f.blocks.iter().enumerate() {
        if !reach[bi] { continue; }
        if matches!(b.term, Term::Resume) { h.u8(0); }
        for (ii, i) in b.insts.iter().enumerate() {
            if ii + 1 == b.insts.len() && matches!(b.term, Term::Invoke(..)) { continue; }
            if let Inst::Call(c, _, _) = i {
                match c {
                    Callee::Fn(slot) => { h.u8(1); h.u32(*slot); }
                    _ => { h.u8(2); h.u8(crate::opt::may_unwind(c, &|_| false) as u8); }
                }
            }
        }
    }
    Fingerprint(h.finish128())
}

fn inputs_fp(inputs: &Vec<(u32, bool)>) -> Fingerprint { facts_fp(inputs) }

/// One immutable build boundary, including the available candidates (absence is an
/// input too). No live Unit or process-global compilation state is read by providers.
/// Separate contexts let off-lock builds finish against their original preparations.
/// rustc_middle/src/ty/context/tls.rs:27,63: Q1 supplies the scoped ImplicitCtxt.
pub(crate) struct QueryCx {
    engine: QueryEngine,
    nounwind: QueryTable<u32, bool>,
    inputs: QueryTable<u32, Vec<(u32, bool)>>,
    bodies: Vec<(u32, Arc<Func>)>,
}

impl QueryCx {
    pub(crate) fn new(candidates: &[(u32, Arc<Func>, u8)]) -> Self {
        let mut engine = QueryEngine::new();
        let mut bodies = Vec::with_capacity(candidates.len());
        let mut available = crate::sip::Sip128::new();
        for (slot, body, _) in candidates {
            // The inliner's snapshot lookup selects the first body for a slot.
            if bodies.iter().any(|b: &(u32, Arc<Func>)| b.0 == *slot) { continue; }
            // Input keys are the candidate's position, not its slot: the graph allocates a
            // 256-node page per key/256, and slots are sparse (one 5 KB page per candidate
            // and kind, per compiled function). The stable identity is still the slot.
            let key = bodies.len() as u32;
            bodies.push((*slot, body.clone()));
            available.u32(*slot);
            engine.register_input(1, key, slot_fp(slot)).unwrap();
            engine.set_input(1, key, body_fp(body)).unwrap();
            engine.register_input(2, key, slot_fp(slot)).unwrap();
            engine.set_input(2, key, facts_fp(&body.nounwind_inputs)).unwrap();
        }
        engine.register_input(0, 0, Fingerprint::from_bytes(b"candidate snapshot")).unwrap();
        engine.set_input(0, 0, Fingerprint(available.finish128())).unwrap();
        Self { engine, nounwind: QueryTable::new(3, slot_fp, bool_fp), inputs: QueryTable::new(4, slot_fp, inputs_fp), bodies }
    }

    fn force(&self, id: QueryId) -> Result<(), QueryError> {
        match id.kind {
            3 => { self.get(self.nounwind.key(id.key).ok_or(QueryError::MissingQuery(id))?)?; Ok(()) }
            _ => Err(QueryError::MissingQuery(id)),
        }
    }

    fn get(&self, slot: u32) -> Result<bool, QueryError> {
        self.engine.enter_context(|| self.nounwind.get(&self.engine, slot, &mut |slot| {
            self.engine.input(0, 0)?;
            let Some(key) = self.bodies.iter().position(|b| b.0 == slot) else { return Ok(false) };
            let body = &self.bodies[key].1;
            self.engine.input(1, key as u32)?;
            self.engine.input(2, key as u32)?;
            // Use this body's own build answers, never answers derived from its caller's
            // snapshot. Unbuilt/recursive callees were recorded as false in that build.
            // Rust's unwind ABI defaults to may-unwind without rustc_nounwind:
            // rustc_middle/src/ty/layout.rs:1238,1300. No optimistic cycle seed.
            Ok(!crate::opt::body_may_unwind(body, &|callee| {
                body.nounwind_inputs.iter().find(|f| f.0 == callee).is_some_and(|f| f.1)
            }))
        }, &|id| self.force(id)))
    }

    /// The optimizer receives a query oracle explicitly. Afterwards the caller's input
    /// query reads the same immutable answers, retaining caller -> callee dependency edges.
    /// Q6's full RIR query will consume this input query; cache.rs persists its result now.
    pub(crate) fn optimize(mut self, slot: u32, mut f: Func, build: &mut dyn FnMut(&mut Func, &dyn Fn(u32) -> bool) -> Result<Vec<(u32, usize)>, String>) -> Build {
        let used = RefCell::new(crate::fxhash::HashMap::default());
        let answer = |callee| {
            let value = self.get(callee).expect("nounwind query");
            used.borrow_mut().insert(callee, value);
            value
        };
        let inlined = build(&mut f, &answer)?;
        // Capture the answers needed when this body later becomes a callee input.
        crate::opt::body_may_unwind(&f, &answer);
        let mut requested = Vec::new();
        for fact in used.into_inner() { requested.push(fact); }
        requested.sort_unstable();
        f.nounwind_inputs = self.build_inputs(slot, &requested);
        Ok((f, inlined))
    }

    fn build_inputs(&mut self, slot: u32, requested: &[(u32, bool)]) -> Vec<(u32, bool)> {
        self.engine.register_input(0, 2, Fingerprint::from_bytes(b"consulted callees")).unwrap();
        self.engine.set_input(0, 2, facts_fp(requested)).unwrap();
        self.engine.enter_context(|| self.inputs.get(&self.engine, slot, &mut |_| {
            self.engine.input(0, 2)?;
            let mut inputs = Vec::with_capacity(requested.len());
            for &(callee, _) in requested { inputs.push((callee, self.get(callee)?)); }
            Ok(inputs)
        }, &|id| self.force(id))).expect("build nounwind inputs query")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(callees: &[u32]) -> Func {
        let mut f = Func::new("nounwind-test".into(), 0);
        f.block();
        for &callee in callees {
            f.blocks[0].insts.push(Inst::Call(Callee::Fn(callee), vec![], vec![]));
            f.blocks[0].pos.push(0);
        }
        f.blocks[0].term = Term::Ret(vec![]);
        f
    }

    #[test]
    fn candidate_body_and_declared_answers_are_dependencies() {
        let mut f = body(&[2]);
        f.nounwind_inputs.push((2, true));
        let mut q = QueryCx::new(&[(1, Arc::new(f), 0)]);
        assert_eq!(q.build_inputs(10, &[(1, true), (99, false)]), vec![(1, true), (99, false)]);
        let root = q.inputs.query_id(&10).unwrap();
        let callee = q.nounwind.query_id(&1).unwrap();
        let absent = q.nounwind.query_id(&99).unwrap();
        assert_eq!(q.engine.dependencies(root), vec![QueryId { kind: 0, key: 2 }, callee, absent]);
        assert!(q.engine.dependencies(callee).contains(&QueryId { kind: 1, key: 0 }));
        assert!(q.engine.dependencies(callee).contains(&QueryId { kind: 2, key: 0 }));
        assert!(q.engine.dependencies(absent).contains(&QueryId { kind: 0, key: 0 }));
    }

    #[test]
    fn same_slot_in_other_build_cannot_change_an_answer() {
        let a = QueryCx::new(&[(1, Arc::new(body(&[])), 0)]);
        let b = QueryCx::new(&[(1, Arc::new(body(&[99])), 0)]);
        assert!(a.get(1).unwrap());
        assert!(!b.get(1).unwrap());
        assert!(a.get(1).unwrap());
        assert!(!QueryCx::new(&[]).get(1).unwrap());
        let first = Arc::new(body(&[]));
        let duplicate = QueryCx::new(&[(1, first.clone(), 0), (1, Arc::new(body(&[99])), 0)]);
        assert!(duplicate.get(1).unwrap());
        assert_eq!(duplicate.engine.input(1, 0).unwrap(), body_fp(&first));
    }

    #[test]
    fn recursive_and_unbuilt_calls_stay_conservative() {
        let a = Arc::new(body(&[2]));
        let b = Arc::new(body(&[1]));
        let q = QueryCx::new(&[(1, a, 0), (2, b, 0), (3, Arc::new(body(&[3])), 0)]);
        for slot in [1, 2, 3, 99] { assert!(!q.get(slot).unwrap()); }
    }

    #[test]
    fn callee_uses_its_own_inputs() {
        let mut a = body(&[2]);
        a.nounwind_inputs.push((2, false));
        let candidates = [(1, Arc::new(a), 0), (2, Arc::new(body(&[])), 0)];
        let q = QueryCx::new(&candidates);
        assert!(!q.get(1).unwrap());
        assert!(q.get(2).unwrap());
    }

    #[test]
    fn optimizer_records_both_answers_and_preserves_cleanup() {
        let mut f = body(&[1]);
        f.block();
        f.block();
        f.blocks[0].term = Term::Invoke(1, 2);
        f.blocks[1].term = Term::Ret(vec![]);
        // A nonempty pad must survive a may-unwind call.
        f.blocks[2].insts.push(Inst::Call(Callee::Fn(99), vec![], vec![]));
        f.blocks[2].pos.push(0);
        f.blocks[2].term = Term::Resume;
        for known in [false, true] {
            let candidates = if known { vec![(1, Arc::new(body(&[])), 0)] } else { vec![] };
            let q = QueryCx::new(&candidates);
            let (out, _) = q.optimize(0, f.clone(), &mut |out, nounwind| {
                crate::opt::demote_invokes(out, nounwind);
                // Repeated consultation must be deduplicated.
                assert_eq!(nounwind(1), known);
                assert!(!nounwind(77));
                Ok(vec![])
            }).unwrap();
            assert_eq!(matches!(out.blocks[0].term, Term::Jump(1)), known);
            assert_eq!(out.nounwind_inputs, vec![(1, known), (77, false)]);
        }
    }

    #[test]
    fn unreachable_unwind_does_not_taint_body_and_resume_does() {
        let mut f = body(&[]);
        f.block();
        f.blocks[1].term = Term::Resume;
        let q = QueryCx::new(&[(1, Arc::new(f.clone()), 0)]);
        assert!(q.get(1).unwrap());
        f.blocks[0].term = Term::Jump(1);
        assert!(!QueryCx::new(&[(1, Arc::new(f), 0)]).get(1).unwrap());
    }
}
