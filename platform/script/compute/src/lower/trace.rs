//! A kernel's lowering kept for a host's own analysis of it ([`KernelTrace`]): its AIR as lowered,
//! before any optimisation; every call of a helper that gave one f32, with its arguments; and one
//! element run on the reference interpreter with every value read back.
//!
//! A traced lowering keeps the calls and values where they can be read: it inlines every helper
//! (none is a function of its own), lowers a helper whose one `return` is its last statement as
//! that value (no wrapper), binds a `let` never assigned again to its value, and reads a call's
//! value-typed places at the call. A kernel that is not traced lowers as ever.
use super::kernel::{Access, BufferDecl, KernelLowered, K_COUNT, K_PARAMS};
use super::*;
use crate::boundary::forever;
use crate::ir;
use makepad_relax_api::relax_api;
use std::sync::Arc;

/// What a value's definition is, as far as a reader of the AIR needs: a constant of the program,
/// a read of the run's context (the same all through it), a read of memory, f32 arithmetic, or
/// anything else (`args` are its operands).
#[relax_api(plain)]
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TraceDefKind {
    Const,
    Context,
    Memory,
    Neg,
    Add,
    Sub,
    Mul,
    Div,
    Other,
}

/// One value's definition.
#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct TraceDef {
    pub val: u32,
    pub args: [u32; 3],
    pub argc: u8,
    pub kind: TraceDefKind,
    /// An f32 value.
    pub float: bool,
}

#[relax_api(plain)]
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TraceArgKind {
    Literal,
    Float,
    Vector,
    Other,
}

/// A call's argument: a literal, or its lanes' values.
#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct TraceArg<'a> {
    pub literal: f64,
    pub lanes: &'a [u32],
    pub kind: TraceArgKind,
}

/// A call of a helper that gave one f32.
#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct TraceCall<'a> {
    pub name: &'a str,
    pub params: &'a [&'a str],
    pub args: &'a [TraceArg<'a>],
    pub value: u32,
    /// The helper is the prelude's or a module's, not the program's own.
    pub library: bool,
}

/// What a host reads of a trace: the definitions of the values in the blocks open around the
/// element's own code (the program's and the element loop's; a value in a branch or a helper's
/// wrapper is in none), every call in order, the parameters' names, and how many values a run
/// reads back.
#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy)]
pub struct TraceView<'a> {
    pub defs: &'a [TraceDef],
    pub calls: &'a [TraceCall<'a>],
    pub params: &'a [&'a str],
    pub values: usize,
}

/// A call noted while a kernel lowers.
struct Noted {
    name: String,
    library: bool,
    params: Vec<String>,
    args: Vec<V>,
    value: Val,
}

/// A traced lowering's notes.
#[derive(Default)]
pub(super) struct TraceScope {
    calls: Vec<Noted>,
}

impl TraceScope {
    pub(super) fn finish(self, program: &Program) -> TraceParts {
        TraceParts { program: program.clone(), calls: self.calls }
    }
}

/// A traced lowering's result, as [`kernel::lower_kernel`] gives it.
pub struct TraceParts {
    program: Program,
    calls: Vec<Noted>,
}

/// Whether a helper's body returns anywhere (not counting the local functions in it).
pub(super) fn has_return(body: &[Stmt]) -> bool {
    body.iter().any(|s| match s {
        Stmt::Return(..) => true,
        Stmt::Assign { target, value, .. } => returns_in(target) || returns_in(value),
        Stmt::Let { value, .. } | Stmt::Expr(value) => returns_in(value),
        Stmt::For { from, to, body, .. } => returns_in(from) || returns_in(to) || has_return(body),
        Stmt::While { cond, body, .. } => returns_in(cond) || has_return(body),
        Stmt::Loop { body, .. } => has_return(body),
        _ => false,
    })
}

/// Whether an expression holds a `return` (in a block of it).
pub(super) fn returns_in(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::If(arms, else_) => arms.iter().any(|(c, b)| returns_in(c) || has_return(b)) || else_.as_ref().is_some_and(|b| has_return(b)),
        ExprKind::Match(s, arms) => returns_in(s) || arms.iter().any(|(_, b)| has_return(b)),
        ExprKind::Block(b) => has_return(b),
        ExprKind::Bin(_, a, b) | ExprKind::Index(a, b) | ExprKind::ArrayRepeat(a, b) => returns_in(a) || returns_in(b),
        ExprKind::Neg(a) | ExprKind::Not(a) | ExprKind::Field(a, _) => returns_in(a),
        ExprKind::Call(_, args) | ExprKind::ArrayList(args) => args.iter().any(returns_in),
        ExprKind::StructLit(_, f) => f.iter().any(|(_, a)| returns_in(a)),
        _ => false,
    }
}

