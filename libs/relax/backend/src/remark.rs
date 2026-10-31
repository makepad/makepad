//! Optimisation remarks the back end records for an O3 build (makepad inspect `remarks`, `asm`):
//! which values the register allocator kept in stack slots and the circumstances -- how many calls
//! their range spans (caller-saved registers do not survive a call), how many values of their
//! class held registers where they start (pressure), how often they are used. Side metadata of
//! the finished allocation only: nothing here changes an instruction, a register or a slot.
//!
//! The front end turns the sink on around one build on its thread (`begin` / `take`); with it off
//! (every other build) `note_spills` returns at once.

use crate::regalloc::{Alloc, Loc};
use crate::rir::*;
use std::cell::RefCell;

/// One value the allocator spilled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpillRemark {
    /// source position of its definition (0: a parameter / block entry value)
    pub pos: u64,
    /// float / vector class (else integer)
    pub flt: bool,
    /// uses of the value
    pub uses: u32,
    /// calls inside its live range
    pub calls: u32,
    /// values of its class in registers live where it starts
    pub pressure: u32,
    /// positions its range spans (allocator positions: two per instruction)
    pub span: u32,
}

/// A loop a vectoriser looked at and how far it got: `stage` is the last check it passed (each
/// pass names its stages, `LoopRemark::describe`); `done`: it was vectorised.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LoopRemark {
    /// source position of the loop's head (its first instruction, else its terminator)
    pub pos: u64,
    pub pass: &'static str,
    pub stage: u8,
    pub done: bool,
}

/// What one build recorded.
#[derive(Default, Debug)]
pub struct Remarks {
    pub spills: Vec<SpillRemark>,
    pub loops: Vec<LoopRemark>,
}

thread_local! {
    static SINK: RefCell<Option<Remarks>> = const { RefCell::new(None) };
}

/// Collecting for one build on this thread; dropping it without `take` stops collecting.
pub struct Collect(());

impl Drop for Collect {
    fn drop(&mut self) {
        SINK.with(|s| *s.borrow_mut() = None);
    }
}

/// Starts collecting remarks for the build that follows on this thread (its optimisation and
/// its code generation).
pub fn begin() -> Collect {
    SINK.with(|s| *s.borrow_mut() = Some(Remarks::default()));
    Collect(())
}

/// Stops collecting and returns what the build recorded.
pub fn take(c: Collect) -> Remarks {
    let r = SINK.with(|s| s.borrow_mut().take()).unwrap_or_default();
    drop(c);
    r
}

fn on() -> bool {
    SINK.with(|s| s.borrow().is_some())
}

/// A vectoriser starts on the loop whose head is at `pos` (a later look at the same loop by the
/// same pass replaces the earlier record).
pub fn loop_try(pass: &'static str, pos: u64) {
    if !on() {
        return;
    }
    SINK.with(|s| {
        if let Some(r) = s.borrow_mut().as_mut() {
            r.loops.retain(|l| !(l.pass == pass && l.pos == pos));
            r.loops.push(LoopRemark { pos, pass, stage: 0, done: false });
        }
    });
}

/// The current loop passed check `stage`.
pub fn loop_stage(stage: u8) {
    if !on() {
        return;
    }
    SINK.with(|s| {
        if let Some(l) = s.borrow_mut().as_mut().and_then(|r| r.loops.last_mut()) {
            l.stage = l.stage.max(stage);
        }
    });
}

/// The current loop was vectorised.
pub fn loop_done() {
    if !on() {
        return;
    }
    SINK.with(|s| {
        if let Some(l) = s.borrow_mut().as_mut().and_then(|r| r.loops.last_mut()) {
            l.done = true;
        }
    });
}

