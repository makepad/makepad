//! Cheap, target-independent O1 optimisations on RIR. Every pass is linear
//! (or a few linear sweeps) in the function size:
//! - inlining of tiny leaf callees (bottom-up: callees are already optimised),
//! - constant folding incl. loads from immutable data (const arrays, literals),
//! - copy propagation for single-definition vregs,
//! - immediate operand forms, branch folding, jump threading, dead code removal,
//! - block re-layout so fall-through follows the hot edge.

use crate::regalloc::{for_defs, for_uses, reachable, term_uses, uses_defs};
use std::cell::Cell;

/// CPU ns per pass (all threads, MAKEPAD_RUN_PASS_TIMES): 0 fold/copy-prop, 1 cse, 2 local copy-prop,
/// 3 dce, 4 jumps+merge, 5 inline, 6 sink, 7 layout, 8 sroa, 9 live_dce (top level), 10 licm,
/// 11 regalloc, 12 emit, 13 verify, 14 compact, 15 thread_known
pub static PASS_NS: [std::sync::atomic::AtomicU64; 16] = [const { std::sync::atomic::AtomicU64::new(0) }; 16];

/// Per-pass timing is opt-in (MAKEPAD_RUN_PASS_TIMES): the thread CPU clock is a syscall.
/// MAKEPAD_RUN_OPT diagnostic toggles beyond OptCfg's fields: `-freecap` (no size allowance for
/// O3 free inlines: R13's 1.9 MB functions, to exercise arm64 branch relaxation),
/// `-costmodel` (rustc MIR cost admission); `+single` turns the single-caller admission on;
/// `-ic` disables the InstCombine subset (nonnull, phi compares, convs, niches, sinking).
pub fn opt_off(options: &relax_session::Options, tok: &str) -> bool {
    options.opt.iter().any(|t| t.trim() == tok)
}

pub fn pass_clock() -> u64 {
    if PASS_ON.with(|c| c.get()) {
        if fast_clock() && !crate::fncost::enabled() {
            // monotonic (vDSO) clock: per-pass timing of tiny functions without the cost
            // of the thread-CPU syscall dominating them
            static T0: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
            T0.get_or_init(std::time::Instant::now).elapsed().as_nanos() as u64 + 1
        } else {
            crate::cpu_ns()
        }
    } else {
        0
    }
}

fn fast_clock() -> bool {
    static F: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *F.get_or_init(|| std::env::var("MAKEPAD_RUN_PASS_TIMES").map_or(false, |v| v == "wall"))
}

/// MAKEPAD_RUN_OPT=-ifconv: no if-conversion (bisection).
fn if_conv_on(options: &relax_session::Options) -> bool {
    !options.opt.iter().any(|p| p == "-ifconv")
}

/// MAKEPAD_RUN_OPT=-thread: no jump threading over known values (bisection).
fn thread_on(options: &relax_session::Options) -> bool {
    !options.opt.iter().any(|p| p == "-thread")
}

thread_local! {
    static PASS_ON: Cell<bool> = Cell::new(std::env::var("MAKEPAD_RUN_PASS_TIMES").is_ok());
    /// this thread's ns per pass (pass_local: one build's split)
    static PASS_LOCAL: Cell<[u64; 16]> = const { Cell::new([0; 16]) };
}

/// Per-pass timing on this thread from now on (a O3 trace times each build's passes).
pub fn pass_timing_on() {
    PASS_ON.with(|c| c.set(true));
}

/// This thread's ns per pass so far (PASS_NAMES order); a build's split is the difference.
pub fn pass_local() -> [u64; 16] {
    PASS_LOCAL.with(|c| c.get())
}

pub fn pass_time(i: usize, t0: u64) {
    if t0 == 0 {
        return;
    }
    let now = pass_clock();
    PASS_NS[i].fetch_add(now - t0, std::sync::atomic::Ordering::Relaxed);
    PASS_LOCAL.with(|c| {
        let mut a = c.get();
        a[i] += now - t0;
        c.set(a);
    });
}

/// MAKEPAD_RUN_OPT_HIST: opt-pipeline cpu ns (0..6) and function counts (6..12) by optimised size
pub static HIST: [std::sync::atomic::AtomicU64; 12] = [const { std::sync::atomic::AtomicU64::new(0) }; 12];
pub const PASS_NAMES: [&str; 16] = ["fold", "cse", "lcopy", "dce", "jumps", "inline", "sink", "layout", "sroa", "live_dce", "licm", "regalloc", "emit", "verify", "compact", "thread_known"];
use crate::rir::*;

#[derive(Clone, Copy)]
pub struct OptCfg {
    /// O1: inline Relax core's builtins (scalar/math/slice helpers) into callers
    pub inline: bool,
    /// also inline small non-recursive leaf functions of user code (budget-based, O1)
    pub inline_all: bool,
    pub fold: bool,
    pub cse: bool,
    pub sroa: bool,
    pub licm: bool,
    pub imm: bool,
    pub layout: bool,
}

impl OptCfg {
    /// `MAKEPAD_RUN_OPT=none` or a comma list of passes to disable (`-inline,-fold`).
    pub fn from_options(options: &relax_session::Options) -> OptCfg {
        let mut c = OptCfg { inline: true, inline_all: true, fold: true, cse: true, sroa: true, licm: true, imm: true, layout: true };
        {
            let v = &options.opt;
            if v.len() == 1 && v[0] == "none" {
                c = OptCfg { inline: false, inline_all: false, fold: false, cse: false, sroa: false, licm: false, imm: false, layout: false };
            }
            for part in v {
                match part.as_str() {
                    "-inline" => c.inline = false,
                    "-fold" => c.fold = false,
                    "-cse" => c.cse = false,
                    "-sroa" => c.sroa = false,
                    "-licm" => c.licm = false,
                    "-imm" => c.imm = false,
                    "-layout" => c.layout = false,
                    "+inline_all" => c.inline_all = true,
                    "-inline_all" => c.inline_all = false,
                    _ => {}
                }
            }
        }
        c
    }
}

pub const INLINE_MAX_INSTS: usize = 40;
/// callees this small inline outside the per-caller budget
pub const INLINE_FREE_INSTS: usize = 24;
/// size limit for #[inline] callees (leaf or not)
pub const INLINE_HINT_INSTS: usize = 60;
pub const INLINE_MAX_BLOCKS: usize = 8;
pub const INLINE_BUDGET: usize = 600;

/// Leaf-inlining budget (insts, blocks); MAKEPAD_RUN_INLINE_LEAF=insts,blocks overrides.
pub fn leaf_budget(options: &relax_session::Options) -> (usize, usize) {
    options.inline_leaf
}

/// Inlining rules (the first four also apply at O1; counted per rule in the stats):
pub const RULE_TINY: usize = 0; // body no bigger than the call sequence it replaces
pub const RULE_ALWAYS: usize = 1; // #[inline(always)], or #[inline] and small
pub const RULE_SMALL_LEAF: usize = 2; // small leaf under the size budget
pub const RULE_CHAIN: usize = 3; // core instance made for a closure (fused iterator chains)
pub const RULE_COST: usize = 4; // rustc's MIR cost model, at every non-cold site
pub const RULE_SINGLE: usize = 5; // LLVM's last call to a local function
pub const RULE_NAMES: [&str; 6] = ["tiny(<=call)", "inline-attr", "small-leaf", "closure-chain", "rustc-cost", "single-caller"];
pub const ATTR_ALWAYS: u8 = 1;
pub const ATTR_HINT: u8 = 2;
/// a core instance made for a closure or fn item (iterator adapter, Option/Result combinator)
pub const ATTR_CHAIN: u8 = 4;
/// the caller knows this callee is cold (a JIT: it has never run, so its call sits on a path
/// the program has not taken): it stays a call unless tiny or admitted by the cost model
pub const ATTR_COLD: u8 = 8;
/// size limit of rule 4 (fused chains; the callee may call: its callees were inlined bottom-up)
pub const INLINE_CHAIN_INSTS: usize = 120;
pub const INLINE_SINGLE_INSTS: usize = 3000;
const HISTORY_DEPTH_LIMIT: usize = 20;
const TOP_DOWN_DEPTH_LIMIT: usize = 5;

fn inline_call_like(inst: &Inst) -> bool {
    matches!(inst, Inst::Call(..)) || matches!(inst, Inst::Copy(_, _, n) if i64::from(*n) > INLINE_COPY_MAX)
}

fn inline_call_count(f: &Func) -> usize {
    f.blocks.iter().flat_map(|b| &b.insts).filter(|i| inline_call_like(i)).count()
}

/// rustc_mir_transform's CostChecker on RIR. Calls replace the ordinary instruction
/// charge; operands (moves, constants and addresses) are free. Recursion is checked at
/// the site, independently of cost. RIR has no switch terminator.
pub fn rustc_inline_cost(callee: &Func, _callee_id: u32) -> usize {
    const INSTR_COST: usize = 5;
    const CALL_PENALTY: usize = 25;
    const LANDINGPAD_PENALTY: usize = 50;
    const RESUME_PENALTY: usize = 45;
    if callee.blocks.is_empty() { return 0; }
    let mut penalty: usize = 0;
    // As in rustc, this is a body-wide count, not a bonus per call/block.
    let mut bonus = if inline_call_count(callee) == 1 { CALL_PENALTY } else { 0 };
    for (bi, live) in reachable(callee).into_iter().enumerate() {
        if !live { continue; }
        let block = &callee.blocks[bi];
        for (ii, inst) in block.insts.iter().enumerate() {
            penalty += match inst {
                Inst::Mov(..) | Inst::Iconst(..) | Inst::Fconst(..) | Inst::SlotAddr(..) | Inst::Addr(..) | Inst::FnAddr(..) => 0,
                i if inline_call_like(i) => CALL_PENALTY,
                _ => INSTR_COST,
            };
            // Only the last call is invoked; earlier calls in the block have no
            // cleanup edge (Term::Invoke's RIR invariant).
            if matches!(inst, Inst::Call(..)) && ii + 1 == block.insts.len() && matches!(block.term, Term::Invoke(..)) {
                penalty += LANDINGPAD_PENALTY;
            }
        }
        match block.term {
            Term::Branch(..) => penalty += INSTR_COST,
            Term::Resume => penalty += RESUME_PENALTY,
            Term::Unreachable => bonus += INSTR_COST,
            Term::Jump(..) | Term::Ret(..) | Term::Invoke(..) => {}
        }
    }
    penalty.saturating_sub(bonus)
}

/// LLVM's single-caller rule is opt-in (`MAKEPAD_RUN_OPT=+single`) until html_parse's regression and
/// its O3 compile cost are fixed.
pub fn single_on(options: &relax_session::Options) -> bool {
    options.opt.iter().any(|t| t.trim() == "+single")
}

pub fn rustc_inline_threshold(callee: &Func, attrs: u8) -> usize {
    let threshold = if attrs & (ATTR_HINT | ATTR_ALWAYS) != 0 { 100 } else { 50 };
    threshold + if callee.blocks.len() <= 3 { threshold / 4 } else { 0 }
}

fn callee_shape(f: &Func, callee_id: u32) -> (usize, bool, bool) {
    // (instruction count, leaf, recursive)
    let mut n = 0;
    let mut leaf = true;
    let mut rec = false;
    for b in &f.blocks {
        n += b.insts.len();
        // a call on a path that ends in Unreachable (a panic: unwrap's failure, a bounds check)
        // never returns: it does not make the callee a non-leaf
        let cold = matches!(b.term, Term::Unreachable | Term::Resume);
        for i in &b.insts {
            match i {
                Inst::Call(..) if cold => {}
                Inst::Call(Callee::Host(_), _, _) | Inst::Call(Callee::CHost(..), _, _) => {}
                Inst::Call(Callee::Fn(c), _, _) => {
                    leaf = false;
                    if *c == callee_id {
                        rec = true;
                    }
                }
                Inst::Call(..) => leaf = false,
                _ => {}
            }
        }
    }
    (n, leaf, rec)
}

/// Which rule (if any) inlines this call.
/// Size limits of the inlining rules per tier (instructions of the callee's optimised RIR).
#[derive(Clone, Copy)]
pub struct InlineLimits {
    /// Instruction allowance (net growth at O3, raw callee size at O1).
    pub budget: usize,
    /// small-leaf rule
    pub leaf_insts: usize,
    pub leaf_blocks: usize,
    /// #[inline] (and Relax core at O3)
    pub hint_insts: usize,
    /// rule 4: core instances made for a closure / fn item
    pub chain_insts: usize,
    /// #[inline(always)]
    pub always_insts: usize,
    /// any callee up to INLINE_FREE_INSTS instructions inlines (attributes or not)
    pub free_any: bool,
    /// O3 site priority and net-growth accounting.
    pub ordered: bool,
    pub hot_insts: usize,
    /// Original caller growth ceiling, shared by both O3 rounds.
    pub growth_limit: Option<usize>,
}

impl InlineLimits {
    /// O1 ("obviously inline"); MAKEPAD_RUN_INLINE_LEAF=insts,blocks overrides the leaf rule
    pub fn tier0(options: &relax_session::Options) -> InlineLimits {
        let (leaf_insts, leaf_blocks) = leaf_budget(options);
        InlineLimits { budget: INLINE_BUDGET, leaf_insts, leaf_blocks, hint_insts: INLINE_HINT_INSTS, chain_insts: INLINE_CHAIN_INSTS, always_insts: 200, free_any: false, ordered: false, hot_insts: 0, growth_limit: None }
    }
    /// O1 (startup code, no opt pipeline): only Relax core callees whose optimised
    /// body is about as small as the call that reaches them (accessors, index, len, next, the
    /// checked/wrapping integer helpers); jit.rs base_inline_wanted picks the candidates
    pub fn baseline() -> InlineLimits {
        InlineLimits { budget: 400, leaf_insts: 16, leaf_blocks: 3, hint_insts: 24, chain_insts: 0, always_insts: 24, free_any: true, ordered: false, hot_insts: 0, growth_limit: None }
    }
    /// O3 (hot functions): rustc --release-like reach
    pub fn tier1() -> InlineLimits {
        InlineLimits { budget: 4000, leaf_insts: 150, leaf_blocks: 24, hint_insts: 250, chain_insts: 300, always_insts: 600, free_any: true, ordered: true, hot_insts: 80, growth_limit: None }
    }
}

pub fn inline_rule(f: &Func, callee_id: u32, nargs: usize, nrets: usize, attrs: u8, lim: &InlineLimits) -> Option<usize> {
    let (n, leaf, rec) = callee_shape(f, callee_id);
    if rec {
        return None;
    }
    // a callee that never returns (panic helpers) only runs on cold paths: keep it a call
    if !f.blocks.iter().any(|b| matches!(b.term, Term::Ret(_))) {
        return None;
    }
    if n <= nargs + nrets + 2 && f.blocks.len() <= 2 {
        return Some(RULE_TINY);
    }
    if attrs & ATTR_ALWAYS != 0 && n <= lim.always_insts {
        return Some(RULE_ALWAYS);
    }
    // #[inline] (and Relax core's small fns, which real std marks #[inline]): the callee may
    // call others (Vec::push's grow path)
    if attrs & ATTR_HINT != 0 && n <= lim.hint_insts {
        return Some(RULE_ALWAYS);
    }
    if attrs & ATTR_CHAIN != 0 && n <= lim.chain_insts {
        return Some(RULE_CHAIN);
    }
    if leaf && n <= lim.leaf_insts && f.blocks.len() <= lim.leaf_blocks {
        return Some(RULE_SMALL_LEAF);
    }
    if lim.free_any && n <= INLINE_FREE_INSTS {
        return Some(RULE_TINY);
    }
    None
}

/// Inlines calls chosen by `inline_rule`. `get(id)` returns a callee's optimised RIR and
/// its inline attributes. Returns (inlined callee slot, rule) per inlined call site.
pub fn inline_calls(options: &relax_session::Options, f: &mut Func, self_id: u32, get: &dyn Fn(u32) -> Option<(*const Func, u8)>, nounwind: &dyn Fn(u32) -> bool, lim: &InlineLimits) -> Vec<(u32, usize)> {
    if lim.ordered {
        return inline_calls_ordered(options, f, self_id, get, &|_, _| false, nounwind, lim, None, None);
    }
    let mut inlined = Vec::new();
    let mut budget = lim.budget;
    // free inlines have their own allowance of the same size: thousands of tiny calls in one
    // big function (a parser's match) otherwise grew it without bound (R13: 246k insts, 1.9 MB
    // of arm64 code whose conditional branches no longer reached their targets)
    let mut free_budget = if opt_off(options, "-freecap") { usize::MAX } else { lim.budget };
    let mut bi = 0;
    while bi < f.blocks.len() {
        let mut ii = 0;
        // a block that ends in Unreachable is a panic path, one that ends in Resume a cleanup
        // pad: their calls stay calls
        let cold = matches!(f.blocks[bi].term, Term::Unreachable | Term::Resume);
        while ii < f.blocks[bi].insts.len() && !cold {
            let (id, args, rets) = match &f.blocks[bi].insts[ii] {
                Inst::Call(Callee::Fn(id), a, r) if *id != self_id => (*id, a, r),
                _ => {
                    ii += 1;
                    continue;
                }
            };
            let (cp, attrs) = match get(id) {
                Some(p) => p,
                None => {
                    ii += 1;
                    continue;
                }
            };
            let callee: &Func = unsafe { &*cp };
            let mut size = 0;
            for b in &callee.blocks {
                size += b.insts.len();
            }
            let rule = inline_rule(callee, id, args.len(), rets.len(), attrs, lim);
            // callees about as small as a call's own argument moves are free: the budget limits
            // growth, and inlining them shrinks the caller (LLVM's inline cost for a callee
            // below the call penalty); a big caller would otherwise keep its tiny calls
            let free = size <= INLINE_FREE_INSTS && size <= free_budget;
            let rule = if attrs & ATTR_COLD != 0 && !free { None } else { rule };
            let rule = match rule {
                Some(r) if (free || size <= budget) && callee.params.len() == args.len() && callee.rets.len() == rets.len() => r,
                _ => {
                    ii += 1;
                    continue;
                }
            };
            if free {
                free_budget -= size;
            } else {
                budget -= size;
            }
            inlined.push((id, rule));
            // Most baseline callees stay calls. Copy operands only after admission,
            // when rewriting the caller actually needs to own them.
            let (args, rets) = (args.clone(), rets.clone());
            inline_site(f, bi, ii, callee, &args, &rets, nounwind);
            break;
        }
        bi += 1;
    }
    inlined
}

/// Fraction of the hottest block required for the hot-site rule.
pub const HOT_FREQ: f32 = 0.05;

fn inline_inst_count(f: &Func) -> usize {
    f.blocks.iter().map(|b| b.insts.len()).sum()
}

/// Optional diagnostics. Rejections describe the last inline round; successes span both.
/// Hot sites count inlined sites plus remaining hot calls, without recounting queue tails.
/// Growth spans the first round's input through the last round's output.
#[derive(Default)]
pub struct InlineStats {
    pub inlined: usize,
    pub rules: [usize; RULE_NAMES.len()],
    hot_inlined: usize,
    hot_left: usize,
    growth_before: Option<usize>,
    growth_after: usize,
    pub left: [usize; 5],
    pub rejected: std::collections::BTreeMap<(String, &'static str), usize>,
}

impl InlineStats {
    fn reject(&mut self, name: String, reason: usize) {
        const REASONS: [&str; 5] = ["cold", "no-rir", "no-rule", "budget", "arity"];
        self.left[reason] += 1;
        *self.rejected.entry((name, REASONS[reason])).or_default() += 1;
    }

    pub fn report(&self, name: &str) {
        let mut top: Vec<_> = self.rejected.iter().collect();
        top.sort_by_key(|(key, count)| (std::cmp::Reverse(**count), *key));
        let top = top.into_iter().enumerate()
            .filter(|(i, ((_, why), _))| *i < 5 || *why == "budget")
            .map(|(_, ((name, reason), n))| format!("{} ({}, {})", name, reason, n))
            .collect::<Vec<_>>().join("; ");
        eprintln!("inline-stats {}: sites {}, inlined {}, hot {}, growth {}->{} insts, left: cold {}, no-rir {}, no-rule {}, budget {}, arity {}; top: {}",
            name, self.inlined + self.left.iter().sum::<usize>(), self.inlined,
            self.hot_inlined + self.hot_left, self.growth_before.unwrap_or(0), self.growth_after,
            self.left[0], self.left[1], self.left[2], self.left[3], self.left[4], top);
        eprintln!("  rules: {}", RULE_NAMES.iter().zip(self.rules).map(|(rule, n)| format!("{} {}", rule, n)).collect::<Vec<_>>().join(", "));
    }
}

impl InlineLimits {
    pub fn tier1_for(f: &Func) -> Self {
        let mut lim = Self::tier1();
        let size = inline_inst_count(f);
        lim.growth_limit = Some(size.saturating_mul(2).max(size.saturating_add(4000)));
        lim
    }
}

fn site_rule(options: &relax_session::Options, callee: &Func, id: u32, nargs: usize, nrets: usize, attrs: u8, hot: bool, unsampled: bool, single: bool, depth: usize, multi_depth: usize, lim: &InlineLimits) -> Option<usize> {
    // Unlike the leaf heuristic, recursion on a panic path still forbids inlining.
    if callee.blocks.iter().any(|b| b.insts.iter().any(|i| matches!(i, Inst::Call(Callee::Fn(c), _, _) if *c == id)))
        || !callee.blocks.iter().any(|b| matches!(b.term, Term::Ret(_))) {
        return None;
    }
    if single && single_on(options) && attrs & ATTR_COLD == 0 && inline_inst_count(callee) <= INLINE_SINGLE_INSTS {
        return Some(RULE_SINGLE);
    }
    if !opt_off(options, "-costmodel") && depth < HISTORY_DEPTH_LIMIT {
        let past_depth = multi_depth > TOP_DOWN_DEPTH_LIMIT;
        let threshold = if past_depth { 30 } else { rustc_inline_threshold(callee, attrs) };
        if (!past_depth || callee.blocks.len() == 1) && rustc_inline_cost(callee, id) <= threshold {
            return Some(RULE_COST);
        }
    }
    // The existing size heuristics admit only tiny bodies or inline(always) at
    // unvisited sites; the two additional admissions above do not need samples.
    if unsampled && inline_inst_count(callee) > INLINE_FREE_INSTS {
        return (attrs & ATTR_ALWAYS != 0 && inline_inst_count(callee) <= lim.always_insts).then_some(RULE_ALWAYS);
    }
    // Keep ordinary low-priority sites inlinable, but reserve expansive
    // hinted bodies for hot sites and closure chains. This bound also applies
    // before samples exist, preventing structural builds from spending their
    // growth allowance on large peripheral helpers.
    let mut lim = *lim;
    if !hot && attrs & ATTR_CHAIN == 0 { lim.hint_insts = lim.hint_insts.min(120); }
    inline_rule(callee, id, nargs, nrets, attrs, &lim).or_else(|| {
        (hot && inline_inst_count(callee) <= lim.hot_insts).then_some(RULE_SMALL_LEAF)
    })
}

/// Frequencies must describe this exact RIR. New blocks inherit the site's priority.
/// The queue uses stable block/instruction indices: splitting invalidates only the tail,
/// whose calls are enqueued again in the new continuation block.
/// `single(caller, callee)` proves that caller's O1 body owns the sole reference.
pub fn inline_calls_ordered(options: &relax_session::Options, f: &mut Func, self_id: u32, get: &dyn Fn(u32) -> Option<(*const Func, u8)>, single: &dyn Fn(u32, u32) -> bool, nounwind: &dyn Fn(u32) -> bool, lim: &InlineLimits, freqs: Option<&[f32]>, mut stats: Option<&mut InlineStats>) -> Vec<(u32, usize)> {
    use std::cmp::Reverse;
    use std::collections::BinaryHeap;
    let depths = crate::regalloc::loop_depths(f, &reachable(f));
    let freqs = freqs.filter(|v| v.len() == f.blocks.len());
    let mut priorities: Vec<(u32, bool, bool)> = depths.iter().enumerate().map(|(b, d)| {
        freqs.map_or((*d, *d >= 1 && f.blocks.len() <= 64, false), |v| (v[b].max(0.0).to_bits(), v[b] >= HOT_FREQ, v[b] == 0.0))
    }).collect();
    let mut loop_sites: Vec<bool> = depths.iter().map(|d| *d > 0).collect();
    let mut ancestry = vec![vec![self_id]; f.blocks.len()];
    let mut multi_depths = vec![0; f.blocks.len()];
    let mut queue = BinaryHeap::new();
    let mut budget = lim.budget;
    let mut inst_count = inline_inst_count(f);
    let mut growth_limit = lim.growth_limit.unwrap_or_else(|| inst_count.saturating_mul(2).max(inst_count.saturating_add(4000)));
    // Keep the separate raw-size free allowance.
    let mut free_budget = if opt_off(options, "-freecap") { usize::MAX } else { InlineLimits::tier1().budget };
    let mut inlined = Vec::new();
    if let Some(s) = stats.as_deref_mut() {
        s.growth_before.get_or_insert(inst_count);
        s.hot_left = 0;
        s.left = [0; 5];
        s.rejected.clear();
    }
    // Scalar closure combinators do not carry a per-element call penalty.
    // Look through adapter forwarding bodies for an actual loop before giving
    // a chain priority over equally hot ordinary calls.
    fn loop_chain(id: u32, get: &dyn Fn(u32) -> Option<(*const Func, u8)>) -> bool {
        let mut pending = vec![id];
        let mut seen = std::collections::HashSet::new();
        while let Some(id) = pending.pop() {
            if !seen.insert(id) { continue; }
            let Some((p, attrs)) = get(id) else { continue };
            if attrs & ATTR_CHAIN == 0 { continue; }
            let f = unsafe { &*p };
            if crate::regalloc::loop_depths(f, &reachable(f)).iter().any(|d| *d != 0) { return true; }
            for b in &f.blocks {
                for i in &b.insts {
                    if let Inst::Call(Callee::Fn(c), _, _) = i { pending.push(*c); }
                }
            }
        }
        false
    }
    let mut chains = std::collections::HashMap::new();
    let mut enqueue = |f: &Func, start: usize, priorities: &[(u32, bool, bool)], loop_sites: &[bool], queue: &mut BinaryHeap<_>| {
        for bi in start..f.blocks.len() {
            if matches!(f.blocks[bi].term, Term::Unreachable | Term::Resume) { continue; }
            for (ii, inst) in f.blocks[bi].insts.iter().enumerate() {
                if let Inst::Call(Callee::Fn(id), _, _) = inst {
                    let found = get(*id);
                    let size = found.map_or(usize::MAX, |(p, _)| inline_inst_count(unsafe { &*p }));
                    // Closure adapters need to be fused as a chain. Importing
                    // their loop after unrelated work has spent the budget
                    // leaves an indirect/aggregate call on every iteration.
                    let chain = *chains.entry(*id).or_insert_with(|| {
                        loop_chain(*id, get) || found.map_or(false, |(p, _)| unsafe { &*p }.blocks.iter().any(|b| {
                            b.insts.iter().any(|i| match i {
                                Inst::Call(Callee::Fn(c), _, _) => loop_chain(*c, get),
                                _ => false,
                            })
                        }))
                    });
                    // Once the loop is exposed, its per-element adapters
                    // remain part of the chain even though next() has no loop.
                    let chain = chain || (loop_sites[bi] && found.map_or(false, |(_, a)| a & ATTR_CHAIN != 0));
                    // A straight-line adapter can introduce the hottest loop in the
                    // caller. Complete that chain before spending growth on
                    // ordinary calls, including on the structural first build.
                    queue.push((chain, priorities[bi].0, Reverse(size), Reverse(bi), Reverse(ii)));
                }
            }
        }
    };
    enqueue(f, 0, &priorities, &loop_sites, &mut queue);
    while let Some((_, _, Reverse(size), Reverse(bi), Reverse(ii))) = queue.pop() {
        let Some(Inst::Call(Callee::Fn(id), args, rets)) = f.blocks[bi].insts.get(ii) else { continue };
        let (id, args, rets) = (*id, args.clone(), rets.clone());
        let found = get(id);
        let callee = found.map(|(p, _)| unsafe { &*p });
        let cost = size.saturating_sub(args.len() + rets.len() + 3);
        // Preserve the raw-size R13 free cap; the paid budget charges net growth.
        let free = size <= INLINE_FREE_INSTS && size <= free_budget;
        // A tiny body (no bigger than the call's own argument/return moves, or within the free
        // size) is admitted whatever the caller has grown to: refusing it keeps a call that costs
        // more than the body (sandbox leap, node 165: encode_cascades left core::mem::drop x130,
        // rc::header_of x140, slice::iter::next x26 as calls -- "budget" -- once big chains had
        // filled the growth ceiling). Only the raw free allowance is charged for it.
        let tiny = size <= INLINE_FREE_INSTS;
        // ... and one no bigger than the call itself costs nothing: no allowance at all.
        let no_growth = cost == 0;
        let rule = found.and_then(|(_, attrs)| site_rule(options, callee.unwrap(), id, args.len(), rets.len(), attrs, priorities[bi].1, priorities[bi].2, single(*ancestry[bi].last().unwrap(), id), ancestry[bi].len() - 1, multi_depths[bi], lim));
        // New admissions respect the actual growth ceiling, including ABI moves.
        let growth = callee.map_or(0, |c| size.saturating_add(args.len()).saturating_add(
            c.blocks.iter().filter(|b| matches!(b.term, Term::Ret(_))).count().saturating_mul(rets.len())
        ).saturating_sub(1));
        let reason = if matches!(f.blocks[bi].term, Term::Unreachable | Term::Resume) {
            Some(0)
        } else if callee.is_none() {
            Some(1)
        } else if ancestry[bi].contains(&id) || rule.is_none() {
            Some(2)
        } else if found.unwrap().1 & ATTR_COLD != 0 && !free && rule != Some(RULE_COST) {
            Some(0)
        } else if callee.unwrap().params.len() != args.len() || callee.unwrap().rets.len() != rets.len() {
            Some(4)
        } else if match rule.unwrap() {
            _ if no_growth => false,
            RULE_COST => size > free_budget || !tiny && inst_count.saturating_add(growth) > growth_limit,
            RULE_SINGLE => inst_count.saturating_add(growth) > growth_limit,
            _ => !free && (cost > budget || !tiny && inst_count > growth_limit),
        } {
            Some(3)
        } else {
            None
        };
        if reason.is_some() {
            continue;
        }
        match rule.unwrap() {
            // the body moves here (its one caller): the ceiling grows with it, so the calls
            // around it (Vec::push in the absorbed loop) still fit
            RULE_SINGLE => growth_limit = growth_limit.saturating_add(growth),
            _ if no_growth => {}
            RULE_COST => free_budget -= size,
            _ if free => free_budget -= size,
            _ => budget -= cost,
        }
        inlined.push((id, rule.unwrap()));
        if let Some(s) = stats.as_deref_mut() {
            s.inlined += 1;
            s.rules[rule.unwrap()] += 1;
            s.hot_inlined += usize::from(priorities[bi].1);
        }
        let start = f.blocks.len();
        let old_block_size = f.blocks[bi].insts.len();
        inline_site(f, bi, ii, callee.unwrap(), &args, &rets, nounwind);
        inst_count = inst_count - old_block_size + f.blocks[bi].insts.len()
            + f.blocks[start..].iter().map(|b| b.insts.len()).sum::<usize>();
        priorities.resize(f.blocks.len(), priorities[bi]);
        let parent_loop = loop_sites[bi];
        loop_sites.resize(f.blocks.len(), parent_loop);
        if !parent_loop {
            let cf = callee.unwrap();
            for (cb, depth) in crate::regalloc::loop_depths(cf, &reachable(cf)).iter().enumerate() {
                loop_sites[start + 1 + cb] = *depth > 0;
            }
        }
        let parent = ancestry[bi].clone();
        ancestry.resize(f.blocks.len(), parent);
        let multi_depth = multi_depths[bi];
        multi_depths.resize(f.blocks.len(), multi_depth);
        // The continuation belongs to the caller, all other new blocks to the callee.
        for chain in &mut ancestry[start + 1..] { chain.push(id); }
        if inline_call_count(callee.unwrap()) > 1 {
            for depth in &mut multi_depths[start + 1..] { *depth += 1; }
        }
        enqueue(f, start, &priorities, &loop_sites, &mut queue);
    }
    if let Some(s) = stats.as_deref_mut() {
        s.growth_after = inst_count;
        for (bi, block) in f.blocks.iter().enumerate() {
            for inst in &block.insts {
                let Inst::Call(Callee::Fn(id), args, rets) = inst else { continue };
                s.hot_left += usize::from(priorities[bi].1 && !matches!(block.term, Term::Unreachable | Term::Resume));
                let found = get(*id);
                let callee = found.map(|(p, _)| unsafe { &*p });
                let reason = if matches!(block.term, Term::Unreachable | Term::Resume) {
                    0
                } else if let Some((_, attrs)) = found {
                    let c = callee.unwrap();
                    let rule = site_rule(options, c, *id, args.len(), rets.len(), attrs, priorities[bi].1, priorities[bi].2, single(*ancestry[bi].last().unwrap(), *id), ancestry[bi].len() - 1, multi_depths[bi], lim);
                    if ancestry[bi].contains(id) || rule.is_none() {
                        2
                    } else if attrs & ATTR_COLD != 0 && rule != Some(RULE_COST) && !(inline_inst_count(c) <= INLINE_FREE_INSTS && inline_inst_count(c) <= free_budget) {
                        0
                    } else if c.params.len() != args.len() || c.rets.len() != rets.len() {
                        4
                    } else {
                        3
                    }
                } else {
                    1
                };
                s.reject(callee.map_or_else(|| format!("fn#{}", id), |c| c.name.clone()), reason);
            }
        }
    }
    inlined
}


fn inline_site(f: &mut Func, bi: usize, ii: usize, callee: &Func, args: &[VReg], rets: &[VReg], nounwind: &dyn Fn(u32) -> bool) {
    for c in &callee.sret_callees {
        if !f.sret_callees.contains(c) {
            f.sret_callees.push(*c);
        }
    }
    // split the block after the call
    let blk = &mut f.blocks[bi];
    let tail = blk.insts.split_off(ii + 1);
    let tail_pos = blk.pos.split_off(ii + 1);
    blk.insts.pop();
    let call_pos = blk.pos.pop().unwrap_or(0);
    let mut old_term = std::mem::replace(&mut blk.term, Term::Unreachable);
    let old_term_pos = blk.term_pos;
    // the invoked call itself (last in its block): its body continues at the normal
    // target, and whatever unwinds in it continues at the invoke's cleanup pad
    let mut unwind = None;
    if let Term::Invoke(n, u) = old_term {
        if tail.is_empty() {
            old_term = Term::Jump(n);
            unwind = Some(u);
        }
    }
    let cont = f.blocks.len() as u32;
    f.blocks.push(Block { insts: tail, pos: tail_pos, term: old_term, term_pos: old_term_pos });
    // map callee vregs and slots
    let mut vmap = Vec::with_capacity(callee.vregs.len());
    for c in &callee.vregs {
        vmap.push(f.vreg(*c));
    }
    for v in &callee.nonnull {
        f.nonnull.push(vmap[v.0 as usize]);
    }
    let slot_base = f.slots.len() as u32;
    // an inlined body's SlotAddrs were moved by its own optimisation: not `born` here
    f.slots.extend(callee.slots.iter().map(|s| Slot { born: false, ..s.clone() }));
    let boff = f.blocks.len() as u32;
    // parameter moves, then jump into the callee body
    for (k, p) in callee.params.iter().enumerate() {
        let d = vmap[p.0 as usize];
        f.blocks[bi].insts.push(Inst::Mov(d, args[k]));
        f.blocks[bi].pos.push(call_pos);
    }
    f.blocks[bi].term = Term::Jump(boff);
    f.blocks[bi].term_pos = call_pos;
    let m = |v: VReg| vmap[v.0 as usize];
    // calls that may unwind in the callee's blocks, when the callee is inlined at an
    // Invoke: each ends a block of its own whose Invoke continues at the caller's pad
    // (blocks split this way are appended after the callee's)
    let mut extra: Vec<Block> = Vec::new();
    let nextra_base = boff + callee.blocks.len() as u32;
    for cb in &callee.blocks {
        let mut insts = Vec::with_capacity(cb.insts.len());
        for x in &cb.insts {
            insts.push(map_inst(x, &m, slot_base));
        }
        let mut pos = cb.pos.clone();
        let mut term = match &cb.term {
            Term::Jump(t) => Term::Jump(t + boff),
            Term::Branch(c, t, e) => Term::Branch(m(*c), t + boff, e + boff),
            Term::Ret(vals) => {
                for (k, v) in vals.iter().enumerate() {
                    insts.push(Inst::Mov(rets[k], m(*v)));
                    pos.push(cb.term_pos);
                }
                Term::Jump(cont)
            }
            Term::Unreachable => Term::Unreachable,
            Term::Invoke(t, e) => Term::Invoke(t + boff, e + boff),
            // the callee's cleanup is done: unwinding goes on in the caller, at the
            // invoke's pad if the call was invoked
            Term::Resume => match unwind {
                Some(u) => Term::Jump(u),
                None => Term::Resume,
            },
        };
        let mut term_pos = cb.term_pos;
        if let Some(u) = unwind {
            // split after every call that may unwind and is not already invoked
            let last_invoked = matches!(cb.term, Term::Invoke(..));
            let n = insts.len();
            let mut cuts: Vec<usize> = Vec::new();
            for (k, x) in insts.iter().enumerate() {
                if let Inst::Call(c, _, _) = x {
                    if may_unwind(c, nounwind) && !(last_invoked && k + 1 == n) {
                        cuts.push(k);
                    }
                }
            }
            if !cuts.is_empty() {
                // pieces: [0..=c0], [c0+1..=c1], ..., [ck+1..n] with the original term
                let mut rest_i = insts;
                let mut rest_p = pos;
                let mut pieces: Vec<(Vec<Inst>, Vec<u64>)> = Vec::new();
                let mut taken = 0;
                for &c in &cuts {
                    let tail_i = rest_i.split_off(c + 1 - taken);
                    let tail_p = rest_p.split_off(c + 1 - taken);
                    pieces.push((std::mem::replace(&mut rest_i, tail_i), std::mem::replace(&mut rest_p, tail_p)));
                    taken = c + 1;
                }
                // the first piece stays this block; the others become extra blocks
                let first_extra = nextra_base + extra.len() as u32;
                let mut it = pieces.into_iter();
                let (i0, p0) = it.next().unwrap();
                insts = i0;
                pos = p0;
                let orig_term = std::mem::replace(&mut term, Term::Invoke(first_extra, u));
                let orig_pos = term_pos;
                term_pos = pos.last().copied().unwrap_or(orig_pos);
                for (k, (ik, pk)) in it.enumerate() {
                    let tp = pk.last().copied().unwrap_or(orig_pos);
                    extra.push(Block { insts: ik, pos: pk, term: Term::Invoke(first_extra + k as u32 + 1, u), term_pos: tp });
                }
                extra.push(Block { insts: rest_i, pos: rest_p, term: orig_term, term_pos: orig_pos });
            }
        }
        f.blocks.push(Block { insts, pos, term, term_pos });
    }
    f.blocks.extend(extra);
}

fn map_inst(x: &Inst, m: &dyn Fn(VReg) -> VReg, slot_base: u32) -> Inst {
    match x {
        Inst::Iconst(d, v) => Inst::Iconst(m(*d), *v),
        Inst::Fconst(d, v, w) => Inst::Fconst(m(*d), *v, *w),
        Inst::Mov(d, a) => Inst::Mov(m(*d), m(*a)),
        Inst::IBin(o, t, d, a, b) => Inst::IBin(*o, *t, m(*d), m(*a), m(*b)),
        Inst::IBinI(o, t, d, a, k) => Inst::IBinI(*o, *t, m(*d), m(*a), *k),
        Inst::INeg(t, d, a) => Inst::INeg(*t, m(*d), m(*a)),
        Inst::INot(t, d, a) => Inst::INot(*t, m(*d), m(*a)),
        Inst::IBit(o, d, a) => Inst::IBit(*o, m(*d), m(*a)),
        Inst::ICmp(c, s, d, a, b) => Inst::ICmp(*c, *s, m(*d), m(*a), m(*b)),
        Inst::ICmpI(c, s, d, a, k) => Inst::ICmpI(*c, *s, m(*d), m(*a), *k),
        Inst::FBin(o, w, d, a, b) => Inst::FBin(*o, *w, m(*d), m(*a), m(*b)),
        Inst::FUnary(o, w, d, a) => Inst::FUnary(*o, *w, m(*d), m(*a)),
        Inst::FCmp(c, w, d, a, b) => Inst::FCmp(*c, *w, m(*d), m(*a), m(*b)),
        Inst::Conv(c, d, a) => Inst::Conv(*c, m(*d), m(*a)),
        Inst::Load(k, d, b, o) => Inst::Load(*k, m(*d), m(*b), *o),
        Inst::Store(k, b, o, s) => Inst::Store(*k, m(*b), *o, m(*s)),
        Inst::SlotAddr(d, s) => Inst::SlotAddr(m(*d), s + slot_base),
        Inst::Addr(d, a) => Inst::Addr(m(*d), *a),
        Inst::FnAddr(d, id) => Inst::FnAddr(m(*d), *id),
        Inst::Call(c, a, r) => {
            let c2 = match c {
                Callee::Indirect(v) => Callee::Indirect(m(*v)),
                Callee::Fn(i) => Callee::Fn(*i),
                Callee::Host(h) => Callee::Host(*h),
                Callee::CHost(h, sig) => Callee::CHost(*h, sig.clone()),
                Callee::CIndirect(v, sig) => Callee::CIndirect(m(*v), sig.clone()),
                Callee::CFn(i, sig) => Callee::CFn(*i, sig.clone()),
            };
            let mut a2 = Vec::new();
            for v in a {
                a2.push(m(*v));
            }
            let mut r2 = Vec::new();
            for v in r {
                r2.push(m(*v));
            }
            Inst::Call(c2, a2, r2)
        }
        Inst::Copy(a, b, n) => Inst::Copy(m(*a), m(*b), *n),
        Inst::LoadX(k, d, b, x, sh, o) => Inst::LoadX(*k, m(*d), m(*b), m(*x), *sh, *o),
        Inst::StoreX(k, b, x, sh, o, v) => Inst::StoreX(*k, m(*b), m(*x), *sh, *o, m(*v)),
        Inst::AtomicLoad(k, o, d, a) => Inst::AtomicLoad(*k, *o, m(*d), m(*a)),
        Inst::AtomicStore(k, o, a, v) => Inst::AtomicStore(*k, *o, m(*a), m(*v)),
        Inst::AtomicRmw(op, k, o, d, a, v) => Inst::AtomicRmw(*op, *k, *o, m(*d), m(*a), m(*v)),
        Inst::AtomicCas(k, o1, o2, d, a, e, n) => Inst::AtomicCas(*k, *o1, *o2, m(*d), m(*a), m(*e), m(*n)),
        Inst::Fence(o, st) => Inst::Fence(*o, *st),
        Inst::FpEnv(s, v) => Inst::FpEnv(*s, m(*v)),
        Inst::Cpuid(ds, a, b) => Inst::Cpuid(ds.map(m), m(*a), m(*b)),
        Inst::Xgetbv(d, a) => Inst::Xgetbv(m(*d), m(*a)),
        Inst::TlsAddr(d, off) => Inst::TlsAddr(m(*d), *off),
        Inst::Sel(d, c, a, b) => Inst::Sel(m(*d), m(*c), m(*a), m(*b)),
        Inst::VBin(o, d, a, b) => Inst::VBin(*o, m(*d), m(*a), m(*b)),
        Inst::VUn(o, d, a) => Inst::VUn(*o, m(*d), m(*a)),
        Inst::VSel(d, k, a, b) => Inst::VSel(m(*d), m(*k), m(*a), m(*b)),
        Inst::VFma(np, na, d, a, b, c) => Inst::VFma(*np, *na, m(*d), m(*a), m(*b), m(*c)),
        Inst::VSplat(d, a) => Inst::VSplat(m(*d), m(*a)),
        Inst::VExtract(d, a, l) => Inst::VExtract(m(*d), m(*a), *l),
        Inst::VInsert(d, v, l, a) => Inst::VInsert(m(*d), m(*v), *l, m(*a)),
        Inst::VLoad(d, b, o) => Inst::VLoad(m(*d), m(*b), *o),
        Inst::VStore(b, o, v) => Inst::VStore(m(*b), *o, m(*v)),
        Inst::VAny(d, a) => Inst::VAny(m(*d), m(*a)),
        Inst::VConst(d, k) => Inst::VConst(m(*d), *k),
        Inst::VShift(o, d, a, k) => Inst::VShift(*o, m(*d), m(*a), *k),
        Inst::VGather(d, b, i) => Inst::VGather(m(*d), m(*b), m(*i)),
        Inst::VX(op, d, a, b) => Inst::VX(*op, m(*d), m(*a), m(*b)),
        Inst::VX3(op, d, a, b, c) => Inst::VX3(*op, m(*d), m(*a), m(*b), m(*c)),
        Inst::FMulAdd(np, na, w, d, a, b, c) => Inst::FMulAdd(*np, *na, *w, m(*d), m(*a), m(*b), m(*c)),
    }
}

// ------------------------------------------------------------ folding

#[derive(Clone, Copy, PartialEq, Debug)]
enum K {
    I(i64),
    F(u64, bool),
    /// a symbol's value + offset (an address the embedder relocates)
    A(Sym),
}

/// What the embedder says about `n` bytes at a constant address in immutable data
/// (`RoReader`): their value, a pointer (the 8 bytes hold symbol + offset), or "do not fold"
/// (not immutable, or bytes whose meaning the embedder cannot vouch for across runs).
pub enum RoRead {
    /// the bytes, little-endian, zero-extended to 64 bits
    Bytes(u64),
    Ptr(Sym),
    /// For a zero-size query only: this many bytes starting at the symbol are immutable.
    /// Unlike a scalar read this does not require classifying pointer words or padding.
    Range(u64),
    No,
}

/// Loads at constant addresses fold through this (None: no load folding).
/// A query with n = 0 asks for an immutable extent (Range), without reading any bytes.
/// `ctx` names the embedder state the symbols belong to (the JIT unit the function is built
/// for): a process can hold more than one unit (the proc-macro world next to the program), and
/// a symbol id read against another unit's tables names another object.
#[derive(Clone, Copy, Debug)]
pub struct RoReader {
    read: fn(usize, Sym, u32) -> RoRead,
    ctx: usize,
}

impl RoReader {
    pub fn new(read: fn(usize, Sym, u32) -> RoRead, ctx: usize) -> RoReader {
        RoReader { read, ctx }
    }

    /// `n` bytes at symbol + offset (n = 0: the immutable extent).
    pub fn read(&self, s: Sym, n: u32) -> RoRead {
        (self.read)(self.ctx, s, n)
    }
}

/// `IBit` on a constant (the semantics every backend implements).
pub fn bit_op(o: BitOp, x: u64) -> u64 {
    match o {
        BitOp::Popcnt => x.count_ones() as u64,
        BitOp::Clz => x.leading_zeros() as u64,
        BitOp::Ctz => x.trailing_zeros() as u64,
        BitOp::Bswap => x.swap_bytes(),
        BitOp::Rev => x.reverse_bits(),
    }
}

/// Rewrites `IBit` ops a target lacks (`keep(op)` false) into shift/mask sequences
/// (branch-free SWAR in 64-bit arithmetic; bit reverse keeps a byte swap when kept).
pub fn expand_bit_ops(f: &mut Func, keep: &dyn Fn(BitOp) -> bool) {
    let u = IntTy { bits: 64, signed: false };
    for bi in 0..f.blocks.len() {
        if !f.blocks[bi].insts.iter().any(|i| matches!(i, Inst::IBit(o, _, _) if !keep(*o))) {
            continue;
        }
        let old = std::mem::take(&mut f.blocks[bi].insts);
        let old_pos = std::mem::take(&mut f.blocks[bi].pos);
        let mut out: Vec<Inst> = Vec::with_capacity(old.len() + 32);
        let mut pos: Vec<u64> = Vec::with_capacity(old.len() + 32);
        for (k, i) in old.into_iter().enumerate() {
            let p = old_pos.get(k).copied().unwrap_or(0);
            let (o, d, x) = match i {
                Inst::IBit(o, d, x) if !keep(o) => (o, d, x),
                other => {
                    out.push(other);
                    pos.push(p);
                    continue;
                }
            };
            let mut e = BitEx { f: &mut *f, out: &mut out, u };
            let r = match o {
                BitOp::Popcnt => e.popcnt(x),
                BitOp::Clz => {
                    let mut y = x;
                    for k in [1, 2, 4, 8, 16, 32] {
                        let s = e.binc(IOp::Shr, y, k);
                        y = e.bin(IOp::Or, y, s);
                    }
                    let pc = e.popcnt(y);
                    let n = e.k(64);
                    e.bin(IOp::Sub, n, pc)
                }
                BitOp::Ctz => {
                    let xm1 = e.binc(IOp::Sub, x, 1);
                    let nx = e.binc(IOp::Xor, x, -1);
                    let y = e.bin(IOp::And, nx, xm1);
                    e.popcnt(y)
                }
                BitOp::Bswap => {
                    let y = e.swap(x, 8, 0x00ff_00ff_00ff_00ff);
                    let y = e.swap(y, 16, 0x0000_ffff_0000_ffff);
                    let a = e.binc(IOp::Shr, y, 32);
                    let b = e.binc(IOp::Shl, y, 32);
                    e.bin(IOp::Or, a, b)
                }
                BitOp::Rev => {
                    let mut y = x;
                    for (k, m) in [(1, 0x5555_5555_5555_5555u64), (2, 0x3333_3333_3333_3333), (4, 0x0f0f_0f0f_0f0f_0f0f)] {
                        y = e.swap(y, k, m);
                    }
                    if keep(BitOp::Bswap) {
                        let r = e.f.vreg(Cls::I);
                        e.out.push(Inst::IBit(BitOp::Bswap, r, y));
                        r
                    } else {
                        let y = e.swap(y, 8, 0x00ff_00ff_00ff_00ff);
                        let y = e.swap(y, 16, 0x0000_ffff_0000_ffff);
                        let a = e.binc(IOp::Shr, y, 32);
                        let b = e.binc(IOp::Shl, y, 32);
                        e.bin(IOp::Or, a, b)
                    }
                }
            };
            out.push(Inst::Mov(d, r));
            while pos.len() < out.len() {
                pos.push(p);
            }
        }
        f.blocks[bi].insts = out;
        f.blocks[bi].pos = pos;
    }
}

struct BitEx<'a> {
    f: &'a mut Func,
    out: &'a mut Vec<Inst>,
    u: IntTy,
}

impl BitEx<'_> {
    fn k(&mut self, v: i64) -> VReg {
        let d = self.f.vreg(Cls::I);
        self.out.push(Inst::Iconst(d, v));
        d
    }
    fn bin(&mut self, op: IOp, a: VReg, b: VReg) -> VReg {
        let d = self.f.vreg(Cls::I);
        self.out.push(Inst::IBin(op, self.u, d, a, b));
        d
    }
    fn binc(&mut self, op: IOp, a: VReg, k: i64) -> VReg {
        let c = self.k(k);
        self.bin(op, a, c)
    }
    /// ((x >> k) & m) | ((x & m) << k)
    fn swap(&mut self, x: VReg, k: i64, m: u64) -> VReg {
        let a = self.binc(IOp::Shr, x, k);
        let a = self.binc(IOp::And, a, m as i64);
        let b = self.binc(IOp::And, x, m as i64);
        let b = self.binc(IOp::Shl, b, k);
        self.bin(IOp::Or, a, b)
    }
    fn popcnt(&mut self, x: VReg) -> VReg {
        let s1 = self.binc(IOp::Shr, x, 1);
        let s1 = self.binc(IOp::And, s1, 0x5555_5555_5555_5555);
        let a = self.bin(IOp::Sub, x, s1);
        let lo = self.binc(IOp::And, a, 0x3333_3333_3333_3333);
        let s2 = self.binc(IOp::Shr, a, 2);
        let hi = self.binc(IOp::And, s2, 0x3333_3333_3333_3333);
        let b = self.bin(IOp::Add, lo, hi);
        let s4 = self.binc(IOp::Shr, b, 4);
        let c = self.bin(IOp::Add, b, s4);
        let c = self.binc(IOp::And, c, 0x0f0f_0f0f_0f0f_0f0f);
        let c = self.binc(IOp::Mul, c, 0x0101_0101_0101_0101);
        self.binc(IOp::Shr, c, 56)
    }
}

fn ext(v: i64, it: IntTy) -> i64 {
    crate::rir::extend_const(v, it)
}

fn fold_ibin(op: IOp, it: IntTy, a: i64, b: i64) -> Option<i64> {
    let r = match op {
        IOp::Add => a.wrapping_add(b),
        IOp::Sub => a.wrapping_sub(b),
        IOp::Mul => a.wrapping_mul(b),
        IOp::And => a & b,
        IOp::Or => a | b,
        IOp::Xor => a ^ b,
        IOp::Shl => a.wrapping_shl((b & 63) as u32),
        IOp::Shr => {
            if it.signed {
                a.wrapping_shr((b & 63) as u32)
            } else {
                ((a as u64) >> (b & 63)) as i64
            }
        }
        IOp::Rotl => {
            let k = (b as u32) % (it.bits as u32).max(1);
            match it.bits {
                32 => (a as u32).rotate_left(k) as i64,
                64 => (a as u64).rotate_left(k) as i64,
                _ => return None,
            }
        }
        IOp::MulHi => {
            if it.signed {
                ((a as i128 * b as i128) >> 64) as i64
            } else {
                ((a as u64 as u128 * b as u64 as u128) >> 64) as i64
            }
        }
        IOp::Div | IOp::Rem => {
            if b == 0 || (it.signed && b == -1) {
                return None;
            }
            if it.signed {
                if op == IOp::Div {
                    a / b
                } else {
                    a % b
                }
            } else if op == IOp::Div {
                ((a as u64) / (b as u64)) as i64
            } else {
                ((a as u64) % (b as u64)) as i64
            }
        }
    };
    Some(ext(r, it))
}

fn cmp_i(c: Cond, signed: bool, a: i64, b: i64) -> bool {
    if signed {
        match c {
            Cond::Eq => a == b,
            Cond::Ne => a != b,
            Cond::Lt => a < b,
            Cond::Le => a <= b,
            Cond::Gt => a > b,
            Cond::Ge => a >= b,
        }
    } else {
        let (a, b) = (a as u64, b as u64);
        match c {
            Cond::Eq => a == b,
            Cond::Ne => a != b,
            Cond::Lt => a < b,
            Cond::Le => a <= b,
            Cond::Gt => a > b,
            Cond::Ge => a >= b,
        }
    }
}

fn fval(bits: u64, w: bool) -> f64 {
    if w {
        f64::from_bits(bits)
    } else {
        f32::from_bits(bits as u32) as f64
    }
}

fn fold_fbin(op: FOp, w: bool, a: u64, b: u64) -> u64 {
    // select forms return an operand's exact bits
    if op == FOp::MinLt || op == FOp::MaxGt {
        let (x, y) = (fval(a, w), fval(b, w));
        let pick_a = if op == FOp::MinLt { x < y } else { x > y };
        return if pick_a { a } else { b };
    }
    if w {
        let (x, y) = (f64::from_bits(a), f64::from_bits(b));
        let r = match op {
            FOp::Add => x + y,
            FOp::Sub => x - y,
            FOp::Mul => x * y,
            FOp::Div => x / y,
            FOp::Min => x.min(y),
            FOp::Max => x.max(y),
            FOp::MinLt | FOp::MaxGt => 0.0,
        };
        r.to_bits()
    } else {
        let (x, y) = (f32::from_bits(a as u32), f32::from_bits(b as u32));
        let r = match op {
            FOp::Add => x + y,
            FOp::Sub => x - y,
            FOp::Mul => x * y,
            FOp::Div => x / y,
            FOp::Min => x.min(y),
            FOp::Max => x.max(y),
            FOp::MinLt | FOp::MaxGt => 0.0,
        };
        r.to_bits() as u64
    }
}

fn fold_fun(op: FUn, w: bool, a: u64) -> u64 {
    if w {
        let x = f64::from_bits(a);
        let r = match op {
            FUn::Neg => -x,
            FUn::Abs => x.abs(),
            FUn::Sqrt => x.sqrt(),
            FUn::Floor => x.floor(),
            FUn::Ceil => x.ceil(),
            FUn::Trunc => x.trunc(),
            FUn::RoundEven => x.round_ties_even(),
            FUn::RoundAway => x.round(),
        };
        r.to_bits()
    } else {
        let x = f32::from_bits(a as u32);
        let r = match op {
            FUn::Neg => -x,
            FUn::Abs => x.abs(),
            FUn::Sqrt => x.sqrt(),
            FUn::Floor => x.floor(),
            FUn::Ceil => x.ceil(),
            FUn::Trunc => x.trunc(),
            FUn::RoundEven => x.round_ties_even(),
            FUn::RoundAway => x.round(),
        };
        r.to_bits() as u64
    }
}

fn cmp_f(c: Cond, a: f64, b: f64) -> bool {
    match c {
        Cond::Eq => a == b,
        Cond::Ne => a != b,
        Cond::Lt => a < b,
        Cond::Le => a <= b,
        Cond::Gt => a > b,
        Cond::Ge => a >= b,
    }
}

fn f_to_int(x: f64, it: IntTy) -> i64 {
    let v = match (it.bits, it.signed) {
        (8, true) => x as i8 as i64,
        (8, false) => x as u8 as i64,
        (16, true) => x as i16 as i64,
        (16, false) => x as u16 as i64,
        (32, true) => x as i32 as i64,
        (32, false) => x as u32 as i64,
        (_, true) => x as i64,
        (_, false) => x as u64 as i64,
    };
    v
}

fn fold_conv(c: Conv, k: K) -> Option<K> {
    Some(match (c, k) {
        (Conv::IntToInt(it), K::I(v)) => K::I(ext(v, it)),
        (Conv::IntToInt(_), K::A(a)) => K::A(a),
        (Conv::IntToF(from, w), K::I(v)) => {
            let x = if from.bits == 64 && !from.signed { (v as u64) as f64 } else { v as f64 };
            if w {
                K::F(x.to_bits(), true)
            } else {
                // single rounding to f32
                let y = if from.bits == 64 && !from.signed { (v as u64) as f32 } else { v as f32 };
                K::F(y.to_bits() as u64, false)
            }
        }
        (Conv::FToInt(w, it), K::F(b, _)) => K::I(f_to_int(fval(b, w), it)),
        (Conv::F32ToF64, K::F(b, _)) => K::F((f32::from_bits(b as u32) as f64).to_bits(), true),
        (Conv::F64ToF32, K::F(b, _)) => K::F((f64::from_bits(b) as f32).to_bits() as u64, false),
        (Conv::BitsToF(w), K::I(v)) => K::F(if w { v as u64 } else { v as u32 as u64 }, w),
        (Conv::FToBits(w), K::F(b, _)) => K::I(if w { b as i64 } else { b as u32 as i64 }),
        _ => return None,
    })
}

fn swap_cond(c: Cond) -> Cond {
    match c {
        Cond::Lt => Cond::Gt,
        Cond::Le => Cond::Ge,
        Cond::Gt => Cond::Lt,
        Cond::Ge => Cond::Le,
        x => x,
    }
}

// VX and VX3 (including SHA) only define their result. No scalar constant folding
// or lane-wise algebra applies to the SHA operations.
fn is_pure(i: &Inst) -> bool {
    !is_mem_effect(i)
}

/// Instructions with effects beyond their result vregs (memory writes, calls, ordering).
pub fn is_mem_effect_pub(i: &Inst) -> bool {
    is_mem_effect(i)
}

fn is_mem_effect(i: &Inst) -> bool {
    matches!(i, Inst::Call(..) | Inst::Store(..) | Inst::StoreX(..) | Inst::Copy(..) | Inst::AtomicLoad(..) | Inst::AtomicStore(..) | Inst::AtomicRmw(..) | Inst::AtomicCas(..) | Inst::Fence(..) | Inst::FpEnv(..) | Inst::Cpuid(..) | Inst::Xgetbv(..) | Inst::VStore(..))
}

fn const_inst(d: VReg, k: K) -> Inst {
    match k {
        K::I(v) => Inst::Iconst(d, v),
        K::F(b, w) => Inst::Fconst(d, b, w),
        K::A(a) => Inst::Addr(d, a),
    }
}

/// Folding, copy propagation, immediates, branch folding and DCE.
/// `ro` answers constant loads (symbol + offset) from immutable data: they fold to their value,
/// or to a symbolic address for a pointer word (`RoReader`).
/// Renumbers the vregs a function still mentions densely (0..n): lowering allocates many that
/// later passes delete, and every pass sizes its per-vreg tables by `f.vregs.len()` -- at
/// twice the instruction count that table work dominated the optimiser.
#[inline(never)]
pub fn compact_vregs(f: &mut Func) {
    let nv = f.vregs.len();
    const NONE: u32 = u32::MAX;
    let mut map = vec![NONE; nv];
    let mut cls = Vec::new();
    let mut n = 0u32;
    let mut note = |v: VReg, map: &mut Vec<u32>, cls: &mut Vec<Cls>, vregs: &Vec<Cls>| {
        let i = v.0 as usize;
        if i < map.len() && map[i] == NONE {
            map[i] = n;
            cls.push(vregs[i]);
            n += 1;
        }
    };
    let mut uses = Vec::new();
    for p in &f.params {
        note(*p, &mut map, &mut cls, &f.vregs);
    }
    for b in &f.blocks {
        for i in &b.insts {
            for_uses(i, |v| note(v, &mut map, &mut cls, &f.vregs));
            for_defs(i, |v| note(v, &mut map, &mut cls, &f.vregs));
        }
        term_uses(&b.term, &mut uses);
        for v in &uses {
            note(*v, &mut map, &mut cls, &f.vregs);
        }
    }
    for v in &f.noalias {
        note(*v, &mut map, &mut cls, &f.vregs);
    }
    for (v, _) in &f.deref {
        note(*v, &mut map, &mut cls, &f.vregs);
    }
    if (n as usize) == nv {
        return;
    }
    let m = |v: VReg| VReg(map[v.0 as usize]);
    for b in f.blocks.iter_mut() {
        for i in b.insts.iter_mut() {
            i.vregs_mut(|v| *v = VReg(map[v.0 as usize]));
        }
        b.term = match &b.term {
            Term::Branch(c, x, y) => Term::Branch(m(*c), *x, *y),
            Term::Ret(v) => Term::Ret(v.iter().map(|x| m(*x)).collect()),
            other => other.clone(),
        };
    }
    for p in f.params.iter_mut() {
        *p = m(*p);
    }
    if let Some(p) = f.sret {
        f.sret = Some(m(p));
    }
    for p in f.noalias.iter_mut() {
        *p = m(*p);
    }
    for p in f.deref.iter_mut() {
        p.0 = m(p.0);
    }
    f.nonnull.retain(|v| (v.0 as usize) < map.len() && map[v.0 as usize] != NONE);
    for p in f.nonnull.iter_mut() {
        *p = m(*p);
    }
    f.vregs = cls;
}

// OnceLock cannot be reset through a shared static; see this crate's zone_take.
pub(crate) static T: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();

fn trace_opt(f: &Func, tag: &str) {
    if let Some(t) = T.get_or_init(|| std::env::var("MAKEPAD_RUN_TRACE_OPT").ok()) {
        if f.name.contains(t.as_str()) {
            eprintln!("=== {} {}\n{}", tag, f.name, f.dump());
        }
    }
}

/// Values whose every byte is a canonical boolean mask (0x00 or 0xff).
pub(crate) fn vector_byte_masks(f: &Func) -> Vec<bool> {
    // A byte blend is equivalent to a bitwise select only when each byte is
    // entirely zero or one. Require this of every definition, including params.
    let mut masks = vec![true; f.vregs.len()];
    for p in &f.params {
        masks[p.0 as usize] = false;
    }
    for b in &f.blocks {
        for i in &b.insts {
            let mask = match i {
                Inst::VX(VX::CmpEq(_) | VX::CmpGtS(_), ..) | Inst::VBin(VOp::ICmp(_) | VOp::FCmp(_), ..) => true,
                Inst::VConst(_, k) => k.to_le_bytes().iter().all(|b| *b == 0 || *b == 255),
                _ => false,
            };
            if !mask {
                for_defs(i, |d| masks[d.0 as usize] = false);
            }
        }
    }
    masks
}

/// Lane-op peepholes for intrinsic chains: redundant mask expansion, widening,
/// and vectors round-tripped through scalar registers.
fn vx_peep(f: &mut Func, ndef: &[u32]) {
    let nv = f.vregs.len();
    let masks = vector_byte_masks(f);
    let mut zero = vec![false; nv];
    let mut def: Vec<Option<(VX, VReg)>> = vec![None; nv];
    // single-def scalar results (zero-extended lanes) and their lane width
    let mut lane_bits = vec![0u8; nv];
    for b in &f.blocks {
        for i in &b.insts {
            if let Inst::VConst(d, 0) = i {
                zero[d.0 as usize] = ndef[d.0 as usize] == 1;
            }
            if let Inst::VX(op, d, a, _) = i {
                if ndef[d.0 as usize] == 1 && ndef[a.0 as usize] == 1 {
                    def[d.0 as usize] = Some((*op, *a));
                }
                if ndef[d.0 as usize] == 1 {
                    if let VX::Extract(w, _) | VX::AddAcross(w) | VX::MaxAcrossU(w) = op {
                        lane_bits[d.0 as usize] = w.bits() as u8;
                    }
                }
            }
        }
    }
    // the vector whose low 64 bits `x` holds
    let low = |x: VReg| -> VReg {
        if let Some((VX::FromLo64, t)) = def[x.0 as usize] {
            if let Some((VX::Extract(VW::D, 0), y)) = def[t.0 as usize] {
                return y;
            }
        }
        x
    };
    for b in &mut f.blocks {
        for i in &mut b.insts {
            if let Inst::VX(VX::CmpGtS(VW::B), d, a, b) = *i {
                // Expanding an already canonical comparison mask's sign bits
                // is an identity (the lowering of blendv_epi8 does this).
                if zero[a.0 as usize] && masks[b.0 as usize] {
                    *i = Inst::Mov(d, b);
                    continue;
                }
            }
            // the zero extension a lane extract already did
            if let Inst::Conv(Conv::IntToInt(t), d, x) = *i {
                let lb = lane_bits[x.0 as usize];
                if lb != 0 && !t.signed && lb <= t.bits {
                    *i = Inst::Mov(d, x);
                }
                continue;
            }
            if let Inst::VX(op, d, a, bb) = i {
                match *op {
                    VX::WidenLoU(w) | VX::WidenLoS(w) => {
                        let y = low(*a);
                        let signed = matches!(op, VX::WidenLoS(_));
                        if let Some((VX::BytesShrI(8), z)) = def[y.0 as usize] {
                            *op = if signed { VX::WidenHiS(w) } else { VX::WidenHiU(w) };
                            *a = z;
                        } else {
                            *a = y;
                        }
                        *bb = *a;
                    }
                    VX::ZipLo(VW::D) => {
                        *a = low(*a);
                        *bb = low(*bb);
                    }
                    VX::Extract(w, lane) if (lane as u32 + 1) * w.bits() <= 64 => {
                        *a = low(*a);
                        *bb = *a;
                    }
                    _ => {}
                }
                let _ = d;
            }
        }
    }
}

// Deferred edits keep the definition tables and the current folding walk's indices valid.
struct FoldEdit {
    block: usize,
    at: usize,
    remove: usize,
    insts: Vec<Inst>,
    pos: u64,
}

fn fold_insert(f: &Func, edits: &mut Vec<FoldEdit>, block: usize, after: usize, inst: Inst) {
    edits.push(FoldEdit { block, at: after + 1, remove: 0, insts: vec![inst], pos: f.blocks[block].pos[after] });
}

/// All definitions must be constants or copies of such values. Params and copy cycles
/// fail too; at most four copy edges are followed.
fn const_phi(f: &Func, def_at: &[Vec<(usize, usize)>], ndef: &[u32], v: VReg, depth: u32) -> bool {
    let Some(ds) = def_at.get(v.0 as usize) else { return false };
    if depth > 4 || ds.is_empty() || ds.len() != ndef[v.0 as usize] as usize {
        return false;
    }
    ds.iter().all(|&(bi, ii)| match f.blocks[bi].insts[ii] {
        Inst::Iconst(..) => true,
        Inst::Mov(_, x) => const_phi(f, def_at, ndef, x, depth + 1),
        _ => false,
    })
}

// Targets of DFS back edges, as in loop_depths. These include natural loop headers
// and conservatively cover cycles in irreducible flow too; block order is irrelevant.
fn loop_headers(f: &Func, depth: &[u32]) -> Vec<bool> {
    let mut header = vec![false; f.blocks.len()];
    if depth.iter().all(|&d| d == 0) { return header; }
    let mut state = vec![0u8; f.blocks.len()];
    let mut work = vec![(0usize, false)];
    while let Some((b, done)) = work.pop() {
        if done {
            state[b] = 2;
            continue;
        }
        match state[b] {
            1 => header[b] = true,
            2 => {}
            _ => {
                state[b] = 1;
                work.push((b, true));
                let mut succs = f.blocks[b].term.succs();
                for s in [succs.next(), succs.next()].into_iter().rev().flatten() {
                    work.push((s as usize, false));
                }
            }
        }
    }
    header
}

// In a loop, every definition must precede the read within this iteration: no
// path from a definition may reach a loop header without first passing the read.
// Header reads/defs and definitions after a same-block read are left alone.
fn defs_in_same_iteration(f: &Func, depth: &[u32], header: &[bool], defs: &[(usize, usize)], at: (usize, usize)) -> bool {
    if defs.iter().any(|&(b, _)| depth[b] != depth[at.0]) { return false; }
    if depth[at.0] == 0 { return true; }
    if header[at.0] || defs.iter().any(|&(b, i)| header[b] || (b == at.0 && i >= at.1)) { return false; }
    let mut seen = vec![false; f.blocks.len()];
    seen[at.0] = true;
    let mut work: Vec<usize> = defs.iter().map(|&(b, _)| b).collect();
    while let Some(b) = work.pop() {
        if seen[b] { continue; }
        if header[b] { return false; }
        seen[b] = true;
        work.extend(f.blocks[b].term.succs().map(|s| s as usize));
    }
    true
}

// const_phi has already bounded/validated the copy graph. Check each copied value
// at its read, since a copy can itself capture a value from the previous iteration.
fn phi_in_same_iteration(
    f: &Func, def_at: &[Vec<(usize, usize)>], kn: &[Option<K>], v: VReg,
    at: (usize, usize), depth: &[u32], header: &[bool],
) -> bool {
    let defs = &def_at[v.0 as usize];
    defs_in_same_iteration(f, depth, header, defs, at) && defs.iter().all(|&(b, i)| {
        match f.blocks[b].insts[i] {
            // A known constant needs no boolean shadow at its source definition.
            Inst::Mov(_, x) if !matches!(kn[x.0 as usize], Some(K::I(_))) => {
                phi_in_same_iteration(f, def_at, kn, x, (b, i), depth, header)
            }
            _ => true,
        }
    })
}

/// Shadow each assignment with its comparison result. A copy of another multi-def
/// value copies its boolean shadow at the same point, preserving the value it read.
fn phi_compare(
    f: &mut Func, def_at: &[Vec<(usize, usize)>], kn: &[Option<K>], v: VReg,
    cc: Cond, signed: bool, k: i64, narrow: Option<IntTy>,
    shadows: &mut Vec<(VReg, VReg)>, edits: &mut Vec<FoldEdit>,
) -> VReg {
    if let Some(&(_, b)) = shadows.iter().find(|(x, _)| *x == v) {
        return b;
    }
    let b = f.vreg(Cls::I);
    shadows.push((v, b));
    let cmp = |x| cmp_i(cc, signed, narrow.map_or(x, |t| ext(x, t)), k) as i64;
    for &(bi, ii) in &def_at[v.0 as usize] {
        let inst = match f.blocks[bi].insts[ii] {
            Inst::Iconst(_, x) => Inst::Iconst(b, cmp(x)),
            Inst::Mov(_, x) => match kn[x.0 as usize] {
                Some(K::I(kx)) => Inst::Iconst(b, cmp(kx)),
                _ => Inst::Mov(b, phi_compare(f, def_at, kn, x, cc, signed, k, narrow, shadows, edits)),
            },
            _ => unreachable!(),
        };
        fold_insert(f, edits, bi, ii, inst);
    }
    b
}

fn unchanged_between(f: &Func, ndef: &[u32], v: VReg, from: (usize, usize), to: (usize, usize)) -> bool {
    if ndef[v.0 as usize] == 1 {
        return true;
    }
    if from.0 != to.0 || from.1 >= to.1 {
        return false;
    }
    !f.blocks[from.0].insts[from.1 + 1..to.1].iter().any(|i| {
        let mut writes = false;
        for_defs(i, |d| writes |= d == v);
        writes
    })
}

/// A vreg defined only by `SlotAddr`s of one slot, more than once (inlining and sinking
/// duplicate a slot address into each path), holds one value everywhere: it gets a single
/// definition at the entry instead. The slot passes then follow it (slot_scan reads a slot
/// address only through single-def vregs and takes a multi-def one as escaping: reconcile's
/// 152-byte `Option<MirrorEntry>` temporaries were never forwarded) and the allocator
/// rematerialises it. Not in baseline bodies, whose SlotAddr positions mark slot lifetimes
/// (Slot::born), nor for call arguments, which the allocator does not rematerialise (see
/// unify_slot_addrs).
pub fn merge_slot_addr_defs(f: &mut Func) -> bool {
    if f.baseline || f.blocks.is_empty() {
        return false;
    }
    let nv = f.vregs.len();
    let mut ndef = vec![0u32; nv];
    let mut nsa = vec![0u32; nv];
    // the slot of a vreg's SlotAddr defs (u32::MAX: none yet, u32::MAX - 1: several slots)
    let mut slot = vec![u32::MAX; nv];
    for p in &f.params {
        ndef[p.0 as usize] += 2;
    }
    for b in &f.blocks {
        for i in &b.insts {
            for_defs(i, |d| ndef[d.0 as usize] += 1);
            if let Inst::Call(_, args, _) = i {
                for a in args {
                    nsa[a.0 as usize] = u32::MAX / 2;
                }
            }
            if let Inst::SlotAddr(d, s2) = i {
                let d = d.0 as usize;
                nsa[d] += 1;
                slot[d] = if slot[d] == u32::MAX || slot[d] == *s2 { *s2 } else { u32::MAX - 1 };
            }
        }
    }
    let one = |v: usize| ndef[v] > 1 && nsa[v] == ndef[v] && slot[v] < u32::MAX - 1;
    if !(0..nv).any(one) {
        return false;
    }
    let mut hoist: Vec<(VReg, u32)> = Vec::new();
    for b in f.blocks.iter_mut() {
        let mut k = 0;
        let keep: Vec<bool> = b.insts.iter().map(|i| !matches!(i, Inst::SlotAddr(d, _) if one(d.0 as usize))).collect();
        for i in &b.insts {
            if let Inst::SlotAddr(d, s2) = i {
                if one(d.0 as usize) && !hoist.iter().any(|h| h.0 == *d) {
                    hoist.push((*d, *s2));
                }
            }
        }
        b.insts.retain(|_| {
            k += 1;
            keep[k - 1]
        });
        let mut k2 = 0;
        b.pos.retain(|_| {
            k2 += 1;
            keep.get(k2 - 1).copied().unwrap_or(true)
        });
    }
    let p0 = f.blocks[0].pos.first().copied().unwrap_or(f.blocks[0].term_pos);
    let n = hoist.len();
    f.blocks[0].insts.splice(0..0, hoist.iter().map(|&(d, s2)| Inst::SlotAddr(d, s2)));
    f.blocks[0].pos.splice(0..0, std::iter::repeat(p0).take(n));
    true
}

pub fn simplify(options: &relax_session::Options, f: &mut Func, ro: Option<RoReader>, imm: bool, cse: bool) {
    let tc = pass_clock();
    compact_vregs(f);
    merge_slot_addr_defs(f);
    pass_time(14, tc);
    trace_opt(f, "simplify-start");
    let mut uses = Vec::new();
    let mut defs = Vec::new();
    let ic = !opt_off(options, "-ic");
    let (ic_conv, ic_phi, ic_sink, ic_nn) = (ic && !opt_off(options, "-icconv"), ic && !opt_off(options, "-icphi"), ic && !opt_off(options, "-icsink"), ic && !opt_off(options, "-icnn"));
    // two rounds: a third and fourth changed code size by < 0.1% on the counter (6,941 vs
    // 6,945 KB) at ~25% of the optimiser's time
    for _round in 0..2 {
        // passes in a round may add vregs (div_by_const)
        let nv = f.vregs.len();
        let tf = pass_clock();
        let reach = reachable(f);
        let mut has_vx = false;
        // definition counts (params count as a definition)
        let mut ndef = vec![0u32; nv];
        for p in &f.params {
            ndef[p.0 as usize] += 1;
        }
        for (bi, b) in f.blocks.iter().enumerate() {
            if !reach[bi] {
                continue;
            }
            for i in &b.insts {
                for_defs(i, |d| {
                    ndef[d.0 as usize] += 1;
                });
                has_vx |= matches!(i, Inst::VX(..));
            }
        }
        if has_vx {
            vx_peep(f, &ndef);
        }
        let single = |v: VReg, ndef: &Vec<u32>| ndef[v.0 as usize] == 1;
        // known constants and copy aliases of single-def vregs
        let mut kn: Vec<Option<K>> = vec![None; nv];
        let mut alias: Vec<u32> = (0..nv as u32).collect();
        let mut def_inst: Vec<Option<(IOp, VReg, i64)>> = vec![None; nv];
        // single-def integer compares: (cond, signed, a, b-or-imm, is_imm); operands must be
        // single-def too so the copy reads the same values
        let mut cmp_def: Vec<Option<(Cond, bool, VReg, i64, bool)>> = vec![None; nv];
        // single-def selects of single-def operands: (cond, a, b)
        let mut sel_def: Vec<Option<(VReg, VReg, VReg)>> = vec![None; nv];
        // IntToInt definitions also retain their source location: RIR operands can be
        // reassigned between the inner conversion and its use.
        let mut conv_def: Vec<Option<(IntTy, VReg, usize, usize)>> = vec![None; if ic { nv } else { 0 }];
        let mut bin_def: Vec<Option<(IOp, IntTy, VReg, i64, usize, usize)>> = vec![None; if ic { nv } else { 0 }];
        let mut phi_defs = vec![Vec::new(); if ic { nv } else { 0 }];
        // uses per vreg (profitability of the phi-compare fold: only when the phi dies)
        let mut phi_nuse = vec![0u32; if ic_phi { nv } else { 0 }];
        // Only build loop information when a constant-phi compare is otherwise eligible.
        let mut phi_loops = None;
        if ic_phi {
            let mut tu = Vec::new();
            for b in &f.blocks {
                for i in &b.insts {
                    for_uses(i, |u| phi_nuse[u.0 as usize] += 1);
                }
                term_uses(&b.term, &mut tu);
                for u in &tu {
                    phi_nuse[u.0 as usize] += 1;
                }
            }
        }
        // values known to fit in the low N bits (zero above), every definition counted (a loop
        // variable assigned only masked values stays narrow): 0 = no definition seen yet,
        // 64 = unknown; a fixpoint below
        let mut zw: Vec<u8> = vec![0; nv];
        for p in &f.params {
            zw[p.0 as usize] = 64;
        }
        // single-def function addresses (devirtualised calls)
        let mut fn_def: Vec<u32> = vec![u32::MAX; nv];
        for (bi, b) in f.blocks.iter().enumerate() {
            if !reach[bi] {
                continue;
            }
            for (ii, i) in b.insts.iter().enumerate() {
                if ic {
                    match *i {
                        Inst::Iconst(d, _) | Inst::Mov(d, _) => phi_defs[d.0 as usize].push((bi, ii)),
                        Inst::Conv(Conv::IntToInt(t), d, x) if single(d, &ndef) => conv_def[d.0 as usize] = Some((t, x, bi, ii)),
                        Inst::IBinI(op @ (IOp::Add | IOp::Sub | IOp::And), t, d, x, k) if single(d, &ndef) => bin_def[d.0 as usize] = Some((op, t, x, k, bi, ii)),
                        _ => {}
                    }
                }
                if let Inst::FnAddr(d, id) = i {
                    if single(*d, &ndef) {
                        fn_def[d.0 as usize] = *id;
                    }
                }
                match i {
                    Inst::ICmpI(c, sg, d, a, k) if single(*d, &ndef) && single(*a, &ndef) => cmp_def[d.0 as usize] = Some((*c, *sg, *a, *k, true)),
                    Inst::ICmp(c, sg, d, a, b2) if single(*d, &ndef) && single(*a, &ndef) && single(*b2, &ndef) => cmp_def[d.0 as usize] = Some((*c, *sg, *a, b2.0 as i64, false)),
                    Inst::Sel(d, c, a, b2) if single(*d, &ndef) && single(*c, &ndef) && single(*a, &ndef) && single(*b2, &ndef) => sel_def[d.0 as usize] = Some((*c, *a, *b2)),
                    _ => {}
                }
            }
        }
        // narrow widths: monotone, a few sweeps
        let mut settled = false;
        for _ in 0..8 {
            let mut grew = false;
            for (bi, b) in f.blocks.iter().enumerate() {
                if !reach[bi] {
                    continue;
                }
                for i in &b.insts {
                    let w = |v: &VReg, zw: &Vec<u8>| zw[v.0 as usize];
                    let mut set = |d: VReg, b: u8, zw: &mut Vec<u8>| {
                        if b > zw[d.0 as usize] {
                            zw[d.0 as usize] = b;
                            grew = true;
                        }
                    };
                    match i {
                        Inst::Load(Mem::Int(n, false), d, ..) | Inst::LoadX(Mem::Int(n, false), d, ..) => set(*d, (*n * 8).min(64), &mut zw),
                        Inst::Conv(Conv::IntToInt(t), d, x) if !t.signed => {
                            let b = t.bits.min(w(x, &zw).max(1));
                            set(*d, if w(x, &zw) == 0 { 0 } else { b }, &mut zw)
                        }
                        Inst::IBinI(IOp::And, _, d, x, m) if *m >= 0 => {
                            let b = ((64 - (*m as u64).leading_zeros()) as u8).min(w(x, &zw));
                            set(*d, b, &mut zw)
                        }
                        Inst::IBinI(IOp::Shr, it, d, x, k) if !it.signed && it.bits == 64 && (0..64).contains(k) => {
                            let b = (64 - *k as u8).min(w(x, &zw));
                            set(*d, b, &mut zw)
                        }
                        Inst::Iconst(d, v) if *v >= 0 => set(*d, (64 - (*v as u64).leading_zeros()) as u8, &mut zw),
                        Inst::ICmp(_, _, d, ..) | Inst::ICmpI(_, _, d, ..) | Inst::FCmp(_, _, d, ..) => set(*d, 1, &mut zw),
                        Inst::Mov(d, x) => {
                            let b = w(x, &zw);
                            set(*d, b, &mut zw)
                        }
                        _ => for_defs(i, |d| set(d, 64, &mut zw)),
                    }
                }
            }
            if !grew {
                settled = true;
                break;
            }
        }
        if !settled {
            // still growing: nothing is known
            zw.iter_mut().for_each(|x| *x = 64);
        }
        for (bi, b) in f.blocks.iter().enumerate() {
            if !reach[bi] {
                continue;
            }
            for i in &b.insts {
                match i {
                    Inst::Iconst(d, v) if single(*d, &ndef) => kn[d.0 as usize] = Some(K::I(*v)),
                    Inst::Fconst(d, v, w) if single(*d, &ndef) => kn[d.0 as usize] = Some(K::F(*v, *w)),
                    Inst::Addr(d, a) if single(*d, &ndef) => kn[d.0 as usize] = Some(K::A(*a)),
                    Inst::Mov(d, s) if single(*d, &ndef) && single(*s, &ndef) => alias[d.0 as usize] = s.0,
                    Inst::IBinI(IOp::Add, _, d, a, k) if single(*d, &ndef) && single(*a, &ndef) => def_inst[d.0 as usize] = Some((IOp::Add, *a, *k)),
                    _ => {}
                }
            }
        }
        // resolve alias chains
        for v in 0..nv {
            let mut r = alias[v];
            let mut guard = 0;
            while alias[r as usize] != r && guard < 64 {
                r = alias[r as usize];
                guard += 1;
            }
            alias[v] = r;
        }
        for v in 0..nv {
            if alias[v] != v as u32 && kn[v].is_none() {
                kn[v] = kn[alias[v] as usize];
            }
        }
        let mut changed = false;
        let mut edits = Vec::new();
        // memmoves of a known small size, expanded after the walk: (block, index, size)
        let mut moves: Vec<(usize, usize, u32)> = Vec::new();
        let a = |v: VReg| VReg(alias[v.0 as usize]);
        for bi in 0..f.blocks.len() {
            if !reach[bi] {
                continue;
            }
            for ii in 0..f.blocks[bi].insts.len() {
                // skip instructions with nothing to rename or fold (most of them)
                uses_defs(&f.blocks[bi].insts[ii], &mut uses, &mut defs);
                let mut interesting = false;
                for u in &uses {
                    if alias[u.0 as usize] != u.0 || kn[u.0 as usize].is_some() || def_inst[u.0 as usize].is_some() {
                        interesting = true;
                        break;
                    }
                }
                if !interesting && !matches!(f.blocks[bi].insts[ii], Inst::IBin(..) | Inst::ICmp(..) | Inst::IBinI(..) | Inst::ICmpI(..) | Inst::Conv(..) | Inst::Call(Callee::Indirect(_), ..)) {
                    continue;
                }
                let old = f.blocks[bi].insts[ii].clone();
                let mut new = rename(&old, &a);
                let kv = |v: VReg, kn: &Vec<Option<K>>| kn[v.0 as usize];
                let folded: Option<(VReg, K)> = match &new {
                    Inst::Mov(d, s) => kv(*s, &kn).map(|k| (*d, k)),
                    Inst::IBin(op, it, d, x, y) => match (kv(*x, &kn), kv(*y, &kn)) {
                        (Some(K::I(p)), Some(K::I(q))) => fold_ibin(*op, *it, p, q).map(|r| (*d, K::I(r))),
                        (Some(K::A(p)), Some(K::I(q))) if *op == IOp::Add => Some((*d, K::A(p.plus(q)))),
                        (Some(K::I(p)), Some(K::A(q))) if *op == IOp::Add => Some((*d, K::A(q.plus(p)))),
                        _ => None,
                    },
                    Inst::IBinI(op, it, d, x, q) => match kv(*x, &kn) {
                        Some(K::I(p)) => fold_ibin(*op, *it, p, *q).map(|r| (*d, K::I(r))),
                        Some(K::A(p)) if *op == IOp::Add => Some((*d, K::A(p.plus(*q)))),
                        // x & 0, x * 0
                        _ if *q == 0 && matches!(op, IOp::And | IOp::Mul) => Some((*d, K::I(0))),
                        _ => None,
                    },
                    Inst::INeg(it, d, x) => match kv(*x, &kn) {
                        Some(K::I(p)) => Some((*d, K::I(ext(p.wrapping_neg(), *it)))),
                        _ => None,
                    },
                    Inst::IBit(o, d, x) => match kv(*x, &kn) {
                        Some(K::I(p)) => Some((*d, K::I(bit_op(*o, p as u64) as i64))),
                        _ => None,
                    },
                    Inst::INot(it, d, x) => match kv(*x, &kn) {
                        Some(K::I(p)) => Some((*d, K::I(ext(!p, *it)))),
                        _ => None,
                    },
                    Inst::ICmp(c, s, d, x, y) => match (kv(*x, &kn), kv(*y, &kn)) {
                        (Some(K::I(p)), Some(K::I(q))) => Some((*d, K::I(cmp_i(*c, *s, p, q) as i64))),
                        _ => None,
                    },
                    Inst::ICmpI(c, s, d, x, q) => match kv(*x, &kn) {
                        Some(K::I(p)) => Some((*d, K::I(cmp_i(*c, *s, p, *q) as i64))),
                        _ => None,
                    },
                    Inst::FBin(op, w, d, x, y) => match (kv(*x, &kn), kv(*y, &kn)) {
                        (Some(K::F(p, _)), Some(K::F(q, _))) => Some((*d, K::F(fold_fbin(*op, *w, p, q), *w))),
                        _ => None,
                    },
                    Inst::FUnary(op, w, d, x) => match kv(*x, &kn) {
                        Some(K::F(p, _)) => Some((*d, K::F(fold_fun(*op, *w, p), *w))),
                        _ => None,
                    },
                    Inst::Sel(d, c, x, y) => match kv(*c, &kn) {
                        Some(K::I(p)) => kv(if p != 0 { *x } else { *y }, &kn).map(|k| (*d, k)),
                        _ => None,
                    },
                    Inst::FCmp(c, w, d, x, y) => match (kv(*x, &kn), kv(*y, &kn)) {
                        (Some(K::F(p, _)), Some(K::F(q, _))) => Some((*d, K::I(cmp_f(*c, fval(p, *w), fval(q, *w)) as i64))),
                        _ => None,
                    },
                    Inst::Conv(c, d, x) => match kv(*x, &kn) {
                        Some(k) => fold_conv(*c, k).map(|r| (*d, r)),
                        None => None,
                    },
                    Inst::Load(m, d, b, off) => match kv(*b, &kn) {
                        Some(K::A(addr)) => {
                            let at = addr.plus(*off as i64);
                            let n = match m {
                                Mem::Int(n, _) => *n as u32,
                                Mem::F32 => 4,
                                Mem::F64 => 8,
                            };
                            match ro.map_or(RoRead::No, |rd| rd.read(at, n)) {
                                RoRead::Bytes(raw) => {
                                    let k = match m {
                                        Mem::Int(1, s) => K::I(if *s { raw as u8 as i8 as i64 } else { raw as u8 as i64 }),
                                        Mem::Int(2, s) => K::I(if *s { raw as u16 as i16 as i64 } else { raw as u16 as i64 }),
                                        Mem::Int(4, s) => K::I(if *s { raw as u32 as i32 as i64 } else { raw as u32 as i64 }),
                                        Mem::Int(_, _) => K::I(raw as i64),
                                        Mem::F32 => K::F(raw as u32 as u64, false),
                                        Mem::F64 => K::F(raw, true),
                                    };
                                    Some((*d, k))
                                }
                                RoRead::Ptr(p) if matches!(m, Mem::Int(8, _)) => Some((*d, K::A(p))),
                                _ => None,
                            }
                        }
                        _ => None,
                    },
                    _ => None,
                };
                if let Some((d, k)) = folded {
                    new = const_inst(d, k);
                    if single(d, &ndef) && kn[d.0 as usize].is_none() {
                        kn[d.0 as usize] = Some(k);
                    }
                } else {
                    // address arithmetic into load/store offsets; immediates
                    new = match new {
                        Inst::Load(m, d, b, off) => match def_inst[b.0 as usize] {
                            Some((IOp::Add, x, k)) if (off as i64 + k).abs() < (1 << 30) => Inst::Load(m, d, x, off + k as i32),
                            _ => Inst::Load(m, d, b, off),
                        },
                        Inst::Store(m, b, off, s) => match def_inst[b.0 as usize] {
                            Some((IOp::Add, x, k)) if (off as i64 + k).abs() < (1 << 30) => Inst::Store(m, x, off + k as i32, s),
                            _ => Inst::Store(m, b, off, s),
                        },
                        // memcpy of a known small size: an inline copy (SROA and the backends'
                        // copy sequences see it; `ptr::copy_nonoverlapping(p, q, 1)` of a T)
                        Inst::Call(Callee::Host(a), ref args, ref rets)
                            if rets.is_empty() && args.len() == 3 && Some(a) == mem_fns().map(|m| m.0) && matches!(kn[args[2].0 as usize], Some(K::I(n)) if (0..=INLINE_COPY_MAX).contains(&n)) =>
                        {
                            match kn[args[2].0 as usize] {
                                Some(K::I(n)) => Inst::Copy(args[0], args[1], n as u32),
                                _ => unreachable!(),
                            }
                        }
                        Inst::Sel(d, c, x, y) if matches!(kn[c.0 as usize], Some(K::I(_))) => match kn[c.0 as usize] {
                            Some(K::I(p)) if p != 0 => Inst::Mov(d, x),
                            _ => Inst::Mov(d, y),
                        },
                        Inst::Sel(d, _, x, y) if x == y => Inst::Mov(d, x),
                        Inst::IBin(op, it, d, x, y) if imm && op != IOp::Div && op != IOp::Rem && op != IOp::MulHi => {
                            let fits = |k: Option<K>| match k {
                                Some(K::I(v)) if v >= i32::MIN as i64 && v <= i32::MAX as i64 => Some(v),
                                _ => None,
                            };
                            let commutative = matches!(op, IOp::Add | IOp::Mul | IOp::And | IOp::Or | IOp::Xor);
                            if let Some(v) = fits(kn[y.0 as usize]) {
                                Inst::IBinI(op, it, d, x, v)
                            } else if commutative && fits(kn[x.0 as usize]).is_some() {
                                Inst::IBinI(op, it, d, y, fits(kn[x.0 as usize]).unwrap())
                            } else {
                                Inst::IBin(op, it, d, x, y)
                            }
                        }
                        Inst::ICmp(c, s, d, x, y) if imm => {
                            let fits = |k: Option<K>| match k {
                                Some(K::I(v)) if v >= i32::MIN as i64 && v <= i32::MAX as i64 => Some(v),
                                _ => None,
                            };
                            if let Some(v) = fits(kn[y.0 as usize]) {
                                Inst::ICmpI(c, s, d, x, v)
                            } else if let Some(v) = fits(kn[x.0 as usize]) {
                                Inst::ICmpI(swap_cond(c), s, d, y, v)
                            } else {
                                Inst::ICmp(c, s, d, x, y)
                            }
                        }
                        other => other,
                    };
                    // x * 1.0, 1.0 * x, x / 1.0, x - 0.0, x + -0.0, -0.0 + x: x itself (exact in
                    // IEEE arithmetic, a NaN's payload aside, which Rust leaves unspecified;
                    // LLVM's instsimplify does the same) -- `Vec4 { w: 1.0, .. }` through a matrix
                    if let Inst::FBin(op, w, d, x, y) = new {
                        let kf = |v: VReg| match kn[v.0 as usize] {
                            Some(K::F(b, _)) => Some(b),
                            _ => None,
                        };
                        let (one, nz) = if w { (1.0f64.to_bits(), (-0.0f64).to_bits()) } else { (1.0f32.to_bits() as u64, (-0.0f32).to_bits() as u64) };
                        let same = match op {
                            FOp::Mul | FOp::Div if kf(y) == Some(one) => Some(x),
                            FOp::Mul if kf(x) == Some(one) => Some(y),
                            FOp::Sub if kf(y) == Some(0) => Some(x),
                            FOp::Add if kf(y) == Some(nz) => Some(x),
                            FOp::Add if kf(x) == Some(nz) => Some(y),
                            _ => None,
                        };
                        if let Some(v) = same {
                            new = Inst::Mov(d, v);
                        }
                    }
                    // an unsigned compare of a narrow value with a constant beyond its range
                    if let Inst::ICmpI(c, false, d, x, k) = new {
                        let wx = zw[x.0 as usize];
                        if wx < 63 && k >= 0 {
                            let max = (1u64 << wx) - 1;
                            let r = match c {
                                Cond::Lt if max < k as u64 => Some(1),
                                Cond::Le if max <= k as u64 => Some(1),
                                Cond::Ge if max < k as u64 => Some(0),
                                Cond::Gt if max <= k as u64 => Some(0),
                                Cond::Eq if max < k as u64 => Some(0),
                                Cond::Ne if max < k as u64 => Some(1),
                                _ => None,
                            };
                            if let Some(v) = r {
                                new = Inst::Iconst(d, v);
                                if single(d, &ndef) {
                                    kn[d.0 as usize] = Some(K::I(v));
                                }
                            }
                        }
                    }
                    // a zero-extension or mask of a value already that narrow
                    if let Inst::Conv(Conv::IntToInt(t), d, x) = new {
                        if !t.signed && zw[x.0 as usize] <= t.bits {
                            new = Inst::Mov(d, x);
                        } else if ic_conv {
                            if let Some((t1, y, cb, ci)) = conv_def[x.0 as usize] {
                                if t == t1 {
                                    new = Inst::Mov(d, x);
                                } else if t.bits <= t1.bits && unchanged_between(f, &ndef, y, (cb, ci), (bi, ii)) {
                                    new = Inst::Conv(Conv::IntToInt(t), d, y);
                                }
                            }
                        }
                    }
                    if let Inst::IBinI(IOp::And, _, d, x, m) = new {
                        if m > 0 && (m as u64 + 1).is_power_of_two() && zw[x.0 as usize] as u32 <= 64 - (m as u64).leading_zeros() {
                            new = Inst::Mov(d, x);
                        }
                    }
                    // a call through a known function's address: a direct call (inlinable)
                    if let Inst::Call(Callee::Indirect(v), ref args, ref rets) = new {
                        if fn_def[v.0 as usize] != u32::MAX {
                            new = Inst::Call(Callee::Fn(fn_def[v.0 as usize]), args.clone(), rets.clone());
                        }
                    }
                    // x <=> x
                    if let Inst::ICmp(c, _, d, x, y) = new {
                        if x == y {
                            let v = matches!(c, Cond::Eq | Cond::Le | Cond::Ge) as i64;
                            new = Inst::Iconst(d, v);
                            if single(d, &ndef) {
                                kn[d.0 as usize] = Some(K::I(v));
                            }
                        }
                    }
                    // a compare result (0 / 1) masked with 1
                    if let Inst::IBinI(IOp::And, _, d, x, 1) = new {
                        if cmp_def[x.0 as usize].is_some() {
                            new = Inst::Mov(d, x);
                        }
                    }
                    // unsigned compares with 0 / 1 that are equality tests
                    if let Inst::ICmpI(c, false, d, x, k) = new {
                        new = match (c, k) {
                            (Cond::Le, 0) | (Cond::Lt, 1) => Inst::ICmpI(Cond::Eq, false, d, x, 0),
                            (Cond::Gt, 0) | (Cond::Ge, 1) => Inst::ICmpI(Cond::Ne, false, d, x, 0),
                            _ => new,
                        };
                    }
                    // `(c ? k1 : k2) == k` with constants k1 != k2: c itself (or !c); c is a
                    // compare result (0 / 1)
                    if let Inst::ICmpI(cc @ (Cond::Eq | Cond::Ne), _, d, x, k) = new {
                        if let Some((c, a, b)) = sel_def[x.0 as usize] {
                            if let (Some(K::I(ka)), Some(K::I(kb)), Some(_)) = (kn[a.0 as usize], kn[b.0 as usize], cmp_def[c.0 as usize]) {
                                if ka != kb && (k == ka || k == kb) {
                                    let is_c = (k == ka) == (cc == Cond::Eq);
                                    new = if is_c { Inst::Mov(d, c) } else { Inst::IBinI(IOp::Xor, U64, d, c, 1) };
                                }
                            }
                        }
                    }
                    if ic_phi {
                        // A compare of an Ordering-like phi of constants, optionally
                        // narrowed first: distribute the compare over its assignments.
                        if let Inst::ICmpI(cc, signed, d, x, k) = new {
                            let (root, narrow, snapshot) = match conv_def[x.0 as usize] {
                                Some((t, y, cb, ci)) if t.bits < 64 => (y, Some(t), Some((cb, ci))),
                                _ => (x, None, None),
                            };
                            // only when the phi has no other use: otherwise its constants stay
                            // live next to the compare's (markdown +5%, splash_parse +2% at O1)
                            let dies = (root.0 as usize) < phi_nuse.len() && phi_nuse[root.0 as usize] == 1 && (narrow.is_none() || phi_nuse.get(x.0 as usize) == Some(&1));
                            if dies && ndef[root.0 as usize] > 1 && const_phi(f, &phi_defs, &ndef, root, 0) {
                                let (depth, header) = phi_loops.get_or_insert_with(|| {
                                    let depth = crate::regalloc::loop_depths(f, &reach);
                                    let header = loop_headers(f, &depth);
                                    (depth, header)
                                });
                                let at = snapshot.unwrap_or((bi, ii));
                                // Loop-carried shadows add assignments/register pressure in the
                                // hot loop (llm_host). Keep only per-iteration Ordering-like phis.
                                if (snapshot.is_none() || defs_in_same_iteration(f, depth, header, &[at], (bi, ii)))
                                    && phi_in_same_iteration(f, &phi_defs, &kn, root, at, depth, header)
                                {
                                    let mut b = phi_compare(f, &phi_defs, &kn, root, cc, signed, k, narrow, &mut Vec::new(), &mut edits);
                                    if let Some((cb, ci)) = snapshot {
                                        let saved = f.vreg(Cls::I);
                                        fold_insert(f, &mut edits, cb, ci, Inst::Mov(saved, b));
                                        b = saved;
                                    }
                                    new = Inst::Mov(d, b);
                                }
                            }
                        }
                        // ((x - c1) & (2^n - 1)) == 0 -> (x & (2^n - 1)) == (c1 & mask).
                        if let Inst::ICmpI(cc @ (Cond::Eq | Cond::Ne), signed, d, a, 0) = new {
                            if let Some((IOp::And, mt, s, m, _, _)) = bin_def[a.0 as usize] {
                                if m > 0 && (m as u64 + 1).is_power_of_two() {
                                    if let Some((op @ (IOp::Add | IOp::Sub), st, x, c, sb, si)) = bin_def[s.0 as usize] {
                                        let bits = (64 - (m as u64).leading_zeros()) as u8;
                                        if bits <= mt.bits && bits <= st.bits {
                                            let a2 = f.vreg(Cls::I);
                                            // Capture x where the subtraction read it; x itself may be mutable.
                                            fold_insert(f, &mut edits, sb, si, Inst::IBinI(IOp::And, U64, a2, x, m));
                                            let c1 = if op == IOp::Add { c.wrapping_neg() } else { c };
                                            new = Inst::ICmpI(cc, signed, d, a2, c1 & m);
                                        }
                                    }
                                }
                            }
                        }
                    }
                    if let Inst::IBinI(IOp::Xor, _, d, x, 1) = new {
                        // !cmp -> the inverted integer compare
                        if single(x, &ndef) {
                            if let Some((c, sg, a, kk, is_imm)) = cmp_def[x.0 as usize] {
                                let inv = match c {
                                    Cond::Eq => Cond::Ne,
                                    Cond::Ne => Cond::Eq,
                                    Cond::Lt => Cond::Ge,
                                    Cond::Ge => Cond::Lt,
                                    Cond::Le => Cond::Gt,
                                    Cond::Gt => Cond::Le,
                                };
                                new = if is_imm { Inst::ICmpI(inv, sg, d, a, kk) } else { Inst::ICmp(inv, sg, d, a, VReg(kk as u32)) };
                            }
                        }
                    }
                    if let Inst::IBinI(op, it, d, x, k) = new {
                        let ident = match op {
                            IOp::Add | IOp::Sub | IOp::Or | IOp::Xor | IOp::Shl | IOp::Shr => k == 0,
                            IOp::Mul | IOp::Div => k == 1,
                            _ => false,
                        };
                        // narrow types: the operand is already extended by its type
                        if k == 0 && matches!(op, IOp::And | IOp::Mul) {
                            new = Inst::Iconst(d, 0);
                            if single(d, &ndef) {
                                kn[d.0 as usize] = Some(K::I(0));
                            }
                        } else if ident {
                            new = Inst::Mov(d, x);
                        } else if op == IOp::And && k == -1 {
                            new = Inst::Mov(d, x);
                        } else if op == IOp::Mul && k > 1 && (k & (k - 1)) == 0 && k < (1 << 30) {
                            new = Inst::IBinI(IOp::Shl, it, d, x, k.trailing_zeros() as i64);
                        }
                    }
                    if let Inst::IBinI(IOp::Add, _, d, x, k) = new {
                        if single(d, &ndef) && single(x, &ndef) {
                            def_inst[d.0 as usize] = Some((IOp::Add, x, k));
                        }
                    }
                }
                // memmove of a known small size: loads, then stores (overlap-safe; not a
                // `Copy`, which is a memcpy to the passes and the backends)
                if let Inst::Call(Callee::Host(m), args, rets) = &new {
                    if rets.is_empty() && args.len() == 3 && Some(*m) == mem_fns().map(|m| m.1) {
                        if let Some(K::I(n)) = kn[args[2].0 as usize] {
                            if (0..=INLINE_COPY_MAX).contains(&n) {
                                moves.push((bi, ii, n as u32));
                            }
                        }
                    }
                }
                if !same_inst(&old, &new) {
                    changed = true;
                    f.blocks[bi].insts[ii] = new;
                }
            }
            // terminator (renamed in place)
            match &mut f.blocks[bi].term {
                Term::Branch(c, x, y) => {
                    let c2 = a(*c);
                    let (x, y) = (*x, *y);
                    f.blocks[bi].term = match kn[c2.0 as usize] {
                        Some(K::I(v)) => Term::Jump(if v != 0 { x } else { y }),
                        _ => Term::Branch(c2, x, y),
                    };
                }
                Term::Ret(v) => {
                    for x in v.iter_mut() {
                        *x = a(*x);
                    }
                }
                _ => {}
            }
        }
        changed |= !moves.is_empty();
        for &(bi, ii, n) in moves.iter().rev() {
            let Inst::Call(_, ref args, _) = f.blocks[bi].insts[ii] else { unreachable!() };
            let (d, s2) = (args[0], args[1]);
            // 8-byte words, the tail as one overlapping word; under 8 bytes, 4/2/1 pieces
            let mut parts: Vec<(u8, i32)> = Vec::new();
            if n >= 8 {
                parts.extend((0..n / 8).map(|j| (8u8, j as i32 * 8)));
                if n % 8 != 0 {
                    parts.push((8, n as i32 - 8));
                }
            } else {
                let mut o = 0;
                for w in [4u32, 2, 1] {
                    if (n - o) >= w {
                        parts.push((w as u8, o as i32));
                        o += w;
                    }
                }
            }
            let ts: Vec<VReg> = parts.iter().map(|_| f.vreg(Cls::I)).collect();
            let mut seq: Vec<Inst> = parts.iter().zip(&ts).map(|(&(w, o), &t)| Inst::Load(Mem::Int(w, false), t, s2, o)).collect();
            seq.extend(parts.iter().zip(&ts).map(|(&(w, o), &t)| Inst::Store(Mem::Int(w, false), d, o, t)));
            let p = f.blocks[bi].pos[ii];
            edits.push(FoldEdit { block: bi, at: ii, remove: 1, insts: seq, pos: p });
        }
        edits.sort_by_key(|e| (e.block, e.at));
        for e in edits.into_iter().rev() {
            let b = &mut f.blocks[e.block];
            let n = e.insts.len();
            b.insts.splice(e.at..e.at + e.remove, e.insts);
            b.pos.splice(e.at..e.at + e.remove, std::iter::repeat(e.pos).take(n));
        }
        // unsigned division / remainder by a constant: multiply-high and shifts
        changed |= div_by_const(f, &kn, nv);
        pass_time(0, tf);
        let t = pass_clock();
        trace_opt(f, "fold");
        changed |= local_copy_prop(f);
        changed |= forward_narrow_stores(f);
        trace_opt(f, "lcopy");
        pass_time(2, t);
        let t = pass_clock();
        // CSE runs in the first round of each call: later rounds fold what it exposed; a
        // second CSE pass found next to nothing (counter: same code size) for 15% of opt
        if cse && _round == 0 {
            changed |= local_cse(options, f);
            trace_opt(f, "cse");
        }
        if ic_sink {
            changed |= sink_to_successor(f);
        }
        pass_time(1, t);
        let t = pass_clock();
        changed |= dce(f);
        pass_time(3, t);
        let t = pass_clock();
        thread_jumps(f);
        trace_opt(f, "dce+jumps");
        changed |= ic_nn && fold_nonnull(f);
        let tt = pass_clock();
        let threaded = thread_on(options) && thread_known(f);
        pass_time(15, tt);
        trace_opt(f, "thread");
        changed |= threaded;
        if if_conv_on(options) {
            changed |= if_convert(f);
        }
        changed |= merge_blocks(f);
        trace_opt(f, "merge");
        pass_time(4, t);
        if threaded {
            let t = pass_clock();
            changed |= live_dce(f);
            pass_time(3, t);
        }
        if !changed {
            break;
        }
    }
    // byte loads assembled into a wider value (after the rounds: it adds vregs; the next simplify folds around it)
    if f.blocks.iter().any(|b| b.insts.iter().any(|i| matches!(i, Inst::Load(Mem::Int(1, false), ..) | Inst::LoadX(Mem::Int(1, false), ..)))) {
        let mut nd = vec![0u32; f.vregs.len()];
        for p in &f.params {
            nd[p.0 as usize] += 1;
        }
        for b in &f.blocks {
            for i in &b.insts {
                for_defs(i, |d| nd[d.0 as usize] += 1);
            }
        }
        if combine_loads(f, &nd) {
            dce(f);
        }
    }
    // x << k | x >> (w - k): one rotate (rol / ror)
    if f.blocks.iter().any(|b| b.insts.iter().any(|i| matches!(i, Inst::IBin(IOp::Or, ..)))) && combine_rotates(f) {
        dce(f);
    }
    // x / 2^k: x * 2^-k (adds vregs, so after the rounds)
    if recip_pow2_div(f) {
        dce(f);
    }
    // branch on `c == 0` / `c != 0` / `c ^ 1` of a compare c: branch on c (swapping the targets)
    if branch_on_compare(f) {
        dce(f);
    }

}

/// A block ending `d = !c; branch d, t, e` (d = `c == 0`, `c ^ 1`; `c != 0` is c itself) where c is
/// a compare result (every definition of c an `ICmp` / `ICmpI` / `FCmp`, so 0 or 1) and d has no
/// other use branches on c with the targets swapped. This is the only exact form for floats: `!(a <= b)`
/// is not `a > b` on NaN, but the backends branch on a fused float compare with its unordered
/// case handled, so the compare stays one compare + one jcc / b.cond (rustc's `if !(s <= z)`).
fn branch_on_compare(f: &mut Func) -> bool {
    let nv = f.vregs.len();
    let mut cmp_only = vec![true; nv];
    let mut uses = vec![0u32; nv];
    for p in &f.params {
        cmp_only[p.0 as usize] = false;
    }
    let mut tu = Vec::new();
    for b in &f.blocks {
        for i in &b.insts {
            for_defs(i, |d| {
                if !matches!(i, Inst::ICmp(..) | Inst::ICmpI(..) | Inst::FCmp(..)) {
                    cmp_only[d.0 as usize] = false;
                }
            });
            for_uses(i, |u| uses[u.0 as usize] += 1);
        }
        tu.clear();
        term_uses(&b.term, &mut tu);
        for u in &tu {
            uses[u.0 as usize] += 1;
        }
    }
    let mut changed = false;
    for bi in 0..f.blocks.len() {
        let Term::Branch(d, t, e) = f.blocks[bi].term else { continue };
        if t == e || uses[d.0 as usize] != 1 {
            continue;
        }
        let Some(last) = f.blocks[bi].insts.last() else { continue };
        let (c, invert) = match *last {
            Inst::ICmpI(Cond::Eq, _, dd, c, 0) if dd == d => (c, true),
            Inst::ICmpI(Cond::Ne, _, dd, c, 0) if dd == d => (c, false),
            Inst::IBinI(IOp::Xor, _, dd, c, 1) if dd == d => (c, true),
            _ => continue,
        };
        // c is not redefined by the removed instruction itself, and holds a 0 / 1 compare
        if c == d || !cmp_only[c.0 as usize] || uses[c.0 as usize] != 1 {
            continue;
        }
        let b = &mut f.blocks[bi];
        b.insts.pop();
        b.pos.pop();
        b.term = if invert { Term::Branch(c, e, t) } else { Term::Branch(c, t, e) };
        changed = true;
    }
    changed
}

/// The reciprocal of a power-of-two float constant (`bits` of an f32 / f64) when that is itself
/// a finite non-zero power of two, normal or subnormal: then `x / c` and `x * (1 / c)` are the
/// correctly rounded value of the same real number for every x (NaN, inf, zero and signs
/// included; an overflow or underflow of `x * r` rounds identically in both). Zero, inf, NaN and
/// non-powers of two have none; 1 / c rounds when it is not representable, which the power-of-two
/// check on the result rejects (an overflow gives inf, an underflow 0).
fn exact_recip(w: bool, bits: u64) -> Option<u64> {
    if w {
        let pow2 = |b: u64| {
            let m = b & ((1u64 << 52) - 1);
            let e = (b >> 52) & 0x7ff;
            e != 0x7ff && if e == 0 { m.count_ones() == 1 } else { m == 0 }
        };
        if !pow2(bits) {
            return None;
        }
        let r = (1.0 / f64::from_bits(bits)).to_bits();
        pow2(r).then_some(r)
    } else {
        let pow2 = |b: u32| {
            let m = b & ((1u32 << 23) - 1);
            let e = (b >> 23) & 0xff;
            e != 0xff && if e == 0 { m.count_ones() == 1 } else { m == 0 }
        };
        let bits = bits as u32;
        if !pow2(bits) {
            return None;
        }
        let r = (1.0 / f32::from_bits(bits)).to_bits();
        pow2(r).then_some(r as u64)
    }
}

/// `x / c` for a single-definition float constant c = +-2^k with an exact reciprocal becomes
/// `x * c^-1` (LLVM does the same for exact reciprocals; a multiply is far cheaper than a divide).
fn recip_pow2_div(f: &mut Func) -> bool {
    if !f.blocks.iter().any(|b| b.insts.iter().any(|i| matches!(i, Inst::FBin(FOp::Div, ..)))) {
        return false;
    }
    let nv = f.vregs.len();
    let mut ndef = vec![0u32; nv];
    for p in &f.params {
        ndef[p.0 as usize] += 1;
    }
    let mut kf: Vec<Option<u64>> = vec![None; nv];
    for b in &f.blocks {
        for i in &b.insts {
            for_defs(i, |d| ndef[d.0 as usize] += 1);
            if let Inst::Fconst(d, bits, _) = i {
                kf[d.0 as usize] = Some(*bits);
            }
        }
    }
    let mut changed = false;
    for bi in 0..f.blocks.len() {
        let mut out: Vec<Inst> = Vec::new();
        let mut pos: Vec<_> = Vec::new();
        let old = std::mem::take(&mut f.blocks[bi].insts);
        let old_pos = std::mem::take(&mut f.blocks[bi].pos);
        for (i, p) in old.into_iter().zip(old_pos) {
            if let Inst::FBin(FOp::Div, w, d, x, y) = i {
                if ndef[y.0 as usize] == 1 {
                    if let Some(r) = kf[y.0 as usize].and_then(|b| exact_recip(w, b)) {
                        let c = f.vreg(if w { Cls::F64 } else { Cls::F32 });
                        out.push(Inst::Fconst(c, r, w));
                        out.push(Inst::FBin(FOp::Mul, w, d, x, c));
                        pos.push(p);
                        pos.push(p);
                        changed = true;
                        continue;
                    }
                }
            }
            out.push(i);
            pos.push(p);
        }
        f.blocks[bi].insts = out;
        f.blocks[bi].pos = pos;
    }
    changed
}

/// Rotates: `d = (x << k) | (x >> (w - k))` (either operand order) with both shifts by
/// constants of the same unsigned 32-bit or 64-bit type, x, the shifts and d single-def, becomes
/// `d = x rotl k` (the shifts stay for DCE when nothing else reads them). rustc's LLVM matches the
/// same funnel shift; `rotate_left` lowers to the shift pair.
fn combine_rotates(f: &mut Func) -> bool {
    let nv = f.vregs.len();
    let mut ndef = vec![0u32; nv];
    for p in &f.params {
        ndef[p.0 as usize] += 1;
    }
    let mut def_at: Vec<(u32, u32)> = vec![(u32::MAX, 0); nv];
    for (bi, b) in f.blocks.iter().enumerate() {
        for (k, i) in b.insts.iter().enumerate() {
            for_defs(i, |d| {
                ndef[d.0 as usize] += 1;
                def_at[d.0 as usize] = (bi as u32, k as u32);
            });
        }
    }
    let shift = |f: &Func, v: VReg| -> Option<(IOp, IntTy, VReg, i64)> {
        if ndef[v.0 as usize] != 1 {
            return None;
        }
        let (bi, k) = def_at[v.0 as usize];
        if bi == u32::MAX {
            return None;
        }
        match f.blocks[bi as usize].insts[k as usize] {
            Inst::IBinI(op @ (IOp::Shl | IOp::Shr), it, _, x, c) if ndef[x.0 as usize] == 1 => Some((op, it, x, c)),
            _ => None,
        }
    };
    let mut changed = false;
    for bi in 0..f.blocks.len() {
        for k in 0..f.blocks[bi].insts.len() {
            let (it, d, p, q) = match f.blocks[bi].insts[k] {
                Inst::IBin(IOp::Or, it, d, p, q) => (it, d, p, q),
                _ => continue,
            };
            let w = it.bits as i64;
            if !(w == 64 || (w == 32 && !it.signed)) {
                continue;
            }
            let (Some(a), Some(b)) = (shift(f, p), shift(f, q)) else { continue };
            let (l, r) = if a.0 == IOp::Shl { (a, b) } else { (b, a) };
            if l.0 != IOp::Shl || r.0 != IOp::Shr || l.2 != r.2 || r.1.signed || l.1.bits as i64 != w || r.1.bits as i64 != w {
                continue;
            }
            if !(1..w).contains(&l.3) || l.3 + r.3 != w {
                continue;
            }
            f.blocks[bi].insts[k] = Inst::IBinI(IOp::Rotl, IntTy { bits: w as u8, signed: false }, d, l.2, l.3);
            changed = true;
        }
    }
    changed
}

/// Removes pure instructions whose results are dead by liveness: the definitions `dce`'s use
/// counts cannot see because the vreg has several (SROA cells' entry zeros overwritten before
/// any read, instructions `thread_known` copied whose values are not needed on that path).
pub fn live_dce(f: &mut Func) -> bool {
    let nv = f.vregs.len();
    let nb = f.blocks.len();
    if nb == 0 {
        return false;
    }
    let mut uses = Vec::new();
    let mut defs = Vec::new();
    // only multi-def vregs take part (a single-def dead vreg is dce's, by use counts):
    // liveness runs over their dense indices
    let mut ndef = vec![0u8; nv];
    // vregs reaching a second definition (the candidates), numbered below in vreg order
    let mut multi: Vec<u32> = Vec::new();
    for b in &f.blocks {
        for i in &b.insts {
            for_defs(i, |d| {
                let c = &mut ndef[d.0 as usize];
                if *c == 1 {
                    multi.push(d.0);
                }
                *c = c.saturating_add(1);
            });
        }
    }
    for p in &f.params {
        ndef[p.0 as usize] = 0;
    }
    multi.retain(|v| ndef[*v as usize] > 1);
    if multi.is_empty() {
        return false;
    }
    multi.sort_unstable();
    let mut idx = vec![u32::MAX; nv];
    for (k, v) in multi.iter().enumerate() {
        idx[*v as usize] = k as u32;
    }
    let m = multi.len() as u32;
    let words = (m as usize + 63) / 64;
    let reach = reachable(f);
    let mut gen = vec![0u64; nb * words];
    let mut kill = vec![0u64; nb * words];
    for (bi, b) in f.blocks.iter().enumerate() {
        if !reach[bi] {
            continue;
        }
        let g = &mut gen[bi * words..(bi + 1) * words];
        let k = &mut kill[bi * words..(bi + 1) * words];
        for i in &b.insts {
            for_uses(i, |u| {
                let x = idx[u.0 as usize];
                if x != u32::MAX && k[x as usize / 64] & (1u64 << (x % 64)) == 0 {
                    g[x as usize / 64] |= 1u64 << (x % 64);
                }
            });
            for_defs(i, |d| {
                let x = idx[d.0 as usize];
                if x != u32::MAX {
                    k[x as usize / 64] |= 1u64 << (x % 64);
                }
            });
        }
        term_uses(&b.term, &mut uses);
        for u in &uses {
            let x = idx[u.0 as usize];
            if x != u32::MAX && k[x as usize / 64] & (1u64 << (x % 64)) == 0 {
                g[x as usize / 64] |= 1u64 << (x % 64);
            }
        }
    }
    let (live_in, _) = crate::regalloc::solve_liveness(f, &reach, words, &gen, &kill, false);
    let mut any = false;
    let mut live = vec![0u64; words];
    for bi in 0..nb {
        if !reach[bi] {
            continue;
        }
        live.iter_mut().for_each(|w| *w = 0);
        for t in f.blocks[bi].term.succs() {
            for w in 0..words {
                live[w] |= live_in[t as usize * words + w];
            }
        }
        let b = &mut f.blocks[bi];
        term_uses(&b.term, &mut uses);
        for u in &uses {
            let x = idx[u.0 as usize];
            if x != u32::MAX {
                live[x as usize / 64] |= 1u64 << (x % 64);
            }
        }
        let mut keep = vec![true; b.insts.len()];
        let mut dropped = false;
        for k in (0..b.insts.len()).rev() {
            let i = &b.insts[k];
            uses_defs(i, &mut uses, &mut defs);
            let dead = !defs.is_empty()
                && defs.iter().all(|d| {
                    let x = idx[d.0 as usize];
                    x != u32::MAX && live[x as usize / 64] & (1u64 << (x % 64)) == 0
                })
                && is_pure(i)
                && !matches!(i, Inst::Call(..));
            if dead {
                keep[k] = false;
                dropped = true;
                continue;
            }
            for d in &defs {
                let x = idx[d.0 as usize];
                if x != u32::MAX {
                    live[x as usize / 64] &= !(1u64 << (x % 64));
                }
            }
            for u in &uses {
                let x = idx[u.0 as usize];
                if x != u32::MAX {
                    live[x as usize / 64] |= 1u64 << (x % 64);
                }
            }
        }
        if dropped {
            any = true;
            let mut k = 0;
            b.insts.retain(|_| {
                k += 1;
                keep[k - 1]
            });
            let mut k = 0;
            b.pos.retain(|_| {
                k += 1;
                keep[k - 1]
            });
        }
    }
    any
}

fn no_noalias(options: &relax_session::Options) -> bool {
    options.no_noalias
}

/// Largest constant-size memcpy turned into an inline `Copy`.
const INLINE_COPY_MAX: i64 = 64;

static MEM_FNS: std::sync::OnceLock<(Sym, Sym)> = std::sync::OnceLock::new();

/// The host memcpy / memmove addresses (the runtime registers them once): calls to them
/// with a known small size fold to inline copies.
pub fn set_mem_fns(memcpy: Sym, memmove: Sym) {
    let _ = MEM_FNS.set((memcpy, memmove));
}

fn mem_fns() -> Option<(Sym, Sym)> {
    MEM_FNS.get().copied()
}

static UNWIND_HOSTS: std::sync::OnceLock<Vec<Sym>> = std::sync::OnceLock::new();

/// The runtime functions a panic starts unwinding from (the panic entries): an inlined call of
/// one of them inside an invoked call's body unwinds into the caller's cleanup pad.
pub fn set_unwind_hosts(hosts: Vec<Sym>) {
    let _ = UNWIND_HOSTS.get_or_init(|| {
        let _k = relax_session::phase::kept();
        hosts.clone()
    });
}

/// Whether a call of `c` may unwind: Relax code not known to be nounwind, calls through
/// pointers, the panic entries. C ABI calls never unwind (rustc: `extern "C"` aborts instead).
pub fn may_unwind(c: &Callee, nounwind: &dyn Fn(u32) -> bool) -> bool {
    match c {
        Callee::Fn(id) => !nounwind(*id),
        Callee::Indirect(_) => true,
        // a C-unwind call: a panic may cross it (callee facts are per Rust slot, not asked)
        Callee::CFn(_, s) | Callee::CIndirect(_, s) => s.unwinds,
        Callee::Host(a) => UNWIND_HOSTS.get().map_or(false, |v| v.contains(a)),
        _ => false,
    }
}

/// Whether a function body can unwind out of itself: a reachable Resume, or a call that may
/// unwind outside an Invoke (an invoked call's unwinding is caught by its pad, which then
/// resumes -- counted by the Resume).
pub fn body_may_unwind(f: &Func, nounwind: &dyn Fn(u32) -> bool) -> bool {
    let reach = crate::regalloc::reachable(f);
    for (bi, b) in f.blocks.iter().enumerate() {
        if !reach[bi] {
            continue;
        }
        if matches!(b.term, Term::Resume) {
            return true;
        }
        let n = b.insts.len();
        for (k, i) in b.insts.iter().enumerate() {
            if let Inst::Call(c, _, _) = i {
                let invoked = k + 1 == n && matches!(b.term, Term::Invoke(..));
                if !invoked && may_unwind(c, nounwind) {
                    return true;
                }
            }
        }
    }
    false
}

/// Invokes whose call cannot unwind (callee known nounwind) or whose pad does nothing (a chain
/// of empty blocks to a Resume) become plain calls; their pads go away with dead blocks.
pub static INVOKES_SEEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static INVOKES_DEMOTED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub fn demote_invokes(f: &mut Func, nounwind: &dyn Fn(u32) -> bool) -> bool {
    let mut changed = false;
    for bi in 0..f.blocks.len() {
        let (n, u) = match f.blocks[bi].term {
            Term::Invoke(n, u) => (n, u),
            _ => continue,
        };
        let callee_unwinds = match f.blocks[bi].insts.last() {
            Some(Inst::Call(c, _, _)) => may_unwind(c, nounwind),
            _ => true,
        };
        let mut x = u as usize;
        let mut empty_pad = false;
        for _ in 0..64 {
            let b = &f.blocks[x];
            if !b.insts.is_empty() {
                break;
            }
            match b.term {
                Term::Resume => {
                    empty_pad = true;
                    break;
                }
                Term::Jump(t) => x = t as usize,
                _ => break,
            }
        }
        INVOKES_SEEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if !callee_unwinds || empty_pad {
            f.blocks[bi].term = Term::Jump(n);
            changed = true;
            INVOKES_DEMOTED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    }
    changed
}

/// Most instructions a branch block may have to be copied into a predecessor by `thread_known`.
const THREAD_MAX_INSTS: usize = 8;

/// Jump threading over values known on an edge: a predecessor P that ends in `Jump(B)` and
/// knows (from its own constants, or its single-predecessor chain's) the values B's branch
/// condition is computed from, jumps straight to the branch's target. This is the
/// construct-then-match of small enums after inlining (`Some(x)` built, then `is_some` /
/// `?` / `match` on it): the tag is a constant on each incoming edge. The path's
/// instructions are copied into P (RIR is not SSA: a vreg may have several definitions);
/// `live_dce` then removes the copies this path does not need.
#[inline(never)]
/// `p == 0` / `p != 0` of a pointer that cannot be null become constants: the Option<&T> /
/// Option<NonNull<T>> niche tests a slice iterator's next() leaves per element (rustc knows
/// the pointers are nonnull). Non-null: stack slot, symbol and function addresses, loads
/// lowering marked (Func::nonnull: a field whose layout niche excludes 0), dereferenceable
/// params, in-bounds pointer arithmetic from a non-null base, and
/// a vreg all of whose definitions are non-null (a loop's pointer cell: start, then +4).
fn fold_nonnull(f: &mut Func) -> bool {
    let nv = f.vregs.len();
    // optimistic: every vreg some definition could make non-null starts as such; a definition
    // that is not removes it, to a fixpoint
    let mut nn = vec![true; nv];
    let mut has_def = vec![false; nv];
    let mut marked = vec![false; nv];
    for v in &f.nonnull {
        marked[v.0 as usize] = true;
    }
    for p in &f.params {
        has_def[p.0 as usize] = true;
        if !marked[p.0 as usize] && !f.deref.iter().any(|(d, _)| d == p) {
            nn[p.0 as usize] = false;
        }
    }
    let mut any_cmp = false;
    for b in &f.blocks {
        for i in &b.insts {
            for_defs(i, |d| has_def[d.0 as usize] = true);
            if let Inst::ICmpI(Cond::Eq | Cond::Ne, _, _, _, 0) = i {
                any_cmp = true;
            }
        }
    }
    if !any_cmp {
        return false;
    }
    for v in 0..nv {
        nn[v] &= has_def[v];
    }
    let mut changed = true;
    while changed {
        changed = false;
        for b in &f.blocks {
            for i in &b.insts {
                let ok = |nn: &[bool]| -> bool {
                    match i {
                        Inst::SlotAddr(..) | Inst::Addr(..) | Inst::FnAddr(..) => true,
                        Inst::Load(Mem::Int(8, _), d, ..) | Inst::LoadX(Mem::Int(8, _), d, ..) => marked[d.0 as usize],
                        Inst::Mov(_, x) => nn[x.0 as usize],
                        // Integer constants (even nonzero ones) never seed this rule:
                        // the base must trace back to one of the pointer leaves above.
                        Inst::IBinI(IOp::Add, it, _, x, _) | Inst::IBin(IOp::Add, it, _, x, _) => it.bits == 64 && nn[x.0 as usize],
                        Inst::Sel(_, _, x, y) => nn[x.0 as usize] && nn[y.0 as usize],
                        _ => false,
                    }
                };
                let good = ok(&nn);
                // `good` is computed before visiting any definition, including multi-def
                // instructions; updating here preserves the old simultaneous decision.
                for_defs(i, |d| {
                    if !good && nn[d.0 as usize] {
                        nn[d.0 as usize] = false;
                        changed = true;
                    }
                });
            }
        }
    }
    let mut folded = false;
    for b in f.blocks.iter_mut() {
        for i in b.insts.iter_mut() {
            if let Inst::ICmpI(c @ (Cond::Eq | Cond::Ne), _, d, x, 0) = *i {
                if has_def[x.0 as usize] && nn[x.0 as usize] {
                    *i = Inst::Iconst(d, (c == Cond::Ne) as i64);
                    folded = true;
                }
            }
        }
    }
    folded
}

/// Move a result used in just one successor onto that edge. Decisions use the original
/// use blocks, and arrivals are installed only at the end: one sweep, without cascading.
fn sink_to_successor(f: &mut Func) -> bool {
    const NONE: usize = usize::MAX;
    const MANY: usize = usize::MAX - 1;
    let nv = f.vregs.len();
    let nb = f.blocks.len();
    let depth = crate::regalloc::loop_depths(f, &reachable(f));
    let header = loop_headers(f, &depth);
    let mut ndef = vec![0u32; nv];
    let mut use_block = vec![NONE; nv];
    let mut npred = vec![0u32; nb];
    for p in &f.params {
        ndef[p.0 as usize] += 1;
    }
    let mut uses = Vec::new();
    for (bi, b) in f.blocks.iter().enumerate() {
        let mut note_use = |v: VReg| {
            let at = &mut use_block[v.0 as usize];
            *at = if *at == NONE || *at == bi { bi } else { MANY };
        };
        for i in &b.insts {
            for_uses(i, &mut note_use);
            for_defs(i, |d| ndef[d.0 as usize] += 1);
        }
        term_uses(&b.term, &mut uses);
        for &u in &uses {
            note_use(u);
        }
        for s in b.term.succs() {
            npred[s as usize] += 1;
        }
    }
    let mut arrivals: Vec<Vec<(Inst, u64)>> = vec![Vec::new(); nb];
    let mut written = vec![NONE; nv];
    let mut changed = false;
    let mut defs = Vec::new();
    for bi in 0..nb {
        // loops.rs runs later: counted loops need their header's body intact, and
        // byte-map widening needs loads/ALU ops to remain on its linear loop chain.
        if header[bi] { continue; }
        // Do not move work into an unwind pad or across an invoke's final call.
        if !matches!(f.blocks[bi].term, Term::Branch(..) | Term::Jump(..)) {
            continue;
        }
        let b = &f.blocks[bi];
        let mut remove = vec![false; b.insts.len()];
        let mut cand: Vec<(usize, usize)> = Vec::new();
        let mut memory_effect = false;
        let mut fp_env = false;
        for (ii, i) in b.insts.iter().enumerate().rev() {
            uses_defs(i, &mut uses, &mut defs);
            let load = matches!(i, Inst::Load(..));
            let movable = is_pure(i) && !matches!(i, Inst::LoadX(..) | Inst::VLoad(..) | Inst::VGather(..) | Inst::TlsAddr(..));
            if movable && !fp_env && (!load || !memory_effect) && defs.len() == 1 && ndef[defs[0].0 as usize] == 1 {
                let s = use_block[defs[0].0 as usize];
                if s < nb && s != bi && s != 0 && npred[s] == 1 && b.term.succs().any(|t| t as usize == s)
                    && depth[bi] == depth[s]
                    && uses.iter().all(|v| written[v.0 as usize] != bi)
                {
                    cand.push((ii, s));
                    remove[ii] = true;
                }
            }
            for &d in &defs {
                written[d.0 as usize] = bi;
            }
            memory_effect |= is_mem_effect(i);
            fp_env |= matches!(i, Inst::FpEnv(..));
        }
        // A block left with only its branch compare is what jump threading duplicates into a
        // predecessor: an iterator `next` whose element copy left its guard block, once inlined
        // into a loop, became a header that threading copied into the latch (png's byte-add
        // loop lost loops.rs's "store; jump header" shape and its vectorization: 2.2x
        // instructions). Copies save nothing by moving, so they stay when the block would
        // otherwise be emptied down to its terminator's compare.
        if b.insts.len() - cand.len() <= 1 {
            for &(ii, _) in &cand {
                if matches!(b.insts[ii], Inst::Mov(..)) {
                    remove[ii] = false;
                }
            }
        }
        for &(ii, s) in &cand {
            if remove[ii] {
                arrivals[s].push((b.insts[ii].clone(), b.pos[ii]));
                changed = true;
            }
        }
        if remove.iter().any(|&r| r) {
            let b = &mut f.blocks[bi];
            let mut ii = 0;
            b.insts.retain(|_| { let keep = !remove[ii]; ii += 1; keep });
            ii = 0;
            b.pos.retain(|_| { let keep = !remove[ii]; ii += 1; keep });
        }
    }
    for (b, incoming) in f.blocks.iter_mut().zip(arrivals) {
        if !incoming.is_empty() {
            let (insts, pos): (Vec<_>, Vec<_>) = incoming.into_iter().rev().unzip();
            b.insts.splice(0..0, insts);
            b.pos.splice(0..0, pos);
        }
    }
    changed
}

fn thread_known(f: &mut Func) -> bool {
    let n = f.blocks.len();
    // a branch on a vreg whose last definition in its own block is a constant (multi-def drop
    // flags, `Iconst(c, 0); ...; Branch(c)`): a jump
    let mut folded = false;
    let (mut us, mut ds) = (Vec::new(), Vec::new());
    for b in f.blocks.iter_mut() {
        if let Term::Branch(c, t, e) = b.term {
            for i in b.insts.iter().rev() {
                if let Inst::Iconst(d, k) = i {
                    if *d == c {
                        b.term = Term::Jump(if *k != 0 { t } else { e });
                        folded = true;
                        break;
                    }
                }
                uses_defs(i, &mut us, &mut ds);
                if ds.contains(&c) {
                    break;
                }
            }
        }
    }
    // predecessors (count and the single one)
    let mut npred = vec![0u32; n];
    let mut pred1 = vec![u32::MAX; n];
    for (bi, b) in f.blocks.iter().enumerate() {
        let mut add = |t: u32| {
            npred[t as usize] += 1;
            pred1[t as usize] = bi as u32;
        };
        for t in b.term.succs() {
            add(t);
        }
    }
    // candidate branch blocks: few pure instructions, ending in Branch
    let cand: Vec<bool> = f
        .blocks
        .iter()
        .map(|b| matches!(b.term, Term::Branch(..)) && b.insts.len() <= THREAD_MAX_INSTS && b.insts.iter().all(|i| is_pure(i) && !matches!(i, Inst::Call(..))))
        .collect();
    if !cand.iter().any(|x| *x) {
        return folded;
    }
    // known integer values at a block's end (from its own constants and moves, entering with
    // its single predecessor's when it has one); computed on demand, a short chain
    fn eval(i: &Inst, kn: &mut std::collections::HashMap<u32, i64>, uses: &mut Vec<VReg>, defs: &mut Vec<VReg>) {
        let get = |kn: &std::collections::HashMap<u32, i64>, v: VReg| kn.get(&v.0).copied();
        let (d, val) = match i {
            Inst::Iconst(d, v) => (*d, Some(*v)),
            Inst::Mov(d, a) => (*d, get(kn, *a)),
            Inst::IBinI(op, it, d, a, k) => (*d, get(kn, *a).and_then(|x| fold_ibin(*op, *it, x, *k))),
            Inst::IBin(op, it, d, a, b) => (*d, match (get(kn, *a), get(kn, *b)) {
                (Some(x), Some(y)) => fold_ibin(*op, *it, x, y),
                _ => None,
            }),
            Inst::ICmpI(c, sg, d, a, k) => (*d, get(kn, *a).map(|x| cmp_i(*c, *sg, x, *k) as i64)),
            Inst::ICmp(c, sg, d, a, b) => (*d, match (get(kn, *a), get(kn, *b)) {
                (Some(x), Some(y)) => Some(cmp_i(*c, *sg, x, y) as i64),
                _ => None,
            }),
            Inst::Conv(c @ Conv::IntToInt(_), d, a) => (*d, get(kn, *a).and_then(|x| match fold_conv(*c, K::I(x)) {
                Some(K::I(v)) => Some(v),
                _ => None,
            })),
            _ => {
                if kn.is_empty() {
                    return;
                }
                uses_defs(i, uses, defs);
                for d in defs.iter() {
                    kn.remove(&d.0);
                }
                return;
            }
        };
        match val {
            Some(v) => {
                kn.insert(d.0, v);
            }
            None => {
                kn.remove(&d.0);
            }
        }
    }
    // loop headers (blocks with a predecessor they dominate): a path may not pass through
    // one, or P would enter its loop past the header (irreducible flow, which the dominator-
    // based passes -- CSE's join rules, LICM -- assume away)
    let mut header: Option<Vec<bool>> = None;
    let mut kn: std::collections::HashMap<u32, i64> = std::collections::HashMap::new();
    let (mut uses, mut defs) = (Vec::new(), Vec::new());
    let mut changed = false;
    for p in 0..n {
        // the path P jumps along: blocks of few pure instructions ending in Jump, then a branch
        let mut path: Vec<usize> = Vec::new();
        let mut ninst = 0;
        let mut x = match f.blocks[p].term {
            Term::Jump(b) => b as usize,
            _ => continue,
        };
        loop {
            if x == p || path.contains(&x) || path.len() >= 3 {
                break;
            }
            let blk = &f.blocks[x];
            ninst += blk.insts.len();
            if ninst > THREAD_MAX_INSTS || !blk.insts.iter().all(|i| is_pure(i) && !matches!(i, Inst::Call(..))) {
                break;
            }
            path.push(x);
            match blk.term {
                Term::Jump(y) => x = y as usize,
                _ => break,
            }
        }
        let b = match path.last() {
            Some(&b) if cand[b] => b,
            _ => continue,
        };
        // known values at P's end: walk up P's single-predecessor chain (bounded)
        let mut chain = vec![p];
        let mut q = p;
        while chain.len() < 4 && npred[q] == 1 && !chain.contains(&(pred1[q] as usize)) && matches!(f.blocks[pred1[q] as usize].term, Term::Jump(_)) {
            q = pred1[q] as usize;
            chain.push(q);
        }
        kn.clear();
        for &c in chain.iter().rev() {
            for i in &f.blocks[c].insts {
                eval(i, &mut kn, &mut uses, &mut defs);
            }
        }
        if kn.is_empty() {
            continue;
        }
        for &pb in &path {
            for i in &f.blocks[pb].insts {
                eval(i, &mut kn, &mut uses, &mut defs);
            }
        }
        let (c, t, e) = match f.blocks[b].term {
            Term::Branch(c, t, e) => (c, t, e),
            _ => continue,
        };
        let cv = match kn.get(&c.0) {
            Some(x) => *x,
            None => continue,
        };
        let target = if cv != 0 { t } else { e };
        if path.contains(&(target as usize)) {
            continue;
        }
        let hd = header.get_or_insert_with(|| {
            let idom = idoms(f);
            let (pre, post, _) = dom_numbers(&idom);
            let dominates = |a: usize, b: usize| pre[a] != u32::MAX && pre[b] != u32::MAX && pre[a] <= pre[b] && post[b] <= post[a];
            let mut h = vec![false; n];
            for (bi, b) in f.blocks.iter().enumerate() {
                for t in b.term.succs() {
                    if dominates(t as usize, bi) {
                        h[t as usize] = true;
                    }
                }
            }
            h
        });
        if path.iter().any(|&x| hd[x]) {
            continue;
        }
        {
            // MAKEPAD_RUN_THREAD_MAX=n: only the first n threads of the process (bisection)
            static MAX: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
            static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let m = *MAX.get_or_init(|| std::env::var("MAKEPAD_RUN_THREAD_MAX").ok().and_then(|v| v.parse().ok()).unwrap_or(u64::MAX));
            if m != u64::MAX {
                let k = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if k >= m {
                    continue;
                }
                if k + 1 == m {
                    eprintln!("thread #{} in {}: b{} -> path {:?} -> b{}\n{}", k, f.name, p, path, target, f.dump());
                }
            }
        }
        let tp = f.blocks[b].term_pos;
        // all of the path's instructions (any of them may be read later, also by the same
        // block through a loop); live_dce drops the ones this path does not need
        for &pb in &path {
            let (ii, pp) = (f.blocks[pb].insts.clone(), f.blocks[pb].pos.clone());
            f.blocks[p].insts.extend(ii);
            f.blocks[p].pos.extend(pp);
        }
        f.blocks[p].term = Term::Jump(target);
        f.blocks[p].term_pos = tp;
        changed = true;
    }
    changed || folded
}


fn same_inst(a: &Inst, b: &Inst) -> bool {
    a == b
}

fn rename(i: &Inst, a: &dyn Fn(VReg) -> VReg) -> Inst {
    // operands only: definitions keep their vreg
    match i {
        Inst::Mov(d, s) => Inst::Mov(*d, a(*s)),
        Inst::IBin(o, t, d, x, y) => Inst::IBin(*o, *t, *d, a(*x), a(*y)),
        Inst::IBinI(o, t, d, x, k) => Inst::IBinI(*o, *t, *d, a(*x), *k),
        Inst::INeg(t, d, x) => Inst::INeg(*t, *d, a(*x)),
        Inst::INot(t, d, x) => Inst::INot(*t, *d, a(*x)),
        Inst::IBit(o, d, x) => Inst::IBit(*o, *d, a(*x)),
        Inst::ICmp(c, s, d, x, y) => Inst::ICmp(*c, *s, *d, a(*x), a(*y)),
        Inst::ICmpI(c, s, d, x, k) => Inst::ICmpI(*c, *s, *d, a(*x), *k),
        Inst::FBin(o, w, d, x, y) => Inst::FBin(*o, *w, *d, a(*x), a(*y)),
        Inst::FUnary(o, w, d, x) => Inst::FUnary(*o, *w, *d, a(*x)),
        Inst::FCmp(c, w, d, x, y) => Inst::FCmp(*c, *w, *d, a(*x), a(*y)),
        Inst::Conv(c, d, x) => Inst::Conv(*c, *d, a(*x)),
        Inst::Load(m, d, b, o) => Inst::Load(*m, *d, a(*b), *o),
        Inst::Store(m, b, o, s) => Inst::Store(*m, a(*b), *o, a(*s)),
        Inst::Call(c, args, r) => {
            let c2 = match c {
                Callee::Indirect(v) => Callee::Indirect(a(*v)),
                Callee::CIndirect(v, sig) => Callee::CIndirect(a(*v), sig.clone()),
                x => x.clone(),
            };
            let mut n = Vec::new();
            for v in args {
                n.push(a(*v));
            }
            Inst::Call(c2, n, r.clone())
        }
        Inst::Copy(x, y, n) => Inst::Copy(a(*x), a(*y), *n),
        Inst::AtomicLoad(k, o, d, x) => Inst::AtomicLoad(*k, *o, *d, a(*x)),
        Inst::AtomicStore(k, o, x, v) => Inst::AtomicStore(*k, *o, a(*x), a(*v)),
        Inst::AtomicRmw(op, k, o, d, x, v) => Inst::AtomicRmw(*op, *k, *o, *d, a(*x), a(*v)),
        Inst::AtomicCas(k, o1, o2, d, x, e, n) => Inst::AtomicCas(*k, *o1, *o2, *d, a(*x), a(*e), a(*n)),
        Inst::Sel(d, c, x, y) => Inst::Sel(*d, a(*c), a(*x), a(*y)),
        Inst::VBin(o, d, x, y) => Inst::VBin(*o, *d, a(*x), a(*y)),
        Inst::VUn(o, d, x) => Inst::VUn(*o, *d, a(*x)),
        Inst::VSel(d, k, x, y) => Inst::VSel(*d, a(*k), a(*x), a(*y)),
        Inst::VFma(np, na, d, x, y, z) => Inst::VFma(*np, *na, *d, a(*x), a(*y), a(*z)),
        Inst::VSplat(d, x) => Inst::VSplat(*d, a(*x)),
        Inst::VExtract(d, x, l) => Inst::VExtract(*d, a(*x), *l),
        Inst::VInsert(d, v, l, x) => Inst::VInsert(*d, a(*v), *l, a(*x)),
        Inst::VLoad(d, b, o) => Inst::VLoad(*d, a(*b), *o),
        Inst::VStore(b, o, v) => Inst::VStore(a(*b), *o, a(*v)),
        Inst::VAny(d, x) => Inst::VAny(*d, a(*x)),
        Inst::VGather(d, b, i) => Inst::VGather(*d, a(*b), a(*i)),
        Inst::VX(op, d, x, y) => Inst::VX(*op, *d, a(*x), a(*y)),
        Inst::VX3(op, d, x, y, z) => Inst::VX3(*op, *d, a(*x), a(*y), a(*z)),
        Inst::VShift(o, d, x, k) => Inst::VShift(*o, *d, a(*x), *k),
        Inst::FMulAdd(np, na, w, d, x, y, z) => Inst::FMulAdd(*np, *na, *w, *d, a(*x), a(*y), a(*z)),
        Inst::FpEnv(true, x) => Inst::FpEnv(true, a(*x)),
        Inst::Cpuid(ds, x, y) => Inst::Cpuid(*ds, a(*x), a(*y)),
        Inst::Xgetbv(d, x) => Inst::Xgetbv(*d, a(*x)),
        other => other.clone(),
    }
}

/// Removes pure instructions whose results are unused (and `Mov(x, x)`), with a
/// worklist: removing an instruction decrements its operands' use counts.
#[inline(never)]
fn dce(f: &mut Func) -> bool {
    let reach = reachable(f);
    let nv = f.vregs.len();
    let mut cnt = vec![0u32; nv];
    // defining instruction (block, index) of single-def vregs, for the cascade
    const MULTI: u32 = u32::MAX - 1;
    let mut def_at: Vec<(u32, u32)> = vec![(u32::MAX, 0); nv];
    let mut uses = Vec::new();
    let mut total = 0usize;
    for (bi, b) in f.blocks.iter().enumerate() {
        if !reach[bi] {
            continue;
        }
        total += b.insts.len();
        for (k, i) in b.insts.iter().enumerate() {
            for_uses(i, |u| cnt[u.0 as usize] += 1);
            for_defs(i, |d| {
                let e = &mut def_at[d.0 as usize];
                *e = if e.0 == u32::MAX { (bi as u32, k as u32) } else { (MULTI, 0) };
            });
        }
        term_uses(&b.term, &mut uses);
        for u in &uses {
            cnt[u.0 as usize] += 1;
        }
    }
    if total == 0 {
        return false;
    }
    // an instruction is dead when it is a self-move or pure with every result unused; its
    // removal can make its operands' definitions dead (the set is the same fixpoint the
    // per-instruction worklist reached)
    let mut removed: Vec<Vec<bool>> = vec![Vec::new(); f.blocks.len()];
    let mut work: Vec<(u32, u32)> = Vec::new();
    let mut any = false;
    let is_dead = |i: &Inst, cnt: &[u32]| -> bool {
        match i {
            Inst::Mov(d, s) if d == s => true,
            i if is_pure(i) => {
                let mut nd = 0;
                let mut used = false;
                for_defs(i, |d| {
                    nd += 1;
                    used |= cnt[d.0 as usize] != 0;
                });
                nd > 0 && !used
            }
            _ => false,
        }
    };
    // last instruction first, cascades before the next one (the order the old all-in-one
    // worklist popped them: a multi-def operand is not revisited, so the order matters)
    for bi in (0..f.blocks.len()).rev() {
        if !reach[bi] {
            continue;
        }
        for k in (0..f.blocks[bi].insts.len()).rev() {
            work.push((bi as u32, k as u32));
            while let Some((wb, wk)) = work.pop() {
                let (wb, wk) = (wb as usize, wk as usize);
                if removed[wb].get(wk).copied().unwrap_or(false) {
                    continue;
                }
                let i = &f.blocks[wb].insts[wk];
                if !is_dead(i, &cnt) {
                    continue;
                }
                if removed[wb].is_empty() {
                    removed[wb] = vec![false; f.blocks[wb].insts.len()];
                }
                removed[wb][wk] = true;
                any = true;
                for_uses(i, |u| {
                    let c = &mut cnt[u.0 as usize];
                    *c -= 1;
                    if *c == 0 {
                        let d = def_at[u.0 as usize];
                        if d.0 < MULTI {
                            work.push(d);
                        }
                    }
                });
            }
        }
    }
    if any {
        for (bi, b) in f.blocks.iter_mut().enumerate() {
            let rs = &removed[bi];
            if rs.is_empty() {
                continue;
            }
            let mut k = 0;
            let mut j = 0;
            b.insts.retain(|_| {
                let keep = !rs[k];
                k += 1;
                keep
            });
            b.pos.retain(|_| {
                let keep = !rs[j];
                j += 1;
                keep
            });
        }
    }
    any
}

/// Redirects jumps through empty `Jump` blocks.
#[inline(never)]
fn thread_jumps(f: &mut Func) {
    let n = f.blocks.len();
    if n <= 1 {
        return;
    }
    let mut target: Vec<u32> = (0..n as u32).collect();
    for b in 0..n {
        let mut t = b as u32;
        let mut guard = 0;
        while guard < 16 {
            let blk = &f.blocks[t as usize];
            match blk.term {
                Term::Jump(x) if blk.insts.is_empty() && x != t => t = x,
                _ => break,
            }
            guard += 1;
        }
        target[b] = t;
    }
    for b in &mut f.blocks {
        b.term = match b.term.clone() {
            Term::Jump(x) => Term::Jump(target[x as usize]),
            Term::Branch(c, x, y) => {
                let (x, y) = (target[x as usize], target[y as usize]);
                if x == y {
                    Term::Jump(x)
                } else {
                    Term::Branch(c, x, y)
                }
            }
            t => t,
        };
    }
}

/// Reorders reachable blocks so a block's jump/false-branch target follows it, drops
/// unreachable blocks and puts panic paths (blocks ending in `Unreachable`) last.
pub fn relayout(f: &mut Func) {
    let n = f.blocks.len();
    if n <= 1 {
        return;
    }
    let reach = reachable(f);
    // blocks only reachable through unwind edges (cleanup pads) are cold
    let mut normal = vec![false; n];
    let mut st = vec![0u32];
    while let Some(b) = st.pop() {
        if normal[b as usize] {
            continue;
        }
        normal[b as usize] = true;
        match f.blocks[b as usize].term {
            Term::Invoke(t, _) => st.push(t),
            ref t => st.extend(t.succs()),
        }
    }
    let mut placed = vec![false; n];
    let mut order: Vec<u32> = Vec::new();
    let mut cold: Vec<u32> = Vec::new();
    let mut next = 0u32;
    let mut unplaced = 0;
    loop {
        let mut b = next;
        while !placed[b as usize] && reach[b as usize] {
            placed[b as usize] = true;
            if (matches!(f.blocks[b as usize].term, Term::Unreachable | Term::Resume) || !normal[b as usize]) && b != 0 {
                cold.push(b);
            } else {
                order.push(b);
            }
            b = match f.blocks[b as usize].term {
                Term::Jump(t) | Term::Invoke(t, _) => t,
                Term::Branch(_, t, e) => {
                    // fall through to the true successor (then-block, loop body) unless it is
                    // placed already or a panic path
                    let t_cold = matches!(f.blocks[t as usize].term, Term::Unreachable);
                    if !placed[t as usize] && !t_cold {
                        t
                    } else {
                        e
                    }
                }
                _ => break,
            };
        }
        // Placement is monotone: earlier blocks never need to be searched again.
        while unplaced < n && (placed[unplaced] || !reach[unplaced]) {
            unplaced += 1;
        }
        if unplaced == n {
            break;
        }
        next = unplaced as u32;
    }
    order.extend(cold);
    let mut map = vec![u32::MAX; n];
    for (i, b) in order.iter().enumerate() {
        map[*b as usize] = i as u32;
    }
    let old = std::mem::take(&mut f.blocks);
    let mut slots: Vec<Option<Block>> = Vec::with_capacity(n);
    for b in old {
        slots.push(Some(b));
    }
    for b in &order {
        let mut blk = slots[*b as usize].take().unwrap();
        blk.term.map_succs(|t| map[t as usize]);
        f.blocks.push(blk);
    }
}

pub fn inst_count(f: &Func) -> usize {
    let mut n = 0;
    for b in &f.blocks {
        n += b.insts.len() + 1;
    }
    n
}

/// Appends a block's single-predecessor jump target to it (straight-line code after
/// inlining and branch folding), so compares end up next to their branches.
#[inline(never)]
/// libdivide's unsigned 64-bit magic for d > 1 not a power of two: (magic, shift, add)
fn udiv_magic(d: u64) -> (u64, u32, bool) {
    let l = 63 - d.leading_zeros();
    let num: u128 = 1u128 << (64 + l);
    let mut m = (num / d as u128) as u64;
    let rem = (num % d as u128) as u64;
    let e = d - rem;
    if e < (1u64 << l) {
        (m.wrapping_add(1), l, false)
    } else {
        m = m.wrapping_add(m);
        let twice = rem.wrapping_add(rem);
        if twice >= d || twice < rem {
            m = m.wrapping_add(1);
        }
        (m.wrapping_add(1), l, true)
    }
}

/// `x / c` and `x % c` for an unsigned x and a constant c > 1: a power of two becomes a shift
/// / mask, anything else multiply-high and shifts (libdivide's u64 algorithm, exact for every
/// 64-bit x, so also for narrower zero-extended values). A divide takes 20-40 cycles.
fn div_by_const(f: &mut Func, kn: &[Option<K>], nv: usize) -> bool {
    let mut changed = false;
    for bi in 0..f.blocks.len() {
        let mut k = 0;
        while k < f.blocks[bi].insts.len() {
            // signed by a power of two 2^s (s < 31): bias = (x >>a 63) >>u (64 - s) is 2^s - 1 for
            // negative x, else 0; t = x + bias; quotient t >>a s, remainder x - (t & -2^s) (x is
            // sign-extended to 64 bits, so 64-bit steps are exact; the last step re-extends)
            if let Inst::IBin(op @ (IOp::Div | IOp::Rem), it, d, x, y) = f.blocks[bi].insts[k] {
                if it.signed && (y.0 as usize) < nv {
                    if let Some(K::I(c)) = kn[y.0 as usize] {
                        if c > 1 && (c as u64).is_power_of_two() && c.trailing_zeros() < 31 && c < (1i64 << (it.bits.min(64) - 1)) {
                            let sh = c.trailing_zeros() as i64;
                            let pos = f.blocks[bi].pos[k];
                            let (sg, bias, t) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
                            let mut seq = vec![
                                Inst::IBinI(IOp::Shr, I64, sg, x, 63),
                                Inst::IBinI(IOp::Shr, U64, bias, sg, 64 - sh),
                                Inst::IBin(IOp::Add, I64, t, x, bias),
                            ];
                            if op == IOp::Div {
                                seq.push(Inst::IBinI(IOp::Shr, it, d, t, sh));
                            } else {
                                let m = f.vreg(Cls::I);
                                seq.push(Inst::IBinI(IOp::And, I64, m, t, -c));
                                seq.push(Inst::IBin(IOp::Sub, it, d, x, m));
                            }
                            let n = seq.len();
                            f.blocks[bi].insts.splice(k..k + 1, seq);
                            f.blocks[bi].pos.splice(k..k + 1, std::iter::repeat(pos).take(n));
                            k += n;
                            changed = true;
                            continue;
                        }
                    }
                }
            }
            let (op, it, d, x, c) = match f.blocks[bi].insts[k] {
                Inst::IBin(op @ (IOp::Div | IOp::Rem), it, d, x, y) if !it.signed && (y.0 as usize) < nv => match kn[y.0 as usize] {
                    Some(K::I(c)) if c > 1 && (it.bits == 64 || (c as u64) < (1u64 << it.bits)) => (op, it, d, x, c as u64),
                    _ => {
                        k += 1;
                        continue;
                    }
                },
                _ => {
                    k += 1;
                    continue;
                }
            };
            let pos = f.blocks[bi].pos[k];
            let mut seq: Vec<Inst> = Vec::new();
            let q = if op == IOp::Div { d } else { f.vreg(Cls::I) };
            if c.is_power_of_two() {
                if op == IOp::Div {
                    seq.push(Inst::IBinI(IOp::Shr, U64, q, x, c.trailing_zeros() as i64));
                } else {
                    if c - 1 <= i32::MAX as u64 {
                        seq.push(Inst::IBinI(IOp::And, U64, d, x, (c - 1) as i64));
                    } else {
                        let mk = f.vreg(Cls::I);
                        seq.push(Inst::Iconst(mk, (c - 1) as i64));
                        seq.push(Inst::IBin(IOp::And, U64, d, x, mk));
                    }
                }
            } else {
                let (m, sh, add) = udiv_magic(c);
                let mk = f.vreg(Cls::I);
                seq.push(Inst::Iconst(mk, m as i64));
                let hi = f.vreg(Cls::I);
                seq.push(Inst::IBin(IOp::MulHi, U64, hi, x, mk));
                if add {
                    let t = f.vreg(Cls::I);
                    seq.push(Inst::IBin(IOp::Sub, U64, t, x, hi));
                    let t2 = f.vreg(Cls::I);
                    seq.push(Inst::IBinI(IOp::Shr, U64, t2, t, 1));
                    let t3 = f.vreg(Cls::I);
                    seq.push(Inst::IBin(IOp::Add, U64, t3, t2, hi));
                    seq.push(Inst::IBinI(IOp::Shr, U64, q, t3, sh as i64));
                } else {
                    seq.push(Inst::IBinI(IOp::Shr, U64, q, hi, sh as i64));
                }
                if op == IOp::Rem {
                    let p = f.vreg(Cls::I);
                    if c <= i32::MAX as u64 {
                        seq.push(Inst::IBinI(IOp::Mul, U64, p, q, c as i64));
                    } else {
                        let ck = f.vreg(Cls::I);
                        seq.push(Inst::Iconst(ck, c as i64));
                        seq.push(Inst::IBin(IOp::Mul, U64, p, q, ck));
                    }
                    seq.push(Inst::IBin(IOp::Sub, U64, d, x, p));
                }
            }
            if op == IOp::Rem && c.is_power_of_two() {
                // (the mask above is the whole sequence)
            }
            let _ = it;
            let n = seq.len();
            f.blocks[bi].insts.splice(k..k + 1, seq);
            f.blocks[bi].pos.splice(k..k + 1, std::iter::repeat(pos).take(n));
            k += n;
            changed = true;
        }
    }
    changed
}

/// Load combining: a value assembled from byte loads of consecutive addresses (`b0 | b1 << 8`,
/// four or eight bytes the same way; little- or big-endian order: `read_u16_le`,
/// `u16::from_be_bytes([p[0], p[1]])`) becomes one wide load (plus a byte swap for big
/// endian), as LLVM's load combining does. The wide load replaces the root `Or`, so the
/// bytes' loads must reach it with no memory write, call or block merge in between (the
/// root's block, then single-predecessor blocks back up to the loads), and the address
/// registers must have one definition.
fn combine_loads(f: &mut Func, ndef: &[u32]) -> bool {
    let nv = f.vregs.len();
    // single-def defining instruction per vreg: (block, index)
    let mut def_at: Vec<(u32, u32)> = vec![(u32::MAX, 0); nv];
    let mut has_or = false;
    for (bi, b) in f.blocks.iter().enumerate() {
        for (k, i) in b.insts.iter().enumerate() {
            for_defs(i, |d| {
                if ndef[d.0 as usize] == 1 {
                    def_at[d.0 as usize] = (bi as u32, k as u32);
                }
            });
            has_or |= matches!(i, Inst::IBin(IOp::Or, ..));
        }
    }
    if !has_or {
        return false;
    }
    let n = f.blocks.len();
    let mut npred = vec![0u32; n];
    let mut pred1 = vec![u32::MAX; n];
    for (bi, b) in f.blocks.iter().enumerate() {
        for t in b.term.succs() {
            npred[t as usize] += 1;
            pred1[t as usize] = bi as u32;
        }
    }
    // (byte index within the value, bytes (a 1-, 2- or 4-byte load: little endian), address
    // key: the sorted registers summed into the address, its displacement, the load's location)
    type Lane = (u32, u32, Vec<u32>, i64, (u32, u32));
    // an address as a sum of registers plus a constant (`p + i + 1` == `(p + i) + 1`)
    fn canon(f: &Func, def_at: &[(u32, u32)], v: VReg, regs: &mut Vec<u32>, depth: u32) -> Option<i64> {
        if depth > 6 || regs.len() > 4 {
            return None;
        }
        let (bi, k) = def_at[v.0 as usize];
        if bi != u32::MAX {
            match f.blocks[bi as usize].insts[k as usize] {
                Inst::IBinI(IOp::Add, it, _, x, c) if it.bits == 64 => return Some(canon(f, def_at, x, regs, depth + 1)? + c),
                Inst::Mov(_, x) => return canon(f, def_at, x, regs, depth + 1),
                Inst::IBin(IOp::Add, it, _, a, b) if it.bits == 64 => {
                    let c1 = canon(f, def_at, a, regs, depth + 1)?;
                    return Some(c1 + canon(f, def_at, b, regs, depth + 1)?);
                }
                _ => {}
            }
        }
        regs.push(v.0);
        Some(0)
    }
    fn lanes(f: &Func, def_at: &[(u32, u32)], ndef: &[u32], v: VReg, shift: u32, out: &mut Vec<Lane>, depth: u32) -> bool {
        if depth > 12 || out.len() > 8 {
            return false;
        }
        let (bi, k) = def_at[v.0 as usize];
        if bi == u32::MAX {
            return false;
        }
        match f.blocks[bi as usize].insts[k as usize] {
            Inst::Load(Mem::Int(w @ (1 | 2 | 4), false), _, base, off) if ndef[base.0 as usize] == 1 => {
                let mut regs = Vec::new();
                match canon(f, def_at, base, &mut regs, 0) {
                    Some(c) if regs.iter().all(|r| ndef[*r as usize] == 1) => {
                        regs.sort_unstable();
                        out.push((shift, w as u32, regs, off as i64 + c, (bi, k)));
                        true
                    }
                    _ => false,
                }
            }
            Inst::LoadX(Mem::Int(w @ (1 | 2 | 4), false), _, base, idx, 0, off) if ndef[base.0 as usize] == 1 && ndef[idx.0 as usize] == 1 => {
                let mut regs = Vec::new();
                let c1 = canon(f, def_at, base, &mut regs, 0);
                let c2 = canon(f, def_at, idx, &mut regs, 0);
                match (c1, c2) {
                    (Some(a), Some(b)) if regs.iter().all(|r| ndef[*r as usize] == 1) => {
                        regs.sort_unstable();
                        out.push((shift, w as u32, regs, off as i64 + a + b, (bi, k)));
                        true
                    }
                    _ => false,
                }
            }
            // every lane's bytes must survive the node's width (a narrowing conversion or shift
            // drops the bytes above it); lane indices here are relative to `shift`
            Inst::Conv(Conv::IntToInt(t), _, x) if !t.signed && t.bits >= 8 => {
                let at = out.len();
                lanes(f, def_at, ndef, x, shift, out, depth + 1) && out[at..].iter().all(|l| l.0 - shift + l.1 <= t.bits as u32 / 8)
            }
            Inst::IBinI(IOp::Shl, it, _, x, sh) if sh % 8 == 0 && sh >= 0 && sh < 64 => {
                let at = out.len();
                lanes(f, def_at, ndef, x, shift + sh as u32 / 8, out, depth + 1) && out[at..].iter().all(|l| l.0 - shift + l.1 <= it.bits as u32 / 8)
            }
            Inst::Mov(_, x) => lanes(f, def_at, ndef, x, shift, out, depth + 1),
            Inst::IBin(IOp::Or, it, _, x, y) => {
                let at = out.len();
                lanes(f, def_at, ndef, x, shift, out, depth + 1) && lanes(f, def_at, ndef, y, shift, out, depth + 1) && out[at..].iter().all(|l| l.0 - shift + l.1 <= it.bits as u32 / 8)
            }
            _ => false,
        }
    }
    let mut changed = false;
    for bi in 0..n {
        let mut k = 0;
        while k < f.blocks[bi].insts.len() {
            let (it, d) = match f.blocks[bi].insts[k] {
                Inst::IBin(IOp::Or, it, d, _, _) => (it, d),
                _ => {
                    k += 1;
                    continue;
                }
            };
            let mut ls: Vec<Lane> = Vec::new();
            let root = match f.blocks[bi].insts[k] {
                Inst::IBin(IOp::Or, _, _, x, y) => (x, y),
                _ => unreachable!(),
            };
            let ok = lanes(f, &def_at, ndef, root.0, 0, &mut ls, 0) && lanes(f, &def_at, ndef, root.1, 0, &mut ls, 0);
            let nb: usize = ls.iter().map(|l| l.1 as usize).sum();
            if !ok || !(nb == 2 || nb == 4 || nb == 8) || (it.bits as usize) < nb * 8 {
                k += 1;
                continue;
            }
            let key = ls[0].2.clone();
            // every byte of the value from exactly one lane
            let mut lanes_ok = ls.iter().all(|l| l.2 == key) && {
                let mut seen = vec![false; nb];
                ls.iter().all(|l| (l.0 + l.1) as usize <= nb && (l.0..l.0 + l.1).all(|b| !std::mem::replace(&mut seen[b as usize], true)))
            };
            // a lane at byte i sits at disp0 + i (little endian; wider lanes are little-endian
            // loads themselves) or, single bytes only, at disp0 + (n-1-i) (big endian)
            let disp_of = |i: u32| ls.iter().find(|l| l.0 == i).map(|l| l.3);
            let mut big = false;
            let mut disp0 = 0;
            if lanes_ok {
                disp0 = disp_of(0).unwrap_or(i64::MIN);
                if disp_of(0).is_some() && ls.iter().all(|l| l.3 == disp0 + l.0 as i64) {
                } else if ls.iter().all(|l| l.1 == 1) && (0..nb as u32).all(|i| disp_of(i).is_some() && disp_of(i) == disp_of(0).map(|d| d - i as i64)) {
                    big = true;
                    disp0 = disp_of(nb as u32 - 1).unwrap();
                } else {
                    lanes_ok = false;
                }
            }
            if !lanes_ok {
                k += 1;
                continue;
            }
            // the byte loads reach the root without a memory effect in between: walk back from
            // the root through single-predecessor blocks
            let mut need: Vec<(u32, u32)> = ls.iter().map(|l| l.4).collect();
            let mut safe = true;
            let mut cb = bi as u32;
            let mut upto = k;
            let mut steps = 0;
            'walk: loop {
                for j in (0..upto).rev() {
                    if let Some(p) = need.iter().position(|x| *x == (cb, j as u32)) {
                        need.swap_remove(p);
                        if need.is_empty() {
                            break 'walk;
                        }
                    }
                    if is_mem_effect(&f.blocks[cb as usize].insts[j]) {
                        safe = false;
                        break 'walk;
                    }
                }
                steps += 1;
                if steps > 4 || npred[cb as usize] != 1 || cb == 0 {
                    safe = false;
                    break;
                }
                cb = pred1[cb as usize];
                upto = f.blocks[cb as usize].insts.len();
            }
            // the address registers are defined before the root (they reach the byte loads)
            if !safe {
                k += 1;
                continue;
            }
            // the wide load uses the lowest byte's own address operands
            let low = ls.iter().find(|l| l.3 == disp0).unwrap().4;
            let mem = Mem::Int(nb as u8, false);
            let t = f.vreg(Cls::I);
            let load = match f.blocks[low.0 as usize].insts[low.1 as usize] {
                Inst::Load(_, _, base, off) => Inst::Load(mem, if big { t } else { d }, base, off),
                Inst::LoadX(_, _, base, x, sh, off) => Inst::LoadX(mem, if big { t } else { d }, base, x, sh, off),
                _ => unreachable!(),
            };
            let pos = f.blocks[bi].pos[k];
            if big {
                let s2 = f.vreg(Cls::I);
                f.blocks[bi].insts[k] = Inst::IBinI(IOp::Shr, U64, d, s2, 64 - 8 * nb as i64);
                f.blocks[bi].insts.insert(k, Inst::IBit(BitOp::Bswap, s2, t));
                f.blocks[bi].pos.insert(k, pos);
                f.blocks[bi].insts.insert(k, load);
                f.blocks[bi].pos.insert(k, pos);
                k += 3;
            } else {
                f.blocks[bi].insts[k] = load;
                k += 1;
            }
            changed = true;
        }
    }
    changed
}

/// If-conversion of branches that only choose values: `if c { x = a } else { x = b }`
/// (a diamond whose arms are a few register copies, or a triangle with one such arm) becomes
/// `x = c ? a : b` (Sel: cmov / csel) and a jump to the join -- no branch to mispredict
/// (rustc / LLVM SimplifyCFG speculates such arms into selects). The arms must have the
/// branch as their only predecessor, copy into distinct registers and read none of them.
fn if_convert(f: &mut Func) -> bool {
    const MAX_MOVS: usize = 3;
    let n = f.blocks.len();
    if n < 3 {
        return false;
    }
    let reach = reachable(f);
    let mut npred = vec![0u32; n];
    for (bi, b) in f.blocks.iter().enumerate() {
        if reach[bi] {
            for t in b.term.succs() {
                npred[t as usize] += 1;
            }
        }
    }
    // an arm: (copies, join) when block x is only copies then a jump, entered only from the branch
    let arm = |f: &Func, npred: &[u32], x: u32| -> Option<(Vec<(VReg, VReg)>, u32)> {
        let b = &f.blocks[x as usize];
        if npred[x as usize] != 1 || b.insts.len() > MAX_MOVS {
            return None;
        }
        let j = match b.term {
            Term::Jump(j) if j != x => j,
            _ => return None,
        };
        let mut mv = Vec::new();
        for i in &b.insts {
            match i {
                Inst::Mov(d, s) if d != s && matches!(f.vregs[d.0 as usize], Cls::I | Cls::F32 | Cls::F64) => mv.push((*d, *s)),
                _ => return None,
            }
        }
        Some((mv, j))
    };
    let mut changed = false;
    for bi in 0..n {
        if !reach[bi] {
            continue;
        }
        let (c, t, e) = match f.blocks[bi].term {
            Term::Branch(c, t, e) if t != e && t as usize != bi && e as usize != bi => (c, t, e),
            _ => continue,
        };
        let (mt, me, j) = match (arm(f, &npred, t), arm(f, &npred, e)) {
            (Some((mt, jt)), Some((me, je))) if jt == je => (mt, me, jt),
            (Some((mt, jt)), _) if jt == e => (mt, Vec::new(), e),
            (_, Some((me, je))) if je == t => (Vec::new(), me, t),
            _ => continue,
        };
        if mt.is_empty() && me.is_empty() {
            continue;
        }
        // destinations, each once per arm; sources and the condition are not among them
        let mut dests: Vec<VReg> = Vec::new();
        let mut ok = true;
        for arm_movs in [&mt, &me] {
            for (k, (d, _)) in arm_movs.iter().enumerate() {
                if arm_movs[..k].iter().any(|x| x.0 == *d) {
                    ok = false;
                }
                if !dests.contains(d) {
                    dests.push(*d);
                }
            }
        }
        if !ok || dests.contains(&c) || mt.iter().chain(me.iter()).any(|(_, s)| dests.contains(s)) {
            continue;
        }
        // a choice the join then branches on stays a branch: the branch can be threaded to
        // its known outcome on each arm (an Option's Some / None test after a select would
        // turn a threadable branch into a select plus a test)
        {
            let mut taint = dests.clone();
            let mut seen = Vec::new();
            let mut at = j;
            let mut feeds_branch = false;
            loop {
                if seen.contains(&at) {
                    break;
                }
                seen.push(at);
                for i in &f.blocks[at as usize].insts {
                    let mut hit = false;
                    for_uses(i, |u| hit |= taint.contains(&u));
                    if hit {
                        for_defs(i, |d| taint.push(d));
                    }
                }
                match f.blocks[at as usize].term {
                    Term::Jump(next) => at = next,
                    Term::Branch(jc, ..) => {
                        feeds_branch = taint.contains(&jc);
                        break;
                    }
                    _ => break,
                }
            }
            if feeds_branch {
                continue;
            }
        }
        let pos = f.blocks[bi].term_pos;
        for d in dests {
            let a = mt.iter().find(|x| x.0 == d).map_or(d, |x| x.1);
            let b = me.iter().find(|x| x.0 == d).map_or(d, |x| x.1);
            // the same value on both paths: a plain copy (none if it is d's own)
            let i = if a == b { Inst::Mov(d, a) } else { Inst::Sel(d, c, a, b) };
            if a == d && b == d {
                continue;
            }
            f.blocks[bi].insts.push(i);
            f.blocks[bi].pos.push(pos);
        }
        f.blocks[bi].term = Term::Jump(j);
        // the arms are entered from here only: now unreachable
        npred[t as usize] = 0;
        npred[e as usize] = npred[e as usize].saturating_sub(1);
        changed = true;
    }
    changed
}

fn merge_blocks(f: &mut Func) -> bool {
    let n = f.blocks.len();
    if n <= 1 {
        return false;
    }
    let reach = reachable(f);
    let mut preds = vec![0u32; n];
    for (bi, b) in f.blocks.iter().enumerate() {
        if !reach[bi] {
            continue;
        }
        for t in b.term.succs() {
            preds[t as usize] += 1;
        }
    }
    preds[0] += 1; // entry
    let mut changed = false;
    for bi in 0..n {
        if !reach[bi] {
            continue;
        }
        loop {
            let t = match f.blocks[bi].term {
                Term::Jump(t) if t as usize != bi && preds[t as usize] == 1 => t as usize,
                _ => break,
            };
            let tb = std::mem::replace(&mut f.blocks[t], Block { insts: Vec::new(), pos: Vec::new(), term: Term::Unreachable, term_pos: 0 });
            let b = &mut f.blocks[bi];
            b.insts.extend(tb.insts);
            b.pos.extend(tb.pos);
            b.term = tb.term;
            b.term_pos = tb.term_pos;
            preds[t] = 0;
            changed = true;
        }
    }
    changed
}

/// Moves single-use constant definitions next to their use in the same block, so
/// constants do not occupy registers across long expressions.
pub fn sink_consts(f: &mut Func) {
    let nv = f.vregs.len();
    let mut ndef = vec![0u32; nv];
    let mut nuse = vec![0u32; nv];
    let mut uses = Vec::new();
    let mut defs = Vec::new();
    for p in &f.params {
        ndef[p.0 as usize] += 1;
    }
    for b in &f.blocks {
        for i in &b.insts {
            for_uses(i, |u| {
                nuse[u.0 as usize] += 1;
            });
            for_defs(i, |d| {
                ndef[d.0 as usize] += 1;
            });
        }
        term_uses(&b.term, &mut uses);
        for u in &uses {
            nuse[u.0 as usize] += 1;
        }
    }
    // constants used only on a cold path (a block ending in Unreachable: panics) move there
    let nb = f.blocks.len();
    let mut use_block = vec![u32::MAX; nv]; // u32::MAX-1: several blocks
    for (bi, b) in f.blocks.iter().enumerate() {
        for i in &b.insts {
            uses_defs(i, &mut uses, &mut defs);
            for u in &uses {
                let ub = &mut use_block[u.0 as usize];
                if *ub == u32::MAX {
                    *ub = bi as u32;
                } else if *ub != bi as u32 {
                    *ub = u32::MAX - 1;
                }
            }
        }
        term_uses(&b.term, &mut uses);
        for u in &uses {
            let ub = &mut use_block[u.0 as usize];
            if *ub == u32::MAX {
                *ub = bi as u32;
            } else if *ub != bi as u32 {
                *ub = u32::MAX - 1;
            }
        }
    }
    for bi in 0..nb {
        let mut k = 0;
        while k < f.blocks[bi].insts.len() {
            let d = match &f.blocks[bi].insts[k] {
                Inst::Fconst(d, _, _) | Inst::Iconst(d, _) | Inst::Addr(d, _) if ndef[d.0 as usize] == 1 => *d,
                _ => {
                    k += 1;
                    continue;
                }
            };
            let ub = use_block[d.0 as usize];
            if ub < nb as u32 && ub != bi as u32 && matches!(f.blocks[ub as usize].term, Term::Unreachable | Term::Resume) {
                let i = f.blocks[bi].insts.remove(k);
                let p = f.blocks[bi].pos.remove(k);
                f.blocks[ub as usize].insts.insert(0, i);
                f.blocks[ub as usize].pos.insert(0, p);
            } else {
                k += 1;
            }
        }
    }
    // a stack slot's address used in other blocks is computed again where it is used (a
    // frame-pointer add): one long-lived register per escaping slot from the entry block
    // (SROA's write-back addresses, lowering's slots) spilled and reloaded otherwise
    let mut slot_of: Vec<u32> = vec![u32::MAX; nv];
    let mut any_slot = false;
    for b in &f.blocks {
        for i in &b.insts {
            if let Inst::SlotAddr(d, sl) = i {
                if ndef[d.0 as usize] == 1 {
                    slot_of[d.0 as usize] = *sl;
                    any_slot = true;
                }
            }
        }
    }
    if any_slot {
        // per block: the slot-address vregs it uses that are defined elsewhere
        let mut def_block = vec![u32::MAX; nv];
        for (bi, b) in f.blocks.iter().enumerate() {
            for i in &b.insts {
                if let Inst::SlotAddr(d, _) = i {
                    if slot_of[d.0 as usize] != u32::MAX {
                        def_block[d.0 as usize] = bi as u32;
                    }
                }
            }
        }
        let mut still_used = vec![false; nv];
        for bi in 0..nb {
            let mut map: Vec<(VReg, VReg)> = Vec::new();
            let mut k = 0;
            while k < f.blocks[bi].insts.len() {
                let mut need: Vec<VReg> = Vec::new();
                for_uses(&f.blocks[bi].insts[k], |u| {
                    if slot_of.get(u.0 as usize).map_or(false, |x| *x != u32::MAX) && def_block[u.0 as usize] != bi as u32 && !need.contains(&u) && !map.iter().any(|x| x.0 == u) {
                        need.push(u);
                    }
                });
                for u in need {
                    let nvr = f.vreg(Cls::I);
                    let p = f.blocks[bi].pos[k];
                    f.blocks[bi].insts.insert(k, Inst::SlotAddr(nvr, slot_of[u.0 as usize]));
                    f.blocks[bi].pos.insert(k, p);
                    k += 1;
                    map.push((u, nvr));
                }
                if !map.is_empty() {
                    f.blocks[bi].insts[k].vregs_mut(|v| {
                        if let Some(x) = map.iter().find(|x| x.0 == *v) {
                            *v = x.1;
                        }
                    });
                }
                for_uses(&f.blocks[bi].insts[k], |u| {
                    if slot_of.get(u.0 as usize).map_or(false, |x| *x != u32::MAX) && def_block[u.0 as usize] == bi as u32 {
                        still_used[u.0 as usize] = true;
                    }
                });
                k += 1;
            }
            term_uses(&f.blocks[bi].term, &mut uses);
            for u in uses.clone() {
                if !slot_of.get(u.0 as usize).map_or(false, |x| *x != u32::MAX) {
                    continue;
                }
                if def_block[u.0 as usize] == bi as u32 {
                    still_used[u.0 as usize] = true;
                    continue;
                }
                let nvr = match map.iter().find(|x| x.0 == u) {
                    Some(x) => x.1,
                    None => {
                        let nvr = f.vreg(Cls::I);
                        let at = if matches!(f.blocks[bi].term, Term::Invoke(..)) && !f.blocks[bi].insts.is_empty() { f.blocks[bi].insts.len() - 1 } else { f.blocks[bi].insts.len() };
                        let p = f.blocks[bi].term_pos;
                        f.blocks[bi].insts.insert(at, Inst::SlotAddr(nvr, slot_of[u.0 as usize]));
                        f.blocks[bi].pos.insert(at, p);
                        map.push((u, nvr));
                        nvr
                    }
                };
                match &mut f.blocks[bi].term {
                    Term::Branch(c, _, _) if *c == u => *c = nvr,
                    Term::Ret(vs) => vs.iter_mut().for_each(|v| {
                        if *v == u {
                            *v = nvr
                        }
                    }),
                    _ => {}
                }
            }
        }
        // definitions nothing in their own block reads any more
        for b in &mut f.blocks {
            let mut k = 0;
            while k < b.insts.len() {
                if let Inst::SlotAddr(d, _) = b.insts[k] {
                    if slot_of.get(d.0 as usize).map_or(false, |x| *x != u32::MAX) && !still_used[d.0 as usize] {
                        b.insts.remove(k);
                        b.pos.remove(k);
                        continue;
                    }
                }
                k += 1;
            }
        }
    }
    let nv = f.vregs.len();
    let mut nuse = vec![0u32; nv];
    let mut ndef = vec![0u32; nv];
    for p in &f.params {
        ndef[p.0 as usize] += 1;
    }
    for b in &f.blocks {
        for i in &b.insts {
            for_uses(i, |u| nuse[u.0 as usize] += 1);
            for_defs(i, |d| ndef[d.0 as usize] += 1);
        }
        term_uses(&b.term, &mut uses);
        for u in &uses {
            nuse[u.0 as usize] += 1;
        }
    }
    for b in &mut f.blocks {
        let n = b.insts.len();
        let mut out: Vec<Inst> = Vec::with_capacity(n);
        let mut out_pos: Vec<u64> = Vec::with_capacity(n);
        // constants waiting for their use: (vreg, inst, pos)
        let mut held: Vec<(u32, Inst, u64)> = Vec::new();
        for k in 0..n {
            let i = b.insts[k].clone();
            let p = b.pos[k];
            let movable = match &i {
                Inst::Fconst(d, _, _) | Inst::Iconst(d, _) | Inst::Addr(d, _) | Inst::SlotAddr(d, _) => ndef[d.0 as usize] == 1 && nuse[d.0 as usize] == 1,
                _ => false,
            };
            if movable {
                let d = match &i {
                    Inst::Fconst(d, _, _) | Inst::Iconst(d, _) | Inst::Addr(d, _) | Inst::SlotAddr(d, _) => d.0,
                    _ => 0,
                };
                held.push((d, i, p));
                continue;
            }
            uses_defs(&i, &mut uses, &mut defs);
            for u in &uses {
                let mut j = 0;
                while j < held.len() {
                    if held[j].0 == u.0 {
                        let (_, hi, hp) = held.remove(j);
                        out.push(hi);
                        out_pos.push(hp);
                    } else {
                        j += 1;
                    }
                }
            }
            out.push(i);
            out_pos.push(p);
        }
        // the rest are used by the terminator or in other blocks (before an invoked call,
        // which stays last)
        let at = if matches!(b.term, Term::Invoke(..)) && !out.is_empty() { out.len() - 1 } else { out.len() };
        for (k, (_, hi, hp)) in held.into_iter().enumerate() {
            out.insert(at + k, hi);
            out_pos.insert(at + k, hp);
        }
        b.insts = out;
        b.pos = out_pos;
    }
}

/// Within a block, uses of `a` after `Mov(a, b)` read `b` directly until either is
/// redefined (handles copies of multi-definition vregs that global propagation skips).
#[inline(never)]
/// Forward little-endian words assembled by byte stores. Track the current values of
/// mutable RIR registers, and analyse all replacements before changing instruction indices.
pub fn forward_narrow_stores(f: &mut Func) -> bool {
    let nv = f.vregs.len();
    let mut ndef = vec![0u32; nv];
    let mut defs = vec![None; nv];
    for p in &f.params {
        ndef[p.0 as usize] += 1;
    }
    let mut preds = vec![Vec::new(); f.blocks.len()];
    for (bi, b) in f.blocks.iter().enumerate() {
        for t in b.term.succs() {
            preds[t as usize].push(bi);
        }
        for (ii, inst) in b.insts.iter().enumerate() {
            for_defs(inst, |d| {
                ndef[d.0 as usize] += 1;
                defs[d.0 as usize] = Some((bi, ii));
            });
        }
    }
    // Resolve simple address derivations. Unknown provenance must not justify moving
    // a load across stores: it might point back into the destination slot.
    fn root(f: &Func, defs: &[Option<(usize, usize)>], ndef: &[u32], v: VReg, depth: usize) -> Option<(bool, u32)> {
        if depth > 32 || ndef[v.0 as usize] != 1 {
            return None;
        }
        let Some((bi, ii)) = defs[v.0 as usize] else { return Some((false, v.0)) };
        match f.blocks[bi].insts[ii] {
            Inst::SlotAddr(_, sl) => Some((true, sl)),
            Inst::Mov(_, a) | Inst::IBinI(IOp::Add | IOp::Sub, IntTy { bits: 64, .. }, _, a, _) => root(f, defs, ndef, a, depth + 1),
            _ => None,
        }
    }
    let sc = slot_scan(f);
    // a base address: the slot for a SlotAddr (several SlotAddr vregs of one slot are one base),
    // else the vreg
    let canon = |v: VReg| -> (bool, u32) {
        if ndef[v.0 as usize] == 1 {
            if let Some((bi, ii)) = defs[v.0 as usize] {
                if let Inst::SlotAddr(_, sl) = f.blocks[bi].insts[ii] {
                    return (true, sl);
                }
            }
        }
        (false, v.0)
    };
    // (value, store index, value version)
    type Byte = (VReg, usize, u32);
    let mut edits = Vec::new();
    let mut versions = vec![0u32; nv];
    for (bi, b) in f.blocks.iter().enumerate() {
        let mut base = None;
        let mut bytes = std::collections::HashMap::<i64, Byte>::new();
        for (li, inst) in b.insts.iter().enumerate() {
            if let Inst::Load(m, d, a, off) = *inst {
                let n = mem_size(m) as usize;
                if matches!(n, 2 | 4 | 8) && base == Some(canon(a)) {
                    let lanes: Option<Vec<Byte>> = (0..n).map(|i| bytes.get(&(off as i64 + i as i64)).copied()).collect();
                    if let Some(lanes) = lanes {
                        if lanes.iter().all(|&(v, _, ver)| versions[v.0 as usize] == ver) {
                            let direct = (|| {
                                let mut loads = Vec::new();
                                let mut source = None;
                                for (i, &(v, si, _)) in lanes.iter().enumerate() {
                                    if ndef[v.0 as usize] != 1 { return None; }
                                    let loc = defs[v.0 as usize]?;
                                    let Inst::Load(Mem::Int(1, false), _, p, k) = f.blocks[loc.0].insts[loc.1] else { return None };
                                    let key = (p, k as i64 - i as i64);
                                    if source.is_some_and(|s| s != key) || (loc.0 == bi && loc.1 >= si) { return None; }
                                    source = Some(key);
                                    loads.push(loc);
                                }
                                let (p, k) = source?;
                                // the word's source may be anything (a loop's pointer) when the
                                // destination is a stack slot whose address goes nowhere but load /
                                // store bases: nothing else can point into it
                                let br = root(f, &defs, &ndef, a, 0)?;
                                let pr = match root(f, &defs, &ndef, p, 0) {
                                    Some(r) => r,
                                    None if br.0 && !sc.esc[br.1 as usize] => (false, u32::MAX),
                                    None => return None,
                                };
                                if pr == br || !(pr.0 || br.0 || f.noalias.contains(&p) || f.noalias.contains(&a)) { return None; }
                                // Follow single-predecessor blocks (a Branch may have another
                                // successor, but the next block has just one entry).
                                let mut chain = vec![bi];
                                while !loads.iter().all(|l| chain.contains(&l.0)) {
                                    let cur = *chain.last()?;
                                    if cur == 0 || preds[cur].len() != 1 { return None; }
                                    let pred = preds[cur][0];
                                    if chain.contains(&pred) || !matches!(f.blocks[pred].term, Term::Jump(_) | Term::Branch(..)) { return None; }
                                    chain.push(pred);
                                }
                                chain.reverse();
                                let first = loads.iter().map(|&(lb, ii)| (chain.iter().position(|&c| c == lb).unwrap(), ii)).min()?;
                                for (ci, &cb) in chain.iter().enumerate().skip(first.0) {
                                    let start = if ci == first.0 { first.1 } else { 0 };
                                    let end = if cb == bi { li } else { f.blocks[cb].insts.len() };
                                    for ii in start..end {
                                        if is_mem_effect(&f.blocks[cb].insts[ii]) && !(cb == bi && lanes.iter().any(|l| l.1 == ii)) { return None; }
                                    }
                                }
                                Some(Inst::Load(m, d, p, i32::try_from(k).ok()?))
                            })();
                            edits.push((bi, li, m, d, lanes, direct));
                        }
                    }
                }
            }
            match *inst {
                Inst::Store(m, a, off, v) => {
                    if base != Some(canon(a)) {
                        bytes.clear();
                        base = Some(canon(a));
                    }
                    for i in 0..mem_size(m) {
                        bytes.remove(&(off as i64 + i as i64));
                    }
                    if matches!(m, Mem::Int(1, _)) {
                        bytes.insert(off as i64, (v, li, versions[v.0 as usize]));
                    }
                }
                _ if is_mem_effect(inst) => bytes.clear(),
                _ => {}
            }
            for_defs(inst, |d| {
                versions[d.0 as usize] += 1;
                if base == Some((false, d.0)) { bytes.clear(); }
            });
        }
    }
    let changed = !edits.is_empty();
    for (bi, li, m, d, lanes, direct) in edits.into_iter().rev() {
        let mut ins = Vec::new();
        if let Some(load) = direct {
            ins.push(load);
        } else {
            let ty = IntTy { bits: mem_size(m) as u8 * 8, signed: false };
            let mut acc = None;
            for (i, (v, _, _)) in lanes.into_iter().enumerate() {
                // A byte store discards all high bits, even for signed or wide inputs.
                let byte = f.vreg(Cls::I);
                ins.push(Inst::Conv(Conv::IntToInt(IntTy { bits: 8, signed: false }), byte, v));
                let shifted = if i == 0 { byte } else {
                    let t = f.vreg(Cls::I);
                    ins.push(Inst::IBinI(IOp::Shl, ty, t, byte, (8 * i) as i64));
                    t
                };
                acc = Some(if let Some(prev) = acc {
                    let t = f.vreg(Cls::I);
                    ins.push(Inst::IBin(IOp::Or, ty, t, prev, shifted));
                    t
                } else { shifted });
            }
            let conv = match m {
                Mem::Int(n, signed) => Conv::IntToInt(IntTy { bits: n * 8, signed }),
                Mem::F32 => Conv::BitsToF(false),
                Mem::F64 => Conv::BitsToF(true),
            };
            ins.push(Inst::Conv(conv, d, acc.unwrap()));
        }
        let b = &mut f.blocks[bi];
        let pos = b.pos[li];
        b.pos.splice(li..=li, std::iter::repeat(pos).take(ins.len()));
        b.insts.splice(li..=li, ins);
    }
    changed
}

fn local_copy_prop(f: &mut Func) -> bool {
    let mut changed = false;
    let mut uses = Vec::new();
    let mut defs = Vec::new();
    let mut map: Vec<(VReg, VReg)> = Vec::new();
    for b in &mut f.blocks {
        map.clear();
        for k in 0..b.insts.len() {
            if !map.is_empty() {
                let look = |v: VReg| -> VReg {
                    for (a, s) in map.iter() {
                        if *a == v {
                            return *s;
                        }
                    }
                    v
                };
                let n = rename(&b.insts[k], &look);
                if n != b.insts[k] {
                    b.insts[k] = n;
                    changed = true;
                }
            }
            uses_defs(&b.insts[k], &mut uses, &mut defs);
            for d in &defs {
                map.retain(|(a, s)| a != d && s != d);
            }
            if let Inst::Mov(d, s) = b.insts[k] {
                if d != s {
                    map.push((d, s));
                }
            }
        }
        if !map.is_empty() {
            let look = |v: VReg| -> VReg {
                for (a, s) in map.iter() {
                    if *a == v {
                        return *s;
                    }
                }
                v
            };
            let t = match &b.term {
                Term::Branch(c, x, y) => Term::Branch(look(*c), *x, *y),
                Term::Ret(v) => {
                    let mut n = Vec::new();
                    for x in v {
                        n.push(look(*x));
                    }
                    Term::Ret(n)
                }
                t => t.clone(),
            };
            b.term = t;
        }
    }
    changed
}

/// Immediate dominators (Cooper/Harvey/Kennedy) over reachable blocks; entry's idom is itself.
pub fn idoms(f: &Func) -> Vec<u32> {
    let n = f.blocks.len();
    if n == 1 {
        return vec![0];
    }
    // reverse postorder
    let mut rpo: Vec<u32> = Vec::with_capacity(n);
    let mut seen = vec![false; n];
    let mut stack: Vec<(u32, u8)> = Vec::with_capacity(2 * n);
    stack.push((0, 0));
    while let Some((b, st)) = stack.pop() {
        if st == 0 {
            if seen[b as usize] {
                continue;
            }
            seen[b as usize] = true;
            stack.push((b, 1));
            // pushed in reverse: the first successor is visited first
            let succ: [Option<u32>; 2] = {
                let mut it = f.blocks[b as usize].term.succs();
                [it.next(), it.next()]
            };
            for t in succ.iter().rev().flatten() {
                stack.push((*t, 0));
            }
        } else {
            rpo.push(b);
        }
    }
    rpo.reverse();
    let mut order = vec![u32::MAX; n];
    for (i, b) in rpo.iter().enumerate() {
        order[*b as usize] = i as u32;
    }
    // predecessors in reverse postorder, flat (pred_off[b]..pred_off[b + 1])
    let mut pred_off = vec![0u32; n + 1];
    for &b in &rpo {
        for t in f.blocks[b as usize].term.succs() {
            pred_off[t as usize + 1] += 1;
        }
    }
    for i in 0..n {
        pred_off[i + 1] += pred_off[i];
    }
    let mut fill = pred_off.clone();
    let mut preds = vec![0u32; pred_off[n] as usize];
    for &b in &rpo {
        for t in f.blocks[b as usize].term.succs() {
            preds[fill[t as usize] as usize] = b;
            fill[t as usize] += 1;
        }
    }
    let mut idom = vec![u32::MAX; n];
    idom[0] = 0;
    let mut changed = true;
    while changed {
        changed = false;
        for &b in rpo.iter().skip(1) {
            let mut new = u32::MAX;
            for &p in &preds[pred_off[b as usize] as usize..pred_off[b as usize + 1] as usize] {
                if idom[p as usize] == u32::MAX {
                    continue;
                }
                if new == u32::MAX {
                    new = p;
                } else {
                    let (mut x, mut y) = (p, new);
                    while x != y {
                        while order[x as usize] > order[y as usize] {
                            x = idom[x as usize];
                        }
                        while order[y as usize] > order[x as usize] {
                            y = idom[y as usize];
                        }
                    }
                    new = x;
                }
            }
            if new != u32::MAX && idom[b as usize] != new {
                idom[b as usize] = new;
                changed = true;
            }
        }
    }
    idom
}

/// Dominator-tree pre/post numbers: `a` dominates `b` iff pre[a] <= pre[b] && post[b] <= post[a].
/// Dominator-tree children, flat: block b's children (ascending) are
/// `child[off[b]..off[b + 1]]`.
pub(crate) struct DomKids {
    off: Vec<u32>,
    child: Vec<u32>,
}

impl std::ops::Index<usize> for DomKids {
    type Output = [u32];
    #[inline]
    fn index(&self, b: usize) -> &[u32] {
        &self.child[self.off[b] as usize..self.off[b + 1] as usize]
    }
}

pub(crate) fn dom_numbers(idom: &[u32]) -> (Vec<u32>, Vec<u32>, DomKids) {
    let n = idom.len();
    let mut off = vec![0u32; n + 1];
    for b in 1..n {
        if idom[b] != u32::MAX && idom[b] as usize != b {
            off[idom[b] as usize + 1] += 1;
        }
    }
    for i in 0..n {
        off[i + 1] += off[i];
    }
    let mut fill = off.clone();
    let mut child = vec![0u32; off[n] as usize];
    for b in 1..n {
        if idom[b] != u32::MAX && idom[b] as usize != b {
            let p = idom[b] as usize;
            child[fill[p] as usize] = b as u32;
            fill[p] += 1;
        }
    }
    let children = DomKids { off, child };
    let mut pre = vec![u32::MAX; n];
    let mut post = vec![0u32; n];
    let mut clock = 0u32;
    let mut stack: Vec<(u32, bool)> = Vec::with_capacity(2 * n);
    stack.push((0, false));
    while let Some((b, done)) = stack.pop() {
        if done {
            post[b as usize] = clock;
            clock += 1;
            continue;
        }
        pre[b as usize] = clock;
        clock += 1;
        stack.push((b, true));
        for &c in children[b as usize].iter().rev() {
            stack.push((c, false));
        }
    }
    (pre, post, children)
}

/// Path state of the CSE walk kept in per-vreg arrays (set and cleared with an undo log,
/// so entering and leaving a dominator subtree costs only what it changed).
struct Facts {
    /// branch facts: vreg has this value
    fact: Vec<Option<i64>>,
    /// unsigned upper bound: vreg <u bound (the tightest known)
    ub: Vec<Option<i64>>,
    /// unsigned lower bound: vreg >u bound (the tightest known)
    lb: Vec<Option<i64>>,
    /// (which array, vreg, previous value)
    undo: Vec<(u8, u32, Option<i64>)>,
    /// multi-definition vregs with a fact set on the current path (a join re-checks them)
    multi: Vec<u32>,
    /// undo length -> multi length marks
    multi_undo: Vec<(usize, usize)>,
}

impl Facts {
    fn arr(&mut self, k: u8) -> &mut Vec<Option<i64>> {
        match k {
            0 => &mut self.fact,
            1 => &mut self.ub,
            _ => &mut self.lb,
        }
    }
    fn set(&mut self, k: u8, v: VReg, x: Option<i64>, multi: bool) {
        let old = self.arr(k)[v.0 as usize];
        if old == x {
            return;
        }
        self.undo.push((k, v.0, old));
        self.arr(k)[v.0 as usize] = x;
        if multi && x.is_some() {
            self.multi_undo.push((self.undo.len(), self.multi.len()));
            self.multi.push(v.0);
        }
    }
    fn tighten_ub(&mut self, v: VReg, b: i64, multi: bool) {
        match self.ub[v.0 as usize] {
            Some(o) if (o as u64) <= (b as u64) => {}
            _ => self.set(1, v, Some(b), multi),
        }
    }
    fn tighten_lb(&mut self, v: VReg, b: i64, multi: bool) {
        match self.lb[v.0 as usize] {
            Some(o) if (o as u64) >= (b as u64) => {}
            _ => self.set(2, v, Some(b), multi),
        }
    }
    fn clear(&mut self, v: VReg) {
        for k in 0..3 {
            if self.arr(k)[v.0 as usize].is_some() {
                self.set(k, v, None, false);
            }
        }
    }
    fn rewind(&mut self, mark: usize) {
        while self.undo.len() > mark {
            let (k, v, old) = self.undo.pop().unwrap();
            self.arr(k)[v as usize] = old;
        }
        while let Some(&(u, m)) = self.multi_undo.last() {
            if u <= mark {
                break;
            }
            self.multi.truncate(m);
            self.multi_undo.pop();
        }
    }
}

#[derive(Clone, Default)]
struct CseState {
    /// (expression key, dest, operands, is load)
    avail: Vec<([u64; 3], VReg, [VReg; 2], bool)>,
    copies: Vec<(VReg, VReg)>,
}

enum CseWork {
    /// block, its inherited state, the edge it was entered by (Some((cond vreg, value,
    /// compare behind the cond)) for a direct child; None for a join child)
    Enter(u32, CseState, Option<Option<(VReg, i64, Option<(Cond, VReg, i64)>)>>, u32),
    Leave(usize),
}

/// CSE over the dominator tree. A block entered through its only predecessor keeps all
/// state of that predecessor plus the branch fact of the edge (`c` true/false, and for an
/// unsigned `x < k` compare the bound `x <u k`). A block reached from its immediate
/// dominator through a join keeps only state about single-definition vregs (their values
/// cannot change on the way). Repeated compares fold, so bounds checks implied by a loop
/// condition (also through copies of the loop variable) disappear.
/// Cost: linear in the function plus, per join, the multi-definition facts on the path.
/// Whether local_cse can change anything: it replaces an expression whose key an earlier
/// one (or a store, for a load) produced, renames uses of `Mov` copies, and folds with
/// facts that only branches create. Without a branch, a copy or a repeated key it is a no-op.
fn cse_can_change(f: &Func) -> bool {
    let mut keys: Vec<[u64; 3]> = Vec::new();
    for b in &f.blocks {
        if matches!(b.term, Term::Branch(..)) {
            return true;
        }
        for i in &b.insts {
            match i {
                Inst::Mov(..) => return true,
                Inst::Store(m, base, off, _) => {
                    if let Some(k) = cse_key(&Inst::Load(*m, VReg(0), *base, *off)) {
                        keys.push(k);
                    }
                }
                Inst::SlotAddr(..) => {}
                i if is_mem_effect(i) => {}
                _ => {
                    if let Some(k) = cse_key(i) {
                        keys.push(k);
                    }
                }
            }
        }
    }
    keys.sort_unstable();
    keys.windows(2).any(|w| w[0] == w[1])
}

fn local_cse(options: &relax_session::Options, f: &mut Func) -> bool {
    if !cse_can_change(f) {
        return false;
    }
    let n = f.blocks.len();
    let nv = f.vregs.len();
    let mut uses = Vec::new();
    let mut defs = Vec::new();
    let mut ndef = vec![0u32; nv];
    for p in &f.params {
        ndef[p.0 as usize] += 1;
    }
    // store forwarding / load invalidation needs pointer roots only when both occur
    let (mut has_load, mut has_store) = (false, false);
    for b in &f.blocks {
        for i in &b.insts {
            match i {
                Inst::Load(..) => has_load = true,
                Inst::Store(..) => has_store = true,
                _ => {}
            }
            for_defs(i, |d| ndef[d.0 as usize] += 1);
        }
    }
    let idom = idoms(f);
    let mut npred = vec![0u32; n];
    for (bi, b) in f.blocks.iter().enumerate() {
        if idom[bi] == u32::MAX {
            continue;
        }
        for t in b.term.succs() {
            npred[t as usize] += 1;
        }
    }
    let (pre, post, children) = dom_numbers(&idom);
    // cleanup pads (unwind targets) and what they dominate run only while unwinding: no CSE
    // there (cold; pads are a tenth of the code with unwinding on)
    let mut pad = vec![false; n];
    for b in &f.blocks {
        if let Term::Invoke(_, u) = b.term {
            pad[u as usize] = true;
        }
    }
    let single = |v: VReg| ndef[v.0 as usize] == 1;
    // pointer roots of single-def address vregs: 1+slot for stack slots, 1<<40|vreg for
    // noalias params, 0 unknown
    let mut root = vec![0u64; nv];
    for p in &f.noalias {
        if ndef[p.0 as usize] == 1 {
            root[p.0 as usize] = (1u64 << 40) | p.0 as u64;
        }
    }
    // LOADED: a pointer that is not derived from a noalias param by address arithmetic (other
    // params, loaded pointers, call results). It may point anywhere except into a noalias
    // param's memory (rustc marks `&mut T` (T: Unpin) and frozen `&T` params LLVM `noalias`:
    // rustc_ty_utils/src/abi.rs, `no_alias`; LLVM's "based on" does not pass through memory).
    const LOADED: u64 = 3u64 << 40;
    if has_load && has_store {
        for p in &f.params {
            if ndef[p.0 as usize] == 1 && root[p.0 as usize] == 0 {
                root[p.0 as usize] = LOADED;
            }
        }
    }
    // defs in order of appearance in reverse postorder approximated by block order
    for _ in 0..if has_load && has_store { 2 } else { 0 } {
        for b in &f.blocks {
            for i in &b.insts {
                match i {
                    Inst::SlotAddr(d, sl) if single(*d) => root[d.0 as usize] = 1 + *sl as u64,
                    Inst::Mov(d, x) | Inst::IBinI(IOp::Add | IOp::Sub, _, d, x, _) if single(*d) => root[d.0 as usize] = root[x.0 as usize],
                    Inst::IBin(IOp::Add, _, d, x, y) if single(*d) => {
                        let (rx, ry) = (root[x.0 as usize], root[y.0 as usize]);
                        root[d.0 as usize] = if rx != 0 && ry == 0 { rx } else if ry != 0 && rx == 0 { ry } else { 0 };
                    }
                    Inst::Load(Mem::Int(8, _), d, _, _) if single(*d) => root[d.0 as usize] = LOADED,
                    Inst::Call(_, _, rets) => {
                        for r in rets {
                            if single(*r) {
                                root[r.0 as usize] = LOADED;
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    // noalias params stored as a value somewhere: rustc's `noalias` (LLVM's: an access through
    // a pointer not *based on* the param, e.g. one loaded from memory, may not touch the
    // param's memory, whatever the callees do) already covers calls; a param this function
    // itself stores is still left alone (cheap insurance for unsafe code in our own std)
    // which vregs may hold a noalias param's address (any def count: copies through
    // multi-def vregs too), to a fixpoint; a param whose address is stored is captured
    let mut captured: Vec<u32> = Vec::new();
    if has_load && has_store && !f.noalias.is_empty() {
        let mut taint: Vec<u32> = vec![u32::MAX; nv];
        for p in &f.noalias {
            taint[p.0 as usize] = p.0;
        }
        let mut more = true;
        let mut rounds = 0;
        while more && rounds < 8 {
            more = false;
            rounds += 1;
            for b in &f.blocks {
                for i in &b.insts {
                    let (d, srcs): (VReg, [VReg; 2]) = match i {
                        Inst::Mov(d, x) | Inst::IBinI(IOp::Add | IOp::Sub, _, d, x, _) | Inst::Conv(_, d, x) => (*d, [*x, *x]),
                        Inst::IBin(IOp::Add | IOp::Sub, _, d, x, y) => (*d, [*x, *y]),
                        Inst::Sel(d, _, x, y) => (*d, [*x, *y]),
                        _ => continue,
                    };
                    for x in srcs {
                        let t = taint[x.0 as usize];
                        if t != u32::MAX && taint[d.0 as usize] == u32::MAX {
                            taint[d.0 as usize] = t;
                            more = true;
                        }
                    }
                }
            }
        }
        if more {
            // did not settle: treat every noalias param as captured
            captured.extend(f.noalias.iter().map(|p| p.0));
        }
        for b in &f.blocks {
            for i in &b.insts {
                let v = match i {
                    Inst::Store(_, _, _, v) | Inst::StoreX(_, _, _, _, _, v) | Inst::AtomicStore(_, _, _, v) | Inst::AtomicRmw(_, _, _, _, _, v) => *v,
                    Inst::AtomicCas(_, _, _, _, _, _, n) => *n,
                    _ => continue,
                };
                if taint[v.0 as usize] != u32::MAX {
                    captured.push(taint[v.0 as usize]);
                }
            }
            if let Term::Ret(vs) = &b.term {
                for v in vs {
                    if taint[v.0 as usize] != u32::MAX {
                        captured.push(taint[v.0 as usize]);
                    }
                }
            }
        }
    }
    let may_alias = |a: VReg, b: VReg| -> bool {
        if a == b {
            return true;
        }
        let (ra, rb) = (root[a.0 as usize], root[b.0 as usize]);
        // an uncaptured noalias param's memory vs a pointer not derived from it
        let na_vs_loaded = |x: u64, y: u64| x >> 40 == 1 && y == LOADED && !captured.contains(&((x & 0xffff_ffff) as u32));
        if (na_vs_loaded(ra, rb) || na_vs_loaded(rb, ra)) && !no_noalias(options) {
            return false;
        }
        // otherwise LOADED is just unknown
        let (ra, rb) = (if ra == LOADED { 0 } else { ra }, if rb == LOADED { 0 } else { rb });
        !(ra != 0 && rb != 0 && ra != rb)
    };
    // definition blocks per multi-def vreg; unsigned `w = v - c` definitions per w
    // (multi-def vregs only, by midx[v] - 1)
    let mut midx = vec![0u32; nv];
    let mut def_blocks: Vec<Vec<u32>> = Vec::new();
    let mut def_sites: Vec<Vec<(u32, u32)>> = Vec::new();
    let mut sub_defs: Vec<(VReg, u32, VReg, i64)> = Vec::new();
    for (bi, b) in f.blocks.iter().enumerate() {
        for (k, i) in b.insts.iter().enumerate() {
            if let Inst::IBinI(IOp::Sub, it, wd, x, c) = i {
                if !it.signed && *c > 0 {
                    sub_defs.push((*wd, bi as u32, *x, *c));
                }
            }
            uses_defs(i, &mut uses, &mut defs);
            for d in &defs {
                if ndef[d.0 as usize] > 1 {
                    if midx[d.0 as usize] == 0 {
                        def_blocks.push(Vec::new());
                        def_sites.push(Vec::new());
                        midx[d.0 as usize] = def_blocks.len() as u32;
                    }
                    let m = midx[d.0 as usize] as usize - 1;
                    if def_blocks[m].last() != Some(&(bi as u32)) {
                        def_blocks[m].push(bi as u32);
                    }
                    def_sites[m].push((bi as u32, k as u32));
                }
            }
        }
    }
    let dominates = |a: u32, b: u32| -> bool {
        let (a, b) = (a as usize, b as usize);
        pre[a] != u32::MAX && pre[b] != u32::MAX && pre[a] <= pre[b] && post[b] <= post[a]
    };
    // loop headers: blocks with a predecessor they dominate
    let mut is_header = vec![false; n];
    for (bi, b) in f.blocks.iter().enumerate() {
        if idom[bi] == u32::MAX {
            continue;
        }
        for t in b.term.succs() {
            if dominates(t, bi as u32) {
                is_header[t as usize] = true;
            }
        }
    }
    // Loop header `h` entered from `d`: is `v` only ever decreased inside the loop (each
    // in-loop definition is `v = w` with `w = v - c`, c > 0, guarded by `v > c-1`)? Then an
    // upper bound known when entering the loop holds at the header on every iteration.
    let blocks_ptr: *const Vec<Block> = &f.blocks;
    let non_increasing = |v: VReg, d: u32, h: u32| -> bool {
        let blocks = unsafe { &*blocks_ptr };
        if midx[v.0 as usize] == 0 {
            return true;
        }
        for &(db, dk) in &def_sites[midx[v.0 as usize] as usize - 1] {
            if dominates(db, d) {
                continue;
            }
            if !dominates(h, db) {
                return false;
            }
            let w = match blocks[db as usize].insts[dk as usize] {
                Inst::Mov(_, w) => w,
                _ => return false,
            };
            // w's definition `w = v - c` with a guard on v dominating it
            let mut ok = false;
            if let Some(&(_, bi2, _, c)) = sub_defs.iter().find(|s| s.0 == w && s.2 == v) {
                // a block on the dominator chain of bi2 entered by the true edge of
                // `v > k` (k >= c-1) or `v != 0` (c == 1)
                let mut xb = bi2;
                let mut guard = 0;
                while xb != h && guard < 1000 {
                    let p = idom[xb as usize];
                    if p == u32::MAX || p == xb {
                        break;
                    }
                    if npred[xb as usize] == 1 {
                        if let Term::Branch(cv, tt, _) = blocks[p as usize].term {
                            if tt == xb {
                                for i3 in &blocks[p as usize].insts {
                                    match i3 {
                                        Inst::ICmpI(Cond::Gt, false, cd, cx, k) if *cd == cv && *cx == v && *k >= c - 1 => ok = true,
                                        Inst::ICmpI(Cond::Ne, _, cd, cx, 0) if *cd == cv && *cx == v && c == 1 => ok = true,
                                        _ => {}
                                    }
                                }
                            }
                        }
                    }
                    xb = p;
                    guard += 1;
                }
            }
            if !ok {
                return false;
            }
        }
        true
    };
    let mut fx = Facts { fact: vec![None; nv], ub: vec![None; nv], lb: vec![None; nv], undo: Vec::new(), multi: Vec::new(), multi_undo: Vec::new() };
    let mut changed = false;
    let mut work: Vec<CseWork> = vec![CseWork::Enter(0, CseState::default(), None, u32::MAX)];
    let mut ops: Vec<VReg> = Vec::new();
    // vregs ever entered into an avail or copies list (on any path): a def of any other
    // vreg has nothing to invalidate there
    let mut mentioned = vec![false; nv];
    while let Some(wk) = work.pop() {
        let (bi, mut st, edge, parent) = match wk {
            CseWork::Leave(mark) => {
                fx.rewind(mark);
                continue;
            }
            CseWork::Enter(b, s, e, p) => (b as usize, s, e, p),
        };
        work.push(CseWork::Leave(fx.undo.len()));
        match edge {
            Some(None) => {}
            Some(Some((c, val, cmp))) => {
                // the branch fact of the edge, and range facts of an unsigned compare
                fx.set(0, c, Some(val), !single(c));
                if let Some((cc, x, k)) = cmp {
                    let m = !single(x);
                    if (cc == Cond::Eq && val == 1) || (cc == Cond::Ne && val == 0) {
                        fx.set(0, x, Some(k), m);
                    }
                    // x <u k on the true edge of `x < k`, the false edge of `x >= k`
                    if (cc == Cond::Lt && val == 1) || (cc == Cond::Ge && val == 0) {
                        fx.tighten_ub(x, k, m);
                    }
                    if k != -1 && ((cc == Cond::Le && val == 1) || (cc == Cond::Gt && val == 0)) {
                        fx.tighten_ub(x, k.wrapping_add(1), m);
                    }
                    // x >u k on the true edge of `x > k`, the false edge of `x <= k`
                    if (cc == Cond::Gt && val == 1) || (cc == Cond::Le && val == 0) {
                        fx.tighten_lb(x, k, m);
                    }
                    if k != 0 && ((cc == Cond::Ge && val == 1) || (cc == Cond::Lt && val == 0)) {
                        fx.tighten_lb(x, k.wrapping_sub(1), m);
                    }
                    if k == 0 && ((cc == Cond::Ne && val == 1) || (cc == Cond::Eq && val == 0)) {
                        fx.tighten_lb(x, 0, m);
                    }
                }
            }
            None if parent != u32::MAX => {
                // a join: keep what cannot change on the way from the dominator `parent`
                let header = is_header[bi];
                let stable = |v: VReg| -> bool {
                    if v.0 == u32::MAX || single(v) {
                        return true;
                    }
                    if header {
                        return false;
                    }
                    let blocks = unsafe { &*blocks_ptr };
                    if midx[v.0 as usize] == 0 {
                        return true;
                    }
                    for &db in &def_blocks[midx[v.0 as usize] as usize - 1] {
                        // a latch that only jumps back to the dominator redefines v before
                        // the next pass through there: the last pass to the join sees no def
                        let latch_to_here = matches!(blocks[db as usize].term, Term::Jump(t) if t == parent);
                        if !(dominates(db, parent) || dominates(bi as u32, db) || latch_to_here) {
                            return false;
                        }
                    }
                    true
                };
                st.avail.retain(|e| !e.3 && stable(e.1) && stable(e.2[0]) && stable(e.2[1]));
                st.copies.retain(|x| stable(x.0) && stable(x.1));
                let multi: Vec<u32> = fx.multi.clone();
                for v in multi {
                    let v = VReg(v);
                    if stable(v) {
                        continue;
                    }
                    fx.set(0, v, None, false);
                    fx.set(2, v, None, false);
                    if !(header && fx.ub[v.0 as usize].is_some() && non_increasing(v, parent, bi as u32)) {
                        fx.set(1, v, None, false);
                    }
                }
            }
            None => {}
        }
        let b = &mut f.blocks[bi];
        for k in 0..b.insts.len() {
            uses_defs(&b.insts[k], &mut uses, &mut defs);
            if !st.copies.is_empty() {
                let hit_copy = uses.iter().any(|u| st.copies.iter().any(|(d, _)| d == u));
                if hit_copy {
                    let cp = &st.copies;
                    let look = |v: VReg| -> VReg {
                        for (d, s2) in cp.iter() {
                            if *d == v {
                                return *s2;
                            }
                        }
                        v
                    };
                    let n2 = rename(&b.insts[k], &look);
                    if n2 != b.insts[k] {
                        b.insts[k] = n2;
                        changed = true;
                        uses_defs(&b.insts[k], &mut uses, &mut defs);
                    }
                }
            }
            // A checked slice length is constant on the successful equality edge.
            // Use the path fact here: global folding cannot assign one constant to
            // the length vreg on both the success and the panic paths.
            if let Inst::Call(Callee::Host(a), args, rets) = &b.insts[k] {
                if rets.is_empty() && args.len() == 3 && Some(*a) == mem_fns().map(|m| m.0) {
                    if let Some(n) = fx.fact[args[2].0 as usize] {
                        if (0..=INLINE_COPY_MAX).contains(&n) {
                            b.insts[k] = Inst::Copy(args[0], args[1], n as u32);
                            changed = true;
                            uses_defs(&b.insts[k], &mut uses, &mut defs);
                        }
                    }
                }
            }
            // Both edges of a guard constrain the unsigned range. In particular,
            // the success edge of `len >= n` also proves a later `len < n` false.
            if let Inst::ICmpI(cc, false, d, y, k2) = b.insts[k] {
                let lo = fx.lb[y.0 as usize].and_then(|b| (b as u64).checked_add(1)).unwrap_or(0);
                let hi = fx.ub[y.0 as usize].and_then(|b| (b as u64).checked_sub(1)).unwrap_or(u64::MAX);
                let k2 = k2 as u64;
                let result = match cc {
                    Cond::Lt if hi < k2 => Some(true),
                    Cond::Lt if lo >= k2 => Some(false),
                    Cond::Le if hi <= k2 => Some(true),
                    Cond::Le if lo > k2 => Some(false),
                    Cond::Gt if lo > k2 => Some(true),
                    Cond::Gt if hi <= k2 => Some(false),
                    Cond::Ge if lo >= k2 => Some(true),
                    Cond::Ge if hi < k2 => Some(false),
                    Cond::Eq | Cond::Ne if k2 < lo || k2 > hi => Some(cc == Cond::Ne),
                    Cond::Eq | Cond::Ne if lo == hi && lo == k2 => Some(cc == Cond::Eq),
                    _ => None,
                };
                if let Some(result) = result {
                    b.insts[k] = Inst::Iconst(d, result as i64);
                    changed = true;
                    uses_defs(&b.insts[k], &mut uses, &mut defs);
                }
            }
            let is_load = matches!(b.insts[k], Inst::Load(..));
            ops.clear();
            ops.extend_from_slice(&uses);
            let cand = match &b.insts[k] {
                Inst::Mov(..) | Inst::SlotAddr(..) => None,
                i if is_mem_effect(i) => None,
                _ => {
                    if defs.len() == 1 && !uses.contains(&defs[0]) {
                        Some(defs[0])
                    } else {
                        None
                    }
                }
            };
            let key = match cand {
                Some(_) => cse_key(&b.insts[k]),
                None => None,
            };
            let mut replaced = false;
            if let (Some(d), Some(key)) = (cand, key) {
                if let Some(e) = st.avail.iter().find(|x| x.0 == key).map(|x| x.1) {
                    if e != d {
                        b.insts[k] = match fx.fact[e.0 as usize] {
                            Some(x) => Inst::Iconst(d, x),
                            None => Inst::Mov(d, e),
                        };
                        changed = true;
                        replaced = true;
                    }
                }
            }
            match &b.insts[k] {
                Inst::Call(..) | Inst::Copy(..) | Inst::AtomicLoad(..) | Inst::AtomicStore(..) | Inst::AtomicRmw(..) | Inst::AtomicCas(..) | Inst::Fence(..) | Inst::FpEnv(..) | Inst::Cpuid(..) | Inst::Xgetbv(..) | Inst::VStore(..) => st.avail.retain(|x| !x.3),
                Inst::Store(m, base, off, v) => {
                    let (base, off, v, m) = (*base, *off as i64, *v, *m);
                    let sz = mem_size(m) as i64;
                    st.avail.retain(|x| {
                        if !x.3 {
                            return true;
                        }
                        let lb = x.2[0];
                        if !may_alias(lb, base) {
                            return true;
                        }
                        if lb == base {
                            // same base: disjoint byte ranges survive
                            let lo = x.0[2] as i64;
                            let lsz = load_key_size(x.0[0]);
                            return lo + lsz <= off || off + sz <= lo;
                        }
                        false
                    });
                    // forward the stored value to later loads of the same cell
                    if st.avail.len() < 48 {
                        let key = cse_key(&Inst::Load(m, VReg(0), base, off as i32)).unwrap();
                        st.avail.push((key, v, [base, v], true));
                        mentioned[v.0 as usize] = true;
                        mentioned[base.0 as usize] = true;
                    }
                }
                _ => {}
            }
            // (a replacement keeps the destination: `defs` still holds)
            for d in &defs {
                let d = *d;
                fx.clear(d);
                if !mentioned[d.0 as usize] {
                    continue;
                }
                if st.avail.iter().any(|(_, e, ops, _)| *e == d || ops[0] == d || ops[1] == d) {
                    st.avail.retain(|(_, e, ops, _)| *e != d && ops[0] != d && ops[1] != d);
                }
                if st.copies.iter().any(|(x, y)| *x == d || *y == d) {
                    st.copies.retain(|(x, y)| *x != d && *y != d);
                }
            }
            if let Inst::IBinI(IOp::Sub, it, d, x, c) = b.insts[k] {
                if !it.signed && c > 0 && fx.lb[x.0 as usize].is_some_and(|lb| lb >= c - 1) {
                    if let Some(bnd) = fx.ub[x.0 as usize] {
                        fx.tighten_ub(d, bnd, !single(d));
                    }
                }
            }
            if let Inst::Mov(d, s2) = b.insts[k] {
                if d != s2 {
                    if st.copies.len() < 32 {
                        st.copies.push((d, s2));
                        mentioned[d.0 as usize] = true;
                        mentioned[s2.0 as usize] = true;
                    }
                    // a copy inherits the source's bound (snapshot of the same value)
                    if let Some(bound) = fx.ub[s2.0 as usize] {
                        fx.tighten_ub(d, bound, !single(d));
                    }
                    if let Some(bound) = fx.lb[s2.0 as usize] {
                        fx.tighten_lb(d, bound, !single(d));
                    }
                }
            }
            if let (Some(d), false, Some(key)) = (cand, replaced, key) {
                if st.avail.len() < 48 {
                    let o0 = if !ops.is_empty() { ops[0] } else { VReg(u32::MAX) };
                    let o1 = if ops.len() > 1 { ops[1] } else { VReg(u32::MAX) };
                    st.avail.push((key, d, [o0, o1], is_load));
                    mentioned[d.0 as usize] = true;
                    for o in [o0, o1] {
                        if o.0 != u32::MAX {
                            mentioned[o.0 as usize] = true;
                        }
                    }
                }
            }
        }
        if !st.copies.is_empty() {
            let cp = &st.copies;
            let look = |v: VReg| -> VReg {
                for (d, s2) in cp.iter() {
                    if *d == v {
                        return *s2;
                    }
                }
                v
            };
            b.term = match &b.term {
                Term::Branch(c, x, y) => Term::Branch(look(*c), *x, *y),
                Term::Ret(v) => Term::Ret(v.iter().map(|x| look(*x)).collect()),
                t => t.clone(),
            };
        }
        if let Term::Branch(c, t, e) = b.term {
            if let Some(x) = fx.fact[c.0 as usize] {
                b.term = Term::Jump(if x != 0 { t } else { e });
                changed = true;
            }
        }
        // the compare behind the branch condition (unsigned compares give range facts)
        let mut cmp_of_cond: Option<(Cond, VReg, i64)> = None;
        if let Term::Branch(c, _, _) = b.term {
            for i in &b.insts {
                // RIR allows reassignment: the edge describes the operands at
                // the compare, not a later definition of either operand.
                for_defs(i, |d| {
                    if d == c || cmp_of_cond.map_or(false, |(_, x, _)| d == x) {
                        cmp_of_cond = None;
                    }
                });
                if let Inst::ICmpI(cc, false, d, x, kk) = i {
                    if *d == c && *x != c {
                        cmp_of_cond = Some((*cc, *x, *kk));
                    }
                }
            }
        }
        let term = b.term.clone();
        // children in the dominator tree (pushed in reverse: visited in order); the last
        // pushed takes the state by move
        let nch = children[bi].len();
        let mut st_opt = Some(st);
        for ci in (0..nch).rev() {
            let ch = children[bi][ci];
            if pad[ch as usize] {
                continue;
            }
            let cs = if ci == 0 { st_opt.take().unwrap() } else { st_opt.as_ref().unwrap().clone() };
            let edge = if npred[ch as usize] == 1 {
                match term {
                    Term::Branch(c, t, e) if t != e && (ch == t || ch == e) => Some(Some((c, if ch == t { 1 } else { 0 }, cmp_of_cond))),
                    _ => Some(None),
                }
            } else {
                None
            };
            work.push(CseWork::Enter(ch, cs, edge, bi as u32));
        }
    }
    changed
}

/// Compact CSE key of a pure single-result instruction (dest excluded).
/// Byte size of the load encoded in a `cse_key` tag word.
fn load_key_size(k0: u64) -> i64 {
    let mk = k0 >> 8;
    if mk & (1 << 16) != 0 {
        4
    } else if mk & (2 << 16) != 0 {
        8
    } else {
        (mk & 0xff) as i64
    }
}

fn cse_key(i: &Inst) -> Option<[u64; 3]> {
    fn it(t: &IntTy) -> u64 {
        t.bits as u64 | ((t.signed as u64) << 8)
    }
    fn cond(c: &Cond) -> u64 {
        *c as u64
    }
    let v = |r: &VReg| r.0 as u64;
    Some(match i {
        Inst::Iconst(_, x) => [1, *x as u64, 0],
        Inst::Fconst(_, x, w) => [2 | ((*w as u64) << 8), *x, 0],
        Inst::IBin(o, t, _, a, b) => [3 | ((*o as u64) << 8) | (it(t) << 16), v(a), v(b)],
        Inst::IBinI(o, t, _, a, k) => [4 | ((*o as u64) << 8) | (it(t) << 16), v(a), *k as u64],
        Inst::INeg(t, _, a) => [5 | (it(t) << 16), v(a), 0],
        Inst::INot(t, _, a) => [6 | (it(t) << 16), v(a), 0],
        Inst::IBit(o, _, a) => [19 | ((*o as u64) << 8), v(a), 0],
        Inst::ICmp(c, s2, _, a, b) => [7 | (cond(c) << 8) | ((*s2 as u64) << 16), v(a), v(b)],
        Inst::ICmpI(c, s2, _, a, k) => [8 | (cond(c) << 8) | ((*s2 as u64) << 16), v(a), *k as u64],
        Inst::FBin(o, w, _, a, b) => [9 | ((*o as u64) << 8) | ((*w as u64) << 16), v(a), v(b)],
        Inst::FUnary(o, w, _, a) => [10 | ((*o as u64) << 8) | ((*w as u64) << 16), v(a), 0],
        Inst::FCmp(c, w, _, a, b) => [11 | (cond(c) << 8) | ((*w as u64) << 16), v(a), v(b)],
        Inst::Conv(c, _, a) => {
            let ck = match c {
                Conv::IntToInt(t) => 1 | (it(t) << 8),
                Conv::IntToF(t, w) => 2 | (it(t) << 8) | ((*w as u64) << 24),
                Conv::FToInt(w, t) => 3 | (it(t) << 8) | ((*w as u64) << 24),
                Conv::F32ToF64 => 4,
                Conv::F64ToF32 => 5,
                Conv::BitsToF(w) => 6 | ((*w as u64) << 8),
                Conv::FToBits(w) => 7 | ((*w as u64) << 8),
            };
            [12 | (ck << 8), v(a), 0]
        }
        Inst::Load(m, _, b, o) => {
            let mk = match m {
                Mem::Int(n, s2) => *n as u64 | ((*s2 as u64) << 8),
                Mem::F32 => 1 << 16,
                Mem::F64 => 2 << 16,
            };
            [13 | (mk << 8), v(b), *o as i64 as u64]
        }
        Inst::Addr(_, x) => [14, x.id as u64, x.off as u64],
        Inst::FnAddr(_, x) => [15, *x as u64, 0],
        Inst::VExtract(_, a, l) => [16 | ((*l as u64) << 8), v(a), 0],
        Inst::VConst(_, x) => [17, *x as u64, 0],
        Inst::VSplat(_, a) => [18, v(a), 0],
        _ => return None,
    })
}


/// Scalar replacement of stack slots. A slot qualifies when every use of its address -- the
/// SlotAddr vreg or an address derived from it by adding a constant (a field, an enum
/// payload) -- is the base of a load or store, an operand of a `Copy`, or another such
/// derivation. Its accessed cells (offset, kind) become vregs. Slots joined by copies form a
/// group with one cell layout in the group's coordinates (a copy of slot B at offset 4 into
/// slot A at offset 1 puts B's offset x at A's x - 3), so the chains of small aggregates
/// lowering makes (an Option built in one slot, copied to a temporary, its payload copied
/// into a field of the result) become register moves. A copy between a group and other
/// memory (a call's result slot, an out-pointer, a slice's bytes) becomes per-cell loads or
/// stores. Overlapping accesses become one unsigned integer cell only when every access
/// has the same offset and size, with different kinds handled by bit reinterpretation
/// or integer extension. Overlaps with different offsets or sizes stay in memory.
/// Groups with both incoming and outgoing memory copies tile unnamed bytes of each copied
/// range (at most 64 bytes) with integer cells, preserving padding through the copies.
/// Copies must not cut cells; these groups get a 16-cell copy budget instead of eight.
/// Most cells a group joined by copies may have (scalar pairs, Option/Result of a few fields).
const SROA_COPY_CELLS: usize = 8;
const SROA_COPY_IN_OUT_CELLS: usize = 16;
/// A 128-bit vector cell (VLoad/VStore) in sroa's cell table (no integer access is 16 bytes)
const SROA_V128: Mem = Mem::Int(16, false);

#[derive(Clone, Copy)]
struct SroaCell {
    off: i64,
    mem: Mem,
    // Different kinds at the same offset and size share unsigned integer bits.
    widened: bool,
}

#[derive(Clone, Copy)]
enum SroaReject {
    Escape,
    Overlap,
    KindMix,
    CopyCut,
    CopyCells,
    CopyInOut,
    Other,
}

#[derive(Default)]
struct SroaStats {
    // Early rejects are singleton candidates; copies coalesce the remaining candidates.
    candidates: usize,
    promoted: usize,
    rejects: [usize; 7],
}

fn sroa_reject_slot(ok: &mut [bool], sl: usize, why: SroaReject, stats: &mut Option<&mut SroaStats>) {
    if ok[sl] {
        ok[sl] = false;
        if let Some(s) = stats {
            s.rejects[why as usize] += 1;
        }
    }
}

relax_session::gen_local! {
    /// sroa's answer to "does this call keep or return a pointer passed as argument k?":
    /// set by the O3 driver from the callees' RIR (param_nocapture), None elsewhere
    static SROA_NOCAP: std::cell::RefCell<Option<Box<dyn Fn(&Callee, usize) -> bool>>>;
}

/// Installs (Some) or removes (None) sroa's no-capture oracle for this thread.
pub fn set_sroa_nocapture(q: Option<Box<dyn Fn(&Callee, usize) -> bool>>) {
    // (try_with: the JIT compiles on threads whose thread-locals are being destroyed)
    let _ = SROA_NOCAP.try_with(|c| *c.borrow_mut() = q);
}

fn sroa_nocapture(c: &Callee, k: usize) -> bool {
    SROA_NOCAP.try_with(|q| q.borrow().as_ref().map_or(false, |f| f(c, k))).unwrap_or(false)
}

/// Whether `f` neither keeps nor returns the pointer it gets as parameter `k`: no value
/// derived from it (moves, address arithmetic, selects, casts) is stored to memory, returned,
/// used as an indirect callee, or passed to a call `sub` does not vouch for. Uses through it
/// as a load / store / copy base are fine (that is what the callee is for).
pub fn param_nocapture(f: &Func, k: usize, sub: &dyn Fn(&Callee, usize) -> bool) -> bool {
    let Some(&p) = f.params.get(k) else { return false };
    let nv = f.vregs.len();
    let mut t = vec![false; nv];
    t[p.0 as usize] = true;
    for _ in 0..8 {
        let mut more = false;
        for b in &f.blocks {
            for i in &b.insts {
                let (d, x, y): (VReg, VReg, VReg) = match i {
                    Inst::Mov(d, x) | Inst::IBinI(_, _, d, x, _) | Inst::Conv(_, d, x) => (*d, *x, *x),
                    Inst::IBin(_, _, d, x, y) | Inst::Sel(d, _, x, y) => (*d, *x, *y),
                    _ => continue,
                };
                if (t[x.0 as usize] || t[y.0 as usize]) && !t[d.0 as usize] {
                    t[d.0 as usize] = true;
                    more = true;
                }
            }
        }
        if !more {
            break;
        }
    }
    let mut us = Vec::new();
    for b in &f.blocks {
        for i in &b.insts {
            match i {
                Inst::Store(_, _, _, v) | Inst::StoreX(_, _, _, _, _, v) | Inst::VStore(_, _, v) | Inst::AtomicStore(_, _, _, v) | Inst::AtomicRmw(_, _, _, _, _, v) => {
                    if t[v.0 as usize] {
                        return false;
                    }
                }
                Inst::AtomicCas(_, _, _, _, _, e, n2) => {
                    if t[e.0 as usize] || t[n2.0 as usize] {
                        return false;
                    }
                }
                Inst::Call(c, args, _) => {
                    if let Callee::Indirect(v) | Callee::CIndirect(v, _) = c {
                        if t[v.0 as usize] {
                            return false;
                        }
                    }
                    for (j, a) in args.iter().enumerate() {
                        if t[a.0 as usize] && !sub(c, j) {
                            return false;
                        }
                    }
                }
                _ => {}
            }
        }
        term_uses(&b.term, &mut us);
        if let Term::Ret(_) = b.term {
            if us.iter().any(|v| t[v.0 as usize]) {
                return false;
            }
        }
    }
    true
}

/// Keep addressable 16-byte SIMD values in vector registers, including the
/// integer lanes used by the aggregate ABI and partial slice copies. SROA's
/// disjoint scalar cells cannot represent these overlapping accesses.
/// Only slots reached by a vector access (or a whole copy of one) qualify.
/// Unknown uses reject the slot. Terminal panic blocks get the current value
/// materialized at entry and retain their original memory operations.
/// Slots promote_vector_slots can hold in V128 registers: whole 16-byte chunks, up to 64 bytes.
fn vec_slot_size(n: u32) -> bool {
    n >= 16 && n <= 64 && n % 16 == 0
}

pub fn promote_vector_slots(f: &mut Func) -> bool {
    if !f.slots.iter().any(|s| vec_slot_size(s.size)) || !f.blocks.iter().any(|b| b.insts.iter().any(|i| matches!(i, Inst::VLoad(..) | Inst::VStore(..)))) {
        return false;
    }
    if f.blocks.iter().any(|b| b.term.succs().any(|s| s == 0)) {
        return false;
    }
    unify_slot_addrs(f);
    let sc = slot_scan_inner(f, true);
    const NONE: u32 = u32::MAX;
    let at = |v: VReg, off: i32| {
        let (s, o) = sc.vs[v.0 as usize];
        (s, o + off as i64)
    };
    let mut ok: Vec<bool> = f.slots.iter().enumerate().map(|(s, x)| vec_slot_size(x.size) && !sc.esc[s]).collect();
    let mut vector = vec![false; ok.len()];
    let ssz: Vec<i64> = f.slots.iter().map(|x| x.size as i64).collect();
    for b in &f.blocks {
        for i in &b.insts {
            let mut access = |v: VReg, off: i32, n: u32, valid: bool| {
                let (s, o) = at(v, off);
                if s != NONE && (!valid || o < 0 || o + n as i64 > ssz[s as usize] || (n < 16 && o % n.max(1) as i64 != 0) || (n == 16 && o % 16 != 0)) {
                    ok[s as usize] = false;
                }
            };
            match *i {
                Inst::VLoad(_, p, o) | Inst::VStore(p, o, _) => {
                    access(p, o, 16, true);
                    let (s, _) = at(p, o);
                    if s != NONE {
                        vector[s as usize] = true;
                    }
                }
                Inst::Load(m, _, p, o) | Inst::Store(m, p, o, _) => {
                    access(p, o, mem_size(m) as u32, matches!(m, Mem::Int(1 | 2 | 4 | 8, _)));
                }
                Inst::Copy(d, s, n) => {
                    // Copies are split into aligned lanes below, so their byte
                    // offsets need not align to the entire copy's length.
                    for p in [d, s] {
                        let (sl, o) = at(p, 0);
                        if sl != NONE && (o < 0 || o + n as i64 > ssz[sl as usize]) {
                            ok[sl as usize] = false;
                        }
                    }
                }
                Inst::LoadX(_, _, p, ..) | Inst::StoreX(_, p, ..) => access(p, 0, 0, false),
                _ => {}
            }
        }
    }
    loop {
        let mut changed = false;
        for b in &f.blocks {
            for i in &b.insts {
                if let Inst::Copy(d, s, n) = *i {
                    if n % 16 != 0 {
                        continue;
                    }
                    let (ds, ss) = (at(d, 0).0, at(s, 0).0);
                    if ds != NONE && ss != NONE && (vector[ds as usize] || vector[ss as usize]) {
                        for sl in [ds, ss] {
                            if !vector[sl as usize] {
                                vector[sl as usize] = true;
                                changed = true;
                            }
                        }
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }
    for s in 0..ok.len() {
        ok[s] &= vector[s];
    }
    if !ok.iter().any(|x| *x) {
        return false;
    }
    // one V128 per 16-byte chunk of a promoted slot
    let mut regs: Vec<Vec<VReg>> = vec![Vec::new(); ok.len()];
    let mut ptrs = vec![VReg(NONE); ok.len()];
    let mut init = Vec::new();
    for s in 0..ok.len() {
        if ok[s] {
            ptrs[s] = f.vreg(Cls::I);
            for _ in 0..ssz[s] / 16 {
                let r = f.vreg(Cls::V128);
                init.push(Inst::VConst(r, 0));
                regs[s].push(r);
            }
            init.push(Inst::SlotAddr(ptrs[s], s as u32));
        }
    }
    let cell = |p: VReg, o: i32| -> Option<(VReg, u8)> {
        let (s, off) = at(p, o);
        if s != NONE && ok[s as usize] {
            Some((regs[s as usize][(off / 16) as usize], (off % 16) as u8))
        } else {
            None
        }
    };
    let width = |n| match n {
        1 => VW::B,
        2 => VW::H,
        4 => VW::S,
        8 => VW::D,
        _ => unreachable!(),
    };
    for bi in 0..f.blocks.len() {
        if matches!(f.blocks[bi].term, Term::Unreachable) {
            // No successor can read a promoted value after these calls. The
            // original instructions preserve mutations and aliases within the
            // terminal block, including addresses retained by panic formatting.
            let mut flush = Vec::new();
            for s in 0..ok.len() {
                if ok[s]
                    && f.blocks[bi].insts.iter().any(|i| {
                        let mut hit = false;
                        for_uses(i, |v| hit |= at(v, 0).0 == s as u32);
                        hit
                    })
                {
                    for (k, r) in regs[s].iter().enumerate() {
                        flush.push(Inst::VStore(ptrs[s], 16 * k as i32, *r));
                    }
                }
            }
            let p = f.blocks[bi].pos.first().copied().unwrap_or(f.blocks[bi].term_pos);
            f.blocks[bi].pos.splice(0..0, std::iter::repeat(p).take(flush.len()));
            f.blocks[bi].insts.splice(0..0, flush);
            continue;
        }
        let old = std::mem::take(&mut f.blocks[bi].insts);
        let oldp = std::mem::take(&mut f.blocks[bi].pos);
        let mut out = Vec::new();
        let mut pos = Vec::new();
        for (i, p) in old.into_iter().zip(oldp) {
            match i {
                Inst::VLoad(d, b, o) if cell(b, o).is_some() => out.push(Inst::Mov(d, cell(b, o).unwrap().0)),
                Inst::VStore(b, o, v) if cell(b, o).is_some() => out.push(Inst::Mov(cell(b, o).unwrap().0, v)),
                Inst::Load(Mem::Int(n, sg), d, b, o) if cell(b, o).is_some() => {
                    let (v, off) = cell(b, o).unwrap();
                    out.push(Inst::VX(VX::Extract(width(n), off / n), d, v, v));
                    if sg && n < 8 {
                        out.push(Inst::Conv(Conv::IntToInt(IntTy { bits: n * 8, signed: true }), d, d));
                    }
                }
                Inst::Store(Mem::Int(n, _), b, o, x) if cell(b, o).is_some() => {
                    let (v, off) = cell(b, o).unwrap();
                    out.push(Inst::VX(VX::Insert(width(n), off / n), v, v, x));
                }
                Inst::Copy(d, s, n) if cell(d, 0).is_some() || cell(s, 0).is_some() => {
                    if n % 16 == 0 && [d, s].iter().all(|&p| cell(p, 0).map_or(true, |(_, off)| off == 0)) {
                        // whole 16-byte chunks: read every chunk before writing any
                        let mut tmp = Vec::new();
                        for k in 0..(n / 16) as i32 {
                            let r = f.vreg(Cls::V128);
                            out.push(match cell(s, 16 * k) {
                                Some((v, _)) => Inst::Mov(r, v),
                                None => Inst::VLoad(r, s, 16 * k),
                            });
                            tmp.push(r);
                        }
                        for (k, r) in tmp.into_iter().enumerate() {
                            out.push(match cell(d, 16 * k as i32) {
                                Some((v, _)) => Inst::Mov(v, r),
                                None => Inst::VStore(d, 16 * k as i32, r),
                            });
                        }
                    } else {
                        // Read all lanes before writing, preserving Copy's
                        // snapshot semantics even for overlapping slot ranges.
                        let mut lanes = Vec::new();
                        let mut o = 0;
                        while o < n {
                            let mut w = 8u8;
                            while w as u32 > n - o || [d, s].iter().any(|&p| cell(p, o as i32).map_or(false, |(_, off)| off % w != 0)) {
                                w /= 2;
                            }
                            let r = f.vreg(Cls::I);
                            out.push(match cell(s, o as i32) {
                                Some((v, off)) => Inst::VX(VX::Extract(width(w), off / w), r, v, v),
                                None => Inst::Load(Mem::Int(w, false), r, s, o as i32),
                            });
                            lanes.push((o, w, r));
                            o += w as u32;
                        }
                        for (o, w, r) in lanes {
                            out.push(match cell(d, o as i32) {
                                Some((v, off)) => Inst::VX(VX::Insert(width(w), off / w), v, v, r),
                                None => Inst::Store(Mem::Int(w, false), d, o as i32, r),
                            });
                        }
                    }
                }
                _ => out.push(i),
            }
            pos.resize(out.len(), p);
        }
        f.blocks[bi].insts = out;
        f.blocks[bi].pos = pos;
    }
    let p = f.blocks[0].pos.first().copied().unwrap_or(f.blocks[0].term_pos);
    f.blocks[0].pos.splice(0..0, std::iter::repeat(p).take(init.len()));
    f.blocks[0].insts.splice(0..0, init);
    // Lowering initializes byte arrays one field at a time. Fold those lane
    // writes after promotion so a zeroed buffer remains one vector constant,
    // rather than sixteen dependent inserts on every call. Facts are local:
    // promoted registers can have different values on incoming loop edges.
    for b in &mut f.blocks {
        let mut ints = vec![None; f.vregs.len()];
        let mut vectors = vec![None; f.vregs.len()];
        for i in &mut b.insts {
            let (ik, vk) = match *i {
                Inst::Iconst(_, k) => (Some(k), None),
                Inst::VConst(_, k) => (None, Some((k as u128) * 0x00000001000000010000000100000001)),
                Inst::Mov(_, s) => (ints[s.0 as usize], vectors[s.0 as usize]),
                Inst::VX(VX::Insert(w, lane), _, a, x) => {
                    let v = vectors[a.0 as usize].zip(ints[x.0 as usize]).map(|(v, x)| {
                        let bits = w.bits();
                        let shift = lane as u32 * bits;
                        let mask = (1u128 << bits) - 1;
                        (v & !(mask << shift)) | (((x as u128) & mask) << shift)
                    });
                    (None, v)
                }
                _ => (None, None),
            };
            if let (Inst::VX(VX::Insert(..), d, ..), Some(v)) = (&*i, vk) {
                let word = v as u32;
                if v == (word as u128) * 0x00000001000000010000000100000001 {
                    *i = Inst::VConst(*d, word);
                }
            }
            for_defs(i, |d| {
                ints[d.0 as usize] = ik;
                vectors[d.0 as usize] = vk;
            });
        }
    }
    true
}

pub fn sroa(f: &mut Func) -> bool {
    sroa_with(f, false)
}

/// Post-inline sroa (O3, after the last inlining round): groups copied both in and out
/// are tiled (LLVM's whole-alloca slicing). Not at O1: tiling a callee's copies grows the
/// body the inliner sizes (tsr's RangeRecord `get` stopped inlining: +40% at O3).
pub fn sroa_post_inline(f: &mut Func) -> bool {
    sroa_with(f, true)
}

fn sroa_with(f: &mut Func, tile: bool) -> bool {
    static STATS: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let mut stats = STATS.get_or_init(|| std::env::var("MAKEPAD_RUN_SROA_STATS").map_or(false, |v| v == "1"))
        .then(|| SroaStats { candidates: f.slots.len(), ..SroaStats::default() });
    let changed = sroa_inner(f, stats.as_mut(), tile);
    if let Some(s) = stats {
        eprintln!("sroa {}: candidates={} promoted={} escape={} overlap={} kind-mix={} copy-cut={} copy-cells={} copy-in+out={} other={}",
            f.name, s.candidates, s.promoted, s.rejects[0], s.rejects[1], s.rejects[2], s.rejects[3], s.rejects[4], s.rejects[5], s.rejects[6]);
    }
    changed
}

fn sroa_inner(f: &mut Func, mut stats: Option<&mut SroaStats>, tile: bool) -> bool {
    let nv = f.vregs.len();
    let ns = f.slots.len();
    if ns == 0 {
        return false;
    }
    // the cells' zero definitions go at the top of the entry block: it must not be a loop head
    for b in &f.blocks {
        match b.term {
            Term::Jump(0) | Term::Branch(_, 0, _) | Term::Branch(_, _, 0) => {
                if let Some(s) = &mut stats {
                    s.rejects[SroaReject::Other as usize] = ns;
                }
                return false;
            }
            _ => {}
        }
    }
    let mut uses = Vec::new();
    let mut defs = Vec::new();
    let mut ndef = vec![0u32; nv];
    for p in &f.params {
        ndef[p.0 as usize] += 1;
    }
    // vreg -> (slot, offset) of the address it holds (SlotAddr / SlotAddr + k; a vreg defined
    // several times only by SlotAddr of one slot -- copies made by jump threading -- counts)
    const NO: (u32, i64) = (u32::MAX, 0);
    let mut addr: Vec<(u32, i64)> = vec![NO; nv];
    let mut slot_defs: Vec<(u32, u32)> = vec![(u32::MAX, 0); nv]; // (slot, SlotAddr def count)
    // one scan: the instructions the phases below look at (`rel`, in block order) and the
    // operands of all the others and of the terminators (an address used there escapes)
    let mut rel: Vec<(u32, u32)> = Vec::new();
    let mut other_uses: Vec<VReg> = Vec::new();
    for (bi, b) in f.blocks.iter().enumerate() {
        for (k, i) in b.insts.iter().enumerate() {
            for_defs(i, |d| ndef[d.0 as usize] += 1);
            match i {
                Inst::SlotAddr(d, sl) => {
                    let e = &mut slot_defs[d.0 as usize];
                    if e.0 == u32::MAX || e.0 == *sl {
                        *e = (*sl, e.1 + 1);
                    } else {
                        *e = (u32::MAX - 1, e.1 + 1);
                    }
                    rel.push((bi as u32, k as u32));
                }
                Inst::IBinI(IOp::Add, ..) | Inst::Mov(..) | Inst::Load(..) | Inst::VLoad(..) | Inst::Store(..) | Inst::VStore(..) | Inst::Copy(..) | Inst::Call(..) => rel.push((bi as u32, k as u32)),
                _ => for_uses(i, |u| other_uses.push(u)),
            }
        }
        term_uses(&b.term, &mut uses);
        other_uses.extend_from_slice(&uses);
    }
    for _ in 0..3 {
        let mut more = false;
        for &(bi, k) in &rel {
            {
                let i = &f.blocks[bi as usize].insts[k as usize];
                match i {
                    Inst::SlotAddr(d, sl) if slot_defs[d.0 as usize] == (*sl, ndef[d.0 as usize]) && addr[d.0 as usize].0 == u32::MAX => {
                        addr[d.0 as usize] = (*sl, 0);
                        more = true;
                    }
                    Inst::IBinI(IOp::Add, it, d, a, k) if it.bits == 64 && ndef[d.0 as usize] == 1 && addr[a.0 as usize].0 != u32::MAX && addr[d.0 as usize].0 == u32::MAX => {
                        let (sl, o) = addr[a.0 as usize];
                        addr[d.0 as usize] = (sl, o + *k);
                        more = true;
                    }
                    Inst::Mov(d, a) if f.vregs[d.0 as usize] == Cls::I && ndef[d.0 as usize] == 1 && addr[a.0 as usize].0 != u32::MAX && addr[d.0 as usize].0 == u32::MAX => {
                        addr[d.0 as usize] = addr[a.0 as usize];
                        more = true;
                    }
                    _ => {}
                }
            }
        }
        if !more {
            break;
        }
    }
    let at = |v: VReg| addr[v.0 as usize];
    // a slot qualifies when its addresses are only bases, copy operands or derivations
    let mut ok = vec![true; ns];
    // slots whose address is passed to calls that are their last uses (an iterator handed
    // to its drop after the loop): the cells live in registers and are written back to the
    // slot right before such a call. Nothing touches the slot after it on any path, so no
    // pointer the callee may keep can see stale cells.
    let mut esc = vec![false; ns];
    let mut touched = false;
    for &(bi, k) in &rel {
        {
            let i = &f.blocks[bi as usize].insts[k as usize];
            match i {
                Inst::Load(_, _, base, off) | Inst::VLoad(_, base, off) if at(*base).0 != u32::MAX => {
                    touched = true;
                    // an access outside the slot (a branch that is dead for this instance:
                    // the second word of a thin pointer) keeps the slot in memory
                    let n = if let Inst::Load(m, ..) = i { mem_size(*m) } else { 16 };
                    let (sl, o) = at(*base);
                    if o + (*off as i64) < 0 || o + *off as i64 + n as i64 > f.slots[sl as usize].size as i64 {
                        sroa_reject_slot(&mut ok, sl as usize, SroaReject::Other, &mut stats);
                    }
                }
                Inst::Store(_, base, off, v) | Inst::VStore(base, off, v) => {
                    if at(*base).0 != u32::MAX {
                        touched = true;
                        let n = if let Inst::Store(m, ..) = i { mem_size(*m) } else { 16 };
                        let (sl, o) = at(*base);
                        if o + (*off as i64) < 0 || o + *off as i64 + n as i64 > f.slots[sl as usize].size as i64 {
                            sroa_reject_slot(&mut ok, sl as usize, SroaReject::Other, &mut stats);
                        }
                    }
                    // the stored value must not be an address itself
                    if at(*v).0 != u32::MAX {
                        sroa_reject_slot(&mut ok, at(*v).0 as usize, SroaReject::Escape, &mut stats);
                    }
                }
                Inst::Copy(d, s2, n) => {
                    for v in [*d, *s2] {
                        let (sl, o) = at(v);
                        if sl != u32::MAX {
                            touched = true;
                            if o < 0 || o + *n as i64 > f.slots[sl as usize].size as i64 {
                                sroa_reject_slot(&mut ok, sl as usize, SroaReject::Other, &mut stats);
                            }
                        }
                    }
                }
                Inst::SlotAddr(..) => {}
                Inst::IBinI(IOp::Add, it, d, a, _) if it.bits == 64 && at(*a).0 != u32::MAX && at(*d).0 != u32::MAX => {}
                Inst::Mov(d, a) if at(*a).0 != u32::MAX && at(*d).0 != u32::MAX => {}
                Inst::Call(c, args, _) => {
                    for u in args {
                        if at(*u).0 != u32::MAX {
                            esc[at(*u).0 as usize] = true;
                        }
                    }
                    if let Callee::Indirect(v) | Callee::CIndirect(v, _) = c {
                        if at(*v).0 != u32::MAX {
                            sroa_reject_slot(&mut ok, at(*v).0 as usize, SroaReject::Escape, &mut stats);
                        }
                    }
                }
                _ => {
                    uses_defs(i, &mut uses, &mut defs);
                    for u in &uses {
                        if at(*u).0 != u32::MAX {
                            sroa_reject_slot(&mut ok, at(*u).0 as usize, SroaReject::Escape, &mut stats);
                        }
                    }
                }
            }
        }
    }
    for u in &other_uses {
        if at(*u).0 != u32::MAX {
            sroa_reject_slot(&mut ok, at(*u).0 as usize, SroaReject::Escape, &mut stats);
        }
    }
    if esc.iter().any(|x| *x) {
        for &(bi, k) in &rel {
            {
                let i = &f.blocks[bi as usize].insts[k as usize];
                if let Inst::Copy(d, s2, _) = i {
                    for v in [*d, *s2] {
                        if at(v).0 != u32::MAX && esc[at(v).0 as usize] {
                            sroa_reject_slot(&mut ok, at(v).0 as usize, SroaReject::Escape, &mut stats);
                        }
                    }
                }
            }
        }
    }
    if !touched || !ok.iter().any(|x| *x) {
        if let Some(s) = &mut stats {
            s.rejects[SroaReject::Other as usize] += ok.iter().filter(|x| **x).count();
        }
        return false;
    }
    let mem_key = |m: Mem| -> u32 {
        match m {
            Mem::Int(n, sg) => n as u32 * 2 + sg as u32,
            Mem::F32 => 100,
            Mem::F64 => 101,
        }
    };
    // groups: union-find with offsets (slot offset x = root offset x + delta[slot])
    let mut root = vec![0u32; ns];
    let mut delta = vec![0i64; ns];
    let mut gcells: Vec<Vec<SroaCell>>;
    let mut first_groups = true;
    let depth = crate::regalloc::loop_depths(f, &crate::regalloc::reachable(f));
    loop {
        fn find(p: &mut [u32], dl: &mut [i64], x: u32) -> (u32, i64) {
            // (root, delta of x relative to root)
            let mut path = Vec::new();
            let mut r = x;
            while p[r as usize] != r {
                path.push(r);
                r = p[r as usize];
            }
            // compress: deltas accumulate from x up to the root
            let mut acc = 0i64;
            for &y in path.iter().rev() {
                acc += dl[y as usize];
                dl[y as usize] = acc;
                p[y as usize] = r;
            }
            (r, if x == r { 0 } else { dl[x as usize] })
        }
        let mut par: Vec<u32> = (0..ns as u32).collect();
        let mut dl = vec![0i64; ns];
        let mut bad = vec![None; ns];
        for &(bi, k) in &rel {
            let i = &f.blocks[bi as usize].insts[k as usize]; {
                if let Inst::Copy(d, s2, _) = i {
                    let ((a, ao), (c, co)) = (at(*d), at(*s2));
                    if a != u32::MAX && c != u32::MAX && ok[a as usize] && ok[c as usize] {
                        // byte ao + x of a is byte co + x of c: pos_a(y) = pos_c(y - ao + co)
                        let (ra, da) = find(&mut par, &mut dl, a);
                        let (rc, dc) = find(&mut par, &mut dl, c);
                        // root coords: a's y -> y + da; c's z -> z + dc; need ao + da == co + dc
                        if ra == rc {
                            if ao + da != co + dc {
                                bad[ra as usize] = Some(SroaReject::Other);
                            }
                        } else {
                            // hang ra under rc: ra's coords shift by k so that ao + da + k == co + dc
                            par[ra as usize] = rc;
                            dl[ra as usize] = co + dc - ao - da;
                            if first_groups {
                                if let Some(s) = &mut stats {
                                    s.candidates -= 1;
                                }
                            }
                        }
                    }
                }
            }
        }
        for sl in 0..ns {
            let (r, d) = find(&mut par, &mut dl, sl as u32);
            root[sl] = r;
            delta[sl] = d;
        }
        for sl in 0..ns {
            if let Some(why) = bad[sl] {
                bad[root[sl] as usize] = Some(why);
            }
        }
        first_groups = false;
        // per group root: cells in root coordinates, copies in from / out to memory
        let mut cells: Vec<(u32, i64, Mem)> = Vec::new();
        let mut has_in = vec![false; ns];
        let mut has_out = vec![false; ns];
        let mut access_depth = vec![0u32; ns];
        let mut copies: Vec<(u32, i64, i64, u32)> = Vec::new(); // (group, lo, hi, loop depth)
        let okat = |v: VReg| -> Option<(u32, i64)> {
            let (sl, o) = at(v);
            if sl != u32::MAX && ok[sl as usize] {
                Some((sl, o))
            } else {
                None
            }
        };
        for &(bi, k) in &rel {
            let i = &f.blocks[bi as usize].insts[k as usize]; {
                match i {
                    Inst::Load(m, _, base, off) | Inst::Store(m, base, off, _) => {
                        if let Some((sl, o)) = okat(*base) {
                            cells.push((root[sl as usize], o + *off as i64 + delta[sl as usize], *m));
                            access_depth[root[sl as usize] as usize] = access_depth[root[sl as usize] as usize].max(depth[bi as usize]);
                        }
                    }
                    Inst::VLoad(_, base, off) | Inst::VStore(base, off, _) => {
                        if let Some((sl, o)) = okat(*base) {
                            cells.push((root[sl as usize], o + *off as i64 + delta[sl as usize], SROA_V128));
                            access_depth[root[sl as usize] as usize] = access_depth[root[sl as usize] as usize].max(depth[bi as usize]);
                        }
                    }
                    Inst::Copy(d, s2, n) => {
                        let (ga, gc) = (okat(*d), okat(*s2));
                        if let Some((sl, o)) = ga {
                            let lo = o + delta[sl as usize];
                            copies.push((root[sl as usize], lo, lo + *n as i64, depth[bi as usize]));
                            if gc.is_none() {
                                has_in[root[sl as usize] as usize] = true;
                            }
                        }
                        if let Some((sl, o)) = gc {
                            let lo = o + delta[sl as usize];
                            copies.push((root[sl as usize], lo, lo + *n as i64, depth[bi as usize]));
                            if ga.is_none() {
                                has_out[root[sl as usize] as usize] = true;
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        cells.sort_unstable_by_key(|c| (c.0, c.1, mem_key(c.2)));
        cells.dedup_by(|b, a| a.0 == b.0 && a.1 == b.1 && mem_key(a.2) == mem_key(b.2));
        // Maximal overlapping components, including different kinds at the same offset.
        // Adjacent (but disjoint) cells stay separate.
        gcells = vec![Vec::new(); ns];
        let mut start = 0;
        while start < cells.len() {
            let (g, lo, m) = cells[start];
            let mut hi = lo + mem_size(m) as i64;
            let mut end = start + 1;
            let mut kind_mix = false;
            while end < cells.len() && cells[end].0 == g && cells[end].1 < hi {
                kind_mix |= cells[end].1 == cells[end - 1].1;
                hi = hi.max(cells[end].1 + mem_size(cells[end].2) as i64);
                end += 1;
            }
            let widened = end > start + 1;
            let size = hi - lo;
            // pure: every access is at lo with the cell's size (only the kind differs: bitcasts)
            let pure = cells[start..end].iter().all(|c| c.1 == lo && mem_size(c.2) as i64 == size);
            if !pure {
                bad[g as usize].get_or_insert(if kind_mix { SroaReject::KindMix } else { SroaReject::Overlap });
            } else {
                gcells[g as usize].push(SroaCell { off: lo, mem: if widened { Mem::Int(size as u8, false) } else { m }, widened });
            }
            start = end;
        }
        copies.sort_unstable();
        copies.dedup();
        // A group copied both ways must preserve even bytes that no scalar access names.
        // Split gaps at every copy endpoint so tiling itself never introduces a copy cut.
        let mut ci = 0;
        while ci < copies.len() {
            let g = copies[ci].0 as usize;
            let mut ce = ci + 1;
            while ce < copies.len() && copies[ce].0 as usize == g {
                ce += 1;
            }
            if has_in[g] && has_out[g] && bad[g].is_none() && !tile {
                bad[g] = Some(SroaReject::CopyInOut);
            } else if has_in[g] && has_out[g] && bad[g].is_none() {
                if copies[ci..ce].iter().any(|c| c.2 - c.1 > 64) {
                    bad[g] = Some(SroaReject::CopyInOut);
                } else {
                    let mut bounds: Vec<i64> = copies[ci..ce].iter().flat_map(|c| [c.1, c.2]).collect();
                    bounds.sort_unstable();
                    bounds.dedup();
                    let mut gaps = Vec::new();
                    let mut k = 0;
                    let mut range = ci;
                    let mut covered_hi = i64::MIN;
                    for w in bounds.windows(2) {
                        let (lo, hi) = (w[0], w[1]);
                        while range < ce && copies[range].1 <= lo {
                            covered_hi = covered_hi.max(copies[range].2);
                            range += 1;
                        }
                        if hi > covered_hi {
                            continue;
                        }
                        let mut off = lo;
                        while off < hi {
                            while k < gcells[g].len() && gcells[g][k].off + mem_size(gcells[g][k].mem) as i64 <= off {
                                k += 1;
                            }
                            if k < gcells[g].len() && gcells[g][k].off <= off {
                                off = hi.min(gcells[g][k].off + mem_size(gcells[g][k].mem) as i64);
                                continue;
                            }
                            let end = gcells[g].get(k).map_or(hi, |c| hi.min(c.off));
                            while off < end {
                                let mut n = 8u8;
                                while off % n as i64 != 0 || off + n as i64 > end {
                                    n /= 2;
                                }
                                gaps.push(SroaCell { off, mem: Mem::Int(n, false), widened: false });
                                off += n as i64;
                            }
                        }
                    }
                    gcells[g].extend(gaps);
                    gcells[g].sort_unstable_by_key(|c| c.off);
                    // Each copy of a tiled group becomes one load or store per cell: narrow
                    // cells (a 16-byte group read bytewise) turn a two-word copy into 16 byte
                    // moves per copy (brotli ProcessCommandsInternal: +28% instructions). Keep
                    // the group in memory unless every copied range has at most two cells per
                    // 8-byte word (plus two for an unaligned tail).
                    let dense = copies[ci..ce].iter().all(|&(_, lo, hi, _)| {
                        let n = gcells[g].iter().filter(|c| c.off >= lo && c.off < hi).count() as i64;
                        n <= (hi - lo + 7) / 8 * 2 + 2
                    });
                    if !dense {
                        bad[g] = Some(SroaReject::CopyCells);
                    }
                }
            }
            ci = ce;
        }
        // A copy may not cut a cell in two. Keep large copies intact unless
        // promotion removes accesses in a deeper loop: initialization of a
        // compound iterator should not keep its state in memory per element.
        for &(g, lo, hi, copy_depth) in &copies {
            let g = g as usize;
            let mut cnt = 0;
            for c in &gcells[g] {
                let end = c.off + mem_size(c.mem) as i64;
                if (c.off < lo && end > lo) || (c.off < hi && end > hi) {
                    bad[g].get_or_insert(SroaReject::CopyCut);
                }
                if c.off >= lo && end <= hi {
                    cnt += 1;
                }
            }
            let limit = if has_in[g] && has_out[g] { SROA_COPY_IN_OUT_CELLS } else { SROA_COPY_CELLS };
            if cnt > limit && copy_depth >= access_depth[g] {
                bad[g].get_or_insert(SroaReject::CopyCells);
            }
        }
        if let Some(s) = &mut stats {
            for g in 0..ns {
                if ok[g] && root[g] as usize == g {
                    if let Some(why) = bad[g] {
                        s.rejects[why as usize] += 1;
                    }
                }
            }
        }
        let mut changed = false;
        for sl in 0..ns {
            if ok[sl] && bad[root[sl] as usize].is_some() {
                ok[sl] = false;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    // escaped slots: every escaping call must be a last use (no access of the slot after it,
    // on any path, the call's own block included)
    let mut esc_slots: Vec<u32> = Vec::new();
    // (block, instruction, slot): no-capture calls after which the slot's cells are reloaded
    let mut reload_sites: Vec<(u32, u32, u32)> = Vec::new();
    // The deepest loop a load / store of each escaped slot sits in.
    let acc_depth = if (0..ns).any(|sl| ok[sl] && esc[sl]) {
        let mut ad = vec![0u32; ns];
        for (bi, b) in f.blocks.iter().enumerate() {
            for i in &b.insts {
                if let Inst::Load(_, _, base, _) | Inst::Store(_, base, _, _) | Inst::VLoad(_, base, _) | Inst::VStore(base, _, _) = i {
                    let sl = at(*base).0;
                    if sl != u32::MAX {
                        ad[sl as usize] = ad[sl as usize].max(depth[bi]);
                    }
                }
            }
        }
        ad
    } else {
        Vec::new()
    };
    if (0..ns).any(|sl| ok[sl] && esc[sl]) {
        let nb = f.blocks.len();
        let succ = |bi: usize| -> Vec<usize> { f.blocks[bi].term.succs().map(|t| t as usize).collect() };
        // per slot: blocks that access it (loads/stores/copies through its addresses)
        // an access: a load or store, or another call that gets the slot (a call in a loop
        // reaches itself: the iterator handed to a non-inlined next())
        let accesses = |i: &Inst, sl: u32| -> bool {
            match i {
                Inst::Load(_, _, b, _) | Inst::Store(_, b, _, _) | Inst::VLoad(_, b, _) | Inst::VStore(b, _, _) => at(*b).0 == sl,
                Inst::Call(_, args, _) => args.iter().any(|a| at(*a).0 == sl),
                _ => false,
            }
        };
        let mut seen = vec![false; nb];
        // escape call sites per candidate slot, in block / instruction order (one scan)
        let mut sites: Vec<Vec<(u32, u32)>> = vec![Vec::new(); ns];
        for &(bi, k) in &rel {
            let (i, bi, k) = (&f.blocks[bi as usize].insts[k as usize], bi as usize, k as usize); {
                if let Inst::Call(_, args, _) = i {
                    for a in args {
                        let sl = at(*a).0;
                        if sl != u32::MAX && ok[sl as usize] && esc[sl as usize] && sites[sl as usize].last() != Some(&(bi as u32, k as u32)) {
                            sites[sl as usize].push((bi as u32, k as u32));
                        }
                    }
                }
            }
        }
        for sl in 0..ns {
            if !(ok[sl] && esc[sl]) {
                continue;
            }
            // few escape sites only (each last-use site costs a reachability walk; no-capture
            // sites are cheap: a Vec's push loop has a few grow calls)
            if sites[sl].len() > 12 {
                sroa_reject_slot(&mut ok, sl, SroaReject::Escape, &mut stats);
                continue;
            }
            let mut good = true;
            'sites: for &(bi, k) in &sites[sl] {
                let (bi, k) = (bi as usize, k as usize);
                // a call that keeps no pointer to the slot (O3: proven from the callee's
                // RIR): the cells are written back before it and read again after it, so the
                // slot may be used afterwards (the iterator / Vec handed to a non-inlined
                // spec_extend ahead of the loop that then runs on registers)
                // ... and only when the call runs less often than the slot's accesses: a flush and
                // reload per call in the slot's own loop costs more than the memory traffic it
                // saves (bench_rt sort_ints / shape got slower with calls inside the hot loop)
                let nocap = depth[bi] < acc_depth[sl]
                    && match &f.blocks[bi].insts[k] {
                        Inst::Call(c, args, _) => args.iter().enumerate().all(|(j, a)| at(*a).0 != sl as u32 || (at(*a).1 == 0 && sroa_nocapture(c, j))),
                        _ => false,
                    };
                if nocap {
                    reload_sites.push((bi as u32, k as u32, sl as u32));
                    continue;
                }
                {
                    // later in the same block
                    if f.blocks[bi].insts[k + 1..].iter().any(|j| accesses(j, sl as u32)) {
                        good = false;
                        break 'sites;
                    }
                    // any block reachable from here
                    seen.iter_mut().for_each(|x| *x = false);
                    let mut stack = succ(bi);
                    while let Some(x) = stack.pop() {
                        if seen[x] {
                            continue;
                        }
                        seen[x] = true;
                        if f.blocks[x].insts.iter().any(|j| accesses(j, sl as u32)) {
                            good = false;
                            break 'sites;
                        }
                        stack.extend(succ(x));
                    }
                }
            }
            if good {
                esc_slots.push(sl as u32);
            } else {
                sroa_reject_slot(&mut ok, sl, SroaReject::Escape, &mut stats);
                reload_sites.retain(|r| r.2 != sl as u32);
            }
        }
    }
    if let Some(s) = &mut stats {
        s.promoted = (0..ns).filter(|&sl| ok[sl] && root[sl] as usize == sl).count();
    }
    if !ok.iter().any(|x| *x) {
        return false;
    }
    // one vreg per (slot, group cell inside the slot), zero at entry (a cell read before any
    // write of it reads padding or an unwritten variant field: any value will do)
    let mut slot_regs: Vec<Vec<u32>> = vec![Vec::new(); ns];
    let mut init: Vec<Inst> = Vec::new();
    let mut vzero = None;
    for sl in 0..ns {
        if !ok[sl] {
            continue;
        }
        let g = root[sl] as usize;
        let lo = delta[sl];
        let hi = delta[sl] + f.slots[sl].size as i64;
        let mut regs = Vec::with_capacity(gcells[g].len());
        for k in 0..gcells[g].len() {
            let SroaCell { off, mem: m, .. } = gcells[g][k];
            if off < lo || off + mem_size(m) as i64 > hi {
                regs.push(u32::MAX);
                continue;
            }
            if m == SROA_V128 {
                let z = match vzero {
                    Some(z) => z,
                    None => {
                        let z = f.vreg(Cls::I);
                        init.push(Inst::Iconst(z, 0));
                        vzero = Some(z);
                        z
                    }
                };
                let r = f.vreg(Cls::V128);
                init.push(Inst::VX(VX::FromLo64, r, z, z));
                regs.push(r.0);
                continue;
            }
            let r = f.vreg(match m {
                Mem::F32 => Cls::F32,
                Mem::F64 => Cls::F64,
                Mem::Int(..) => Cls::I,
            });
            init.push(match m {
                Mem::F32 => Inst::Fconst(r, 0, false),
                Mem::F64 => Inst::Fconst(r, 0, true),
                Mem::Int(..) => Inst::Iconst(r, 0),
            });
            regs.push(r.0);
        }
        slot_regs[sl] = regs;
    }
    let esc_addr: Vec<(u32, VReg)> = esc_slots.iter().map(|&sl| (sl, f.vreg(Cls::I))).collect();
    for &(sl, v) in &esc_addr {
        init.push(Inst::SlotAddr(v, sl));
    }
    // Every promoted access names an entire cell, including mixed-kind scalar accesses.
    let cell_at = |sl: u32, roff: i64| -> (VReg, SroaCell) {
        let g = root[sl as usize] as usize;
        let k = gcells[g].binary_search_by_key(&roff, |c| c.off).expect("sroa cell");
        let c = gcells[g][k];
        let r = slot_regs[sl as usize][k];
        assert!(r != u32::MAX, "sroa cell outside its slot");
        (VReg(r), c)
    };
    let cell = |sl, roff| cell_at(sl, roff).0;
    let okat = |v: VReg| -> Option<(u32, i64)> {
        let (sl, o) = at(v);
        if sl != u32::MAX && ok[sl as usize] {
            Some((sl, o + delta[sl as usize]))
        } else {
            None
        }
    };
    // only blocks with an access of a replaced slot are rebuilt
    let touches = |i: &Inst| -> bool {
        match i {
            Inst::Load(_, _, base, _) | Inst::VLoad(_, base, _) | Inst::Store(_, base, _, _) | Inst::VStore(base, _, _) => okat(*base).is_some(),
            Inst::Copy(d, s2, _) => okat(*d).is_some() || okat(*s2).is_some(),
            Inst::Call(_, args, _) => !esc_addr.is_empty() && args.iter().any(|x| esc_addr.iter().any(|e| e.0 == at(*x).0)),
            _ => false,
        }
    };
    let mut dirty = vec![false; f.blocks.len()];
    for &(bi, k) in &rel {
        if !dirty[bi as usize] && touches(&f.blocks[bi as usize].insts[k as usize]) {
            dirty[bi as usize] = true;
        }
    }
    // a reload site's cells, read back from the slot after the call (as many as the flush wrote)
    let reload = |sl: u32, a: VReg, out: &mut Vec<Inst>| {
        let g = root[sl as usize] as usize;
        for (k, &SroaCell { off, mem: m, .. }) in gcells[g].iter().enumerate() {
            if slot_regs[sl as usize][k] != u32::MAX {
                let (o, r) = ((off - delta[sl as usize]) as i32, VReg(slot_regs[sl as usize][k]));
                out.push(if m == SROA_V128 { Inst::VLoad(r, a, o) } else { Inst::Load(m, r, a, o) });
            }
        }
    };
    // invoked reload sites: the reloads go on both edges (blocks made below)
    let mut edge_reloads: Vec<(usize, u32, VReg)> = Vec::new();
    for (bi, (b, _)) in f.blocks.iter_mut().zip(dirty.iter()).enumerate().filter(|x| *x.1 .1) {
        let mut out: Vec<Inst> = Vec::with_capacity(b.insts.len());
        let mut pos: Vec<u64> = Vec::with_capacity(b.pos.len());
        for (k, i) in b.insts.iter().enumerate() {
            let p = b.pos[k];
            match i {
                Inst::Load(m, d, base, off) if okat(*base).is_some() => {
                    let (sl, o) = okat(*base).unwrap();
                    let (r, c) = cell_at(sl, o + *off as i64);
                    if c.widened {
                        out.push(match *m {
                            Mem::Int(n, signed) if n < 8 => Inst::Conv(Conv::IntToInt(IntTy { bits: n * 8, signed }), *d, r),
                            Mem::Int(..) => Inst::Mov(*d, r),
                            Mem::F32 | Mem::F64 => Inst::Conv(Conv::BitsToF(*m == Mem::F64), *d, r),
                        });
                    } else {
                        out.push(Inst::Mov(*d, r));
                    }
                    pos.push(p);
                }
                Inst::VLoad(d, base, off) if okat(*base).is_some() => {
                    let (sl, o) = okat(*base).unwrap();
                    out.push(Inst::Mov(*d, cell(sl, o + *off as i64)));
                    pos.push(p);
                }
                Inst::VStore(base, off, v) if okat(*base).is_some() => {
                    let (sl, o) = okat(*base).unwrap();
                    out.push(Inst::Mov(cell(sl, o + *off as i64), *v));
                    pos.push(p);
                }
                Inst::Store(m, base, off, v) if okat(*base).is_some() => {
                    let (sl, o) = okat(*base).unwrap();
                    let (r, c) = cell_at(sl, o + *off as i64);
                    if c.widened {
                        out.push(match m {
                            Mem::F32 | Mem::F64 => Inst::Conv(Conv::FToBits(*m == Mem::F64), r, *v),
                            Mem::Int(..) => Inst::Mov(r, *v),
                        });
                    } else {
                        out.push(match m {
                            Mem::Int(n, sg) if *n < 8 => Inst::Conv(Conv::IntToInt(IntTy { bits: *n * 8, signed: *sg }), r, *v),
                            _ => Inst::Mov(r, *v),
                        });
                    }
                    pos.push(p);
                }
                Inst::Copy(d, s2, n) if okat(*d).is_some() || okat(*s2).is_some() => {
                    let (da, sa) = (okat(*d), okat(*s2));
                    // the group's cells inside the copied range, in root coordinates
                    let (g, lo) = match (da, sa) {
                        (Some((sl, o)), _) => (root[sl as usize] as usize, o),
                        (None, Some((sl, o))) => (root[sl as usize] as usize, o),
                        _ => unreachable!(),
                    };
                    let src_lo = sa.map(|x| x.1).unwrap_or(0);
                    for &SroaCell { off, mem: m, .. } in &gcells[g] {
                        if off < lo || off + mem_size(m) as i64 > lo + *n as i64 {
                            continue;
                        }
                        let rel = off - lo; // byte offset inside the copied range
                        out.push(match (da, sa) {
                            (Some((dsl, _)), Some((ssl, _))) => Inst::Mov(cell(dsl, off), cell(ssl, src_lo + rel)),
                            (Some((dsl, _)), None) if m == SROA_V128 => Inst::VLoad(cell(dsl, off), *s2, rel as i32),
                            (None, Some((ssl, _))) if m == SROA_V128 => Inst::VStore(*d, rel as i32, cell(ssl, src_lo + rel)),
                            (Some((dsl, _)), None) => Inst::Load(m, cell(dsl, off), *s2, rel as i32),
                            (None, Some((ssl, _))) => Inst::Store(m, *d, rel as i32, cell(ssl, src_lo + rel)),
                            _ => unreachable!(),
                        });
                        pos.push(p);
                    }
                }
                Inst::Call(_, args, _) if !esc_addr.is_empty() => {
                    // an escaped slot's last use: the slot gets its cells' values first
                    for &(sl, a) in &esc_addr {
                        if !args.iter().any(|x| at(*x).0 == sl) {
                            continue;
                        }
                        let g = root[sl as usize] as usize;
                        for (k, &SroaCell { off, mem: m, .. }) in gcells[g].iter().enumerate() {
                            if slot_regs[sl as usize][k] != u32::MAX {
                                let (o, r) = ((off - delta[sl as usize]) as i32, VReg(slot_regs[sl as usize][k]));
                                out.push(if m == SROA_V128 { Inst::VStore(a, o, r) } else { Inst::Store(m, a, o, r) });
                                pos.push(p);
                            }
                        }
                    }
                    out.push(i.clone());
                    pos.push(p);
                    for &(sb, sk, sl) in &reload_sites {
                        if sb as usize != bi || sk as usize != k {
                            continue;
                        }
                        let a = esc_addr.iter().find(|e| e.0 == sl).map(|e| e.1).expect("reload site slot");
                        if k + 1 == b.insts.len() && matches!(b.term, Term::Invoke(..)) {
                            edge_reloads.push((bi, sl, a));
                        } else {
                            let n0 = out.len();
                            reload(sl, a, &mut out);
                            pos.extend(std::iter::repeat(p).take(out.len() - n0));
                        }
                    }
                }
                _ => {
                    out.push(i.clone());
                    pos.push(p);
                }
            }
        }
        b.insts = out;
        b.pos = pos;
    }
    // an invoked no-capture call: its normal and unwind successors each get a block that
    // reloads the cells first (the callee may have written the slot before unwinding)
    let mut by_block: Vec<(usize, Vec<(u32, VReg)>)> = Vec::new();
    for (bi, sl, a) in edge_reloads {
        match by_block.iter_mut().find(|x| x.0 == bi) {
            Some(x) => x.1.push((sl, a)),
            None => by_block.push((bi, vec![(sl, a)])),
        }
    }
    for (bi, sls) in by_block {
        let Term::Invoke(n, u) = f.blocks[bi].term else { continue };
        let p = f.blocks[bi].term_pos;
        let mut targets = [n, u];
        for t in targets.iter_mut() {
            let mut insts = Vec::new();
            for &(sl, a) in &sls {
                reload(sl, a, &mut insts);
            }
            let np = insts.len();
            let nb = f.blocks.len() as u32;
            f.blocks.push(Block { insts, pos: vec![p; np], term: Term::Jump(*t), term_pos: p });
            *t = nb;
        }
        f.blocks[bi].term = Term::Invoke(targets[0], targets[1]);
    }
    let p0 = f.blocks[0].pos.first().copied().unwrap_or(0);
    let n0 = init.len();
    let b0 = &mut f.blocks[0];
    b0.insts.splice(0..0, init);
    b0.pos.splice(0..0, std::iter::repeat(p0).take(n0));
    true
}

fn mem_size(m: Mem) -> i32 {
    match m {
        Mem::Int(n, _) => n as i32,
        Mem::F32 => 4,
        Mem::F64 => 8,
    }
}

/// Loop-invariant code motion: pure, non-trapping, single-definition instructions inside a
/// natural loop whose operands come from outside it (or from other hoisted instructions)
/// move to the end of the loop's preheader (its single outside predecessor ending in a
/// jump to the header). Loads stay (stores/calls in the loop), integer div/rem stay (trap).
/// No edge goes to the same or an earlier block: the CFG has no cycle, so no loop.
fn forward_only(f: &Func) -> bool {
    f.blocks.iter().enumerate().all(|(bi, b)| b.term.succs().all(|t| t as usize > bi))
}

pub fn licm(f: &mut Func) -> bool {
    if forward_only(f) {
        return false;
    }
    let n = f.blocks.len();
    let idom = idoms(f);
    let dominates = |a: u32, b: u32| -> bool {
        let mut x = b;
        let mut guard = 0;
        loop {
            if x == a {
                return true;
            }
            let p = idom[x as usize];
            if p == u32::MAX || p == x || guard > 10000 {
                return false;
            }
            x = p;
            guard += 1;
        }
    };
    let mut preds: Vec<Vec<u32>> = vec![Vec::new(); n];
    for (bi, b) in f.blocks.iter().enumerate() {
        if idom[bi] == u32::MAX {
            continue;
        }
        for t in b.term.succs() {
            preds[t as usize].push(bi as u32);
        }
    }
    // natural loops per header
    let mut loops: Vec<(u32, Vec<bool>)> = Vec::new();
    for h in 0..n {
        let mut latches = Vec::new();
        for &p in &preds[h] {
            if dominates(h as u32, p) {
                latches.push(p);
            }
        }
        if latches.is_empty() {
            continue;
        }
        let mut body = vec![false; n];
        body[h] = true;
        let mut stack = latches.clone();
        while let Some(x) = stack.pop() {
            if body[x as usize] {
                continue;
            }
            body[x as usize] = true;
            for &p in &preds[x as usize] {
                stack.push(p);
            }
        }
        loops.push((h as u32, body));
    }
    // inner loops first
    loops.sort_by_key(|l| l.1.iter().filter(|x| **x).count());
    let nv = f.vregs.len();
    let mut uses = Vec::new();
    let mut defs = Vec::new();
    let mut ndef = vec![0u32; nv];
    for p in &f.params {
        ndef[p.0 as usize] += 1;
    }
    let mut def_block = vec![u32::MAX; nv];
    for (bi, b) in f.blocks.iter().enumerate() {
        for i in &b.insts {
            for_defs(i, |d| {
                ndef[d.0 as usize] += 1;
                def_block[d.0 as usize] = bi as u32;
            });
        }
    }
    let mut changed = false;
    for (h, body) in &loops {
        let outside: Vec<u32> = preds[*h as usize].iter().copied().filter(|p| !body[*p as usize]).collect();
        if outside.len() != 1 {
            continue;
        }
        let pre = outside[0];
        if !matches!(f.blocks[pre as usize].term, Term::Jump(t) if t == *h) {
            continue;
        }
        let mut hoisted: Vec<(Inst, u64)> = Vec::new();
        // a loop that writes no memory and calls nothing: loads from dereferenceable params
        // read the same value on every iteration and may run before it
        let read_only = (0..n).all(|bi| !body[bi] || (f.blocks[bi].insts.iter().all(|i| !is_mem_effect(i)) && matches!(f.blocks[bi].term, Term::Jump(_) | Term::Branch(..) | Term::Ret(_) | Term::Unreachable)));
        // vregs with a definition inside the loop (multi-def ones never become invariant
        // through hoisting: only single-def instructions move)
        let mut def_in_loop = vec![false; nv];
        for bi in 0..n {
            if body[bi] {
                for i2 in &f.blocks[bi].insts {
                    for_defs(i2, |d| def_in_loop[d.0 as usize] = true);
                }
            }
        }
        let mut progress = true;
        while progress {
            progress = false;
            // constants some invariant user needs: a pure, single-def user in the loop whose
            // only in-loop operand is the constant (computed once per sweep: it was a scan of
            // the whole loop per constant)
            let mut needed = vec![false; nv];
            for b2 in 0..n {
                if !body[b2] {
                    continue;
                }
                for i2 in &f.blocks[b2].insts {
                    if matches!(i2, Inst::Load(..) | Inst::Store(..) | Inst::Call(..) | Inst::Copy(..) | Inst::Mov(..)) {
                        continue;
                    }
                    uses_defs(i2, &mut uses, &mut defs);
                    if !(defs.len() == 1 && ndef[defs[0].0 as usize] == 1) {
                        continue;
                    }
                    let mut inloop: Option<VReg> = None;
                    let mut ok = true;
                    for u in &uses {
                        let db = def_block[u.0 as usize];
                        if db != u32::MAX && body[db as usize] {
                            match inloop {
                                None => inloop = Some(*u),
                                Some(x) if x == *u => {}
                                Some(_) => ok = false,
                            }
                        }
                    }
                    if let (true, Some(v)) = (ok, inloop) {
                        needed[v.0 as usize] = true;
                    }
                }
            }
            for bi in 0..n {
                if !body[bi] {
                    continue;
                }
                let mut k = 0;
                while k < f.blocks[bi].insts.len() {
                    let i = &f.blocks[bi].insts[k];
                    let pure = match i {
                        Inst::IBin(IOp::Div | IOp::Rem, ..) | Inst::IBinI(IOp::Div | IOp::Rem, ..) => false,
                        Inst::Load(m, _, base, off) if read_only && *off >= 0 && ndef[base.0 as usize] == 1 && f.deref.iter().any(|(p, sz)| p == base && (*off as i64 + mem_size(*m) as i64) <= *sz as i64) => true,
                        Inst::Load(..) | Inst::VLoad(..) | Inst::VGather(..) | Inst::Mov(..) => false,
                        i if is_mem_effect(i) => false,
                        _ => true,
                    };
                    if !pure {
                        k += 1;
                        continue;
                    }
                    uses_defs(i, &mut uses, &mut defs);
                    let mut inv = defs.len() == 1 && ndef[defs[0].0 as usize] == 1;
                    for u in &uses {
                        let db = def_block[u.0 as usize];
                        let in_loop = db != u32::MAX && body[db as usize];
                        // multi-def operands are invariant only if no definition is in the loop
                        if in_loop || (ndef[u.0 as usize] > 1 && def_in_loop[u.0 as usize]) {
                            inv = false;
                        }
                    }
                    // bare constants are not worth a register across the loop on their own:
                    // hoisted only when some invariant user needs it
                    let is_const = matches!(i, Inst::Iconst(..) | Inst::Fconst(..) | Inst::Addr(..) | Inst::FnAddr(..) | Inst::SlotAddr(..) | Inst::VConst(..));
                    if inv && is_const {
                        inv = needed[defs[0].0 as usize];
                    }
                    if inv {
                        let i = f.blocks[bi].insts.remove(k);
                        let p = f.blocks[bi].pos.remove(k);
                        def_block[defs[0].0 as usize] = pre;
                        hoisted.push((i, p));
                        progress = true;
                        changed = true;
                    } else {
                        k += 1;
                    }
                }
            }
        }
        for (i, p) in hoisted {
            f.blocks[pre as usize].insts.push(i);
            f.blocks[pre as usize].pos.push(p);
        }
    }
    changed
}

/// 64-bit shifts by a register take the count mod 64 on both backends (x64 shl/shr/sar/shlx,
/// arm64 lslv/lsrv/asrv): `s = y & 63` feeding only such a shift, in the same block, goes.
fn fold_shift_masks(f: &mut Func) {
    let nv = f.vregs.len();
    let mut nuse = vec![0u32; nv];
    let mut uses = Vec::new();
    for b in &f.blocks {
        for i in &b.insts {
            for_uses(i, |u| nuse[u.0 as usize] += 1);
        }
        term_uses(&b.term, &mut uses);
        for u in &uses {
            nuse[u.0 as usize] += 1;
        }
    }
    let mut defs = Vec::new();
    for b in &mut f.blocks {
        let n = b.insts.len();
        let mut dead = vec![false; n];
        for k in 0..n {
            let (s0, it) = match b.insts[k] {
                Inst::IBin(IOp::Shl | IOp::Shr, it, _, _, s0) if it.bits == 64 => (s0, it),
                _ => continue,
            };
            let _ = it;
            if nuse[s0.0 as usize] != 1 {
                continue;
            }
            // the mask: defined earlier in this block, its source not redefined since
            let mut found = None;
            for j in (0..k).rev() {
                let mut defines = false;
                for_defs(&b.insts[j], |d| defines |= d == s0);
                if defines {
                    if let Inst::IBinI(IOp::And, mt, _, y, 63) = b.insts[j] {
                        if mt.bits == 64 {
                            found = Some((j, y));
                        }
                    }
                    break;
                }
            }
            let (j, y) = match found {
                Some(x) => x,
                None => continue,
            };
            let mut ok = true;
            for m in j + 1..k {
                defs.clear();
                for_defs(&b.insts[m], |d| defs.push(d));
                if defs.contains(&y) {
                    ok = false;
                }
            }
            if !ok {
                continue;
            }
            if let Inst::IBin(_, _, _, _, ref mut s1) = b.insts[k] {
                *s1 = y;
            }
            dead[j] = true;
        }
        if dead.iter().any(|x| *x) {
            let mut k = 0;
            b.insts.retain(|_| {
                k += 1;
                !dead[k - 1]
            });
            let mut k2 = 0;
            b.pos.retain(|_| {
                k2 += 1;
                !dead[k2 - 1]
            });
        }
    }
}

/// One address per stack slot (backend preparation, before addr_modes): lowering and constant
/// sinking leave a `SlotAddr(v, s)` next to each access; they become one value defined at the
/// entry, which the allocator keeps in a register, folds into the access (x64 [rbp + ..], arm64
/// [sp + ..]) or recomputes where it has no register (Alloc::remat), instead of an add per access.
fn unify_slot_addrs(f: &mut Func) {
    let nv = f.vregs.len();
    let mut ndef = vec![0u32; nv];
    let mut n_slot_defs = 0usize;
    // a call argument cannot be recomputed at its use (Alloc::remat excludes call arguments): an
    // entry definition of one stays live through the whole function and, in a large function, is
    // computed and spilled on every entry (a large interpreter's opcode(): ~245 slot addresses, 1238
    // instructions per call); such a SlotAddr keeps its own definition next to the call
    let mut call_arg = vec![false; nv];
    for b in &f.blocks {
        for i in &b.insts {
            for_defs(i, |d| ndef[d.0 as usize] += 1);
            if matches!(i, Inst::SlotAddr(..)) {
                n_slot_defs += 1;
            }
            if let Inst::Call(_, args, _) = i {
                for a in args {
                    call_arg[a.0 as usize] = true;
                }
            }
        }
    }
    if n_slot_defs < 2 {
        return;
    }
    let mut canon: std::collections::HashMap<u32, VReg> = std::collections::HashMap::new();
    let mut map: Vec<u32> = (0..nv as u32).collect();
    let mut order: Vec<u32> = Vec::new();
    for b in &f.blocks {
        for i in &b.insts {
            if let Inst::SlotAddr(d, s) = i {
                if ndef[d.0 as usize] == 1 && !call_arg[d.0 as usize] {
                    let c = *canon.entry(*s).or_insert_with(|| {
                        order.push(*s);
                        VReg(u32::MAX)
                    });
                    let _ = c;
                }
            }
        }
    }
    if order.is_empty() {
        return;
    }
    for &s in &order {
        let v = f.vreg(Cls::I);
        canon.insert(s, v);
    }
    for b in &f.blocks {
        for i in &b.insts {
            if let Inst::SlotAddr(d, s) = i {
                if ndef[d.0 as usize] == 1 && !call_arg[d.0 as usize] {
                    map[d.0 as usize] = canon[s].0;
                }
            }
        }
    }
    for b in &mut f.blocks {
        let mut k = 0;
        let mut keep = Vec::with_capacity(b.insts.len());
        for i in &b.insts {
            keep.push(!matches!(i, Inst::SlotAddr(d, _) if ndef[d.0 as usize] == 1 && !call_arg[d.0 as usize]));
        }
        b.insts.retain(|_| {
            k += 1;
            keep[k - 1]
        });
        let mut k2 = 0;
        b.pos.retain(|_| {
            k2 += 1;
            keep[k2 - 1]
        });
        for i in b.insts.iter_mut() {
            i.vregs_mut(|r| {
                if (r.0 as usize) < nv {
                    r.0 = map[r.0 as usize];
                }
            });
        }
        match &mut b.term {
            Term::Branch(c, _, _) => {
                if (c.0 as usize) < nv {
                    c.0 = map[c.0 as usize];
                }
            }
            Term::Ret(vs) => {
                for v in vs.iter_mut() {
                    if (v.0 as usize) < nv {
                        v.0 = map[v.0 as usize];
                    }
                }
            }
            _ => {}
        }
    }
    let p0 = f.blocks[0].pos.first().copied().unwrap_or(f.blocks[0].term_pos);
    let defs: Vec<Inst> = order.iter().map(|s| Inst::SlotAddr(canon[s], *s)).collect();
    let n = defs.len();
    f.blocks[0].insts.splice(0..0, defs);
    f.blocks[0].pos.splice(0..0, std::iter::repeat(p0).take(n));
}

/// Backend address modes (run on the codegen copy only, after every IR pass): a load or
/// store whose base is `p + (i << k)` (k <= 3) or `p + i`, computed in the same block for
/// this access only, becomes LoadX/StoreX and the address arithmetic disappears.
/// Copies of 1, 2, 4, 8 or 16 bytes become integer loads then stores, so address modes can fold
/// their addresses (a stable-partition step `copy(sel + count * 8, src, 8)` keeps `count << 3` and
/// the add as temporaries otherwise; as [base + index * 8] it is one store).
fn expand_small_copies(f: &mut Func) {
    for bi in 0..f.blocks.len() {
        if !f.blocks[bi].insts.iter().any(|i| matches!(i, Inst::Copy(_, _, 1 | 2 | 4 | 8 | 16))) {
            continue;
        }
        let old = std::mem::take(&mut f.blocks[bi].insts);
        let oldp = std::mem::take(&mut f.blocks[bi].pos);
        let mut ins = Vec::with_capacity(old.len() + 4);
        let mut pos = Vec::with_capacity(old.len() + 4);
        for (i, p) in old.into_iter().zip(oldp) {
            match i {
                Inst::Copy(d, s2, n @ (1 | 2 | 4 | 8 | 16)) => {
                    let (w, k) = if n == 16 { (8u8, 2) } else { (n as u8, 1) };
                    let m = Mem::Int(w, false);
                    let ts: Vec<VReg> = (0..k).map(|_| f.vreg(Cls::I)).collect();
                    // every load before any store, as one memcpy reads its source
                    for (j, t) in ts.iter().enumerate() {
                        ins.push(Inst::Load(m, *t, s2, j as i32 * w as i32));
                        pos.push(p);
                    }
                    for (j, t) in ts.iter().enumerate() {
                        ins.push(Inst::Store(m, d, j as i32 * w as i32, *t));
                        pos.push(p);
                    }
                }
                other => {
                    ins.push(other);
                    pos.push(p);
                }
            }
        }
        f.blocks[bi].insts = ins;
        f.blocks[bi].pos = pos;
    }
}

pub fn addr_modes(f: &mut Func) {
    unify_slot_addrs(f);
    fold_shift_masks(f);
    expand_small_copies(f);
    let nv = f.vregs.len();
    let mut uses = Vec::new();
    let mut defs = Vec::new();
    let mut nuse = vec![0u32; nv];
    let mut ndef = vec![0u32; nv];
    for p in &f.params {
        ndef[p.0 as usize] += 1;
    }
    for b in &f.blocks {
        for i in &b.insts {
            for_uses(i, |u| {
                nuse[u.0 as usize] += 1;
            });
            for_defs(i, |d| {
                ndef[d.0 as usize] += 1;
            });
        }
        term_uses(&b.term, &mut uses);
        for u in &uses {
            nuse[u.0 as usize] += 1;
        }
    }
    for b in &mut f.blocks {
        let n = b.insts.len();
        // index of the defining inst in this block for single-def vregs
        let mut def_at: std::collections::HashMap<u32, usize> = std::collections::HashMap::new();
        let mut dead = vec![false; n];
        for k in 0..n {
            let (m, base, off) = match &b.insts[k] {
                Inst::Load(m, _, base, off) => (*m, *base, *off),
                Inst::Store(m, base, off, _) => (*m, *base, *off),
                i => {
                    for_defs(i, |d| {
                        if ndef[d.0 as usize] == 1 {
                            def_at.insert(d.0, k);
                        }
                    });
                    continue;
                }
            };
            let _ = m;
            let ai = match def_at.get(&base.0) {
                Some(&ai) if nuse[base.0 as usize] == 1 && !dead[ai] => ai,
                _ => continue,
            };
            let (p, q) = match b.insts[ai] {
                Inst::IBin(IOp::Add, it, _, p, q) if it.bits == 64 => (p, q),
                _ => continue,
            };
            // which operand is the scaled index
            let mut found: Option<(VReg, VReg, u8, Option<usize>)> = None;
            for (pp, qq) in [(p, q), (q, p)] {
                if let Some(&si) = def_at.get(&qq.0) {
                    if nuse[qq.0 as usize] == 1 && !dead[si] {
                        if let Inst::IBinI(IOp::Shl, it, _, idx, sh) = b.insts[si] {
                            if it.bits == 64 && sh <= 3 {
                                found = Some((pp, idx, sh as u8, Some(si)));
                                break;
                            }
                        }
                    }
                }
            }
            let (pp, idx, sh, si) = match found {
                Some(x) => x,
                None => (p, q, 0, None),
            };
            // `idx = x +- c` computed in this block for this access only: index by x, fold c into
            // the offset (the add goes). When idx is used elsewhere too it is computed anyway:
            // indexing by it is free and keeps arm64's register-offset form.
            let idx_orig = idx;
            let mut idx = idx;
            let mut extra: i64 = 0;
            let mut fold_at: Option<usize> = None;
            if let Some(&ii) = def_at.get(&idx.0) {
                // x64: the displacement is free in every form, so an index used elsewhere too is
                // still replaced by its operand here (`a[j - 1]` reads [b + j*4 - 4]; the subtract
                // stays for its other uses and can then sink to them); arm64 keeps the
                // register-offset form instead
                let shared_ok = cfg!(target_arch = "x86_64") && !dead[ii];
                if (nuse[idx.0 as usize] == 1 && !dead[ii]) || shared_ok {
                    let xc = match b.insts[ii] {
                        Inst::IBinI(IOp::Add, it, _, x, c) if it.bits == 64 && c.abs() < (1 << 20) => Some((x, c)),
                        Inst::IBinI(IOp::Sub, it, _, x, c) if it.bits == 64 && c.abs() < (1 << 20) => Some((x, -c)),
                        _ => None,
                    };
                    if let Some((x, c)) = xc {
                        let mut ok2 = true;
                        for j in ii + 1..k {
                            uses_defs(&b.insts[j], &mut uses, &mut defs);
                            if defs.contains(&x) {
                                ok2 = false;
                            }
                        }
                        if ok2 {
                            idx = x;
                            extra = c << sh;
                            if nuse[idx_orig.0 as usize] == 1 {
                                fold_at = Some(ii);
                            } else {
                                nuse[idx_orig.0 as usize] -= 1;
                                nuse[x.0 as usize] += 1;
                            }
                        }
                    }
                }
            }
            // operands must not be redefined between their use in the arithmetic and here: the base
            // after the add, the index after the shift (or the add, unscaled); a def before its use
            // (a SlotAddr / constant made right before the add) is fine
            let idx_from = si.unwrap_or(ai);
            let mut clobbered = false;
            for j in idx_from.min(ai) + 1..k {
                uses_defs(&b.insts[j], &mut uses, &mut defs);
                if (j > ai && defs.contains(&pp)) || (j > idx_from && defs.contains(&idx)) {
                    clobbered = true;
                }
            }
            if clobbered {
                continue;
            }
            b.insts[k] = match b.insts[k] {
                Inst::Load(m, d, _, off2) => Inst::LoadX(m, d, pp, idx, sh, off2 + extra as i32),
                Inst::Store(m, _, off2, v) => Inst::StoreX(m, pp, idx, sh, off2 + extra as i32, v),
                ref other => other.clone(),
            };
            let _ = off;
            dead[ai] = true;
            if let Some(ii) = fold_at {
                dead[ii] = true;
            }
            if let Some(si) = si {
                dead[si] = true;
            }
        }
        if dead.iter().any(|x| *x) {
            let mut k = 0;
            let mut j = 0;
            b.insts.retain(|_| {
                let keep = !dead[k];
                k += 1;
                keep
            });
            b.pos.retain(|_| {
                let keep = !dead[j];
                j += 1;
                keep
            });
        }
    }
}

/// Loop rotation on the codegen copy: a latch ending in `jump H` to a loop header whose
/// block is only a short compare-and-branch gets a copy of that test, so each iteration
/// runs one conditional branch at the bottom instead of a jump plus the header test.
/// A latch whose branch goes back to the header on its false edge (`if skip { continue }`)
/// gets the copy as a new block laid out after it: the latch falls into the test instead of
/// jumping to the header. Header values used only in the header get fresh vregs in the copy;
/// a header value the loop reads elsewhere (`let i = it.start` of a range's `next`, a loaded length) keeps its
/// vreg -- the copy is the very code the jump would have run, so it defines the same values.
pub fn rotate_loops(f: &mut Func) {
    if forward_only(f) {
        return;
    }
    let n = f.blocks.len();
    let idom = idoms(f);
    let dominates = |a: u32, b: u32| -> bool {
        let mut x = b;
        let mut guard = 0;
        loop {
            if x == a {
                return true;
            }
            let p = idom[x as usize];
            if p == u32::MAX || p == x || guard > 10000 {
                return false;
            }
            x = p;
            guard += 1;
        }
    };
    let mut uses = Vec::new();
    let mut defs = Vec::new();
    let mut nuse = vec![0u32; f.vregs.len()];
    for b in &f.blocks {
        for i in &b.insts {
            for_uses(i, |u| nuse[u.0 as usize] += 1);
        }
        term_uses(&b.term, &mut uses);
        for u in &uses {
            nuse[u.0 as usize] += 1;
        }
    }
    // latches ending in a branch whose false edge goes to the header get a new block with the
    // header's copy there (laid out after the latch below)
    let mut added = false;
    for l in 0..n {
        let (h, branch_latch) = match f.blocks[l].term {
            Term::Jump(h) if idom[l] != u32::MAX && dominates(h, l as u32) => (h as usize, false),
            Term::Branch(_, t, e) if t != e && e as usize != l && idom[l] != u32::MAX && dominates(e, l as u32) => (e as usize, true),
            _ => continue,
        };
        let hb = f.blocks[h].clone();
        if hb.insts.len() > 4 || !matches!(hb.term, Term::Branch(..)) {
            continue;
        }
        // header defs must be used only inside the header (they get fresh copies)
        let mut ok = true;
        let mut local_uses = vec![0u32; f.vregs.len()];
        for i in &hb.insts {
            match i {
                Inst::Iconst(..) | Inst::Fconst(..) | Inst::ICmp(..) | Inst::ICmpI(..) | Inst::FCmp(..) | Inst::IBinI(..) | Inst::IBin(..) | Inst::Mov(..) | Inst::Conv(..) | Inst::Load(..) => {}
                _ => ok = false,
            }
            for_uses(i, |u| local_uses[u.0 as usize] += 1);
        }
        term_uses(&hb.term, &mut uses);
        for u in &uses {
            local_uses[u.0 as usize] += 1;
        }
        // (header def, its copy's vreg): u32::MAX = a fresh one, the def itself when the loop
        // reads it outside the header
        let mut map: Vec<(VReg, VReg)> = Vec::new();
        for i in &hb.insts {
            uses_defs(i, &mut uses, &mut defs);
            if matches!(i, Inst::IBin(IOp::Div | IOp::Rem, ..)) {
                ok = false;
            }
            for d in &defs {
                let keep = nuse[d.0 as usize] != local_uses[d.0 as usize];
                map.push((*d, if keep { *d } else { VReg(u32::MAX) }));
            }
        }
        // a header def read in the header before it is (re)defined carries its value around
        // the loop (`n = n - 1`): the copy must update the real vreg, not a fresh one
        let mut seen_def: Vec<VReg> = Vec::new();
        for i in &hb.insts {
            uses_defs(i, &mut uses, &mut defs);
            if uses.iter().any(|u| map.iter().any(|(a, b)| a == u && a != b) && !seen_def.contains(u)) {
                ok = false;
            }
            seen_def.extend(defs.iter().cloned());
        }
        if !ok {
            continue;
        }
        for m in map.iter_mut() {
            if m.1 == VReg(u32::MAX) {
                let c = f.vregs[m.0 .0 as usize];
                m.1 = f.vreg(c);
            }
        }
        let look = |v: VReg| -> VReg {
            for (a, b) in &map {
                if *a == v {
                    return *b;
                }
            }
            v
        };
        let insts: Vec<Inst> = hb.insts.iter().map(|i| map_inst(i, &look, 0)).collect();
        let term = match hb.term {
            Term::Branch(c, t, e) => Term::Branch(look(c), t, e),
            t => t,
        };
        if branch_latch {
            let nb = f.blocks.len() as u32;
            f.blocks.push(Block { insts, pos: hb.pos.clone(), term, term_pos: hb.term_pos });
            if let Term::Branch(_, _, ref mut e) = f.blocks[l].term {
                *e = nb;
            }
            added = true;
        } else {
            f.blocks[l].insts.extend(insts);
            f.blocks[l].pos.extend(hb.pos.iter().copied());
            f.blocks[l].term = term;
            f.blocks[l].term_pos = hb.term_pos;
        }
    }
    // the header copies follow their latches (a branch's false target is laid out next)
    if added {
        relayout(f);
    }
}

/// Post-increment pointers share one register: an iterator step `p = x; t = p + K; x = t`
/// (or `t = x + K`, the same value there)
/// (slice::Iter::next inlined into a loop: the element pointer p and the advanced cursor x) keeps
/// p alive next to x for the element's field accesses, so every step copied the cursor (`mov p, x;
/// add x, K`). Those accesses read x instead -- at offset `off - K` after the step (x = p + K
/// there), unchanged before it -- when x has no other definition on any path from the step to
/// them; p then dies and the step becomes `x = x + K` (`add x, K`, no copy). Only when every use
/// of p is a load / store base with a constant offset (or the step's add itself).
pub fn fold_post_increments(f: &mut Func) -> bool {
    let nv = f.vregs.len();
    let nb = f.blocks.len();
    let mut ndef = vec![0u32; nv];
    let mut nuse = vec![0u32; nv];
    for p in &f.params {
        ndef[p.0 as usize] += 2;
    }
    let mut tu = Vec::new();
    for b in &f.blocks {
        for i in &b.insts {
            for_defs(i, |d| ndef[d.0 as usize] += 1);
            for_uses(i, |u| nuse[u.0 as usize] += 1);
        }
        term_uses(&b.term, &mut tu);
        for u in &tu {
            nuse[u.0 as usize] += 1;
        }
    }
    // candidates: (block, i0 = Mov(p, x), i1 = Add(t, p, K), i2 = Mov(x, t))
    let mut cands: Vec<(usize, usize, usize, usize, VReg, VReg, VReg, i64)> = Vec::new();
    for (bi, b) in f.blocks.iter().enumerate() {
        for (i0, inst) in b.insts.iter().enumerate() {
            let Inst::Mov(p, x) = *inst else { continue };
            if p == x || ndef[p.0 as usize] != 1 || ndef[x.0 as usize] < 2 || f.vregs[p.0 as usize] != Cls::I {
                continue;
            }
            // the add of p and the move back into x, with no other write of p or x before them
            let mut add: Option<(usize, VReg, i64)> = None;
            let mut back: Option<usize> = None;
            for (k, j) in b.insts.iter().enumerate().skip(i0 + 1) {
                let mut hit = false;
                for_defs(j, |d| hit |= d == x || d == p);
                match *j {
                    Inst::IBinI(IOp::Add, it, t, a, kk) if (a == p || a == x) && add.is_none() && it.bits == 64 && ndef[t.0 as usize] == 1 && nuse[t.0 as usize] == 1 && t != x => {
                        add = Some((k, t, kk));
                        continue;
                    }
                    Inst::Mov(d, s2) if d == x && add.is_some_and(|a| a.1 == s2) => {
                        back = Some(k);
                        break;
                    }
                    _ => {}
                }
                if hit {
                    break;
                }
            }
            if let (Some((i1, t, k)), Some(i2)) = (add, back) {
                cands.push((bi, i0, i1, i2, p, x, t, k));
            }
        }
    }
    if cands.is_empty() {
        return false;
    }
    let idom = idoms(f);
    let dominates = |a: usize, b: usize| -> bool {
        let mut y = b as u32;
        for _ in 0..100000 {
            if y as usize == a {
                return true;
            }
            let q = idom[y as usize];
            if q == u32::MAX || q == y {
                return false;
            }
            y = q;
        }
        false
    };
    let mut dead: Vec<(usize, usize)> = Vec::new();
    for (m, i0, i1, i2, p, x, _t, k) in cands {
        // p's uses: (block, inst, case-2 = after the step)
        let mut uses: Vec<(usize, usize, bool)> = Vec::new();
        let mut ok = true;
        'scan: for (bi, b) in f.blocks.iter().enumerate() {
            term_uses(&b.term, &mut tu);
            if tu.contains(&p) {
                ok = false;
                break;
            }
            for (ii, i) in b.insts.iter().enumerate() {
                let mut reads = false;
                for_uses(i, |u| reads |= u == p);
                if !reads {
                    continue;
                }
                if bi == m && ii == i1 {
                    continue;
                }
                let base_only = match *i {
                    Inst::Load(_, _, base, _) | Inst::VLoad(_, base, _) => base == p,
                    Inst::Store(_, base, _, v) | Inst::VStore(base, _, v) => base == p && v != p,
                    _ => false,
                };
                if !base_only {
                    ok = false;
                    break 'scan;
                }
                let after = if bi == m {
                    if ii <= i0 {
                        ok = false;
                        break 'scan;
                    }
                    ii > i2
                } else {
                    if !dominates(m, bi) {
                        ok = false;
                        break 'scan;
                    }
                    true
                };
                if after {
                    let off = match *i {
                        Inst::Load(_, _, _, o) | Inst::VLoad(_, _, o) | Inst::Store(_, _, o, _) | Inst::VStore(_, o, _) => o as i64,
                        _ => unreachable!(),
                    };
                    if i32::try_from(off - k).is_err() {
                        ok = false;
                        break 'scan;
                    }
                }
                uses.push((bi, ii, after));
            }
        }
        if !ok || uses.is_empty() {
            continue;
        }
        // x written elsewhere: that value must not reach a use after the step
        let mut ev: Vec<(u32, u32, u8)> = Vec::new();
        for (bi, b) in f.blocks.iter().enumerate() {
            for (ii, i) in b.insts.iter().enumerate() {
                let mut w = false;
                for_defs(i, |d| w |= d == x);
                if w && !(bi == m && ii == i2) {
                    ev.push((bi as u32, ii as u32, 1));
                }
            }
        }
        ev.push((m as u32, i2 as u32, 3));
        for &(bi, ii, after) in &uses {
            if after {
                ev.push((bi as u32, ii as u32, 2));
            }
        }
        ev.sort_unstable();
        let scan = |bk: u32, from: u32, mut dirty: bool| -> Option<bool> {
            let lo = ev.partition_point(|e| (e.0, e.1) < (bk, from));
            for e in &ev[lo..] {
                if e.0 != bk {
                    break;
                }
                match e.2 {
                    1 => dirty = true,
                    2 if dirty => return None,
                    3 => dirty = false,
                    _ => {}
                }
            }
            Some(dirty)
        };
        let mut seen = vec![false; nb];
        let mut work: Vec<u32> = Vec::new();
        for e in ev.iter().filter(|e| e.2 == 1) {
            match scan(e.0, e.1 + 1, true) {
                None => ok = false,
                Some(true) => work.push(e.0),
                Some(false) => {}
            }
        }
        while ok {
            let Some(bx) = work.pop() else { break };
            for s in f.blocks[bx as usize].term.succs() {
                if seen[s as usize] {
                    continue;
                }
                seen[s as usize] = true;
                match scan(s, 0, true) {
                    None => ok = false,
                    Some(true) => work.push(s),
                    Some(false) => {}
                }
            }
        }
        if !ok {
            continue;
        }
        for (bi, ii, after) in uses {
            let d = if after { -k } else { 0 };
            match &mut f.blocks[bi].insts[ii] {
                Inst::Load(_, _, base, o) | Inst::VLoad(_, base, o) | Inst::Store(_, base, o, _) | Inst::VStore(base, o, _) => {
                    *base = x;
                    *o = (*o as i64 + d) as i32;
                }
                _ => unreachable!(),
            }
        }
        // the step in place where x took t: x = x + K (t had no other reader); p = x and the
        // add into t have no reader left
        let Inst::IBinI(_, it, ..) = f.blocks[m].insts[i1] else { unreachable!() };
        f.blocks[m].insts[i2] = Inst::IBinI(IOp::Add, it, x, x, k);
        dead.push((m, i0));
        dead.push((m, i1));
    }
    dead.sort_unstable_by(|a, b| b.cmp(a));
    for &(m, i0) in &dead {
        f.blocks[m].insts.remove(i0);
        if i0 < f.blocks[m].pos.len() {
            f.blocks[m].pos.remove(i0);
        }
    }
    !dead.is_empty()
}

/// Copy forwarding between stack slots: a slot whose one write is a whole-slot `Copy` from
/// another slot, both never escaping, reads the source slot instead when no write to the
/// source can come between the copy and a read (moves of a big value through inlined
/// calls: `Option<T>` unwrapped, passed and returned as copy chains of one 272-byte value).
/// A source may also be a single call's result buffer. Disjoint destination writes keep
/// their slot: only reads wholly within the copied prefix are redirected in that case.
/// Returns whether any copy went.
pub fn forward_slot_copies(f: &mut Func) -> bool {
    let ns = f.slots.len();
    if ns < 2 {
        return false;
    }
    let mut any = false;
    // a chain resolves one link per round
    for _round in 0..8 {
        let SlotScan { vs, esc, writes, rd_end, whole, ret_call, .. } = slot_scan(f);
        let sl = |v: VReg| vs[v.0 as usize].0;
        const NONE: u32 = u32::MAX;
        // slots read at runtime offsets too (an array iterator's `data[start]`): only loads /
        // copies out through `slot + i`; their reads are events like the others
        let dynr = dyn_read_slots(f, &vs);
        // Constant byte extent of an access; indexed accesses cannot be split safely.
        let extent = |i: &Inst, write: bool| -> Option<(i64, i64)> {
            let (base, off, n) = match (i, write) {
                (Inst::Load(m, _, base, off), false) | (Inst::Store(m, base, off, _), true) => (*base, *off as i64, mem_size(*m) as i64),
                (Inst::VLoad(_, base, off), false) | (Inst::VStore(base, off, _), true) => (*base, *off as i64, 16),
                (Inst::Copy(d, s, n), _) => (if write { *d } else { *s }, 0, *n as i64),
                _ => return None,
            };
            let start = vs[base.0 as usize].1.checked_add(off)?;
            Some((start, start.checked_add(n)?))
        };
        // reads / writes of each slot in program order: (block, inst, write)
        let mut ev: Vec<Vec<(u32, u32, bool)>> = vec![Vec::new(); ns];
        for (s2, reads) in dynr.iter().enumerate() {
            if let Some(reads) = reads {
                for &(bi, ii) in reads {
                    ev[s2].push((bi, ii, false));
                }
            }
        }
        for (bi, blk) in f.blocks.iter().enumerate() {
            for (ii, i) in blk.insts.iter().enumerate() {
                let (r, w) = match i {
                    Inst::Store(_, base, ..) | Inst::VStore(base, ..) | Inst::StoreX(_, base, ..) => (NONE, sl(*base)),
                    Inst::Copy(d, s2, _) => (sl(*s2), sl(*d)),
                    Inst::Load(_, _, base, _) | Inst::VLoad(_, base, _) | Inst::LoadX(_, _, base, ..) => (sl(*base), NONE),
                    _ => continue,
                };
                if r != NONE {
                    ev[r as usize].push((bi as u32, ii as u32, false));
                }
                if w != NONE {
                    ev[w as usize].push((bi as u32, ii as u32, true));
                }
            }
        }
        for (s, call) in ret_call.iter().enumerate() {
            if let Some((bi, ii)) = *call {
                ev[s].push((bi as u32, ii as u32, true));
            }
        }
        // one candidate per source slot per round (a copy of A into B and C both forwarded
        // keeps the analysis of A's writes simple)
        let mut used = vec![false; ns];
        let mut repl: Vec<(u32, u32, usize, usize, Option<u32>)> = Vec::new();
        let nb = f.blocks.len();
        let mut seen = vec![0u32; nb];
        let mut stamp = 0u32;
        for b in 0..ns {
            // a destination read at runtime offsets qualifies when the copy fills all of it (a
            // read outside the slot is undefined behaviour, so every read sees copied bytes)
            let dyn_ok = |n: u32| dynr[b].is_some() && n >= f.slots[b].size;
            let (cb, ci, a) = match whole[b] {
                Some(w) if !esc[b] || matches!(f.blocks[w.0].insts[w.1], Inst::Copy(_, _, n) if dyn_ok(n)) => w,
                _ => continue,
            };
            if (esc[a as usize] && ret_call[a as usize].is_none()) || used[a as usize] || used[b] {
                continue;
            }
            let Inst::Copy(_, _, n) = f.blocks[cb].insts[ci] else { unreachable!() };
            let partial = writes[b] != 1;
            // runtime-offset reads have no constant extent: no partial forwarding with them
            if partial && dynr[b].is_some() {
                continue;
            }
            if !partial && rd_end[b] > n as i64 && !dyn_ok(n) {
                continue;
            }
            // the events that matter, per block in order: A written (1), B read (2), the copy (3)
            let mut evs: Vec<(u32, u32, u8)> = Vec::new();
            for &(eb, ei, w) in &ev[a as usize] {
                if w && !(eb as usize == cb && ei as usize == ci) {
                    evs.push((eb, ei, 1));
                }
            }
            let mut ok = true;
            for &(eb, ei, w) in &ev[b] {
                if partial {
                    if w && eb as usize == cb && ei as usize == ci {
                        continue;
                    }
                    let Some((start, end)) = extent(&f.blocks[eb as usize].insts[ei as usize], w) else {
                        ok = false;
                        break;
                    };
                    if start >= n as i64 || end <= 0 {
                        continue;
                    }
                    if w || start < 0 || end > n as i64 {
                        ok = false;
                        break;
                    }
                }
                if !w {
                    evs.push((eb, ei, 2));
                }
            }
            if !ok {
                continue;
            }
            evs.push((cb as u32, ci as u32, 3));
            evs.sort_unstable();
            // scan block `bk` from `from` (entered dirty or not): Some(exits dirty) or None (a
            // read of B while A changed since the copy)
            let scan = |bk: u32, from: u32, mut dirty: bool| -> Option<bool> {
                let lo = evs.partition_point(|e| (e.0, e.1) < (bk, from));
                for e in &evs[lo..] {
                    if e.0 != bk {
                        break;
                    }
                    match e.2 {
                        1 => dirty = true,
                        2 if dirty => return None,
                        3 => dirty = false,
                        _ => {}
                    }
                }
                Some(dirty)
            };
            // dirtiness starts at A's writes and flows forward until the copy runs again
            stamp += 1;
            let mut work: Vec<u32> = Vec::new();
            for e in &evs {
                if e.2 == 1 {
                    match scan(e.0, e.1, true) {
                        None => ok = false,
                        Some(true) => work.push(e.0),
                        Some(false) => {}
                    }
                }
            }
            while ok {
                let Some(x) = work.pop() else { break };
                for sx in f.blocks[x as usize].term.succs() {
                    if seen[sx as usize] == stamp {
                        continue;
                    }
                    seen[sx as usize] = stamp;
                    match scan(sx, 0, true) {
                        None => ok = false,
                        Some(true) => work.push(sx),
                        Some(false) => {}
                    }
                }
            }
            if ok {
                used[a as usize] = true;
                used[b] = true;
                repl.push((b as u32, a, cb, ci, partial.then_some(n)));
            }
        }
        if repl.is_empty() {
            break;
        }
        let mut to = vec![(NONE, None); ns];
        for &(b, a, _, _, prefix) in &repl {
            to[b as usize] = (a, prefix);
        }
        let mut dels: Vec<(usize, usize)> = repl.iter().map(|r| (r.2, r.3)).collect();
        dels.sort_unstable_by(|x, y| y.cmp(x));
        for (cb, ci) in dels {
            f.blocks[cb].insts.remove(ci);
            if ci < f.blocks[cb].pos.len() {
                f.blocks[cb].pos.remove(ci);
            }
        }
        for blk in f.blocks.iter_mut() {
            for i in blk.insts.iter_mut() {
                if let Inst::SlotAddr(_, s) = i {
                    if let (a, None) = to[*s as usize] {
                        if a != NONE {
                            *s = a;
                        }
                    }
                }
            }
        }
        if repl.iter().any(|r| r.4.is_some()) {
            for bi in 0..nb {
                let insts = std::mem::take(&mut f.blocks[bi].insts);
                let pos = std::mem::take(&mut f.blocks[bi].pos);
                for (ii, mut i) in insts.into_iter().enumerate() {
                    let range = extent(&i, false);
                    let base = match &mut i {
                        Inst::Load(_, _, base, _) | Inst::VLoad(_, base, _) | Inst::Copy(_, base, _) => Some(base),
                        _ => None,
                    };
                    if let Some(base) = base {
                        let (s, off) = vs[base.0 as usize];
                        if s != NONE {
                            if let (a, Some(n)) = to[s as usize] {
                                if range.is_some_and(|(start, end)| start >= 0 && end <= n as i64) {
                                    let t = f.vreg(Cls::I);
                                    f.blocks[bi].insts.push(Inst::SlotAddr(t, a));
                                    if let Some(&p) = pos.get(ii) { f.blocks[bi].pos.push(p); }
                                    *base = t;
                                    if off != 0 {
                                        let t2 = f.vreg(Cls::I);
                                        f.blocks[bi].insts.push(Inst::IBinI(IOp::Add, U64, t2, t, off));
                                        if let Some(&p) = pos.get(ii) { f.blocks[bi].pos.push(p); }
                                        *base = t2;
                                    }
                                }
                            }
                        }
                    }
                    f.blocks[bi].insts.push(i);
                    if let Some(&p) = pos.get(ii) { f.blocks[bi].pos.push(p); }
                }
            }
        }
        any = true;
    }
    any
}

/// The stack slots' addresses and uses: `vs[v]` = (slot, offset) of a vreg holding a slot
/// address (single-definition SlotAddr, adds and moves of it; slot u32::MAX = none); a slot
/// escapes when its address goes anywhere but a load / store / copy base or a copy source.
struct SlotScan {
    vs: Vec<(u32, i64)>,
    esc: Vec<bool>,
    writes: Vec<u32>,
    reads: Vec<u32>,
    /// end of the furthest byte read
    rd_end: Vec<i64>,
    /// a copy into the slot's start from another slot's start: (block, inst, source slot)
    whole: Vec<Option<(usize, usize, u32)>>,
    /// A ret slot escaping only into one call's arguments: (block, inst). `esc` stays
    /// conservative for the other passes; forwarding counts this call as a write.
    ret_call: Vec<Option<(usize, usize)>>,
}

fn slot_scan(f: &Func) -> SlotScan {
    slot_scan_inner(f, false)
}

fn slot_scan_inner(f: &Func, terminal_calls: bool) -> SlotScan {
    let ns = f.slots.len();
    let nv = f.vregs.len();
    let mut ndef = vec![0u32; nv];
    for p in &f.params {
        ndef[p.0 as usize] += 1;
    }
    for b in &f.blocks {
        for i in &b.insts {
            for_defs(i, |d| ndef[d.0 as usize] += 1);
        }
    }
    // vreg -> (slot, Some(offset) when known); slot addresses and adds/moves of them
    const NONE: u32 = u32::MAX;
    let mut vs: Vec<(u32, i64)> = vec![(NONE, 0); nv];
    // a slot address in a vreg defined more than once is not followed: the slot escapes
    let mut multi = vec![false; ns];
    for b in &f.blocks {
        for i in &b.insts {
            if let Inst::SlotAddr(d, s) = i {
                if ndef[d.0 as usize] == 1 {
                    vs[d.0 as usize] = (*s, 0);
                } else {
                    multi[*s as usize] = true;
                }
            }
        }
    }
    let mut grew = true;
    while grew {
        grew = false;
        for b in &f.blocks {
            for i in &b.insts {
                let (d, s, k) = match i {
                    Inst::Mov(d, s) => (*d, *s, 0),
                    Inst::IBinI(IOp::Add, _, d, s, k) => (*d, *s, *k),
                    _ => continue,
                };
                let (sl, o) = vs[s.0 as usize];
                if sl != NONE && ndef[d.0 as usize] == 1 && vs[d.0 as usize].0 == NONE {
                    vs[d.0 as usize] = (sl, o + k);
                    grew = true;
                }
            }
        }
    }
    // per slot: escaped, write count, the one whole copy (block, inst, source slot)
    // Poison a potential result-buffer call on any other escape or a second call.
    const BAD_CALL: (usize, usize) = (usize::MAX, usize::MAX);
    let mut ret_call = multi.iter().map(|&m| m.then_some(BAD_CALL)).collect::<Vec<_>>();
    let mut esc = multi;
    let mut writes = vec![0u32; ns];
    // end of the furthest byte read (a copy of fewer bytes than the slot holds -- its tail
    // padding -- still covers every read)
    let mut rd_end = vec![0i64; ns];
    let mut whole: Vec<Option<(usize, usize, u32)>> = vec![None; ns];
    let mut reads = vec![0u32; ns];
    let sl = |v: VReg| vs[v.0 as usize].0;
    let mut tu = Vec::new();
    for (bi, b) in f.blocks.iter().enumerate() {
        for (ii, i) in b.insts.iter().enumerate() {
            match i {
                Inst::Load(m, _, base, off) => {
                    if sl(*base) != NONE {
                        let e = &mut rd_end[sl(*base) as usize];
                        *e = (*e).max(vs[base.0 as usize].1 + *off as i64 + mem_size(*m) as i64);
                    }
                }
                Inst::VLoad(_, base, off) => {
                    if sl(*base) != NONE {
                        let e = &mut rd_end[sl(*base) as usize];
                        *e = (*e).max(vs[base.0 as usize].1 + *off as i64 + 16);
                    }
                }
                Inst::LoadX(_, _, base, x, _, _) => {
                    if sl(*base) != NONE {
                        rd_end[sl(*base) as usize] = i64::MAX;
                    }
                    if sl(*x) != NONE {
                        esc[sl(*x) as usize] = true;
                        ret_call[sl(*x) as usize] = Some(BAD_CALL);
                    }
                }
                Inst::Store(_, base, _, src) | Inst::VStore(base, _, src) => {
                    if sl(*base) != NONE {
                        writes[sl(*base) as usize] += 1;
                    }
                    if sl(*src) != NONE {
                        esc[sl(*src) as usize] = true;
                        ret_call[sl(*src) as usize] = Some(BAD_CALL);
                    }
                }
                Inst::StoreX(_, base, x, _, _, src) => {
                    if sl(*base) != NONE {
                        writes[sl(*base) as usize] += 1;
                    }
                    for v in [x, src] {
                        if sl(*v) != NONE {
                            esc[sl(*v) as usize] = true;
                            ret_call[sl(*v) as usize] = Some(BAD_CALL);
                        }
                    }
                }
                Inst::Copy(d, s, n) => {
                    if sl(*s) != NONE {
                        let e = &mut rd_end[sl(*s) as usize];
                        *e = (*e).max(vs[s.0 as usize].1 + *n as i64);
                    }
                    let ds = sl(*d);
                    if ds != NONE {
                        writes[ds as usize] += 1;
                        let ss = sl(*s);
                        if vs[d.0 as usize].1 == 0 && *n <= f.slots[ds as usize].size && ss != NONE && ss != ds && vs[s.0 as usize].1 == 0 && f.slots[ss as usize].size >= *n {
                            whole[ds as usize] = Some((bi, ii, ss));
                        }
                    }
                }
                // Vector promotion materializes the cells before entering a
                // terminal block; calls there may observe or retain the address.
                Inst::Call(..) if terminal_calls && matches!(b.term, Term::Unreachable) => {}
                Inst::Call(c, args, _) => {
                    for arg in args {
                        let s = sl(*arg);
                        if s != NONE {
                            esc[s as usize] = true;
                            let call = &mut ret_call[s as usize];
                            *call = Some(match *call {
                                None => (bi, ii),
                                Some(at) if at == (bi, ii) => at,
                                _ => BAD_CALL,
                            });
                        }
                    }
                    if let Callee::Indirect(v) | Callee::CIndirect(v, _) = c {
                        if sl(*v) != NONE {
                            esc[sl(*v) as usize] = true;
                            ret_call[sl(*v) as usize] = Some(BAD_CALL);
                        }
                    }
                }
                Inst::SlotAddr(..) => {}
                Inst::Mov(d, s) | Inst::IBinI(IOp::Add, _, d, s, _) if sl(*s) != NONE && sl(*d) == sl(*s) => {}
                _ => {
                    for_uses(i, |v| {
                        if sl(v) != NONE {
                            esc[sl(v) as usize] = true;
                            ret_call[sl(v) as usize] = Some(BAD_CALL);
                        }
                    });
                }
            }
        }
        term_uses(&b.term, &mut tu);
        for v in &tu {
            if sl(*v) != NONE {
                esc[sl(*v) as usize] = true;
                ret_call[sl(*v) as usize] = Some(BAD_CALL);
            }
        }
    }
    for b in &f.blocks {
        for i in &b.insts {
            match i {
                Inst::Load(_, _, base, _) | Inst::VLoad(_, base, _) | Inst::LoadX(_, _, base, ..) | Inst::Copy(_, base, _) => {
                    if sl(*base) != NONE {
                        reads[sl(*base) as usize] += 1;
                    }
                }
                _ => {}
            }
        }
    }
    for (s, call) in ret_call.iter_mut().enumerate() {
        if !f.slots[s].ret || *call == Some(BAD_CALL) {
            *call = None;
        }
    }
    SlotScan { vs, esc, writes, reads, rd_end, whole, ret_call }
}

/// Replace a private, read-only stack copy of immutable data with the data's address.
/// Follow arbitrary indexed address arithmetic, not just constant field offsets. Only a
/// single whole-slot initializer is allowed; it must dominate every memory read. All other
/// uses (including calls, pointer comparisons, volatile/atomic accesses and stores through
/// derived pointers) reject the slot. Thus neither its identity nor its contents can escape.
/// The source extent is vouched for by the embedder, so aliasing calls/writes cannot change
/// it, even when the object's bytes cannot be scalar-folded (pointers, unions or padding).
pub fn forward_const_slots(f: &mut Func, ro: Option<RoReader>) -> bool {
    let Some(ro) = ro else { return false };
    if f.slots.is_empty() || !f.blocks.iter().any(|b| b.insts.iter().any(|i| matches!(i, Inst::Copy(..)))) {
        return false;
    }
    const NONE: u32 = u32::MAX;
    let nv = f.vregs.len();
    let ns = f.slots.len();
    let mut ndef = vec![0u32; nv];
    for p in &f.params {
        ndef[p.0 as usize] += 1;
    }
    for b in &f.blocks {
        for i in &b.insts {
            for_defs(i, |v| ndef[v.0 as usize] += 1);
        }
    }
    // Slot roots include pointers with runtime offsets; exact offsets are needed only for
    // the initializer. Single definitions prevent pointer reassignments hiding an escape.
    let mut root = vec![NONE; nv];
    let mut offset = vec![None; nv];
    let mut sym = vec![None; nv];
    let mut bad = vec![false; ns];
    for b in &f.blocks {
        for i in &b.insts {
            match *i {
                Inst::SlotAddr(d, s) if ndef[d.0 as usize] == 1 => {
                    root[d.0 as usize] = s;
                    offset[d.0 as usize] = Some(0i64);
                }
                Inst::SlotAddr(_, s) => bad[s as usize] = true,
                Inst::Addr(d, s) if ndef[d.0 as usize] == 1 => sym[d.0 as usize] = Some(s),
                _ => {}
            }
        }
    }
    loop {
        let mut grew = false;
        for b in &f.blocks {
            for i in &b.insts {
                let (d, a, off) = match *i {
                    Inst::Mov(d, a) => (d, a, Some(0)),
                    Inst::IBinI(IOp::Add, IntTy { bits: 64, .. }, d, a, k) => (d, a, Some(k)),
                    Inst::IBin(IOp::Add, IntTy { bits: 64, .. }, d, a, b) if root[b.0 as usize] == NONE => (d, a, None),
                    Inst::IBin(IOp::Add, IntTy { bits: 64, .. }, d, a, b) if root[a.0 as usize] == NONE => (d, b, None),
                    Inst::IBin(IOp::Sub, IntTy { bits: 64, .. }, d, a, b) if root[b.0 as usize] == NONE => (d, a, None),
                    _ => continue,
                };
                let (di, ai) = (d.0 as usize, a.0 as usize);
                if ndef[di] != 1 {
                    continue;
                }
                if root[di] == NONE && root[ai] != NONE {
                    root[di] = root[ai];
                    offset[di] = offset[ai].zip(off).and_then(|(a, b)| a.checked_add(b));
                    grew = true;
                }
                if sym[di].is_none() {
                    if let (Some(s), Some(k)) = (sym[ai], off) {
                        sym[di] = Some(s.plus(k));
                        grew = true;
                    }
                }
            }
        }
        if !grew {
            break;
        }
    }
    let mut init = vec![None; ns];
    for (bi, b) in f.blocks.iter().enumerate() {
        for (ii, i) in b.insts.iter().enumerate() {
            if let Inst::Copy(d, src, n) = *i {
                let s = root[d.0 as usize];
                if s == NONE {
                    continue;
                }
                let si = s as usize;
                if init[si].is_some() {
                    bad[si] = true;
                }
                if offset[d.0 as usize] == Some(0) && n == f.slots[si].size {
                    if let Some(src) = sym[src.0 as usize] {
                        if matches!(ro.read(src, 0), RoRead::Range(len) if len >= n as u64) {
                            init[si] = Some((bi, ii, src));
                        }
                    }
                }
            }
        }
    }
    if init.iter().enumerate().all(|(s, x)| x.is_none() || bad[s]) {
        return false;
    }
    let (pre, post, _) = dom_numbers(&idoms(f));
    let mut tu = Vec::new();
    for (bi, b) in f.blocks.iter().enumerate() {
        for (ii, i) in b.insts.iter().enumerate() {
            for_uses(i, |v| {
                let s = root[v.0 as usize];
                if s == NONE {
                    return;
                }
                let si = s as usize;
                let Some((cb, ci, _)) = init[si] else {
                    return;
                };
                let read = match *i {
                    Inst::Load(_, _, base, _) | Inst::VLoad(_, base, _) | Inst::VGather(_, base, _) => v == base,
                    Inst::LoadX(_, _, base, idx, ..) => v == base && v != idx,
                    Inst::Copy(d, src, _) => v == src && v != d,
                    _ => false,
                };
                if read {
                    if pre[bi] == u32::MAX || pre[cb] == u32::MAX || pre[cb] > pre[bi] || post[bi] > post[cb] || (bi == cb && ii <= ci) {
                        bad[si] = true;
                    }
                    return;
                }
                let derived = match *i {
                    Inst::Mov(d, a) | Inst::IBinI(IOp::Add, IntTy { bits: 64, .. }, d, a, _) => v == a && root[d.0 as usize] == s && ndef[d.0 as usize] == 1,
                    Inst::IBin(op @ (IOp::Add | IOp::Sub), IntTy { bits: 64, .. }, d, a, b) => {
                        root[d.0 as usize] == s && ndef[d.0 as usize] == 1 && ((v == a && root[b.0 as usize] == NONE) || (op == IOp::Add && v == b && root[a.0 as usize] == NONE))
                    }
                    _ => false,
                };
                if !derived && !(bi == cb && ii == ci && matches!(i, Inst::Copy(d, _, _) if *d == v)) {
                    bad[si] = true;
                }
            });
        }
        term_uses(&b.term, &mut tu);
        for v in &tu {
            if root[v.0 as usize] != NONE {
                bad[root[v.0 as usize] as usize] = true;
            }
        }
    }
    let mut changed = false;
    for (bi, b) in f.blocks.iter_mut().enumerate() {
        let has_pos = b.pos.len() == b.insts.len();
        let mut removed = Vec::new();
        for (ii, i) in b.insts.iter_mut().enumerate() {
            if let Inst::SlotAddr(d, s) = *i {
                if let Some((_, _, src)) = init[s as usize].filter(|_| !bad[s as usize]) {
                    *i = Inst::Addr(d, src);
                    changed = true;
                }
            } else if let Inst::Copy(d, _, _) = *i {
                let s = root[d.0 as usize];
                if s != NONE && !bad[s as usize] && init[s as usize].is_some_and(|(cb, ci, _)| cb == bi && ci == ii) {
                    removed.push(ii);
                }
            }
        }
        for ii in removed.into_iter().rev() {
            b.insts.remove(ii);
            if has_pos {
                b.pos.remove(ii);
            }
        }
    }
    changed
}

/// Store-to-load forwarding through stack slots, within a block: a load from a slot that a
/// store earlier in the block wrote (same bytes) takes the stored value; one that a copy
/// into the slot covers reads the copy's source instead, and the search goes on from there
/// (rustc / LLVM GVN forward through memcpy). An `Option<u16>` built in one slot, copied
/// into the match result's slot and read back field by field becomes the u16 load from the
/// font data and the tag constant (store-forwarding stalls of byte copies followed by a
/// halfword load were most of shape's GPOS lookup). Then slots nothing reads any more lose
/// their writes. Returns whether anything changed.
pub fn forward_slot_loads(f: &mut Func) -> bool {
    const NONE: u32 = u32::MAX;
    const WALK: usize = 96;
    let sc = slot_scan(f);
    if sc.vs.iter().all(|x| x.0 == NONE) {
        return false;
    }
    let vs = &sc.vs;
    let esc = &sc.esc;
    // where an access goes: a slot and its byte offset, or another pointer
    #[derive(Clone, Copy)]
    struct At {
        ptr: VReg,
        off: i64,
        slot: u32,
        soff: i64,
    }
    let at = |p: VReg, off: i64| -> At {
        let (s, o) = vs[p.0 as usize];
        At {
            ptr: p,
            off,
            slot: s,
            soff: if s != NONE { o + off } else { 0 },
        }
    };
    // may instruction `i` write bytes [a.soff, +k) of `a` (or alias it at all, off a slot)?
    let may_write = |i: &Inst, a: &At, k: i64| -> bool {
        let tgt = |b: VReg, off: i64, n: i64| -> bool {
            let (s, o) = vs[b.0 as usize];
            if a.slot != NONE && !esc[a.slot as usize] {
                // a non-escaping slot: only its own address writes it
                return s == a.slot && o + off < a.soff + k && a.soff < o + off + n;
            }
            if s != NONE && !esc[s as usize] {
                // a write to a non-escaping slot never touches other memory
                return a.slot == s && o + off < a.soff + k && a.soff < o + off + n;
            }
            true
        };
        match i {
            Inst::Store(m, b, off, _) => tgt(*b, *off as i64, mem_size(*m) as i64),
            Inst::VStore(b, off, _) => tgt(*b, *off as i64, 16),
            Inst::Copy(d, _, n) => tgt(*d, 0, *n as i64),
            Inst::StoreX(_, b, ..) => tgt(*b, 0, i64::MAX / 4) || vs[b.0 as usize].0 == NONE,
            _ if is_mem_effect(i) => !(a.slot != NONE && !esc[a.slot as usize]),
            _ => false,
        }
    };
    let mut changed = false;
    let mut ins: Vec<(usize, Inst, Inst)> = Vec::new();
    for bi in 0..f.blocks.len() {
        for li in 0..f.blocks[bi].insts.len() {
            let (m, d, base, off) = match f.blocks[bi].insts[li] {
                Inst::Load(m, d, base, off) => (m, d, base, off),
                Inst::VLoad(d, base, off) => (SROA_V128, d, base, off),
                _ => continue,
            };
            if vs[base.0 as usize].0 == NONE {
                continue;
            }
            let k = if m == SROA_V128 { 16 } else { mem_size(m) as i64 };
            let mut cur = at(base, off as i64);
            let mut moved = false;
            let mut value: Option<VReg> = None;
            let lo = li.saturating_sub(WALK);
            let mut j = li;
            while j > lo {
                j -= 1;
                let i = &f.blocks[bi].insts[j];
                // a slot is its own address: only a plain pointer must stay what it was
                if cur.slot == NONE {
                    let mut redef = false;
                    for_defs(i, |x| redef |= x == cur.ptr);
                    if redef {
                        break;
                    }
                }
                // the write that made these bytes
                let src = match i {
                    Inst::VStore(b2, off2, v) if m == SROA_V128 && cur.slot != NONE && vs[b2.0 as usize].0 == cur.slot && vs[b2.0 as usize].1 + *off2 as i64 == cur.soff => {
                        Some(Err(*v))
                    }
                    Inst::Store(m2, b2, off2, v)
                        if m != SROA_V128 && cur.slot != NONE && vs[b2.0 as usize].0 == cur.slot && vs[b2.0 as usize].1 + *off2 as i64 == cur.soff && mem_size(*m2) as i64 == k =>
                    {
                        let same = match (m, *m2) {
                            (Mem::Int(..), Mem::Int(..)) | (Mem::F32, Mem::F32) | (Mem::F64, Mem::F64) => true,
                            _ => false,
                        };
                        if same {
                            Some(Err(*v))
                        } else {
                            None
                        }
                    }
                    Inst::Copy(d2, s2, n2) if cur.slot != NONE && vs[d2.0 as usize].0 == cur.slot => {
                        let od = vs[d2.0 as usize].1;
                        if od <= cur.soff && cur.soff + k <= od + *n2 as i64 {
                            Some(Ok(at(*s2, cur.soff - od)))
                        } else {
                            None
                        }
                    }
                    _ => None,
                };
                match src {
                    Some(Err(v)) => {
                        // the stored value, unless redefined before the load
                        let mut later = false;
                        for x in &f.blocks[bi].insts[j + 1..li] {
                            for_defs(x, |y| later |= y == v);
                        }
                        if !later {
                            value = Some(v);
                        }
                        break;
                    }
                    Some(Ok(nat)) => {
                        // the copy's source, unless it changes (memory or pointer) before the load
                        let mut bad = false;
                        for x in &f.blocks[bi].insts[j + 1..li] {
                            if nat.slot == NONE {
                                for_defs(x, |y| bad |= y == nat.ptr);
                            }
                            bad |= may_write(x, &nat, k);
                        }
                        if bad {
                            break;
                        }
                        cur = nat;
                        moved = true;
                    }
                    None => {
                        if may_write(i, &cur, k) {
                            break;
                        }
                    }
                }
            }
            if let Some(v) = value {
                let ni = match m {
                    Mem::Int(n, signed) if m != SROA_V128 && n < 8 => Inst::Conv(Conv::IntToInt(IntTy { bits: n * 8, signed }), d, v),
                    _ => Inst::Mov(d, v),
                };
                f.blocks[bi].insts[li] = ni;
                changed = true;
            } else if moved {
                if cur.slot != NONE {
                    // the slot's address afresh, right before the load
                    let a = f.vreg(Cls::I);
                    ins.push((
                        li,
                        Inst::SlotAddr(a, cur.slot),
                        if m == SROA_V128 {
                            Inst::VLoad(d, a, cur.soff as i32)
                        } else {
                            Inst::Load(m, d, a, cur.soff as i32)
                        },
                    ));
                } else {
                    f.blocks[bi].insts[li] = if m == SROA_V128 {
                        Inst::VLoad(d, cur.ptr, cur.off as i32)
                    } else {
                        Inst::Load(m, d, cur.ptr, cur.off as i32)
                    };
                }
                changed = true;
            }
        }
        if !ins.is_empty() {
            let blk = &mut f.blocks[bi];
            let has_pos = blk.pos.len() == blk.insts.len();
            for (li, sa, ld) in ins.drain(..).rev() {
                blk.insts[li] = ld;
                blk.insts.insert(li, sa);
                if has_pos {
                    let p = blk.pos[li];
                    blk.pos.insert(li, p);
                }
            }
        }
    }
    if !changed {
        return false;
    }
    // writes to slots nothing reads
    let sc = slot_scan(f);
    let dead = |v: VReg| {
        let s = sc.vs[v.0 as usize].0;
        s != NONE && !sc.esc[s as usize] && sc.reads[s as usize] == 0
    };
    for b in f.blocks.iter_mut() {
        let mut k = 0;
        let mut keep = Vec::with_capacity(b.insts.len());
        for i in &b.insts {
            keep.push(!match i {
                Inst::Store(_, base, ..) | Inst::VStore(base, ..) | Inst::Copy(base, ..) => dead(*base),
                _ => false,
            });
        }
        if keep.iter().all(|x| *x) {
            continue;
        }
        let has_pos = b.pos.len() == b.insts.len();
        let mut ki = keep.iter();
        b.insts.retain(|_| *ki.next().unwrap());
        if has_pos {
            b.pos.retain(|_| {
                let r = keep[k];
                k += 1;
                r
            });
        }
    }
    true
}

/// Destination forwarding of slot copies: a slot B whose only read is one whole copy into
/// another slot C (at some offset), with every write of B in the copy's block before it and
/// nothing touching those bytes of C in between, is built in C directly: B's address becomes
/// C's (plus the offset) and the copy goes (LLVM memcpyopt's destination side: a value built
/// in a temporary then moved into the variable or the enclosing aggregate -- `Some(x)` /
/// `Ok((a, b))` assembled then copied into the match result). Returns whether any went.
pub fn forward_slot_dests(f: &mut Func) -> bool {
    const NONE: u32 = u32::MAX;
    let mut any = false;
    for _round in 0..8 {
        let sc = slot_scan(f);
        let vs = &sc.vs;
        let ns = f.slots.len();
        let sl = |v: VReg| vs[v.0 as usize].0;
        // per slot: the block and index range of its accesses (first, last), or many blocks
        let mut span: Vec<Option<(usize, usize, usize)>> = vec![None; ns];
        let mut multi_blk = vec![false; ns];
        let mut the_copy: Vec<Option<(usize, usize, u32, i64, u32)>> = vec![None; ns];
        for (bi, b) in f.blocks.iter().enumerate() {
            for (ii, i) in b.insts.iter().enumerate() {
                let mut touch = |v: VReg, ii: usize| {
                    let s = sl(v);
                    if s == NONE {
                        return;
                    }
                    match &mut span[s as usize] {
                        None => span[s as usize] = Some((bi, ii, ii)),
                        Some(x) => {
                            if x.0 != bi {
                                multi_blk[s as usize] = true;
                            } else {
                                x.2 = ii;
                            }
                        }
                    }
                };
                match i {
                    Inst::Load(_, _, base, _) | Inst::VLoad(_, base, _) | Inst::LoadX(_, _, base, ..) | Inst::Store(_, base, ..) | Inst::VStore(base, ..) | Inst::StoreX(_, base, ..) => touch(*base, ii),
                    Inst::Copy(d, s2, n) => {
                        touch(*d, ii);
                        touch(*s2, ii);
                        let (bs, cs) = (sl(*s2), sl(*d));
                        if bs != NONE && cs != NONE && bs != cs && vs[s2.0 as usize].1 == 0 && *n == f.slots[bs as usize].size {
                            the_copy[bs as usize] = Some((bi, ii, cs, vs[d.0 as usize].1, *n));
                        }
                    }
                    _ => {}
                }
            }
        }
        let mut used = vec![false; ns];
        // (B, C, offset, block, copy index)
        let mut repl: Vec<(u32, u32, i64, usize, usize)> = Vec::new();
        for b in 0..ns {
            let Some((cb, ci, c, o, n)) = the_copy[b] else { continue };
            if sc.esc[b] || sc.esc[c as usize] || multi_blk[b] || sc.reads[b] != 1 || used[b] || used[c as usize] {
                continue;
            }
            let Some((sb, first, last)) = span[b] else { continue };
            if sb != cb || last != ci || o < 0 || o + n as i64 > f.slots[c as usize].size as i64 {
                continue;
            }
            // nothing touches C's bytes [o, o+n) from B's first access up to the copy
            let mut clash = false;
            for i in &f.blocks[cb].insts[first..ci] {
                let hit = |v: VReg, off: i64, k: i64| -> bool {
                    let (s, so) = vs[v.0 as usize];
                    s == c && so + off < o + n as i64 && o < so + off + k
                };
                clash |= match i {
                    Inst::Load(m, _, base, off) | Inst::Store(m, base, off, _) => hit(*base, *off as i64, mem_size(*m) as i64),
                    Inst::VLoad(_, base, off) | Inst::VStore(base, off, _) => hit(*base, *off as i64, 16),
                    Inst::LoadX(_, _, base, ..) | Inst::StoreX(_, base, ..) => sl(*base) == c,
                    Inst::Copy(d, s2, k) => hit(*d, 0, *k as i64) || hit(*s2, 0, *k as i64),
                    _ => false,
                };
            }
            if clash {
                continue;
            }
            used[b] = true;
            used[c as usize] = true;
            repl.push((b as u32, c, o, cb, ci));
        }
        if repl.is_empty() {
            break;
        }
        let mut to: Vec<Option<(u32, i64)>> = vec![None; ns];
        for &(b, c, o, ..) in &repl {
            to[b as usize] = Some((c, o));
        }
        let mut dels: Vec<(usize, usize)> = repl.iter().map(|r| (r.3, r.4)).collect();
        dels.sort_unstable_by(|x, y| y.cmp(x));
        for (cb, ci) in dels {
            f.blocks[cb].insts.remove(ci);
            if ci < f.blocks[cb].pos.len() {
                f.blocks[cb].pos.remove(ci);
            }
        }
        for bi in 0..f.blocks.len() {
            let mut k = 0;
            while k < f.blocks[bi].insts.len() {
                if let Inst::SlotAddr(d, s) = f.blocks[bi].insts[k] {
                    if let Some((c, o)) = to[s as usize] {
                        if o == 0 {
                            f.blocks[bi].insts[k] = Inst::SlotAddr(d, c);
                        } else {
                            let t = f.vreg(Cls::I);
                            f.blocks[bi].insts[k] = Inst::SlotAddr(t, c);
                            f.blocks[bi].insts.insert(k + 1, Inst::IBinI(IOp::Add, IntTy { bits: 64, signed: false }, d, t, o));
                            if f.blocks[bi].pos.len() + 1 == f.blocks[bi].insts.len() {
                                let p = f.blocks[bi].pos[k];
                                f.blocks[bi].pos.insert(k + 1, p);
                            }
                            k += 1;
                        }
                    }
                }
                k += 1;
            }
        }
        any = true;
    }
    any
}

/// Stack slot sharing ("stack coloring"), as the backends lay out the frame (`slot_layout`):
/// slots whose lifetimes cannot overlap share storage. Lowering gives every temporary, call
/// result buffer and by-value argument copy a slot of its own, and inlining gives each inlined
/// body its own slots, so without sharing a frame is the sum of them all (Scope's recursive
/// type parser: 8 KB per parse_type frame, rustc 0.5 KB; Amp: 0.9 MB O3 frames on 2 MB kernel
/// worker stacks; rustc's stack colouring overlaps them). A slot's lifetime is approximated by
/// the blocks between its accesses: live where some access can reach and some access can be
/// reached (both along CFG edges, unwind edges included). An access is any instruction or
/// terminator using a value derived from the slot's address (address arithmetic, moves,
/// selects, and call results when a call takes it). A slot whose address is stored to memory,
/// returned, or handed to a call that may keep it (see `slot_sets`) keeps the whole function.
/// Slots below `min` bytes are left alone. Returns, per slot, the slot whose storage it uses
/// (itself when it keeps its own); None when nothing shares.
pub fn slot_colors(f: &Func, min: u32) -> Option<Vec<u32>> {
    let addressed = crate::rir::used_slots(f);
    slot_colors_used(f, min, &addressed)
}

fn slot_colors_used(f: &Func, min: u32, addressed: &[bool]) -> Option<Vec<u32>> {
    let ns = f.slots.len();
    let nb = f.blocks.len();
    if ns < 2 || nb == 0 {
        return None;
    }
    // a slot nothing addresses any more (forwarded into another: forward_slot_dests_x) has no
    // lifetime, so it "fits" next to everything -- and a colour takes its members' largest size:
    // Score's SamplerBackend::new, built in place into its result buffer, kept a 108 KB frame
    // for the 768-byte temporaries that shared a colour with the dead 108 KB slot
    // nothing much to share: not worth the analysis
    let (n, bytes) = (0..ns).filter(|&s| addressed[s] && f.slots[s].size >= min).fold((0, 0u64), |(n, b), s| (n + 1, b + f.slots[s].size as u64));
    if n < 2 || bytes < SLOT_SHARE_MIN_BYTES {
        return None;
    }
    let (der, esc, arg_ok) = slot_sets(f)?;
    // candidate slots: big, not escaping; the largest COLOR_MAX of them (colouring is
    // candidates x colours x block words: a big inlined function has thousands of slots)
    const COLOR_MAX: usize = 1024;
    let mut cand: Vec<usize> = (0..ns).filter(|&s| f.slots[s].size >= min && !esc[s] && addressed[s]).collect();
    if cand.len() > COLOR_MAX {
        cand.sort_by(|&x, &y| f.slots[y].size.cmp(&f.slots[x].size).then(x.cmp(&y)));
        cand.truncate(COLOR_MAX);
        cand.sort_unstable();
    }
    if cand.len() < 2 {
        return None;
    }
    let mut ci = vec![usize::MAX; ns];
    for (k, &s) in cand.iter().enumerate() {
        ci[s] = k;
    }
    let nc = cand.len();
    // Positions: instruction i reads at 2i and writes at 2i + 1 (the terminator is instruction
    // insts.len()). An access takes both, except a whole-slot copy between two slots' own
    // addresses: its source is read at 2i, its destination written at 2i + 1, so a slot copied
    // into another and dead after can share its storage (the copy then copies a slot onto
    // itself, which changes nothing) -- the chains of temporaries a value is moved through.
    let exact = slot_addr_vregs(f);
    // which slot an exact address is the address of
    let mut exact_slot = vec![u32::MAX; f.vregs.len()];
    for b in &f.blocks {
        for i in &b.insts {
            if let Inst::SlotAddr(d, sl) = i {
                if exact[d.0 as usize] {
                    exact_slot[d.0 as usize] = *sl;
                }
            }
        }
    }
    // A result buffer (`Slot::ret`) passed as one to one call only, by its own address: that
    // is the call lowering made it for, which writes the whole value (its type is the slot's),
    // so the slot's earlier contents are dead before it
    let mut sret_uses = vec![0u32; ns];
    for b in &f.blocks {
        for i in &b.insts {
            if let Inst::Call(Callee::Fn(id), args, _) = i {
                if f.sret_callees.contains(id) {
                    if let Some(a) = args.first() {
                        for &sl in &der[a.0 as usize] {
                            sret_uses[sl as usize] += 1;
                        }
                    }
                }
            }
        }
    }
    // accesses per candidate: (block, first, last position) per block with any, in block order,
    // and per such block the access kinds that decide liveness at its ends: the first read or
    // whole write (`ent`: Some(true) read, Some(false) whole write, None neither) and whether the
    // last access is the consuming call of an argument copy (`ext`)
    const READ: u8 = 0;
    const WRITE: u8 = 1;
    const KILL: u8 = 2;
    const CONSUME: u8 = 3;
    let mut acc: Vec<Vec<(u32, u32, u32)>> = vec![Vec::new(); nc];
    let mut ent: Vec<Vec<Option<bool>>> = vec![Vec::new(); nc];
    let mut ext: Vec<Vec<bool>> = vec![Vec::new(); nc];
    let mut tu = Vec::new();
    for (bi, b) in f.blocks.iter().enumerate() {
        let note = |k: usize, lo: u32, hi: u32, kind: u8, acc: &mut Vec<Vec<(u32, u32, u32)>>, ent: &mut Vec<Vec<Option<bool>>>, ext: &mut Vec<Vec<bool>>| {
            let a = &mut acc[k];
            match a.last_mut() {
                Some(x) if x.0 == bi as u32 => x.2 = x.2.max(hi),
                _ => {
                    a.push((bi as u32, lo, hi));
                    ent[k].push(None);
                    ext[k].push(false);
                }
            }
            let e = ent[k].last_mut().unwrap();
            if e.is_none() {
                *e = match kind {
                    READ | CONSUME => Some(true),
                    KILL => Some(false),
                    _ => None,
                };
            }
            *ext[k].last_mut().unwrap() = kind == CONSUME;
        };
        let each = |v: VReg, g: &mut dyn FnMut(usize)| {
            for &d in &der[v.0 as usize] {
                let k = ci[d as usize];
                if k != usize::MAX {
                    g(k);
                }
            }
        };
        for (ii, i) in b.insts.iter().enumerate() {
            let p = 2 * ii as u32;
            // address arithmetic is no access to the slot's memory (slot addresses may sit at the
            // function entry, computed once): only what reads or writes through the address, or
            // hands it on (calls, compares, terminators), keeps the slot live
            match i {
                Inst::SlotAddr(..) | Inst::Mov(..) | Inst::IBin(IOp::Add, ..) | Inst::IBinI(IOp::Add, ..) | Inst::IBinI(IOp::Sub, ..) => {}
                Inst::Copy(d, sv, n) => {
                    let split = exact[d.0 as usize] && exact[sv.0 as usize];
                    each(*sv, &mut |k| note(k, p, if split { p } else { p + 1 }, READ, &mut acc, &mut ent, &mut ext));
                    let whole = |k: usize| exact[d.0 as usize] && exact_slot[d.0 as usize] as usize == cand[k] && *n >= f.slots[cand[k]].size;
                    each(*d, &mut |k| {
                        let kind = if whole(k) { KILL } else { WRITE };
                        note(k, if split { p + 1 } else { p }, p + 1, kind, &mut acc, &mut ent, &mut ext)
                    });
                }
                Inst::Store(_, a, _, v) | Inst::StoreX(_, a, _, _, _, v) | Inst::VStore(a, _, v) | Inst::AtomicStore(_, _, a, v) => {
                    each(*a, &mut |k| note(k, p, p + 1, WRITE, &mut acc, &mut ent, &mut ext));
                    each(*v, &mut |k| note(k, p, p + 1, READ, &mut acc, &mut ent, &mut ext));
                    if let Inst::StoreX(_, _, x, ..) = i {
                        each(*x, &mut |k| note(k, p, p + 1, READ, &mut acc, &mut ent, &mut ext));
                    }
                }
                Inst::Call(c, args, _) => {
                    let sret = matches!(c, Callee::Fn(id) if f.sret_callees.contains(id));
                    if let Callee::Indirect(v) | Callee::CIndirect(v, _) = c {
                        each(*v, &mut |k| note(k, p, p + 1, READ, &mut acc, &mut ent, &mut ext));
                    }
                    for (j, a) in args.iter().enumerate() {
                        each(*a, &mut |k| {
                            let s = cand[k];
                            let only = der[a.0 as usize].len() == 1;
                            let kind = if sret && j == 0 && only && f.slots[s].ret && sret_uses[s] == 1 && exact_slot[a.0 as usize] as usize == s && !args[1..].iter().any(|x| der[x.0 as usize].contains(&(s as u32))) {
                                KILL
                            } else if arg_ok[s] && only {
                                CONSUME
                            } else {
                                READ
                            };
                            note(k, p, p + 1, kind, &mut acc, &mut ent, &mut ext)
                        });
                    }
                }
                _ => for_uses(i, |v| each(v, &mut |k| note(k, p, p + 1, READ, &mut acc, &mut ent, &mut ext))),
            }
        }
        term_uses(&b.term, &mut tu);
        let p = 2 * b.insts.len() as u32;
        for v in &tu {
            each(*v, &mut |k| note(k, p, p + 1, READ, &mut acc, &mut ent, &mut ext));
        }
    }
    let live = if let Some(positions) = f.baseline.then(|| slot_linear_positions(f)).flatten() {
        // No back edges or joins: the access pass above already gives each slot's
        // lifetime. Combine the blocks' intervals in execution order, without CFG
        // fixed points or a second vreg-liveness analysis for born slots.
        acc.iter().map(|accesses| {
            let mut lo = u32::MAX;
            let mut hi = 0;
            for &(b, a, z) in accesses {
                lo = lo.min(positions[b as usize] + a);
                hi = hi.max(positions[b as usize] + z);
            }
            if lo == u32::MAX { Vec::new() } else { vec![(0, lo, hi)] }
        }).collect()
    } else {
        slot_live_intervals(f, &cand, &der, &acc, &ent, &ext)
    };
    // MAKEPAD_RUN_COLOR_TRACE=<name substring>: each slot of the matching fns, escaping or not
    static TRACE: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    if let Some(w) = TRACE.get_or_init(|| std::env::var("MAKEPAD_RUN_COLOR_TRACE").ok()) {
        if f.name.contains(w.as_str()) {
            for s in 0..ns {
                if f.slots[s].size >= min && addressed[s] {
                    let k = ci[s];
                    let iv: &[(u32, u32, u32)] = if k != usize::MAX { &live[k] } else { &[] };
                    eprintln!("color {}: slot {} size {} escapes {} candidate {} live {:?}", f.name, s, f.slots[s].size, esc[s], k != usize::MAX, iv);
                }
            }
        }
    }
    // greedy colouring, biggest first; a colour's storage is its first (largest) member's. A
    // candidate takes the first colour none of whose members' intervals meets one of its own:
    // per block, the intervals placed there with their colours (merging each colour's interval
    // list per placement and walking it per test was quadratic in a big function)
    let mut order: Vec<usize> = (0..nc).collect();
    order.sort_by(|&x, &y| f.slots[cand[y]].size.cmp(&f.slots[cand[x]].size).then(x.cmp(&y)));
    let mut occ: Vec<Vec<(u32, u32, u32)>> = vec![Vec::new(); nb];
    let mut reps: Vec<usize> = Vec::new();
    let mut hit: Vec<u32> = Vec::new();
    let mut to: Vec<u32> = (0..ns as u32).collect();
    let mut shared = false;
    for (n, k) in order.into_iter().enumerate() {
        let s = cand[k];
        let stamp = n as u32 + 1;
        for &(b, lo, hi) in &live[k] {
            for &(c, x, y) in &occ[b as usize] {
                if !(y < lo || hi < x) {
                    hit[c as usize] = stamp;
                }
            }
        }
        let c = match (0..reps.len()).find(|&c| hit[c] != stamp) {
            Some(c) => {
                to[s] = reps[c] as u32;
                shared = true;
                c
            }
            None => {
                reps.push(s);
                hit.push(0);
                reps.len() - 1
            }
        };
        for &(b, lo, hi) in &live[k] {
            occ[b as usize].push((c as u32, lo, hi));
        }
    }
    if shared { Some(to) } else { None }
}

/// Positions for a straight-line body, including chains of unconditional jumps.
/// Require every block in the chain; all other CFGs keep the full lifetime analysis.
fn slot_linear_positions(f: &Func) -> Option<Vec<u32>> {
    if f.blocks.is_empty() || f.blocks.iter().any(|b| matches!(b.term, Term::Branch(..) | Term::Invoke(..))) {
        return None;
    }
    let mut positions = vec![u32::MAX; f.blocks.len()];
    let (mut b, mut pos, mut seen) = (0usize, 0u32, 0usize);
    loop {
        if positions[b] != u32::MAX {
            return None;
        }
        positions[b] = pos;
        seen += 1;
        pos += 2 * f.blocks[b].insts.len() as u32 + 2;
        match f.blocks[b].term {
            Term::Jump(next) => b = next as usize,
            Term::Ret(_) | Term::Unreachable | Term::Resume => return (seen == f.blocks.len()).then_some(positions),
            Term::Branch(..) | Term::Invoke(..) => return None,
        }
    }
}

fn slot_live_intervals(f: &Func, cand: &[usize], der: &[Vec<u32>], acc: &[Vec<(u32, u32, u32)>], ent: &[Vec<Option<bool>>], ext: &[Vec<bool>]) -> Vec<Vec<(u32, u32, u32)>> {
    let nb = f.blocks.len();
    let nc = cand.len();
    // predecessors
    let mut preds: Vec<Vec<u32>> = vec![Vec::new(); nb];
    for (bi, b) in f.blocks.iter().enumerate() {
        for s in b.term.succs() {
            preds[s as usize].push(bi as u32);
        }
    }
    // Live intervals per candidate, (block, first, last position) in block order: where the
    // slot may hold a value (W: forward from its accesses, ended by an argument copy's
    // consuming call -- the callee owns the copy, the caller never reads it) that may still be
    // read (R: backward from its reads, ended by a whole write -- a copy over all of it, or the
    // call it is the result buffer of). Within a block from its entry when live there, else
    // from its first access, to its end when live there, else to its last access.
    // All candidates at once, one bit each (per-candidate walks over the CFG were most of the
    // analysis' cost at cold start: a slot live across a big function walked all of it).
    let w = (nc + 63) / 64;
    // per block: candidates with an access (acc), whose last access is no consuming call (W
    // gen), whose first is a read (R gen), whose accesses neither read first nor kill (R passes)
    let mut has = vec![0u64; nb * w];
    let mut w_gen = vec![0u64; nb * w];
    let mut r_gen = vec![0u64; nb * w];
    let mut r_pass = vec![0u64; nb * w];
    for k in 0..nc {
        let bit = 1u64 << (k % 64);
        for (j, &(b, _, _)) in acc[k].iter().enumerate() {
            let o = b as usize * w + k / 64;
            has[o] |= bit;
            if !ext[k][j] {
                w_gen[o] |= bit;
            }
            match ent[k][j] {
                Some(true) => r_gen[o] |= bit,
                Some(false) => {}
                None => r_pass[o] |= bit,
            }
        }
    }
    // W (forward): w_out = gen | (w_in & !has), w_in = union of the predecessors' w_out
    // R (backward): r_in = gen | (r_out & (!has | pass)), r_out = union of the successors' r_in
    let mut w_in = vec![0u64; nb * w];
    let mut w_out = w_gen.clone();
    let mut r_in = r_gen.clone();
    let mut r_out = vec![0u64; nb * w];
    let order = block_rpo(f);
    let mut changed = true;
    while changed {
        changed = false;
        for &b in &order {
            let b = b as usize;
            for j in 0..w {
                let mut x = 0u64;
                for &pb in &preds[b] {
                    x |= w_out[pb as usize * w + j];
                }
                let o = b * w + j;
                let y = w_gen[o] | (x & !has[o]);
                if x != w_in[o] || y != w_out[o] {
                    w_in[o] = x;
                    w_out[o] = y;
                    changed = true;
                }
            }
        }
    }
    changed = true;
    while changed {
        changed = false;
        for &b in order.iter().rev() {
            let b = b as usize;
            for j in 0..w {
                let mut x = 0u64;
                for s in f.blocks[b].term.succs() {
                    x |= r_in[s as usize * w + j];
                }
                let o = b * w + j;
                let y = r_gen[o] | (x & (!has[o] | r_pass[o]));
                if x != r_out[o] || y != r_in[o] {
                    r_out[o] = x;
                    r_in[o] = y;
                    changed = true;
                }
            }
        }
    }
    // intervals, in block order: within a block from its entry when live there, else from the
    // first access, to its end when live there, else to the last access
    let mut live: Vec<Vec<(u32, u32, u32)>> = vec![Vec::new(); nc];
    let mut ai = vec![0usize; nc];
    for b in 0..nb {
        let end = 2 * f.blocks[b].insts.len() as u32 + 1;
        for j in 0..w {
            let o = b * w + j;
            let lin = w_in[o] & r_in[o];
            let lout = w_out[o] & r_out[o];
            let mut m = lin | lout | has[o];
            while m != 0 {
                let t = m.trailing_zeros();
                m &= m - 1;
                let k = j * 64 + t as usize;
                let (li, lo) = (lin >> t & 1 != 0, lout >> t & 1 != 0);
                if has[o] >> t & 1 != 0 {
                    let (_, x, y) = acc[k][ai[k]];
                    ai[k] += 1;
                    live[k].push((b as u32, if li { 0 } else { x }, if lo { end } else { y }));
                } else {
                    live[k].push((b as u32, 0, end));
                }
            }
        }
    }
    // Lowered code (a baseline body: no pass has moved anything): lowering emits one SlotAddr
    // per slot, where the slot's value begins (the temporary, the `let`, the call's result
    // buffer or argument copy), so a slot's earlier contents are dead whenever its SlotAddr runs
    // -- a temporary inside a loop body is a new one per iteration (rustc: StorageLive). And
    // memory a slot that does not escape can be reached only through values derived from its
    // address. So such a slot is live only where one of those values is: in a loop that frees
    // what the reachability above cannot (it makes every slot a loop body touches live around
    // the whole loop). Passes move, sink and duplicate SlotAddrs (and inlined bodies come
    // optimised), so this holds only for `Slot::born` slots of a baseline body.
    if f.baseline && cand.iter().any(|&s| f.slots[s].born) {
        born_liveness(f, cand, der, &mut live);
    }
    live
}

/// Per vreg: defined once, by a SlotAddr (the slot's own address, offset 0).
fn slot_addr_vregs(f: &Func) -> Vec<bool> {
    let nv = f.vregs.len();
    let mut ndef = vec![0u32; nv];
    let mut sa = vec![false; nv];
    for p in &f.params {
        ndef[p.0 as usize] += 1;
    }
    for b in &f.blocks {
        for i in &b.insts {
            for_defs(i, |d| ndef[d.0 as usize] += 1);
            if let Inst::SlotAddr(d, _) = i {
                sa[d.0 as usize] = true;
            }
        }
    }
    (0..nv).map(|v| sa[v] && ndef[v] == 1).collect()
}

/// Every block once: reverse postorder of those reachable from the entry, then the others in
/// index order (dataflow sweeps in this order settle in about loop depth + 2 passes; lowering's
/// block numbers are not in flow order -- a join is made before the blocks that reach it).
fn block_rpo(f: &Func) -> Vec<u32> {
    let nb = f.blocks.len();
    let mut seen = vec![false; nb];
    let mut post: Vec<u32> = Vec::with_capacity(nb);
    let mut stack: Vec<(u32, crate::rir::Succs)> = Vec::new();
    if nb > 0 {
        seen[0] = true;
        stack.push((0, f.blocks[0].term.succs()));
    }
    while let Some((b, it)) = stack.last_mut() {
        match it.next() {
            Some(s) if !seen[s as usize] => {
                seen[s as usize] = true;
                stack.push((s, f.blocks[s as usize].term.succs()));
            }
            Some(_) => {}
            None => {
                post.push(*b);
                stack.pop();
            }
        }
    }
    post.reverse();
    post.extend((0..nb as u32).filter(|&b| !seen[b as usize]));
    post
}

/// `slot_colors` for the `Slot::born` candidates of a baseline body: their intervals narrowed
/// to where a value derived from their address is live (backward liveness of those values;
/// within a block from the entry when one is live in, else from its first reference, to the
/// end when one is live out, else to its last reference).
fn born_liveness(f: &Func, cand: &[usize], der: &[Vec<u32>], live: &mut [Vec<(u32, u32, u32)>]) {
    let nb = f.blocks.len();
    let ns = f.slots.len();
    let nc = cand.len();
    let mut ci = vec![usize::MAX; ns];
    for (k, &s) in cand.iter().enumerate() {
        if f.slots[s].born {
            ci[s] = k;
        }
    }
    // the values that may point into a born candidate, numbered
    let mut idx = vec![u32::MAX; der.len()];
    let mut vals: Vec<u32> = Vec::new();
    for (v, d) in der.iter().enumerate() {
        if d.iter().any(|&s| ci[s as usize] != usize::MAX) {
            idx[v] = vals.len() as u32;
            vals.push(v as u32);
        }
    }
    if vals.is_empty() {
        return;
    }
    let nx = vals.len();
    let w = (nx + 63) / 64;
    let has = |set: &[u64], x: usize| set[x / 64] & (1 << (x % 64)) != 0;
    // per block: values used before any def (gen), defined (kill)
    let mut gen = vec![0u64; nb * w];
    let mut kill = vec![0u64; nb * w];
    let mut tu = Vec::new();
    for (bi, b) in f.blocks.iter().enumerate() {
        let (g, k) = (&mut gen[bi * w..(bi + 1) * w], &mut kill[bi * w..(bi + 1) * w]);
        let use_v = |x: usize, g: &mut [u64], k: &[u64]| {
            if k[x / 64] & (1 << (x % 64)) == 0 {
                g[x / 64] |= 1 << (x % 64);
            }
        };
        for i in &b.insts {
            for_uses(i, |v| {
                let x = idx[v.0 as usize];
                if x != u32::MAX {
                    use_v(x as usize, g, k);
                }
            });
            for_defs(i, |v| {
                let x = idx[v.0 as usize];
                if x != u32::MAX {
                    k[x as usize / 64] |= 1 << (x % 64);
                }
            });
        }
        term_uses(&b.term, &mut tu);
        for v in &tu {
            let x = idx[v.0 as usize];
            if x != u32::MAX {
                use_v(x as usize, g, k);
            }
        }
    }
    // live_in = gen | (live_out & !kill), live_out = union of the successors' live_in
    let mut lin = vec![0u64; nb * w];
    let mut lout = vec![0u64; nb * w];
    let order = block_rpo(f);
    let mut changed = true;
    while changed {
        changed = false;
        for &bi in order.iter().rev() {
            let bi = bi as usize;
            for j in 0..w {
                let mut o = 0u64;
                for s in f.blocks[bi].term.succs() {
                    o |= lin[s as usize * w + j];
                }
                let i = gen[bi * w + j] | (o & !kill[bi * w + j]);
                if o != lout[bi * w + j] || i != lin[bi * w + j] {
                    lout[bi * w + j] = o;
                    lin[bi * w + j] = i;
                    changed = true;
                }
            }
        }
    }
    // per block: each present value's interval, then each born candidate's (their hull)
    let mut born: Vec<Vec<(u32, u32, u32)>> = vec![Vec::new(); nc];
    let mut first = vec![u32::MAX; nx];
    let mut last = vec![0u32; nx];
    let mut touched: Vec<usize> = Vec::new();
    let mut present: Vec<usize> = Vec::new();
    for (bi, b) in f.blocks.iter().enumerate() {
        touched.clear();
        let mut note = |v: VReg, lo: u32, hi: u32, touched: &mut Vec<usize>| {
            let x = idx[v.0 as usize];
            if x != u32::MAX {
                let x = x as usize;
                if first[x] == u32::MAX {
                    first[x] = lo;
                    touched.push(x);
                }
                last[x] = hi;
            }
        };
        for (ii, i) in b.insts.iter().enumerate() {
            // positions as slot_colors' (a reference takes both of its instruction's)
            let p = 2 * ii as u32;
            for_uses(i, |v| note(v, p, p + 1, &mut touched));
            for_defs(i, |v| note(v, p, p + 1, &mut touched));
        }
        let end = 2 * b.insts.len() as u32 + 1;
        term_uses(&b.term, &mut tu);
        for v in &tu {
            note(*v, end - 1, end, &mut touched);
        }
        let (li, lo) = (&lin[bi * w..(bi + 1) * w], &lout[bi * w..(bi + 1) * w]);
        present.clear();
        present.extend_from_slice(&touched);
        for j in 0..w {
            let mut m = li[j] | lo[j];
            while m != 0 {
                let x = j * 64 + m.trailing_zeros() as usize;
                m &= m - 1;
                if first[x] == u32::MAX {
                    present.push(x);
                }
            }
        }
        for &x in &present {
            let a = if has(li, x) || first[x] == u32::MAX { 0 } else { first[x] };
            let z = if has(lo, x) || first[x] == u32::MAX { end } else { last[x] };
            for &s in &der[vals[x] as usize] {
                let k = ci[s as usize];
                if k == usize::MAX {
                    continue;
                }
                match born[k].last_mut() {
                    Some(e) if e.0 == bi as u32 => {
                        e.1 = e.1.min(a);
                        e.2 = e.2.max(z);
                    }
                    _ => born[k].push((bi as u32, a, z)),
                }
            }
        }
        for &x in &touched {
            first[x] = u32::MAX;
        }
    }
    // narrowed: the intersection of both (each covers every position the slot is live at)
    for (k, &s) in cand.iter().enumerate() {
        if !f.slots[s].born {
            continue;
        }
        let (a, b) = (&live[k], &born[k]);
        let mut out = Vec::with_capacity(a.len());
        let mut j = 0;
        for &(blk, x, y) in a.iter() {
            while j < b.len() && b[j].0 < blk {
                j += 1;
            }
            if j < b.len() && b[j].0 == blk {
                let (lo, hi) = (x.max(b[j].1), y.min(b[j].2));
                if lo <= hi {
                    out.push((blk, lo, hi));
                }
            }
        }
        live[k] = out;
    }
}

/// The frame offsets of a function's stack slots: `off` is where the slot area starts (bytes
/// below the frame's base); each slot something addresses gets the end of its storage
/// (offset of its lowest byte below the base, its alignment respected), slots sharing storage
/// (`slot_colors`) the same; unaddressed slots take no space. Returns the offsets and the new
/// end of the area. `share` false (MAKEPAD_RUN_OPT=-slotcolor): every slot keeps its own storage.
pub fn slot_layout(f: &Func, mut off: i64, share: bool) -> (Vec<i64>, i64) {
    let ns = f.slots.len();
    if ns == 0 {
        return (Vec::new(), off);
    }
    let used = crate::rir::used_slots(f);
    // Small baseline frames do not repay the address/lifetime analysis. Keep
    // colouring sizeable frames: recursive parsers rely on their stack savings.
    let share = share && (!f.baseline || ns >= 8 && f.slots.iter().map(|s| s.size as u64).sum::<u64>() >= SLOT_SHARE_MIN_BYTES);
    let to = if share { slot_colors_used(f, SLOT_SHARE_MIN, &used) } else { None };
    let mut at = vec![off; ns];
    let Some(to) = to else {
        for (s, slot) in f.slots.iter().enumerate() {
            if used[s] {
                let al = slot.align.max(1) as i64;
                off += slot.size as i64;
                off = (off + al - 1) / al * al;
                at[s] = off;
            }
        }
        return (at, off);
    };
    // a shared storage's size and alignment: its members' largest
    let mut size: Vec<u32> = f.slots.iter().map(|s| s.size).collect();
    let mut align: Vec<u32> = f.slots.iter().map(|s| s.align.max(1)).collect();
    for s in 0..ns {
        let r = to[s] as usize;
        if r != s && used[s] {
            size[r] = size[r].max(f.slots[s].size);
            align[r] = align[r].max(f.slots[s].align.max(1));
        }
    }
    for s in 0..ns {
        if !used[s] || to[s] as usize != s {
            continue;
        }
        let al = align[s] as i64;
        off += size[s] as i64;
        off = (off + al - 1) / al * al;
        at[s] = off;
    }
    for s in 0..ns {
        if to[s] as usize != s {
            at[s] = at[to[s] as usize];
        }
    }
    (at, off)
}

/// slots from this size up share storage when their lifetimes cannot overlap (slot_layout)
pub const SLOT_SHARE_MIN: u32 = 8;
/// functions whose slots of that size take less than this keep them apart (slot_colors)
const SLOT_SHARE_MIN_BYTES: u64 = 256;

/// Destination forwarding of big slots, across blocks and through address arithmetic: a slot
/// B that is only written (stores and copies into it, through any index) and then copied whole
/// into slot C is built in C directly, when no access to B can follow the copy and nothing
/// reads or writes C's bytes (nor, when C's address escapes, any memory through another
/// pointer, nor a call) where B is live before the copy. `Big { a: [0; 40000], n: 1 }` then
/// `Box::new`: the array filled in a loop, copied into the struct, the struct copied into the
/// argument -- three 160 KB slots became one. Returns whether any went.
pub fn forward_slot_dests_x(options: &relax_session::Options, f: &mut Func, min: u32) -> bool {
    if opt_off(options, "-fwd") || !f.slots.iter().any(|s| s.size >= min) {
        return false;
    }
    const NONE: u32 = u32::MAX;
    const MULTI: u32 = u32::MAX - 1;
    // the caller's result buffer as a destination (no slot of ours: noalias + nocapture)
    const SRET: u32 = u32::MAX - 2;
    let mut any = false;
    for _round in 0..8 {
        let Some((der, esc)) = slot_derivation(f) else { return any };
        let ns = f.slots.len();
        let nb = f.blocks.len();
        // values that are the result buffer plus a constant (single definitions)
        let mut sret_off: Vec<Option<i64>> = vec![None; f.vregs.len()];
        if let Some(p) = f.sret {
            let mut ndef = vec![0u32; f.vregs.len()];
            for pp in &f.params {
                ndef[pp.0 as usize] += 1;
            }
            for b in &f.blocks {
                for i in &b.insts {
                    for_defs(i, |d| ndef[d.0 as usize] += 1);
                }
            }
            if ndef[p.0 as usize] == 1 {
                sret_off[p.0 as usize] = Some(0);
                let mut grew = true;
                while grew {
                    grew = false;
                    for b in &f.blocks {
                        for i in &b.insts {
                            let (d, s, k) = match i {
                                Inst::Mov(d, s) => (*d, *s, 0),
                                Inst::IBinI(IOp::Add, _, d, s, k) => (*d, *s, *k),
                                _ => continue,
                            };
                            if let Some(o) = sret_off[s.0 as usize] {
                                if ndef[d.0 as usize] == 1 && sret_off[d.0 as usize].is_none() {
                                    sret_off[d.0 as usize] = Some(o + k);
                                    grew = true;
                                }
                            }
                        }
                    }
                }
            }
        }
        // any use of the result buffer (other than as a forwarding destination) is an access to it
        let uses_sret = |i: &Inst| -> bool {
            if matches!(i, Inst::Mov(..) | Inst::IBinI(IOp::Add, ..)) {
                let mut d_s = false;
                for_defs(i, |d| d_s |= sret_off[d.0 as usize].is_some());
                if d_s {
                    return false;
                }
            }
            let mut x = false;
            for_uses(i, |v| x |= sret_off[v.0 as usize].is_some());
            x
        };
        let mut sret_blocks: Vec<bool> = vec![false; nb];
        let mut tu0 = Vec::new();
        for (bi, b) in f.blocks.iter().enumerate() {
            sret_blocks[bi] = b.insts.iter().any(|i| uses_sret(i));
            term_uses(&b.term, &mut tu0);
            sret_blocks[bi] |= tu0.iter().any(|v| sret_off[v.0 as usize].is_some());
        }
        let ds = |v: VReg| der[v.0 as usize];
        let one = |v: VReg| {
            let d = der[v.0 as usize];
            if d == NONE || d == MULTI {
                NONE
            } else {
                d
            }
        };
        // per slot: blocks touching it; whether anything but writes and the one whole copy out
        // of it uses it (a read); the whole copy out of it
        let mut acc: Vec<Vec<u32>> = vec![Vec::new(); ns];
        let mut read: Vec<bool> = vec![false; ns];
        let mut out: Vec<Option<(usize, usize, u32, i64, u32)>> = vec![None; ns];
        let mut nout: Vec<u32> = vec![0; ns];
        let mut unknown_mem: Vec<bool> = vec![false; nb];
        // constant offsets of single-def address values (SlotAddr + IBinI Add / Mov chains)
        let sc = slot_scan(f);
        for (bi, b) in f.blocks.iter().enumerate() {
            for (ii, i) in b.insts.iter().enumerate() {
                let mut touched: Vec<u32> = Vec::new();
                if let Inst::SlotAddr(_, s) = i {
                    touched.push(*s);
                }
                for_uses(i, |v| {
                    let d = one(v);
                    if d != NONE && !touched.contains(&d) {
                        touched.push(d);
                    }
                });
                // address arithmetic is no access (the address of C is computed ahead of the
                // copy into it)
                let addr_only = matches!(i, Inst::SlotAddr(..) | Inst::Mov(..) | Inst::IBin(IOp::Add, ..) | Inst::IBinI(IOp::Add, ..) | Inst::IBinI(IOp::Shl, ..));
                if !addr_only {
                    for &t in &touched {
                        if acc[t as usize].last() != Some(&(bi as u32)) {
                            acc[t as usize].push(bi as u32);
                        }
                    }
                }
                match i {
                    Inst::Store(_, base, _, v) | Inst::StoreX(_, base, _, _, _, v) => {
                        if one(*base) == NONE {
                            unknown_mem[bi] = true;
                        }
                        let dv = one(*v);
                        if dv != NONE {
                            read[dv as usize] = true;
                        }
                        if let Inst::StoreX(_, _, x, ..) = i {
                            let dx = one(*x);
                            if dx != NONE {
                                read[dx as usize] = true;
                            }
                        }
                    }
                    Inst::VStore(base, _, _) => {
                        if one(*base) == NONE {
                            unknown_mem[bi] = true;
                        }
                    }
                    Inst::Copy(d, s2, n) => {
                        if one(*d) == NONE || one(*s2) == NONE {
                            unknown_mem[bi] = true;
                        }
                        let (bs, cs) = (one(*s2), one(*d));
                        if bs != NONE {
                            let (ss, so) = sc.vs[s2.0 as usize];
                            if cs != NONE && bs != cs && ss == bs && so == 0 && *n == f.slots[bs as usize].size && sc.vs[d.0 as usize].0 == cs {
                                out[bs as usize] = Some((bi, ii, cs, sc.vs[d.0 as usize].1, *n));
                                nout[bs as usize] += 1;
                            } else if cs == NONE && ss == bs && so == 0 && *n == f.slots[bs as usize].size && sret_off[d.0 as usize].is_some() {
                                out[bs as usize] = Some((bi, ii, SRET, sret_off[d.0 as usize].unwrap(), *n));
                                nout[bs as usize] += 1;
                            } else {
                                read[bs as usize] = true;
                                nout[bs as usize] += 2;
                            }
                        }
                    }
                    Inst::SlotAddr(..) | Inst::Mov(..) | Inst::IBin(IOp::Add, ..) | Inst::IBinI(IOp::Add, ..) | Inst::IBinI(IOp::Shl, ..) => {}
                    _ => {
                        // a call's result buffer (first argument of a callee returning in memory)
                        // at a slot's start is only written by the call
                        let mut wonly = NONE;
                        if let Inst::Call(Callee::Fn(id), args, _) = i {
                            if let Some(a0) = args.first() {
                                let d = one(*a0);
                                if d != NONE && f.sret_callees.contains(id) && sc.vs[a0.0 as usize] == (d, 0) && !args[1..].iter().any(|x| one(*x) == d) {
                                    wonly = d;
                                }
                            }
                        }
                        // loads through it, calls taking it, compares...: a read
                        for &t in &touched {
                            if t != wonly {
                                read[t as usize] = true;
                            }
                        }
                        if is_mem_effect(i) || matches!(i, Inst::AtomicLoad(..)) {
                            unknown_mem[bi] = true;
                        }
                        match i {
                            Inst::Load(_, _, base, _) | Inst::VLoad(_, base, _) | Inst::LoadX(_, _, base, ..) if one(*base) == NONE => unknown_mem[bi] = true,
                            _ => {}
                        }
                    }
                }
            }
        }
        // terminators using a slot address: a read
        let mut tu = Vec::new();
        for b in &f.blocks {
            term_uses(&b.term, &mut tu);
            for v in &tu {
                let d = ds(*v);
                if d != NONE && d != MULTI {
                    read[d as usize] = true;
                }
            }
        }
        let mut preds: Vec<Vec<u32>> = vec![Vec::new(); nb];
        for (bi, b) in f.blocks.iter().enumerate() {
            for s in b.term.succs() {
                preds[s as usize].push(bi as u32);
            }
        }
        let mut used = vec![false; ns];
        // one slot into the result buffer per round: sret_blocks / uses_sret describe the
        // buffer's accesses before this round, so a second slot moved into it in the same round
        // is not checked against the first one's live range (png's decode_into_inner: two
        // results built on overlapping paths, one freed the other's Vec)
        let mut sret_taken = false;
        let mut repl: Vec<(u32, u32, i64, usize, usize)> = Vec::new();
        let mut st: Vec<u32> = Vec::new();
        for b in 0..ns {
            let Some((cb, ci, c, o, n)) = out[b] else { continue };
            let to_sret = c == SRET;
            if to_sret && sret_taken {
                continue;
            }
            // reads of B before the copy are fine when B moves into the result buffer: nothing
            // else names that buffer, so it holds exactly what B would (a cleanup pad dropping a
            // field of the half-built value reads it there -- rustc builds into sret the same way)
            if f.slots[b].size < min || esc[b] || (read[b] && !to_sret) || nout[b] != 1 || used[b] || o < 0 {
                continue;
            }
            if !to_sret && (used[c as usize] || o + n as i64 > f.slots[c as usize].size as i64) {
                continue;
            }
            let uses_slot = |i: &Inst, t: u32| -> bool {
                if matches!(i, Inst::SlotAddr(..) | Inst::Mov(..) | Inst::IBin(IOp::Add, ..) | Inst::IBinI(IOp::Add, ..) | Inst::IBinI(IOp::Shl, ..)) {
                    return false;
                }
                let mut x = false;
                for_uses(i, |v| x |= one(v) == t);
                x
            };
            // the copy is B's last access
            if f.blocks[cb].insts[ci + 1..].iter().any(|i| uses_slot(i, b as u32)) {
                continue;
            }
            let mut fwd = vec![false; nb];
            st.extend(f.blocks[cb].term.succs());
            while let Some(x) = st.pop() {
                if !fwd[x as usize] {
                    fwd[x as usize] = true;
                    st.extend(f.blocks[x as usize].term.succs());
                }
            }
            if acc[b].iter().any(|&x| fwd[x as usize]) {
                continue;
            }
            // B's live region before the copy
            let mut from_b = vec![false; nb];
            for &x in &acc[b] {
                if !from_b[x as usize] {
                    from_b[x as usize] = true;
                    st.push(x);
                }
            }
            while let Some(x) = st.pop() {
                for s2 in f.blocks[x as usize].term.succs() {
                    if !from_b[s2 as usize] {
                        from_b[s2 as usize] = true;
                        st.push(s2);
                    }
                }
            }
            // B is live wherever a path leads from one of its accesses to another one (not only
            // to the copy): a read of B on a path that never reaches the copy still needs B's
            // bytes. `Result<&T, E>::cloned` (O3, by-value self moved into the result
            // buffer for the Err path's copy): the Ok path read the &T from self after the
            // inlined T::clone had written the Ok payload over it.
            let mut to_cb = vec![false; nb];
            for &x in &acc[b] {
                if !to_cb[x as usize] {
                    to_cb[x as usize] = true;
                    st.push(x);
                }
            }
            if !to_cb[cb] {
                to_cb[cb] = true;
                st.push(cb as u32);
            }
            while let Some(x) = st.pop() {
                for &p in &preds[x as usize] {
                    if !to_cb[p as usize] {
                        to_cb[p as usize] = true;
                        st.push(p);
                    }
                }
            }
            let c_esc = !to_sret && esc[c as usize];
            let c_acc = |x: usize| if to_sret { sret_blocks[x] } else { acc[c as usize].contains(&(x as u32)) };
            let mut clash = (0..nb).any(|x| x != cb && from_b[x] && to_cb[x] && (c_acc(x) || (c_esc && unknown_mem[x])));
            if !clash && to_sret {
                // the result buffer: nothing in cb before the copy may touch it
                clash = f.blocks[cb].insts[..ci].iter().any(|i| uses_sret(i));
            }
            if !clash && to_sret {
                // B's bytes become the buffer's: an access of B that can follow an access of the
                // buffer and leave the function without passing the copy would hand the caller
                // B's half-built bytes over what that access put there (block granularity)
                let mut after_sret = vec![false; nb];
                for x in 0..nb {
                    if sret_blocks[x] && !after_sret[x] {
                        after_sret[x] = true;
                        st.push(x as u32);
                    }
                }
                while let Some(x) = st.pop() {
                    for s2 in f.blocks[x as usize].term.succs() {
                        if !after_sret[s2 as usize] {
                            after_sret[s2 as usize] = true;
                            st.push(s2);
                        }
                    }
                }
                // blocks with a path to a function exit that does not pass cb
                let mut exits = vec![false; nb];
                for x in 0..nb {
                    if x != cb && f.blocks[x].term.succs().next().is_none() {
                        exits[x] = true;
                        st.push(x as u32);
                    }
                }
                while let Some(x) = st.pop() {
                    for &p in &preds[x as usize] {
                        if p as usize != cb && !exits[p as usize] {
                            exits[p as usize] = true;
                            st.push(p);
                        }
                    }
                }
                clash = acc[b].iter().any(|&x| x as usize != cb && after_sret[x as usize] && exits[x as usize]);
            }
            if !clash && !to_sret {
                // in cb, from the block start (B may be live there) to the copy
                for i in &f.blocks[cb].insts[..ci] {
                    let unk = match i {
                        Inst::Load(_, _, base, _) | Inst::VLoad(_, base, _) | Inst::LoadX(_, _, base, ..) | Inst::Store(_, base, ..) | Inst::VStore(base, ..) | Inst::StoreX(_, base, ..) => one(*base) == NONE,
                        Inst::Copy(d, s2, _) => one(*d) == NONE || one(*s2) == NONE,
                        _ => is_mem_effect(i) || matches!(i, Inst::AtomicLoad(..)),
                    };
                    if uses_slot(i, c) || (c_esc && unk) {
                        clash = true;
                        break;
                    }
                }
            }
            if clash {
                continue;
            }
            used[b] = true;
            if !to_sret {
                used[c as usize] = true;
            }
            sret_taken |= to_sret;
            repl.push((b as u32, c, o, cb, ci));
        }
        if repl.is_empty() {
            break;
        }
        let mut to: Vec<Option<(u32, i64)>> = vec![None; ns];
        for &(b, c, o, ..) in &repl {
            to[b as usize] = Some((c, o));
            if c != SRET {
                let al = f.slots[b as usize].align.max(f.slots[c as usize].align);
                f.slots[c as usize].align = al;
            }
        }
        let sret = f.sret;
        let mut dels: Vec<(usize, usize)> = repl.iter().map(|r| (r.3, r.4)).collect();
        dels.sort_unstable_by(|x, y| y.cmp(x));
        for (cb, ci) in dels {
            f.blocks[cb].insts.remove(ci);
            if ci < f.blocks[cb].pos.len() {
                f.blocks[cb].pos.remove(ci);
            }
        }
        for bi in 0..f.blocks.len() {
            let mut k = 0;
            while k < f.blocks[bi].insts.len() {
                if let Inst::SlotAddr(d, s) = f.blocks[bi].insts[k] {
                    if let Some((c, o)) = to[s as usize] {
                        if c == SRET {
                            let p = sret.unwrap();
                            f.blocks[bi].insts[k] = if o == 0 { Inst::Mov(d, p) } else { Inst::IBinI(IOp::Add, IntTy { bits: 64, signed: false }, d, p, o) };
                        } else if o == 0 {
                            f.blocks[bi].insts[k] = Inst::SlotAddr(d, c);
                        } else {
                            let t = f.vreg(Cls::I);
                            f.blocks[bi].insts[k] = Inst::SlotAddr(t, c);
                            f.blocks[bi].insts.insert(k + 1, Inst::IBinI(IOp::Add, IntTy { bits: 64, signed: false }, d, t, o));
                            if f.blocks[bi].pos.len() + 1 == f.blocks[bi].insts.len() {
                                let p = f.blocks[bi].pos[k];
                                f.blocks[bi].pos.insert(k + 1, p);
                            }
                            k += 1;
                        }
                    }
                }
                k += 1;
            }
        }
        any = true;
    }
    any
}

/// Drops slots no SlotAddr refers to any more (forwarding and colouring leave them behind;
/// codegen lays out every slot of the function) and renumbers the rest. Returns bytes freed.
pub fn compact_slots(f: &mut Func) -> u64 {
    let ns = f.slots.len();
    let mut used = vec![false; ns];
    for b in &f.blocks {
        for i in &b.insts {
            if let Inst::SlotAddr(_, s) = i {
                used[*s as usize] = true;
            }
        }
    }
    if used.iter().all(|u| *u) {
        return 0;
    }
    let mut map = vec![u32::MAX; ns];
    let mut slots = Vec::new();
    let mut freed = 0u64;
    for s in 0..ns {
        if used[s] {
            map[s] = slots.len() as u32;
            slots.push(f.slots[s].clone());
        } else {
            freed += f.slots[s].size as u64;
        }
    }
    f.slots = slots;
    for b in f.blocks.iter_mut() {
        for i in b.insts.iter_mut() {
            if let Inst::SlotAddr(_, s) = i {
                *s = map[*s as usize];
            }
        }
    }
    freed
}

/// Values derived from stack slot addresses: `der[v]` = the slot v points into (u32::MAX none,
/// u32::MAX - 1 several), through moves, address arithmetic (any index), selects and call
/// results of calls taking it; `esc[s]`: s's address is stored to memory, returned, a
/// parameter, or mixed with another slot's. None when the fixpoint does not settle.
fn slot_derivation(f: &Func) -> Option<(Vec<u32>, Vec<bool>)> {
    const NONE: u32 = u32::MAX;
    const MULTI: u32 = u32::MAX - 1;
    let ns = f.slots.len();
    let nv = f.vregs.len();
    let mut der: Vec<u32> = vec![NONE; nv];
    let mut esc = vec![false; ns];
    let join = |a: u32, b: u32, esc: &mut Vec<bool>| -> u32 {
        if a == NONE || a == b {
            return b;
        }
        if b == NONE {
            return a;
        }
        if a != MULTI {
            esc[a as usize] = true;
        }
        if b != MULTI {
            esc[b as usize] = true;
        }
        MULTI
    };
    // derivation fixpoint
    let mut changed = true;
    let mut rounds = 0;
    while changed && rounds < 16 {
        changed = false;
        rounds += 1;
        for b in &f.blocks {
            for i in &b.insts {
                let src = match i {
                    Inst::SlotAddr(_, s) => *s,
                    Inst::Load(..) | Inst::LoadX(..) | Inst::VLoad(..) | Inst::AtomicLoad(..) | Inst::Iconst(..) | Inst::Fconst(..) | Inst::Addr(..) | Inst::FnAddr(..) | Inst::TlsAddr(..) | Inst::VConst(..) => NONE,
                    // stores, copies: no result (their operands are not joined)
                    _ => {
                        let mut has_def = false;
                        for_defs(i, |_| has_def = true);
                        if !has_def {
                            continue;
                        }
                        let mut s = NONE;
                        for_uses(i, |v| {
                            let d = der[v.0 as usize];
                            if d != NONE {
                                s = join(s, d, &mut esc);
                            }
                        });
                        s
                    }
                };
                if src == NONE {
                    continue;
                }
                for_defs(i, |d| {
                    let old = der[d.0 as usize];
                    let new = join(old, src, &mut esc);
                    if new != old {
                        der[d.0 as usize] = new;
                        changed = true;
                    }
                });
            }
        }
    }
    if changed {
        // no fixpoint in the bound
        return None;
    }
    let dslot = |v: VReg| der[v.0 as usize];
    // escapes: a derived value stored as data, returned, or ambiguous
    for b in &f.blocks {
        for i in &b.insts {
            let stored: &[VReg] = match i {
                Inst::Store(_, _, _, v) | Inst::StoreX(_, _, _, _, _, v) | Inst::AtomicStore(_, _, _, v) | Inst::AtomicRmw(_, _, _, _, _, v) => std::slice::from_ref(v),
                Inst::AtomicCas(_, _, _, _, _, e, n) => {
                    for v in [e, n] {
                        let d = dslot(*v);
                        if d != NONE && d != MULTI {
                            esc[d as usize] = true;
                        }
                    }
                    &[]
                }
                _ => &[],
            };
            for v in stored {
                let d = dslot(*v);
                if d != NONE && d != MULTI {
                    esc[d as usize] = true;
                }
            }
        }
        if let Term::Ret(vs) = &b.term {
            for v in vs {
                let d = dslot(*v);
                if d != NONE && d != MULTI {
                    esc[d as usize] = true;
                }
            }
        }
    }
    for p in &f.params {
        let d = dslot(*p);
        if d != NONE && d != MULTI {
            esc[d as usize] = true;
        }
    }
    Some((der, esc))
}

/// color_slots' address analysis: the SET of slots (up to SLOT_SET_MAX) each value may point
/// into -- through moves, address arithmetic, selects and calls (a call's results may point
/// into any slot its arguments point into: `slice::get(&a, &b) -> &T`); a value that may point
/// into several slots keeps all of them live where it is used, instead of making them escape.
/// esc[s]: an address into s is stored to memory, returned, a parameter, or in a set too big.
fn slot_sets(f: &Func) -> Option<(Vec<Vec<u32>>, Vec<bool>, Vec<bool>)> {
    const SLOT_SET_MAX: usize = 4;
    let ns = f.slots.len();
    let nv = f.vregs.len();
    let mut der: Vec<Vec<u32>> = vec![Vec::new(); nv];
    let mut esc = vec![false; ns];
    let mut changed = true;
    let mut rounds = 0;
    let mut src: Vec<u32> = Vec::new();
    while changed && rounds < 16 {
        changed = false;
        rounds += 1;
        for b in &f.blocks {
            for i in &b.insts {
                src.clear();
                match i {
                    Inst::SlotAddr(_, s) => src.push(*s),
                    Inst::Load(..) | Inst::LoadX(..) | Inst::VLoad(..) | Inst::AtomicLoad(..) | Inst::Iconst(..) | Inst::Fconst(..) | Inst::Addr(..) | Inst::FnAddr(..) | Inst::TlsAddr(..) | Inst::VConst(..) => continue,
                    // a call's results do not point into its own memory (a result buffer, an
                    // argument copy: see below; a copy that turns out not to be one escapes)
                    Inst::Call(c, args, rets) => {
                        if rets.is_empty() {
                            continue;
                        }
                        if let Callee::Indirect(v) | Callee::CIndirect(v, _) = c {
                            src.extend(der[v.0 as usize].iter().copied());
                        }
                        let sret = matches!(c, Callee::Fn(id) if f.sret_callees.contains(id));
                        for (k, a) in args.iter().enumerate() {
                            let d = &der[a.0 as usize];
                            if (sret && k == 0) || matches!(d.as_slice(), [x] if f.slots[*x as usize].arg) {
                                continue;
                            }
                            for &x in d {
                                if !src.contains(&x) {
                                    src.push(x);
                                }
                            }
                        }
                    }
                    _ => {
                        let mut has_def = false;
                        for_defs(i, |_| has_def = true);
                        if !has_def {
                            continue;
                        }
                        for_uses(i, |v| {
                            for &d in &der[v.0 as usize] {
                                if !src.contains(&d) {
                                    src.push(d);
                                }
                            }
                        });
                    }
                }
                if src.is_empty() {
                    continue;
                }
                for_defs(i, |d| {
                    let cur = &mut der[d.0 as usize];
                    for &s in &src {
                        if !cur.contains(&s) {
                            cur.push(s);
                            changed = true;
                        }
                    }
                });
            }
        }
        // sets too big: their slots escape, the sets stop growing
        for v in der.iter_mut() {
            if v.len() > SLOT_SET_MAX {
                for &s in v.iter() {
                    esc[s as usize] = true;
                }
                v.truncate(SLOT_SET_MAX);
            }
        }
    }
    if changed {
        return None;
    }
    let mark = |v: VReg, esc: &mut Vec<bool>| {
        for &s in &der[v.0 as usize] {
            esc[s as usize] = true;
        }
    };
    for b in &f.blocks {
        for i in &b.insts {
            match i {
                Inst::Store(_, _, _, v) | Inst::StoreX(_, _, _, _, _, v) | Inst::AtomicStore(_, _, _, v) | Inst::AtomicRmw(_, _, _, _, _, v) => mark(*v, &mut esc),
                Inst::AtomicCas(_, _, _, _, _, e, n) => {
                    mark(*e, &mut esc);
                    mark(*n, &mut esc);
                }
                _ => {}
            }
        }
        if let Term::Ret(vs) = &b.term {
            for v in vs {
                mark(*v, &mut esc);
            }
        }
    }
    // a slot address handed to a call together with another argument that may point to memory:
    // the callee may store the address there (an iterator adapter built into its result buffer
    // keeps `&hist`: `hist.iter().enumerate().take(n)` not inlined), and later reads of that
    // memory are loads, invisible to `der` -- the slot escapes. Only constants are known not to
    // point anywhere. (deflate's radix_sort_symbols: `offsets` was coloured onto `hist`, dead by
    // `der` after the adapter call while the loop still read it through the adapter.)
    let mut konst = vec![false; nv];
    let mut ndef = vec![0u32; nv];
    for p in &f.params {
        ndef[p.0 as usize] += 1;
    }
    for b in &f.blocks {
        for i in &b.insts {
            for_defs(i, |d| ndef[d.0 as usize] += 1);
            if let Inst::Iconst(d, _) | Inst::Fconst(d, _, _) | Inst::FnAddr(d, _) = i {
                konst[d.0 as usize] = true;
            }
        }
    }
    // The call's own memory does not escape through it: a result buffer (the first argument of
    // a call returning in memory: `sret_callees`) and a by-value argument copy (`Slot::arg`) --
    // the callee writes the result / owns the copy and keeps no pointer to either past the call
    // (rustc: indirect arguments and the indirect return place are `noalias captures(address)`:
    // no access through a kept pointer after the return). A result buffer is still memory the
    // callee may store the other arguments' addresses into (the result can hold them); a copy
    // is not, when nothing reads it (the callee owns it). A copy counts as one only while it is
    // what lowering made: handed to one call, as one argument, never read, nor mixed with other
    // slots' addresses (passes such as destination forwarding may give it other uses).
    let mut arg_ok: Vec<bool> = f.slots.iter().map(|s| s.arg).collect();
    if arg_ok.iter().any(|x| *x) {
        let mut calls = vec![0u32; ns];
        let one = |v: VReg| -> Option<usize> {
            match der[v.0 as usize].as_slice() {
                [s] => Some(*s as usize),
                _ => None,
            }
        };
        for b in &f.blocks {
            for i in &b.insts {
                match i {
                    Inst::Call(c, args, _) => {
                        if let Callee::Indirect(v) | Callee::CIndirect(v, _) = c {
                            for &d in &der[v.0 as usize] {
                                arg_ok[d as usize] = false;
                            }
                        }
                        for a in args {
                            match one(*a) {
                                Some(d) => calls[d] += 1,
                                None => {
                                    for &d in &der[a.0 as usize] {
                                        arg_ok[d as usize] = false;
                                    }
                                }
                            }
                        }
                    }
                    Inst::Load(_, _, a, _) | Inst::LoadX(_, _, a, ..) | Inst::VLoad(_, a, _) | Inst::VGather(_, a, _) | Inst::AtomicLoad(_, _, _, a) | Inst::AtomicRmw(_, _, _, _, a, _) | Inst::AtomicCas(_, _, _, _, a, _, _) | Inst::Copy(_, a, _) => {
                        for &d in &der[a.0 as usize] {
                            arg_ok[d as usize] = false;
                        }
                    }
                    _ => {}
                }
            }
        }
        for d in 0..ns {
            if calls[d] != 1 {
                arg_ok[d] = false;
            }
            // the derivation above left its call results out
            if f.slots[d].arg && !arg_ok[d] {
                esc[d] = true;
            }
        }
    }
    for b in &f.blocks {
        for i in &b.insts {
            if let Inst::Call(c, args, _) = i {
                if args.iter().any(|a| !der[a.0 as usize].is_empty()) {
                    let sret = matches!(c, Callee::Fn(id) if f.sret_callees.contains(id));
                    let copy = |a: VReg| matches!(der[a.0 as usize].as_slice(), [d] if arg_ok[*d as usize]);
                    // arguments that may point to memory the callee can store an address into
                    let pointerish = |a: VReg| !(konst[a.0 as usize] && ndef[a.0 as usize] == 1) && !copy(a);
                    let n = args.iter().filter(|a| pointerish(**a)).count();
                    for (k, a) in args.iter().enumerate() {
                        if (sret && k == 0) || copy(*a) {
                            continue;
                        }
                        if n > pointerish(*a) as usize {
                            mark(*a, &mut esc);
                        }
                    }
                }
            }
        }
    }
    Some((der, esc, arg_ok))
}

/// Sinks a pure single-definition, single-use arithmetic result into the block that uses it when
/// that block's only predecessor is the defining block: `t = j - 1` computed at a loop's top for
/// the step `j = t` at its bottom moves next to the step, where the allocator gives t and j one
/// register (`sub r, 1` instead of `lea t, [j - 1] ... mov j, t`, the value no longer live across
/// the loop body). The operands must not be redefined after the definition in its block nor
/// before the use in the using block, and the defining block's terminator must not read the
/// result. Returns whether anything moved.
pub fn sink_single_uses(f: &mut Func) -> bool {
    let nv = f.vregs.len();
    let nb = f.blocks.len();
    let mut ndef = vec![0u32; nv];
    let mut nuse = vec![0u32; nv];
    let mut use_blk = vec![u32::MAX; nv];
    for p in &f.params {
        ndef[p.0 as usize] += 1;
    }
    let mut tu = Vec::new();
    for (bi, b) in f.blocks.iter().enumerate() {
        for i in &b.insts {
            for_defs(i, |d| ndef[d.0 as usize] += 1);
            for_uses(i, |u| {
                nuse[u.0 as usize] += 1;
                use_blk[u.0 as usize] = bi as u32;
            });
        }
        term_uses(&b.term, &mut tu);
        for u in &tu {
            nuse[u.0 as usize] += 1;
            use_blk[u.0 as usize] = u32::MAX - 1;
        }
    }
    let mut npred = vec![0u32; nb];
    let mut pred1 = vec![u32::MAX; nb];
    for (bi, b) in f.blocks.iter().enumerate() {
        for s in b.term.succs() {
            npred[s as usize] += 1;
            pred1[s as usize] = bi as u32;
        }
    }
    let mut changed = false;
    for bi in 0..nb {
        let mut k = 0;
        while k < f.blocks[bi].insts.len() {
            let i = f.blocks[bi].insts[k].clone();
            let (d, ops): (VReg, Vec<VReg>) = match i {
                Inst::IBinI(op, _, d, a, _) if !matches!(op, IOp::Div | IOp::Rem) => (d, vec![a]),
                Inst::IBin(op, _, d, a, b) if !matches!(op, IOp::Div | IOp::Rem | IOp::MulHi) => (d, vec![a, b]),
                _ => {
                    k += 1;
                    continue;
                }
            };
            let dv = d.0 as usize;
            let ub = use_blk[dv];
            if ndef[dv] != 1 || nuse[dv] != 1 || ub as usize >= nb || ub as usize == bi || npred[ub as usize] != 1 || pred1[ub as usize] != bi as u32 || ops.contains(&d) {
                k += 1;
                continue;
            }
            // operands not redefined after the def here
            let mut ok = true;
            for j in k + 1..f.blocks[bi].insts.len() {
                for_defs(&f.blocks[bi].insts[j], |x| ok &= !ops.contains(&x));
            }
            // ... nor before the use there
            let ubk = ub as usize;
            let mut at = None;
            for (j, ins) in f.blocks[ubk].insts.iter().enumerate() {
                let mut used = false;
                for_uses(ins, |u| used |= u == d);
                if used {
                    at = Some(j);
                    break;
                }
                for_defs(ins, |x| ok &= !ops.contains(&x));
            }
            let Some(at) = at else {
                k += 1;
                continue;
            };
            if !ok {
                k += 1;
                continue;
            }
            let p = f.blocks[bi].pos.remove(k);
            f.blocks[bi].insts.remove(k);
            f.blocks[ubk].insts.insert(at, i);
            f.blocks[ubk].pos.insert(at, p);
            changed = true;
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot_copy_fixture(ret: bool) -> (Func, [VReg; 5]) {
        let mut f = Func::new("slot_copy".into(), 0);
        let (sa, sb) = (f.slot(16, 8), f.slot(24, 8));
        f.slots[sa as usize].ret = ret;
        let regs = std::array::from_fn(|_| f.vreg(Cls::I));
        let [a, b, k, x, y] = regs;
        f.params.push(k);
        f.rets = vec![Cls::I, Cls::I];
        f.block();
        f.blocks[0].insts = vec![
            Inst::SlotAddr(a, sa), Inst::SlotAddr(b, sb),
            if ret { Inst::Call(Callee::Fn(7), vec![a], vec![]) } else { Inst::Store(Mem::Int(8, false), a, 0, k) },
            Inst::Copy(b, a, 16),
            Inst::Load(Mem::Int(8, false), x, b, 0), Inst::Load(Mem::Int(8, false), y, b, 8),
        ];
        f.blocks[0].term = Term::Ret(vec![x, y]);
        f.blocks[0].pos = vec![0; f.blocks[0].insts.len()];
        (f, regs)
    }

    #[test]
    fn slot_copies_forward_private_and_single_call_result_slots() {
        for ret in [false, true] {
            let (mut f, [a, b, _, _, _]) = slot_copy_fixture(ret);
            let scan = slot_scan(&f);
            assert_eq!(scan.esc[0], ret); // other slot passes still see the call escape
            assert_eq!(scan.ret_call[0], ret.then_some((0, 2)));
            let mut expected = f.blocks[0].insts.clone();
            expected.remove(3);
            expected[1] = Inst::SlotAddr(b, 0);
            assert!(forward_slot_copies(&mut f));
            assert_eq!(f.blocks[0].insts, expected);
            let scan = slot_scan(&f);
            assert_eq!(scan.vs[a.0 as usize], (0, 0));
            assert_eq!(scan.vs[b.0 as usize], (0, 0));
            assert!(f.blocks[0].insts.iter().filter_map(|i| match i {
                Inst::Load(_, _, base, _) => Some(scan.vs[base.0 as usize].0),
                _ => None,
            }).all(|s| s == 0));
            assert_eq!(f.blocks[0].insts.len(), f.blocks[0].pos.len());
            f.verify().unwrap();
        }
    }

    #[test]
    fn slot_copies_keep_a_result_passed_to_two_calls() {
        let (mut f, [a, _, _, _, _]) = slot_copy_fixture(true);
        // Two calls before the copy: this must reject even though neither dirties a read.
        f.blocks[0].insts.insert(3, Inst::Call(Callee::Fn(8), vec![a], vec![]));
        f.blocks[0].pos = vec![0; f.blocks[0].insts.len()];
        let before = f.blocks[0].insts.clone();
        assert!(!forward_slot_copies(&mut f));
        assert_eq!(f.blocks[0].insts, before);
        f.verify().unwrap();
    }

    #[test]
    fn slot_copies_forward_only_the_prefix_with_a_tail_store() {
        let (mut f, [a, b, k, x, y]) = slot_copy_fixture(false);
        f.blocks[0].insts.insert(4, Inst::Store(Mem::Int(8, false), b, 16, k));
        f.blocks[0].insts[6] = Inst::Load(Mem::Int(8, false), y, b, 16);
        // A source write after the forwarded read must not block the remaining tail read.
        f.blocks[0].insts.insert(6, Inst::Store(Mem::Int(8, false), a, 0, k));
        f.blocks[0].pos = (0..f.blocks[0].insts.len() as u64).collect();
        assert!(forward_slot_copies(&mut f));
        let scan = slot_scan(&f);
        assert_eq!(scan.vs[b.0 as usize], (1, 0));
        for (i, &pos) in f.blocks[0].insts.iter().zip(&f.blocks[0].pos) {
            match i {
                Inst::Load(_, d, base, off) if *d == x => {
                    assert_eq!((scan.vs[base.0 as usize], *off, pos), ((0, 0), 0, 5));
                }
                Inst::Load(_, d, base, off) if *d == y => {
                    assert_eq!((scan.vs[base.0 as usize], *off, pos), ((1, 0), 16, 7));
                }
                Inst::SlotAddr(_, 0) if pos != 0 => assert_eq!(pos, 5),
                Inst::Store(_, base, 16, _) => assert_eq!(scan.vs[base.0 as usize], (1, 0)),
                Inst::Copy(..) => panic!("prefix copy survived"),
                _ => {}
            }
        }
        assert_eq!(f.blocks[0].insts.len(), f.blocks[0].pos.len());
        f.verify().unwrap();
    }

    #[test]
    fn slot_copies_keep_an_overwritten_prefix() {
        let (mut f, [_, b, k, _, _]) = slot_copy_fixture(false);
        f.blocks[0].insts.insert(4, Inst::Store(Mem::Int(8, false), b, 8, k));
        f.blocks[0].pos = vec![0; f.blocks[0].insts.len()];
        let before = f.blocks[0].insts.clone();
        assert!(!forward_slot_copies(&mut f));
        assert_eq!(f.blocks[0].insts, before);
        f.verify().unwrap();
    }

    #[test]
    fn slot_copies_keep_reads_after_a_source_write() {
        for ret in [false, true] {
            for partial in [false, true] {
                let (mut f, [a, b, k, _, _]) = slot_copy_fixture(ret);
                if ret {
                    // Move the sole result-buffer call between the copy and the read.
                    f.blocks[0].insts.swap(2, 3);
                } else {
                    f.blocks[0].insts.insert(4, Inst::Store(Mem::Int(8, false), a, 0, k));
                }
                if partial { f.blocks[0].insts.push(Inst::Store(Mem::Int(8, false), b, 16, k)); }
                f.blocks[0].pos = vec![0; f.blocks[0].insts.len()];
                let before = f.blocks[0].insts.clone();
                assert!(!forward_slot_copies(&mut f));
                assert_eq!(f.blocks[0].insts, before);
                f.verify().unwrap();
            }
        }
    }

    #[test]
    fn slot_copies_keep_noncall_escapes_and_unmarked_call_buffers() {
        for escape in 0..4 {
            let (mut f, [a, _, k, _, _]) = slot_copy_fixture(true);
            match escape {
                0 => f.slots[0].ret = false,
                1 => f.blocks[0].insts.push(Inst::Store(Mem::Int(8, false), k, 0, a)),
                2 => {
                    let cmp = f.vreg(Cls::I);
                    f.blocks[0].insts.push(Inst::ICmpI(Cond::Eq, false, cmp, a, 0));
                }
                _ => f.blocks[0].term = Term::Ret(vec![a, k]),
            }
            f.blocks[0].pos = vec![0; f.blocks[0].insts.len()];
            let before = f.blocks[0].insts.clone();
            assert!(!forward_slot_copies(&mut f));
            assert_eq!(f.blocks[0].insts, before);
            f.verify().unwrap();
        }
    }

    #[test]
    fn slot_copies_keep_straddling_and_indexed_accesses() {
        for access in 0..4 {
            let (mut f, [_, b, k, x, _]) = slot_copy_fixture(false);
            f.blocks[0].insts.insert(4, Inst::Store(Mem::Int(8, false), b, 16, k));
            match access {
                0 => f.blocks[0].insts[5] = Inst::Load(Mem::Int(8, false), x, b, 12),
                1 => f.blocks[0].insts[5] = Inst::LoadX(Mem::Int(8, false), x, b, k, 0, 16),
                2 => f.blocks[0].insts[4] = Inst::StoreX(Mem::Int(8, false), b, k, 0, 16, k),
                _ => f.blocks[0].insts.push(Inst::Copy(k, b, 24)),
            }
            f.blocks[0].pos = vec![0; f.blocks[0].insts.len()];
            let before = f.blocks[0].insts.clone();
            assert!(!forward_slot_copies(&mut f));
            assert_eq!(f.blocks[0].insts, before);
            f.verify().unwrap();
        }
    }

    #[test]
    fn slot_copies_forward_derived_vector_and_copy_reads_with_a_tail_copy() {
        let (mut f, [_, b, k, x, _]) = slot_copy_fixture(false);
        let (field, tail, vector) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::V128));
        f.blocks[0].insts.splice(4..5, [
            Inst::IBinI(IOp::Add, U64, tail, b, 16), Inst::Copy(tail, k, 8),
            Inst::IBinI(IOp::Add, U64, field, b, 8), Inst::Load(Mem::Int(8, false), x, field, 0),
            Inst::VLoad(vector, b, 0), Inst::Copy(k, field, 8),
        ]);
        f.blocks[0].pos = vec![0; f.blocks[0].insts.len()];
        assert!(forward_slot_copies(&mut f));
        let scan = slot_scan(&f);
        assert_eq!(scan.vs[b.0 as usize], (1, 0));
        assert_eq!(scan.vs[tail.0 as usize], (1, 16));
        for i in &f.blocks[0].insts {
            match i {
                Inst::Load(_, d, base, 0) if *d == x => assert_eq!(scan.vs[base.0 as usize], (0, 8)),
                Inst::VLoad(_, base, 0) => assert_eq!(scan.vs[base.0 as usize], (0, 0)),
                Inst::Copy(d, src, 8) if *d == k => assert_eq!(scan.vs[src.0 as usize], (0, 8)),
                Inst::Copy(d, src, 8) => assert_eq!((*d, *src), (tail, k)),
                Inst::Copy(..) => panic!("prefix copy survived"),
                _ => {}
            }
        }
        assert_eq!(f.blocks[0].insts.len(), f.blocks[0].pos.len());
        f.verify().unwrap();
    }

    #[test]
    fn dests_x_moves_one_slot_per_round_into_the_result_buffer() {
        // two results built side by side, each returned on its own path: only one may be built
        // in the caller's buffer, or the second's stores overwrite the first's
        let mut f = Func::new("two_results".into(), 0);
        let (p, c) = (f.vreg(Cls::I), f.vreg(Cls::I));
        f.params.extend([p, c]);
        f.sret = Some(p);
        let (s0, s1) = (f.slot(32, 8), f.slot(32, 8));
        let v: Vec<VReg> = (0..6).map(|_| f.vreg(Cls::I)).collect();
        for _ in 0..3 { f.block(); }
        f.blocks[0].insts = vec![
            Inst::SlotAddr(v[0], s0), Inst::SlotAddr(v[1], s1), Inst::Iconst(v[2], 1), Inst::Iconst(v[3], 2),
            Inst::Store(Mem::Int(8, false), v[0], 0, v[2]), Inst::Store(Mem::Int(8, false), v[1], 0, v[3]),
        ];
        f.blocks[0].term = Term::Branch(c, 1, 2);
        f.blocks[1].insts = vec![Inst::SlotAddr(v[4], s0), Inst::Copy(p, v[4], 32)];
        f.blocks[1].term = Term::Ret(vec![]);
        f.blocks[2].insts = vec![Inst::SlotAddr(v[5], s1), Inst::Copy(p, v[5], 32)];
        f.blocks[2].term = Term::Ret(vec![]);
        for b in &mut f.blocks { b.pos = vec![0; b.insts.len()]; }
        assert!(forward_slot_dests_x(&relax_session::Options::default(), &mut f, 32));
        let left = f.blocks.iter().flat_map(|b| &b.insts).filter(|i| matches!(i, Inst::SlotAddr(..))).count();
        assert!(left >= 1, "both slots were moved into the result buffer");
    }

    #[test]
    fn dests_x_keeps_a_slot_read_after_result_writes_out_of_the_result_buffer() {
        // Result<&T, E>::cloned: self (by value, in a slot) is copied whole into the result
        // buffer on the Err path; the Ok path reads the &T from self after writing the Ok
        // payload over the bytes self would occupy there
        let mut f = Func::new("cloned".into(), 0);
        let (p, src, c) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
        f.params.extend([p, src, c]);
        f.sret = Some(p);
        let s = f.slot(72, 8);
        let v: Vec<VReg> = (0..8).map(|_| f.vreg(Cls::I)).collect();
        for _ in 0..3 { f.block(); }
        f.blocks[0].insts = vec![Inst::SlotAddr(v[0], s), Inst::Copy(v[0], src, 72)];
        f.blocks[0].term = Term::Branch(c, 1, 2);
        f.blocks[1].insts = vec![
            Inst::SlotAddr(v[1], s), Inst::Load(Mem::Int(8, false), v[2], v[1], 8),
            Inst::Iconst(v[3], 0), Inst::Store(Mem::Int(8, false), p, 0, v[3]),
            // the payload store lands where self's byte 8.. would be (out + 8 + 8)
            Inst::Store(Mem::Int(8, false), p, 16, v[2]),
            Inst::Load(Mem::Int(8, false), v[4], v[1], 8), Inst::Store(Mem::Int(8, false), p, 24, v[4]),
        ];
        f.blocks[1].term = Term::Ret(vec![]);
        f.blocks[2].insts = vec![
            Inst::IBinI(IOp::Add, U64, v[6], p, 8), Inst::SlotAddr(v[7], s), Inst::Copy(v[6], v[7], 72),
            Inst::Iconst(v[5], 1), Inst::Store(Mem::Int(8, false), p, 0, v[5]),
        ];
        f.blocks[2].term = Term::Ret(vec![]);
        for b in &mut f.blocks { b.pos = vec![0; b.insts.len()]; }
        f.verify().unwrap();
        assert!(!forward_slot_dests_x(&relax_session::Options::default(), &mut f, 32), "self was moved into the result buffer");
    }

    #[test]
    fn dests_x_moves_a_slot_into_the_result_buffer_when_no_other_path_reads_it() {
        // the same shape without the Ok path's reads of self: building self in place is fine
        let mut f = Func::new("err_only".into(), 0);
        let (p, src, c) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
        f.params.extend([p, src, c]);
        f.sret = Some(p);
        let s = f.slot(72, 8);
        let v: Vec<VReg> = (0..8).map(|_| f.vreg(Cls::I)).collect();
        for _ in 0..3 { f.block(); }
        f.blocks[0].insts = vec![Inst::SlotAddr(v[0], s), Inst::Copy(v[0], src, 72)];
        f.blocks[0].term = Term::Branch(c, 1, 2);
        f.blocks[1].insts = vec![Inst::Iconst(v[3], 0), Inst::Store(Mem::Int(8, false), p, 0, v[3])];
        f.blocks[1].term = Term::Ret(vec![]);
        f.blocks[2].insts = vec![
            Inst::IBinI(IOp::Add, U64, v[6], p, 8), Inst::SlotAddr(v[7], s), Inst::Copy(v[6], v[7], 72),
            Inst::Iconst(v[5], 1), Inst::Store(Mem::Int(8, false), p, 0, v[5]),
        ];
        f.blocks[2].term = Term::Ret(vec![]);
        for b in &mut f.blocks { b.pos = vec![0; b.insts.len()]; }
        f.verify().unwrap();
        assert!(forward_slot_dests_x(&relax_session::Options::default(), &mut f, 32));
    }

    #[test]
    fn dests_x_keeps_a_slot_written_after_a_result_write_on_an_exit_path_out_of_the_result_buffer() {
        // the buffer is written in full (Err), then B is written on a path that returns that
        // Err without copying B: B built in the buffer would overwrite the returned bytes
        let mut f = Func::new("err_then_build".into(), 0);
        let (p, c) = (f.vreg(Cls::I), f.vreg(Cls::I));
        f.params.extend([p, c]);
        f.sret = Some(p);
        let s = f.slot(32, 8);
        let v: Vec<VReg> = (0..4).map(|_| f.vreg(Cls::I)).collect();
        for _ in 0..4 { f.block(); }
        f.blocks[0].insts = vec![Inst::Iconst(v[0], 7), Inst::Store(Mem::Int(8, false), p, 8, v[0])];
        f.blocks[0].term = Term::Jump(1);
        f.blocks[1].insts = vec![Inst::SlotAddr(v[1], s), Inst::Iconst(v[2], 9), Inst::Store(Mem::Int(8, false), v[1], 0, v[2])];
        f.blocks[1].term = Term::Branch(c, 2, 3);
        f.blocks[2].insts = vec![Inst::SlotAddr(v[3], s), Inst::Copy(p, v[3], 32)];
        f.blocks[2].term = Term::Ret(vec![]);
        f.blocks[3].insts = vec![];
        f.blocks[3].term = Term::Ret(vec![]);
        for b in &mut f.blocks { b.pos = vec![0; b.insts.len()]; }
        f.verify().unwrap();
        assert!(!forward_slot_dests_x(&relax_session::Options::default(), &mut f, 32), "B was built over the returned result");
    }

    fn sroa_fixture(size: u32) -> (Func, VReg) {
        let mut f = Func::new("sroa".into(), 0);
        let base = f.vreg(Cls::I);
        let sl = f.slot(size, 8);
        f.block();
        f.blocks[0].insts.push(Inst::SlotAddr(base, sl));
        f.blocks[0].term = Term::Ret(vec![]);
        (f, base)
    }

    fn sroa_prepare(f: &mut Func) {
        for b in &mut f.blocks {
            b.pos = vec![123; b.insts.len()];
            b.term_pos = 123;
        }
        f.verify().unwrap();
    }

    fn sroa_promote(f: &mut Func) {
        sroa_prepare(f);
        let mut stats = SroaStats { candidates: f.slots.len(), ..SroaStats::default() };
        assert!(sroa_inner(f, Some(&mut stats), true));
        assert_eq!(stats.candidates, 1);
        assert_eq!(stats.promoted, 1);
        assert_eq!(stats.rejects, [0; 7]);
        // The normal cleanup removes the now-unused slot address and its derivations.
        dce(f);
        f.verify().unwrap();
        for b in &f.blocks {
            assert_eq!(b.insts.len(), b.pos.len());
            assert!(b.pos.iter().all(|&p| p == 123));
            assert!(!b.insts.iter().any(|i| matches!(i, Inst::SlotAddr(..) | Inst::Copy(..))));
        }
    }

    fn sroa_rejects(f: &mut Func, why: SroaReject) {
        sroa_prepare(f);
        let before = f.dump();
        let mut stats = SroaStats { candidates: f.slots.len(), ..SroaStats::default() };
        assert!(!sroa_inner(f, Some(&mut stats), true));
        assert_eq!(f.dump(), before);
        assert_eq!(stats.candidates, 1);
        assert_eq!(stats.promoted, 0);
        assert_eq!(stats.rejects[why as usize], 1);
        assert_eq!(stats.rejects.iter().sum::<usize>(), 1);
    }

    fn sroa_copy_fixture(size: u32, fields: &[(i32, Mem)]) -> (Func, VReg, VReg) {
        let (mut f, base) = sroa_fixture(size);
        let src = f.vreg(Cls::I);
        let dst = f.vreg(Cls::I);
        f.params.extend([src, dst]);
        f.blocks[0].insts.push(Inst::Copy(base, src, size));
        let mut rets = Vec::new();
        for &(off, m) in fields {
            let cls = match m { Mem::F32 => Cls::F32, Mem::F64 => Cls::F64, _ => Cls::I };
            let r = f.vreg(cls);
            f.rets.push(cls);
            rets.push(r);
            f.blocks[0].insts.push(Inst::Load(m, r, base, off));
        }
        f.blocks[0].insts.push(Inst::Copy(dst, base, size));
        f.blocks[0].term = Term::Ret(rets);
        (f, src, dst)
    }

    #[test]
    fn sroa_option_float_and_integer_payload_subfields_stay_in_memory() {
        let (mut f, base) = sroa_fixture(16);
        let cond = f.vreg(Cls::I);
        let value = f.vreg(Cls::F64);
        let bits = f.vreg(Cls::I);
        f.params.extend([cond, value, bits]);
        let tag = f.vreg(Cls::I);
        let payload = f.vreg(Cls::F64);
        let high = f.vreg(Cls::I);
        f.rets.extend([Cls::I, Cls::F64, Cls::I]);
        for _ in 0..3 { f.block(); }
        f.blocks[0].term = Term::Branch(cond, 1, 2);
        f.blocks[1].insts = vec![Inst::Iconst(tag, 1), Inst::Store(Mem::Int(1, false), base, 0, tag), Inst::Store(Mem::F64, base, 8, value)];
        f.blocks[1].term = Term::Jump(3);
        f.blocks[2].insts = vec![
            Inst::Iconst(tag, 0), Inst::Store(Mem::Int(1, false), base, 0, tag),
            Inst::Store(Mem::Int(8, false), base, 8, bits),
            // Also exercise a partial variant write and a signed read at a nonzero offset.
            Inst::Iconst(high, -1), Inst::Store(Mem::Int(1, true), base, 15, high),
        ];
        f.blocks[2].term = Term::Jump(3);
        f.blocks[3].insts = vec![Inst::Load(Mem::Int(1, false), tag, base, 0), Inst::Load(Mem::F64, payload, base, 8), Inst::Load(Mem::Int(1, true), high, base, 15)];
        f.blocks[3].term = Term::Ret(vec![tag, payload, high]);
        sroa_rejects(&mut f, SroaReject::KindMix);
    }

    #[test]
    fn sroa_u32_read_as_two_u16_halves_stays_in_memory() {
        let (mut f, base) = sroa_fixture(4);
        let value = f.vreg(Cls::I);
        let low = f.vreg(Cls::I);
        let high = f.vreg(Cls::I);
        f.params.push(value);
        f.rets.extend([Cls::I, Cls::I]);
        f.blocks[0].insts.extend([
            Inst::Store(Mem::Int(4, false), base, 0, value),
            Inst::Load(Mem::Int(2, false), low, base, 0), Inst::Load(Mem::Int(2, false), high, base, 2),
        ]);
        f.blocks[0].term = Term::Ret(vec![low, high]);
        sroa_rejects(&mut f, SroaReject::KindMix);
    }

    #[test]
    fn sroa_signed_and_unsigned_loads_share_a_cell() {
        for n in [1, 2, 4, 8] {
            let (mut f, base) = sroa_fixture(n as u32);
            let value = f.vreg(Cls::I);
            let signed = f.vreg(Cls::I);
            let unsigned = f.vreg(Cls::I);
            f.params.push(value);
            f.rets.extend([Cls::I, Cls::I]);
            f.blocks[0].insts.extend([
                Inst::Store(Mem::Int(n, true), base, 0, value),
                Inst::Load(Mem::Int(n, true), signed, base, 0), Inst::Load(Mem::Int(n, false), unsigned, base, 0),
            ]);
            f.blocks[0].term = Term::Ret(vec![signed, unsigned]);
            sroa_promote(&mut f);
            let insts = &f.blocks[0].insts;
            assert!(!insts.iter().any(|i| matches!(i, Inst::Load(..) | Inst::Store(..))));
            assert!(insts.iter().any(|i| matches!(*i, Inst::Mov(_, v) if v == value)));
            if n < 8 {
                for (d, sg) in [(signed, true), (unsigned, false)] {
                    assert!(insts.iter().any(|i| matches!(*i, Inst::Conv(Conv::IntToInt(t), r, _) if r == d && t == (IntTy { bits: n * 8, signed: sg }))));
                }
            }
        }
    }

    #[test]
    fn sroa_f32_subfield_stays_in_memory() {
        let (mut f, base) = sroa_fixture(8);
        let bits = f.vreg(Cls::I);
        let value = f.vreg(Cls::F32);
        let result = f.vreg(Cls::F32);
        let whole = f.vreg(Cls::I);
        f.params.extend([bits, value]);
        f.rets.extend([Cls::F32, Cls::I]);
        f.blocks[0].insts.extend([
            Inst::Store(Mem::Int(8, false), base, 0, bits), Inst::Store(Mem::F32, base, 4, value),
            Inst::Load(Mem::F32, result, base, 4), Inst::Load(Mem::Int(8, false), whole, base, 0),
        ]);
        f.blocks[0].term = Term::Ret(vec![result, whole]);
        sroa_rejects(&mut f, SroaReject::Overlap);
    }

    #[test]
    fn sroa_union_over_eight_bytes_stays_in_memory() {
        let (mut f, base) = sroa_fixture(16);
        let value = f.vreg(Cls::I);
        f.params.push(value);
        // The middle access connects two otherwise disjoint payload words.
        for off in [0, 4, 8] {
            f.blocks[0].insts.push(Inst::Store(Mem::Int(8, false), base, off, value));
        }
        sroa_rejects(&mut f, SroaReject::Overlap);
    }

    #[test]
    fn sroa_float_and_integer_kinds_share_a_cell() {
        for (m, n, cls) in [(Mem::F32, 4, Cls::F32), (Mem::F64, 8, Cls::F64)] {
            // Pure kind mixes need no alignment restriction.
            for off in [0, 1] {
                let (mut f, base) = sroa_fixture(off as u32 + n as u32);
                let value = f.vreg(cls);
                let bits = f.vreg(Cls::I);
                let float = f.vreg(cls);
                f.params.push(value);
                f.rets.extend([Cls::I, cls]);
                f.blocks[0].insts.extend([
                    Inst::Store(m, base, off, value),
                    Inst::Load(Mem::Int(n, false), bits, base, off), Inst::Load(m, float, base, off),
                ]);
                f.blocks[0].term = Term::Ret(vec![bits, float]);
                sroa_promote(&mut f);
                let insts = &f.blocks[0].insts;
                assert!(!insts.iter().any(|i| matches!(i, Inst::Load(..) | Inst::Store(..) | Inst::IBin(..) | Inst::IBinI(..))));
                let wide = m == Mem::F64;
                let cell = insts.iter().find_map(|i| match *i {
                    Inst::Conv(Conv::FToBits(w), d, v) if w == wide && v == value => Some(d), _ => None,
                }).unwrap();
                assert!(insts.iter().any(|i| matches!(*i, Inst::Conv(Conv::BitsToF(w), d, r) if w == wide && d == float && r == cell)));
                if n < 8 {
                    assert!(insts.iter().any(|i| matches!(*i, Inst::Conv(Conv::IntToInt(U32), d, r) if d == bits && r == cell)));
                } else {
                    assert!(insts.iter().any(|i| matches!(*i, Inst::Mov(d, r) if d == bits && r == cell)));
                }
            }
        }
    }

    #[test]
    fn sroa_sret_copy_in_and_out_preserves_all_three_words() {
        let (mut f, src, dst) = sroa_copy_fixture(24, &[(8, Mem::Int(8, false))]);
        sroa_promote(&mut f);
        let loads: Vec<_> = f.blocks[0].insts.iter().filter_map(|i| match *i { Inst::Load(m, r, b, o) => Some((m, r, b, o)), _ => None }).collect();
        let stores: Vec<_> = f.blocks[0].insts.iter().filter_map(|i| match *i { Inst::Store(m, b, o, r) => Some((m, r, b, o)), _ => None }).collect();
        assert_eq!(loads.len(), 3);
        assert_eq!(stores.len(), 3);
        for (k, (load, store)) in loads.iter().zip(&stores).enumerate() {
            assert_eq!(*load, (Mem::Int(8, false), load.1, src, k as i32 * 8));
            assert_eq!(*store, (Mem::Int(8, false), load.1, dst, k as i32 * 8));
        }
    }

    #[test]
    fn sroa_tiles_unnamed_bytes_with_aligned_integer_cells() {
        for size in [3, 7, 15, 64] {
            let (mut f, src, dst) = sroa_copy_fixture(size, &[]);
            sroa_promote(&mut f);
            let mut end = 0;
            for inst in &f.blocks[0].insts {
                if let Inst::Load(Mem::Int(n, false), r, b, off) = *inst {
                    assert_eq!(b, src);
                    assert_eq!(off, end);
                    assert_eq!(off % n as i32, 0);
                    let mut expected = 8;
                    while end % expected != 0 || end + expected > size as i32 { expected /= 2; }
                    assert_eq!(n as i32, expected);
                    assert!(f.blocks[0].insts.iter().any(|i| matches!(*i, Inst::Store(Mem::Int(w, false), p, o, v) if (w, p, o, v) == (n, dst, off, r))));
                    end += n as i32;
                }
            }
            assert_eq!(end, size as i32);
        }
    }

    #[test]
    fn sroa_bytewise_copy_in_out_group_stays_in_memory() {
        // 16 one-byte fields copied in and out: tiling would turn each 16-byte copy into 16
        // byte loads or stores
        let fields: Vec<(i32, Mem)> = (0..16).map(|o| (o, Mem::Int(1, false))).collect();
        let (mut f, _, _) = sroa_copy_fixture(16, &fields);
        sroa_rejects(&mut f, SroaReject::CopyCells);
    }

    #[test]
    fn sroa_copy_in_out_range_and_tiled_cell_limits() {
        let (mut large, _, _) = sroa_copy_fixture(65, &[(8, Mem::Int(8, false))]);
        sroa_rejects(&mut large, SroaReject::CopyInOut);
        // Eight named half-words and eight gap half-words (64 bytes) fit the raised budget;
        // sixteen 2-byte fields with 2-byte gaps (32 cells) do not.
        for count in [8, 16] {
            let step = 64 / count;
            let fields: Vec<_> = (0..count).map(|i| (i * step, Mem::Int(if count == 8 { 4 } else { 2 }, false))).collect();
            let (mut f, _, _) = sroa_copy_fixture(64, &fields);
            if count == 8 {
                sroa_promote(&mut f);
                assert_eq!(f.blocks[0].insts.iter().filter(|i| matches!(i, Inst::Load(..))).count(), 16);
            } else {
                sroa_rejects(&mut f, SroaReject::CopyCells);
            }
        }
    }

    #[test]
    fn sroa_gap_cells_respect_overlapping_copy_endpoints() {
        let (mut f, base) = sroa_fixture(16);
        let src = f.vreg(Cls::I);
        let dst = f.vreg(Cls::I);
        let part = f.vreg(Cls::I);
        f.params.extend([src, dst]);
        f.blocks[0].insts.extend([
            Inst::IBinI(IOp::Add, U64, part, base, 3),
            Inst::Copy(base, src, 16), Inst::Copy(part, src, 5), Inst::Copy(dst, base, 16),
        ]);
        sroa_promote(&mut f);
        let stores: Vec<_> = f.blocks[0].insts.iter().filter_map(|i| match *i {
            Inst::Store(m, b, o, _) if b == dst => Some((o, m)), _ => None,
        }).collect();
        assert_eq!(stores, vec![(0, Mem::Int(2, false)), (2, Mem::Int(1, false)), (3, Mem::Int(1, false)), (4, Mem::Int(4, false)), (8, Mem::Int(8, false))]);
        assert_eq!(f.blocks[0].insts.iter().filter(|i| matches!(i, Inst::Load(..))).count(), 7);
    }

    #[test]
    fn sroa_copy_in_out_group_with_offset_members_requires_matching_sizes() {
        for n in [4, 8] {
            let (mut f, base) = sroa_fixture(24);
            let other = f.vreg(Cls::I);
            let part = f.vreg(Cls::I);
            let sl = f.slot(32, 8);
            let src = f.vreg(Cls::I);
            let dst = f.vreg(Cls::I);
            let word = f.vreg(Cls::I);
            let float = f.vreg(Cls::F64);
            f.params.extend([src, dst]);
            f.rets.extend([Cls::I, Cls::F64]);
            f.blocks[0].insts.extend([
                Inst::SlotAddr(other, sl), Inst::IBinI(IOp::Add, U64, part, other, 8),
                Inst::Copy(base, src, 24), Inst::Load(Mem::Int(n, true), word, base, 8),
                Inst::Copy(part, base, 24), Inst::Load(Mem::F64, float, other, 16), Inst::Copy(dst, part, 24),
            ]);
            f.blocks[0].term = Term::Ret(vec![word, float]);
            if n != 8 {
                sroa_rejects(&mut f, SroaReject::KindMix);
                continue;
            }
            sroa_promote(&mut f);
            let insts = &f.blocks[0].insts;
            assert_eq!(insts.iter().filter(|i| matches!(i, Inst::Load(Mem::Int(8, false), ..))).count(), 3);
            assert_eq!(insts.iter().filter(|i| matches!(i, Inst::Store(Mem::Int(8, false), ..))).count(), 3);
            assert!(insts.iter().any(|i| matches!(i, Inst::Conv(Conv::BitsToF(true), ..))));
            assert!(insts.iter().any(|i| matches!(*i, Inst::Mov(d, _) if d == word)));
        }
    }

    #[test]
    fn sroa_partial_access_or_copy_cut_stays_in_memory() {
        for (m, off, why) in [(Mem::Int(2, false), 2, SroaReject::Overlap), (Mem::Int(8, true), 0, SroaReject::CopyCut)] {
            let (mut f, base) = sroa_fixture(8);
            let dst = f.vreg(Cls::I);
            let value = f.vreg(Cls::I);
            let result = f.vreg(Cls::I);
            f.params.extend([dst, value]);
            f.rets.push(Cls::I);
            f.blocks[0].insts.extend([
                Inst::Store(Mem::Int(8, false), base, 0, value), Inst::Load(m, result, base, off), Inst::Copy(dst, base, 4),
            ]);
            f.blocks[0].term = Term::Ret(vec![result]);
            sroa_rejects(&mut f, why);
        }
    }

    #[test]
    fn sroa_copied_group_counts_only_pure_cells_once() {
        for (m, field_off) in [(Mem::Int(2, false), 2), (Mem::Int(8, true), 0)] {
            let (mut f, base) = sroa_fixture(64);
            let other = f.vreg(Cls::I);
            let sl = f.slot(64, 8);
            let value = f.vreg(Cls::I);
            let dst = f.vreg(Cls::I);
            f.params.extend([value, dst]);
            f.blocks[0].insts.push(Inst::SlotAddr(other, sl));
            for off in (0..64).step_by(8) {
                f.blocks[0].insts.push(Inst::Store(Mem::Int(8, false), base, off, value));
            }
            f.blocks[0].insts.push(Inst::Copy(other, base, 64));
            let mut rets = Vec::new();
            for off in (0..64).step_by(8) {
                let r = f.vreg(Cls::I);
                f.rets.push(Cls::I);
                rets.push(r);
                f.blocks[0].insts.push(Inst::Load(m, r, other, off + field_off));
            }
            f.blocks[0].insts.push(Inst::Copy(dst, other, 64));
            f.blocks[0].term = Term::Ret(rets);
            if field_off != 0 {
                sroa_rejects(&mut f, SroaReject::Overlap);
                continue;
            }
            sroa_promote(&mut f);
            assert_eq!(f.blocks[0].insts.iter().filter(|i| matches!(i, Inst::Store(Mem::Int(8, false), ..))).count(), 8);
            assert!(!f.blocks[0].insts.iter().any(|i| matches!(i, Inst::Load(..))));
        }
    }

    #[test]
    fn sroa_size_mix_at_group_members_slot_boundary_stays_in_memory() {
        let (mut f, base) = sroa_fixture(12);
        let other = f.vreg(Cls::I);
        let sl = f.slot(16, 8);
        let value = f.vreg(Cls::I);
        let result = f.vreg(Cls::I);
        f.params.push(value);
        f.rets.push(Cls::I);
        f.blocks[0].insts.extend([
            Inst::SlotAddr(other, sl), Inst::Store(Mem::Int(8, false), other, 8, value),
            Inst::Copy(base, other, 4), Inst::Load(Mem::Int(4, false), result, base, 8),
        ]);
        f.blocks[0].term = Term::Ret(vec![result]);
        sroa_rejects(&mut f, SroaReject::KindMix);
    }
}

#[cfg(test)]
mod instcombine_tests {
    use super::*;

    const I8: IntTy = IntTy { bits: 8, signed: true };

    fn fixture(params: u32, regs: u32, blocks: u32) -> Func {
        let mut f = Func::new("instcombine".into(), 0);
        for _ in 0..regs { f.vreg(Cls::I); }
        f.params = (0..params).map(VReg).collect();
        f.rets = vec![Cls::I];
        for _ in 0..blocks { f.block(); }
        f
    }

    fn positions(f: &mut Func) {
        for b in &mut f.blocks { b.pos = vec![123; b.insts.len()]; }
    }

    fn check(f: &Func) {
        f.verify().unwrap();
        for b in &f.blocks {
            assert_eq!(b.insts.len(), b.pos.len());
            assert!(b.pos.iter().all(|&p| p == 123));
        }
    }

    fn simplify_ic(f: &mut Func, ic: bool) {
        let mut options = relax_session::Options::default();
        options.opt = vec!["-thread".into(), "-ifconv".into()];
        if !ic { options.opt.push("-ic".into()); }
        simplify(&options, f, None, true, false);
        check(f);
    }

    // Interpret the integer-only fixtures to check values as well as instruction shapes.
    fn eval(f: &Func, args: &[i64]) -> i64 {
        let mut values = vec![0; f.vregs.len()];
        for (&p, &x) in f.params.iter().zip(args) { values[p.0 as usize] = x; }
        let mut bi = 0;
        for _ in 0..64 {
            for i in &f.blocks[bi].insts {
                let get = |v: VReg| values[v.0 as usize];
                let (d, value) = match *i {
                    Inst::Iconst(d, x) => (d, x),
                    Inst::Mov(d, x) => (d, get(x)),
                    Inst::Conv(Conv::IntToInt(t), d, x) => (d, ext(get(x), t)),
                    Inst::IBinI(op, t, d, x, k) => (d, fold_ibin(op, t, get(x), k).unwrap()),
                    Inst::ICmpI(c, signed, d, x, k) => (d, cmp_i(c, signed, get(x), k) as i64),
                    Inst::Sel(d, c, x, y) => (d, get(if get(c) != 0 { x } else { y })),
                    _ => panic!("unsupported fixture instruction: {i:?}"),
                };
                values[d.0 as usize] = value;
            }
            match &f.blocks[bi].term {
                Term::Jump(b) => bi = *b as usize,
                Term::Branch(c, t, e) => bi = if values[c.0 as usize] != 0 { *t } else { *e } as usize,
                Term::Ret(v) => return values[v[0].0 as usize],
                _ => panic!("fixture did not return"),
            }
        }
        panic!("fixture did not terminate");
    }

    #[test]
    fn nonnull_loop_carried_pointer_arithmetic() {
        let mut f = fixture(3, 9, 3);
        let [base, step, cond, leaf, p, next, back, selected, result] = std::array::from_fn::<_, 9, _>(|i| VReg(i as u32));
        f.nonnull.push(leaf);
        f.blocks[0].insts = vec![Inst::Load(Mem::Int(8, false), leaf, base, 0), Inst::Mov(p, leaf)];
        f.blocks[0].term = Term::Jump(1);
        f.blocks[1].insts = vec![
            Inst::IBin(IOp::Add, U64, next, p, step),
            Inst::IBinI(IOp::Add, U64, back, next, -4),
            Inst::Sel(selected, cond, back, leaf),
            Inst::Mov(p, selected),
            Inst::ICmpI(Cond::Ne, false, result, p, 0),
        ];
        f.blocks[1].term = Term::Branch(cond, 1, 2);
        f.blocks[2].term = Term::Ret(vec![result]);
        positions(&mut f);
        assert!(fold_nonnull(&mut f));
        assert_eq!(f.blocks[1].insts.last(), Some(&Inst::Iconst(result, 1)));
        check(&f);
    }

    #[test]
    fn integer_add_and_nullable_def_stay_unknown() {
        for marked in [false, true] {
            let mut f = fixture(1, 4, 1);
            let (base, p, sum, result) = (VReg(0), VReg(1), VReg(2), VReg(3));
            f.blocks[0].insts = if marked {
                f.nonnull.push(p);
                vec![Inst::Load(Mem::Int(8, false), p, base, 0), Inst::Iconst(p, 0)]
            } else {
                vec![Inst::Iconst(p, 7)]
            };
            f.blocks[0].insts.extend([Inst::IBinI(IOp::Add, U64, sum, p, 4), Inst::ICmpI(Cond::Ne, false, result, sum, 0)]);
            f.blocks[0].term = Term::Ret(vec![result]);
            positions(&mut f);
            assert!(!fold_nonnull(&mut f));
            assert!(matches!(f.blocks[0].insts.last(), Some(Inst::ICmpI(..))));
            check(&f);
        }
    }

    #[test]
    fn nonnull_reaches_fixpoint_past_sixteen_sweeps() {
        let mut f = fixture(0, 23, 23);
        f.blocks[0].insts = vec![Inst::Iconst(VReg(0), 0)];
        f.blocks[0].term = Term::Jump(21);
        for v in 1..=21 {
            let bi = 22 - v;
            f.blocks[bi].insts = vec![Inst::Mov(VReg(v as u32), VReg(v as u32 - 1))];
            f.blocks[bi].term = Term::Jump(if bi == 1 { 22 } else { bi as u32 - 1 });
        }
        f.blocks[22].insts = vec![Inst::ICmpI(Cond::Ne, false, VReg(22), VReg(21), 0)];
        f.blocks[22].term = Term::Ret(vec![VReg(22)]);
        positions(&mut f);
        assert!(!fold_nonnull(&mut f));
        check(&f);
    }

    #[test]
    fn nonnull_address_leaves_and_switch() {
        for leaf in [Inst::SlotAddr(VReg(0), 0), Inst::Addr(VReg(0), Sym::new(0)), Inst::FnAddr(VReg(0), 0)] {
            for ic in [false, true] {
                let mut f = fixture(0, 3, 1);
                f.slot(8, 8);
                f.blocks[0].insts = vec![leaf.clone(), Inst::IBinI(IOp::Add, U64, VReg(1), VReg(0), -4), Inst::ICmpI(Cond::Eq, false, VReg(2), VReg(1), 0)];
                f.blocks[0].term = Term::Ret(vec![VReg(2)]);
                positions(&mut f);
                simplify_ic(&mut f, ic);
                assert_eq!(f.blocks.iter().flat_map(|b| &b.insts).any(|i| matches!(i, Inst::ICmpI(..))), !ic);
                if ic { assert_eq!(eval(&f, &[]), 0); }
            }
        }
    }

    fn phi_fixture(narrow: bool, cc: Cond, signed: bool, k: i64) -> Func {
        let mut f = fixture(2, 5, 4);
        f.blocks[0].term = Term::Branch(VReg(0), 1, 2);
        f.blocks[1].insts = vec![Inst::Iconst(VReg(2), 255)];
        f.blocks[2].insts = vec![Inst::Iconst(VReg(2), 1)];
        f.blocks[1].term = Term::Jump(3);
        f.blocks[2].term = Term::Jump(3);
        if narrow { f.blocks[3].insts.push(Inst::Conv(Conv::IntToInt(I8), VReg(3), VReg(2))); }
        f.blocks[3].insts.push(Inst::ICmpI(cc, signed, VReg(4), if narrow { VReg(3) } else { VReg(2) }, k));
        f.blocks[3].term = Term::Ret(vec![VReg(4)]);
        positions(&mut f);
        f
    }

    #[test]
    fn compares_constant_phis_with_narrowing_and_switch() {
        for narrow in [false, true] {
            for signed in [false, true] {
                for cc in [Cond::Eq, Cond::Ne, Cond::Lt, Cond::Le, Cond::Gt, Cond::Ge] {
                    for ic in [false, true] {
                        let mut f = phi_fixture(narrow, cc, signed, 1);
                        let expected = [eval(&f, &[0, 0]), eval(&f, &[1, 0])];
                        simplify_ic(&mut f, ic);
                        assert_eq!([eval(&f, &[0, 0]), eval(&f, &[1, 0])], expected);
                        assert_eq!(f.blocks.iter().flat_map(|b| &b.insts).any(|i| matches!(i, Inst::ICmpI(..))), !ic);
                    }
                }
            }
        }
    }

    fn ordering_loop_fixture(narrow: bool) -> Func {
        let mut f = fixture(2, 8, 8);
        let [choice, count, ordering, narrowed, result, again, less, equal] = std::array::from_fn::<_, 8, _>(|i| VReg(i as u32));
        f.blocks[0].term = Term::Jump(1);
        f.blocks[1].insts = vec![Inst::ICmpI(Cond::Lt, true, less, choice, 0)];
        f.blocks[1].term = Term::Branch(less, 2, 3);
        f.blocks[2].insts = vec![Inst::Iconst(ordering, if narrow { 255 } else { -1 })];
        f.blocks[2].term = Term::Jump(6);
        f.blocks[3].insts = vec![Inst::ICmpI(Cond::Eq, true, equal, choice, 0)];
        f.blocks[3].term = Term::Branch(equal, 4, 5);
        f.blocks[4].insts = vec![Inst::Iconst(ordering, 0)];
        f.blocks[4].term = Term::Jump(6);
        f.blocks[5].insts = vec![Inst::Iconst(ordering, 1)];
        f.blocks[5].term = Term::Jump(6);
        if narrow { f.blocks[6].insts.push(Inst::Conv(Conv::IntToInt(I8), narrowed, ordering)); }
        f.blocks[6].insts.extend([
            Inst::ICmpI(Cond::Eq, true, result, if narrow { narrowed } else { ordering }, 1),
            Inst::IBinI(IOp::Xor, U64, choice, choice, 1),
            Inst::IBinI(IOp::Sub, U64, count, count, 1),
            Inst::ICmpI(Cond::Ne, false, again, count, 0),
        ]);
        f.blocks[6].term = Term::Branch(again, 1, 7);
        f.blocks[7].term = Term::Ret(vec![result]);
        positions(&mut f);
        f
    }

    fn has_ordering_compare(f: &Func) -> bool {
        f.blocks.iter().flat_map(|b| &b.insts).any(|i| matches!(i, Inst::ICmpI(Cond::Eq, true, _, _, 1)))
    }

    #[test]
    fn per_iteration_ordering_phi_still_folds() {
        for narrow in [false, true] {
            for ic in [false, true] {
                let mut f = ordering_loop_fixture(narrow);
                let args: Vec<_> = [-1, 0, 1].into_iter().flat_map(|choice| (1..=3).map(move |n| [choice, n])).collect();
                let expected: Vec<_> = args.iter().map(|a| eval(&f, a)).collect();
                simplify_ic(&mut f, ic);
                assert_eq!(has_ordering_compare(&f), !ic);
                assert_eq!(args.iter().map(|a| eval(&f, a)).collect::<Vec<_>>(), expected);
            }
        }
    }

    #[test]
    fn loop_carried_constant_phi_is_not_folded() {
        for narrow in [false, true] {
            let mut f = fixture(1, 5, 4);
            let [count, value, result, again, narrowed] = std::array::from_fn::<_, 5, _>(|i| VReg(i as u32));
            f.blocks[0].insts = vec![Inst::Iconst(value, 255)];
            f.blocks[0].term = Term::Jump(1);
            if narrow { f.blocks[1].insts.push(Inst::Conv(Conv::IntToInt(I8), narrowed, value)); }
            f.blocks[1].insts.extend([
                Inst::ICmpI(Cond::Eq, true, result, if narrow { narrowed } else { value }, 1),
                Inst::IBinI(IOp::Sub, U64, count, count, 1),
                Inst::ICmpI(Cond::Ne, false, again, count, 0),
            ]);
            f.blocks[1].term = Term::Branch(again, 2, 3);
            f.blocks[2].insts = vec![Inst::Iconst(value, 1)];
            f.blocks[2].term = Term::Jump(1);
            f.blocks[3].term = Term::Ret(vec![result]);
            positions(&mut f);
            let nv = f.vregs.len();
            let expected: Vec<_> = (1..=3).map(|n| eval(&f, &[n])).collect();
            simplify_ic(&mut f, true);
            assert!(has_ordering_compare(&f));
            assert!(f.vregs.len() <= nv, "a loop-carried phi must not gain a boolean shadow");
            assert_eq!((1..=3).map(|n| eval(&f, &[n])).collect::<Vec<_>>(), expected);
        }
    }

    #[test]
    fn phi_header_latch_and_bypass_definitions_are_not_folded() {
        for case in 0..3 {
            let mut f = ordering_loop_fixture(false);
            match case {
                0 => f.blocks[1].insts.insert(0, Inst::Iconst(VReg(2), -1)),
                1 => f.blocks[6].insts.push(Inst::Iconst(VReg(2), 1)),
                // Same loop depth, but the definition can return to the header
                // without executing the compare.
                _ => f.blocks[2].term = Term::Branch(VReg(0), 1, 6),
            }
            positions(&mut f);
            let expected: Vec<_> = (1..=3).map(|n| eval(&f, &[0, n])).collect();
            simplify_ic(&mut f, true);
            assert!(has_ordering_compare(&f));
            assert_eq!((1..=3).map(|n| eval(&f, &[0, n])).collect::<Vec<_>>(), expected);
        }
    }

    #[test]
    fn phi_copy_of_a_loop_carried_value_is_not_folded() {
        let mut f = ordering_loop_fixture(false);
        let carried = f.vreg(Cls::I);
        f.blocks[0].insts.push(Inst::Iconst(carried, 255));
        for b in [2, 4, 5] { f.blocks[b].insts[0] = Inst::Mov(VReg(2), carried); }
        f.blocks[6].insts.push(Inst::Iconst(carried, 1));
        positions(&mut f);
        let expected: Vec<_> = (1..=3).map(|n| eval(&f, &[0, n])).collect();
        simplify_ic(&mut f, true);
        assert!(has_ordering_compare(&f));
        assert_eq!((1..=3).map(|n| eval(&f, &[0, n])).collect::<Vec<_>>(), expected);
    }

    #[test]
    fn phi_with_nonconstant_definition_is_untouched() {
        let mut f = phi_fixture(false, Cond::Eq, false, 1);
        f.blocks[2].insts[0] = Inst::IBinI(IOp::Or, U64, VReg(2), VReg(1), 2);
        simplify_ic(&mut f, true);
        assert!(f.blocks.iter().flat_map(|b| &b.insts).any(|i| matches!(i, Inst::ICmpI(..))));
        assert_eq!(eval(&f, &[0, 1]), 0);
    }

    #[test]
    fn phi_copies_and_narrowing_capture_the_old_value() {
        for narrow in [false, true] {
            let mut f = phi_fixture(false, Cond::Eq, true, 1);
            f.blocks[3].insts = vec![
                if narrow { Inst::Conv(Conv::IntToInt(I8), VReg(3), VReg(2)) } else { Inst::Mov(VReg(3), VReg(2)) },
                Inst::Iconst(VReg(2), 1),
                Inst::ICmpI(Cond::Eq, true, VReg(4), VReg(3), 1),
            ];
            if !narrow {
                // Make the copied value a multi-def phi too, with its last definition
                // still reading the original phi before it is overwritten.
                f.blocks[3].insts.insert(0, Inst::Iconst(VReg(3), -1));
            }
            positions(&mut f);
            simplify_ic(&mut f, true);
            assert_eq!(eval(&f, &[0, 0]), 1);
            assert_eq!(eval(&f, &[1, 0]), 0);
            assert!(!f.blocks.iter().flat_map(|b| &b.insts).any(|i| matches!(i, Inst::ICmpI(..))));
        }
    }

    #[test]
    fn phi_copy_depth_is_bounded() {
        let mut f = fixture(0, 6, 1);
        f.blocks[0].insts.push(Inst::Iconst(VReg(0), 1));
        for v in 1..6 { f.blocks[0].insts.push(Inst::Mov(VReg(v), VReg(v - 1))); }
        f.blocks[0].term = Term::Ret(vec![VReg(5)]);
        positions(&mut f);
        let defs: Vec<_> = (0..6).map(|ii| vec![(0, ii)]).collect();
        assert!(const_phi(&f, &defs, &[1; 6], VReg(4), 0));
        assert!(!const_phi(&f, &defs, &[1; 6], VReg(5), 0));
        check(&f);
    }

    fn conv_fixture(types: &[IntTy]) -> Func {
        let mut f = fixture(1, types.len() as u32 + 1, 1);
        for (i, &t) in types.iter().enumerate() {
            f.blocks[0].insts.push(Inst::Conv(Conv::IntToInt(t), VReg(i as u32 + 1), VReg(i as u32)));
        }
        f.blocks[0].term = Term::Ret(vec![VReg(types.len() as u32)]);
        positions(&mut f);
        f
    }

    #[test]
    fn conversion_chains_and_switch() {
        for types in [&[I32, I32, I32][..], &[I32, I8][..], &[I32, U32][..]] {
            for ic in [false, true] {
                let mut f = conv_fixture(types);
                let expected: Vec<_> = [-1, 0, 255, 256, i64::MAX].iter().map(|&x| eval(&f, &[x])).collect();
                simplify_ic(&mut f, ic);
                assert_eq!(f.blocks[0].insts.iter().filter(|i| matches!(i, Inst::Conv(..))).count(), if ic { 1 } else { types.len() });
                assert_eq!([-1, 0, 255, 256, i64::MAX].iter().map(|&x| eval(&f, &[x])).collect::<Vec<_>>(), expected);
            }
        }
    }

    #[test]
    fn widening_conversion_and_reassigned_source_are_preserved() {
        let mut f = conv_fixture(&[I8, I32]);
        simplify_ic(&mut f, true);
        assert_eq!(f.blocks[0].insts.iter().filter(|i| matches!(i, Inst::Conv(..))).count(), 2);
        assert_eq!(eval(&f, &[255]), -1);

        let mut f = conv_fixture(&[I32, I8]);
        f.blocks[0].insts.insert(1, Inst::Iconst(VReg(0), 0));
        positions(&mut f);
        simplify_ic(&mut f, true);
        assert_eq!(f.blocks[0].insts.iter().filter(|i| matches!(i, Inst::Conv(..))).count(), 2);
        assert_eq!(eval(&f, &[258]), 2);
    }

    fn niche_fixture(op: IOp, cc: Cond, mask: i64) -> Func {
        let mut f = fixture(1, 4, 1);
        f.blocks[0].insts = vec![
            Inst::IBinI(op, U64, VReg(1), VReg(0), if op == IOp::Add { -3 } else { 3 }),
            Inst::IBinI(IOp::And, U64, VReg(2), VReg(1), mask),
            Inst::ICmpI(cc, false, VReg(3), VReg(2), 0),
        ];
        f.blocks[0].term = Term::Ret(vec![VReg(3)]);
        positions(&mut f);
        f
    }

    #[test]
    fn niche_compares_and_switch() {
        for op in [IOp::Add, IOp::Sub] {
            for cc in [Cond::Eq, Cond::Ne] {
                for ic in [false, true] {
                    let mut f = niche_fixture(op, cc, 7);
                    simplify_ic(&mut f, ic);
                    assert!(f.blocks[0].insts.iter().any(|i| matches!(i, Inst::ICmpI(c, _, _, _, k) if *c == cc && *k == if ic { 3 } else { 0 })));
                    assert_eq!(f.blocks[0].insts.iter().any(|i| matches!(i, Inst::IBinI(IOp::Add | IOp::Sub, ..))), !ic);
                    for x in -16..16 {
                        assert_eq!(eval(&f, &[x]), (((x - 3) & 7 == 0) == (cc == Cond::Eq)) as i64);
                    }
                }
            }
        }
    }

    #[test]
    fn niche_non_low_mask_and_multiple_definitions_are_preserved() {
        for multi in [false, true] {
            let mut f = niche_fixture(IOp::Add, Cond::Eq, if multi { 7 } else { 5 });
            if multi { f.blocks[0].insts.insert(1, Inst::IBinI(IOp::Add, U64, VReg(1), VReg(0), -5)); }
            positions(&mut f);
            let expected: Vec<_> = (0..8).map(|x| eval(&f, &[x])).collect();
            simplify_ic(&mut f, true);
            assert!(f.blocks[0].insts.iter().any(|i| matches!(i, Inst::IBinI(IOp::Add, ..))));
            assert_eq!((0..8).map(|x| eval(&f, &[x])).collect::<Vec<_>>(), expected);
        }
    }

    #[test]
    fn niche_captures_source_before_reassignment() {
        let mut f = niche_fixture(IOp::Add, Cond::Eq, 7);
        f.blocks[0].insts.insert(1, Inst::Iconst(VReg(0), 0));
        positions(&mut f);
        simplify_ic(&mut f, true);
        assert_eq!(eval(&f, &[3]), 1);
        assert_eq!(eval(&f, &[4]), 0);
    }

    #[test]
    fn niche_signed_mask_and_truncating_subtraction() {
        for mask in [255, 511] {
            let mut f = niche_fixture(IOp::Add, Cond::Eq, mask);
            f.blocks[0].insts[0] = Inst::IBinI(IOp::Add, I8, VReg(1), VReg(0), -255);
            f.blocks[0].insts[1] = Inst::IBinI(IOp::And, if mask == 255 { I8 } else { U64 }, VReg(2), VReg(1), mask);
            let expected: Vec<_> = [-1, 0, 1, 255, 256, 511].iter().map(|&x| eval(&f, &[x])).collect();
            simplify_ic(&mut f, true);
            assert_eq!([-1, 0, 1, 255, 256, 511].iter().map(|&x| eval(&f, &[x])).collect::<Vec<_>>(), expected);
            assert_eq!(f.blocks[0].insts.iter().any(|i| matches!(i, Inst::IBinI(IOp::Add, ..))), mask == 511);
        }
    }

    fn sink_fixture(load: bool) -> Func {
        let mut f = fixture(3, 4, 3);
        f.blocks[0].insts = vec![if load { Inst::Load(Mem::Int(8, false), VReg(3), VReg(1), 8) } else { Inst::IBinI(IOp::Add, U64, VReg(3), VReg(2), 4) }];
        f.blocks[0].term = Term::Branch(VReg(0), 1, 2);
        f.blocks[1].term = Term::Ret(vec![VReg(3)]);
        f.blocks[2].term = Term::Ret(vec![VReg(2)]);
        positions(&mut f);
        f
    }

    #[test]
    fn sinks_loads_and_alu_to_the_using_successor() {
        for load in [false, true] {
            let mut f = sink_fixture(load);
            let inst = f.blocks[0].insts[0].clone();
            assert!(sink_to_successor(&mut f));
            assert!(f.blocks[0].insts.is_empty());
            assert_eq!(f.blocks[1].insts, vec![inst]);
            assert!(f.blocks[2].insts.is_empty());
            check(&f);
        }
    }

    #[test]
    fn sinking_leaves_copies_in_place() {
        // an iterator `next`'s element copy before its guard: once inlined into a loop the
        // guard block must keep it (or threading rotates the loop away from loops.rs's shape)
        let mut f = sink_fixture(false);
        f.blocks[0].insts = vec![Inst::Mov(VReg(3), VReg(2))];
        positions(&mut f);
        assert!(!sink_to_successor(&mut f));
        assert_eq!(f.blocks[0].insts, vec![Inst::Mov(VReg(3), VReg(2))]);
        check(&f);
    }

    #[test]
    fn sinking_leaves_loop_header_loads_and_alu_in_place() {
        for load in [false, true] {
            let mut f = fixture(3, 4, 4);
            f.blocks[0].term = Term::Jump(1);
            f.blocks[1].insts = sink_fixture(load).blocks[0].insts.clone();
            f.blocks[1].term = Term::Branch(VReg(0), 2, 3);
            f.blocks[2].insts = vec![Inst::Store(Mem::Int(8, false), VReg(1), 0, VReg(3))];
            f.blocks[2].term = Term::Jump(1);
            f.blocks[3].term = Term::Ret(vec![VReg(2)]);
            positions(&mut f);
            let before = f.dump();
            assert!(!sink_to_successor(&mut f));
            assert_eq!(f.dump(), before);
            check(&f);
        }
    }

    #[test]
    fn sinking_does_not_cross_loop_depths() {
        for nested in [false, true] {
            let mut f = fixture(3, 4, 6);
            f.blocks[0].term = Term::Jump(1);
            f.blocks[1].term = Term::Branch(VReg(0), 2, 5);
            f.blocks[2].term = Term::Jump(3);
            f.blocks[3].insts = vec![Inst::Load(Mem::Int(8, false), VReg(3), VReg(1), 0)];
            f.blocks[3].term = Term::Branch(VReg(0), if nested { 2 } else { 1 }, 4);
            f.blocks[4].insts = vec![Inst::Store(Mem::Int(8, false), VReg(1), 0, VReg(3))];
            f.blocks[4].term = if nested { Term::Jump(1) } else { Term::Ret(vec![VReg(2)]) };
            f.blocks[5].term = Term::Ret(vec![VReg(2)]);
            positions(&mut f);
            let depth = crate::regalloc::loop_depths(&f, &reachable(&f));
            assert_eq!((depth[3], depth[4]), if nested { (2, 1) } else { (1, 0) });
            let before = f.dump();
            assert!(!sink_to_successor(&mut f));
            assert_eq!(f.dump(), before);
            check(&f);
        }
    }

    #[test]
    fn byte_add_loop_still_vectorizes_after_sinking() {
        let mut f = fixture(4, 8, 5);
        let [a, b, out, count, x, y, sum, more] = std::array::from_fn::<_, 8, _>(|i| VReg(i as u32));
        f.blocks[0].term = Term::Jump(1);
        f.blocks[1].insts = vec![Inst::Load(Mem::Int(1, false), x, a, 0), Inst::ICmpI(Cond::Ne, false, more, count, 0)];
        f.blocks[1].term = Term::Branch(more, 2, 4);
        f.blocks[1].term_pos = 123;
        f.blocks[2].insts = vec![Inst::Load(Mem::Int(1, false), y, b, 0)];
        f.blocks[2].term = Term::Jump(3);
        f.blocks[3].insts = vec![
            Inst::IBin(IOp::Add, IntTy { bits: 8, signed: false }, sum, x, y),
            Inst::Store(Mem::Int(1, false), out, 0, sum),
            Inst::IBinI(IOp::Add, U64, a, a, 1),
            Inst::IBinI(IOp::Add, U64, b, b, 1),
            Inst::IBinI(IOp::Add, U64, out, out, 1),
            Inst::IBinI(IOp::Sub, U64, count, count, 1),
        ];
        f.blocks[3].term = Term::Jump(1);
        f.blocks[4].term = Term::Ret(vec![count]);
        positions(&mut f);
        let header = f.blocks[1].insts.clone();
        assert!(sink_to_successor(&mut f));
        assert_eq!(f.blocks[1].insts, header);
        assert!(f.blocks[2].insts.is_empty()); // Same-depth body-to-body sinking remains enabled.
        assert!(crate::loops::vectorize_reductions(&relax_session::Options::default(), &mut f));
        let wide = &f.blocks.iter().find(|b| b.insts.iter().any(|i| matches!(i, Inst::VX(VX::Add(VW::B), ..)))).unwrap().insts;
        assert_eq!(wide.iter().filter(|i| matches!(i, Inst::VLoad(..))).count(), 2);
        assert_eq!(wide.iter().filter(|i| matches!(i, Inst::VStore(..))).count(), 1);
        check(&f);
    }

    #[test]
    fn store_call_copy_and_atomics_block_load_sinking() {
        for effect in [
            Inst::Store(Mem::Int(8, false), VReg(1), 8, VReg(2)),
            Inst::Call(Callee::Fn(0), vec![], vec![]),
            Inst::Copy(VReg(1), VReg(2), 8),
            Inst::AtomicLoad(Mem::Int(8, false), AtomOrd::Acquire, VReg(4), VReg(1)),
            Inst::AtomicStore(Mem::Int(8, false), AtomOrd::SeqCst, VReg(1), VReg(2)),
            Inst::Fence(AtomOrd::SeqCst, false),
        ] {
            let mut f = sink_fixture(true);
            f.vreg(Cls::I);
            f.blocks[0].insts.push(effect);
            positions(&mut f);
            let before = f.dump();
            assert!(!sink_to_successor(&mut f));
            assert_eq!(f.dump(), before);
            check(&f);
        }
    }

    #[test]
    fn extra_uses_predecessors_and_redefinitions_block_sinking() {
        for case in 0..4 {
            let mut f = sink_fixture(true);
            match case {
                0 => f.blocks[2].term = Term::Ret(vec![VReg(3)]),
                1 => f.blocks[2].term = Term::Jump(1),
                2 => f.blocks[0].insts.push(Inst::Mov(VReg(1), VReg(2))),
                _ => f.blocks[1].insts.push(Inst::Iconst(VReg(3), 1)),
            }
            positions(&mut f);
            let before = f.dump();
            assert!(!sink_to_successor(&mut f));
            assert_eq!(f.dump(), before);
            check(&f);
        }
    }

    #[test]
    fn sinking_does_not_cascade_within_one_pass() {
        let mut f = fixture(2, 4, 4);
        f.blocks[0].insts = vec![Inst::IBinI(IOp::Add, U64, VReg(2), VReg(1), 4)];
        f.blocks[0].term = Term::Branch(VReg(0), 1, 3);
        f.blocks[1].insts = vec![Inst::IBinI(IOp::Add, U64, VReg(3), VReg(2), 8)];
        f.blocks[1].term = Term::Jump(2);
        f.blocks[2].term = Term::Ret(vec![VReg(3)]);
        f.blocks[3].term = Term::Ret(vec![VReg(1)]);
        positions(&mut f);
        assert!(sink_to_successor(&mut f));
        assert_eq!(f.blocks[1].insts, vec![Inst::IBinI(IOp::Add, U64, VReg(2), VReg(1), 4)]);
        assert_eq!(f.blocks[2].insts, vec![Inst::IBinI(IOp::Add, U64, VReg(3), VReg(2), 8)]);
        assert_eq!(eval(&f, &[1, 7]), 19);
        check(&f);
    }

    #[test]
    fn sinking_switch() {
        for ic in [false, true] {
            let mut f = sink_fixture(true);
            simplify_ic(&mut f, ic);
            assert_eq!(f.blocks[0].insts.iter().any(|i| matches!(i, Inst::Load(..))), !ic);
            assert_eq!(f.blocks[1].insts.iter().any(|i| matches!(i, Inst::Load(..))), ic);
        }
    }
}

#[cfg(test)]
mod narrow_store_tests {
    use super::*;

    fn fixture(m: Mem, loads: bool) -> (Func, VReg, VReg, VReg) {
        let mut f = Func::new("narrow_stores".into(), 0);
        let p = f.vreg(Cls::I);
        f.params.push(p);
        let b = f.vreg(Cls::I);
        let d = f.vreg(match m { Mem::F32 => Cls::F32, Mem::F64 => Cls::F64, _ => Cls::I });
        f.rets.push(f.vregs[d.0 as usize]);
        let sl = f.slot(16, 8);
        f.block();
        f.blocks[0].insts.push(Inst::SlotAddr(b, sl));
        let n = mem_size(m);
        let mut values = Vec::new();
        for i in 0..n {
            let v = f.vreg(Cls::I);
            values.push(v);
            f.blocks[0].insts.push(if loads { Inst::Load(Mem::Int(1, false), v, p, 12 + i) } else { Inst::Iconst(v, -1 - i as i64) });
        }
        for i in (0..n).rev() {
            f.blocks[0].insts.push(Inst::Store(Mem::Int(1, false), b, 2 + i, values[i as usize]));
        }
        f.blocks[0].insts.push(Inst::Load(m, d, b, 2));
        f.blocks[0].pos = vec![123; f.blocks[0].insts.len()];
        f.blocks[0].term = Term::Ret(vec![d]);
        (f, p, b, d)
    }

    fn check(f: &Func) {
        f.verify().unwrap();
        for b in &f.blocks {
            assert_eq!(b.insts.len(), b.pos.len());
            assert!(b.pos.iter().all(|&p| p == 123));
        }
    }

    #[test]
    fn forwards_byte_loads() {
        for m in [Mem::Int(2, true), Mem::Int(4, false), Mem::Int(8, false), Mem::F32, Mem::F64] {
            let (mut f, p, _, d) = fixture(m, true);
            assert!(forward_narrow_stores(&mut f));
            assert!(matches!(f.blocks[0].insts.last(), Some(Inst::Load(mm, dd, pp, 12)) if (*mm, *dd, *pp) == (m, d, p)));
            assert_eq!(f.blocks[0].insts.iter().filter(|i| matches!(i, Inst::Store(..))).count(), mem_size(m) as usize);
            check(&f);
        }
    }

    #[test]
    fn call_blocks_forwarding() {
        let (mut f, _, _, _) = fixture(Mem::Int(4, false), true);
        let at = f.blocks[0].insts.len() - 1;
        f.blocks[0].insts.insert(at, Inst::Call(Callee::Fn(0), vec![], vec![]));
        f.blocks[0].pos.insert(at, 123);
        let before = f.dump();
        assert!(!forward_narrow_stores(&mut f));
        assert_eq!(f.dump(), before);
        check(&f);
    }

    #[test]
    fn assembles_values_and_extends_result() {
        for m in [Mem::Int(2, true), Mem::Int(4, false), Mem::Int(8, true), Mem::F32, Mem::F64] {
            let (mut f, _, _, d) = fixture(m, false);
            assert!(forward_narrow_stores(&mut f));
            let mut known = vec![None; f.vregs.len()];
            for inst in &f.blocks[0].insts {
                match *inst {
                    Inst::Iconst(v, x) => known[v.0 as usize] = Some(K::I(x)),
                    Inst::Conv(c, v, a) => known[v.0 as usize] = fold_conv(c, known[a.0 as usize].unwrap()),
                    Inst::IBinI(IOp::Shl, t, v, a, k) => {
                        let Some(K::I(x)) = known[a.0 as usize] else { panic!() };
                        known[v.0 as usize] = Some(K::I(extend_const(x << k, t)));
                    }
                    Inst::IBin(IOp::Or, t, v, a, b) => {
                        let (Some(K::I(x)), Some(K::I(y))) = (known[a.0 as usize], known[b.0 as usize]) else { panic!() };
                        known[v.0 as usize] = Some(K::I(extend_const(x | y, t)));
                    }
                    _ => {}
                }
            }
            let bits = (0..mem_size(m)).fold(0u64, |v, i| v | ((255 - i as u64) << (8 * i))) as i64;
            match (m, known[d.0 as usize].unwrap()) {
                (Mem::Int(n, signed), K::I(x)) => assert_eq!(x, extend_const(bits, IntTy { bits: n * 8, signed })),
                (Mem::F32, K::F(x, false)) => assert_eq!(x, bits as u32 as u64),
                (Mem::F64, K::F(x, true)) => assert_eq!(x, bits as u64),
                _ => panic!("wrong result kind"),
            }
            assert_eq!(f.blocks[0].insts.iter().filter(|i| matches!(i, Inst::IBin(IOp::Or, ..))).count(), mem_size(m) as usize - 1);
            check(&f);
        }
    }

    #[test]
    fn forwards_from_single_predecessor_chain() {
        let (mut f, p, _, d) = fixture(Mem::Int(4, false), true);
        let tail = f.blocks[0].insts.split_off(5);
        f.blocks[0].pos.truncate(5);
        f.block();
        f.blocks[1].pos = vec![123; tail.len()];
        f.blocks[1].insts = tail;
        f.blocks[1].term = Term::Ret(vec![d]);
        f.blocks[0].term = Term::Branch(p, 1, 1);
        assert!(forward_narrow_stores(&mut f));
        assert!(matches!(f.blocks[1].insts.last(), Some(Inst::Load(Mem::Int(4, false), _, pp, 12)) if *pp == p));
        check(&f);
    }

    #[test]
    fn same_slot_and_outside_store_use_assembly() {
        for same_slot in [false, true] {
            let (mut f, p, b, _) = fixture(Mem::Int(4, false), true);
            if same_slot {
                f.params.clear();
                f.blocks[0].insts.insert(1, Inst::IBinI(IOp::Add, U64, p, b, 10));
                f.blocks[0].pos.insert(1, 123);
            } else {
                let at = f.blocks[0].insts.len() - 1;
                f.blocks[0].insts.insert(at, Inst::Store(Mem::Int(4, false), b, 8, p));
                f.blocks[0].pos.insert(at, 123);
            }
            assert!(forward_narrow_stores(&mut f));
            assert!(matches!(f.blocks[0].insts.last(), Some(Inst::Conv(..))));
            check(&f);
        }
    }

    #[test]
    fn redefined_value_or_base_blocks_forwarding() {
        for redefine_base in [false, true] {
            let (mut f, _, b, _) = fixture(Mem::Int(4, false), false);
            let v = if redefine_base { b } else {
                let Inst::Store(_, _, _, v) = f.blocks[0].insts[5] else { panic!() };
                v
            };
            let at = f.blocks[0].insts.len() - 1;
            f.blocks[0].insts.insert(at, Inst::Iconst(v, 0));
            f.blocks[0].pos.insert(at, 123);
            assert!(!forward_narrow_stores(&mut f));
            check(&f);
        }
    }
}

#[cfg(test)]
mod inline_priority_tests {
    use super::*;

    fn callee(size: usize) -> Func {
        let mut f = Func::new("callee".into(), 0);
        let v = f.vreg(Cls::I);
        f.block();
        // Real work: the cost model deliberately does not charge constants.
        f.blocks[0].insts = vec![Inst::IBinI(IOp::Add, I64, v, v, 1); size];
        if size != 0 { f.blocks[0].insts[0] = Inst::Iconst(v, 1); }
        f.blocks[0].pos = vec![0; size];
        f.blocks[0].term = Term::Ret(vec![]);
        f
    }

    fn caller() -> Func {
        let mut f = Func::new("caller".into(), 0);
        let cond = f.vreg(Cls::I);
        f.params.push(cond);
        for _ in 0..3 { f.block(); }
        f.blocks[0].insts = vec![Inst::Call(Callee::Fn(1), vec![], vec![])];
        f.blocks[0].pos = vec![0];
        f.blocks[0].term = Term::Jump(1);
        f.blocks[1].insts = vec![Inst::Call(Callee::Fn(2), vec![], vec![])];
        f.blocks[1].pos = vec![0];
        f.blocks[1].term = Term::Branch(cond, 1, 2);
        f.blocks[2].term = Term::Ret(vec![]);
        f
    }

    fn one_call(id: u32) -> Func {
        let mut f = callee(0);
        f.blocks[0].insts.push(Inst::Call(Callee::Fn(id), vec![], vec![]));
        f.blocks[0].pos.push(0);
        f
    }

    fn cheap_callee(size: usize) -> Func {
        let mut f = callee(size);
        f.blocks[0].insts.fill(Inst::Iconst(VReg(0), 1));
        f
    }

    #[test]
    fn hot_loop_wins_and_tier0_keeps_program_order() {
        let options = &relax_session::Options::default();
        let c = callee(30);
        let get = |_: u32| Some((&c as *const Func, 0));
        let mut lim = InlineLimits::tier1();
        lim.budget = 30;
        let mut f = caller();
        assert_eq!(inline_calls(options, &mut f, 0, &get, &|_| false, &lim), vec![(2, RULE_SMALL_LEAF)]);
        f.verify().unwrap();
        let mut lim = InlineLimits::tier0(options);
        lim.budget = 30;
        let mut f = caller();
        assert_eq!(inline_calls(options, &mut f, 0, &get, &|_| false, &lim), vec![(1, RULE_SMALL_LEAF)]);
        f.verify().unwrap();
    }

    #[test]
    fn net_cost_and_frequency_priority() {
        let options = &relax_session::Options::default();
        let c = callee(30);
        let get = |_: u32| Some((&c as *const Func, 0));
        let mut lim = InlineLimits::tier1();
        lim.budget = 27;
        let mut f = caller();
        assert_eq!(inline_calls_ordered(options, &mut f, 0, &get, &|_, _| false, &|_| false, &lim, Some(&[1.0, 0.5, 0.0]), None), vec![(1, RULE_SMALL_LEAF)]);
        lim.budget = 54;
        let mut f = caller();
        assert_eq!(inline_calls(options, &mut f, 0, &get, &|_| false, &lim).len(), 2);
        f.verify().unwrap();
    }

    #[test]
    fn hot_nonleaf_and_exposed_calls() {
        let options = &relax_session::Options::default();
        let mut c = callee(30);
        c.blocks[0].insts[29] = Inst::Call(Callee::Fn(3), vec![], vec![]);
        let leaf = callee(25);
        let get = |id: u32| Some((if id == 3 { &leaf } else { &c } as *const Func, 0));
        let mut lim = InlineLimits::tier1();
        lim.budget = 49; // 30 - 3 + 25 - 3
        let mut f = caller();
        assert_eq!(inline_calls(options, &mut f, 0, &get, &|_| false, &lim), vec![(2, RULE_SMALL_LEAF), (3, RULE_SMALL_LEAF)]);
        f.verify().unwrap();
    }

    #[test]
    fn recursive_panic_and_arity_stay_calls() {
        let options = &relax_session::Options::default();
        let mut c = callee(30);
        c.blocks[0].insts[0] = Inst::Call(Callee::Fn(2), vec![], vec![]);
        let mut f = caller();
        let get = |id: u32| (id == 2).then_some((&c as *const Func, ATTR_ALWAYS));
        assert!(inline_calls(options, &mut f, 0, &get, &|_| false, &InlineLimits::tier1()).is_empty());
        c.blocks[0].insts[0] = Inst::Iconst(VReg(0), 1);
        c.blocks[0].term = Term::Unreachable;
        let get = |_: u32| Some((&c as *const Func, ATTR_ALWAYS));
        assert!(inline_calls(options, &mut f, 0, &get, &|_| false, &InlineLimits::tier1()).is_empty());
        c.blocks[0].term = Term::Ret(vec![]);
        c.params.push(VReg(0));
        let get = |_: u32| Some((&c as *const Func, ATTR_ALWAYS));
        let mut stats = InlineStats::default();
        assert!(inline_calls_ordered(options, &mut f, 0, &get, &|_, _| false, &|_| false, &InlineLimits::tier1(), None, Some(&mut stats)).is_empty());
        assert_eq!(stats.left, [0, 0, 0, 0, 2]);
    }

    #[test]
    fn smaller_callee_wins_at_equal_priority_and_stats_count_tail_once() {
        let options = &relax_session::Options::default();
        let big = callee(40);
        let small = callee(30);
        let get = |id: u32| Some((if id == 1 { &big } else { &small } as *const Func, 0));
        let mut f = caller();
        f.blocks[0].insts.push(Inst::Call(Callee::Fn(2), vec![], vec![]));
        f.blocks[0].pos.push(0);
        f.blocks[1].insts.clear();
        f.blocks[1].pos.clear();
        let mut lim = InlineLimits::tier1();
        lim.budget = 27;
        let mut stats = InlineStats::default();
        assert_eq!(inline_calls_ordered(options, &mut f, 0, &get, &|_, _| false, &|_| false, &lim, None, Some(&mut stats)), vec![(2, RULE_SMALL_LEAF)]);
        assert_eq!(stats.inlined, 1);
        assert_eq!(stats.left, [0, 0, 0, 1, 0]);
        f.verify().unwrap();
    }

    #[test]
    fn call_arguments_and_returns_reduce_cost() {
        let options = &relax_session::Options::default();
        let mut c = callee(30);
        c.params.push(VReg(0));
        c.rets.push(Cls::I);
        c.blocks[0].term = Term::Ret(vec![VReg(0)]);
        let mut f = caller();
        f.blocks[0].insts.clear();
        f.blocks[0].pos.clear();
        let result = f.vreg(Cls::I);
        f.blocks[1].insts[0] = Inst::Call(Callee::Fn(2), vec![VReg(0)], vec![result]);
        let get = |_: u32| Some((&c as *const Func, 0));
        let mut lim = InlineLimits::tier1();
        lim.budget = 25; // 30 - (one argument + one return + 3)
        assert_eq!(inline_calls(options, &mut f, 0, &get, &|_| false, &lim).len(), 1);
        f.verify().unwrap();
    }

    #[test]
    fn only_sampled_hot_block_gets_hot_rule_in_all_loop_function() {
        let options = &relax_session::Options::default();
        let mut c = callee(30);
        c.blocks[0].insts[29] = Inst::Call(Callee::Fn(99), vec![], vec![]);
        let get = |id: u32| (id != 99).then_some((&c as *const Func, 0));
        let mut f = caller();
        f.blocks[1].term = Term::Jump(2);
        f.blocks[2].insts = vec![Inst::Call(Callee::Fn(3), vec![], vec![])];
        f.blocks[2].pos = vec![0];
        f.blocks[2].term = Term::Jump(0);
        assert!(crate::regalloc::loop_depths(&f, &reachable(&f)).iter().all(|d| *d >= 1));
        let mut stats = InlineStats::default();
        assert_eq!(inline_calls_ordered(options, &mut f, 0, &get, &|_, _| false, &|_| false, &InlineLimits::tier1(), Some(&[0.01, 1.0, 0.0]), Some(&mut stats)), vec![(2, RULE_SMALL_LEAF)]);
        assert_eq!(stats.hot_inlined, 1);
        assert_eq!(stats.hot_left, 1); // the exposed call inherits hotness
        assert_eq!(stats.growth_before, Some(3));
        assert_eq!(stats.growth_after, inline_inst_count(&f));
        f.verify().unwrap();
    }

    #[test]
    fn hot_threshold_and_small_loop_fallback() {
        let options = &relax_session::Options::default();
        let mut c = callee(30);
        c.blocks[0].insts[29] = Inst::Call(Callee::Fn(99), vec![], vec![]);
        let get = |id: u32| (id != 99).then_some((&c as *const Func, 0));
        for (freq, expected) in [(0.049, 0), (HOT_FREQ, 1)] {
            let mut f = caller();
            assert_eq!(inline_calls_ordered(options, &mut f, 0, &get, &|_, _| false, &|_| false, &InlineLimits::tier1(), Some(&[0.01, freq, 1.0]), None).len(), expected);
        }
        for (blocks, expected) in [(64, 1), (65, 0)] {
            let mut f = caller();
            while f.blocks.len() < blocks { f.block(); }
            assert_eq!(inline_calls_ordered(options, &mut f, 0, &get, &|_, _| false, &|_| false, &InlineLimits::tier1(), None, None).len(), expected);
        }
    }

    #[test]
    fn unsampled_sites_only_allow_free_or_always() {
        let options = &relax_session::Options::default();
        for (size, attrs, expected) in [(30, 0, 0), (30, ATTR_HINT, 0), (30, ATTR_CHAIN, 0), (30, ATTR_ALWAYS, 2), (INLINE_FREE_INSTS, 0, 2)] {
            let c = callee(size);
            let get = |_: u32| Some((&c as *const Func, attrs));
            let mut f = caller();
            assert_eq!(inline_calls_ordered(options, &mut f, 0, &get, &|_, _| false, &|_| false, &InlineLimits::tier1(), Some(&[0.0, 0.0, 1.0]), None).len(), expected);
            f.verify().unwrap();
        }
        // inline(always) still consumes the paid allowance.
        let c = callee(30);
        let get = |_: u32| Some((&c as *const Func, ATTR_ALWAYS));
        let mut lim = InlineLimits::tier1();
        lim.budget = 27;
        assert_eq!(inline_calls_ordered(options, &mut caller(), 0, &get, &|_, _| false, &|_| false, &lim, Some(&[0.0, 0.0, 1.0]), None).len(), 1);
    }

    #[test]
    fn actual_growth_stops_paid_inlining_before_net_budget_runs_out() {
        let options = &relax_session::Options::default();
        let c = callee(25);
        let get = |_: u32| Some((&c as *const Func, 0));
        let mut f = callee(0);
        f.blocks[0].insts = vec![Inst::Call(Callee::Fn(1), vec![], vec![]); 200];
        f.blocks[0].pos = vec![0; 200];
        let lim = InlineLimits::tier1_for(&f);
        let mut stats = InlineStats::default();
        let inlined = inline_calls_ordered(options, &mut f, 0, &get, &|_, _| false, &|_| false, &lim, None, Some(&mut stats));
        // Each inline grows by 24 instructions but costs only 22 budget units.
        assert_eq!(inlined.len(), 167);
        assert_eq!(inline_inst_count(&f), 4208);
        assert_eq!(stats.growth_after, 4208);
        assert_eq!(stats.left[3], 33);
        f.verify().unwrap();
    }

    #[test]
    fn growth_cap_survives_rounds_and_still_allows_free_sites() {
        let options = &relax_session::Options::default();
        let paid = callee(30);
        let free = callee(INLINE_FREE_INSTS);
        let get = |id: u32| Some((if id == 3 { &free } else { &paid } as *const Func, 0));
        let mut f = caller();
        let lim = InlineLimits::tier1_for(&f);
        assert_eq!(lim.growth_limit, Some(4002));
        // Model growth from the first round; the original ceiling must still apply.
        let v = f.vreg(Cls::I);
        f.blocks[0].insts.extend(vec![Inst::Iconst(v, 1); 4001]);
        f.blocks[0].pos.extend(vec![0; 4001]);
        f.blocks[0].insts.push(Inst::Call(Callee::Fn(3), vec![], vec![]));
        f.blocks[0].pos.push(0);
        let mut stats = InlineStats::default();
        assert_eq!(inline_calls_ordered(options, &mut f, 0, &get, &|_, _| false, &|_| false, &lim, None, Some(&mut stats)).len(), 1);
        assert_eq!(stats.left[3], 2);
        assert_eq!(stats.growth_after, inline_inst_count(&f));
        assert!(inline_calls_ordered(options, &mut f, 0, &get, &|_, _| false, &|_| false, &lim, None, Some(&mut stats)).is_empty());
        f.verify().unwrap();
    }

    #[test]
    fn budget_does_not_scale_with_caller() {
        assert_eq!(InlineLimits::tier1_for(&callee(100)).budget, 4000);
        assert_eq!(InlineLimits::tier1_for(&callee(16000)).budget, 4000);
        assert_eq!(InlineLimits::tier1_for(&callee(30000)).budget, 4000);
        assert_eq!(InlineLimits::tier1_for(&callee(16000)).growth_limit, Some(32000));
    }

    #[test]
    fn rustc_cost_small_body_ignores_operands_and_unreachable_blocks() {
        let mut f = callee(3); // two arithmetic instructions
        let v = VReg(0);
        let fp = f.vreg(Cls::F64);
        let slot = f.slot(8, 8);
        f.blocks[0].insts.extend([
            Inst::Mov(v, v), Inst::Fconst(fp, 0, true), Inst::SlotAddr(v, slot),
            Inst::Addr(v, Sym { id: 0, off: 0 }), Inst::FnAddr(v, 9),
        ]);
        f.block();
        f.blocks[1].insts = vec![Inst::IBinI(IOp::Add, I64, v, v, 1); 100];
        f.blocks[1].term = Term::Resume; // dead: neither the instructions nor Resume cost
        assert_eq!(rustc_inline_cost(&f, 1), 10);
        assert_eq!(rustc_inline_threshold(&f, 0), 62);
        assert_eq!(rustc_inline_threshold(&f, ATTR_HINT), 125);
        assert_eq!(rustc_inline_threshold(&f, ATTR_ALWAYS), 125);
        f.block();
        f.block();
        assert_eq!(rustc_inline_threshold(&f, 0), 50);
        assert_eq!(rustc_inline_threshold(&f, ATTR_HINT), 100);
    }

    #[test]
    fn rustc_cost_single_call_bonus_requires_exactly_one_call() {
        let mut f = callee(2); // 5 for arithmetic
        f.blocks[0].insts.push(Inst::Call(Callee::Fn(9), vec![], vec![]));
        assert_eq!(rustc_inline_cost(&f, 1), 5); // 5 + 25 - 25
        f.blocks[0].insts.push(Inst::Call(Callee::Indirect(VReg(0)), vec![], vec![]));
        assert_eq!(rustc_inline_cost(&f, 1), 55); // 5 + 25 + 25; no bonus
    }

    #[test]
    fn rustc_cost_copy_cleanup_branch_and_unreachable() {
        let mut f = callee(1);
        let v = VReg(0);
        f.blocks[0].insts.push(Inst::Copy(v, v, INLINE_COPY_MAX as u32));
        assert_eq!(rustc_inline_cost(&f, 1), 5);
        f.blocks[0].insts.push(Inst::Copy(v, v, INLINE_COPY_MAX as u32 + 1));
        assert_eq!(rustc_inline_cost(&f, 1), 5); // one call-like copy gets the bonus
        f.blocks[0].insts.push(Inst::Call(Callee::Host(Sym { id: 0, off: 0 }), vec![], vec![]));
        f.block();
        f.block();
        f.blocks[0].term = Term::Invoke(1, 2);
        f.blocks[1].term = Term::Ret(vec![]);
        f.blocks[2].term = Term::Resume;
        assert_eq!(rustc_inline_cost(&f, 1), 150); // 5 + 25 + 25 + 50 + 45
        f.blocks[0].term = Term::Branch(v, 1, 2);
        f.blocks[2].term = Term::Unreachable;
        assert_eq!(rustc_inline_cost(&f, 1), 55); // 5 + 25 + 25 + 5 - 5
        f.blocks[0].insts.clear();
        assert_eq!(rustc_inline_cost(&f, 1), 0); // bonuses saturate at zero
    }

    #[test]
    fn unsampled_site_inlines_cost_model_callee_even_when_never_run() {
        let options = &relax_session::Options::default();
        let c = cheap_callee(30);
        for attrs in [0, ATTR_COLD] {
            let get = |id| (id == 1).then_some((&c as *const Func, attrs));
            let mut f = one_call(1);
            let mut lim = InlineLimits::tier1();
            lim.budget = 0;
            let mut stats = InlineStats::default();
            assert_eq!(inline_calls_ordered(options, &mut f, 0, &get, &|_, _| false, &|_| false, &lim, Some(&[0.0]), Some(&mut stats)), vec![(1, RULE_COST)]);
            assert_eq!(stats.rules[RULE_COST], 1);
            f.verify().unwrap();
        }
    }

    #[test]
    fn single_caller_500_insts_inlines_but_two_refs_do_not() {
        let options = &{ let mut o = relax_session::Options::default(); o.opt.push("+single".into()); o };
        let c = callee(500);
        let get = |id| (id == 1).then_some((&c as *const Func, 0));
        for refs in [1, 2] {
            let single = |owner, id| refs == 1 && owner == 0 && id == 1;
            let mut f = one_call(1);
            let mut lim = InlineLimits::tier1();
            lim.budget = 0;
            let mut stats = InlineStats::default();
            let result = inline_calls_ordered(options, &mut f, 0, &get, &single, &|_| false, &lim, Some(&[0.0]), Some(&mut stats));
            assert_eq!(result, if refs == 1 { vec![(1, RULE_SINGLE)] } else { vec![] });
            assert_eq!(stats.rules[RULE_SINGLE], usize::from(refs == 1));
            f.verify().unwrap();
        }
    }

    #[test]
    fn cold_single_caller_stays_a_call() {
        let options = &{ let mut o = relax_session::Options::default(); o.opt.push("+single".into()); o };
        let c = callee(500);
        let get = |id| (id == 1).then_some((&c as *const Func, ATTR_COLD));
        let mut f = one_call(1);
        assert!(inline_calls_ordered(options, &mut f, 0, &get, &|_, _| true, &|_| false, &InlineLimits::tier1(), Some(&[1.0]), None).is_empty());
        assert!(matches!(f.blocks[0].insts[0], Inst::Call(Callee::Fn(1), _, _)));
    }

    #[test]
    fn cost_model_and_single_keep_calls_on_cold_paths() {
        let options = &{ let mut o = relax_session::Options::default(); o.opt.push("+single".into()); o };
        let c = cheap_callee(30);
        let get = |id| (id == 1).then_some((&c as *const Func, 0));
        for term in [Term::Unreachable, Term::Resume] {
            let mut f = one_call(1);
            f.blocks[0].term = term;
            assert!(inline_calls_ordered(options, &mut f, 0, &get, &|_, _| true, &|_| false, &InlineLimits::tier1(), None, None).is_empty());
        }
    }

    #[test]
    fn cost_model_and_single_switches_disable_their_rules() {
        // -costmodel turns the cost model off; the single-caller rule is off unless +single
        for (toggle, c) in [("-costmodel", cheap_callee(30)), ("", callee(500))] {
            let mut options = relax_session::Options::default();
            options.opt.push(toggle.into());
            let get = |id| (id == 1).then_some((&c as *const Func, 0));
            let mut f = one_call(1);
            assert!(inline_calls_ordered(&options, &mut f, 0, &get, &|_, _| toggle.is_empty(), &|_| false, &InlineLimits::tier1(), Some(&[0.0]), None).is_empty());
        }
    }

    #[test]
    fn cost_model_free_allowance_and_growth_limit() {
        let options = &relax_session::Options::default();
        let c = cheap_callee(1000);
        let get = |id| (id == 1).then_some((&c as *const Func, 0));
        let mut f = one_call(1);
        f.blocks[0].insts = vec![f.blocks[0].insts[0].clone(); 5];
        f.blocks[0].pos = vec![0; 5];
        let mut lim = InlineLimits::tier1();
        lim.budget = 0;
        lim.growth_limit = Some(10000);
        assert_eq!(inline_calls_ordered(options, &mut f, 0, &get, &|_, _| false, &|_| false, &lim, None, None), vec![(1, RULE_COST); 4]);
        f.verify().unwrap();
        lim.growth_limit = Some(999);
        assert!(inline_calls_ordered(options, &mut one_call(1), 0, &get, &|_, _| false, &|_| false, &lim, None, None).is_empty());
    }

    #[test]
    fn single_caller_respects_size_and_growth_caps() {
        let options = &{ let mut o = relax_session::Options::default(); o.opt.push("+single".into()); o };
        for (size, ceiling, expected) in [(3000, 3000, 1), (3001, 4001, 0), (500, 499, 0)] {
            let c = callee(size);
            let get = |id| (id == 1).then_some((&c as *const Func, 0));
            let mut lim = InlineLimits::tier1();
            lim.budget = 0;
            lim.growth_limit = Some(ceiling);
            assert_eq!(inline_calls_ordered(options, &mut one_call(1), 0, &get, &|_, _| true, &|_| false, &lim, None, None).len(), expected);
        }
    }

    #[test]
    fn single_caller_ignores_paid_and_free_budgets() {
        let options = &{ let mut o = relax_session::Options::default(); o.opt.push("+single".into()); o };
        let c = callee(500);
        let get = |_: u32| Some((&c as *const Func, 0));
        let mut f = callee(0);
        f.blocks[0].insts = (1..=9).map(|id| Inst::Call(Callee::Fn(id), vec![], vec![])).collect();
        f.blocks[0].pos = vec![0; 9];
        let mut lim = InlineLimits::tier1();
        lim.budget = 0;
        lim.growth_limit = Some(5000);
        assert_eq!(inline_calls_ordered(options, &mut f, 0, &get, &|owner, _| owner == 0, &|_| false, &lim, None, None).len(), 9);
        assert_eq!(inline_inst_count(&f), 4500); // exceeds the 4000-instruction free allowance
        f.verify().unwrap();
    }

    #[test]
    fn cost_model_history_and_multi_call_depth_limits() {
        let options = &relax_session::Options::default();
        let mut c = cheap_callee(30); // too large for the unsampled free rule
        let lim = InlineLimits::tier1();
        let rule = |c: &Func, depth, multi_depth| site_rule(options, c, 1, 0, 0, 0, false, true, false, depth, multi_depth, &lim);
        assert_eq!(rule(&c, HISTORY_DEPTH_LIMIT - 1, 0), Some(RULE_COST));
        assert_eq!(rule(&c, HISTORY_DEPTH_LIMIT, 0), None);
        c.blocks[0].insts.extend(vec![Inst::IBinI(IOp::Add, I64, VReg(0), VReg(0), 1); 6]);
        assert_eq!(rule(&c, 6, TOP_DOWN_DEPTH_LIMIT + 1), Some(RULE_COST));
        c.blocks[0].insts.push(Inst::IBinI(IOp::Add, I64, VReg(0), VReg(0), 1));
        assert_eq!(rule(&c, 6, TOP_DOWN_DEPTH_LIMIT), Some(RULE_COST));
        assert_eq!(rule(&c, 6, TOP_DOWN_DEPTH_LIMIT + 1), None);
        c.blocks[0].insts.pop();
        c.block();
        c.blocks[1].term = Term::Ret(vec![]);
        assert_eq!(rule(&c, 6, TOP_DOWN_DEPTH_LIMIT + 1), None);
    }

    #[test]
    fn cost_model_depths_follow_inline_ancestry() {
        let options = &relax_session::Options::default();
        for (multi, expected) in [(false, 20), (true, 6)] {
            let callees: Vec<Func> = (1..=22).map(|id| {
                let mut c = cheap_callee(30);
                c.blocks[0].insts[29] = Inst::Call(Callee::Fn(id + 1), vec![], vec![]);
                if multi { c.blocks[0].insts[28] = Inst::Call(Callee::Fn(99), vec![], vec![]); }
                c
            }).collect();
            let get = |id: u32| callees.get(id.wrapping_sub(1) as usize).map(|c| (c as *const Func, 0));
            let mut f = one_call(1);
            let inlined = inline_calls_ordered(options, &mut f, 0, &get, &|_, _| false, &|_| false, &InlineLimits::tier1(), Some(&[0.0]), None);
            assert_eq!(inlined, (1..=expected).map(|id| (id, RULE_COST)).collect::<Vec<_>>());
            f.verify().unwrap();
        }
    }

    #[test]
    fn single_caller_requires_the_tier0_reference_owner() {
        let options = &{ let mut o = relax_session::Options::default(); o.opt.push("+single".into()); o };
        let mut wrapper = cheap_callee(30);
        wrapper.blocks[0].insts[29] = Inst::Call(Callee::Fn(2), vec![], vec![]);
        let body = callee(500);
        let get = |id| match id {
            1 => Some((&wrapper as *const Func, 0)),
            2 => Some((&body as *const Func, 0)),
            _ => None,
        };
        for owner in [0, 1] {
            let mut f = one_call(1);
            let single = |caller, callee| caller == owner && callee == 2;
            let result = inline_calls_ordered(options, &mut f, 0, &get, &single, &|_| false, &InlineLimits::tier1(), Some(&[0.0]), None);
            assert_eq!(result, if owner == 1 { vec![(1, RULE_COST), (2, RULE_SINGLE)] } else { vec![(1, RULE_COST)] });
            f.verify().unwrap();
        }
    }

}

#[cfg(test)]
mod slot_layout_tests {
    use super::*;

    /// slot_layout's offsets of two slots in a one-block loop `b1` (b0 jumps in, b1 loops on
    /// `c` or returns through b2), built by `body` from the slots' address vregs
    fn loop_layout(baseline: bool, slots: &[(u32, bool, bool)], body: &dyn Fn(&mut Func, &[VReg], VReg) -> Vec<Inst>) -> (Vec<i64>, i64) {
        let mut f = Func::new("loop".into(), 0);
        let (p, c) = (f.vreg(Cls::I), f.vreg(Cls::I));
        f.params.extend([p, c]);
        f.baseline = baseline;
        let mut addrs = Vec::new();
        for &(size, arg, ret) in slots {
            let s = f.slot(size, 8);
            f.slots[s as usize].arg = arg;
            f.slots[s as usize].ret = ret;
            f.slots[s as usize].born = true;
            addrs.push(f.vreg(Cls::I));
        }
        for _ in 0..3 {
            f.block();
        }
        f.blocks[0].term = Term::Jump(1);
        f.blocks[1].insts = body(&mut f, &addrs, p);
        f.blocks[1].term = Term::Branch(c, 1, 2);
        f.blocks[2].term = Term::Ret(vec![]);
        for b in &mut f.blocks {
            b.pos = vec![0; b.insts.len()];
        }
        slot_layout(&f, 0, true)
    }

    #[test]
    fn slot_layout_shares_argument_copies_consumed_in_a_loop() {
        // `out.push(a); out.push(b)` in a loop: each copy is built, then owned by its call
        let body = |f: &mut Func, a: &[VReg], p: VReg| {
            let k = f.vreg(Cls::I);
            vec![
                Inst::Iconst(k, 1),
                Inst::SlotAddr(a[0], 0), Inst::Store(Mem::Int(8, false), a[0], 0, k), Inst::Call(Callee::Fn(5), vec![p, a[0]], vec![]),
                Inst::SlotAddr(a[1], 1), Inst::Store(Mem::Int(8, false), a[1], 8, k), Inst::Call(Callee::Fn(5), vec![p, a[1]], vec![]),
            ]
        };
        let (at, end) = loop_layout(false, &[(128, true, false), (128, true, false)], &body);
        assert_eq!((at[0], end), (at[1], 128));
        // not argument copies: handed to a call with another pointer, they may be kept
        let (at, end) = loop_layout(false, &[(128, false, false), (128, false, false)], &body);
        assert_ne!(at[0], at[1]);
        assert_eq!(end, 256);
    }

    #[test]
    fn slot_layout_shares_a_temporary_copied_into_the_next() {
        // a value moved through a chain of temporaries: each copy is the source's last access
        let mut f = Func::new("chain".into(), 0);
        let (s0, s1) = (f.slot(128, 8), f.slot(128, 8));
        let (a, b, k, x) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
        f.block();
        f.blocks[0].insts = vec![
            Inst::Iconst(k, 3), Inst::SlotAddr(a, s0), Inst::Store(Mem::Int(8, false), a, 16, k),
            Inst::SlotAddr(b, s1), Inst::Copy(b, a, 128), Inst::Load(Mem::Int(8, false), x, b, 16),
        ];
        f.blocks[0].term = Term::Ret(vec![x]);
        f.blocks[0].pos = vec![0; 6];
        let (at, end) = slot_layout(&f, 0, true);
        assert_eq!((at[0], end), (at[1], 128));
        // a partial copy (offset 8 into the destination) leaves them apart
        let off = f.vreg(Cls::I);
        f.blocks[0].insts[4] = Inst::Copy(off, a, 16);
        f.blocks[0].insts.insert(4, Inst::IBinI(IOp::Add, U64, off, b, 8));
        f.blocks[0].pos.push(0);
        let (at, _) = slot_layout(&f, 0, true);
        assert_ne!(at[0], at[1]);
    }

    #[test]
    fn slot_layout_reads_born_slots_of_baseline_bodies_by_their_addresses() {
        // two temporaries written and read in one loop body: around the loop each looks live
        // (its contents could be read on the next iteration), unless lowering's SlotAddr marks
        // where its value begins
        let body = |f: &mut Func, a: &[VReg], _p: VReg| {
            let (k, x, y) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
            vec![
                Inst::Iconst(k, 1),
                Inst::SlotAddr(a[0], 0), Inst::Store(Mem::Int(8, false), a[0], 0, k), Inst::Load(Mem::Int(8, false), x, a[0], 0),
                Inst::SlotAddr(a[1], 1), Inst::Store(Mem::Int(8, false), a[1], 0, x), Inst::Load(Mem::Int(8, false), y, a[1], 0),
            ]
        };
        // eight slots: a baseline body with fewer than 8 slots (or fewer slot bytes than
        // SLOT_SHARE_MIN_BYTES) is laid out sequentially, the cheap cold layout; the body uses
        // slots 0 and 1, the others only make the frame big enough to be coloured
        let slots = [(128, false, false); 8];
        let (at, _) = loop_layout(true, &slots, &body);
        assert_eq!(at[0], at[1]);
        let (at, _) = loop_layout(false, &slots, &body);
        assert_ne!(at[0], at[1]);
    }

    #[test]
    fn slot_layout_ends_a_result_buffer_before_its_call() {
        // `let t = f(); use(t); let u = f(); use(u);` in a loop: each call writes all of its
        // result buffer, so neither holds anything across the back edge
        let body = |f: &mut Func, a: &[VReg], _p: VReg| {
            let (x, y) = (f.vreg(Cls::I), f.vreg(Cls::I));
            vec![
                Inst::SlotAddr(a[0], 0), Inst::Call(Callee::Fn(7), vec![a[0]], vec![]), Inst::Load(Mem::Int(8, false), x, a[0], 0),
                Inst::SlotAddr(a[1], 1), Inst::Call(Callee::Fn(7), vec![a[1]], vec![]), Inst::Load(Mem::Int(8, false), y, a[1], 8),
            ]
        };
        let with_sret = |f: &mut Func, a: &[VReg], p: VReg| {
            f.sret_callees.push(7);
            body(f, a, p)
        };
        let (at, end) = loop_layout(false, &[(128, false, true), (128, false, true)], &with_sret);
        assert_eq!((at[0], end), (at[1], 128));
        // not known to write all of it: the old contents may be read on the next iteration
        let (at, _) = loop_layout(false, &[(128, false, false), (128, false, false)], &with_sret);
        assert_ne!(at[0], at[1]);
    }
}

/// Rust `min`/`max` (`FOp::Min`/`Max`: a NaN operand yields the other) with one operand that is
/// never NaN become `a < b ? a : b` / `a > b ? a : b` (`MinLt`/`MaxGt`) with that operand second:
/// `minss` returns its second operand when either is NaN, which is then the right answer -- one
/// instruction instead of the five of the general NaN fix-up. Never NaN: non-NaN constants,
/// integer conversions, and (to a fixpoint over every definition, loop-carried ones included)
/// moves, negations / absolute values of such values and min/max results with such an operand
/// (a bounding box's running `lo = lo.min(p)` from `f32::MAX`). Returns whether anything changed.
pub fn min_max_non_nan(f: &mut Func) -> bool {
    let nv = f.vregs.len();
    // candidates: vregs all of whose definitions are of a kind that can be non-NaN
    let mut non_nan = vec![false; nv];
    let mut seen = vec![false; nv];
    let mut any_minmax = false;
    for b in &f.blocks {
        for i in &b.insts {
            match i {
                Inst::FBin(FOp::Min | FOp::Max, ..) => {
                    any_minmax = true;
                }
                _ => {}
            }
        }
    }
    if !any_minmax {
        return false;
    }
    for p in &f.params {
        seen[p.0 as usize] = true; // params: unknown
    }
    for b in &f.blocks {
        for i in &b.insts {
            let ok = match i {
                Inst::Fconst(_, bits, f64_) => {
                    if *f64_ { !f64::from_bits(*bits).is_nan() } else { !f32::from_bits(*bits as u32).is_nan() }
                }
                Inst::Conv(Conv::IntToF(..), ..) => true,
                Inst::Mov(..) | Inst::FUnary(FUn::Neg | FUn::Abs, ..) | Inst::FBin(FOp::Min | FOp::Max | FOp::MinLt | FOp::MaxGt, ..) => true,
                _ => false,
            };
            for_defs(i, |d| {
                let k = d.0 as usize;
                if !seen[k] {
                    seen[k] = true;
                    non_nan[k] = ok;
                } else if !ok {
                    non_nan[k] = false;
                }
            });
        }
    }
    // operands decide, to a fixpoint (only falls)
    let mut changed = true;
    while changed {
        changed = false;
        for b in &f.blocks {
            for i in &b.insts {
                let (d, ok) = match *i {
                    Inst::Mov(d, s) => (d, non_nan[s.0 as usize]),
                    Inst::FUnary(FUn::Neg | FUn::Abs, _, d, s) => (d, non_nan[s.0 as usize]),
                    Inst::FBin(FOp::Min | FOp::Max, _, d, a, b2) => (d, non_nan[a.0 as usize] || non_nan[b2.0 as usize]),
                    Inst::FBin(FOp::MinLt | FOp::MaxGt, _, d, _, b2) => (d, non_nan[b2.0 as usize]),
                    _ => continue,
                };
                if non_nan[d.0 as usize] && !ok {
                    non_nan[d.0 as usize] = false;
                    changed = true;
                }
            }
        }
    }
    let mut any = false;
    for b in &mut f.blocks {
        for i in &mut b.insts {
            if let Inst::FBin(op @ (FOp::Min | FOp::Max), w, d, a, b2) = *i {
                let lt = if op == FOp::Min { FOp::MinLt } else { FOp::MaxGt };
                if non_nan[b2.0 as usize] {
                    *i = Inst::FBin(lt, w, d, a, b2);
                    any = true;
                } else if non_nan[a.0 as usize] {
                    *i = Inst::FBin(lt, w, d, b2, a);
                    any = true;
                }
            }
        }
    }
    any
}


/// Slots whose addresses are used only as (a) bases of loads / stores / vector accesses at
/// constant offsets, copy operands and constant derivations -- what `slot_scan` follows -- and
/// (b) `slot + i` with a runtime `i` (one IBin add of a slot address and a non-address, then
/// constant adds) used only as the base of loads and as copy sources: Some(the instructions
/// making such runtime-offset reads), else None (the slot escapes, or none such reads).
fn dyn_read_slots(f: &Func, vs: &[(u32, i64)]) -> Vec<Option<Vec<(u32, u32)>>> {
    const NONE: u32 = u32::MAX;
    let ns = f.slots.len();
    let nv = f.vregs.len();
    let mut ndef = vec![0u32; nv];
    for p in &f.params {
        ndef[p.0 as usize] += 1;
    }
    for b in &f.blocks {
        for i in &b.insts {
            for_defs(i, |d| ndef[d.0 as usize] += 1);
        }
    }
    let sl = |v: VReg| vs[v.0 as usize].0;
    // runtime-offset addresses: vreg -> slot
    let mut dynv = vec![NONE; nv];
    for _ in 0..3 {
        for b in &f.blocks {
            for i in &b.insts {
                match *i {
                    Inst::IBin(IOp::Add, it, d, x, y) if it.bits == 64 && ndef[d.0 as usize] == 1 => {
                        let (sx, sy) = (sl(x), sl(y));
                        let (dx, dy) = (dynv[x.0 as usize], dynv[y.0 as usize]);
                        let other_plain = |o: VReg| sl(o) == NONE && dynv[o.0 as usize] == NONE;
                        if sx != NONE && other_plain(y) {
                            dynv[d.0 as usize] = sx;
                        } else if sy != NONE && other_plain(x) {
                            dynv[d.0 as usize] = sy;
                        } else if dx != NONE && other_plain(y) {
                            dynv[d.0 as usize] = dx;
                        } else if dy != NONE && other_plain(x) {
                            dynv[d.0 as usize] = dy;
                        }
                    }
                    Inst::IBinI(IOp::Add, it, d, x, _) if it.bits == 64 && ndef[d.0 as usize] == 1 && dynv[x.0 as usize] != NONE => {
                        dynv[d.0 as usize] = dynv[x.0 as usize];
                    }
                    Inst::Mov(d, x) if ndef[d.0 as usize] == 1 && dynv[x.0 as usize] != NONE => {
                        dynv[d.0 as usize] = dynv[x.0 as usize];
                    }
                    _ => {}
                }
            }
        }
    }
    let mut bad = vec![false; ns];
    let mut reads: Vec<Vec<(u32, u32)>> = vec![Vec::new(); ns];
    let mut tu = Vec::new();
    for (bi, b) in f.blocks.iter().enumerate() {
        for (ii, i) in b.insts.iter().enumerate() {
            match *i {
                Inst::Load(_, _, base, _) if dynv[base.0 as usize] != NONE => {
                    reads[dynv[base.0 as usize] as usize].push((bi as u32, ii as u32));
                    continue;
                }
                Inst::Copy(d, s2, _) if dynv[s2.0 as usize] != NONE && dynv[d.0 as usize] == NONE => {
                    reads[dynv[s2.0 as usize] as usize].push((bi as u32, ii as u32));
                    if sl(d) != NONE || true {
                        // the destination is checked below like any operand
                    }
                }
                _ => {}
            }
            // every other use of a runtime-offset address escapes its slot; uses of constant
            // addresses are judged by slot_scan (its `esc`); the derivations above are fine
            let deriv = matches!(*i, Inst::IBin(IOp::Add, _, d, ..) | Inst::IBinI(IOp::Add, _, d, ..) | Inst::Mov(d, _) if dynv[d.0 as usize] != NONE);
            let copy_src = matches!(*i, Inst::Copy(d, s2, _) if dynv[s2.0 as usize] != NONE && dynv[d.0 as usize] == NONE);
            if deriv {
                continue;
            }
            for_uses(i, |v| {
                let dv = dynv[v.0 as usize];
                if dv != NONE && !(copy_src && matches!(*i, Inst::Copy(_, s2, _) if s2 == v)) {
                    bad[dv as usize] = true;
                }
            });
        }
        term_uses(&b.term, &mut tu);
        for v in &tu {
            if dynv[v.0 as usize] != NONE {
                bad[dynv[v.0 as usize] as usize] = true;
            }
        }
    }
    // the slot's constant-address uses must be what slot_scan follows, plus the IBin adds
    let mut const_ok = vec![true; ns];
    for b in &f.blocks {
        for i in &b.insts {
            match *i {
                Inst::Load(_, _, base, _) | Inst::VLoad(_, base, _) => {
                    let _ = base;
                }
                Inst::Store(_, base, _, v) | Inst::VStore(base, _, v) => {
                    let _ = base;
                    if sl(v) != NONE {
                        const_ok[sl(v) as usize] = false;
                    }
                    if dynv[v.0 as usize] != NONE {
                        bad[dynv[v.0 as usize] as usize] = true;
                    }
                }
                Inst::Copy(..) | Inst::SlotAddr(..) => {}
                Inst::Mov(d, x) | Inst::IBinI(IOp::Add, _, d, x, _) if sl(x) != NONE && sl(d) == sl(x) => {}
                Inst::IBin(IOp::Add, _, d, x, y) if dynv[d.0 as usize] != NONE && (sl(x) != NONE || sl(y) != NONE) => {}
                _ => for_uses(i, |v| {
                    if sl(v) != NONE {
                        const_ok[sl(v) as usize] = false;
                    }
                }),
            }
        }
        term_uses(&b.term, &mut tu);
        for v in &tu {
            if sl(*v) != NONE {
                const_ok[sl(*v) as usize] = false;
            }
        }
    }
    (0..ns).map(|s2| (!bad[s2] && const_ok[s2] && !reads[s2].is_empty()).then(|| std::mem::take(&mut reads[s2]))).collect()
}

/// Write-through promotion of a slot that is also read at runtime offsets (an array iterator:
/// `IntoIter { data, start, end }` reads `data[start]`): its constant-offset cells live in
/// vregs -- loads of them read the vreg, stores also set the vreg, a copy into the slot reloads
/// the cells it covers -- while memory stays exact for the runtime-offset reads (every store is
/// kept). The loop counter `start` then is a register, so the loop can be fully unrolled, after
/// which the reads have constant offsets and ordinary SROA removes the slot. Slots qualify per
/// `dyn_read_slots`; their constant accesses must be loads / stores of one kind and size per
/// offset, not overlapping, and copies into them must not cut a cell. Returns whether anything
/// changed.
pub fn promote_dyn_read_slots(f: &mut Func) -> bool {
    let ns = f.slots.len();
    if ns == 0 || !f.blocks.iter().any(|b| b.insts.iter().any(|i| matches!(i, Inst::IBin(IOp::Add, ..)))) {
        return false;
    }
    let SlotScan { vs, .. } = slot_scan(f);
    let dynr = dyn_read_slots(f, &vs);
    if dynr.iter().all(|x| x.is_none()) {
        return false;
    }
    const NONE: u32 = u32::MAX;
    let sl = |v: VReg| vs[v.0 as usize].0;
    let at = |v: VReg| vs[v.0 as usize].1;
    // cells per slot: (offset, kind)
    let mut cells: Vec<Vec<(i64, Mem)>> = vec![Vec::new(); ns];
    let mut ok: Vec<bool> = dynr.iter().map(|x| x.is_some()).collect();
    let mut copies_in: Vec<Vec<(i64, i64)>> = vec![Vec::new(); ns];
    for b in &f.blocks {
        for i in &b.insts {
            match *i {
                Inst::Load(m, _, base, off) | Inst::Store(m, base, off, _) if sl(base) != NONE => {
                    cells[sl(base) as usize].push((at(base) + off as i64, m));
                }
                Inst::VLoad(_, base, _) | Inst::VStore(base, ..) if sl(base) != NONE => ok[sl(base) as usize] = false,
                Inst::Copy(d, _, n) if sl(d) != NONE => copies_in[sl(d) as usize].push((at(d), at(d) + n as i64)),
                _ => {}
            }
        }
    }
    for s2 in 0..ns {
        if !ok[s2] {
            continue;
        }
        let c = &mut cells[s2];
        c.sort_unstable_by_key(|x| (x.0, mem_size(x.1)));
        c.dedup_by(|b2, a| a.0 == b2.0 && a.1 == b2.1);
        for w in c.windows(2) {
            // one kind per offset, no overlaps
            if w[0].0 + mem_size(w[0].1) as i64 > w[1].0 {
                ok[s2] = false;
            }
        }
        if c.is_empty() || c.iter().any(|x| x.0 < 0 || x.0 + mem_size(x.1) as i64 > f.slots[s2].size as i64) {
            ok[s2] = false;
        }
        for &(lo, hi) in &copies_in[s2] {
            for x in c.iter() {
                let end = x.0 + mem_size(x.1) as i64;
                if (x.0 < lo && end > lo) || (x.0 < hi && end > hi) {
                    ok[s2] = false;
                }
            }
        }
    }
    if !ok.iter().any(|x| *x) {
        return false;
    }
    // one vreg per cell
    let mut cell_vreg: Vec<Vec<VReg>> = vec![Vec::new(); ns];
    for s2 in 0..ns {
        if ok[s2] {
            for &(_, m) in &cells[s2] {
                let cls = match m {
                    Mem::F32 => Cls::F32,
                    Mem::F64 => Cls::F64,
                    Mem::Int(..) => Cls::I,
                };
                let v = f.vreg(cls);
                cell_vreg[s2].push(v);
            }
        }
    }
    let find = |s2: usize, off: i64| -> Option<usize> { cells[s2].iter().position(|x| x.0 == off) };
    // the cells start as zeros at entry (read before any write only in undefined programs)
    let mut init = Vec::new();
    for s2 in 0..ns {
        if ok[s2] {
            for (k, &(_, m)) in cells[s2].iter().enumerate() {
                let v = cell_vreg[s2][k];
                init.push(match m {
                    Mem::F32 => Inst::Fconst(v, 0, false),
                    Mem::F64 => Inst::Fconst(v, 0, true),
                    Mem::Int(..) => Inst::Iconst(v, 0),
                });
            }
        }
    }
    let p0 = f.blocks[0].pos.first().copied().unwrap_or(f.blocks[0].term_pos);
    let ninit = init.len();
    f.blocks[0].insts.splice(0..0, init);
    f.blocks[0].pos.splice(0..0, std::iter::repeat(p0).take(ninit));
    for bi in 0..f.blocks.len() {
        if !f.blocks[bi].insts.iter().any(|i| matches!(*i, Inst::Load(_, _, base, _) | Inst::Store(_, base, _, _) | Inst::Copy(base, _, _) if sl(base) != NONE && ok[sl(base) as usize])) {
            continue;
        }
        let old = std::mem::take(&mut f.blocks[bi].insts);
        let oldp = std::mem::take(&mut f.blocks[bi].pos);
        let mut ins = Vec::with_capacity(old.len() + 4);
        let mut pos = Vec::with_capacity(old.len() + 4);
        for (i, p) in old.into_iter().zip(oldp) {
            match i {
                Inst::Load(_, d, base, off) if sl(base) != NONE && ok[sl(base) as usize] => {
                    let s2 = sl(base) as usize;
                    let k = find(s2, at(base) + off as i64).unwrap();
                    ins.push(Inst::Mov(d, cell_vreg[s2][k]));
                    pos.push(p);
                }
                Inst::Store(m, base, off, v) if sl(base) != NONE && ok[sl(base) as usize] => {
                    let s2 = sl(base) as usize;
                    let k = find(s2, at(base) + off as i64).unwrap();
                    ins.push(Inst::Store(m, base, off, v));
                    pos.push(p);
                    // the cell holds what a load of it gives: a narrow integer re-extended
                    ins.push(match m {
                        Mem::Int(n, signed) if n < 8 => Inst::Conv(Conv::IntToInt(IntTy { bits: n * 8, signed }), cell_vreg[s2][k], v),
                        _ => Inst::Mov(cell_vreg[s2][k], v),
                    });
                    pos.push(p);
                }
                Inst::Copy(d, s3, n) if sl(d) != NONE && ok[sl(d) as usize] => {
                    let s2 = sl(d) as usize;
                    let (lo, hi) = (at(d), at(d) + n as i64);
                    let copy_at = ins.len();
                    ins.push(Inst::Copy(d, s3, n));
                    pos.push(p);
                    for (k, &(off, m)) in cells[s2].iter().enumerate() {
                        if off >= lo && off + mem_size(m) as i64 <= hi {
                            // the copied bytes' value when this block stored them into the source
                            // slot itself (an iterator built in a temporary, then moved): that
                            // value, so the cell is a known constant where it was one
                            let stored = (sl(s3) != NONE).then(|| {
                                let want = at(s3) + (off - lo);
                                let mut found = None;
                                for j in (0..copy_at).rev() {
                                    match ins[j] {
                                        Inst::Store(m2, b2, o2, v) if sl(b2) == sl(s3) && at(b2) + o2 as i64 == want && m2 == m => {
                                            found = Some(v);
                                            break;
                                        }
                                        // another field of the source: no overlap, keep looking
                                        Inst::Store(m2, b2, o2, _) if sl(b2) == sl(s3) && {
                                            let o = at(b2) + o2 as i64;
                                            o + mem_size(m2) as i64 <= want || want + mem_size(m) as i64 <= o
                                        } => {}
                                        // a store into another known slot cannot touch the source
                                        Inst::Store(_, b2, ..) if sl(b2) != NONE && sl(b2) != sl(s3) => {}
                                        Inst::Store(..) | Inst::Copy(..) | Inst::Call(..) | Inst::StoreX(..) | Inst::VStore(..) => break,
                                        _ => {}
                                    }
                                }
                                found
                            }).flatten();
                            match stored {
                                Some(v) => ins.push(match m {
                                    Mem::Int(sz, signed) if sz < 8 => Inst::Conv(Conv::IntToInt(IntTy { bits: sz * 8, signed }), cell_vreg[s2][k], v),
                                    _ => Inst::Mov(cell_vreg[s2][k], v),
                                }),
                                None => ins.push(Inst::Load(m, cell_vreg[s2][k], d, (off - lo) as i32)),
                            }
                            pos.push(p);
                        }
                    }
                }
                other => {
                    ins.push(other);
                    pos.push(p);
                }
            }
        }
        f.blocks[bi].insts = ins;
        f.blocks[bi].pos = pos;
    }
    true
}

/// Stores into a slot that nothing ever reads back at those bytes (no load, no copy out of the
/// slot overlaps them) go: after full unrolling, a write-through counter's memory copy
/// (`start` of an array iterator) is written every iteration and read by nobody, and it keeps
/// the slot from being forwarded or scalar-replaced. Only slots `slot_scan` follows completely
/// (not escaping). Returns whether any store went.
pub fn drop_unread_slot_stores(f: &mut Func) -> bool {
    let ns = f.slots.len();
    if ns == 0 {
        return false;
    }
    let SlotScan { vs, esc, .. } = slot_scan(f);
    const NONE: u32 = u32::MAX;
    let sl = |v: VReg| vs[v.0 as usize].0;
    let at = |v: VReg| vs[v.0 as usize].1;
    let mut reads: Vec<Vec<(i64, i64)>> = vec![Vec::new(); ns];
    for b in &f.blocks {
        for i in &b.insts {
            match *i {
                Inst::Load(m, _, base, off) if sl(base) != NONE => {
                    let o = at(base) + off as i64;
                    reads[sl(base) as usize].push((o, o + mem_size(m) as i64));
                }
                Inst::VLoad(_, base, off) if sl(base) != NONE => {
                    let o = at(base) + off as i64;
                    reads[sl(base) as usize].push((o, o + 16));
                }
                Inst::LoadX(_, _, base, ..) if sl(base) != NONE => reads[sl(base) as usize].push((i64::MIN, i64::MAX)),
                Inst::Copy(_, s2, n) if sl(s2) != NONE => {
                    let o = at(s2);
                    reads[sl(s2) as usize].push((o, o + n as i64));
                }
                _ => {}
            }
        }
    }
    let unread = |s2: usize, lo: i64, hi: i64| -> bool { !esc[s2] && reads[s2].iter().all(|&(a, b2)| b2 <= lo || hi <= a) };
    let mut any = false;
    for b in &mut f.blocks {
        let mut k = 0;
        while k < b.insts.len() {
            let drop = match b.insts[k] {
                Inst::Store(m, base, off, _) if sl(base) != NONE => {
                    let o = at(base) + off as i64;
                    unread(sl(base) as usize, o, o + mem_size(m) as i64)
                }
                _ => false,
            };
            if drop {
                b.insts.remove(k);
                if k < b.pos.len() {
                    b.pos.remove(k);
                }
                any = true;
            } else {
                k += 1;
            }
        }
    }
    any
}

#[cfg(test)]
mod rotate_tests {
    use super::*;

    /// `while i < n { s += i; i += 1 }` with the header `i = k; c = i < n; branch c` (b1),
    /// body + latch b2 (`jump b1`); i is read in the body, c only in the header.
    fn counted_loop() -> (Func, VReg, VReg) {
        let mut f = Func::new("rot".into(), 0);
        let n = f.vreg(Cls::I);
        f.params.push(n);
        f.rets.push(Cls::I);
        let (k, i, c, s) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
        for _ in 0..4 {
            f.block();
        }
        f.blocks[0].insts = vec![Inst::Iconst(k, 0), Inst::Iconst(s, 0)];
        f.blocks[0].term = Term::Jump(1);
        f.blocks[1].insts = vec![Inst::Mov(i, k), Inst::ICmp(Cond::Lt, false, c, i, n)];
        f.blocks[1].term = Term::Branch(c, 2, 3);
        f.blocks[2].insts = vec![Inst::IBin(IOp::Add, U64T, s, s, i), Inst::IBinI(IOp::Add, U64T, k, i, 1)];
        f.blocks[2].term = Term::Jump(1);
        f.blocks[3].term = Term::Ret(vec![s]);
        for b in &mut f.blocks {
            b.pos = vec![0; b.insts.len()];
        }
        (f, i, c)
    }
    const U64T: IntTy = IntTy { bits: 64, signed: false };

    #[test]
    fn rotation_keeps_a_header_value_the_body_reads() {
        let (mut f, i, c) = counted_loop();
        rotate_loops(&mut f);
        f.verify().unwrap();
        // the latch runs the test itself: i keeps its vreg (the body reads it), c gets a fresh one
        let l = &f.blocks[2];
        assert!(matches!(l.term, Term::Branch(c2, 2, 3) if c2 != c));
        assert!(matches!(l.insts[2], Inst::Mov(d, _) if d == i));
        assert!(matches!(l.insts[3], Inst::ICmp(Cond::Lt, false, _, a, _) if a == i));
        // the header is unchanged (the entry still runs it)
        assert!(matches!(f.blocks[1].term, Term::Branch(c1, 2, 3) if c1 == c));
    }

    /// `while n != 0 { n -= 1; if skip(n) { continue } work }`: the skip branch's false edge
    /// goes to the header; it gets its own copy of the header's test.
    #[test]
    fn rotation_gives_a_branch_latch_its_own_test() {
        let mut f = Func::new("rotb".into(), 0);
        let n0 = f.vreg(Cls::I);
        f.params.push(n0);
        f.rets.push(Cls::I);
        let (n, c, s, k) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
        for _ in 0..5 {
            f.block();
        }
        f.blocks[0].insts = vec![Inst::Mov(n, n0), Inst::Iconst(k, 0)];
        f.blocks[0].term = Term::Jump(1);
        f.blocks[1].insts = vec![Inst::ICmpI(Cond::Ne, false, c, n, 0)];
        f.blocks[1].term = Term::Branch(c, 2, 4);
        f.blocks[2].insts = vec![Inst::IBinI(IOp::Sub, U64T, n, n, 1), Inst::ICmpI(Cond::Eq, false, s, n, 7)];
        f.blocks[2].term = Term::Branch(s, 3, 1);
        f.blocks[3].insts = vec![Inst::IBinI(IOp::Add, U64T, k, k, 1)];
        f.blocks[3].term = Term::Jump(1);
        f.blocks[4].term = Term::Ret(vec![k]);
        for b in &mut f.blocks {
            b.pos = vec![0; b.insts.len()];
        }
        rotate_loops(&mut f);
        f.verify().unwrap();
        // the skip block's false edge goes to a block of its own that tests n again
        let li = f.blocks.iter().position(|b| matches!(b.insts.last(), Some(Inst::ICmpI(Cond::Eq, _, _, _, 7)))).unwrap();
        let Term::Branch(_, _, t) = f.blocks[li].term else { panic!("latch branch") };
        assert!(matches!(f.blocks[t as usize].insts[..], [Inst::ICmpI(Cond::Ne, false, _, x, 0)] if x == n));
        assert_eq!(f.blocks.iter().filter(|b| b.term.succs().any(|x| x == t)).count(), 1);
        assert!(matches!(f.blocks[t as usize].term, Term::Branch(..)));
    }
}

#[cfg(test)]
mod merge_slot_addr_tests {
    use super::*;

    /// `x = if c { None } else { Some(*p) }` then `x` moved into `y` and read: lowering /
    /// inlining define y's address on both paths into the merge. One entry definition makes
    /// the move forwardable: the read goes to x and the copy goes.
    #[test]
    fn duplicated_slot_address_is_one_value() {
        let mut f = Func::new("unify".into(), 0);
        let (p, c) = (f.vreg(Cls::I), f.vreg(Cls::I));
        f.params.extend([p, c]);
        f.rets.push(Cls::I);
        let (x, y) = (f.slot(16, 8), f.slot(16, 8));
        let (ax, ay, k, t) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
        for _ in 0..4 {
            f.block();
        }
        f.blocks[0].insts = vec![Inst::SlotAddr(ax, x)];
        f.blocks[0].term = Term::Branch(c, 1, 2);
        f.blocks[1].insts = vec![Inst::Iconst(k, 2), Inst::Store(Mem::Int(4, false), ax, 8, k), Inst::SlotAddr(ay, y)];
        f.blocks[1].term = Term::Jump(3);
        f.blocks[2].insts = vec![Inst::Copy(ax, p, 16), Inst::SlotAddr(ay, y)];
        f.blocks[2].term = Term::Jump(3);
        f.blocks[3].insts = vec![Inst::Copy(ay, ax, 16), Inst::Load(Mem::Int(4, false), t, ay, 8)];
        f.blocks[3].term = Term::Ret(vec![t]);
        for b in &mut f.blocks {
            b.pos = vec![0; b.insts.len()];
        }
        assert!(merge_slot_addr_defs(&mut f));
        f.verify().unwrap();
        let defs: Vec<usize> = f.blocks.iter().enumerate().flat_map(|(bi, b)| b.insts.iter().filter(move |i| matches!(i, Inst::SlotAddr(d, _) if *d == ay)).map(move |_| bi)).collect();
        assert_eq!(defs, vec![0]);
        assert!(forward_slot_copies(&mut f));
        assert!(!f.blocks[3].insts.iter().any(|i| matches!(i, Inst::Copy(..))));
        // a second pass changes nothing
        assert!(!merge_slot_addr_defs(&mut f));
    }
}

#[cfg(test)]
mod float_identity_tests {
    use super::*;

    /// The identities simplify drops: x * 1.0, x / 1.0, x - 0.0 and x + -0.0 are x; x + 0.0
    /// is not (-0.0 + 0.0 = +0.0) and stays.
    #[test]
    fn exact_float_identities_fold() {
        let opts = relax_session::Options::default();
        for (op, k, folds) in [(FOp::Mul, 1.0f32, true), (FOp::Div, 1.0, true), (FOp::Sub, 0.0, true), (FOp::Add, -0.0, true), (FOp::Add, 0.0, false)] {
            let mut f = Func::new("fid".into(), 0);
            let x = f.vreg(Cls::F32);
            f.params.push(x);
            f.rets.push(Cls::F32);
            let (c, d) = (f.vreg(Cls::F32), f.vreg(Cls::F32));
            f.block();
            f.blocks[0].insts = vec![Inst::Fconst(c, k.to_bits() as u64, false), Inst::FBin(op, false, d, x, c)];
            f.blocks[0].pos = vec![0; 2];
            f.blocks[0].term = Term::Ret(vec![d]);
            simplify(&opts, &mut f, None, true, true);
            f.verify().unwrap();
            let has_op = f.blocks[0].insts.iter().any(|i| matches!(i, Inst::FBin(..)));
            assert_eq!(!has_op, folds, "{op:?} {k}");
        }
    }
}

#[cfg(test)]
mod post_increment_tests {
    use super::*;
    const U64T: IntTy = IntTy { bits: 64, signed: false };

    /// `for e in es { if e.f != 0 { e.g = 1 } }` with es a pointer pair: the element pointer's
    /// accesses move to the cursor (offset - 424 after the step) and the copy goes.
    fn walk(extra_def: bool) -> Func {
        let mut f = Func::new("walk".into(), 0);
        let (start, end) = (f.vreg(Cls::I), f.vreg(Cls::I));
        f.params.extend([start, end]);
        let (x, c, p, t, v, z, one) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
        for _ in 0..5 {
            f.block();
        }
        f.blocks[0].insts = vec![Inst::Mov(x, start), Inst::Iconst(one, 1)];
        f.blocks[0].term = Term::Jump(1);
        f.blocks[1].insts = vec![Inst::ICmp(Cond::Eq, false, c, x, end)];
        f.blocks[1].term = Term::Branch(c, 4, 2);
        f.blocks[2].insts = vec![Inst::Mov(p, x), Inst::Load(Mem::Int(8, false), v, p, 0), Inst::IBinI(IOp::Add, U64T, t, p, 424), Inst::Mov(x, t), Inst::Load(Mem::Int(1, false), z, p, 96)];
        f.blocks[2].term = Term::Branch(z, 3, 1);
        f.blocks[3].insts = vec![Inst::Store(Mem::Int(8, false), p, 8, one)];
        if extra_def {
            f.blocks[3].insts.push(Inst::Mov(x, start));
            f.blocks[3].insts.push(Inst::Store(Mem::Int(8, false), p, 16, one));
        }
        f.blocks[3].term = Term::Jump(1);
        f.blocks[4].term = Term::Ret(vec![]);
        for b in &mut f.blocks {
            b.pos = vec![0; b.insts.len()];
        }
        f
    }

    #[test]
    fn element_pointer_reads_the_cursor() {
        let mut f = walk(false);
        assert!(fold_post_increments(&mut f));
        f.verify().unwrap();
        let x = f.blocks[0].insts.iter().find_map(|i| if let Inst::Mov(d, _) = i { Some(*d) } else { None }).unwrap();
        // the element pointer's copy and the temporary went; the cursor steps in place
        assert!(matches!(f.blocks[2].insts[0], Inst::Load(_, _, b, 0) if b == x));
        assert!(matches!(f.blocks[2].insts[1], Inst::IBinI(IOp::Add, _, d, a, 424) if a == x && d == x));
        assert!(matches!(f.blocks[2].insts[2], Inst::Load(_, _, b, -328) if b == x));
        assert!(matches!(f.blocks[3].insts[0], Inst::Store(_, b, -416, _) if b == x));
    }

    #[test]
    fn cursor_rewritten_before_a_use_keeps_the_pointer() {
        let mut f = walk(true);
        assert!(!fold_post_increments(&mut f));
    }
}
