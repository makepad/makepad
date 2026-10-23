//! Cheap, target-independent O1 optimisations on RIR. Every pass is linear
//! (or a few linear sweeps) in the function size:
//! - inlining of tiny leaf callees (bottom-up: callees are already optimised),
//! - constant folding incl. loads from immutable data (const arrays, literals),
//! - copy propagation for single-definition vregs,
//! - immediate operand forms, branch folding, jump threading, dead code removal,
//! - block re-layout so fall-through follows the hot edge.

use relax_backend::regalloc::{for_defs, for_uses, reachable, term_uses};
use relax_backend::dom::{idoms, dom_numbers};
use relax_backend::frame::slot_colors_used;
use relax_backend::stats::opt_off;

/// MAKEPAD_RUN_OPT_HIST: opt-pipeline cpu ns (0..6) and function counts (6..12) by optimised size
pub static HIST: [std::sync::atomic::AtomicU64; 12] = [const { std::sync::atomic::AtomicU64::new(0) }; 12];
use relax_backend::rir::*;
use relax_backend::passes::*;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
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
        crate::native::install_probe();
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
            Term::Branch(..) | Term::Switch(..) => penalty += INSTR_COST,
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

/// The largest body any O3 rule admits for a callee with these attributes (the cost rule's
/// bodies are small: at most 25 instructions that are not moves or constants): a bottom-up
/// preparation whose O1 body is already larger rarely optimises down into range, so it is not
/// prepared and its sites decide on the snapshot body.
pub fn max_admitted_insts(options: &relax_session::Options, attrs: u8) -> usize {
    let lim = InlineLimits::tier1();
    if single_on(options) {
        return INLINE_SINGLE_INSTS;
    }
    let mut n = lim.leaf_insts.max(lim.hot_insts);
    if attrs & ATTR_ALWAYS != 0 { n = n.max(lim.always_insts); }
    if attrs & ATTR_HINT != 0 { n = n.max(lim.hint_insts); }
    if attrs & ATTR_CHAIN != 0 { n = n.max(lim.chain_insts); }
    n
}

