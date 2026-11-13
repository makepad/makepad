//! A point kernel's GPU route: its AIR (flattened, checked against what a
//! shader can run) translated into shader IR ([`crate::air_shader`]), inside
//! the compiler.
//!
//! The AIR, the shader IR module and the function being built are all the
//! compiler's own (rustc-built) values, so the translation runs here: a host
//! makes a [`ShaderRoute`] per kernel and installs a
//! dispatch over them as a shader fn's body with [`set_dispatch`]; code
//! Relax compiles holds only the handles.
//!
//! A point kernel takes points and gives one value per point; the host names
//! its buffers ([`RouteNames`]). It may read a grid: an f32 input whose words
//! the host keeps in its `program` storage buffer (vec4 texels) from the texel
//! a parameter names. Its word 13 is the grid's word count; a read is clamped
//! to it as the CPU clamps a buffer read to the buffer.
//!
//! SHADER LAW: unary minus has the lowest precedence in the script shader
//! parser; write `(0.0 - a) - b`, never `-a - b`.
use crate::air_shader::{Io, Tr};
use crate::boundary::KernelRef;
use crate::ir::{self as air, Op, Region, Ty};
use relax_api::relax_api;
use makepad_script::shader_compile::ShaderIrFn;
use makepad_script::shader_ir::*;
use makepad_script::*;
use std::rc::Rc;
use std::sync::Arc;

/// Worst-case ops a grid read counts on the GPU route (a dependent load from
/// the program buffer), and the header read once per call that bounds them.
pub const GRID_LOAD_OPS: u64 = 8;
pub const GRID_CALL_OPS: u64 = 4;
/// The grid's word count: word 13 of its header (texel 3, lane y).
const GRID_LEN_WORD: u32 = 13;

/// One kernel's GPU route, or why it has none. A handle: it clones and drops
/// through the compiler's code.
#[relax_api(handle, clone)]
#[derive(Clone)]
pub struct ShaderRoute(Arc<Result<KernelAir, String>>);

/// What a host's point kernels call their parts.
#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy)]
pub struct RouteNames<'a> {
    /// The input holding the points (vec3).
    pub points: &'a str,
    /// The output holding one value per point (f32).
    pub value: &'a str,
    /// The input holding the grid, if the kernel reads one (f32).
    pub grid: &'a str,
    /// The parameter holding the grid's first texel.
    pub grid_at: &'a str,
    /// The start of the kernel's shader fn's name; its key follows.
    pub fn_prefix: &'a str,
}

#[relax_api]
impl ShaderRoute {
    /// The route of `kernel`, a point kernel with parts called `names`, under the
    /// host's limits: at most `max_cost` AIR ops per sample, `max_params` parameters
    /// and `max_frame_words` words of local arrays. `key` names its shader fn.
    pub fn new(kernel: &KernelRef, key: u64, names: RouteNames<'_>, max_cost: u64, max_params: u32, max_frame_words: u32) -> ShaderRoute {
        ShaderRoute(Arc::new(KernelAir::new(kernel, key, names, max_cost, max_params as usize, max_frame_words)))
    }

    /// What the host reads of the route.
    pub fn info(&self) -> RouteInfo<'_> {
        match &*self.0 {
            Ok(k) => RouteInfo { cost: k.cost, params: k.params as u32, error: "" },
            Err(e) => RouteInfo { cost: 0, params: 0, error: e },
        }
    }
}

/// A route's cost (worst-case AIR ops per sample), the parameters a call
/// passes, and why the kernel has no route (empty: it has one).
#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy)]
pub struct RouteInfo<'a> {
    pub cost: u64,
    pub params: u32,
    pub error: &'a str,
}

