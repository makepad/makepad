//! Switch lowering: chains of compares of one integer value against constants (an integer or
//! `char` match, or an enum's discriminant match, in the order the arms were written) become the
//! cheapest of
//!   * a jump table (`Term::Switch`) for a dense set of values,
//!   * bit tests (one shift-and-mask per target) for a few targets over a span of at most 64,
//!   * a balanced binary search over the case ranges, ending in one range test (`x - lo <= hi - lo`,
//!     unsigned) per range,
//! the way rustc's `SwitchInt` is lowered (LLVM's switch lowering: dense clusters, bit-test clusters
//! and a binary tree over the rest).
//!
//! Runs on the function just before register allocation (optimising tiers only): the optimiser's
//! passes see the original compare chains, so none of them has to know a multi-way terminator.
//! A chain is found as the tests reachable from a head block through blocks that hold only a
//! compare of the same value against a constant; the value sets that reach each outcome are
//! tracked as ranges, so `a..=z`, `x == 3` and `x < 9` tests all join one switch. The chain's blocks
//! stay for their other entries and drop out when nothing else enters them.

use crate::regalloc::{for_defs, for_uses, term_uses};
use crate::rir::*;

/// A block that ends in `c = x <cond> k; branch c`: (x, cond, signed, k, taken, not taken).
#[derive(Clone, Copy)]
struct Test {
    x: VReg,
    cond: Cond,
    signed: bool,
    k: i64,
    t: u32,
    e: u32,
}

/// Ranges of values in a total order (the signed domain is the signed values shifted to start at 0).
type Ranges = Vec<(u64, u64)>;

fn enc(v: i64, signed: bool) -> u64 {
    if signed {
        (v as u64) ^ (1 << 63)
    } else {
        v as u64
    }
}

fn dec(v: u64, signed: bool) -> i64 {
    if signed {
        (v ^ (1 << 63)) as i64
    } else {
        v as i64
    }
}

/// The values for which `x <cond> k` holds.
fn holds(cond: Cond, k: u64) -> Ranges {
    match cond {
        Cond::Eq => vec![(k, k)],
        Cond::Ne => {
            let mut v = Vec::new();
            if k > 0 {
                v.push((0, k - 1));
            }
            if k < u64::MAX {
                v.push((k + 1, u64::MAX));
            }
            v
        }
        Cond::Lt => {
            if k == 0 {
                Vec::new()
            } else {
                vec![(0, k - 1)]
            }
        }
        Cond::Le => vec![(0, k)],
        Cond::Gt => {
            if k == u64::MAX {
                Vec::new()
            } else {
                vec![(k + 1, u64::MAX)]
            }
        }
        Cond::Ge => vec![(k, u64::MAX)],
    }
}

fn intersect(a: &Ranges, b: &Ranges) -> Ranges {
    let mut out = Vec::new();
    for &(al, ah) in a {
        for &(bl, bh) in b {
            let (l, h) = (al.max(bl), ah.min(bh));
            if l <= h {
                out.push((l, h));
            }
        }
    }
    out
}

fn subtract(a: &Ranges, b: &Ranges) -> Ranges {
    let mut cur: Ranges = a.clone();
    for &(bl, bh) in b {
        let mut next = Vec::with_capacity(cur.len() + 1);
        for (l, h) in cur {
            if bh < l || bl > h {
                next.push((l, h));
                continue;
            }
            if l < bl {
                next.push((l, bl - 1));
            }
            if h > bh {
                next.push((bh + 1, h));
            }
        }
        cur = next;
    }
    cur
}

struct Chain<'a> {
    tests: &'a [Option<Test>],
    f: &'a Func,
    x: VReg,
    signed: bool,
    /// the order of the compares was fixed by a signed or unsigned relation
    fixed: bool,
    budget: u32,
    leaves: Vec<(u32, Ranges)>,
}

enum Walk {
    Done,
    /// a relational compare of the other signedness: walk again in that domain
    Again(bool),
    Fail,
}