pub fn inline_rule(f: &Func, callee_id: u32, nargs: usize, nrets: usize, attrs: u8, lim: &InlineLimits) -> Option<usize> {
    let (n, leaf, rec) = callee_shape(f, callee_id);
    // a naked fn is its asm only: never inlined (rustc: naked implies inline(never))
    if rec || f.naked {
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
        return inline_calls_ordered(options, f, self_id, get, &|_| None, &|_, _| false, nounwind, lim, None, None);
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
    /// sites admitted only as their constant arguments price the callee
    pub in_context: usize,
    pub rules: [usize; RULE_NAMES.len()],
    hot_inlined: usize,
    hot_left: usize,
    growth_before: Option<usize>,
    growth_after: usize,
    pub left: [usize; 5],
    pub rejected: std::collections::BTreeMap<(String, &'static str), usize>,
    /// calls left in hot blocks (not cold, not panic paths): name, reason, callee size
    pub hot_rejected: std::collections::BTreeMap<(String, &'static str, usize), usize>,
    /// optimisation remarks (makepad inspect `asm`, `remarks`): every call site the queue kept as
    /// a call, at its source position, with the refusal and its numbers
    pub remarks: Vec<SiteRemark>,
    /// record only `remarks` (every O3 build): the end-of-pass census reads callee bodies, which
    /// would add to the build's recorded reads
    pub remarks_only: bool,
}

/// Why the inliner kept one call site a call (an O3 optimisation remark).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SiteRemark {
    /// the call's source position (RIR `Block::pos`, inline context bits included)
    pub pos: u64,
    pub callee: u32,
    pub why: u8,
    /// the site is in a hot block (loop / sampled hot)
    pub hot: bool,
    /// the callee's size as priced (instructions)
    pub size: u32,
    pub a: u32,
    pub b: u32,
}

/// `SiteRemark::why` values.
pub const WHY_NO_BODY: u8 = 1;
pub const WHY_RECURSIVE: u8 = 2;
pub const WHY_NO_RULE: u8 = 3;
pub const WHY_COLD_ATTR: u8 = 4;
pub const WHY_ARITY: u8 = 5;
pub const WHY_FREE_SPENT: u8 = 6;
pub const WHY_GROWTH: u8 = 7;
pub const WHY_BUDGET: u8 = 8;
/// an inspect experiment forbade it (`-inl@<slot>`)
pub const WHY_FORBIDDEN: u8 = 9;

impl SiteRemark {
    /// The refusal in words, with its numbers.
    pub fn describe(&self) -> String {
        match self.why {
            WHY_NO_BODY => "no optimised body of the callee yet (not compiled, or no RIR: a host / foreign fn)".into(),
            WHY_RECURSIVE => "recursive: the callee is already being inlined on this path".into(),
            WHY_NO_RULE => format!(
                "no inline rule admits it: callee {} insts{}{} (rules: tiny <= call, #[inline]/always within their limits, small leaf, rustc cost model, single caller)",
                self.size,
                if self.a as u8 & ATTR_ALWAYS != 0 { ", #[inline(always)]" } else if self.a as u8 & ATTR_HINT != 0 { ", #[inline]" } else { ", no #[inline]" },
                if self.hot { ", hot site" } else { ", site not hot" }
            ),
            WHY_COLD_ATTR => format!("callee is #[cold] and {} insts (above the free size)", self.size),
            WHY_ARITY => "the call's arguments/results do not match the callee's signature (an ABI-lowered call)".into(),
            WHY_FREE_SPENT => format!("the free-size allowance is spent: callee {} insts, {} left", self.size, self.a),
            WHY_GROWTH => format!("growth ceiling: the caller would grow to {} insts, ceiling {} (callee {} insts)", self.a, self.b, self.size),
            WHY_BUDGET => format!("inline budget spent: callee costs {} insts, {} left", self.a, self.b),
            WHY_FORBIDDEN => "forbidden by an inspect experiment (noinline)".into(),
            _ => "kept a call".into(),
        }
    }
}

impl InlineStats {
    /// Stats that record only the optimisation remarks (every O3 build).
    pub fn remarks() -> InlineStats {
        InlineStats { remarks_only: true, ..Default::default() }
    }

    fn reject(&mut self, name: String, reason: usize, hot: Option<usize>) {
        const REASONS: [&str; 5] = ["cold", "no-rir", "no-rule", "budget", "arity"];
        if let Some(size) = hot {
            *self.hot_rejected.entry((name.clone(), REASONS[reason], size)).or_default() += 1;
        }
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
        eprintln!("inline-stats {}: sites {}, inlined {} ({} priced in context), hot {}, growth {}->{} insts, left: cold {}, no-rir {}, no-rule {}, budget {}, arity {}; top: {}",
            name, self.inlined + self.left.iter().sum::<usize>(), self.inlined, self.in_context,
            self.hot_inlined + self.hot_left, self.growth_before.unwrap_or(0), self.growth_after,
            self.left[0], self.left[1], self.left[2], self.left[3], self.left[4], top);
        if !self.hot_rejected.is_empty() {
            eprintln!("  hot calls left: {}", self.hot_rejected.iter().map(|((n, why, size), k)| format!("{} ({}, {} insts, {})", n, why, size, k)).collect::<Vec<_>>().join("; "));
        }
        eprintln!("  rules: {}", RULE_NAMES.iter().zip(self.rules).map(|(rule, n)| format!("{} {}", rule, n)).collect::<Vec<_>>().join(", "));
    }
}

/// O3's caller growth ceiling, scaled by the caller's measured hotness (HotSpot C2's frequent
/// vs small limits, .NET's profile-boosted size tolerance): a caller the sampler found hot
/// (its block frequencies are sampled) gets max(4x, +8000) instructions -- bottom-up inlining
/// made that room usable, max(2x, +4000) refused html_parse's and maps' hot sites for budget
/// (html 0.66, maps 0.97) --; every other caller max(2x, +4000): Stage's lukewarm UI functions
/// grew 3x in code with the big room and its O3 compile CPU went up 4x.
pub fn growth_ceiling(size: usize, hot: bool) -> usize {
    if hot {
        size.saturating_mul(4).max(size.saturating_add(8000))
    } else {
        size.saturating_mul(2).max(size.saturating_add(4000))
    }
}

/// In-context pricing (LLVM InlineCost's simplified values): the body a site whose arguments
/// include constants actually brings. Optimistic constant propagation over executable blocks
/// (Wegman-Zadeck SCCP), integer operations only; RIR registers are not SSA, so a register's
/// value is the meet of its definitions in executable blocks. In the result, an instruction
/// whose value is a constant is an `Iconst`, a branch on a constant a jump, a block no executable
/// edge reaches empty, and what nothing uses any more is gone. `consts`: per parameter, its
/// constant. None: the constants change nothing. (`hash::fast::mix_bytes` with a constant
/// length: the 16-byte loop and two of the three tails are dead.)
pub fn specialize_for_args(callee: &Func, consts: &[Option<i64>]) -> Option<Func> {
    #[derive(Clone, Copy, PartialEq)]
    enum V { Top, C(i64), Bot }
    fn meet(a: V, b: V) -> V {
        match (a, b) {
            (V::Top, x) | (x, V::Top) => x,
            (V::C(x), V::C(y)) if x == y => V::C(x),
            _ => V::Bot,
        }
    }
    if callee.blocks.is_empty() || !consts.iter().any(|c| c.is_some()) {
        return None;
    }
    let mut val = vec![V::Top; callee.vregs.len()];
    for (k, p) in callee.params.iter().enumerate() {
        val[p.0 as usize] = meet(val[p.0 as usize], consts.get(k).copied().flatten().map_or(V::Bot, V::C));
    }
    let mut exec = vec![false; callee.blocks.len()];
    exec[0] = true;
    let bin = |op: IOp, it: IntTy, a: V, b: V| match (a, b) {
        (V::C(x), V::C(y)) => fold_ibin(op, it, x, y).map_or(V::Bot, V::C),
        (V::Bot, _) | (_, V::Bot) => V::Bot,
        _ => V::Top,
    };
    loop {
        let mut changed = false;
        for bi in 0..callee.blocks.len() {
            if !exec[bi] { continue; }
            let b = &callee.blocks[bi];
            for i in &b.insts {
                let g = |v: &VReg| val[v.0 as usize];
                let r = match i {
                    Inst::Iconst(_, k) => V::C(*k),
                    Inst::Mov(_, a) => g(a),
                    Inst::IBin(op, it, _, a, b) => bin(*op, *it, g(a), g(b)),
                    Inst::IBinI(op, it, _, a, k) => bin(*op, *it, g(a), V::C(*k)),
                    Inst::INeg(it, _, a) => bin(IOp::Sub, *it, V::C(0), g(a)),
                    Inst::INot(it, _, a) => bin(IOp::Xor, *it, g(a), V::C(-1)),
                    Inst::ICmp(c, sg, _, a, b) => match (g(a), g(b)) {
                        (V::C(x), V::C(y)) => V::C(cmp_i(*c, *sg, x, y) as i64),
                        (V::Bot, _) | (_, V::Bot) => V::Bot,
                        _ => V::Top,
                    },
                    Inst::ICmpI(c, sg, _, a, k) => match g(a) {
                        V::C(x) => V::C(cmp_i(*c, *sg, x, *k) as i64),
                        x => x,
                    },
                    Inst::Conv(c @ Conv::IntToInt(_), _, a) => match g(a) {
                        V::C(x) => match fold_conv(*c, K::I(x)) { Some(K::I(y)) => V::C(y), _ => V::Bot },
                        x => x,
                    },
                    Inst::Sel(_, c, a, b) => match g(c) {
                        V::C(0) => g(b),
                        V::C(_) => g(a),
                        V::Top => V::Top,
                        V::Bot => meet(g(a), g(b)),
                    },
                    _ => V::Bot,
                };
                i.defs(|d| {
                    let m = meet(val[d.0 as usize], r);
                    if m != val[d.0 as usize] { val[d.0 as usize] = m; changed = true; }
                });
            }
            let mut mark = |t: u32, changed: &mut bool| if !exec[t as usize] { exec[t as usize] = true; *changed = true; };
            match &b.term {
                Term::Branch(c, t, e) => match val[c.0 as usize] {
                    V::C(0) => mark(*e, &mut changed),
                    V::C(_) => mark(*t, &mut changed),
                    V::Top => {}
                    V::Bot => { mark(*t, &mut changed); mark(*e, &mut changed); }
                },
                t => for s in t.succs() { mark(s, &mut changed); },
            }
        }
        if !changed { break; }
    }
    let mut g = callee.clone();
    let mut folded = false;
    for (bi, b) in g.blocks.iter_mut().enumerate() {
        if !exec[bi] {
            if !b.insts.is_empty() || !matches!(b.term, Term::Unreachable) { folded = true; }
            b.insts.clear();
            b.pos.clear();
            b.term = Term::Unreachable;
            continue;
        }
        for i in b.insts.iter_mut() {
            if matches!(i, Inst::Iconst(..)) || !is_pure(i) { continue; }
            let mut one = None;
            let mut n = 0;
            i.defs(|d| { one = Some(d); n += 1; });
            if let (1, Some(d)) = (n, one) {
                if let V::C(k) = val[d.0 as usize] {
                    if callee.vregs[d.0 as usize] == Cls::I {
                        *i = Inst::Iconst(d, k);
                        folded = true;
                    }
                }
            }
        }
        if let Term::Branch(c, t, e) = b.term {
            if let V::C(k) = val[c.0 as usize] {
                b.term = Term::Jump(if k != 0 { t } else { e });
                folded = true;
            }
        }
    }
    if !folded {
        return None;
    }
    dce(&mut g);
    Some(g)
}

impl InlineLimits {
    pub fn tier1_for(f: &Func, hot: bool) -> Self {
        let mut lim = Self::tier1();
        let size = inline_inst_count(f);
        lim.growth_limit = Some(growth_ceiling(size, hot));
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

/// PGO: a site the counts saw run at least this often per call of the function being inlined
/// into (profile::inline_freqs units: 0.1 per run) is very hot: any body up to the
/// inline(always) size may inline there (LLVM's hot-callsite threshold, which rustc PGO uses: it
/// inlines html_parse's process_entity) -- if the body does not loop: what inlining saves is the
/// call, and a body that iterates per call (sdf_glyphs' esdt over a row: +5.6% instructions when
/// it went into each caller) pays for its call many times over.
const PGO_VERY_HOT: f32 = 1.0;
/// ... a body loops when it runs more instructions per call than this many times its size
/// (HotSpot's back-edge / invocation ratio: a loop body is one that iterates about that often per
/// call). Measured on the counts: llm_host's HostDecodeBench::step
/// 3.9 (inlined: its caller specialises it), sdf_glyphs' esdt 31 (kept out of line).
const PGO_LOOP_WORK: f64 = 8.0;

/// A counted body's instructions executed per call over its size (1: straight-line code; a loop
/// raises it by its trips). None: no counts.
pub fn work_per_size(f: &Func) -> Option<f64> {
    let entry = f.blocks.first().map_or(-1.0, |b| b.count);
    if entry <= 0.0 {
        return None;
    }
    let (mut work, mut size) = (0.0f64, 0usize);
    for b in &f.blocks {
        size += b.insts.len() + 1;
        work += b.count.max(0.0) as f64 * (b.insts.len() + 1) as f64;
    }
    Some(work / entry as f64 / size.max(1) as f64)
}

fn pgo_site_limits(lim: &InlineLimits, counted: bool, priority: u32, callee: &Func) -> InlineLimits {
    let loops = || match work_per_size(callee) {
        Some(w) => w > PGO_LOOP_WORK,
        None => relax_backend::regalloc::loop_depths(callee, &reachable(callee)).iter().any(|d| *d > 0),
    };
    if counted && f32::from_bits(priority) >= PGO_VERY_HOT && !loops() {
        InlineLimits { hot_insts: lim.hot_insts.max(lim.always_insts), ..*lim }
    } else {
        *lim
    }
}

fn inline_trace(name: &str) -> bool {
    static T: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    T.get_or_init(|| std::env::var("MAKEPAD_RUN_INLINE_TRACE").ok().filter(|s| !s.is_empty())).as_deref().is_some_and(|t| name.ends_with(t))
}

/// Frequencies must describe this exact RIR. New blocks inherit the site's priority.
/// The queue uses stable block/instruction indices: splitting invalidates only the tail,
/// whose calls are enqueued again in the new continuation block.
/// `single(caller, callee)` proves that caller's O1 body owns the sole reference.
/// `prepared(callee)`: the callee bottom-up (O3), as (the body to inline: its own first inlining
/// round, unoptimised; the optimised body that prices the site). A site takes it when the priced
/// body passes the rules and fits, charging the growth ceiling with the priced size; otherwise
/// the site is decided on `get`'s body as before (top-down), whose calls then come up here again.
pub fn inline_calls_ordered(options: &relax_session::Options, f: &mut Func, self_id: u32, get: &dyn Fn(u32) -> Option<(*const Func, u8)>, prepared: &dyn Fn(u32) -> Option<(*const Func, *const Func)>, single: &dyn Fn(u32, u32) -> bool, nounwind: &dyn Fn(u32) -> bool, lim: &InlineLimits, freqs: Option<&[f32]>, mut stats: Option<&mut InlineStats>) -> Vec<(u32, usize)> {
    use std::cmp::Reverse;
    use std::collections::BinaryHeap;
    let depths = relax_backend::regalloc::loop_depths(f, &reachable(f));
    let freqs = freqs.filter(|v| v.len() == f.blocks.len());
    // PGO: a counted body's frequencies are per entry (profile::inline_freqs: 0.1 per run per call)
    let counted = freqs.is_some() && relax_backend::profile::has_counts(f);
    let mut priorities: Vec<(u32, bool, bool)> = depths.iter().enumerate().map(|(b, d)| {
        freqs.map_or((*d, *d >= 1 && f.blocks.len() <= 64, false), |v| (v[b].max(0.0).to_bits(), v[b] >= HOT_FREQ, v[b] == 0.0))
    }).collect();
    // PGO: a counted body's loop sites are the blocks the counts saw repeat per entry, not every
    // block a loop holds (a loop that never iterates gets no bottom-up preparation)
    let mut loop_sites: Vec<bool> = relax_backend::profile::repeated_blocks(f).unwrap_or_else(|| depths.iter().map(|d| *d > 0).collect());
    let mut ancestry = vec![vec![self_id]; f.blocks.len()];
    let mut multi_depths = vec![0; f.blocks.len()];
    let mut queue = BinaryHeap::new();
    let mut budget = lim.budget;
    let mut inst_count = inline_inst_count(f);
    let mut growth_limit = lim.growth_limit.unwrap_or_else(|| growth_ceiling(inst_count, false));
    // the size the ceiling sees: the actual size, but a priced callee counts its priced size
    let mut priced = inst_count;
    // Keep the separate raw-size free allowance.
    let mut free_budget = if opt_off(options, "-freecap") { usize::MAX } else { InlineLimits::tier1().budget };
    let mut inlined = Vec::new();
    if let Some(s) = stats.as_deref_mut() {
        s.growth_before.get_or_insert(inst_count);
        s.hot_left = 0;
        s.left = [0; 5];
        s.rejected.clear();
        s.hot_rejected.clear();
    }
    // remarks of this round (a later round sees the calls the first one kept again)
    if let Some(s) = stats.as_deref_mut() {
        s.remarks.clear();
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
            if relax_backend::regalloc::loop_depths(f, &reachable(f)).iter().any(|d| *d != 0) { return true; }
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
                    let size = found.map_or(usize::MAX, |(p, _)| inline_inst_count(unsafe { &*if loop_sites[bi] { prepared(*id).map_or(p, |x| x.1) } else { p } }));
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
    // f's registers: (definitions, the constant of a single definition) -- a site's constant
    // arguments price its callee in context (specialize_for_args); kept current as bodies move in
    let mut kdef: Vec<(u32, Option<i64>)> = vec![(0, None); f.vregs.len()];
    fn note_defs(kdef: &mut Vec<(u32, Option<i64>)>, insts: &[Inst], nv: usize) {
        kdef.resize(nv, (0, None));
        for i in insts {
            let k = match i {
                Inst::Iconst(_, k) => Some(*k),
                Inst::Mov(_, a) if kdef[a.0 as usize].0 == 1 => kdef[a.0 as usize].1,
                _ => None,
            };
            i.defs(|d| {
                let e = &mut kdef[d.0 as usize];
                e.0 += 1;
                e.1 = if e.0 == 1 { k } else { None };
            });
        }
    }
    for p in &f.params { kdef[p.0 as usize].0 = 1; }
    for b in 0..f.blocks.len() { note_defs(&mut kdef, &f.blocks[b].insts, f.vregs.len()); }
    let mut specialized: std::collections::HashMap<(u32, bool, Vec<Option<i64>>), Option<Box<Func>>> = std::collections::HashMap::new();
    // MAKEPAD_RUN_INLINE_TRACE=<suffix>: every site decision of the callers whose name ends with it
    let trace = inline_trace(&f.name);
    while let Some((_, _, _, Reverse(bi), Reverse(ii))) = queue.pop() {
        let Some(Inst::Call(Callee::Fn(id), args, rets)) = f.blocks[bi].insts.get(ii) else { continue };
        let (id, args, rets) = (*id, args.clone(), rets.clone());
        let found = get(id);
        let consts: Vec<Option<i64>> = args.iter().map(|a| kdef.get(a.0 as usize).and_then(|e| if e.0 == 1 { e.1 } else { None })).collect();
        // the bottom-up body first, then the snapshot body: (inlined body, priced body); each
        // priced as written, then (constant arguments) as this site sees it
        let mut pick = None;
        // bottom-up bodies serve sites in loops (where a callee's own inlining pays per
        // iteration); elsewhere the snapshot body decides, top-down, and no preparation is made
        let bottom = if loop_sites[bi] { prepared(id) } else { None };
        let plain: Vec<(bool, *const Func, *const Func)> = bottom.map(|(b, s)| (true, b, s)).into_iter().chain(found.map(|(p, _)| (false, p, p))).collect();
        // as written first; the constant-argument pricing only for a site those refused
        let mut tries: Vec<(bool, bool, *const Func, *const Func)> = plain.iter().map(|&(u, b, s)| (u, false, b, s)).collect();
        let any_const = consts.iter().any(|c| c.is_some());
        let mut k = 0;
        // the first try's refusal (the body as written): the site's remark when nothing is taken
        let mut why: Option<SiteRemark> = None;
        let want_why = stats.is_some();
        loop {
            if k == plain.len() && any_const {
                for &(bottom_up, body, shape) in &plain {
                    let spec = specialized.entry((id, bottom_up, consts.clone())).or_insert_with(|| specialize_for_args(unsafe { &*shape }, &consts).map(Box::new));
                    if let Some(g) = spec {
                        tries.push((bottom_up, true, body, &**g as *const Func));
                    }
                }
            }
            let Some(&(_, in_context, body, shape)) = tries.get(k) else { break };
            k += 1;
            let (callee, shape) = unsafe { (&*body, &*shape) };
            let size = inline_inst_count(shape);
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
            let attrs = found.unwrap().1;
            let lim_b = pgo_site_limits(lim, counted, priorities[bi].0, shape);
            let rule = site_rule(options, shape, id, args.len(), rets.len(), attrs, priorities[bi].1, priorities[bi].2, single(*ancestry[bi].last().unwrap(), id), ancestry[bi].len() - 1, multi_depths[bi], &lim_b);
            // New admissions respect the actual growth ceiling, including ABI moves.
            let growth = size.saturating_add(args.len()).saturating_add(
                shape.blocks.iter().filter(|b| matches!(b.term, Term::Ret(_))).count().saturating_mul(rets.len())
            ).saturating_sub(1);
            let refused = matches!(f.blocks[bi].term, Term::Unreachable | Term::Resume)
                || ancestry[bi].contains(&id) || rule.is_none()
                || attrs & ATTR_COLD != 0 && !free && rule != Some(RULE_COST)
                || callee.params.len() != args.len() || callee.rets.len() != rets.len()
                || match rule.unwrap() {
                    _ if no_growth => false,
                    RULE_COST => size > free_budget || !tiny && priced.saturating_add(growth) > growth_limit,
                    RULE_SINGLE => priced.saturating_add(growth) > growth_limit,
                    _ => !free && (cost > budget || !tiny && priced > growth_limit),
                };
            if !refused {
                pick = Some((callee, rule.unwrap(), growth, size, cost, free, no_growth, in_context));
                break;
            }
            if want_why && why.is_none() {
                let mk = |why: u8, a: usize, b: usize| SiteRemark { pos: f.blocks[bi].pos.get(ii).copied().unwrap_or(0), callee: id, why, hot: priorities[bi].1, size: size as u32, a: a.min(u32::MAX as usize) as u32, b: b.min(u32::MAX as usize) as u32 };
                why = Some(if ancestry[bi].contains(&id) {
                    mk(WHY_RECURSIVE, 0, 0)
                } else if rule.is_none() {
                    mk(WHY_NO_RULE, attrs as usize, 0)
                } else if attrs & ATTR_COLD != 0 && !free && rule != Some(RULE_COST) {
                    mk(WHY_COLD_ATTR, 0, 0)
                } else if callee.params.len() != args.len() || callee.rets.len() != rets.len() {
                    mk(WHY_ARITY, 0, 0)
                } else {
                    match rule.unwrap() {
                        RULE_COST if size > free_budget => mk(WHY_FREE_SPENT, free_budget, 0),
                        RULE_COST | RULE_SINGLE => mk(WHY_GROWTH, priced.saturating_add(growth), growth_limit),
                        _ if cost > budget => mk(WHY_BUDGET, cost, budget),
                        _ => mk(WHY_GROWTH, priced, growth_limit),
                    }
                });
            }
        }
        // makepad inspect experiments (`ab <fn> b=inline:<callee>` / `noinline:`): MAKEPAD_RUN_OPT
        // tokens `+inl@<slot>` / `-inl@<slot>` force or forbid this callee's sites
        if !options.opt.is_empty() {
            let tok = |p: &str| options.opt.iter().any(|t| t.strip_prefix(p).and_then(|x| x.parse::<u32>().ok()) == Some(id));
            if tok("-inl@") {
                pick = None;
                why = Some(SiteRemark { pos: f.blocks[bi].pos.get(ii).copied().unwrap_or(0), callee: id, why: WHY_FORBIDDEN, hot: priorities[bi].1, size: 0, a: 0, b: 0 });
            } else if pick.is_none() && tok("+inl@") && !ancestry[bi].contains(&id) {
                if let Some((p, _)) = found {
                    let callee: &Func = unsafe { &*p };
                    if callee.params.len() == args.len() && callee.rets.len() == rets.len() && callee.blocks.iter().any(|b| matches!(b.term, Term::Ret(_))) {
                        let size = inline_inst_count(callee);
                        let growth = size.saturating_add(args.len()).saturating_sub(1);
                        pick = Some((callee, RULE_ALWAYS, growth, size, size.saturating_sub(args.len() + rets.len() + 3), false, true, false));
                    }
                }
            }
        }
        if pick.is_none() {
            if let Some(s) = stats.as_deref_mut() {
                let r = why.unwrap_or(SiteRemark { pos: f.blocks[bi].pos.get(ii).copied().unwrap_or(0), callee: id, why: WHY_NO_BODY, hot: priorities[bi].1, size: 0, a: 0, b: 0 });
                s.remarks.push(r);
            }
        }
        if trace {
            let fq = f32::from_bits(priorities[bi].0);
            let name = found.map_or_else(|| format!("fn#{}", id), |(p, _)| unsafe { &*p }.name.clone());
            match &pick {
                Some((c, r, g, sz, cost, free, _, ic)) => eprintln!("inline-trace {}: take {} freq {:.2} rule {} size {} (body {}) growth {} cost {} free {} ctx {} | budget {} priced {} limit {}", f.name, name, fq, r, sz, inline_inst_count(c), g, cost, free, ic, budget, priced, growth_limit),
                None => eprintln!("inline-trace {}: left {} freq {:.2} tries {} work/size {:?} | budget {} priced {} limit {}", f.name, name, fq, tries.len(), found.and_then(|(p, _)| work_per_size(unsafe { &*p })), budget, priced, growth_limit),
            }
        }
        let Some((callee, rule, growth, size, cost, free, no_growth, in_context)) = pick else { continue };
        let callee = Some(callee);
        let rule = Some(rule);
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
            s.in_context += usize::from(in_context);
            s.rules[rule.unwrap()] += 1;
            s.hot_inlined += usize::from(priorities[bi].1);
        }
        let start = f.blocks.len();
        let old_block_size = f.blocks[bi].insts.len();
        inline_site(f, bi, ii, callee.unwrap(), &args, &rets, nounwind);
        // the call's results lose its definition; the parameter moves and the body add theirs
        // (block `start` is the continuation: instructions that were there already)
        for r in &rets { kdef[r.0 as usize].0 = kdef[r.0 as usize].0.saturating_sub(1); }
        let nv = f.vregs.len();
        note_defs(&mut kdef, &f.blocks[bi].insts[ii..], nv);
        for b in start + 1..f.blocks.len() { note_defs(&mut kdef, &f.blocks[b].insts, nv); }
        let before = inst_count;
        inst_count = inst_count - old_block_size + f.blocks[bi].insts.len()
            + f.blocks[start..].iter().map(|b| b.insts.len()).sum::<usize>();
        // the ceiling counts what moved in (a bottom-up body's unoptimised size: its priced size
        // let collect_hybrid_decode_run take prepared bodies past the room top-down had, llm_host
        // +2.1%), except a body priced in context (its constants fold it right away)
        priced = if in_context { priced.saturating_add(growth) } else { priced + inst_count - before };
        priorities.resize(f.blocks.len(), priorities[bi]);
        let parent_loop = loop_sites[bi];
        loop_sites.resize(f.blocks.len(), parent_loop);
        if !parent_loop {
            let cf = callee.unwrap();
            for (cb, depth) in relax_backend::regalloc::loop_depths(cf, &reachable(cf)).iter().enumerate() {
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
    if let Some(s) = stats.as_deref_mut().filter(|s| !s.remarks_only) {
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
                    let lim_b = pgo_site_limits(lim, counted, priorities[bi].0, c);
                    let rule = site_rule(options, c, *id, args.len(), rets.len(), attrs, priorities[bi].1, priorities[bi].2, single(*ancestry[bi].last().unwrap(), *id), ancestry[bi].len() - 1, multi_depths[bi], &lim_b);
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
                let hot = (priorities[bi].1 && reason != 0).then(|| callee.map_or(0, inline_inst_count));
                s.reject(callee.map_or_else(|| format!("fn#{}", id), |c| c.name.clone()), reason, hot);
            }
        }
    }
    inlined
}


/// The caller's by-value argument copies (`Slot::arg`) handed to a call that is being inlined stop
/// being a call's copy: they become the inlined body's parameter memory, which the body may keep
/// pointers to past what was the call (`format!("{:?}", a)` stores `&a` in an `fmt::Argument` that
/// a later call reads). frame::slot_sets would otherwise read the one call the address now goes to
/// as the copy's owner and colour the copy onto a slot live while the pointer is still read.
fn release_arg_copies(f: &mut Func, args: &[VReg]) {
    if !f.slots.iter().any(|s| s.arg) {
        return;
    }
    // vreg -> the argument copy it addresses: SlotAddr, moves and address arithmetic
    let mut of: Vec<u32> = vec![u32::MAX; f.vregs.len()];
    let mut changed = true;
    while changed {
        changed = false;
        for b in &f.blocks {
            for i in &b.insts {
                let (d, s) = match i {
                    Inst::SlotAddr(d, s) if f.slots[*s as usize].arg => (*d, *s),
                    Inst::Mov(d, a) | Inst::IBinI(_, _, d, a, _) if of[a.0 as usize] != u32::MAX => (*d, of[a.0 as usize]),
                    _ => continue,
                };
                if of[d.0 as usize] != s {
                    of[d.0 as usize] = s;
                    changed = true;
                }
            }
        }
    }
    for a in args {
        let s = of[a.0 as usize];
        if s != u32::MAX {
            f.slots[s as usize].arg = false;
        }
    }
}

fn inline_site(f: &mut Func, bi: usize, ii: usize, callee: &Func, args: &[VReg], rets: &[VReg], nounwind: &dyn Fn(u32) -> bool) {
    release_arg_copies(f, args);
    for c in &callee.sret_callees {
        if !f.sret_callees.contains(c) {
            f.sret_callees.push(*c);
        }
    }
    for c in &callee.nocapture {
        if !f.nocapture.contains(c) {
            f.nocapture.push(c.clone());
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
    // (the code after the call runs as often as the call)
    let call_count = blk.count;
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
    f.blocks.push(Block { insts: tail, pos: tail_pos, term: old_term, term_pos: old_term_pos, count: call_count });
    // the inline stack: the callee's body runs in a new context under the call's (its own
    // contexts follow, renumbered); the callee is named by a position in its own body
    let root = f.inl.len() as u64 + 1;
    let anchor = callee.blocks.iter().flat_map(|b| b.pos.iter().copied().chain(std::iter::once(b.term_pos))).find(|&p| p != 0 && pos_ctx(p) == 0).unwrap_or(0);
    f.inl.push((call_pos, anchor));
    let base = f.inl.len() as u64;
    // a callee position in the caller: its own body (context 0) under the new context, its
    // context c (callee.inl[c - 1]) at f.inl[base + c - 1]
    let ctx_of = move |p: u64| -> u64 {
        match pos_ctx(p) {
            _ if p == 0 => 0,
            0 => pos_with_ctx(p, root),
            POS_CTX_LOST => p,
            c => pos_with_ctx(p, base + c),
        }
    };
    for &(cp, a) in &callee.inl {
        f.inl.push((ctx_of(cp), a));
    }
    // map callee vregs and slots
    let mut vmap = Vec::with_capacity(callee.vregs.len());
    for c in &callee.vregs {
        vmap.push(f.vreg(*c));
    }
    for v in &callee.nonnull {
        f.nonnull.push(vmap[v.0 as usize]);
    }
    for &(v, n, lo, hi) in &callee.ranged {
        f.ranged.push((vmap[v.0 as usize], n, lo, hi));
    }
    let slot_base = f.slots.len() as u32;
    // the callee's named locals, in its new contexts (side metadata: makepad inspect `locals`)
    for d in &callee.dbg {
        let c = if d.ctx == 0 { root } else { base + d.ctx as u64 };
        if c >= POS_CTX_LOST {
            continue;
        }
        let mv = |v: VReg| if v == DBG_NONE { DBG_NONE } else { vmap[v.0 as usize] };
        let loc = match &d.loc {
            DbgLoc::Regs(r) => DbgLoc::Regs(r.iter().map(|v| mv(*v)).collect()),
            DbgLoc::Slot(s, o) if *s != u32::MAX => DbgLoc::Slot(slot_base + s, *o),
            DbgLoc::Slot(s, o) => DbgLoc::Slot(*s, *o),
            DbgLoc::Mem(v, o) => DbgLoc::Mem(mv(*v), *o),
        };
        f.dbg.push(DbgVar { ctx: c as u16, local: d.local, loc, k: d.k.clone() });
    }
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
    // PGO: a counted callee's block counts, scaled from its own entries to this call's
    let scale = (call_count >= 0.0 && callee.blocks.first().is_some_and(|b| b.count > 0.0)).then(|| call_count / callee.blocks[0].count);
    let scaled = |c: f32| match scale {
        Some(k) if c >= 0.0 => c * k,
        _ => -1.0,
    };
    for cb in &callee.blocks {
        let mut insts = Vec::with_capacity(cb.insts.len());
        for x in &cb.insts {
            insts.push(map_inst(x, &m, slot_base));
        }
        let mut pos: Vec<u64> = cb.pos.iter().map(|&p| ctx_of(p)).collect();
        let mut term = match &cb.term {
            Term::Jump(t) => Term::Jump(t + boff),
            Term::Branch(c, t, e) => Term::Branch(m(*c), t + boff, e + boff),
            Term::Ret(vals) => {
                for (k, v) in vals.iter().enumerate() {
                    insts.push(Inst::Mov(rets[k], m(*v)));
                    pos.push(ctx_of(cb.term_pos));
                }
                Term::Jump(cont)
            }
            Term::Unreachable => Term::Unreachable,
            Term::Switch(..) => unreachable!("Switch is made after optimisation"),
            Term::Invoke(t, e) => Term::Invoke(t + boff, e + boff),
            // the callee's cleanup is done: unwinding goes on in the caller, at the
            // invoke's pad if the call was invoked
            Term::Resume => match unwind {
                Some(u) => Term::Jump(u),
                None => Term::Resume,
            },
        };
        let mut term_pos = ctx_of(cb.term_pos);
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
                    extra.push(Block { insts: ik, pos: pk, term: Term::Invoke(first_extra + k as u32 + 1, u), term_pos: tp, count: scaled(cb.count) });
                }
                extra.push(Block { insts: rest_i, pos: rest_p, term: orig_term, term_pos: orig_pos, count: scaled(cb.count) });
            }
        }
        f.blocks.push(Block { insts, pos, term, term_pos, count: scaled(cb.count) });
    }
    f.blocks.extend(extra);
}

/// Instructions with effects beyond their result vregs (memory writes, calls, ordering).
pub fn is_mem_effect_pub(i: &Inst) -> bool {
    is_mem_effect(i)
}

pub fn set_sincos(wide: bool, sin: Sym, cos: Sym, both: Sym) {
    SINCOS.lock().unwrap_or_else(|e| e.into_inner())[wide as usize] = Some((sin.id, cos.id, both));
}

/// The host memcpy / memmove addresses (the runtime registers them once): calls to them
/// with a known small size fold to inline copies.
pub fn set_mem_fns(memcpy: Sym, memmove: Sym) {
    let _ = MEM_FNS.set((memcpy, memmove));
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
    let reach = relax_backend::regalloc::reachable(f);
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

pub fn inst_count(f: &Func) -> usize {
    let mut n = 0;
    for b in &f.blocks {
        n += b.insts.len() + 1;
    }
    n
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

/// How `f` uses the pointer it gets as parameter `k`, as LLVM's capture tracking with `returned`:
/// None when it may keep a copy (a derived value stored to memory, used as an indirect callee, or
/// passed to a call `sub` cannot vouch for); Some(r) when it keeps none, `r` when a value derived
/// from it may be returned. `sub(callee, arg)` answers the same for the calls it is passed to: a
/// call that may return it gives a derived result. Loads, stores and copies through it are fine.
pub fn param_escape(f: &Func, k: usize, sub: &dyn Fn(&Callee, usize) -> Option<bool>) -> Option<bool> {
    let &p = f.params.get(k)?;
    let mut t = vec![false; f.vregs.len()];
    t[p.0 as usize] = true;
    let mut returned = false;
    for _ in 0..16 {
        let mut more = false;
        for b in &f.blocks {
            for i in &b.insts {
                let from = match i {
                    Inst::Mov(_, x) | Inst::IBinI(_, _, _, x, _) | Inst::Conv(_, _, x) => t[x.0 as usize],
                    Inst::IBin(_, _, _, x, y) | Inst::Sel(_, _, x, y) => t[x.0 as usize] || t[y.0 as usize],
                    Inst::Store(_, _, _, v) | Inst::StoreX(_, _, _, _, _, v) | Inst::VStore(_, _, v) | Inst::AtomicStore(_, _, _, v) | Inst::AtomicRmw(_, _, _, _, _, v) if t[v.0 as usize] => return None,
                    Inst::AtomicCas(_, _, _, _, _, e, n) if t[e.0 as usize] || t[n.0 as usize] => return None,
                    Inst::Call(c, args, _) => {
                        if matches!(c, Callee::Indirect(v) | Callee::CIndirect(v, _) if t[v.0 as usize]) || matches!(c, Callee::Asm(_)) && args.iter().any(|a| t[a.0 as usize]) {
                            return None;
                        }
                        let mut r = false;
                        for (j, a) in args.iter().enumerate() {
                            if t[a.0 as usize] {
                                r |= sub(c, j)?;
                            }
                        }
                        r
                    }
                    _ => false,
                };
                if from {
                    i.defs(|d| {
                        if !t[d.0 as usize] {
                            t[d.0 as usize] = true;
                            more = true;
                        }
                    });
                }
            }
            if let Term::Ret(vals) = &b.term {
                returned |= vals.iter().any(|v| t[v.0 as usize]);
            }
        }
        if !more {
            return Some(returned);
        }
    }
    None
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
                    // f32 lanes too (`a.0[0]` of a core::arch __m128: _mm_cvtss_f32, _mm_add_ss)
                    access(p, o, mem_size(m) as u32, matches!(m, Mem::Int(1 | 2 | 4 | 8, _) | Mem::F32));
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
                Inst::Load(Mem::F32, d, b, o) if cell(b, o).is_some() => {
                    let (v, off) = cell(b, o).unwrap();
                    out.push(Inst::VExtract(d, v, off / 4));
                }
                Inst::Store(Mem::F32, b, o, x) if cell(b, o).is_some() => {
                    let (v, off) = cell(b, o).unwrap();
                    out.push(Inst::VInsert(v, v, off / 4, x));
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

/// Post-inline sroa (O3, after the last inlining round): groups copied both in and out
/// are tiled (LLVM's whole-alloca slicing). Not at O1: tiling a callee's copies grows the
/// body the inliner sizes (tsr's RangeRecord `get` stopped inlining: +40% at O3).
pub fn sroa_post_inline(f: &mut Func) -> bool {
    sroa_with(f, true)
}

/// Scalar promotion of loop-carried memory (LLVM's LICM `promoteLoopAccessesToScalars`): a field
/// of the pointee of a `noalias` pointer parameter (`&mut Self`) that a loop loads and stores
/// each iteration lives in a register for the whole loop: loaded once in the preheader, kept in a
/// fresh vreg, stored back on every edge that leaves the loop.
///
/// Legality, as LLVM decides it:
/// * the pointer is `noalias` and not captured: no pointer that is not based on it (derived from it
///   by moves and address arithmetic, or returned by a call that was given one) reaches its pointee,
///   and it is never stored as a value; so nothing in the loop but accesses through the parameter
///   itself can touch the field;
/// * every access in the loop through a pointer based on the parameter is a plain load or store
///   through the parameter, and every one overlapping the field has its exact offset and type;
/// * no call, atomic or fence in the loop for a stored field (a call may unwind with the memory
///   stale; a store is not delayed past one), calls are fine for a field that is only loaded when
///   none is given a based-on pointer, and for a stored field of a by-value argument's private copy
///   (`Func::owned`) when the loop has no Invoke (LLVM's `isNotVisibleOnUnwind`);
/// * the field is dereferenceable in the preheader (`Func::deref`), so loading it there is safe
///   even if the loop runs no iteration; the write-back on a path that never stored writes the
///   value it loaded, which no one can observe through a unique pointer.
/// Most fields promoted in one loop, and most exit write-back stores (exit targets x stored fields).
const PROMOTE_MAX_FIELDS: usize = 4;
const PROMOTE_MAX_WRITEBACKS: usize = 8;

pub fn promote_loop_memory(f: &mut Func) -> bool {
    if forward_only(f) || f.noalias.is_empty() {
        return false;
    }
    let mut changed = false;
    let mut done: std::collections::HashSet<u32> = std::collections::HashSet::new();
    loop {
        let (preds, loops) = natural_loops(f);
        let mut progressed = false;
        for (h, body) in &loops {
            if *h == 0 || done.contains(h) {
                continue;
            }
            done.insert(*h);
            if promote_in_loop(f, &preds, *h, body) {
                changed = true;
                progressed = true;
                break;
            }
        }
        if !progressed {
            break;
        }
    }
    changed
}

fn promote_in_loop(f: &mut Func, preds: &[Vec<u32>], h: u32, body: &[bool]) -> bool {
    let n = f.blocks.len();
    let nv = f.vregs.len();
    // vregs with one definition, the parameters among them
    let mut ndef = vec![0u32; nv];
    for p in &f.params {
        ndef[p.0 as usize] += 1;
    }
    for b in &f.blocks {
        for i in &b.insts {
            for_defs(i, |d| ndef[d.0 as usize] += 1);
        }
    }
    // anything in the loop that stops every promotion, and what stops only stored fields
    let mut atomic_or_fence = false;
    let mut has_call = false;
    let mut has_invoke = false;
    for bi in 0..n {
        if !body[bi] {
            continue;
        }
        has_invoke |= matches!(f.blocks[bi].term, Term::Invoke(..));
        for i in &f.blocks[bi].insts {
            match i {
                Inst::AtomicLoad(..) | Inst::AtomicStore(..) | Inst::AtomicRmw(..) | Inst::AtomicCas(..) | Inst::Fence(..) | Inst::FpEnv(..) => atomic_or_fence = true,
                Inst::Call(..) => has_call = true,
                _ => {}
            }
        }
    }
    if atomic_or_fence {
        return false;
    }
    let mut did = false;
    for pi in 0..f.noalias.len() {
        let p = f.noalias[pi];
        // defined once, as a parameter
        if ndef[p.0 as usize] != 1 || !f.params.contains(&p) {
            continue;
        }
        // MAKEPAD_RUN_OPT=-owned: by-value argument copies as any noalias param (bisection)
        let owned = f.owned.contains(&p) && !relax_backend::stats::opt_env_off("-owned");
        // the vregs based on p: moves and arithmetic on one, and call results when a based-on
        // pointer went into the call
        let mut based = vec![false; nv];
        based[p.0 as usize] = true;
        let mut grew = true;
        while grew {
            grew = false;
            for b in &f.blocks {
                for i in &b.insts {
                    if matches!(i, Inst::Load(..) | Inst::LoadX(..) | Inst::AtomicLoad(..)) {
                        continue;
                    }
                    let mut any = false;
                    for_uses(i, |u| any |= based[u.0 as usize]);
                    if any {
                        for_defs(i, |d| {
                            if !based[d.0 as usize] {
                                based[d.0 as usize] = true;
                                grew = true;
                            }
                        });
                    }
                }
            }
        }
        // captured: stored as a value, or given to a call that may keep or return it
        let mut captured = false;
        for b in &f.blocks {
            for i in &b.insts {
                match i {
                    Inst::Store(_, _, _, src) | Inst::StoreX(_, _, _, _, _, src) | Inst::VStore(_, _, src) | Inst::AtomicStore(_, _, _, src) => captured |= based[src.0 as usize],
                    Inst::AtomicRmw(_, _, _, _, _, src) => captured |= based[src.0 as usize],
                    Inst::AtomicCas(_, _, _, _, _, e, nw) => captured |= based[e.0 as usize] || based[nw.0 as usize],
                    Inst::Call(c, args, _) => {
                        captured |= args.iter().enumerate().any(|(k, a)| based[a.0 as usize] && !sroa_nocapture(c, k));
                        if let Callee::Indirect(v) | Callee::CIndirect(v, _) = c {
                            captured |= based[v.0 as usize];
                        }
                    }
                    _ => {}
                }
            }
            if let Term::Ret(vs) = &b.term {
                captured |= vs.iter().any(|v| based[v.0 as usize]);
            }
        }
        if captured {
            continue;
        }
        // the accesses through p in the loop, and whether anything else based on p is used there
        let mut fields: Vec<(i32, Mem, bool, bool, u32)> = Vec::new(); // offset, type, loaded, stored, accesses
        let mut bad = false;
        for bi in 0..n {
            if !body[bi] {
                continue;
            }
            for i in &f.blocks[bi].insts {
                match i {
                    Inst::Load(m, _, base, off) if *base == p => {
                        match fields.iter_mut().find(|x| x.0 == *off && x.1 == *m) {
                            Some(x) => {
                                x.2 = true;
                                x.4 += 1;
                            }
                            None => fields.push((*off, *m, true, false, 1)),
                        }
                    }
                    Inst::Store(m, base, off, _) if *base == p => {
                        match fields.iter_mut().find(|x| x.0 == *off && x.1 == *m) {
                            Some(x) => {
                                x.3 = true;
                                x.4 += 1;
                            }
                            None => fields.push((*off, *m, false, true, 1)),
                        }
                    }
                    Inst::Call(_, args, _) => bad |= args.iter().any(|a| based[a.0 as usize]),
                    // any other dereference of a based-on pointer
                    Inst::Load(_, _, base, _) | Inst::Store(_, base, _, _) | Inst::VLoad(_, base, _) | Inst::VStore(base, _, _) | Inst::VGather(_, base, _) | Inst::VTbl(_, base, _, _) => bad |= based[base.0 as usize],
                    Inst::Copy(d, s, _) => bad |= based[d.0 as usize] || based[s.0 as usize],
                    _ => {}
                }
            }
        }
        if bad {
            continue;
        }
        // a field is promoted when nothing else in the loop overlaps its bytes
        let size = |m: Mem| mem_size(m);
        let mut take: Vec<(i32, Mem, bool, bool, u32)> = Vec::new();
        for x in &fields {
            let clash = fields.iter().any(|y| !(y.0 == x.0 && y.1 == x.1) && y.0 < x.0 + size(x.1) && x.0 < y.0 + size(y.1));
            // a stored field stays stale in memory over a call: fine for the private copy of a
            // by-value argument (Func::owned) when every call is plain -- an unwind leaves this
            // function at once and nobody reads the copy after (an Invoke's cleanup might)
            if clash || (x.3 && has_call && !(owned && !has_invoke)) {
                continue;
            }
            // dereferenceable in the preheader
            if !f.deref.iter().any(|(q, sz)| *q == p && x.0 >= 0 && (x.0 as i64 + size(x.1) as i64) <= *sz as i64) {
                continue;
            }
            take.push(*x);
        }
        // profitability: a loop is worth it for a field it both loads and stores (the carried state:
        // a cursor index, a bit buffer); load-only fields come along, write-only ones are left
        // Load-only fields alone are LLVM LICM's invariant-load hoisting: a Vec's pointer and length
        // read through `&mut self` in a loop that writes only through the heap pointer (vorbis
        // `self.spectra[i] *= self.curves[i]` reloaded both headers every iteration).
        // MAKEPAD_RUN_OPT=-promoteld: carried fields only.
        if !take.iter().any(|x| x.2 && x.3) && (relax_backend::stats::opt_env_off("-promoteld") || !take.iter().any(|x| x.2)) {
            continue;
        }
        take.retain(|x| x.2);
        // the busiest few fields only (every promoted field is a register held across
        // the loop), and stored fields only when the write-backs on the loop's exits stay few
        take.sort_by(|a, b| b.4.cmp(&a.4));
        take.truncate(PROMOTE_MAX_FIELDS);
        let mut exit_targets: Vec<u32> = Vec::new();
        for bi in 0..n {
            if body[bi] {
                for t in f.blocks[bi].term.succs() {
                    if !body[t as usize] && !exit_targets.contains(&t) {
                        exit_targets.push(t);
                    }
                }
            }
        }
        // a by-value argument's private copy (Func::owned) needs no write-back on an exit that
        // never returns (a bounds-check panic: it unwinds out of the function or aborts) and whose
        // code never looks at the copy: nobody reads it after
        if owned {
            let quiet_dead_end = |t: u32| -> bool {
                let mut seen = vec![false; n];
                let mut work = vec![t];
                while let Some(x) = work.pop() {
                    let x = x as usize;
                    if seen[x] {
                        continue;
                    }
                    seen[x] = true;
                    let blk = &f.blocks[x];
                    if matches!(blk.term, Term::Ret(..)) || body[x] {
                        return false;
                    }
                    let mut touches = false;
                    for i in &blk.insts {
                        for_uses(i, |u| touches |= based[u.0 as usize]);
                    }
                    if touches {
                        return false;
                    }
                    work.extend(blk.term.succs());
                }
                true
            };
            exit_targets.retain(|&t| !quiet_dead_end(t));
        }
        let mut stored_n = 0;
        take.retain(|x| {
            if x.3 {
                stored_n += 1;
                exit_targets.len() * stored_n <= PROMOTE_MAX_WRITEBACKS
            } else {
                true
            }
        });
        if take.is_empty() {
            continue;
        }
        // a preheader: all edges into the header from outside the loop go through one jump block
        let outside: Vec<u32> = preds[h as usize].iter().copied().filter(|q| !body[*q as usize]).collect();
        if outside.is_empty() {
            continue;
        }
        let pre = f.blocks.len() as u32;
        let hp = f.blocks[h as usize].insts.first().map(|_| f.blocks[h as usize].pos[0]).unwrap_or(f.blocks[h as usize].term_pos);
        f.blocks.push(Block { insts: Vec::new(), pos: Vec::new(), term: Term::Jump(h), term_pos: hp, count: -1.0 });
        for &o in &outside {
            let t = &mut f.blocks[o as usize].term;
            retarget(t, h, pre);
        }
        // the new vregs, their initial loads, the rewritten accesses
        let mut regs: Vec<VReg> = Vec::new();
        for x in &take {
            let cls = match x.1 {
                Mem::Int(..) => Cls::I,
                Mem::F32 => Cls::F32,
                Mem::F64 => Cls::F64,
            };
            let r = f.vreg(cls);
            regs.push(r);
            f.blocks[pre as usize].insts.push(Inst::Load(x.1, r, p, x.0));
            f.blocks[pre as usize].pos.push(hp);
        }
        for bi in 0..n {
            if !body[bi] {
                continue;
            }
            for k in 0..f.blocks[bi].insts.len() {
                let rewrite = match &f.blocks[bi].insts[k] {
                    Inst::Load(m, d, base, off) if *base == p => take.iter().position(|x| x.0 == *off && x.1 == *m).map(|t| Inst::Mov(*d, regs[t])),
                    Inst::Store(m, base, off, src) if *base == p => take.iter().position(|x| x.0 == *off && x.1 == *m).map(|t| match m {
                        Mem::Int(w, signed) if *w < 8 => Inst::Conv(Conv::IntToInt(IntTy { bits: *w * 8, signed: *signed }), regs[t], *src),
                        _ => Inst::Mov(regs[t], *src),
                    }),
                    _ => None,
                };
                if let Some(i) = rewrite {
                    f.blocks[bi].insts[k] = i;
                }
            }
        }
        // write-back of the stored fields on every edge out of the loop: one block per distinct
        // target (LoopSimplify's dedicated exits), the loop's edges to it redirected
        let stored: Vec<usize> = (0..take.len()).filter(|t| take[*t].3).collect();
        if !stored.is_empty() {
            for &t in &exit_targets {
                let x = f.blocks.len() as u32;
                let tp = f.blocks.iter().enumerate().find(|(bi, b)| body[*bi] && b.term.succs().any(|s| s == t)).map(|(_, b)| b.term_pos).unwrap_or(0);
                let ins: Vec<Inst> = stored.iter().map(|&k| Inst::Store(take[k].1, p, take[k].0, regs[k])).collect();
                let np = ins.len();
                f.blocks.push(Block { insts: ins, pos: vec![tp; np], term: Term::Jump(t), term_pos: tp, count: -1.0 });
                for bi in 0..n {
                    if body[bi] {
                        retarget(&mut f.blocks[bi].term, t, x);
                    }
                }
            }
        }
        did = true;
        // the loop's structure changed (new blocks): the caller recomputes before the next field
        break;
    }
    did
}

/// Redirects every edge of `term` to `from` to `to`.
fn retarget(term: &mut Term, from: u32, to: u32) {
    match term {
        Term::Jump(t) if *t == from => *t = to,
        Term::Branch(_, a, b) => {
            if *a == from {
                *a = to;
            }
            if *b == from {
                *b = to;
            }
        }
        Term::Invoke(a, b) => {
            if *a == from {
                *a = to;
            }
            if *b == from {
                *b = to;
            }
        }
        _ => {}
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
        f.dbg_map_slots(&|s| match to.get(s as usize) {
            Some(&(a, None)) if a != NONE => Some(a),
            _ => Some(s),
        });
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

/// Copies of a value the same block just stored into a temporary: `Copy(d, t, n)` out of a
/// non-escaping slot whose copied bytes the stores in front of it (same block) cover exactly
/// becomes those stores' values stored at `d` (a value built in a temporary and moved into
/// place: `[SimplexVertex::default(); 4]`, `vs[i] = Vertex { .. }`, a struct literal assigned
/// to a field). The bytes are no longer read back through memory -- each 16-byte piece of the
/// copy, read behind the narrower stores that just wrote it, failed store forwarding -- and the
/// temporary's stores are left for dead_slot_stores. At most 16 stores per copy; a store
/// reaching outside the copied bytes, an indexed or unknown write of the temporary, a value
/// redefined before the copy, or a byte not stored in the block keeps the copy.
pub fn stores_for_copies(f: &mut Func) -> bool {
    if f.slots.is_empty() {
        return false;
    }
    let SlotScan { vs, esc, .. } = slot_scan(f);
    const NONE: u32 = u32::MAX;
    const MAX_STORES: usize = 16;
    let sl = |v: VReg| vs[v.0 as usize].0;
    let mut any = false;
    for bi in 0..f.blocks.len() {
        let mut ii = 0;
        while ii < f.blocks[bi].insts.len() {
            let Inst::Copy(d, t, n) = f.blocks[bi].insts[ii] else { ii += 1; continue };
            let ts = sl(t);
            if ts == NONE || esc[ts as usize] || sl(d) == ts || n == 0 || n > 4096 {
                ii += 1;
                continue;
            }
            let (lo, hi) = (vs[t.0 as usize].1, vs[t.0 as usize].1 + n as i64);
            // backwards from the copy: the latest store of every copied byte
            let mut covered = vec![false; n as usize];
            let mut left = n as usize;
            let mut taken: Vec<(i64, Inst)> = Vec::new();
            let mut defs: Vec<VReg> = Vec::new();
            let mut ok = false;
            for j in (0..ii).rev() {
                let i = &f.blocks[bi].insts[j];
                let st = match *i {
                    Inst::Store(m, base, off, v) if sl(base) == ts => Some((vs[base.0 as usize].1 + off as i64, mem_size(m) as i64, v)),
                    Inst::VStore(base, off, v) if sl(base) == ts => Some((vs[base.0 as usize].1 + off as i64, if f.vregs[v.0 as usize] == Cls::V256 { 32 } else { 16 }, v)),
                    Inst::StoreX(_, base, ..) | Inst::VStoreX(base, ..) if sl(base) == ts => break,
                    Inst::Copy(d2, _, _) if sl(d2) == ts => break,
                    _ => None,
                };
                if let Some((a, sz, v)) = st {
                    let b = a + sz;
                    if b <= lo || a >= hi {
                        // outside the copied bytes
                    } else if a < lo || b > hi {
                        break;
                    } else {
                        let r = (a - lo) as usize..(b - lo) as usize;
                        let fresh = covered[r.clone()].iter().filter(|c| !**c).count();
                        if fresh == r.len() {
                            if defs.contains(&v) || taken.len() == MAX_STORES {
                                break;
                            }
                            let rel = (a - lo) as i32;
                            let ni = match *i {
                                Inst::Store(m, ..) => Inst::Store(m, d, rel, v),
                                _ => Inst::VStore(d, rel, v),
                            };
                            taken.push((a, ni));
                            covered[r].iter_mut().for_each(|c| *c = true);
                            left -= fresh;
                            if left == 0 {
                                ok = true;
                                break;
                            }
                        } else if fresh != 0 {
                            // partly overwritten by a later store
                            break;
                        }
                    }
                }
                for_defs(i, |x| defs.push(x));
            }
            if !ok {
                ii += 1;
                continue;
            }
            taken.sort_by_key(|x| x.0);
            let k = taken.len();
            let pos = f.blocks[bi].pos.get(ii).copied();
            let blk = &mut f.blocks[bi];
            blk.insts.splice(ii..ii + 1, taken.into_iter().map(|x| x.1));
            if let Some(p) = pos {
                blk.pos.splice(ii..ii + 1, std::iter::repeat(p).take(k));
            }
            ii += k;
            any = true;
        }
    }
    any
}

/// Wide integer accesses of a stack slot that other accesses of the slot cut: a pair of
/// `u32` fields stored one by one and moved as one `u64` (or `u16` halves of a `u32`) makes
/// the slot's overlapping cells unpromotable (sroa: one cell per offset and size) and, left in
/// memory, the wide load fails store forwarding behind the narrow stores. A load of `S` bytes
/// becomes two loads of `S/2` joined with a shift and an or, a store becomes two stores of the
/// value and of the value shifted down, when an access of the same non-escaping slot lies
/// inside one half. Repeats on the halves. Returns whether anything was split.
pub fn split_wide_slot_accesses(f: &mut Func) -> bool {
    const NONE: u32 = u32::MAX;
    let mut any = false;
    for _round in 0..3 {
        let sc = slot_scan(f);
        let ns = f.slots.len();
        // per slot: the (offset, size) of every direct access; slots touched any other way
        // (vector or indexed accesses) are left alone
        let mut acc: Vec<Vec<(i64, i64)>> = vec![Vec::new(); ns];
        let mut odd = vec![false; ns];
        let at = |v: VReg| sc.vs[v.0 as usize];
        for b in &f.blocks {
            for i in &b.insts {
                match i {
                    Inst::Load(m, _, base, off) | Inst::Store(m, base, off, _) => {
                        let (sl, o) = at(*base);
                        if sl != NONE {
                            acc[sl as usize].push((o + *off as i64, mem_size(*m) as i64));
                        }
                    }
                    Inst::VLoad(_, base, _) | Inst::VStore(base, ..) | Inst::LoadX(_, _, base, ..) | Inst::StoreX(_, base, ..) => {
                        let (sl, _) = at(*base);
                        if sl != NONE {
                            odd[sl as usize] = true;
                        }
                    }
                    _ => {}
                }
            }
        }
        // is a wide access (offset, size) of this slot cut by another access inside one half?
        let cut = |sl: usize, o: i64, n: i64| -> bool {
            let h = n / 2;
            acc[sl].iter().any(|&(ao, an)| an < n && ao >= o && ao + an <= o + n && (ao + an <= o + h || ao >= o + h))
        };
        let mut changed = false;
        for bi in 0..f.blocks.len() {
            let wide = |i: &Inst| -> bool {
                let (m, base, off) = match i {
                    Inst::Load(m @ Mem::Int(4 | 8, false), _, base, off) | Inst::Store(m @ Mem::Int(4 | 8, _), base, off, _) => (*m, *base, *off),
                    _ => return false,
                };
                let (sl, o) = at(base);
                sl != NONE && !sc.esc[sl as usize] && !odd[sl as usize] && cut(sl as usize, o + off as i64, mem_size(m) as i64)
            };
            if !f.blocks[bi].insts.iter().any(|i| wide(i)) {
                continue;
            }
            let old = std::mem::take(&mut f.blocks[bi].insts);
            let oldp = std::mem::take(&mut f.blocks[bi].pos);
            let keep_pos = oldp.len() == old.len();
            let mut ins = Vec::with_capacity(old.len() + 8);
            let mut pos = Vec::with_capacity(old.len() + 8);
            for (k, i) in old.into_iter().enumerate() {
                let p = if keep_pos { oldp[k] } else { 0 };
                if !wide(&i) {
                    ins.push(i);
                    pos.push(p);
                    continue;
                }
                match i {
                    Inst::Load(Mem::Int(n, _), d, base, off) => {
                        let h = n / 2;
                        let (lo, hi, up) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
                        ins.push(Inst::Load(Mem::Int(h, false), lo, base, off));
                        ins.push(Inst::Load(Mem::Int(h, false), hi, base, off + h as i32));
                        ins.push(Inst::IBinI(IOp::Shl, U64, up, hi, h as i64 * 8));
                        ins.push(Inst::IBin(IOp::Or, U64, d, lo, up));
                        pos.extend([p; 4]);
                    }
                    Inst::Store(Mem::Int(n, _), base, off, v) => {
                        let h = n / 2;
                        let up = f.vreg(Cls::I);
                        ins.push(Inst::Store(Mem::Int(h, false), base, off, v));
                        ins.push(Inst::IBinI(IOp::Shr, U64, up, v, h as i64 * 8));
                        ins.push(Inst::Store(Mem::Int(h, false), base, off + h as i32, up));
                        pos.extend([p; 3]);
                    }
                    _ => unreachable!(),
                }
            }
            f.blocks[bi].insts = ins;
            f.blocks[bi].pos = if keep_pos { pos } else { Vec::new() };
            changed = true;
        }
        if !changed {
            break;
        }
        any = true;
    }
    any
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
                    Inst::Load(_, _, base, _) | Inst::VLoad(_, base, _) | Inst::VGather(_, base, _) | Inst::VTbl(_, base, _, _) => v == base,
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

/// Vregs defined once, as a parameter or by moves and constant offsets of one (pointers into
/// memory that existed before this frame did).
/// The vregs holding addresses into slot `s` (its SlotAddr and moves / adds of it, constant or
/// variable offsets), when every use of them is the address of a load, store or copy or makes
/// another such address: None when the address leaks (stored, passed, returned, compared, ...).
fn slot_derived(f: &Func, s: u32) -> Option<Vec<bool>> {
    let nv = f.vregs.len();
    let mut der = vec![false; nv];
    for b in &f.blocks {
        for i in &b.insts {
            if let Inst::SlotAddr(d, s2) = i {
                if *s2 == s {
                    der[d.0 as usize] = true;
                }
            }
        }
    }
    loop {
        let mut grew = false;
        for b in &f.blocks {
            for i in &b.insts {
                let (d, a, b2) = match *i {
                    Inst::Mov(d, a) | Inst::IBinI(IOp::Add | IOp::Sub, _, d, a, _) => (d, a, None),
                    Inst::IBin(IOp::Add, _, d, a, b2) => (d, a, Some(b2)),
                    _ => continue,
                };
                let from = der[a.0 as usize] || b2.map_or(false, |x| der[x.0 as usize]);
                if from && !der[d.0 as usize] {
                    der[d.0 as usize] = true;
                    grew = true;
                }
            }
        }
        if !grew {
            break;
        }
    }
    let mut tu = Vec::new();
    for b in &f.blocks {
        for i in &b.insts {
            let ok = match *i {
                Inst::Mov(..) | Inst::IBinI(IOp::Add | IOp::Sub, ..) => true,
                Inst::IBin(IOp::Add, _, _, a, b2) => !(der[a.0 as usize] && der[b2.0 as usize]),
                Inst::Load(..) | Inst::VLoad(..) => true,
                Inst::LoadX(_, _, _, x, _, _) | Inst::VLoadX(_, _, x, _, _) => !der[x.0 as usize],
                Inst::Store(_, _, _, v) | Inst::VStore(_, _, v) => !der[v.0 as usize],
                Inst::StoreX(_, _, x, _, _, v) | Inst::VStoreX(_, x, _, _, v) => !der[x.0 as usize] && !der[v.0 as usize],
                Inst::Copy(..) | Inst::SlotAddr(..) => true,
                _ => {
                    let mut bad = false;
                    for_uses(i, |u| bad |= der[u.0 as usize]);
                    !bad
                }
            };
            if !ok {
                return None;
            }
        }
        tu.clear();
        relax_backend::regalloc::term_uses(&b.term, &mut tu);
        if tu.iter().any(|v| der[v.0 as usize]) {
            return None;
        }
    }
    Some(der)
}

fn param_based_vregs(f: &Func) -> Vec<bool> {
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
    let mut pb = vec![false; nv];
    for p in &f.params {
        if ndef[p.0 as usize] == 1 && f.vregs[p.0 as usize] == Cls::I {
            pb[p.0 as usize] = true;
        }
    }
    loop {
        let mut grew = false;
        for b in &f.blocks {
            for i in &b.insts {
                let (d, a) = match *i {
                    Inst::Mov(d, a) | Inst::IBinI(IOp::Add | IOp::Sub, _, d, a, _) => (d, a),
                    _ => continue,
                };
                if pb[a.0 as usize] && !pb[d.0 as usize] && ndef[d.0 as usize] == 1 {
                    pb[d.0 as usize] = true;
                    grew = true;
                }
            }
        }
        if !grew {
            return pb;
        }
    }
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
        let mut param_based: Option<Vec<bool>> = None;
        // (B, C, offset, block, copy index)
        let mut repl: Vec<(u32, u32, i64, usize, usize)> = Vec::new();
        for b in 0..ns {
            let Some((cb, ci, c, o, n)) = the_copy[b] else { continue };
            if sc.esc[b] || multi_blk[b] || sc.reads[b] != 1 || used[b] || used[c as usize] {
                continue;
            }
            let Some((sb, first, last)) = span[b] else { continue };
            if sb != cb || last != ci || o < 0 || o + n as i64 > f.slots[c as usize].size as i64 {
                continue;
            }
            // C's address escapes (a by-value argument's copy handed to the calls of an inlined
            // loop, a closure's environment): then B's writes may land in C early only when nothing
            // from B's first access to the copy can see C through another pointer -- no call, copy
            // or atomic, and every other memory access through a slot or a pointer based on a
            // parameter (made before this frame's slots existed: it never points into them)
            if sc.esc[c as usize] && relax_backend::stats::opt_env_off("-fwdesc") {
                continue;
            }
            // C "escapes" only through address arithmetic slot_scan does not follow (a variable
            // index: `simd_body.v.x.get(lane)` in a loop) and never leaks to memory, a call or a
            // return: no other pointer can see it, only the derived addresses (checked below)
            let derived = if sc.esc[c as usize] { slot_derived(f, c) } else { None };
            if sc.esc[c as usize] && derived.is_none() {
                let pb = param_based.get_or_insert_with(|| param_based_vregs(f));
                let unseen = f.blocks[cb].insts[first..ci].iter().all(|i| match i {
                    Inst::Load(_, _, base, _) | Inst::Store(_, base, _, _) | Inst::VLoad(_, base, _) | Inst::VStore(base, _, _) | Inst::LoadX(_, _, base, ..) | Inst::StoreX(_, base, ..) => sl(*base) != NONE || pb[base.0 as usize],
                    i => !relax_backend::passes::is_mem_effect(i),
                });
                if !unseen {
                    continue;
                }
            }
            // nothing touches C's bytes [o, o+n) from B's first access up to the copy
            let mut clash = false;
            for i in &f.blocks[cb].insts[first..ci] {
                let hit = |v: VReg, off: i64, k: i64| -> bool {
                    let (s, so) = vs[v.0 as usize];
                    s == c && so + off < o + n as i64 && o < so + off + k
                };
                // an access through a derived address of unknown offset may touch any byte of C
                let unknown = |v: VReg| derived.as_ref().map_or(false, |dv| dv[v.0 as usize] && vs[v.0 as usize].0 != c);
                clash |= match i {
                    Inst::Load(_, _, base, _) | Inst::Store(_, base, _, _) | Inst::VLoad(_, base, _) | Inst::VStore(base, _, _) | Inst::LoadX(_, _, base, ..) | Inst::StoreX(_, base, ..) | Inst::VLoadX(_, base, ..) | Inst::VStoreX(base, ..) if unknown(*base) => true,
                    Inst::Copy(d, s2, _) if unknown(*d) || unknown(*s2) => true,
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
    let addressed = relax_backend::rir::used_slots(f);
    slot_colors_used(f, min, &addressed)
}

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
    f.dbg_map_slots(&|s| map.get(s as usize).copied().filter(|x| *x != u32::MAX));
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
                    Inst::Load(..) | Inst::LoadX(..) | Inst::VLoad(..) | Inst::AtomicLoad(..) | Inst::Iconst(..) | Inst::Fconst(..) | Inst::Addr(..) | Inst::FnAddr(..) | Inst::TlsAddr(..) | Inst::TlsStatic(..) | Inst::VConst(..) => NONE,
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

    #[test]
    fn split_wide_access_cut_by_narrow_stores_lets_sroa_promote() {
        // a pair of u32 stored one by one and moved as one u64
        let mut f = Func::new("pair".into(), 0);
        let (a, b) = (f.vreg(Cls::I), f.vreg(Cls::I));
        f.params.extend([a, b]);
        let s = f.slot(8, 8);
        let (p, w, d) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
        f.block();
        f.blocks[0].insts = vec![
            Inst::SlotAddr(p, s),
            Inst::Store(Mem::Int(4, false), p, 0, a),
            Inst::Store(Mem::Int(4, false), p, 4, b),
            Inst::Load(Mem::Int(8, false), w, p, 0),
            Inst::IBinI(IOp::Add, U64, d, w, 1),
        ];
        f.blocks[0].term = Term::Ret(vec![d]);
        f.rets = vec![Cls::I];
        f.blocks[0].pos = vec![0; f.blocks[0].insts.len()];
        assert!(split_wide_slot_accesses(&mut f));
        f.verify().unwrap();
        assert!(!f.blocks[0].insts.iter().any(|i| matches!(i, Inst::Load(Mem::Int(8, _), ..))));
        assert_eq!(f.blocks[0].insts.len(), f.blocks[0].pos.len());
        assert!(sroa(&mut f));
        assert!(!f.blocks[0].insts.iter().any(|i| matches!(i, Inst::Load(..) | Inst::Store(..))), "pair stayed in memory");
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
    use relax_backend::bitops::ext;

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
            let depth = relax_backend::regalloc::loop_depths(&f, &reachable(&f));
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
        assert_eq!(inline_calls_ordered(options, &mut f, 0, &get, &|_| None, &|_, _| false, &|_| false, &lim, Some(&[1.0, 0.5, 0.0]), None), vec![(1, RULE_SMALL_LEAF)]);
        lim.budget = 54;
        let mut f = caller();
        assert_eq!(inline_calls(options, &mut f, 0, &get, &|_| false, &lim).len(), 2);
        f.verify().unwrap();
    }

    /// `caller` with both calls in its loop (bottom-up bodies serve sites in loops)
    fn caller_in_loop() -> Func {
        let mut f = caller();
        f.blocks[0].insts.clear();
        f.blocks[0].pos.clear();
        f.blocks[1].insts = vec![Inst::Call(Callee::Fn(1), vec![], vec![]), Inst::Call(Callee::Fn(2), vec![], vec![])];
        f.blocks[1].pos = vec![0, 0];
        f
    }

    #[test]
    fn priced_body_decides_and_inlined_body_moves() {
        // bottom-up O3: the optimised body prices the site, the inlined-only body is inlined
        let options = &relax_session::Options::default();
        let (raw, opt) = (callee(400), callee(30));
        let get = |_: u32| Some((&raw as *const Func, 0));
        let mut f = caller_in_loop();
        let n = inline_inst_count(&f);
        assert_eq!(inline_calls_ordered(options, &mut f, 0, &get, &|_| Some((&raw as *const Func, &opt as *const Func)), &|_, _| false, &|_| false, &InlineLimits::tier1(), None, None).len(), 2);
        assert!(inline_inst_count(&f) > n + 2 * 390);
        f.verify().unwrap();
        // outside a loop the snapshot body decides (here: too big), inside one the bottom-up body
        let mut f = caller();
        assert_eq!(inline_calls_ordered(options, &mut f, 0, &get, &|_| Some((&raw as *const Func, &opt as *const Func)), &|_, _| false, &|_| false, &InlineLimits::tier1(), None, None), vec![(2, RULE_SMALL_LEAF)]);
        // refused bottom-up (priced too big), the snapshot body decides: here too big as well
        let mut f = caller_in_loop();
        assert!(inline_calls_ordered(options, &mut f, 0, &get, &|_| Some((&opt as *const Func, &raw as *const Func)), &|_, _| false, &|_| false, &InlineLimits::tier1(), None, None).is_empty());
        // ... and here small enough: the snapshot body moves
        let get = |_: u32| Some((&opt as *const Func, 0));
        let mut f = caller_in_loop();
        assert_eq!(inline_calls_ordered(options, &mut f, 0, &get, &|_| Some((&raw as *const Func, &raw as *const Func)), &|_, _| false, &|_| false, &InlineLimits::tier1(), None, None).len(), 2);
        assert!(inline_inst_count(&f) < n + 2 * 40);
    }

    /// `fn(len) { let mut i = 0; while i + 16 <= len { <work>; i += 16 } if len - i >= 8 { <tail> } }`
    fn counted_loop(work: usize) -> Func {
        let mut f = Func::new("counted".into(), 0);
        let len = f.vreg(Cls::I);
        f.params.push(len);
        let (i, t, c, r, c8, x) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
        for _ in 0..6 { f.block(); }
        let it = IntTy { bits: 64, signed: false };
        let mut set = |b: usize, insts: Vec<Inst>, term: Term| { f.blocks[b].pos = vec![0; insts.len()]; f.blocks[b].insts = insts; f.blocks[b].term = term; };
        set(0, vec![Inst::Iconst(i, 0)], Term::Jump(1));
        set(1, vec![Inst::IBinI(IOp::Add, it, t, i, 16), Inst::ICmp(Cond::Le, false, c, t, len)], Term::Branch(c, 2, 3));
        let mut body: Vec<Inst> = (0..work).map(|_| Inst::IBin(IOp::Mul, it, x, x, len)).collect();
        body.push(Inst::IBinI(IOp::Add, it, i, i, 16));
        set(2, body, Term::Jump(1));
        set(3, vec![Inst::IBin(IOp::Sub, it, r, len, i), Inst::ICmpI(Cond::Ge, false, c8, r, 8)], Term::Branch(c8, 4, 5));
        let tail: Vec<Inst> = (0..work).map(|_| Inst::IBin(IOp::Xor, it, x, x, r)).collect();
        set(4, tail, Term::Ret(vec![]));
        set(5, vec![], Term::Ret(vec![]));
        f
    }

    #[test]
    fn constant_arguments_price_in_context() {
        let f = counted_loop(200);
        // len = 4: the loop and the 8-byte tail are dead
        let g = specialize_for_args(&f, &[Some(4)]).unwrap();
        g.verify().unwrap();
        assert!(inline_inst_count(&g) < 4, "{}", g.dump());
        // len = 8: the loop is dead, the tail runs
        let g = specialize_for_args(&f, &[Some(8)]).unwrap();
        assert!(inline_inst_count(&g) >= 200 && inline_inst_count(&g) < 206, "{}", g.dump());
        // len = 40: the loop runs, nothing decided (i is not a constant there)
        assert!(specialize_for_args(&f, &[Some(40)]).map_or(true, |g| inline_inst_count(&g) > 400));
        assert!(specialize_for_args(&f, &[None]).is_none());
        // a site passing len = 4 inlines the 400-instruction body, one passing a variable keeps the call
        let options = &relax_session::Options::default();
        let get = |_: u32| Some((&f as *const Func, 0));
        let site = |arg: Option<i64>| {
            let mut c = Func::new("caller".into(), 0);
            let a = c.vreg(Cls::I);
            c.block();
            c.blocks[0].insts = match arg { Some(k) => vec![Inst::Iconst(a, k)], None => vec![] };
            if arg.is_none() { c.params.push(a); }
            c.blocks[0].insts.push(Inst::Call(Callee::Fn(1), vec![a], vec![]));
            c.blocks[0].pos = vec![0; c.blocks[0].insts.len()];
            c.blocks[0].term = Term::Ret(vec![]);
            c
        };
        let mut c = site(Some(4));
        assert_eq!(inline_calls_ordered(options, &mut c, 0, &get, &|_| None, &|_, _| false, &|_| false, &InlineLimits::tier1(), None, None).len(), 1);
        c.verify().unwrap();
        let mut c = site(None);
        assert!(inline_calls_ordered(options, &mut c, 0, &get, &|_| None, &|_, _| false, &|_| false, &InlineLimits::tier1(), None, None).is_empty());
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
        assert!(inline_calls_ordered(options, &mut f, 0, &get, &|_| None, &|_, _| false, &|_| false, &InlineLimits::tier1(), None, Some(&mut stats)).is_empty());
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
        assert_eq!(inline_calls_ordered(options, &mut f, 0, &get, &|_| None, &|_, _| false, &|_| false, &lim, None, Some(&mut stats)), vec![(2, RULE_SMALL_LEAF)]);
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
        assert!(relax_backend::regalloc::loop_depths(&f, &reachable(&f)).iter().all(|d| *d >= 1));
        let mut stats = InlineStats::default();
        assert_eq!(inline_calls_ordered(options, &mut f, 0, &get, &|_| None, &|_, _| false, &|_| false, &InlineLimits::tier1(), Some(&[0.01, 1.0, 0.0]), Some(&mut stats)), vec![(2, RULE_SMALL_LEAF)]);
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
            assert_eq!(inline_calls_ordered(options, &mut f, 0, &get, &|_| None, &|_, _| false, &|_| false, &InlineLimits::tier1(), Some(&[0.01, freq, 1.0]), None).len(), expected);
        }
        for (blocks, expected) in [(64, 1), (65, 0)] {
            let mut f = caller();
            while f.blocks.len() < blocks { f.block(); }
            assert_eq!(inline_calls_ordered(options, &mut f, 0, &get, &|_| None, &|_, _| false, &|_| false, &InlineLimits::tier1(), None, None).len(), expected);
        }
    }

    #[test]
    fn unsampled_sites_only_allow_free_or_always() {
        let options = &relax_session::Options::default();
        for (size, attrs, expected) in [(30, 0, 0), (30, ATTR_HINT, 0), (30, ATTR_CHAIN, 0), (30, ATTR_ALWAYS, 2), (INLINE_FREE_INSTS, 0, 2)] {
            let c = callee(size);
            let get = |_: u32| Some((&c as *const Func, attrs));
            let mut f = caller();
            assert_eq!(inline_calls_ordered(options, &mut f, 0, &get, &|_| None, &|_, _| false, &|_| false, &InlineLimits::tier1(), Some(&[0.0, 0.0, 1.0]), None).len(), expected);
            f.verify().unwrap();
        }
        // inline(always) still consumes the paid allowance.
        let c = callee(30);
        let get = |_: u32| Some((&c as *const Func, ATTR_ALWAYS));
        let mut lim = InlineLimits::tier1();
        lim.budget = 27;
        assert_eq!(inline_calls_ordered(options, &mut caller(), 0, &get, &|_| None, &|_, _| false, &|_| false, &lim, Some(&[0.0, 0.0, 1.0]), None).len(), 1);
    }

    #[test]
    fn actual_growth_stops_paid_inlining_before_net_budget_runs_out() {
        let options = &relax_session::Options::default();
        let c = callee(25);
        let get = |_: u32| Some((&c as *const Func, 0));
        let mut f = callee(0);
        f.blocks[0].insts = vec![Inst::Call(Callee::Fn(1), vec![], vec![]); 200];
        f.blocks[0].pos = vec![0; 200];
        let mut lim = InlineLimits::tier1_for(&f, true);
        lim.growth_limit = Some(4200);
        let mut stats = InlineStats::default();
        let inlined = inline_calls_ordered(options, &mut f, 0, &get, &|_| None, &|_, _| false, &|_| false, &lim, None, Some(&mut stats));
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
        let lim = InlineLimits::tier1_for(&f, true);
        assert_eq!(lim.growth_limit, Some(8002));
        // Model growth from the first round; the original ceiling must still apply.
        let v = f.vreg(Cls::I);
        f.blocks[0].insts.extend(vec![Inst::Iconst(v, 1); 8001]);
        f.blocks[0].pos.extend(vec![0; 8001]);
        f.blocks[0].insts.push(Inst::Call(Callee::Fn(3), vec![], vec![]));
        f.blocks[0].pos.push(0);
        let mut stats = InlineStats::default();
        assert_eq!(inline_calls_ordered(options, &mut f, 0, &get, &|_| None, &|_, _| false, &|_| false, &lim, None, Some(&mut stats)).len(), 1);
        assert_eq!(stats.left[3], 2);
        assert_eq!(stats.growth_after, inline_inst_count(&f));
        assert!(inline_calls_ordered(options, &mut f, 0, &get, &|_| None, &|_, _| false, &|_| false, &lim, None, Some(&mut stats)).is_empty());
        f.verify().unwrap();
    }

    #[test]
    fn budget_does_not_scale_with_caller() {
        assert_eq!(InlineLimits::tier1_for(&callee(100), true).budget, 4000);
        assert_eq!(InlineLimits::tier1_for(&callee(16000), true).budget, 4000);
        assert_eq!(InlineLimits::tier1_for(&callee(30000), true).budget, 4000);
        assert_eq!(InlineLimits::tier1_for(&callee(16000), true).growth_limit, Some(64000));
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
            assert_eq!(inline_calls_ordered(options, &mut f, 0, &get, &|_| None, &|_, _| false, &|_| false, &lim, Some(&[0.0]), Some(&mut stats)), vec![(1, RULE_COST)]);
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
            let result = inline_calls_ordered(options, &mut f, 0, &get, &|_| None, &single, &|_| false, &lim, Some(&[0.0]), Some(&mut stats));
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
        assert!(inline_calls_ordered(options, &mut f, 0, &get, &|_| None, &|_, _| true, &|_| false, &InlineLimits::tier1(), Some(&[1.0]), None).is_empty());
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
            assert!(inline_calls_ordered(options, &mut f, 0, &get, &|_| None, &|_, _| true, &|_| false, &InlineLimits::tier1(), None, None).is_empty());
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
            assert!(inline_calls_ordered(&options, &mut f, 0, &get, &|_| None, &|_, _| toggle.is_empty(), &|_| false, &InlineLimits::tier1(), Some(&[0.0]), None).is_empty());
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
        assert_eq!(inline_calls_ordered(options, &mut f, 0, &get, &|_| None, &|_, _| false, &|_| false, &lim, None, None), vec![(1, RULE_COST); 4]);
        f.verify().unwrap();
        lim.growth_limit = Some(999);
        assert!(inline_calls_ordered(options, &mut one_call(1), 0, &get, &|_| None, &|_, _| false, &|_| false, &lim, None, None).is_empty());
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
            assert_eq!(inline_calls_ordered(options, &mut one_call(1), 0, &get, &|_| None, &|_, _| true, &|_| false, &lim, None, None).len(), expected);
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
        assert_eq!(inline_calls_ordered(options, &mut f, 0, &get, &|_| None, &|owner, _| owner == 0, &|_| false, &lim, None, None).len(), 9);
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
            let inlined = inline_calls_ordered(options, &mut f, 0, &get, &|_| None, &|_, _| false, &|_| false, &InlineLimits::tier1(), Some(&[0.0]), None);
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
            let result = inline_calls_ordered(options, &mut f, 0, &get, &|_| None, &single, &|_| false, &InlineLimits::tier1(), Some(&[0.0]), None);
            assert_eq!(result, if owner == 1 { vec![(1, RULE_COST), (2, RULE_SINGLE)] } else { vec![(1, RULE_COST)] });
            f.verify().unwrap();
        }
    }

}

#[cfg(test)]
mod slot_layout_tests {
    use super::*;
    use relax_backend::frame::slot_layout;

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
    fn slot_layout_keeps_an_inlined_argument_copy_live_while_the_body_reads_it() {
        // `dbg_arr([1, 2])` inlined: the argument copy becomes the body's `a`, whose address the body
        // hands to a call that keeps it (`fmt::Argument::new_debug(&a)`) and reads it later, after
        // writing a result buffer (t15.rs const_generics read the String's bytes as the array)
        let mut callee = Func::new("callee".into(), 0);
        let rs = callee.slot(24, 8);
        let (p, q, r) = (callee.vreg(Cls::I), callee.vreg(Cls::I), callee.vreg(Cls::I));
        callee.params.push(p);
        callee.block();
        callee.blocks[0].insts = vec![
            Inst::Call(Callee::Fn(3), vec![p], vec![q]),
            Inst::SlotAddr(r, rs), Inst::Call(Callee::Fn(4), vec![r], vec![]),
            Inst::Call(Callee::Fn(5), vec![q], vec![]),
        ];
        callee.blocks[0].pos = vec![0; 4];
        callee.blocks[0].term = Term::Ret(vec![]);
        let mut f = Func::new("caller".into(), 0);
        let s = f.slot(16, 8);
        f.slots[s as usize].arg = true;
        f.slots[s as usize].born = true;
        let (a, k) = (f.vreg(Cls::I), f.vreg(Cls::I));
        f.block();
        f.blocks[0].insts = vec![
            Inst::Iconst(k, 1), Inst::SlotAddr(a, s), Inst::Store(Mem::Int(8, false), a, 0, k),
            Inst::Call(Callee::Fn(1), vec![a], vec![]),
        ];
        f.blocks[0].pos = vec![0; 4];
        f.blocks[0].term = Term::Ret(vec![]);
        inline_site(&mut f, 0, 3, &callee, &[a], &[], &|_| false);
        f.verify().unwrap();
        assert!(!f.slots[s as usize].arg);
        let (at, _) = slot_layout(&f, 0, true);
        assert_ne!(at[0], at[1]);
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

/// The mask idiom of bit readers, `x & ((1 << n) - 1)` (and `x & !(-1 << n)`) on 64-bit values,
/// becomes `x bzhi n` (IOp::Bzhi) when the shift and the mask are read only there: x64 BMI2 `and`
/// of the count + `bzhi` (was `mov 1; shlx; sub; and`, a three-deep chain), arm64 `movn; lslv;
/// bic`. The shift and subtraction are left for DCE. The count must hold the same value at the
/// mask as at the shift: one definition, or none between them in their block. Only where the
/// target has the short form (x64 without BMI2 would expand it again). Returns whether anything
/// changed.
pub fn form_bzhi(options: &relax_session::Options, f: &mut Func) -> bool {
    if opt_off(options, "-bzhi") || !(cfg!(target_arch = "aarch64") || options.cpu.bmi2) {
        return false;
    }
    let nv = f.vregs.len();
    let mut ndef = vec![0u32; nv];
    let mut nuse = vec![0u32; nv];
    let mut def_at = vec![(u32::MAX, 0u32); nv];
    // the constant every definition of a vreg assigns (a constant materialised on several paths)
    let mut kval: Vec<Option<Option<i64>>> = vec![None; nv];
    for p in &f.params {
        ndef[p.0 as usize] += 1;
        kval[p.0 as usize] = Some(None);
    }
    let mut tu = Vec::new();
    for (bi, b) in f.blocks.iter().enumerate() {
        for (k, i) in b.insts.iter().enumerate() {
            let c = if let Inst::Iconst(_, c) = *i { Some(c) } else { None };
            for_defs(i, |d| {
                ndef[d.0 as usize] += 1;
                def_at[d.0 as usize] = (bi as u32, k as u32);
                let e = &mut kval[d.0 as usize];
                *e = Some(match (*e, c) {
                    (None, c) => c,
                    (Some(Some(x)), Some(y)) if x == y => Some(x),
                    _ => None,
                });
            });
            for_uses(i, |u| nuse[u.0 as usize] += 1);
        }
        term_uses(&b.term, &mut tu);
        for u in &tu {
            nuse[u.0 as usize] += 1;
        }
    }
    let single = |v: VReg| ndef[v.0 as usize] == 1 && def_at[v.0 as usize].0 != u32::MAX;
    let def = |f: &Func, v: VReg| -> Inst { let (b, k) = def_at[v.0 as usize]; f.blocks[b as usize].insts[k as usize].clone() };
    let konst = |_: &Func, v: VReg, want: i64| kval[v.0 as usize] == Some(Some(want));
    // (shift's block, index, count) of `m` when it is the low-n-bits mask of a count n
    let mask = |f: &Func, m: VReg| -> Option<(u32, u32, VReg)> {
        if !single(m) || nuse[m.0 as usize] != 1 {
            return None;
        }
        let (s, ones) = match def(f, m) {
            Inst::IBinI(IOp::Sub, it, _, s, 1) if it.bits == 64 => (s, 1),
            Inst::IBinI(IOp::Add, it, _, s, -1) if it.bits == 64 => (s, 1),
            Inst::IBinI(IOp::Xor, it, _, s, -1) if it.bits == 64 => (s, -1),
            _ => return None,
        };
        if !single(s) || nuse[s.0 as usize] != 1 {
            return None;
        }
        match def(f, s) {
            Inst::IBin(IOp::Shl, it, _, one, n) if it.bits == 64 && konst(f, one, ones) => {
                let (b, k) = def_at[s.0 as usize];
                Some((b, k, n))
            }
            _ => None,
        }
    };
    let mut edits = Vec::new();
    for (bi, b) in f.blocks.iter().enumerate() {
        for (k, i) in b.insts.iter().enumerate() {
            let Inst::IBin(IOp::And, it, d, p, q) = *i else { continue };
            if it.bits != 64 {
                continue;
            }
            for (x, m) in [(p, q), (q, p)] {
                let Some((sb, sk, n)) = mask(f, m) else { continue };
                // n unchanged from the shift to here
                let same = if single(n) || f.params.contains(&n) && ndef[n.0 as usize] == 1 {
                    true
                } else if sb as usize == bi && (sk as usize) < k {
                    !b.insts[sk as usize + 1..k].iter().any(|j| {
                        let mut w = false;
                        for_defs(j, |v| w |= v == n);
                        w
                    })
                } else {
                    false
                };
                if same && x != m {
                    edits.push((bi, k, Inst::IBin(IOp::Bzhi, U64, d, x, n)));
                    break;
                }
            }
        }
    }
    let changed = !edits.is_empty();
    for (bi, k, i) in edits {
        f.blocks[bi].insts[k] = i;
    }
    changed
}

/// The few values an integer vreg can hold where it is read, when its definitions make them a
/// small set: constants, compare outcomes (0 / 1, or what the operands' sets give), selects of such
/// values, and unary maps of them (integer conversions, operations with a constant). A vreg with
/// several definitions (RIR is not SSA: a constant per path, `Iconst(v, 0)` in two blocks) holds
/// one of their values. At most four values, eight definitions deep. `defs[v]` is empty for a
/// parameter (unknown).
fn small_set(f: &Func, defs: &[Vec<(u32, u32)>], v: VReg, depth: u32, out: &mut Vec<i64>, budget: &mut u32) -> bool {
    if depth == 0 || defs[v.0 as usize].is_empty() || *budget == 0 {
        return false;
    }
    *budget -= 1;
    defs[v.0 as usize].iter().all(|&(bi, k)| small_set_def(f, defs, &f.blocks[bi as usize].insts[k as usize], depth, out, budget))
}

fn small_set_def(f: &Func, defs: &[Vec<(u32, u32)>], i: &Inst, depth: u32, out: &mut Vec<i64>, budget: &mut u32) -> bool {
    let push = |out: &mut Vec<i64>, x: i64| -> bool {
        if !out.contains(&x) {
            if out.len() == 4 {
                return false;
            }
            out.push(x);
        }
        true
    };
    match *i {
        Inst::Iconst(_, c) => push(out, c),
        // a compare of small sets: its outcomes; else either
        Inst::ICmpI(cc, sg, _, x, c) => {
            let mut xs = Vec::new();
            if small_set(f, defs, x, depth - 1, &mut xs, budget) {
                xs.into_iter().all(|a| push(out, cmp_i(cc, sg, a, c) as i64))
            } else {
                push(out, 0) && push(out, 1)
            }
        }
        Inst::ICmp(cc, sg, _, x, y) => {
            let (mut xs, mut ys) = (Vec::new(), Vec::new());
            if small_set(f, defs, x, depth - 1, &mut xs, budget) && small_set(f, defs, y, depth - 1, &mut ys, budget) {
                xs.iter().all(|&a| ys.iter().all(|&b| push(out, cmp_i(cc, sg, a, b) as i64)))
            } else {
                push(out, 0) && push(out, 1)
            }
        }
        Inst::FCmp(..) => push(out, 0) && push(out, 1),
        Inst::Mov(_, a) => small_set(f, defs, a, depth - 1, out, budget),
        Inst::Sel(_, _, a, b) => small_set(f, defs, a, depth - 1, out, budget) && small_set(f, defs, b, depth - 1, out, budget),
        Inst::Conv(Conv::IntToInt(it), _, a) => {
            let mut xs = Vec::new();
            small_set(f, defs, a, depth - 1, &mut xs, budget) && xs.into_iter().all(|x| push(out, extend_const(x, it)))
        }
        Inst::IBinI(op, it, _, a, c) if !matches!(op, IOp::Div | IOp::Rem) => {
            let mut xs = Vec::new();
            small_set(f, defs, a, depth - 1, &mut xs, budget) && xs.into_iter().all(|x| fold_ibin(op, it, x, c).map_or(false, |y| push(out, y)))
        }
        _ => false,
    }
}

/// Compares and selects decided by the small value sets of their operands (LLVM's fold of a
/// compare of a select of constants, InstCombine `foldCmpLoadFromIndexedGlobal`'s set form):
/// tuple `lt` compares fields through `Ordering` as an i8 (`lt ? -1 : (eq ^ 1)`) and tests the
/// sign-extended byte against `ControlFlow`'s niche (254), which no value of the set is: the
/// branch is always taken. A compare all of whose operand values give one outcome becomes that
/// constant; a select on a condition with one possible value becomes a copy. Returns whether
/// anything changed (simplify then folds the branches).
pub fn fold_small_sets(f: &mut Func) -> bool {
    let nv = f.vregs.len();
    let mut defs: Vec<Vec<(u32, u32)>> = vec![Vec::new(); nv];
    let mut param = vec![false; nv];
    for p in &f.params {
        param[p.0 as usize] = true;
    }
    for (bi, b) in f.blocks.iter().enumerate() {
        for (k, i) in b.insts.iter().enumerate() {
            for_defs(i, |d| {
                if !param[d.0 as usize] {
                    defs[d.0 as usize].push((bi as u32, k as u32));
                }
            });
        }
    }
    // a parameter (also when redefined) has values no definition shows
    for v in 0..nv {
        if param[v] {
            defs[v].clear();
        }
    }
    let mut edits: Vec<(usize, usize, Inst)> = Vec::new();
    for (bi, b) in f.blocks.iter().enumerate() {
        for (k, i) in b.insts.iter().enumerate() {
            let (d, cc, sg, x, y) = match *i {
                Inst::ICmpI(cc, sg, d, x, c) => (d, cc, sg, x, Err(c)),
                Inst::ICmp(cc, sg, d, x, y) => (d, cc, sg, x, Ok(y)),
                Inst::Sel(d, c, a, b2) => {
                    let mut cs = Vec::new();
                    if small_set(f, &defs, c, 8, &mut cs, &mut 64) && !cs.is_empty() {
                        let t = cs.iter().all(|&x| x != 0);
                        if t || cs.iter().all(|&x| x == 0) {
                            edits.push((bi, k, Inst::Mov(d, if t { a } else { b2 })));
                        }
                    }
                    continue;
                }
                _ => continue,
            };
            let mut xs = Vec::new();
            if !small_set(f, &defs, x, 8, &mut xs, &mut 64) || xs.is_empty() {
                continue;
            }
            let ys = match y {
                Err(c) => vec![c],
                Ok(y) => {
                    let mut ys = Vec::new();
                    if !small_set(f, &defs, y, 8, &mut ys, &mut 64) || ys.is_empty() {
                        continue;
                    }
                    ys
                }
            };
            let r0 = cmp_i(cc, sg, xs[0], ys[0]);
            if xs.iter().all(|&a| ys.iter().all(|&b| cmp_i(cc, sg, a, b) == r0)) {
                // a single-value pair is a constant compare simplify folds itself
                if xs.len() > 1 || ys.len() > 1 {
                    edits.push((bi, k, Inst::Iconst(d, r0 as i64)));
                }
            }
        }
    }
    let changed = !edits.is_empty();
    for (bi, k, i) in edits {
        f.blocks[bi].insts[k] = i;
    }
    changed
}

/// Bit-tracking dead code for masks and casts (LLVM's BDCE): per integer vreg, the bits any of
/// its uses reads (a fixed point over every use, so multi-definition variables -- a bit
/// reader's `bitsleft`, read only as `bitsleft as u8` and `(bitsleft >> 3) & 7` -- are covered);
/// an `x & k` whose result is read only within k, or an integer cast whose result is read only
/// within the source's low bits, becomes a copy of x (then coalesced: `bitsleft -= entry & 0xff`
/// was movzx + sub). Narrow operations re-extend their results (both backends), so a narrow
/// result's demanded bits above its width demand its top bit (signed) or nothing (unsigned).
/// MAKEPAD_RUN_OPT=-bdce: off (bisection).
pub fn drop_undemanded_masks(options: &relax_session::Options, f: &mut Func) -> bool {
    if opt_off(options, "-bdce") {
        return false;
    }
    let nv = f.vregs.len();
    const ALL: u64 = !0;
    let mask = |w: u8| if w >= 64 { ALL } else { (1u64 << w) - 1 };
    // the bits of an operation's w-bit result that its demanded 64-bit register bits read
    let narrow = |d: u64, it: IntTy| -> u64 {
        if it.bits >= 64 {
            d
        } else {
            let m = mask(it.bits);
            (d & m) | if it.signed && d & !m != 0 { 1u64 << (it.bits - 1) } else { 0 }
        }
    };
    // the low bits an add / sub / mul result's bits depend on
    let upto = |d: u64| if d == 0 { 0 } else { mask(64 - d.leading_zeros() as u8) };
    let mut dem = vec![0u64; nv];
    let mut changed = true;
    let mut tu = Vec::new();
    let isint = |f: &Func, v: VReg| f.vregs[v.0 as usize] == Cls::I;
    while changed {
        changed = false;
        let mut need = |v: VReg, m: u64, dem: &mut Vec<u64>| {
            let e = &mut dem[v.0 as usize];
            if *e | m != *e {
                *e |= m;
                changed = true;
            }
        };
        for b in &f.blocks {
            for i in &b.insts {
                match *i {
                    Inst::Mov(d, x) => need(x, dem[d.0 as usize], &mut dem),
                    Inst::IBin(op, it, d, x, y) => {
                        let r = narrow(dem[d.0 as usize], it);
                        match op {
                            IOp::Add | IOp::Sub | IOp::Mul => {
                                need(x, upto(r), &mut dem);
                                need(y, upto(r), &mut dem);
                            }
                            // bitwise results are not re-extended: the operands' upper bits pass through
                            IOp::And | IOp::Or | IOp::Xor => {
                                need(x, dem[d.0 as usize], &mut dem);
                                need(y, dem[d.0 as usize], &mut dem);
                            }
                            IOp::Shl | IOp::Shr if it.bits == 64 && x != y => {
                                need(x, ALL, &mut dem);
                                need(y, 63, &mut dem);
                            }
                            _ => {
                                need(x, ALL, &mut dem);
                                need(y, ALL, &mut dem);
                            }
                        }
                    }
                    Inst::IBinI(op, it, d, x, k) => {
                        let full = dem[d.0 as usize];
                        let r = narrow(full, it);
                        // a narrow bitwise immediate may be emitted as the 64-bit form with the
                        // immediate sign-extended: the operand's upper bits pass through
                        let kk = if it.bits >= 64 { k as u64 } else { k as u64 | !mask(it.bits) };
                        let m = match op {
                            IOp::Add | IOp::Sub | IOp::Mul => upto(r),
                            IOp::And => full & kk,
                            IOp::Or => full & !(k as u64 & mask(it.bits)),
                            IOp::Xor => full,
                            // shifted left (and re-extended when narrow)
                            IOp::Shl if (0..it.bits as i64).contains(&k) => r >> k,
                            // a 64-bit logical shift of the register (narrow values too): bit i
                            // of the result is bit i + k of the operand
                            IOp::Shr if !it.signed && (0..it.bits as i64).contains(&k) => full << k,
                            _ => ALL,
                        };
                        need(x, m, &mut dem);
                    }
                    Inst::INeg(it, d, x) | Inst::INot(it, d, x) => {
                        let r = narrow(dem[d.0 as usize], it);
                        need(x, if matches!(i, Inst::INeg(..)) { upto(r) } else { r }, &mut dem);
                    }
                    Inst::Conv(Conv::IntToInt(it), d, x) => need(x, narrow(dem[d.0 as usize], it), &mut dem),
                    Inst::Store(Mem::Int(n, _), base, _, v) if base != v => {
                        need(base, ALL, &mut dem);
                        need(v, mask(8 * n), &mut dem);
                    }
                    Inst::StoreX(Mem::Int(n, _), base, idx, _, _, v) if base != v && idx != v => {
                        need(base, ALL, &mut dem);
                        need(idx, ALL, &mut dem);
                        need(v, mask(8 * n), &mut dem);
                    }
                    _ => for_uses(i, |u| if isint(f, u) { need(u, ALL, &mut dem) }),
                }
            }
            term_uses(&b.term, &mut tu);
            for &u in &tu {
                need(u, ALL, &mut dem);
            }
        }
    }
    let ints: Vec<bool> = f.vregs.iter().map(|c| *c == Cls::I).collect();
    let mut any = false;
    for b in &mut f.blocks {
        for i in &mut b.insts {
            let n = match *i {
                Inst::IBinI(IOp::And, it, d, x, k) if dem[d.0 as usize] & !(k as u64 & mask(it.bits)) == 0 => Inst::Mov(d, x),
                Inst::Conv(Conv::IntToInt(it), d, x) if ints[x.0 as usize] && dem[d.0 as usize] & !mask(it.bits) == 0 => Inst::Mov(d, x),
                _ => continue,
            };
            *i = n;
            any = true;
        }
    }
    any
}

/// A memory access through `p = q + k` (a single-def constant offset) reads `[q + k + off]`
/// when q still holds its value there (defined once, or not redefined between in the block): an
/// unrolled byte loop's `lea rsi,[r12+1]; movzx edx,byte [rsi-1]` per byte was left by
/// fold_post_increments and flatten_step_chains; p then dies (the CRC loop of ogg parse_page:
/// four leas per four bytes). Returns whether an access changed.
pub fn fold_offset_bases(options: &relax_session::Options, f: &mut Func) -> bool {
    if opt_off(options, "-offbase") {
        return false;
    }
    let nv = f.vregs.len();
    let mut ndef = vec![0u32; nv];
    for p in &f.params {
        ndef[p.0 as usize] += 2;
    }
    let mut off_of: Vec<Option<(VReg, i64, u32, u32)>> = vec![None; nv];
    for (bi, b) in f.blocks.iter().enumerate() {
        for (k, i) in b.insts.iter().enumerate() {
            for_defs(i, |d| ndef[d.0 as usize] += 1);
            if let Inst::IBinI(IOp::Add, it, d, q, c) = *i {
                if it.bits == 64 && d != q {
                    off_of[d.0 as usize] = Some((q, c, bi as u32, k as u32));
                }
            }
        }
    }
    let mut changed = false;
    for bi in 0..f.blocks.len() {
        for k in 0..f.blocks[bi].insts.len() {
            let base = match f.blocks[bi].insts[k] {
                Inst::Load(_, _, b, _) | Inst::Store(_, b, _, _) | Inst::VLoad(_, b, _) | Inst::VStore(b, _, _) | Inst::LoadX(_, _, b, ..) | Inst::StoreX(_, b, ..) | Inst::VLoadX(_, b, ..) | Inst::VStoreX(b, ..) => b,
                _ => continue,
            };
            if ndef[base.0 as usize] != 1 {
                continue;
            }
            let Some((q, c, db, dk)) = off_of[base.0 as usize] else { continue };
            let same = ndef[q.0 as usize] == 1
                || db as usize == bi && (dk as usize) < k && (dk as usize + 1..k).all(|j| {
                    let mut hit = false;
                    for_defs(&f.blocks[bi].insts[j], |v| hit |= v == q);
                    !hit
                });
            if !same {
                continue;
            }
            let fits = |o: i32| -> Option<i32> { i32::try_from(o as i64 + c).ok() };
            let n = match f.blocks[bi].insts[k] {
                Inst::Load(m, d, _, o) => fits(o).map(|o| Inst::Load(m, d, q, o)),
                Inst::Store(m, _, o, v) if v != base => fits(o).map(|o| Inst::Store(m, q, o, v)),
                Inst::VLoad(d, _, o) => fits(o).map(|o| Inst::VLoad(d, q, o)),
                Inst::VStore(_, o, v) => fits(o).map(|o| Inst::VStore(q, o, v)),
                Inst::LoadX(m, d, _, x, sh, o) if x != base => fits(o).map(|o| Inst::LoadX(m, d, q, x, sh, o)),
                Inst::StoreX(m, _, x, sh, o, v) if x != base && v != base => fits(o).map(|o| Inst::StoreX(m, q, x, sh, o, v)),
                Inst::VLoadX(d, _, x, sh, o) if x != base => fits(o).map(|o| Inst::VLoadX(d, q, x, sh, o)),
                Inst::VStoreX(_, x, sh, o, v) if x != base => fits(o).map(|o| Inst::VStoreX(q, x, sh, o, v)),
                _ => None,
            };
            if let Some(n) = n {
                f.blocks[bi].insts[k] = n;
                changed = true;
            }
        }
    }
    changed
}

/// `passes::rename` plus the backend-only indexed memory forms `addr_modes` makes (the passes
/// at the end of O3 run after it).
fn rename_operands(i: &Inst, a: &dyn Fn(VReg) -> VReg) -> Inst {
    match *i {
        Inst::LoadX(m, d, b, x, sh, o) => Inst::LoadX(m, d, a(b), a(x), sh, o),
        Inst::StoreX(m, b, x, sh, o, v) => Inst::StoreX(m, a(b), a(x), sh, o, a(v)),
        Inst::VLoadX(d, b, x, sh, o) => Inst::VLoadX(d, a(b), a(x), sh, o),
        Inst::VStoreX(b, x, sh, o, v) => Inst::VStoreX(a(b), a(x), sh, o, a(v)),
        _ => rename(i, a),
    }
}

/// Variables read again after a copy into them: `t = x >> n; x = t; ... use t` (O3, before
/// register allocation). Copy propagation and value numbering make later reads of a variable x
/// (several definitions: a loop-carried bit buffer, a counter) read the single-definition
/// temporary t its value came from. t and x then hold the same value over the same stretch,
/// both live: two registers, and the copy `x = t` stays a move (`shrx rdi,r13,rsi; mov r13,rdi`,
/// `mov rax,r12; sub eax,esi; mov rsi,rax; mov r12,rsi` per bit-reader step in inflate).
/// LLVM has no such pair: SSA phis coalesce.
/// Here every read of t at which x still holds t's value (on every path from the copy, a
/// forward must-analysis over copies, kill at any other definition of x or of t) reads x
/// instead, when that leaves t read only between its definition and the copy in the copy's
/// block: t then dies at the copy and the allocator gives both one register.
/// Candidates: `Mov(x, t)`, x defined more than once, t defined once in the same block before
/// it, not a parameter and not a value the allocator recomputes (constants, addresses).
/// MAKEPAD_RUN_OPT=-revar: off (bisection).
pub fn reuse_copied_variables(options: &relax_session::Options, f: &mut Func) -> bool {
    if opt_off(options, "-revar") {
        return false;
    }
    let nv = f.vregs.len();
    let nb = f.blocks.len();
    let mut ndef = vec![0u32; nv];
    for p in &f.params {
        ndef[p.0 as usize] += 2;
    }
    // single-def vreg -> (block, index) of its definition
    let mut def_at = vec![(u32::MAX, 0u32); nv];
    for (bi, b) in f.blocks.iter().enumerate() {
        for (k, i) in b.insts.iter().enumerate() {
            for_defs(i, |d| {
                ndef[d.0 as usize] += 1;
                def_at[d.0 as usize] = (bi as u32, k as u32);
            });
        }
    }
    let reach = reachable(f);
    // candidates: (block, index of the copy, x, t)
    const NONE: u32 = u32::MAX;
    let mut cand: Vec<(u32, u32, VReg, VReg)> = Vec::new();
    let mut by_t = vec![NONE; nv];
    for (bi, b) in f.blocks.iter().enumerate() {
        if !reach[bi] {
            continue;
        }
        for (k, i) in b.insts.iter().enumerate() {
            let Inst::Mov(x, t) = *i else { continue };
            let (xu, tu) = (x.0 as usize, t.0 as usize);
            if x == t || ndef[tu] != 1 || ndef[xu] < 2 || by_t[tu] != NONE || def_at[tu].0 != bi as u32 || def_at[tu].1 >= k as u32 {
                continue;
            }
            let (db, di) = def_at[tu];
            if matches!(f.blocks[db as usize].insts[di as usize], Inst::Iconst(..) | Inst::Fconst(..) | Inst::VConst(..) | Inst::Addr(..) | Inst::SlotAddr(..)) {
                continue;
            }
            by_t[tu] = cand.len() as u32;
            cand.push((bi as u32, k as u32, x, t));
        }
    }
    if cand.is_empty() {
        return false;
    }
    let nc = cand.len();
    let w = (nc + 63) / 64;
    // x -> its candidates (x is the target of several copies: one per temporary)
    let mut by_x: Vec<Vec<u32>> = vec![Vec::new(); nv];
    for (c, &(_, _, x, _)) in cand.iter().enumerate() {
        by_x[x.0 as usize].push(c as u32);
    }
    let bit = |s: &[u64], c: usize| s[c / 64] >> (c % 64) & 1 != 0;
    // the state change of one instruction: copies set their bit, other defs of x or t clear
    let step = |s: &mut [u64], bi: usize, k: usize, i: &Inst| {
        for_defs(i, |d| {
            for &c in &by_x[d.0 as usize] {
                let c = c as usize;
                if cand[c].0 as usize == bi && cand[c].1 as usize == k {
                    s[c / 64] |= 1 << (c % 64);
                } else {
                    s[c / 64] &= !(1 << (c % 64));
                }
            }
            let c = by_t[d.0 as usize];
            if c != NONE {
                s[c as usize / 64] &= !(1 << (c % 64));
            }
        });
    };
    let preds = relax_backend::regalloc::preds_of(f, &reach);
    let order = relax_backend::frame::block_rpo(f);
    let mut inn = vec![!0u64; nb * w];
    let mut out = vec![!0u64; nb * w];
    let mut cur = vec![0u64; w];
    let mut changed = true;
    while changed {
        changed = false;
        for &b in &order {
            let bi = b as usize;
            if !reach[bi] {
                continue;
            }
            if bi == 0 {
                cur.iter_mut().for_each(|x| *x = 0);
            } else {
                cur.iter_mut().for_each(|x| *x = !0);
                for &p in &preds[bi] {
                    for j in 0..w {
                        cur[j] &= out[p as usize * w + j];
                    }
                }
            }
            inn[bi * w..bi * w + w].copy_from_slice(&cur);
            for (k, i) in f.blocks[bi].insts.iter().enumerate() {
                step(&mut cur, bi, k, i);
            }
            if out[bi * w..bi * w + w] != cur[..] {
                out[bi * w..bi * w + w].copy_from_slice(&cur);
                changed = true;
            }
        }
    }
    // a candidate qualifies when each read of t is either rewritable or lies between t's
    // definition and the copy
    let mut ok = vec![true; nc];
    let mut uses = Vec::new();
    for (bi, b) in f.blocks.iter().enumerate() {
        if !reach[bi] {
            continue;
        }
        cur.copy_from_slice(&inn[bi * w..bi * w + w]);
        let mut check = |cur: &[u64], u: VReg, k: usize| {
            let c = by_t[u.0 as usize];
            if c != NONE && !bit(cur, c as usize) {
                let (cb, ck, _, _) = cand[c as usize];
                if !(cb as usize == bi && k <= ck as usize) {
                    ok[c as usize] = false;
                }
            }
        };
        for (k, i) in b.insts.iter().enumerate() {
            for_uses(i, |u| check(&cur, u, k));
            step(&mut cur, bi, k, i);
        }
        term_uses(&b.term, &mut uses);
        for &u in &uses {
            check(&cur, u, usize::MAX);
        }
    }
    if !ok.iter().any(|&o| o) {
        return false;
    }
    // MAKEPAD_RUN_OPT=-revarx: indexed memory operands keep the temporary (bisection)
    let indexed = !opt_off(options, "-revarx");
    let mut any = false;
    for bi in 0..nb {
        if !reach[bi] {
            continue;
        }
        cur.copy_from_slice(&inn[bi * w..bi * w + w]);
        for k in 0..f.blocks[bi].insts.len() {
            let look = |u: VReg| -> VReg {
                let c = by_t[u.0 as usize];
                if c != NONE && ok[c as usize] && bit(&cur, c as usize) {
                    cand[c as usize].2
                } else {
                    u
                }
            };
            let n = if indexed { rename_operands(&f.blocks[bi].insts[k], &look) } else { rename(&f.blocks[bi].insts[k], &look) };
            if n != f.blocks[bi].insts[k] {
                f.blocks[bi].insts[k] = n;
                any = true;
            }
            step(&mut cur, bi, k, &f.blocks[bi].insts[k]);
        }
        let mut t = f.blocks[bi].term.clone();
        relax_backend::regalloc::term_vregs_mut(&mut t, |v| {
            let c = by_t[v.0 as usize];
            if c != NONE && ok[c as usize] && bit(&cur, c as usize) {
                *v = cand[c as usize].2;
                any = true;
            }
        });
        f.blocks[bi].term = t;
    }
    any
}

/// Dead stores into slots (O3): a store, vector store or copy into a slot whose bytes no later
/// read can see -- on every path from it the bytes are overwritten first, or never read again
/// -- goes. Backward liveness of slot bytes over the CFG, for the slots `slot_scan` follows
/// completely (not escaping): every read of such a slot is a load or a copy out of it.
/// `let mut tmp = [0u8; 16]; tmp.copy_from_slice(src); dst.copy_from_slice(&tmp)` (brotli's
/// ring-buffer copy): the 16 zero byte stores before the copy were kept, and with them the slot
/// (forwarding needs one store per byte range); after full unrolling, a write-through counter's
/// memory copy nobody reads. Slots over 256 bytes are tracked in 256 chunks (a store kills only
/// the chunks it covers). Returns whether any store went.
pub fn dead_slot_stores(f: &mut Func) -> bool {
    let ns = f.slots.len();
    if ns == 0 {
        return false;
    }
    let SlotScan { vs, esc, .. } = slot_scan(f);
    const NONE: u32 = u32::MAX;
    // per tracked slot: first bit, bytes per bit, bit count
    let mut base = vec![NONE; ns];
    let mut gran = vec![1i64; ns];
    let mut nbits = vec![0i64; ns];
    let mut tot = 0usize;
    for s2 in 0..ns {
        let size = f.slots[s2].size as i64;
        if esc[s2] || size == 0 {
            continue;
        }
        let g = (size + 255) / 256;
        base[s2] = tot as u32;
        gran[s2] = g;
        nbits[s2] = (size + g - 1) / g;
        tot += nbits[s2] as usize;
    }
    if tot == 0 {
        return false;
    }
    let w = (tot + 63) / 64;
    let sl = |v: VReg| vs[v.0 as usize].0;
    // bits [a, b) of the bit vector
    fn set(s: &mut [u64], a: usize, b: usize) {
        for i in a..b {
            s[i / 64] |= 1 << (i % 64);
        }
    }
    fn clr(s: &mut [u64], a: usize, b: usize) {
        for i in a..b {
            s[i / 64] &= !(1 << (i % 64));
        }
    }
    fn any(s: &[u64], a: usize, b: usize) -> bool {
        (a..b).any(|i| s[i / 64] >> (i % 64) & 1 != 0)
    }
    // what an instruction does to a tracked slot: (slot, lo, hi) read and written byte ranges,
    // hi = i64::MAX: unknown offset (the whole slot)
    #[derive(Clone, Copy)]
    struct Acc {
        rd: Option<(u32, i64, i64)>,
        wr: Option<(u32, i64, i64)>,
    }
    let acc = |f: &Func, i: &Inst| -> Acc {
        let at = |v: VReg| vs[v.0 as usize].1;
        let tracked = |v: VReg| sl(v) != NONE && base[sl(v) as usize] != NONE;
        let mut a = Acc { rd: None, wr: None };
        let vw = |v: VReg| if f.vregs[v.0 as usize] == Cls::V256 { 32 } else { 16 };
        match *i {
            Inst::Load(m, _, b, o) if tracked(b) => a.rd = Some((sl(b), at(b) + o as i64, at(b) + o as i64 + mem_size(m) as i64)),
            Inst::VLoad(d, b, o) if tracked(b) => a.rd = Some((sl(b), at(b) + o as i64, at(b) + o as i64 + vw(d))),
            Inst::LoadX(_, _, b, ..) | Inst::VLoadX(_, b, ..) if tracked(b) => a.rd = Some((sl(b), 0, i64::MAX)),
            Inst::Store(m, b, o, _) if tracked(b) => a.wr = Some((sl(b), at(b) + o as i64, at(b) + o as i64 + mem_size(m) as i64)),
            Inst::VStore(b, o, v) if tracked(b) => a.wr = Some((sl(b), at(b) + o as i64, at(b) + o as i64 + vw(v))),
            Inst::StoreX(_, b, ..) | Inst::VStoreX(b, ..) if tracked(b) => a.wr = Some((sl(b), 0, i64::MAX)),
            Inst::Copy(d, s2, n) => {
                if tracked(s2) {
                    a.rd = Some((sl(s2), at(s2), at(s2) + n as i64));
                }
                if tracked(d) {
                    a.wr = Some((sl(d), at(d), at(d) + n as i64));
                }
            }
            _ => {}
        }
        a
    };
    // a byte range as bits: (overlapping bits, fully covered bits); None: out of the slot
    let bits = |s2: u32, lo: i64, hi: i64| -> Option<((usize, usize), (usize, usize))> {
        let s2 = s2 as usize;
        let (b0, g, n) = (base[s2] as i64, gran[s2], nbits[s2]);
        if hi == i64::MAX || lo < 0 || hi > f.slots[s2].size as i64 || lo >= hi {
            return None;
        }
        let over = ((b0 + lo / g) as usize, (b0 + (hi + g - 1) / g).min(b0 + n) as usize);
        let full_hi = if hi == f.slots[s2].size as i64 { n } else { hi / g };
        let cov = ((b0 + (lo + g - 1) / g) as usize, (b0 + full_hi) as usize);
        Some((over, if cov.0 < cov.1 { cov } else { (0, 0) }))
    };
    let whole = |s2: u32| (base[s2 as usize] as usize, (base[s2 as usize] as i64 + nbits[s2 as usize]) as usize);
    // backward transfer of one instruction; returns whether its write is dead
    let xfer = |live: &mut [u64], a: Acc| -> bool {
        let mut dead = false;
        if let Some((s2, lo, hi)) = a.wr {
            match bits(s2, lo, hi) {
                Some((over, cov)) => {
                    dead = !any(live, over.0, over.1);
                    clr(live, cov.0, cov.1);
                }
                None => {
                    let (x, y) = whole(s2);
                    dead = hi == i64::MAX && !any(live, x, y);
                }
            }
        }
        if let Some((s2, lo, hi)) = a.rd {
            match bits(s2, lo, hi) {
                Some((over, _)) => set(live, over.0, over.1),
                None => {
                    let (x, y) = whole(s2);
                    set(live, x, y);
                }
            }
        }
        dead
    };
    let nb = f.blocks.len();
    let reach = reachable(f);
    let mut order = relax_backend::frame::block_rpo(f);
    order.reverse();
    let mut live_in = vec![0u64; nb * w];
    let mut cur = vec![0u64; w];
    let mut changed = true;
    while changed {
        changed = false;
        for &b in &order {
            let bi = b as usize;
            if !reach[bi] {
                continue;
            }
            cur.iter_mut().for_each(|x| *x = 0);
            for t in f.blocks[bi].term.succs() {
                for j in 0..w {
                    cur[j] |= live_in[t as usize * w + j];
                }
            }
            for i in f.blocks[bi].insts.iter().rev() {
                let a = acc(f, i);
                xfer(&mut cur, a);
            }
            if live_in[bi * w..bi * w + w] != cur[..] {
                live_in[bi * w..bi * w + w].copy_from_slice(&cur);
                changed = true;
            }
        }
    }
    let mut dead: Vec<(usize, usize)> = Vec::new();
    for bi in 0..nb {
        if !reach[bi] {
            continue;
        }
        cur.iter_mut().for_each(|x| *x = 0);
        for t in f.blocks[bi].term.succs() {
            for j in 0..w {
                cur[j] |= live_in[t as usize * w + j];
            }
        }
        for k in (0..f.blocks[bi].insts.len()).rev() {
            let a = acc(f, &f.blocks[bi].insts[k]);
            if xfer(&mut cur, a) {
                dead.push((bi, k));
            }
        }
    }
    if dead.is_empty() {
        return false;
    }
    // indices were pushed in descending order per block: removal keeps the others valid
    for (bi, k) in dead {
        let b = &mut f.blocks[bi];
        b.insts.remove(k);
        if k < b.pos.len() {
            b.pos.remove(k);
        }
    }
    true
}

/// Slot coalescing along whole copies (O3): `Copy(C <- B)` of one slot's bytes into an equal slot
/// joins B and C into one slot when they do not interfere -- at every write of B the written bytes of
/// C are dead, and at every write of C those of B are, by backward byte liveness over the CFG (the
/// copy itself writes C with B's bytes and is exempt). The copy then vanishes. LLVM's SROA and
/// stack colouring reach this through SSA values; here the slots meet directly, also around
/// loops, where the block-reachability tests of forward_slot_dests_x always see an access
/// "after" the copy. pulldown-cmark's parse loop moved each 80-byte `Item` through three slots
/// (call result -> peek slot -> current slot -> match scrutinee) on every item.
/// A slot handed to calls only as the result buffer of an sret callee counts as fully written there
/// (the callee writes the value; bytes it leaves are padding or an inactive variant's, which Rust
/// leaves undefined). Returns whether any copy went.
pub fn coalesce_slot_copies(f: &mut Func) -> bool {
    const NONE: u32 = u32::MAX;
    let mut any = false;
    for _round in 0..8 {
        let ns = f.slots.len();
        if ns < 2 {
            return any;
        }
        let sc = slot_scan(f);
        let vs = &sc.vs;
        let sl = |v: VReg| vs[v.0 as usize].0;
        // a result buffer written by one sret call and accessed otherwise only by loads, stores and
        // copies: tracked, with the call as a whole write
        let mut sret_call: Vec<Option<(usize, usize)>> = vec![None; ns];
        for s2 in 0..ns {
            if let Some((bi, ii)) = sc.ret_call[s2] {
                if bi == usize::MAX {
                    continue;
                }
                if let Inst::Call(Callee::Fn(id), args, _) = &f.blocks[bi].insts[ii] {
                    let ok = f.sret_callees.contains(id) && args.first().map_or(false, |a| vs[a.0 as usize] == (s2 as u32, 0)) && !args[1..].iter().any(|a| sl(*a) == s2 as u32);
                    if ok {
                        sret_call[s2] = Some((bi, ii));
                    }
                }
            }
        }
        let tracked: Vec<bool> = (0..ns).map(|s2| f.slots[s2].size > 0 && (!sc.esc[s2] || sret_call[s2].is_some())).collect();
        // candidate copies: whole equal-size slots, both tracked
        let mut cands: Vec<(usize, usize, u32, u32)> = Vec::new();
        for (bi, b) in f.blocks.iter().enumerate() {
            for (ii, i) in b.insts.iter().enumerate() {
                if let Inst::Copy(d, s2, n) = *i {
                    let (cs, bs) = (sl(d), sl(s2));
                    if cs != NONE && bs != NONE && cs != bs && vs[d.0 as usize].1 == 0 && vs[s2.0 as usize].1 == 0 && tracked[cs as usize] && tracked[bs as usize] {
                        let (zc, zb) = (f.slots[cs as usize].size, f.slots[bs as usize].size);
                        if n == zc && n == zb {
                            cands.push((bi, ii, bs, cs));
                        }
                    }
                }
            }
        }
        if cands.is_empty() {
            return any;
        }
        // byte liveness of the tracked slots (bits of up to 256 per slot, as dead_slot_stores)
        let mut base = vec![NONE; ns];
        let mut gran = vec![1i64; ns];
        let mut nbits = vec![0i64; ns];
        let mut tot = 0usize;
        for s2 in 0..ns {
            if !tracked[s2] {
                continue;
            }
            let size = f.slots[s2].size as i64;
            let g = (size + 255) / 256;
            base[s2] = tot as u32;
            gran[s2] = g;
            nbits[s2] = (size + g - 1) / g;
            tot += nbits[s2] as usize;
        }
        let w = (tot + 63) / 64;
        // (slot, lo, hi) read / written by an instruction; hi = i64::MAX: anywhere in the slot
        let acc = |i: &Inst, bi: usize, ii: usize| -> (Option<(u32, i64, i64)>, Option<(u32, i64, i64)>) {
            let at = |v: VReg| vs[v.0 as usize].1;
            let tr = |v: VReg| sl(v) != NONE && tracked[sl(v) as usize];
            let vw = |v: VReg| if f.vregs[v.0 as usize] == Cls::V256 { 32 } else { 16 };
            match *i {
                Inst::Load(m, _, b, o) if tr(b) => (Some((sl(b), at(b) + o as i64, at(b) + o as i64 + mem_size(m) as i64)), None),
                Inst::VLoad(d, b, o) if tr(b) => (Some((sl(b), at(b) + o as i64, at(b) + o as i64 + vw(d))), None),
                Inst::LoadX(_, _, b, ..) | Inst::VLoadX(_, b, ..) if tr(b) => (Some((sl(b), 0, i64::MAX)), None),
                Inst::Store(m, b, o, _) if tr(b) => (None, Some((sl(b), at(b) + o as i64, at(b) + o as i64 + mem_size(m) as i64))),
                Inst::VStore(b, o, v) if tr(b) => (None, Some((sl(b), at(b) + o as i64, at(b) + o as i64 + vw(v)))),
                Inst::StoreX(_, b, ..) | Inst::VStoreX(b, ..) if tr(b) => (None, Some((sl(b), 0, i64::MAX))),
                Inst::Copy(d, s2, n) => (
                    if tr(s2) { Some((sl(s2), at(s2), at(s2) + n as i64)) } else { None },
                    if tr(d) { Some((sl(d), at(d), at(d) + n as i64)) } else { None },
                ),
                Inst::Call(..) => {
                    let mut wr = None;
                    for s2 in 0..ns {
                        if sret_call[s2] == Some((bi, ii)) {
                            wr = Some((s2 as u32, 0, f.slots[s2].size as i64));
                        }
                    }
                    (None, wr)
                }
                _ => (None, None),
            }
        };
        // a byte range as bits: (overlapping, fully covered); None: unknown offset (whole slot)
        let bits = |s2: u32, lo: i64, hi: i64| -> Option<((usize, usize), (usize, usize))> {
            let s2 = s2 as usize;
            let (b0, g, n) = (base[s2] as i64, gran[s2], nbits[s2]);
            if hi == i64::MAX || lo < 0 || hi > f.slots[s2].size as i64 || lo >= hi {
                return None;
            }
            let over = ((b0 + lo / g) as usize, (b0 + (hi + g - 1) / g).min(b0 + n) as usize);
            let full_hi = if hi == f.slots[s2].size as i64 { n } else { hi / g };
            let cov = ((b0 + (lo + g - 1) / g) as usize, (b0 + full_hi) as usize);
            Some((over, if cov.0 < cov.1 { cov } else { (0, 0) }))
        };
        let whole = |s2: u32| (base[s2 as usize] as usize, (base[s2 as usize] as i64 + nbits[s2 as usize]) as usize);
        fn setb(s: &mut [u64], a: usize, b: usize) {
            for i in a..b {
                s[i / 64] |= 1 << (i % 64);
            }
        }
        fn clrb(s: &mut [u64], a: usize, b: usize) {
            for i in a..b {
                s[i / 64] &= !(1 << (i % 64));
            }
        }
        fn anyb(s: &[u64], a: usize, b: usize) -> bool {
            (a..b).any(|i| s[i / 64] >> (i % 64) & 1 != 0)
        }
        let xfer = |live: &mut [u64], rd: Option<(u32, i64, i64)>, wr: Option<(u32, i64, i64)>| {
            if let Some((s2, lo, hi)) = wr {
                if let Some((_, cov)) = bits(s2, lo, hi) {
                    clrb(live, cov.0, cov.1);
                }
            }
            if let Some((s2, lo, hi)) = rd {
                match bits(s2, lo, hi) {
                    Some((over, _)) => setb(live, over.0, over.1),
                    None => {
                        let (x, y) = whole(s2);
                        setb(live, x, y);
                    }
                }
            }
        };
        let nb = f.blocks.len();
        let reach = reachable(f);
        let mut order = relax_backend::frame::block_rpo(f);
        order.reverse();
        let mut live_in = vec![0u64; nb * w];
        let mut cur = vec![0u64; w];
        let mut changed = true;
        while changed {
            changed = false;
            for &b in &order {
                let bi = b as usize;
                if !reach[bi] {
                    continue;
                }
                cur.iter_mut().for_each(|x| *x = 0);
                for t in f.blocks[bi].term.succs() {
                    for j in 0..w {
                        cur[j] |= live_in[t as usize * w + j];
                    }
                }
                for (ii, i) in f.blocks[bi].insts.iter().enumerate().rev() {
                    let (rd, wr) = acc(i, bi, ii);
                    xfer(&mut cur, rd, wr);
                }
                if live_in[bi * w..bi * w + w] != cur[..] {
                    live_in[bi * w..bi * w + w].copy_from_slice(&cur);
                    changed = true;
                }
            }
        }
        // per candidate: interference found
        let mut bad = vec![false; cands.len()];
        // slot -> candidates it is in
        let mut of_slot: Vec<Vec<usize>> = vec![Vec::new(); ns];
        for (k, &(_, _, bs, cs)) in cands.iter().enumerate() {
            of_slot[bs as usize].push(k);
            of_slot[cs as usize].push(k);
        }
        for bi in 0..nb {
            if !reach[bi] {
                continue;
            }
            cur.iter_mut().for_each(|x| *x = 0);
            for t in f.blocks[bi].term.succs() {
                for j in 0..w {
                    cur[j] |= live_in[t as usize * w + j];
                }
            }
            for ii in (0..f.blocks[bi].insts.len()).rev() {
                let (rd, wr) = acc(&f.blocks[bi].insts[ii], bi, ii);
                // `cur` is live after this instruction: a write of one candidate slot needs the
                // partner's same bytes dead here
                if let Some((s2, lo, hi)) = wr {
                    for &k in &of_slot[s2 as usize] {
                        let (cb, ci, bs, cs) = cands[k];
                        if (cb, ci) == (bi, ii) || bad[k] {
                            continue;
                        }
                        let other = if s2 == bs { cs } else { bs };
                        let hit = match bits(other, lo, hi) {
                            Some((over, _)) => anyb(&cur, over.0, over.1),
                            None => {
                                let (x, y) = whole(other);
                                anyb(&cur, x, y)
                            }
                        };
                        if hit {
                            bad[k] = true;
                        }
                    }
                }
                xfer(&mut cur, rd, wr);
            }
        }
        // join the first good candidate per slot pair (slots may sit in several candidates)
        let mut used = vec![false; ns];
        let mut to: Vec<u32> = (0..ns as u32).collect();
        let mut dels: Vec<(usize, usize)> = Vec::new();
        for (k, &(cb, ci, bs, cs)) in cands.iter().enumerate() {
            if bad[k] || used[bs as usize] || used[cs as usize] {
                continue;
            }
            used[bs as usize] = true;
            used[cs as usize] = true;
            to[bs as usize] = cs;
            let al = f.slots[bs as usize].align.max(f.slots[cs as usize].align);
            f.slots[cs as usize].align = al;
            dels.push((cb, ci));
        }
        if dels.is_empty() {
            return any;
        }
        dels.sort_unstable_by(|x, y| y.cmp(x));
        for (cb, ci) in dels {
            f.blocks[cb].insts.remove(ci);
            if ci < f.blocks[cb].pos.len() {
                f.blocks[cb].pos.remove(ci);
            }
        }
        for b in f.blocks.iter_mut() {
            for i in b.insts.iter_mut() {
                if let Inst::SlotAddr(_, s2) = i {
                    *s2 = to[*s2 as usize];
                }
            }
        }
        any = true;
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