impl LoopRemark {
    /// The pass and, when it gave up, the first check the loop failed.
    pub fn describe(&self) -> String {
        if self.done {
            return format!("{}: vectorised", self.pass);
        }
        let why = match (self.pass, self.stage) {
            ("stream-map", 0) => "not a single linear cycle (one entry, guards that exit, the back edge after the store) with one definition per value",
            ("stream-map", 1) => "the body has an operation or address it does not widen (u8 / f32 / i32 element loads and one store at stream positions, lane-wise ops, loop-invariant scalars)",
            ("stream-map", 2) => "a stream does not step one element per iteration",
            ("stream-map", 3) => "the exit tests are not a down-counting length, a pointer or counter reaching an end, a counter below a bound or a null test",
            ("stream-map", 4) => "a value computed in the loop is used after it, or the store overlaps a load within one vector",
            ("reduction", 0) => "not a single-block counted loop",
            ("reduction", 1) => "no integer sum over a contiguous array in it",
            ("lane-accumulators", 0) => "no group of independent accumulators of one array",
            _ => "a later check failed",
        };
        format!("{}: not vectorised: {}", self.pass, why)
    }
}

/// The finished allocation `a` of `f`: its spilled values (called by the emitters).
pub fn note_spills(f: &Func, a: &Alloc) {
    if !on() {
        return;
    }
    // the position -> (block, instruction) map and the call positions
    let nb = f.blocks.len().min(a.block_start.len());
    let mut calls: Vec<u32> = Vec::new();
    for bi in 0..nb {
        for (ii, i) in f.blocks[bi].insts.iter().enumerate() {
            if matches!(i, Inst::Call(..)) {
                calls.push(a.block_start[bi] + 2 * ii as u32 + 1);
            }
        }
    }
    calls.sort_unstable();
    let src = |p: u32| -> u64 {
        let k = a.block_start[..nb].partition_point(|&s| s <= p);
        if k == 0 {
            return 0;
        }
        let bi = k - 1;
        let ii = (p.saturating_sub(a.block_start[bi] + 1) / 2) as usize;
        f.blocks[bi].pos.get(ii).copied().unwrap_or(f.blocks[bi].term_pos)
    };
    let is_flt = |v: usize| !matches!(f.vregs.get(v), Some(Cls::I) | None);
    let mut v = Vec::new();
    for (k, l) in a.loc.iter().enumerate() {
        if !matches!(l, Loc::Stack(_)) || k >= a.start.len() || k >= a.end.len() {
            continue;
        }
        let (s, e) = (a.start[k], a.end[k]);
        if e <= s {
            continue;
        }
        let flt = is_flt(k);
        let n_calls = (calls.partition_point(|&c| c < e) - calls.partition_point(|&c| c < s)) as u32;
        let pressure = a.loc.iter().enumerate().filter(|(u, lu)| {
            let u = *u;
            u < a.start.len() && u < a.end.len() && a.start[u] <= s && s < a.end[u] && match lu {
                Loc::Reg(_) => !flt,
                Loc::Flt(_) => flt,
                _ => false,
            }
        }).count() as u32;
        v.push(SpillRemark { pos: src(s), flt, uses: a.uses.get(k).copied().unwrap_or(0), calls: n_calls, pressure, span: e - s });
    }
    SINK.with(|s| {
        if let Some(sink) = s.borrow_mut().as_mut() {
            sink.spills.extend(v);
        }
    });
}

impl SpillRemark {
    /// The likely reason in words: a range across calls (no caller-saved register survives them)
    /// or register pressure where it starts.
    pub fn describe(&self, regs: u32) -> String {
        let what = if self.flt { "float/vector" } else { "integer" };
        let why = if self.calls > 0 && self.pressure + 1 < regs {
            format!("its range spans {} call(s): kept in a stack slot rather than a callee-saved register", self.calls)
        } else if self.pressure + 1 >= regs {
            format!("register pressure: {} {} values held registers where it starts ({} usable)", self.pressure, what, regs)
        } else {
            "spilled with registers free where it starts (a split/heuristic choice of the allocator)".to_string()
        };
        format!("{} value used {}x over {} positions, {} call(s) inside; {}", what, self.uses, self.span / 2, self.calls, why)
    }
}