impl Chain<'_> {
    fn walk(&mut self, b: u32, s: Ranges, head: bool) -> Walk {
        if self.budget == 0 {
            return Walk::Fail;
        }
        self.budget -= 1;
        let link = match self.tests[b as usize] {
            Some(t) if t.x == self.x && (head || self.f.blocks[b as usize].insts.len() == 1) => Some(t),
            _ => None,
        };
        let Some(t) = link else {
            match self.leaves.iter_mut().find(|l| l.0 == b) {
                Some(l) => l.1.extend(s),
                None => self.leaves.push((b, s)),
            }
            return Walk::Done;
        };
        if !matches!(t.cond, Cond::Eq | Cond::Ne) {
            if self.fixed && self.signed != t.signed {
                return Walk::Again(t.signed);
            }
            if !self.fixed {
                self.fixed = true;
                if self.signed != t.signed {
                    return Walk::Again(t.signed);
                }
            }
        }
        let p = holds(t.cond, enc(t.k, self.signed));
        let yes = intersect(&s, &p);
        let no = subtract(&s, &p);
        if !yes.is_empty() {
            match self.walk(t.t, yes, false) {
                Walk::Done => {}
                r => return r,
            }
        }
        if !no.is_empty() {
            match self.walk(t.e, no, false) {
                Walk::Done => {}
                r => return r,
            }
        }
        Walk::Done
    }
}

/// A run of values that go to one block.
#[derive(Clone, Copy)]
struct Cluster {
    lo: u64,
    hi: u64,
    to: u32,
}

enum Node {
    Goto(u32),
    /// `lo <= x <= hi` goes to `to`, else on
    Range { lo: u64, hi: u64, to: u32, no: Box<Node> },
    /// `x < pivot` goes left
    Split { pivot: u64, lt: Box<Node>, ge: Box<Node> },
    /// at most 64 values from `lo`: per target the mask of its values; others go `default`
    Bits { lo: u64, span: u64, groups: Vec<(u32, u64)>, default: u32 },
    Table { lo: u64, targets: Vec<u32>, default: u32 },
}

const MIN_TABLE_CLUSTERS: usize = 4;
/// jump table density: at most this many table entries per cluster
const MAX_SPAN_PER_CLUSTER: u128 = 4;
const MAX_TABLE: u128 = 4096;

struct Planner {
    default: u32,
}

impl Planner {
    fn plan(&self, cl: &[Cluster]) -> Node {
        let n = cl.len();
        if n == 0 {
            return Node::Goto(self.default);
        }
        let lo = cl[0].lo;
        let hi = cl[n - 1].hi;
        let span = hi as u128 - lo as u128 + 1;
        // distinct targets
        let mut dests: Vec<u32> = Vec::new();
        let mut cmps = 0usize;
        for c in cl {
            if !dests.contains(&c.to) {
                dests.push(c.to);
            }
            cmps += if c.lo == c.hi { 1 } else { 2 };
        }
        if span <= 64 && dests.len() <= 3 && ((dests.len() == 1 && cmps >= 3) || (dests.len() == 2 && cmps >= 5) || (dests.len() == 3 && cmps >= 6)) {
            let mut groups: Vec<(u32, u64)> = dests.iter().map(|d| (*d, 0u64)).collect();
            for c in cl {
                let g = groups.iter_mut().find(|g| g.0 == c.to).unwrap();
                for v in c.lo..=c.hi {
                    g.1 |= 1u64 << (v - lo);
                }
            }
            return Node::Bits { lo, span: span as u64, groups, default: self.default };
        }
        // a table pays when its entries are mostly distinct clusters: few clusters over a wide span
        // (identifier characters, say) are cheaper as range compares or bit tests
        if n >= MIN_TABLE_CLUSTERS && span <= MAX_TABLE && span <= MAX_SPAN_PER_CLUSTER * n as u128 {
            let mut targets = vec![self.default; span as usize];
            for c in cl {
                for v in c.lo..=c.hi {
                    targets[(v - lo) as usize] = c.to;
                }
            }
            return Node::Table { lo, targets, default: self.default };
        }
        if n <= 3 {
            let mut node = Node::Goto(self.default);
            for c in cl.iter().rev() {
                node = Node::Range { lo: c.lo, hi: c.hi, to: c.to, no: Box::new(node) };
            }
            return node;
        }
        let mid = n / 2;
        Node::Split { pivot: cl[mid].lo, lt: Box::new(self.plan(&cl[..mid])), ge: Box::new(self.plan(&cl[mid..])) }
    }
}

struct Emit<'a> {
    f: &'a mut Func,
    x: VReg,
    signed: bool,
    pos: u64,
}