impl Lowerer {
    /// In a traced lowering: a call's arguments with its value-typed places read, and a copy to
    /// note. None: this lowering is not traced.
    pub(super) fn traced_args(&mut self, vals: &mut [V]) -> Option<Vec<V>> {
        self.trace.as_ref()?;
        for v in vals.iter_mut() {
            if let V::Place(p) = v {
                let p = p.clone();
                *v = self.read(&p);
            }
        }
        Some(vals.to_vec())
    }

    /// Notes a call of `f` that gave `v` (not one a constant's evaluation made: its values are a
    /// throwaway builder's).
    pub(super) fn note_call(&mut self, f: &FnDecl, args: Vec<V>, v: &V) {
        let library = f.span.start >= self.prelude_base;
        if let (false, Some(scope), V::F(value)) = (self.b.scratch, self.trace.as_mut(), v) {
            let params = f.params.iter().map(|(n, _)| n.clone()).collect();
            scope.calls.push(Noted { name: f.name.clone(), library, params, args, value: *value });
        }
    }
}

/// The definitions of the values `b` defines itself.
fn block_defs(b: &[IS], vals: &[Ty], out: &mut Vec<TraceDef>) {
    let float = |v: u32| vals[v as usize] == Ty::F32;
    for s in b {
        match s {
            IS::Def(v, op) => {
                let (kind, args, argc) = match *op {
                    Op::ConstF(_) | Op::ConstD(_) | Op::ConstI(_) | Op::ConstB(_) => (TraceDefKind::Const, [0; 3], 0),
                    Op::FrameCount | Op::Load { region: Region::Ctx, off: None, .. } => (TraceDefKind::Context, [0; 3], 0),
                    Op::Load { .. } | Op::In { .. } | Op::Get(_) | Op::BufLen(_) => (TraceDefKind::Memory, [0; 3], 0),
                    Op::Un(Un::NegF, a) => (TraceDefKind::Neg, [a.0, 0, 0], 1),
                    Op::Un(_, a) | Op::Wrap(a, _) => (TraceDefKind::Other, [a.0, 0, 0], 1),
                    Op::Bin(Bin::AddF, a, b) => (TraceDefKind::Add, [a.0, b.0, 0], 2),
                    Op::Bin(Bin::SubF, a, b) => (TraceDefKind::Sub, [a.0, b.0, 0], 2),
                    Op::Bin(Bin::MulF, a, b) => (TraceDefKind::Mul, [a.0, b.0, 0], 2),
                    Op::Bin(Bin::DivF, a, b) => (TraceDefKind::Div, [a.0, b.0, 0], 2),
                    Op::Bin(_, a, b) | Op::CmpF(_, a, b) | Op::CmpI(_, a, b) | Op::CmpD(_, a, b) => (TraceDefKind::Other, [a.0, b.0, 0], 2),
                    Op::Sel(a, b, c) | Op::Fma(_, a, b, c) => (TraceDefKind::Other, [a.0, b.0, c.0], 3),
                };
                out.push(TraceDef { val: v.0, args, argc, kind, float: float(v.0) });
            }
            IS::Call { rets, .. } | IS::CallHost { rets, .. } => {
                out.extend(rets.iter().map(|r| TraceDef { val: r.0, args: [0; 3], argc: 0, kind: TraceDefKind::Memory, float: float(r.0) }));
            }
            _ => {}
        }
    }
}

/// A call's text and lanes, which its view borrows.
struct OwnedCall {
    _name: String,
    _params: Vec<String>,
    _lanes: Vec<Vec<u32>>,
    _param_views: Vec<&'static str>,
    _arg_views: Vec<TraceArg<'static>>,
}

struct TraceData {
    program: Program,
    shared: Vec<u32>,
    buffers: Vec<BufferDecl>,
    defs: Vec<TraceDef>,
    values: usize,
    _names: Vec<String>,
    _owned: Vec<OwnedCall>,
    params: Vec<&'static str>,
    calls: Vec<TraceCall<'static>>,
}

/// A kernel's lowering read back (see the module docs). A handle: it clones and drops through
/// the compiler's code.
#[relax_api(handle, clone)]
#[derive(Clone)]
pub struct KernelTrace(Arc<TraceData>);