/// A copy of shader fn `stub` (`kernel_call(slot, p, a, b, c, d) -> f32`: its
/// name and parameters) whose body is the dispatch over `routes` (by slot;
/// `None`: that slot has no GPU route): slot k calls kernel k's function.
/// `key` is the body's identity. None: `stub` is no script fn.
#[relax_api]
pub fn dispatch_fn(vm: &mut ScriptVm, stub: ScriptObject, key: u64, routes: &[Option<&ShaderRoute>]) -> Option<ScriptObject> {
    let ptr = vm.heap().as_fn(stub)?;
    let proto = vm.heap().proto(stub);
    let own = vm.new_with_proto_of(proto);
    vm.heap_mut().vec_push_vec(own, stub, makepad_script::trap::NoTrap);
    vm.heap_mut().set_fn(own, ptr);
    let kernels = routes.iter().map(|r| r.map(|r| r.clone())).collect();
    vm.set_shader_ir_fn(own, Some(Rc::new(Dispatch { key, kernels }) as Rc<dyn ShaderIrFn>));
    Some(own)
}

/// Takes the dispatch body [`dispatch_fn`] gave `f` away again (the variant is released).
#[relax_api]
pub fn release_dispatch(vm: &mut ScriptVm, f: ScriptObject) {
    vm.set_shader_ir_fn(f, None);
}

/// A shader's kernel dispatch.
struct Dispatch {
    key: u64,
    kernels: Vec<Option<ShaderRoute>>,
}

impl ShaderIrFn for Dispatch {
    fn key(&self) -> u64 {
        self.key
    }

    fn build(&self, ir: &mut IrModule, f: &mut IrFunction) -> Result<(), String> {
        if f.params.len() != 6 {
            return Err(format!("kernel_call takes (slot, p, a, b, c, d), not {} params", f.params.len()));
        }
        let mut body = Vec::new();
        for (i, k) in self.kernels.iter().enumerate() {
            let Some(Ok(k)) = k.as_ref().map(|k| k.0.as_ref()) else { continue };
            let kf = k.function(ir, format!("{}{:016x}", k.fn_prefix, k.key))?;
            ir.functions.push(kf);
            let id = (ir.functions.len() - 1) as FuncId;
            let slot = f.expr(ExprKind::Param(0), TY_F32);
            let lim = f.expr(ExprKind::Lit(IrLit::F32(i as f32 + 0.5)), TY_F32);
            let here = f.expr(ExprKind::Binary(BinOp::Lt, slot, lim), TY_BOOL);
            let args: Vec<ExprId> = (1..6).map(|p| {
                let ty = f.params[p].ty;
                f.expr(ExprKind::Param(p as u32), ty)
            }).collect();
            let call = f.expr(ExprKind::Call(id, args), TY_F32);
            body.push(Stmt::If(here, vec![Stmt::Return(call)], Vec::new()));
            f.cost = f.cost.max(k.cost);
        }
        let far = f.expr(ExprKind::Lit(IrLit::F32(1.0e9)), TY_F32);
        body.push(Stmt::Return(far));
        f.body = body;
        Ok(())
    }
}

impl KernelAir {
    fn new(kernel: &KernelRef, key: u64, names: RouteNames<'_>, max_cost: u64, max_params: usize, max_frame_words: u32) -> Result<KernelAir, String> {
        let program = air::flat(kernel.program()).into_owned();
        let grid = match (kernel.buffer_index(names.grid), kernel.param_index(names.grid_at)) {
            (Some(b), Some(at)) => Some((b as u8, at)),
            (None, _) => None,
            (Some(_), None) => return Err(format!("a grid without its parameter `{}`", names.grid_at)),
        };
        // a grid read costs its load on the GPU, and the bound's header read once a call
        let cost = match grid {
            Some((b, _)) => kernel.cost.saturating_add(grid_loads(&program.body, b, 0).saturating_mul(GRID_LOAD_OPS - 1)).saturating_add(GRID_CALL_OPS),
            None => kernel.cost,
        };
        if cost > max_cost {
            return Err(format!("{cost} ops per sample (at most {max_cost} run on the GPU)"));
        }
        let params = kernel.param_count();
        if params > max_params {
            return Err(format!("{params} parameters (at most {max_params})"));
        }
        let (Some(points), Some(value)) = (kernel.buffer_index(names.points), kernel.buffer_index(names.value)) else {
            return Err(format!("not a point kernel ({} in, {} out)", names.points, names.value));
        };
        if program.frame_words > max_frame_words {
            return Err(format!("{} words of local arrays (at most {max_frame_words})", program.frame_words));
        }
        let control = kernel.buffer_index(crate::lower::kernel::CONTROL_BUFFER).map(|c| c as u8);
        let k = KernelAir { key, fn_prefix: names.fn_prefix.to_string(), program, points: points as u8, value: value as u8, control, grid, params, cost };
        // A dry run of the translation: what the shader would refuse, refused now.
        let mut ir = IrModule::new();
        k.function(&mut ir, "check".into())?;
        Ok(k)
    }
}