impl Emit<'_> {
    fn int(&mut self, b: u32, i: Inst) {
        let blk = &mut self.f.blocks[b as usize];
        blk.insts.push(i);
        blk.pos.push(self.pos);
    }
    fn reg(&mut self) -> VReg {
        self.f.vreg(Cls::I)
    }
    /// `c = x <cond> k`: an immediate form where the back ends encode `k` (a signed 32-bit
    /// immediate), else against a constant register.
    fn cmp_k(&mut self, b: u32, cond: Cond, signed: bool, c: VReg, x: VReg, k: i64) {
        if k as i32 as i64 == k {
            self.int(b, Inst::ICmpI(cond, signed, c, x, k));
        } else {
            let kr = self.iconst(b, k);
            self.int(b, Inst::ICmp(cond, signed, c, x, kr));
        }
    }
    /// `d = x - k` (64-bit), as `cmp_k`.
    fn sub_k(&mut self, b: u32, d: VReg, x: VReg, k: i64) {
        if k as i32 as i64 == k {
            self.int(b, Inst::IBinI(IOp::Sub, U64, d, x, k));
        } else {
            let kr = self.iconst(b, k);
            self.int(b, Inst::IBin(IOp::Sub, U64, d, x, kr));
        }
    }
    fn iconst(&mut self, b: u32, k: i64) -> VReg {
        let r = self.reg();
        self.int(b, Inst::Iconst(r, k));
        r
    }

    /// Block that runs `n`: its goto target, or a new block.
    fn block_for(&mut self, n: Node) -> u32 {
        if let Node::Goto(t) = n {
            return t;
        }
        let b = self.f.block();
        self.f.blocks[b as usize].term_pos = self.pos;
        self.fill(b, n);
        b
    }

    /// Writes `n` into the empty block `b`.
    fn fill(&mut self, b: u32, n: Node) {
        let signed = self.signed;
        match n {
            Node::Goto(t) => self.f.blocks[b as usize].term = Term::Jump(t),
            Node::Range { lo, hi, to, no } => {
                let no = self.block_for(*no);
                let c = self.reg();
                let x = self.x;
                let (rl, rh) = (dec(lo, signed), dec(hi, signed));
                if lo == hi {
                    self.cmp_k(b, Cond::Eq, false, c, x, rl);
                } else if lo == 0 {
                    self.cmp_k(b, Cond::Le, signed, c, x, rh);
                } else if hi == u64::MAX {
                    self.cmp_k(b, Cond::Ge, signed, c, x, rl);
                } else {
                    let t = self.reg();
                    self.sub_k(b, t, x, rl);
                    self.cmp_k(b, Cond::Le, false, c, t, hi.wrapping_sub(lo) as i64);
                }
                self.f.blocks[b as usize].term = Term::Branch(c, to, no);
            }
            Node::Split { pivot, lt, ge } => {
                let lt = self.block_for(*lt);
                let ge = self.block_for(*ge);
                let c = self.reg();
                let x = self.x;
                self.cmp_k(b, Cond::Lt, signed, c, x, dec(pivot, signed));
                self.f.blocks[b as usize].term = Term::Branch(c, lt, ge);
            }
            Node::Bits { lo, span, groups, default } => {
                // t = x - lo; if t > span - 1 { default }; then per target: (mask >> t) & 1
                let x = self.x;
                let t = self.reg();
                self.sub_k(b, t, x, dec(lo, signed));
                let inr = self.reg();
                self.cmp_k(b, Cond::Le, false, inr, t, span as i64 - 1);
                let n = groups.len();
                let blocks: Vec<u32> = groups
                    .iter()
                    .map(|_| {
                        let nb = self.f.block();
                        self.f.blocks[nb as usize].term_pos = self.pos;
                        nb
                    })
                    .collect();
                self.f.blocks[b as usize].term = Term::Branch(inr, blocks[0], default);
                for (i, (to, mask)) in groups.into_iter().enumerate() {
                    let at = blocks[i];
                    let m = self.iconst(at, mask as i64);
                    let s = self.reg();
                    self.int(at, Inst::IBin(IOp::Shr, U64, s, m, t));
                    let bit = self.reg();
                    self.int(at, Inst::IBinI(IOp::And, U64, bit, s, 1));
                    let no = if i + 1 < n { blocks[i + 1] } else { default };
                    self.f.blocks[at as usize].term = Term::Branch(bit, to, no);
                }
            }
            Node::Table { lo, targets, default } => {
                let x = self.x;
                self.f.blocks[b as usize].term = Term::Switch(x, std::sync::Arc::new(SwitchTable::new(dec(lo, signed), targets, default)));
            }
        }
    }
}

