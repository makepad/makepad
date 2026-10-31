//! Compiled, cached cost analysis. The host decides admission policy.
use makepad_relax_api::relax_api;
use crate::ir::{Block, Op, Program, Stmt};
use crate::kernel::{Kernel, KernelBody, KernelCtl};

/// The worst case of one element of `kernel`, computed on first ask.
pub fn element_cost(kernel: &Kernel) -> ElementCost {
    let mut cost = ElementCost::default();
    crate::kernel::kernel_ctl(kernel, KernelCtl::Cost(&mut cost));
    cost
}

/// One element's worst case, split so host calls can be bounded per call.
#[relax_api(plain)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[repr(C)]
pub struct ElementCost {
    /// Everything but host calls whose input size is only known at run
    /// time (host calls on constant-size slices included).
    pub fixed_ops: u64,
    /// Host calls per element whose slice lengths are run-time values
    /// (loop caps included): each is bounded by the per-call limit.
    pub open_calls: u64,
    /// Those calls at their declared cost for their largest input.
    pub open_max_ops: u64,
}

impl ElementCost {
    /// The element's worst case when each open call may run `call_ops`.
    pub fn with_call_limit(&self, call_ops: u64) -> u64 {
        self.fixed_ops.saturating_add(self.open_calls.saturating_mul(call_ops))
    }

    fn add(self, o: ElementCost) -> ElementCost {
        ElementCost {
            fixed_ops: self.fixed_ops.saturating_add(o.fixed_ops),
            open_calls: self.open_calls.saturating_add(o.open_calls),
            open_max_ops: self.open_max_ops.saturating_add(o.open_max_ops),
        }
    }

    fn max(self, o: ElementCost) -> ElementCost {
        ElementCost { fixed_ops: self.fixed_ops.max(o.fixed_ops), open_calls: self.open_calls.max(o.open_calls), open_max_ops: self.open_max_ops.max(o.open_max_ops) }
    }

    fn times(self, n: u64) -> ElementCost {
        ElementCost { fixed_ops: self.fixed_ops.saturating_mul(n), open_calls: self.open_calls.saturating_mul(n), open_max_ops: self.open_max_ops.saturating_mul(n) }
    }

    fn fixed(ops: u64) -> ElementCost {
        ElementCost { fixed_ops: ops, ..Default::default() }
    }
}

/// Declared ops of one call of `h` with slice lengths `lens` (a length the
/// compiler knows, else the signature's maximum).
fn host_ops(h: &crate::host::HostFn, lens: &[Option<u32>]) -> u64 {
    let lens: Vec<u32> = lens.iter().zip(h.slices).map(|(l, s)| l.unwrap_or(s.max_words)).collect();
    h.cost_of(&lens)
}

/// The worst case of one element of `kernel`: what [`crate::work`] can
/// count for it at most (each statement its [`crate::work::op`], each pass
/// of a loop its body's ops and two, every loop at its cap, the element
/// loop once, each branch at its dearer side, a function call its body, a
/// host call its declared cost for its input).
pub(crate) fn calculate_cost(kernel: &KernelBody) -> ElementCost {
    let program = kernel.program();
    fn consts(b: &Block, out: &mut std::collections::HashMap<u32, i32>) {
        for s in b {
            match s {
                Stmt::Def(v, Op::ConstI(c)) => {
                    out.insert(v.0, *c);
                }
                Stmt::If(_, t, e) => {
                    consts(t, out);
                    consts(e, out);
                }
                Stmt::Loop { body, .. } => consts(body, out),
                _ => {}
            }
        }
    }
    fn walk(b: &Block, depth: u32, funcs: &[Program], c: &std::collections::HashMap<u32, i32>) -> ElementCost {
        let mut sum = ElementCost::default();
        for s in b {
            let x = match s {
                Stmt::If(_, t, e) => ElementCost::fixed(1).add(walk(t, depth, funcs, c).max(walk(e, depth, funcs, c))),
                Stmt::Loop { cap, body } => {
                    let reps = if depth == 0 { 1 } else { *cap as u64 };
                    walk(body, depth + 1, funcs, c).add(ElementCost::fixed(2)).times(reps)
                }
                Stmt::Call { f, .. } => match funcs.get(*f as usize) {
                    Some(g) => {
                        let mut gc = std::collections::HashMap::new();
                        consts(&g.body, &mut gc);
                        ElementCost::fixed(crate::work::op(s)).add(walk(&g.body, depth.max(1), funcs, &gc))
                    }
                    None => ElementCost::fixed(u64::MAX / 4),
                },
                Stmt::CallHost { f, slices, .. } => {
                    let call = ElementCost::fixed(crate::work::op(s));
                    match crate::host::get(*f) {
                        Some(h) => {
                            let lens: Vec<Option<u32>> = slices.iter().map(|x| c.get(&x.len.0).map(|n| *n as u32)).collect();
                            if lens.iter().all(|l| l.is_some()) {
                                call.add(ElementCost::fixed(host_ops(&h, &lens)))
                            } else {
                                call.add(ElementCost { fixed_ops: 0, open_calls: 1, open_max_ops: host_ops(&h, &vec![None; lens.len()]) })
                            }
                        }
                        // An unknown index costs everything.
                        None => ElementCost::fixed(u64::MAX / 4),
                    }
                }
                s => ElementCost::fixed(crate::work::op(s)),
            };
            sum = sum.add(x);
        }
        sum
    }
    let mut c = std::collections::HashMap::new();
    consts(&program.body, &mut c);
    walk(&program.body, 0, &program.funcs, &c)
}

/// One element's worst case with every host call at its largest input
/// (the trusted bound).
pub fn element_ops(kernel: &Kernel) -> u64 {
    let c = element_cost(kernel);
    c.fixed_ops.saturating_add(c.open_max_ops)
}