/// A kernel's AIR made ready for the shader: flattened (no calls), checked
/// against what the GPU route supports, with its buffers and parameters.
struct KernelAir {
    /// The program's identity (the host's key: its fn is this after `fn_prefix`).
    key: u64,
    fn_prefix: String,
    program: air::Program,
    points: u8,
    value: u8,
    /// The compiler's control word buffer (the cancel token): reads 0.
    control: Option<u8>,
    /// The grid input's buffer and the parameter holding its first texel.
    grid: Option<(u8, usize)>,
    params: usize,
    /// Worst-case AIR ops per sample.
    cost: u64,
}

impl KernelAir {
    /// The shader function `name(p: vec3, a: vec4, b: vec4, c: vec4, d: vec4) -> f32`:
    /// the kernel's value at `p`, parameter k in lane k % 4 of a..d.
    fn function(&self, ir: &mut IrModule, name: String) -> Result<IrFunction, String> {
        let v3 = ir.vec_ty(3, IrScalar::F32);
        let v4 = ir.vec_ty(4, IrScalar::F32);
        let mut f = IrFunction::new(name, TY_F32);
        f.params.push(IrParam { name: id!(p), shadow: 0, ty: v3, inout: false });
        for n in [id!(a), id!(b), id!(c), id!(d)] {
            f.params.push(IrParam { name: n, shadow: 0, ty: v4, inout: false });
        }
        f.cost = self.cost;
        let frame_ty = (self.program.frame_words > 0).then(|| ir.ty(IrTy::Array(TY_I32, self.program.frame_words)));
        let program = match self.grid {
            Some(_) => {
                let texels = ir.ty(IrTy::Array(v4, 0));
                Some(ir.global(IrGlobalKind::Io(IrIo::Storage(false)), id!(program), texels))
            }
            None => None,
        };
        let v4u = ir.vec_ty(4, IrScalar::U32);
        let mut t = Tr::new(&self.program, self.control, f);
        let mut body = Vec::new();
        let mut grid = None;
        if let (Some(g), Some((_, at))) = (program, self.grid) {
            // the grid's first texel and its word count, once a call
            let a = param(&mut t, at);
            let first = t.e(ExprKind::Convert(a), TY_U32);
            let fl = t.f.local(id!(k), 0, TY_U32, true, LocalKind::Temp);
            body.push(Stmt::Local(fl, first));
            let fe = t.local(fl, TY_U32);
            let off = t.lit_u(GRID_LEN_WORD / 4);
            let i = t.bin(BinOp::Add, fe, off, TY_U32);
            let buf = t.e(ExprKind::Global(g), ir.ty(IrTy::Array(v4, 0)));
            let tx = t.e(ExprKind::Index(buf, i), v4);
            let w = t.e(ExprKind::Swizzle(tx, [(GRID_LEN_WORD % 4) as u8, 0, 0, 0], 1), TY_F32);
            let len = t.e(ExprKind::Convert(w), TY_U32);
            let one = t.lit_u(1);
            let last = t.bin(BinOp::Sub, len, one, TY_U32);
            let ll = t.f.local(id!(k), 0, TY_U32, true, LocalKind::Temp);
            body.push(Stmt::Local(ll, last));
            grid = Some((g, ir.ty(IrTy::Array(v4, 0)), fl, ll));
        }
        t.locals(&mut body, frame_ty)?;
        let (result, s) = t.declare(Ty::F32)?;
        body.push(s);
        t.jumps(&mut body)?;
        let io = PointIo { k: self, grid, result, v4, v4u };
        let main = t.block(&io, &self.program.body, 0)?;
        body.extend(main);
        let r = t.local(result, TY_F32);
        // + 0.0: a zero leaves as +0 (the volume's convention)
        let z = t.lit_f(0.0);
        let r = t.f.expr(ExprKind::Binary(BinOp::Add, r, z), TY_F32);
        body.push(Stmt::Return(r));
        t.f.body = body;
        Ok(t.f)
    }
}