/// Switch lowering of every chain of `f` (see the module comment). None when nothing changed; else
/// the new function and, per its block, the block of `f` whose execution frequency it shares (the
/// blocks are renumbered, and block frequencies are by number).
pub fn lower_switches(options: &relax_session::Options, f: &Func) -> Option<(Func, Vec<u32>)> {
    if options.no_jump_tables || f.baseline {
        return None;
    }
    // cheap pre-scan: a chain needs a few blocks ending in a compare and a branch on it
    let mut cands = 0usize;
    for b in &f.blocks {
        if let (Term::Branch(c, _, _), Some(Inst::ICmpI(_, _, d, _, _) | Inst::ICmp(_, _, d, _, _))) = (&b.term, b.insts.last()) {
            if d == c {
                cands += 1;
            }
        }
    }
    if cands < 3 {
        return None;
    }
    let nv = f.vregs.len();
    let mut uses = vec![0u32; nv];
    let mut ndef = vec![0u32; nv];
    let mut kval: Vec<i64> = vec![0; nv];
    let mut tu = Vec::new();
    for b in &f.blocks {
        for i in &b.insts {
            for_uses(i, |u| uses[u.0 as usize] += 1);
            for_defs(i, |d| ndef[d.0 as usize] += 1);
            if let Inst::Iconst(d, k) = i {
                kval[d.0 as usize] = *k;
            }
        }
        term_uses(&b.term, &mut tu);
        for u in &tu {
            uses[u.0 as usize] += 1;
        }
    }
    for p in &f.params {
        ndef[p.0 as usize] += 2;
    }
    let konst = |v: VReg| -> Option<i64> {
        if ndef[v.0 as usize] != 1 {
            return None;
        }
        // single definition: an Iconst anywhere (the def dominates its uses)
        let k = kval[v.0 as usize];
        Some(k)
    };
    let is_const = {
        let mut m = vec![false; nv];
        for b in &f.blocks {
            for i in &b.insts {
                if let Inst::Iconst(d, _) = i {
                    m[d.0 as usize] = ndef[d.0 as usize] == 1;
                }
            }
        }
        m
    };
    let konst = |v: VReg| -> Option<i64> { if is_const[v.0 as usize] { konst(v) } else { None } };
    let nb = f.blocks.len();
    let mut tests: Vec<Option<Test>> = vec![None; nb];
    for (bi, b) in f.blocks.iter().enumerate() {
        let Term::Branch(c, t, e) = b.term else { continue };
        if t == e || uses[c.0 as usize] != 1 {
            continue;
        }
        tests[bi] = match b.insts.last() {
            Some(Inst::ICmpI(cond, signed, d, x, k)) if *d == c && *x != c => Some(Test { x: *x, cond: *cond, signed: *signed, k: *k, t, e }),
            Some(Inst::ICmp(cond, signed, d, x, y)) if *d == c && *x != c && *y != c => match (konst(*x), konst(*y)) {
                (_, Some(k)) => Some(Test { x: *x, cond: *cond, signed: *signed, k, t, e }),
                (Some(k), None) => Some(Test { x: *y, cond: swap(*cond), signed: *signed, k, t, e }),
                _ => None,
            },
            _ => None,
        };
    }
    // a block holding only its compare, entered from a test of the same value, continues that chain
    let mut is_link = vec![false; nb];
    for bi in 0..nb {
        if let Some(t) = tests[bi] {
            for s in [t.t, t.e] {
                let s = s as usize;
                if s != bi && f.blocks[s].insts.len() == 1 && matches!(tests[s], Some(u) if u.x == t.x) {
                    is_link[s] = true;
                }
            }
        }
    }
    let mut out: Option<Func> = None;
    // per block of the result: the block of `f` whose execution frequency it shares
    let mut origin: Vec<u32> = (0..nb as u32).collect();
    for head in 0..nb {
        let Some(h) = tests[head] else { continue };
        if is_link[head] {
            continue;
        }
        let mut chain = Chain { tests: &tests, f, x: h.x, signed: false, fixed: false, budget: 600, leaves: Vec::new() };
        let mut tries = 0;
        let ok = loop {
            chain.leaves.clear();
            chain.budget = 600;
            match chain.walk(head as u32, vec![(0, u64::MAX)], true) {
                Walk::Done => break true,
                Walk::Again(s) if tries == 0 => {
                    tries += 1;
                    chain.signed = s;
                    chain.fixed = true;
                }
                _ => break false,
            }
        };
        if !ok {
            continue;
        }
        let signed = chain.signed;
        let mut leaves = std::mem::take(&mut chain.leaves);
        // per target: sorted merged ranges
        for l in &mut leaves {
            l.1.sort();
            let mut m: Ranges = Vec::new();
            for &(lo, hi) in &l.1 {
                match m.last_mut() {
                    Some(last) if lo <= last.1.saturating_add(1) => last.1 = last.1.max(hi),
                    _ => m.push((lo, hi)),
                }
            }
            l.1 = m;
        }
        // the default: the target covering the most values
        let measure = |r: &Ranges| -> u128 { r.iter().map(|(l, h)| *h as u128 - *l as u128 + 1).sum() };
        let di = (0..leaves.len()).max_by_key(|i| measure(&leaves[*i].1)).unwrap();
        let default = leaves[di].0;
        let mut cl: Vec<Cluster> = Vec::new();
        for (i, (to, rs)) in leaves.iter().enumerate() {
            if i != di {
                for &(lo, hi) in rs {
                    cl.push(Cluster { lo, hi, to: *to });
                }
            }
        }
        cl.sort_by_key(|c| c.lo);
        if cl.len() < 3 {
            continue;
        }
        let planner = Planner { default };
        let node = planner.plan(&cl);
        let g = out.get_or_insert_with(|| f.clone());
        let pos = g.blocks[head].term_pos;
        // the head's compare is replaced by the switch
        let hb = &mut g.blocks[head];
        hb.insts.pop();
        hb.pos.pop();
        let mut em = Emit { f: g, x: h.x, signed, pos };
        let before = em.f.blocks.len();
        em.fill(head as u32, node);
        for _ in before..em.f.blocks.len() {
            origin.push(head as u32);
        }
    }
    let mut g = out?;
    let order = crate::dom::relayout_order(&mut g);
    let origin: Vec<u32> = order.iter().map(|b| origin[*b as usize]).collect();
    Some((g, origin))
}