impl KernelTrace {
    pub(crate) fn new(lowered: KernelLowered, shared: Vec<u32>) -> Result<KernelTrace, String> {
        let TraceParts { program, calls } = lowered.trace.ok_or("a traced lowering without its trace")?;
        let regions = ir::Regions {
            ctx: K_PARAMS + lowered.params.len() as u32,
            state: 1,
            shared: shared.len() as u32,
            frame: program.frame_words,
            shared_writable: false,
            bufs: lowered.buffers.iter().map(|b| b.access != Access::Read).collect(),
            io: false,
        };
        ir::validate(&program, &regions)?;
        let mut defs = Vec::new();
        block_defs(&program.body, &program.vals, &mut defs);
        if let Some(IS::Loop { body, .. }) = program.body.last() {
            block_defs(body, &program.vals, &mut defs);
        }
        // SAFETY (all `forever` below): the views borrow the heap data of the strings and vectors
        // kept next to them in the Arc's data, which never changes; moving a String or Vec leaves
        // its heap data in place, and a view is only handed out behind a borrow of the trace.
        let names: Vec<String> = lowered.params.into_iter().map(|p| p.name).collect();
        let params = names.iter().map(|n| unsafe { forever(n.as_str()) }).collect();
        let mut owned = Vec::with_capacity(calls.len());
        let mut views = Vec::with_capacity(calls.len());
        for c in calls {
            let lanes: Vec<Vec<u32>> = c.args.iter().map(|a| match a {
                V::F(x) => vec![x.0],
                V::Vec(n, l) => l[..*n as usize].iter().map(|x| x.0).collect(),
                _ => Vec::new(),
            }).collect();
            let args: Vec<TraceArg<'static>> = c.args.iter().zip(&lanes).map(|(a, lanes)| {
                let (kind, literal) = match a {
                    V::Lit(x) => (TraceArgKind::Literal, *x),
                    V::F(_) => (TraceArgKind::Float, 0.0),
                    V::Vec(..) => (TraceArgKind::Vector, 0.0),
                    _ => (TraceArgKind::Other, 0.0),
                };
                TraceArg { literal, lanes: unsafe { forever(&lanes[..]) }, kind }
            }).collect();
            let call_params: Vec<&'static str> = c.params.iter().map(|n| unsafe { forever(n.as_str()) }).collect();
            views.push(unsafe { TraceCall { name: forever(c.name.as_str()), params: forever(&call_params[..]), args: forever(&args[..]), value: c.value.0, library: c.library } });
            owned.push(OwnedCall { _name: c.name, _params: c.params, _lanes: lanes, _param_views: call_params, _arg_views: args });
        }
        let values = program.vals.len();
        Ok(KernelTrace(Arc::new(TraceData { program, shared, buffers: lowered.buffers, defs, values, _names: names, _owned: owned, params, calls: views })))
    }
}

#[relax_api]
impl KernelTrace {
    pub fn view(&self) -> TraceView<'_> {
        let t = &*self.0;
        TraceView { defs: &t.defs, calls: &t.calls, params: &t.params, values: t.values }
    }

    /// Runs one element on the reference interpreter: parameters `params` (by [`TraceView::params`]),
    /// buffer `buffer` reading `words`, every other buffer reading 0 and taking the element's
    /// writes; `values` takes each value's bits (a value in a branch not taken: stale or 0).
    pub fn run(&self, params: &[f32], buffer: &str, words: &[f32], values: &mut [u32]) {
        let t = &*self.0;
        let mut ctx = vec![0u32; K_PARAMS as usize + t.params.len()];
        ctx[K_COUNT as usize] = 1;
        for (c, p) in ctx[K_PARAMS as usize..].iter_mut().zip(params) {
            *c = p.to_bits();
        }
        let mut spare: Vec<Vec<u32>> = t.buffers.iter().map(|b| vec![0u32; b.stride.max(1) as usize]).collect();
        let bufs: Vec<ir::RawBuf> = t.buffers.iter().zip(&mut spare).map(|(b, own)| match b.name == buffer && !words.is_empty() {
            true => ir::RawBuf { ptr: words.as_ptr() as *mut u32, len: words.len(), writable: false },
            false => ir::RawBuf { ptr: own.as_mut_ptr(), len: own.len(), writable: b.access != Access::Read },
        }).collect();
        let mut state = [0u32; 1];
        let mut mem = ir::Mem { ctx: &mut ctx, state: &mut state, shared: ir::Shared::Read(&t.shared), bufs: &bufs };
        let zeros = [0f32; 1];
        let (mut o0, mut o1) = ([0f32; 1], [0f32; 1]);
        let mut io = ir::Io { ins: [&zeros, &zeros], outs: [&mut o0, &mut o1] };
        let mut scratch = vec![0u32; t.program.scratch_words()];
        ir::run(&t.program, &mut scratch, &mut mem, &mut io, 1);
        let n = values.len().min(t.values);
        values[..n].copy_from_slice(&scratch[..n]);
    }
}