/// A point kernel's memory in its shader fn: its point the fn's `p`, its value the result
/// local, its parameters lanes of `a..d`, and a gridded kernel's grid: the program buffer (and
/// its type), and locals holding the grid's first texel and its last word.
struct PointIo<'a> {
    k: &'a KernelAir,
    grid: Option<(GlobalId, TyId, LocalId, LocalId)>,
    result: LocalId,
    v4: TyId,
    v4u: TyId,
}

/// The grid reads of a block, each loop at its cap (the outermost, the element loop, once):
/// how [`air::Program::cost`] counts them.
fn grid_loads(b: &air::Block, grid: u8, depth: u32) -> u64 {
    let mut n = 0u64;
    for s in b {
        n = n.saturating_add(match s {
            air::Stmt::Def(_, Op::Load { region: Region::Buf(g), .. }) if *g == grid => 1,
            air::Stmt::If(_, t, e) => grid_loads(t, grid, depth).max(grid_loads(e, grid, depth)),
            air::Stmt::Loop { cap, body } => (if depth == 0 { 1 } else { *cap as u64 }).saturating_mul(grid_loads(body, grid, depth + 1)),
            _ => 0,
        });
    }
    n
}

/// Parameter `k`: lane k % 4 of the fn's a..d.
fn param(t: &mut Tr, k: usize) -> ExprId {
    let v4 = t.f.params[1 + k / 4].ty;
    let v = t.e(ExprKind::Param(1 + (k / 4) as u32), v4);
    t.e(ExprKind::Swizzle(v, [(k % 4) as u8, 0, 0, 0], 1), TY_F32)
}

/// Lane `c` of the fn's point.
fn point(t: &mut Tr, c: u8) -> ExprId {
    let v3 = t.f.params[0].ty;
    let p = t.e(ExprKind::Param(0), v3);
    t.e(ExprKind::Swizzle(p, [c, 0, 0, 0], 1), TY_F32)
}

impl PointIo<'_> {
    /// Word `min(base + off, last)` of the grid (as the CPU's buf_index clamps a read: off as
    /// u32, no wrap), as i32 bits: its texel's lanes as u32 bits, then the lane.
    fn grid_word(&self, t: &mut Tr, base: u32, off: Option<air::Val>) -> Result<ExprId, String> {
        let Some((g, gty, first, last)) = self.grid else { return Err("a grid read without the program buffer".into()) };
        let lm1 = t.local(last, TY_U32);
        let b = t.lit_u(base);
        let over = t.bin(BinOp::Gt, b, lm1, TY_BOOL);
        let lm1b = t.local(last, TY_U32);
        let b2 = t.lit_u(base);
        let bb = t.sel(over, lm1b, b2, TY_U32);
        let w = match off {
            None => bb,
            Some(o) => {
                let o = t.val(o);
                let ou = t.as_u(o);
                let lm1c = t.local(last, TY_U32);
                let room = t.bin(BinOp::Sub, lm1c, bb, TY_U32);
                let big = t.bin(BinOp::Gt, ou, room, TY_BOOL);
                let room2 = t.bin(BinOp::Sub, lm1c, bb, TY_U32);
                let c = t.sel(big, room2, ou, TY_U32);
                t.bin(BinOp::Add, bb, c, TY_U32)
            }
        };
        let two = t.lit_u(2);
        let q = t.bin(BinOp::Shr, w, two, TY_U32);
        let fe = t.local(first, TY_U32);
        let i = t.bin(BinOp::Add, fe, q, TY_U32);
        let buf = t.e(ExprKind::Global(g), gty);
        let tx = t.e(ExprKind::Index(buf, i), self.v4);
        let lanes: Vec<ExprId> = (0..4u8)
            .map(|c| {
                let x = t.e(ExprKind::Swizzle(tx, [c, 0, 0, 0], 1), TY_F32);
                t.as_u(x)
            })
            .collect();
        let bits = t.e(ExprKind::Construct(lanes), self.v4u);
        let three = t.lit_u(3);
        let lane = t.bin(BinOp::BitAnd, w, three, TY_U32);
        let word = t.e(ExprKind::Index(bits, lane), TY_U32);
        Ok(t.as_i(word))
    }
}

