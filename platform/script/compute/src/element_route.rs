//! Element kernels' GPU route: a compute program's `mod.kernel` object (host
//! buffers in and out, one element a thread) as its own compute kernel for
//! every target, its AIR translated into shader IR ([`crate::air_shader`])
//! inside the compiler.
//!
//! The kernel's entry is `kernel_main`, [`ELEMENT_WORKGROUP`] threads a
//! workgroup, IEEE math (no contraction). Its element is
//! `dispatch_range.x + global_id().x`, below `dispatch_range.y`. Each host
//! buffer is a storage buffer of words named as the kernel names it, and the
//! read-only words [`ELEMENT_ARGS`] hold the CPU's context words of the run
//! (its count, time, seed and parameters, at their places), then each host
//! buffer's length in words (by the kernel's buffer index). A read or write
//! clamps as the CPU's does, and an empty buffer reads 0 and takes no write.
//! A reduce kernel, one that writes other elements' records (one thread on the
//! CPU), one that emits records, and f64 have no route.
use crate::air_shader::{Io, Tr};
use crate::boundary::KernelRef;
use crate::ir as air;
use crate::lower::kernel::{K_BASE, K_COUNT, K_PARAMS, K_SEED, K_TIME};
use makepad_relax_api::relax_api;
use makepad_script::shader_ir::*;
use makepad_script::shader_ir_compute::{CompiledKernel, CompiledKernelResult, ComputeTarget};
use makepad_script::*;
use std::sync::Arc;

/// An element kernel's own storage buffer: the run's context words, then the host buffers'
/// lengths.
pub const ELEMENT_ARGS: &str = "#args";

/// Element kernels' threads a workgroup.
pub const ELEMENT_WORKGROUP: u32 = 64;

/// One element kernel's GPU route, or why it has none. A handle: it clones and drops through
/// the compiler's code.
#[relax_api(handle, clone)]
#[derive(Clone)]
pub struct ElementRoute(Arc<Result<ElementAir, String>>);

#[relax_api]
impl ElementRoute {
    /// The route of element kernel `kernel`, with at most `max_frame_words` words of local
    /// arrays.
    pub fn new(kernel: &KernelRef, max_frame_words: u32) -> ElementRoute {
        ElementRoute(Arc::new(ElementAir::new(kernel, max_frame_words)))
    }

    /// The kernel compiled for `target`, or why it has no route.
    pub fn compile(&self, target: ComputeTarget) -> CompiledKernelResult {
        CompiledKernelResult::new(match &*self.0 {
            Ok(k) => k.compile(target),
            Err(e) => Err(e.clone()),
        })
    }
}

/// An element kernel's AIR made ready for the shader: flattened (no calls), checked against
/// what the route supports, with its buffers.
struct ElementAir {
    program: air::Program,
    /// Per host buffer (by the kernel's index): its storage buffer's name and whether the
    /// kernel writes it; None for the control word.
    buffers: Vec<Option<(LiveId, bool)>>,
    /// The compiler's control word buffer (the cancel token): reads 0.
    control: Option<u8>,
    params: usize,
    /// Worst-case AIR ops per element.
    cost: u64,
}

impl ElementAir {
    fn new(kernel: &KernelRef, max_frame_words: u32) -> Result<ElementAir, String> {
        let program = air::flat(kernel.program()).into_owned();
        if program.frame_words > max_frame_words {
            return Err(format!("{} words of local arrays (at most {max_frame_words})", program.frame_words));
        }
        if !matches!(kernel.kind, crate::lower::kernel::KernelKind::Map) {
            return Err("a reduce kernel (no GPU route yet)".into());
        }
        if !kernel.parallel_safe {
            return Err("it writes other elements' records (one thread on the CPU; no GPU route)".into());
        }
        let control = kernel.buffer_index(crate::lower::kernel::CONTROL_BUFFER).map(|c| c as u8);
        let mut buffers = Vec::new();
        for (i, b) in kernel.buffer_views().enumerate() {
            buffers.push(match b.access {
                _ if Some(i as u8) == control => None,
                crate::kernel::Access::Read => Some((LiveId::from_str(b.name), false)),
                crate::kernel::Access::Write => Some((LiveId::from_str(b.name), true)),
                _ => return Err(format!("`{}` emits records (no GPU route)", b.name)),
            });
        }
        let k = ElementAir { program, buffers, control, params: kernel.param_count(), cost: kernel.cost };
        // A dry run of the translation: what the shader would refuse, refused now.
        k.main(&mut IrModule::new())?;
        Ok(k)
    }

