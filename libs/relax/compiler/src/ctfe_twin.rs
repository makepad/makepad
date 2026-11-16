//! Constant evaluation's code. A constant's initializer that ctfe.rs cannot evaluate as data runs
//! as native code (jit.rs run_init_code), and every function it runs there is a *twin*
//! (`FnKey::Ctfe(base)`): the base slot's body as lowered, counting rustc's const-eval steps.
//! rustc counts one step per `ConstEvalCounter`, which its CtfeLimit pass puts into every block
//! that ends in a call and every block a successor of which dominates it
//! (rustc_mir_transform/src/ctfe_limit.rs:15-58); the evaluation's lint fires at the step limit
//! (rustc_const_eval/src/const_eval/machine.rs, LINT_TERMINATOR_LIMIT). A twin counts:
//! - one step before every call of a function slot (drop glue excepted: MIR drops are not calls)
//!   and every indirect call;
//! - one step at every back edge of a source loop (lower.rs `back_edge`, while it lowers the
//!   twin's body: `lowering`). Loops the lowering makes for what MIR does in one statement (an
//!   array repeat) count nothing.
//! Its calls go to the callees' twins; fn values stay the run-time slots (an indirect call counts
//! at its site, not inside its target). A twin is baseline code, never inlined or tiered up, and
//! keeps every frame (`Func::keep_frames`). The disk cache keeps its code under its base's key
//! (cache.rs KeyKind::Ctfe): a static whose value has no entry (std's STDOUT, interior
//! mutability) runs its initializer in every run, and a warm run links the twins it runs. Run-time slots are untouched: a `const fn`
//! that both a constant and the program call counts nothing when the program calls it.
//! The limit and the verdict at it are jit.rs's (CONST_FUEL, ConstEvalScope).

use crate::jit::{self, FnKey, Unit};
use crate::program::DefKind;
use relax_backend::rir::{Callee, Func, Inst};

/// Nanoseconds spent building twins' RIR (lowering and `steps`), for the run summary (`census`).
static BUILD_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// What this module holds of a run, as a fresh process has it (`run_end`).
pub(crate) unsafe fn reset_statics() {
    relax_session::reset_static(&BUILD_NS, std::sync::atomic::AtomicU64::new(0));
}

/// (twins with code, their code bytes, ms building them: RIR and machine code) for a summary line.
pub(crate) fn census(u: &Unit) -> (usize, u64, f64) {
    let (mut n, mut bytes, mut cg) = (0, 0u64, 0u64);
    for f in u.fns.iter().filter(|f| f.compiled && matches!(f.key, FnKey::Ctfe(_))) {
        n += 1;
        bytes += f.code_len as u64;
        cg += f.cg_ns;
    }
    (n, bytes, (BUILD_NS.load(std::sync::atomic::Ordering::Relaxed) + cg) as f64 / 1e6)
}

thread_local! {
    /// the slot whose body this thread lowers for a constant-evaluation twin (twin_rir)
    static LOWERING: std::cell::Cell<Option<u32>> = const { std::cell::Cell::new(None) };
}

/// Whether the body of slot `cur_fn` is lowered for its constant-evaluation twin (lower.rs Lcx::new).
pub(crate) fn lowering(cur_fn: u32) -> bool {
    LOWERING.with(|c| c.get()) == Some(cur_fn)
}

/// The constant-evaluation twin of slot `id` (made on first use); `id` itself for a slot that has
/// none: shims, glue, foreign functions, twins.
pub(crate) fn twin(u: &Unit, id: u32) -> u32 {
    if id as usize >= u.fns.len() {
        return id;
    }
    let base = match &u.fns[id as usize].key {
        FnKey::Inst(d, _) if u.prog.def(*d).kind != DefKind::ForeignFn => id,
        FnKey::Closure(..) => id,
        // an unverified-mode twin is its base's body: the same constant-evaluation twin
        FnKey::Plain(b) => *b,
        _ => return id,
    };
    u.fn_id(FnKey::Ctfe(base))
}

/// Slot `base`'s twin, if one was made.
pub(crate) fn twin_of(u: &Unit, base: u32) -> Option<u32> {
    u.fn_map.get(&FnKey::Ctfe(base))
}

/// Twin `id`'s RIR, built from slot `base`'s lowering (owner thread).
pub(crate) unsafe fn twin_rir(up: *mut Unit, id: u32, base: u32) -> Result<Func, String> {
    /// (restored also when the lowering unwinds: a cancelled request is served again)
    struct Lowering(Option<u32>);
    impl Drop for Lowering {
        fn drop(&mut self) {
            let _ = LOWERING.try_with(|c| c.set(self.0));
        }
    }
    let t0 = std::time::Instant::now();
    jit::begin_build(up, id)?;
    let r = {
        let _lowering = Lowering(LOWERING.with(|c| c.replace(Some(base))));
        jit::source_rir(up, base)
    };
    jit::end_build(up, id);
    let mut f = r?;
    steps(&*up, &mut f);
    BUILD_NS.fetch_add(t0.elapsed().as_nanos() as u64, std::sync::atomic::Ordering::Relaxed);
    Ok(f)
}

/// Code a constant's evaluation runs (a twin's body, a promoted initializer): one step before each
/// call of a function slot that is not drop glue and before each indirect call; direct calls go
/// to the callees' twins; no sibling calls.
pub(crate) fn steps(u: &Unit, f: &mut Func) {
    let tick = u.syms.intern(crate::syms::SymKind::Rt("const_fuel"), crate::lower::const_fuel_addr());
    for b in f.blocks.iter_mut() {
        let mut k = 0;
        while k < b.insts.len() {
            let counted = match &mut b.insts[k] {
                Inst::Call(Callee::Fn(id), ..) => {
                    let glue = (*id as usize) < u.fns.len() && matches!(u.fns[*id as usize].key, FnKey::Glue(..));
                    if !glue {
                        *id = twin(u, *id);
                    }
                    !glue
                }
                Inst::Call(Callee::CFn(..) | Callee::Indirect(_) | Callee::CIndirect(..), ..) => true,
                _ => false,
            };
            if counted {
                let pos = b.pos[k];
                b.insts.insert(k, Inst::Call(Callee::Host(tick), Vec::new(), Vec::new()));
                b.pos.insert(k, pos);
                k += 1;
            }
            k += 1;
        }
    }
    for c in f.sret_callees.iter_mut() {
        *c = twin(u, *c);
    }
    f.keep_frames = true;
}