fn swap(c: Cond) -> Cond {
    match c {
        Cond::Eq => Cond::Eq,
        Cond::Ne => Cond::Ne,
        Cond::Lt => Cond::Gt,
        Cond::Le => Cond::Ge,
        Cond::Gt => Cond::Lt,
        Cond::Ge => Cond::Le,
    }
}

/// Per block: its `Term::Switch` (min 0) needs no range test -- every predecessor reaches it on
/// the edge of an unsigned `x < k` / `x <= k` test of the switch value that fits the table (an
/// enum match whose arms past the table were split off first: rustc's `otherwise: unreachable`).
/// The emitters then index the table straight away.
pub fn switch_in_range(f: &Func) -> Vec<bool> {
    let n = f.blocks.len();
    let mut out = vec![false; n];
    if !f.blocks.iter().any(|b| matches!(b.term, Term::Switch(..))) {
        return out;
    }
    let reach = crate::regalloc::reachable(f);
    let mut preds: Vec<Vec<u32>> = vec![Vec::new(); n];
    for (bi, b) in f.blocks.iter().enumerate() {
        if reach[bi] {
            for t in b.term.succs() {
                preds[t as usize].push(bi as u32);
            }
        }
    }
    for (bi, b) in f.blocks.iter().enumerate() {
        let Term::Switch(x, tab) = &b.term else { continue };
        if !reach[bi] || tab.min != 0 || preds[bi].is_empty() {
            continue;
        }
        let mut redefined = false;
        for i in &b.insts {
            for_defs(i, |d| redefined |= d == *x);
        }
        if redefined {
            continue;
        }
        let len = tab.targets.len() as i64;
        let ok = preds[bi].iter().all(|&p| {
            let pb = &f.blocks[p as usize];
            let Term::Branch(c, t, e) = pb.term else { return false };
            if t == e {
                return false;
            }
            // the compare that last defines c, and x not redefined after it
            let mut fact = None;
            for i in pb.insts.iter().rev() {
                let mut dc = false;
                let mut dx = false;
                for_defs(i, |d| {
                    dc |= d == c;
                    dx |= d == *x;
                });
                if dc {
                    if let Inst::ICmpI(cond, false, _, a, k) = *i {
                        if a == *x {
                            fact = Some((cond, k));
                        }
                    }
                    break;
                }
                if dx {
                    break;
                }
            }
            let Some((cond, k)) = fact else { return false };
            // the largest x on the edge into the switch block
            let hi = match (cond, t as usize == bi) {
                (Cond::Lt, true) | (Cond::Ge, false) => k.checked_sub(1),
                (Cond::Le, true) | (Cond::Gt, false) => Some(k),
                _ => None,
            };
            matches!(hi, Some(h) if h >= 0 && h < len)
        });
        out[bi] = ok;
    }
    out
}