    fn compile(&self, target: ComputeTarget) -> Result<CompiledKernel, String> {
        let mut ir = IrModule::new();
        let main = self.main(&mut ir)?;
        ir.functions.push(main);
        let main = (ir.functions.len() - 1) as FuncId;
        ir.workgroup = [ELEMENT_WORKGROUP, 1, 1];
        // IEEE math, no contraction: the CPU's bits
        ir.precise = true;
        makepad_script::shader_ir_compute::compile_compute_ir(&mut ir, main, target)
    }

    /// The kernel's body (no params, void): its element, when the dispatch has it, run as the
    /// CPU runs one.
    fn main(&self, ir: &mut IrModule) -> Result<IrFunction, String> {
        let words = ir.ty(IrTy::Array(TY_U32, 0));
        let v3u = ir.vec_ty(3, IrScalar::U32);
        let v4u = ir.vec_ty(4, IrScalar::U32);
        let range = ir.global(IrGlobalKind::Io(IrIo::Uniform), id!(dispatch_range), v4u);
        let args = ir.global(IrGlobalKind::Io(IrIo::Storage(false)), LiveId::from_str(ELEMENT_ARGS), words);
        let bufs = self.buffers.iter().map(|b| b.map(|(name, writes)| ir.global(IrGlobalKind::Io(IrIo::Storage(writes)), name, words))).collect();
        let frame_ty = (self.program.frame_words > 0).then(|| ir.ty(IrTy::Array(TY_I32, self.program.frame_words)));
        let mut f = IrFunction::new("element_main".into(), TY_VOID);
        f.cost = self.cost;
        let index = f.local(id!(i), 0, TY_U32, true, LocalKind::Temp);
        let mut t = Tr::new(&self.program, self.control, f);
        let mut body = Vec::new();
        let gid = t.e(ExprKind::Builtin(Builtin::GlobalId, Vec::new()), v3u);
        let gx = t.e(ExprKind::Swizzle(gid, [0, 0, 0, 0], 1), TY_U32);
        let r = t.e(ExprKind::Global(range), v4u);
        let first = t.e(ExprKind::Swizzle(r, [0, 0, 0, 0], 1), TY_U32);
        let count = t.e(ExprKind::Swizzle(r, [1, 0, 0, 0], 1), TY_U32);
        let i = t.bin(BinOp::Add, first, gx, TY_U32);
        body.push(Stmt::Local(index, i));
        t.locals(&mut body, frame_ty)?;
        t.jumps(&mut body)?;
        let inside = t.bin(BinOp::Lt, gx, count, TY_BOOL);
        let io = ElementIo { k: self, args, words, bufs, index };
        let main = t.block(&io, &self.program.body, 0)?;
        body.push(Stmt::If(inside, main, Vec::new()));
        t.f.body = body;
        Ok(t.f)
    }
}

/// An element kernel's storage in its shader: `#args`, the words type, each host buffer's
/// storage buffer, and the local holding the thread's element.
struct ElementIo<'a> {
    k: &'a ElementAir,
    args: GlobalId,
    words: TyId,
    bufs: Vec<Option<GlobalId>>,
    index: LocalId,
}