impl Io for PointIo<'_> {
    fn ctx(&self, t: &mut Tr, base: u32, ty: TyId) -> Result<ExprId, String> {
        let p0 = crate::lower::kernel::K_PARAMS;
        Ok(if base >= p0 && ((base - p0) as usize) < self.k.params {
            let p = param(t, (base - p0) as usize);
            t.word_as(p, true, ty)
        } else if base == crate::lower::kernel::K_COUNT {
            let one = t.lit_i(1);
            t.word_as(one, false, ty)
        } else {
            let z = t.lit_i(0);
            t.word_as(z, false, ty)
        })
    }

    fn load(&self, t: &mut Tr, b: u8, base: u32, off: Option<air::Val>, ty: TyId) -> Result<Option<ExprId>, String> {
        if b == self.k.points {
            // the one element's point: words 0..2, clamped as buf_index does
            let w = match off {
                None => {
                    let c = base.min(2) as u8;
                    point(t, c)
                }
                Some(o) => {
                    let o = t.val(o);
                    let ou = t.as_u(o);
                    let b = t.lit_u(base);
                    let i = t.bin(BinOp::Add, ou, b, TY_U32);
                    let (z, one) = (t.lit_u(0), t.lit_u(1));
                    let is0 = t.bin(BinOp::Eq, i, z, TY_BOOL);
                    let is1 = t.bin(BinOp::Eq, i, one, TY_BOOL);
                    let (px, py, pz) = (point(t, 0), point(t, 1), point(t, 2));
                    let yz = t.sel(is1, py, pz, TY_F32);
                    t.sel(is0, px, yz, TY_F32)
                }
            };
            return Ok(Some(t.word_as(w, true, ty)));
        }
        if Some(b) == self.k.grid.map(|g| g.0) {
            let w = self.grid_word(t, base, off)?;
            return Ok(Some(t.word_as(w, false, ty)));
        }
        Ok(None)
    }

    fn store(&self, t: &mut Tr, b: u8, _base: u32, _off: Option<air::Val>, val: air::Val) -> Result<Option<Stmt>, String> {
        if b != self.k.value {
            return Ok(None);
        }
        let e = t.val(val);
        let e = match t.val_ty(val) {
            Ty::F32 => e,
            Ty::I32 => t.e(ExprKind::Bitcast(e), TY_F32),
            _ => return Err("a bool value".into()),
        };
        let l = t.local(self.result, TY_F32);
        Ok(Some(Stmt::Assign(l, e)))
    }

    fn len(&self, t: &mut Tr, b: u8) -> Result<ExprId, String> {
        if Some(b) == self.k.grid.map(|g| g.0) {
            let Some((_, _, _, last)) = self.grid else { return Err("a grid length without the program buffer".into()) };
            let l = t.local(last, TY_U32);
            let one = t.lit_u(1);
            let n = t.bin(BinOp::Add, l, one, TY_U32);
            return Ok(t.as_i(n));
        }
        Ok(t.lit_i(if b == self.k.points { 3 } else { 1 }))
    }

    fn fma(&self) -> bool {
        false
    }
}