impl ElementIo<'_> {
    /// Word `at` of `#args` (u32).
    fn arg(&self, t: &mut Tr, at: u32) -> ExprId {
        let a = t.e(ExprKind::Global(self.args), self.words);
        let i = t.lit_u(at);
        t.e(ExprKind::Index(a, i), TY_U32)
    }

    /// Host buffer `b`'s length in words (u32): `#args` past the context words.
    fn buf_len(&self, t: &mut Tr, b: u8) -> ExprId {
        self.arg(t, K_PARAMS + self.k.params as u32 + b as u32)
    }

    /// Host buffer `b`'s storage word `min(base + off, len - 1)` (as buf_index clamps: the
    /// offset as u32, no wrap; word 0 when it is empty) and whether it has words; None: `b`
    /// has no storage.
    fn buf_at(&self, t: &mut Tr, b: u8, base: u32, off: Option<air::Val>) -> Option<(ExprId, ExprId)> {
        let g = self.bufs.get(b as usize).copied().flatten()?;
        let len = self.buf_len(t, b);
        let (z, one) = (t.lit_u(0), t.lit_u(1));
        let any = t.bin(BinOp::Ne, len, z, TY_BOOL);
        let lm1 = t.bin(BinOp::Sub, len, one, TY_U32);
        let z2 = t.lit_u(0);
        let last = t.sel(any, lm1, z2, TY_U32);
        let bl = t.lit_u(base);
        let over = t.bin(BinOp::Gt, bl, last, TY_BOOL);
        let bb = t.sel(over, last, bl, TY_U32);
        let w = match off {
            None => bb,
            Some(o) => {
                let o = t.val(o);
                let ou = t.as_u(o);
                let room = t.bin(BinOp::Sub, last, bb, TY_U32);
                let big = t.bin(BinOp::Gt, ou, room, TY_BOOL);
                let c = t.sel(big, room, ou, TY_U32);
                t.bin(BinOp::Add, bb, c, TY_U32)
            }
        };
        let buf = t.e(ExprKind::Global(g), self.words);
        Some((t.e(ExprKind::Index(buf, w), TY_U32), any))
    }
}

impl Io for ElementIo<'_> {
    fn ctx(&self, t: &mut Tr, base: u32, ty: TyId) -> Result<ExprId, String> {
        let w = if base == K_BASE {
            let i = t.local(self.index, TY_U32);
            t.as_i(i)
        } else if matches!(base, K_COUNT | K_TIME | K_SEED) || (base >= K_PARAMS && ((base - K_PARAMS) as usize) < self.k.params) {
            let a = self.arg(t, base);
            t.as_i(a)
        } else {
            t.lit_i(0)
        };
        Ok(t.word_as(w, false, ty))
    }

    fn load(&self, t: &mut Tr, b: u8, base: u32, off: Option<air::Val>, ty: TyId) -> Result<Option<ExprId>, String> {
        let Some((w, any)) = self.buf_at(t, b, base, off) else { return Ok(None) };
        let z = t.lit_u(0);
        let w = t.sel(any, w, z, TY_U32);
        let w = t.as_i(w);
        Ok(Some(t.word_as(w, false, ty)))
    }

    fn store(&self, t: &mut Tr, b: u8, base: u32, off: Option<air::Val>, val: air::Val) -> Result<Option<Stmt>, String> {
        if !self.k.buffers.get(b as usize).copied().flatten().is_some_and(|(_, writes)| writes) {
            return Ok(None);
        }
        let Some((place, any)) = self.buf_at(t, b, base, off) else { return Ok(None) };
        let w = t.to_word(val);
        let w = t.as_u(w);
        Ok(Some(Stmt::If(any, vec![Stmt::Assign(place, w)], Vec::new())))
    }

    fn len(&self, t: &mut Tr, b: u8) -> Result<ExprId, String> {
        if self.bufs.get(b as usize).copied().flatten().is_some() {
            let len = self.buf_len(t, b);
            return Ok(t.as_i(len));
        }
        Ok(t.lit_i(1))
    }

    fn fma(&self) -> bool {
        true
    }
}
