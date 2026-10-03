//! SPIR-V from the shader IR: one SPIR-V 1.3 module per entry point, with
//! only the functions and globals it reaches, for Vulkan.
//!
//! Lowering rules taken from naga's SPIR-V backend (MIT/Apache-2.0, The
//! gfx-rs developers; see platform/script/CREDITS.md), each marked where it
//! is used: integer division and remainder that never divide by zero or
//! overflow (back/spv/writer.rs `write_wrapped_binary_op`); float to integer
//! conversion clamped to the target's range (back/spv/block.rs `write_as`);
//! integer `clamp` as max-then-min, `saturate` as FClamp, `mix` with a
//! splatted scalar selector, `abs` of unsigned as a copy (block.rs, math
//! functions); `Flat` on every integer fragment input, builtins included,
//! and never on vertex inputs (writer.rs `write_varying`); uniform and
//! storage globals wrapped in a `Block` struct, matrices `ColMajor` with a
//! `MatrixStride`, read-only storage `NonWritable` (writer.rs
//! `write_global_variable`, `decorate_struct_member`); private globals
//! zero-initialized.

use crate::shader_ir::*;
use crate::shader_output::TextureType;
use crate::spirv::*;
use makepad_live_id::{id, LiveId};

/// The SPIR-V of entry function `entry` (an `IrFunction` with `entry` set).
/// `xr`: the view-dependent pass matrices select by the view index.
pub fn ir_to_spirv(m: &IrModule, entry: FuncId, xr: bool) -> Result<Vec<u32>, String> {
    let stage = match &m.functions[entry as usize].entry {
        Some(e) => e.stage,
        None => return Err("not an entry point".to_string()),
    };
    let mut g = Gen {
        m,
        b: SpvBuilder::new(),
        ty_ids: vec![0; m.types.len()],
        global_ids: vec![0; m.globals.len()],
        fn_ids: vec![0; m.functions.len()],
        queue: Vec::new(),
        interface: Vec::new(),
        stage,
        xr,
        uses_view_index: false,
        f: FnCx::default(),
    };
    let entry_id = g.b.id();
    g.fn_ids[entry as usize] = entry_id;
    g.gen_function(entry)?;
    while let Some(fi) = g.queue.pop() {
        g.gen_function(fi)?;
    }
    let name = m.functions[entry as usize].name.clone();
    let interface = std::mem::take(&mut g.interface);
    match stage {
        IrStage::Vertex => g.b.entry_point(EXEC_MODEL_VERTEX, entry_id, &name, &interface),
        IrStage::Fragment => {
            g.b.entry_point(EXEC_MODEL_FRAGMENT, entry_id, &name, &interface);
            g.b.execution_mode(entry_id, EXEC_MODE_ORIGIN_UPPER_LEFT);
        }
    }
    Ok(g.b.finish())
}

#[derive(Default)]
struct FnCx {
    func: u32,
    body: Vec<u32>,
    vars: Vec<u32>,
    locals: Vec<u32>,
    params: Vec<u32>,
    open: bool,
    loops: Vec<(u32, u32)>,
    entry_in: Vec<u32>,
    entry_out: Vec<u32>,
    is_entry: bool,
}

/// A pointer and what it points at.
#[derive(Clone, Copy)]
struct Ptr {
    id: u32,
    ty: TyId,
    sc: u32,
}

struct Gen<'a> {
    m: &'a IrModule,
    b: SpvBuilder,
    ty_ids: Vec<u32>,
    global_ids: Vec<u32>,
    fn_ids: Vec<u32>,
    queue: Vec<FuncId>,
    interface: Vec<u32>,
    stage: IrStage,
    xr: bool,
    uses_view_index: bool,
    f: FnCx,
}

fn err<T>(msg: &str) -> Result<T, String> {
    Err(msg.to_string())
}

impl<'a> Gen<'a> {
    // ---- types ----

    fn ty(&mut self, t: TyId) -> u32 {
        if self.ty_ids[t as usize] != 0 {
            return self.ty_ids[t as usize];
        }
        let id = match self.m.get(t).clone() {
            IrTy::Void => self.b.type_void(),
            IrTy::Scalar(s) | IrTy::Vec(1, s) => self.scalar(s),
            IrTy::AbstractInt => self.b.type_i32(),
            IrTy::AbstractFloat => self.b.type_f32(),
            IrTy::Vec(n, s) => {
                let c = self.scalar(s);
                self.b.type_vector(c, n as u32)
            }
            IrTy::Mat(c, r) => {
                let f = self.b.type_f32();
                let col = self.b.type_vector(f, r as u32);
                self.b.type_matrix(col, c as u32)
            }
            IrTy::Array(e, n) => {
                let eid = self.ty(e);
                let stride = ir_array_stride(self.m, e);
                if n == 0 {
                    self.b.type_runtime_array(eid, stride)
                } else {
                    self.b.type_array(eid, n, stride)
                }
            }
            IrTy::Struct(si) => {
                let fields = self.m.structs[si as usize].fields.clone();
                let mut member_ids = Vec::new();
                for f in &fields {
                    member_ids.push(self.ty(f.ty));
                }
                let id = self.b.id();
                let mut words = vec![id];
                words.extend_from_slice(&member_ids);
                spv_inst(&mut self.b.globals, OP_TYPE_STRUCT, &words);
                let offsets = ir_struct_offsets(self.m, si);
                for (k, f) in fields.iter().enumerate() {
                    self.member_layout(id, k as u32, f.ty, offsets[k]);
                }
                id
            }
            IrTy::Texture(tt) => {
                let f = self.b.type_f32();
                let (dim, depth, arrayed) = image_shape(tt);
                if dim == DIM_CUBE && arrayed {
                    self.b.capability(CAP_SAMPLED_CUBE_ARRAY);
                }
                self.b.type_image(f, dim, depth, arrayed)
            }
            IrTy::Sampler(_) => self.b.type_sampler(),
        };
        self.ty_ids[t as usize] = id;
        id
    }

    fn scalar(&mut self, s: IrScalar) -> u32 {
        match s {
            IrScalar::Bool => self.b.type_bool(),
            IrScalar::F32 => self.b.type_f32(),
            IrScalar::F16 => self.b.type_f16(),
            IrScalar::I32 => self.b.type_i32(),
            IrScalar::U32 => self.b.type_u32(),
        }
    }

    /// naga back/spv/writer.rs `decorate_struct_member`: Offset, and
    /// ColMajor + MatrixStride on matrices (also inside arrays).
    fn member_layout(&mut self, id: u32, index: u32, ty: TyId, offset: u32) {
        self.b.member_decorate(id, index, DECO_OFFSET, &[offset]);
        let mut inner = ty;
        while let IrTy::Array(e, _) = self.m.get(inner) {
            inner = *e;
        }
        if let IrTy::Mat(_, r) = self.m.get(inner) {
            let stride = if *r == 2 { 8 } else { 16 };
            self.b.member_decorate(id, index, DECO_COL_MAJOR, &[]);
            self.b.member_decorate(id, index, DECO_MATRIX_STRIDE, &[stride]);
        }
    }

    fn ptr_ty(&mut self, sc: u32, t: TyId) -> u32 {
        let base = self.ty(t);
        self.b.type_pointer(sc, base)
    }

    fn lanes(&self, t: TyId) -> u32 {
        self.m.lanes(t)
    }

    fn scalar_of(&self, t: TyId) -> IrScalar {
        self.m.scalar_of(t).unwrap_or(IrScalar::F32)
    }

    fn vec_id(&mut self, s: IrScalar, n: u32) -> u32 {
        let c = self.scalar(s);
        if n == 1 {
            c
        } else {
            self.b.type_vector(c, n)
        }
    }

    // ---- instructions ----

    fn emit(&mut self, op: u32, operands: &[u32]) {
        spv_inst(&mut self.f.body, op, operands);
    }

    /// `op type result args...` with a raw type id.
    fn ins_id(&mut self, op: u32, ty_id: u32, args: &[u32]) -> u32 {
        let id = self.b.id();
        let mut words = Vec::with_capacity(args.len() + 2);
        words.push(ty_id);
        words.push(id);
        words.extend_from_slice(args);
        spv_inst(&mut self.f.body, op, &words);
        id
    }

    fn ins(&mut self, op: u32, ty: TyId, args: &[u32]) -> u32 {
        let t = self.ty(ty);
        self.ins_id(op, t, args)
    }

    fn ext_id(&mut self, gl: u32, ty_id: u32, args: &[u32]) -> u32 {
        let mut words = Vec::with_capacity(args.len() + 2);
        words.push(self.b.glsl_ext);
        words.push(gl);
        words.extend_from_slice(args);
        self.ins_id(OP_EXT_INST, ty_id, &words)
    }

    fn label(&mut self, id: u32) {
        self.emit(OP_LABEL, &[id]);
        self.f.open = true;
    }

    fn branch(&mut self, target: u32) {
        self.emit(OP_BRANCH, &[target]);
        self.f.open = false;
    }

    fn local_var(&mut self, ty: TyId) -> u32 {
        let p = self.ptr_ty(SC_FUNCTION, ty);
        let id = self.b.id();
        spv_inst(&mut self.f.vars, OP_VARIABLE, &[p, id, SC_FUNCTION]);
        id
    }

    fn load(&mut self, p: Ptr) -> u32 {
        self.ins(OP_LOAD, p.ty, &[p.id])
    }

    // ---- constants ----

    fn const_scalar(&mut self, s: IrScalar, v: f64) -> u32 {
        match s {
            IrScalar::F32 => self.b.const_f32(v as f32),
            IrScalar::F16 => {
                let t = self.b.type_f16();
                self.b.intern_const(OP_CONSTANT, t, &[f32_to_f16_bits(v as f32)])
            }
            IrScalar::I32 => self.b.const_i32(v as i32),
            IrScalar::U32 => self.b.const_u32(v as u32),
            IrScalar::Bool => self.b.const_bool(v != 0.0),
        }
    }

    /// A constant of a scalar or vector type with every lane `v`.
    fn splat_const(&mut self, s: IrScalar, n: u32, v: f64) -> u32 {
        let c = self.const_scalar(s, v);
        if n <= 1 {
            return c;
        }
        let t = self.vec_id(s, n);
        let parts = vec![c; n as usize];
        self.b.const_composite(t, &parts)
    }

    fn splat(&mut self, s: IrScalar, n: u32, v: u32) -> u32 {
        if n <= 1 {
            return v;
        }
        let t = self.vec_id(s, n);
        let parts = vec![v; n as usize];
        self.ins_id(OP_COMPOSITE_CONSTRUCT, t, &parts)
    }

    // ---- globals ----

    fn global_sc(&self, g: GlobalId) -> u32 {
        match self.m.globals[g as usize].kind {
            IrGlobalKind::Io(IrIo::UniformBuffer) | IrGlobalKind::DynUniformBlock | IrGlobalKind::ScopeUniformBlock => SC_UNIFORM,
            IrGlobalKind::InstanceBuffer => SC_STORAGE_BUFFER,
            IrGlobalKind::Io(IrIo::Texture(_)) | IrGlobalKind::XrDepth | IrGlobalKind::Sampler(_) => SC_UNIFORM_CONSTANT,
            _ => SC_PRIVATE,
        }
    }

    fn global_var(&mut self, g: GlobalId) -> u32 {
        if self.global_ids[g as usize] != 0 {
            return self.global_ids[g as usize];
        }
        let glob = self.m.globals[g as usize].clone();
        let sc = self.global_sc(g);
        let id = match sc {
            SC_PRIVATE => {
                let p = self.ptr_ty(SC_PRIVATE, glob.ty);
                let t = self.ty(glob.ty);
                let null = self.b.const_null(t);
                self.b.global_variable(p, SC_PRIVATE, null)
            }
            SC_UNIFORM | SC_STORAGE_BUFFER => {
                // naga back/spv/writer.rs `write_global_variable`: the
                // buffer's type wrapped in a `Block` struct.
                let inner = self.ty(glob.ty);
                let wrapper = self.b.id();
                spv_inst(&mut self.b.globals, OP_TYPE_STRUCT, &[wrapper, inner]);
                self.b.decorate(wrapper, DECO_BLOCK, &[]);
                self.member_layout(wrapper, 0, glob.ty, 0);
                let p = self.b.type_pointer(sc, wrapper);
                let var = self.b.global_variable(p, sc, 0);
                if sc == SC_STORAGE_BUFFER {
                    self.b.decorate(var, DECO_NON_WRITABLE, &[]);
                }
                var
            }
            _ => {
                let p = self.ptr_ty(SC_UNIFORM_CONSTANT, glob.ty);
                self.b.global_variable(p, SC_UNIFORM_CONSTANT, 0)
            }
        };
        if sc != SC_PRIVATE {
            self.b.decorate(id, DECO_DESCRIPTOR_SET, &[0]);
            self.b.decorate(id, DECO_BINDING, &[glob.binding.unwrap_or(0)]);
        }
        if matches!(glob.kind, IrGlobalKind::ViewId) {
            self.uses_view_index = true;
        }
        self.global_ids[g as usize] = id;
        id
    }

    fn global_ptr(&mut self, g: GlobalId) -> Ptr {
        let var = self.global_var(g);
        let sc = self.global_sc(g);
        let ty = self.m.globals[g as usize].ty;
        if sc == SC_UNIFORM || sc == SC_STORAGE_BUFFER {
            let zero = self.b.const_u32(0);
            let p = self.ptr_ty(sc, ty);
            let id = self.ins_id(OP_ACCESS_CHAIN, p, &[var, zero]);
            return Ptr { id, ty, sc };
        }
        Ptr { id: var, ty, sc }
    }

    fn find_global(&self, kind: IrGlobalKind) -> Option<GlobalId> {
        for (i, g) in self.m.globals.iter().enumerate() {
            if g.kind == kind {
                return Some(i as GlobalId);
            }
        }
        None
    }

    // ---- functions ----

    fn fn_id(&mut self, fi: FuncId) -> u32 {
        if self.fn_ids[fi as usize] == 0 {
            self.fn_ids[fi as usize] = self.b.id();
            self.queue.push(fi);
        }
        self.fn_ids[fi as usize]
    }

    fn io_var(&mut self, io: &IrEntryIo, input: bool) -> u32 {
        let sc = if input { SC_INPUT } else { SC_OUTPUT };
        let p = self.ptr_ty(sc, io.ty);
        let var = self.b.global_variable(p, sc, 0);
        self.interface.push(var);
        let is_int = matches!(self.m.scalar_of(io.ty), Some(IrScalar::I32 | IrScalar::U32 | IrScalar::Bool));
        match io.binding {
            IrIoBinding::Location(loc) => {
                self.b.decorate(var, DECO_LOCATION, &[loc]);
                // naga back/spv/writer.rs `write_varying`: no interpolation
                // decorations on vertex inputs or fragment outputs.
                let allowed = !(input && self.stage == IrStage::Vertex) && !(!input && self.stage == IrStage::Fragment);
                if allowed && input && is_int {
                    self.b.decorate(var, DECO_FLAT, &[]);
                }
            }
            IrIoBinding::Builtin(bi) => {
                let builtin = match bi {
                    IrBuiltinIo::Position => {
                        if input {
                            BUILTIN_FRAG_COORD
                        } else {
                            BUILTIN_POSITION
                        }
                    }
                    IrBuiltinIo::VertexIndex => BUILTIN_VERTEX_INDEX,
                    IrBuiltinIo::InstanceIndex => BUILTIN_INSTANCE_INDEX,
                    IrBuiltinIo::ViewIndex => {
                        self.b.capability(CAP_MULTI_VIEW);
                        BUILTIN_VIEW_INDEX
                    }
                    IrBuiltinIo::FrontFacing => BUILTIN_FRONT_FACING,
                    IrBuiltinIo::FragDepth => BUILTIN_FRAG_DEPTH,
                };
                self.b.decorate(var, DECO_BUILT_IN, &[builtin]);
                // naga back/spv/writer.rs `write_varying`: every integer
                // fragment input is Flat (VUID-StandaloneSpirv-Flat-04744).
                if input && self.stage == IrStage::Fragment && is_int {
                    self.b.decorate(var, DECO_FLAT, &[]);
                }
            }
        }
        var
    }

    fn gen_function(&mut self, fi: FuncId) -> Result<(), String> {
        let m = self.m;
        let func = &m.functions[fi as usize];
        let fid = self.fn_ids[fi as usize];
        self.f = FnCx { func: fi, open: true, ..Default::default() };
        let mut header = Vec::new();
        if let Some(entry) = &func.entry {
            self.f.is_entry = true;
            let void = self.b.type_void();
            let fnty = self.b.type_function(void, &[]);
            spv_inst(&mut header, OP_FUNCTION, &[void, fid, 0, fnty]);
            for io in &entry.inputs {
                let v = self.io_var(io, true);
                self.f.entry_in.push(v);
            }
            for io in &entry.outputs {
                let v = self.io_var(io, false);
                self.f.entry_out.push(v);
            }
        } else {
            let ret = self.ty(func.ret);
            let mut param_tys = Vec::new();
            for p in &func.params {
                let t = if p.inout { self.ptr_ty(SC_FUNCTION, p.ty) } else { self.ty(p.ty) };
                param_tys.push(t);
            }
            let fnty = self.b.type_function(ret, &param_tys);
            spv_inst(&mut header, OP_FUNCTION, &[ret, fid, 0, fnty]);
            for t in param_tys {
                let id = self.b.id();
                spv_inst(&mut header, OP_FUNCTION_PARAMETER, &[t, id]);
                self.f.params.push(id);
            }
        }
        let mut locals = Vec::new();
        for l in &func.locals {
            if l.ty == TY_VOID {
                locals.push(0);
                continue;
            }
            locals.push(self.local_var(l.ty));
        }
        self.f.locals = locals;
        self.stmts(&func.body)?;
        if self.f.open {
            if func.ret == TY_VOID || self.f.is_entry {
                self.emit(OP_RETURN, &[]);
            } else {
                self.emit(OP_UNREACHABLE, &[]);
            }
            self.f.open = false;
        }
        let label = self.b.id();
        let out = &mut self.b.functions;
        out.extend_from_slice(&header);
        spv_inst(out, OP_LABEL, &[label]);
        out.extend_from_slice(&self.f.vars);
        out.extend_from_slice(&self.f.body);
        spv_inst(out, OP_FUNCTION_END, &[]);
        Ok(())
    }

    fn func(&self) -> &'a IrFunction {
        &self.m.functions[self.f.func as usize]
    }

    fn ety(&self, e: ExprId) -> TyId {
        self.func().exprs[e as usize].ty
    }

    // ---- statements ----

    fn stmts(&mut self, stmts: &[Stmt]) -> Result<(), String> {
        for s in stmts {
            if !self.f.open {
                break;
            }
            self.stmt(s)?;
        }
        Ok(())
    }

    fn stmt(&mut self, s: &Stmt) -> Result<(), String> {
        match s {
            Stmt::Local(l, init) => {
                let var = self.f.locals[*l as usize];
                if var == 0 {
                    return Ok(());
                }
                let ty = self.func().locals[*l as usize].ty;
                let v = if *init == NO_EXPR || matches!(self.func().exprs[*init as usize].kind, ExprKind::Error) {
                    let t = self.ty(ty);
                    self.b.const_null(t)
                } else {
                    self.value(*init)?
                };
                self.emit(OP_STORE, &[var, v]);
            }
            Stmt::Assign(place, value) => {
                let v = self.value(*value)?;
                self.store(*place, v)?;
            }
            Stmt::CompoundAssign(place, op, value) => {
                if let ExprKind::Swizzle(_, _, n) = self.func().exprs[*place as usize].kind {
                    if n > 1 {
                        let cur = self.value(*place)?;
                        let pty = self.ety(*place);
                        let r = self.value(*value)?;
                        let rty = self.ety(*value);
                        let res = self.binary(*op, cur, pty, r, rty, pty)?;
                        return self.store(*place, res);
                    }
                }
                let p = self.place(*place)?;
                let cur = self.load(p);
                let r = self.value(*value)?;
                let rty = self.ety(*value);
                let res = self.binary(*op, cur, p.ty, r, rty, p.ty)?;
                self.emit(OP_STORE, &[p.id, res]);
            }
            Stmt::Expr(e) => {
                if !matches!(self.func().exprs[*e as usize].kind, ExprKind::Nop | ExprKind::Error) {
                    self.value(*e)?;
                }
            }
            Stmt::If(c, then, els) => {
                let c = self.value(*c)?;
                let then_l = self.b.id();
                let merge = self.b.id();
                let else_l = if els.is_empty() { merge } else { self.b.id() };
                self.emit(OP_SELECTION_MERGE, &[merge, 0]);
                self.emit(OP_BRANCH_CONDITIONAL, &[c, then_l, else_l]);
                self.label(then_l);
                self.stmts(then)?;
                let mut reaches = els.is_empty();
                if self.f.open {
                    self.branch(merge);
                    reaches = true;
                }
                if !els.is_empty() {
                    self.label(else_l);
                    self.stmts(els)?;
                    if self.f.open {
                        self.branch(merge);
                        reaches = true;
                    }
                }
                self.label(merge);
                if !reaches {
                    self.emit(OP_UNREACHABLE, &[]);
                    self.f.open = false;
                }
            }
            Stmt::For(var, start, end, body) => {
                let v = self.f.locals[*var as usize];
                let s = self.value(*start)?;
                self.emit(OP_STORE, &[v, s]);
                let header = self.b.id();
                let body_l = self.b.id();
                let cont = self.b.id();
                let merge = self.b.id();
                self.branch(header);
                self.label(header);
                let cur = self.ins(OP_LOAD, TY_U32, &[v]);
                let e = self.value(*end)?;
                let bool_t = self.b.type_bool();
                let c = self.ins_id(OP_U_LESS_THAN, bool_t, &[cur, e]);
                self.emit(OP_LOOP_MERGE, &[merge, cont, 0]);
                self.emit(OP_BRANCH_CONDITIONAL, &[c, body_l, merge]);
                self.f.open = false;
                self.label(body_l);
                self.f.loops.push((merge, cont));
                let r = self.stmts(body);
                self.f.loops.pop();
                r?;
                if self.f.open {
                    self.branch(cont);
                }
                self.label(cont);
                let cur = self.ins(OP_LOAD, TY_U32, &[v]);
                let one = self.b.const_u32(1);
                let next = self.ins(OP_I_ADD, TY_U32, &[cur, one]);
                self.emit(OP_STORE, &[v, next]);
                self.branch(header);
                self.label(merge);
            }
            Stmt::Loop(body) => {
                let header = self.b.id();
                let body_l = self.b.id();
                let cont = self.b.id();
                let merge = self.b.id();
                self.branch(header);
                self.label(header);
                self.emit(OP_LOOP_MERGE, &[merge, cont, 0]);
                self.branch(body_l);
                self.label(body_l);
                self.f.loops.push((merge, cont));
                let r = self.stmts(body);
                self.f.loops.pop();
                r?;
                if self.f.open {
                    self.branch(cont);
                }
                self.label(cont);
                self.branch(header);
                self.label(merge);
            }
            Stmt::Break => {
                let (merge, _) = match self.f.loops.last() {
                    Some(l) => *l,
                    None => return err("break outside a loop"),
                };
                self.branch(merge);
            }
            Stmt::Continue => {
                let (_, cont) = match self.f.loops.last() {
                    Some(l) => *l,
                    None => return err("continue outside a loop"),
                };
                self.branch(cont);
            }
            Stmt::Return(e) => {
                if self.f.is_entry || *e == NO_EXPR {
                    if *e != NO_EXPR {
                        self.value(*e)?;
                    }
                    self.emit(OP_RETURN, &[]);
                } else {
                    let v = self.value(*e)?;
                    self.emit(OP_RETURN_VALUE, &[v]);
                }
                self.f.open = false;
            }
            Stmt::Discard => {
                self.emit(OP_KILL, &[]);
                self.f.open = false;
            }
        }
        Ok(())
    }

    fn store(&mut self, place: ExprId, v: u32) -> Result<(), String> {
        if let ExprKind::Swizzle(base, lanes, n) = self.func().exprs[place as usize].kind {
            if n > 1 {
                // A multi-lane swizzle write: shuffle the new lanes in.
                let p = self.place(base)?;
                let cur = self.load(p);
                let width = self.lanes(p.ty);
                let mut comps = Vec::new();
                for i in 0..width {
                    let mut src = i;
                    for k in 0..n as usize {
                        if lanes[k] as u32 == i {
                            src = width + k as u32;
                        }
                    }
                    comps.push(src);
                }
                let mut args = vec![cur, v];
                args.extend_from_slice(&comps);
                let t = self.ty(p.ty);
                let merged = self.ins_id(OP_VECTOR_SHUFFLE, t, &args);
                self.emit(OP_STORE, &[p.id, merged]);
                return Ok(());
            }
        }
        let p = self.place(place)?;
        self.emit(OP_STORE, &[p.id, v]);
        Ok(())
    }

    // ---- places ----

    fn is_place(&self, e: ExprId) -> bool {
        if e == NO_EXPR {
            return false;
        }
        match &self.func().exprs[e as usize].kind {
            ExprKind::Local(_) | ExprKind::Global(_) | ExprKind::Deref(_) | ExprKind::EntryOut(_) => true,
            ExprKind::Field(b, _) | ExprKind::Index(b, _) => self.is_place(*b),
            ExprKind::Swizzle(b, _, n) => *n == 1 && self.is_place(*b),
            _ => false,
        }
    }

    fn place(&mut self, e: ExprId) -> Result<Ptr, String> {
        let ty = self.ety(e);
        match self.func().exprs[e as usize].kind.clone() {
            ExprKind::Local(l) => {
                let id = self.f.locals[l as usize];
                Ok(Ptr { id, ty, sc: SC_FUNCTION })
            }
            ExprKind::Global(g) => Ok(self.global_ptr(g)),
            ExprKind::Deref(p) => match self.func().exprs[p as usize].kind {
                ExprKind::Param(i) => Ok(Ptr { id: self.f.params[i as usize], ty, sc: SC_FUNCTION }),
                _ => err("deref of a non-parameter"),
            },
            ExprKind::EntryOut(i) => Ok(Ptr { id: self.f.entry_out[i as usize], ty, sc: SC_OUTPUT }),
            ExprKind::Field(b, idx) if self.is_place(b) => {
                let bp = self.place(b)?;
                let c = self.b.const_u32(idx);
                let pt = self.ptr_ty(bp.sc, ty);
                let id = self.ins_id(OP_ACCESS_CHAIN, pt, &[bp.id, c]);
                Ok(Ptr { id, ty, sc: bp.sc })
            }
            ExprKind::Index(b, i) if self.is_place(b) => {
                let bp = self.place(b)?;
                let iv = self.value(i)?;
                let pt = self.ptr_ty(bp.sc, ty);
                let id = self.ins_id(OP_ACCESS_CHAIN, pt, &[bp.id, iv]);
                Ok(Ptr { id, ty, sc: bp.sc })
            }
            ExprKind::Swizzle(b, lanes, 1) if self.is_place(b) => {
                let bp = self.place(b)?;
                let c = self.b.const_u32(lanes[0] as u32);
                let pt = self.ptr_ty(bp.sc, ty);
                let id = self.ins_id(OP_ACCESS_CHAIN, pt, &[bp.id, c]);
                Ok(Ptr { id, ty, sc: bp.sc })
            }
            _ => {
                // A value used where a place is needed: a temporary.
                let v = self.value(e)?;
                let var = self.local_var(ty);
                self.emit(OP_STORE, &[var, v]);
                Ok(Ptr { id: var, ty, sc: SC_FUNCTION })
            }
        }
    }

    // ---- expressions ----

    fn value(&mut self, e: ExprId) -> Result<u32, String> {
        if e == NO_EXPR {
            return err("a missing value (an earlier shader error)");
        }
        let ty = self.ety(e);
        if self.is_place(e) {
            let p = self.place(e)?;
            return Ok(self.load(p));
        }
        let kind = self.func().exprs[e as usize].kind.clone();
        match kind {
            ExprKind::Lit(l) => Ok(match l {
                IrLit::Bool(b) => self.b.const_bool(b),
                IrLit::F32(v) => self.b.const_f32(v),
                IrLit::F16(v) => self.const_scalar(IrScalar::F16, v as f64),
                IrLit::I32(v) => self.b.const_i32(v),
                IrLit::U32(v) => self.b.const_u32(v),
                IrLit::AbstractInt(v) => self.b.const_i32(v as i32),
                IrLit::AbstractFloat(v) => self.b.const_f32(v as f32),
            }),
            ExprKind::Param(i) => Ok(self.f.params[i as usize]),
            ExprKind::EntryIn(i) => {
                let var = self.f.entry_in[i as usize];
                Ok(self.ins(OP_LOAD, ty, &[var]))
            }
            ExprKind::Field(b, idx) => {
                let bv = self.value(b)?;
                Ok(self.ins(OP_COMPOSITE_EXTRACT, ty, &[bv, idx]))
            }
            ExprKind::Swizzle(b, lanes, n) => {
                let bv = self.value(b)?;
                if n == 1 {
                    return Ok(self.ins(OP_COMPOSITE_EXTRACT, ty, &[bv, lanes[0] as u32]));
                }
                let mut args = vec![bv, bv];
                for k in 0..n as usize {
                    args.push(lanes[k] as u32);
                }
                Ok(self.ins(OP_VECTOR_SHUFFLE, ty, &args))
            }
            ExprKind::Index(b, i) => {
                let bty = self.ety(b);
                if let IrTy::Vec(..) = self.m.get(bty) {
                    let bv = self.value(b)?;
                    let iv = self.value(i)?;
                    return Ok(self.ins(OP_VECTOR_EXTRACT_DYNAMIC, ty, &[bv, iv]));
                }
                // An array or matrix value indexed at run time: through a
                // temporary.
                let p = self.place(b)?;
                let iv = self.value(i)?;
                let pt = self.ptr_ty(p.sc, ty);
                let ptr = self.ins_id(OP_ACCESS_CHAIN, pt, &[p.id, iv]);
                Ok(self.ins(OP_LOAD, ty, &[ptr]))
            }
            ExprKind::Unary(op, a) => {
                let av = self.value(a)?;
                let s = self.scalar_of(ty);
                Ok(match op {
                    UnOp::Neg => {
                        if s.is_float() {
                            self.ins(OP_F_NEGATE, ty, &[av])
                        } else {
                            self.ins(OP_S_NEGATE, ty, &[av])
                        }
                    }
                    UnOp::Not => self.ins(OP_LOGICAL_NOT, ty, &[av]),
                    UnOp::BitNot => self.ins(OP_NOT, ty, &[av]),
                })
            }
            ExprKind::Binary(op, a, b) => {
                let av = self.value(a)?;
                let bv = self.value(b)?;
                let at = self.ety(a);
                let bt = self.ety(b);
                self.binary(op, av, at, bv, bt, ty)
            }
            ExprKind::Convert(a) => {
                let av = self.value(a)?;
                let at = self.ety(a);
                self.convert(av, at, ty)
            }
            ExprKind::Bitcast(a) => {
                let av = self.value(a)?;
                if self.ety(a) == ty {
                    return Ok(av);
                }
                Ok(self.ins(OP_BITCAST, ty, &[av]))
            }
            ExprKind::Construct(args) => self.construct(ty, &args),
            ExprKind::Call(fi, args) => self.call(fi, &args, ty),
            ExprKind::Builtin(op, args) => self.builtin(op, &args, ty),
            ExprKind::Select(c, t, f) => {
                let cv = self.value(c)?;
                let tv = self.value(t)?;
                let fv = self.value(f)?;
                let n = self.lanes(ty);
                let cv = if n > 1 && self.lanes(self.ety(c)) == 1 { self.splat(IrScalar::Bool, n, cv) } else { cv };
                Ok(self.ins(OP_SELECT, ty, &[cv, tv, fv]))
            }
            ExprKind::Sample(s) => self.sample(&s, ty),
            ExprKind::TexSize(t) => self.tex_size(t, ty),
            ExprKind::TexLoad(t, c, layer, lod) => self.tex_load(t, c, layer, lod, ty),
            ExprKind::PassMatrix(which) => self.pass_matrix(which, ty),
            ExprKind::InstanceIndex => match self.find_global(IrGlobalKind::InstanceIndex) {
                Some(g) => {
                    let p = self.global_ptr(g);
                    Ok(self.load(p))
                }
                None => err("instance_index() outside a draw shader"),
            },
            ExprKind::DepthClip(w, c, clip) => {
                let helper = self.m.function_by_name("depth_clip");
                match helper {
                    Some(h) => self.call(h, &[w, c, clip], ty),
                    None => self.value(c),
                }
            }
            ExprKind::AddrOf(p) => {
                let p = self.place(p)?;
                Ok(p.id)
            }
            ExprKind::Nop | ExprKind::Error | ExprKind::Range(..) => err("a value expected (an earlier shader error)"),
            ExprKind::Local(_) | ExprKind::Global(_) | ExprKind::Deref(_) | ExprKind::EntryOut(_) => {
                let p = self.place(e)?;
                Ok(self.load(p))
            }
        }
    }

    fn call(&mut self, fi: FuncId, args: &[ExprId], ty: TyId) -> Result<u32, String> {
        let callee = &self.m.functions[fi as usize];
        let params = callee.params.clone();
        let fid = self.fn_id(fi);
        let mut ids = Vec::new();
        let mut writebacks = Vec::new();
        for (k, a) in args.iter().enumerate() {
            let inout = params.get(k).map(|p| p.inout).unwrap_or(false);
            if !inout {
                ids.push(self.value(*a)?);
                continue;
            }
            // A reference argument: a function-local variable or a pointer
            // param passes as is; anything else (a private global, a part
            // of a variable) goes through a temporary copied in and out.
            let target = match self.func().exprs[*a as usize].kind {
                ExprKind::AddrOf(p) => p,
                _ => *a,
            };
            match self.func().exprs[target as usize].kind.clone() {
                ExprKind::Param(i) => ids.push(self.f.params[i as usize]),
                ExprKind::Local(l) => ids.push(self.f.locals[l as usize]),
                ExprKind::Deref(p) => match self.func().exprs[p as usize].kind {
                    ExprKind::Param(i) => ids.push(self.f.params[i as usize]),
                    _ => return err("deref of a non-parameter"),
                },
                _ => {
                    let pty = params[k].ty;
                    let tmp = self.local_var(pty);
                    if self.is_place(target) {
                        let p = self.place(target)?;
                        let v = self.load(p);
                        self.emit(OP_STORE, &[tmp, v]);
                        writebacks.push((tmp, target, pty));
                    } else {
                        let v = self.value(target)?;
                        self.emit(OP_STORE, &[tmp, v]);
                    }
                    ids.push(tmp);
                }
            }
        }
        let mut words = vec![fid];
        words.extend_from_slice(&ids);
        let r = self.ins(OP_FUNCTION_CALL, ty, &words);
        for (tmp, target, pty) in writebacks {
            let v = self.ins(OP_LOAD, pty, &[tmp]);
            self.store(target, v)?;
        }
        Ok(r)
    }

    /// Numeric conversion (naga back/spv/block.rs `write_as`: float to
    /// integer clamps to the target's representable range first).
    fn convert(&mut self, id: u32, from: TyId, to: TyId) -> Result<u32, String> {
        if from == to {
            return Ok(id);
        }
        let (fs, ts) = match (self.m.scalar_of(from), self.m.scalar_of(to)) {
            (Some(a), Some(b)) => (a, b),
            _ => return err("conversion of a non-numeric value"),
        };
        let n = self.lanes(to);
        if self.lanes(from) != n {
            if self.lanes(from) == 1 {
                // A scalar converted to a vector: convert then splat.
                let c = self.convert_scalar(id, fs, ts, 1)?;
                return Ok(self.splat(ts, n, c));
            }
            return err("conversion between shapes");
        }
        self.convert_scalar(id, fs, ts, n.max(1))
    }

    fn convert_scalar(&mut self, id: u32, fs: IrScalar, ts: IrScalar, n: u32) -> Result<u32, String> {
        if fs == ts {
            return Ok(id);
        }
        let to = self.vec_id(ts, n);
        Ok(match (fs, ts) {
            (IrScalar::Bool, _) => {
                let one = self.splat_const(ts, n, 1.0);
                let zero = self.splat_const(ts, n, 0.0);
                self.ins_id(OP_SELECT, to, &[id, one, zero])
            }
            (_, IrScalar::Bool) => {
                let zero = self.splat_const(fs, n, 0.0);
                let op = if fs.is_float() { OP_F_ORD_NOT_EQUAL } else { OP_I_NOT_EQUAL };
                self.ins_id(op, to, &[id, zero])
            }
            (IrScalar::F32 | IrScalar::F16, IrScalar::I32 | IrScalar::U32) => {
                let (lo, hi) = if ts == IrScalar::I32 { (-2147483648.0, 2147483520.0) } else { (0.0, 4294967040.0) };
                let lo = self.splat_const(fs, n, lo);
                let hi = self.splat_const(fs, n, hi);
                let ft = self.vec_id(fs, n);
                let clamped = self.ext_id(GL_F_CLAMP, ft, &[id, lo, hi]);
                let op = if ts == IrScalar::I32 { OP_CONVERT_F_TO_S } else { OP_CONVERT_F_TO_U };
                self.ins_id(op, to, &[clamped])
            }
            (IrScalar::I32, IrScalar::F32 | IrScalar::F16) => self.ins_id(OP_CONVERT_S_TO_F, to, &[id]),
            (IrScalar::U32, IrScalar::F32 | IrScalar::F16) => self.ins_id(OP_CONVERT_U_TO_F, to, &[id]),
            (IrScalar::F32, IrScalar::F16) | (IrScalar::F16, IrScalar::F32) => self.ins_id(OP_F_CONVERT, to, &[id]),
            _ => self.ins_id(OP_BITCAST, to, &[id]),
        })
    }

    fn binary(&mut self, op: BinOp, a: u32, at: TyId, b: u32, bt: TyId, rt: TyId) -> Result<u32, String> {
        let (al, bl) = (self.lanes(at), self.lanes(bt));
        let is_mat = |m: &IrModule, t: TyId| matches!(m.get(t), IrTy::Mat(..));
        if is_mat(self.m, at) || is_mat(self.m, bt) {
            return self.matrix_binary(op, a, at, b, bt, rt);
        }
        let s = self.scalar_of(at);
        // Float vector times scalar.
        if op == BinOp::Mul && s.is_float() {
            if al > 1 && bl == 1 {
                return Ok(self.ins(OP_VECTOR_TIMES_SCALAR, rt, &[a, b]));
            }
            if al == 1 && bl > 1 {
                return Ok(self.ins(OP_VECTOR_TIMES_SCALAR, rt, &[b, a]));
            }
        }
        let n = al.max(bl);
        let (a, b) = if al == 1 && n > 1 && op != BinOp::Shl && op != BinOp::Shr {
            let sa = self.scalar_of(at);
            (self.splat(sa, n, a), b)
        } else {
            (a, b)
        };
        let b = if bl == 1 && n > 1 {
            let sb = self.scalar_of(bt);
            self.splat(sb, n, b)
        } else {
            b
        };
        let operand_ty = if al >= bl { at } else { bt };
        let o = match op {
            BinOp::Add => if s.is_float() { OP_F_ADD } else { OP_I_ADD },
            BinOp::Sub => if s.is_float() { OP_F_SUB } else { OP_I_SUB },
            BinOp::Mul => if s.is_float() { OP_F_MUL } else { OP_I_MUL },
            BinOp::Div | BinOp::Rem => {
                if s.is_float() {
                    if op == BinOp::Div { OP_F_DIV } else { OP_F_REM }
                } else {
                    return Ok(self.safe_int_div(op, a, b, operand_ty, s, n));
                }
            }
            BinOp::Shl => OP_SHIFT_LEFT_LOGICAL,
            BinOp::Shr => if s == IrScalar::I32 { OP_SHIFT_RIGHT_ARITHMETIC } else { OP_SHIFT_RIGHT_LOGICAL },
            BinOp::BitAnd => if s == IrScalar::Bool { OP_LOGICAL_AND } else { OP_BITWISE_AND },
            BinOp::BitOr => if s == IrScalar::Bool { OP_LOGICAL_OR } else { OP_BITWISE_OR },
            BinOp::BitXor => if s == IrScalar::Bool { OP_LOGICAL_NOT_EQUAL } else { OP_BITWISE_XOR },
            BinOp::LogicAnd => OP_LOGICAL_AND,
            BinOp::LogicOr => OP_LOGICAL_OR,
            BinOp::Eq => match s {
                IrScalar::Bool => OP_LOGICAL_EQUAL,
                s if s.is_float() => OP_F_ORD_EQUAL,
                _ => OP_I_EQUAL,
            },
            BinOp::Ne => match s {
                IrScalar::Bool => OP_LOGICAL_NOT_EQUAL,
                s if s.is_float() => OP_F_ORD_NOT_EQUAL,
                _ => OP_I_NOT_EQUAL,
            },
            BinOp::Lt => match s {
                s if s.is_float() => OP_F_ORD_LESS_THAN,
                IrScalar::I32 => OP_S_LESS_THAN,
                _ => OP_U_LESS_THAN,
            },
            BinOp::Le => match s {
                s if s.is_float() => OP_F_ORD_LESS_THAN_EQUAL,
                IrScalar::I32 => OP_S_LESS_THAN_EQUAL,
                _ => OP_U_LESS_THAN_EQUAL,
            },
            BinOp::Gt => match s {
                s if s.is_float() => OP_F_ORD_GREATER_THAN,
                IrScalar::I32 => OP_S_GREATER_THAN,
                _ => OP_U_GREATER_THAN,
            },
            BinOp::Ge => match s {
                s if s.is_float() => OP_F_ORD_GREATER_THAN_EQUAL,
                IrScalar::I32 => OP_S_GREATER_THAN_EQUAL,
                _ => OP_U_GREATER_THAN_EQUAL,
            },
        };
        if (op == BinOp::Shl || op == BinOp::Shr) && al > 1 && bl == 1 {
            let sb = self.scalar_of(bt);
            let b2 = self.splat(sb, al, b);
            return Ok(self.ins(o, rt, &[a, b2]));
        }
        Ok(self.ins(o, rt, &[a, b]))
    }

    fn matrix_binary(&mut self, op: BinOp, a: u32, at: TyId, b: u32, bt: TyId, rt: TyId) -> Result<u32, String> {
        let am = self.m.get(at).clone();
        let bm = self.m.get(bt).clone();
        match (op, &am, &bm) {
            (BinOp::Mul, IrTy::Mat(..), IrTy::Vec(..)) => Ok(self.ins(OP_MATRIX_TIMES_VECTOR, rt, &[a, b])),
            (BinOp::Mul, IrTy::Vec(..), IrTy::Mat(..)) => Ok(self.ins(OP_VECTOR_TIMES_MATRIX, rt, &[a, b])),
            (BinOp::Mul, IrTy::Mat(..), IrTy::Mat(..)) => Ok(self.ins(OP_MATRIX_TIMES_MATRIX, rt, &[a, b])),
            (BinOp::Mul, IrTy::Mat(..), IrTy::Scalar(_)) => Ok(self.ins(OP_MATRIX_TIMES_SCALAR, rt, &[a, b])),
            (BinOp::Mul, IrTy::Scalar(_), IrTy::Mat(..)) => Ok(self.ins(OP_MATRIX_TIMES_SCALAR, rt, &[b, a])),
            (BinOp::Add | BinOp::Sub, IrTy::Mat(c, r), IrTy::Mat(..)) => {
                let col = self.vec_id(IrScalar::F32, *r as u32);
                let mut cols = Vec::new();
                for k in 0..*c as u32 {
                    let x = self.ins_id(OP_COMPOSITE_EXTRACT, col, &[a, k]);
                    let y = self.ins_id(OP_COMPOSITE_EXTRACT, col, &[b, k]);
                    let o = if op == BinOp::Add { OP_F_ADD } else { OP_F_SUB };
                    cols.push(self.ins_id(o, col, &[x, y]));
                }
                Ok(self.ins(OP_COMPOSITE_CONSTRUCT, rt, &cols))
            }
            _ => err("unsupported matrix operation"),
        }
    }

    /// Integer `/` and `%` that never divide by zero or overflow (naga
    /// back/spv/writer.rs `write_wrapped_binary_op`, inlined).
    fn safe_int_div(&mut self, op: BinOp, l: u32, r: u32, ty: TyId, s: IrScalar, n: u32) -> u32 {
        let bool_t = self.vec_id(IrScalar::Bool, n);
        let zero = self.splat_const(s, n, 0.0);
        let one = self.splat_const(s, n, 1.0);
        let r_zero = self.ins_id(OP_I_EQUAL, bool_t, &[r, zero]);
        let bad = if s == IrScalar::I32 {
            let min = self.splat_const(s, n, i32::MIN as f64);
            let neg = self.splat_const(s, n, -1.0);
            let l_min = self.ins_id(OP_I_EQUAL, bool_t, &[l, min]);
            let r_neg = self.ins_id(OP_I_EQUAL, bool_t, &[r, neg]);
            let both = self.ins_id(OP_LOGICAL_AND, bool_t, &[l_min, r_neg]);
            self.ins_id(OP_LOGICAL_OR, bool_t, &[r_zero, both])
        } else {
            r_zero
        };
        let t = self.ty(ty);
        let divisor = self.ins_id(OP_SELECT, t, &[bad, one, r]);
        let o = match (op, s) {
            (BinOp::Div, IrScalar::I32) => OP_S_DIV,
            (BinOp::Div, _) => OP_U_DIV,
            (_, IrScalar::I32) => OP_S_REM,
            _ => OP_U_MOD,
        };
        self.ins_id(o, t, &[l, divisor])
    }

    fn construct(&mut self, ty: TyId, args: &[ExprId]) -> Result<u32, String> {
        let t = self.ty(ty);
        if args.is_empty() {
            return Ok(self.b.const_null(t));
        }
        match self.m.get(ty).clone() {
            IrTy::Scalar(_) => {
                let v = self.value(args[0])?;
                let at = self.ety(args[0]);
                self.convert(v, at, ty)
            }
            IrTy::Vec(n, s) => {
                if args.len() == 1 {
                    let v = self.value(args[0])?;
                    let at = self.ety(args[0]);
                    if self.lanes(at) == 1 {
                        let c = self.convert_scalar(v, self.scalar_of(at), s, 1)?;
                        return Ok(self.splat(s, n as u32, c));
                    }
                    return self.convert(v, at, ty);
                }
                let mut parts = Vec::new();
                let mut count = 0;
                for a in args {
                    let v = self.value(*a)?;
                    let at = self.ety(*a);
                    count += self.lanes(at);
                    parts.push(v);
                }
                if count != n as u32 {
                    return err("vector constructor with the wrong number of components");
                }
                Ok(self.ins_id(OP_COMPOSITE_CONSTRUCT, t, &parts))
            }
            IrTy::Mat(c, r) => {
                let col_t = self.vec_id(IrScalar::F32, r as u32);
                if args.len() == 1 {
                    let v = self.value(args[0])?;
                    let at = self.ety(args[0]);
                    if at == ty {
                        return Ok(v);
                    }
                    // A scalar: the diagonal.
                    let zero = self.b.const_f32(0.0);
                    let mut cols = Vec::new();
                    for k in 0..c as u32 {
                        let mut comps = Vec::new();
                        for j in 0..r as u32 {
                            comps.push(if j == k { v } else { zero });
                        }
                        cols.push(self.ins_id(OP_COMPOSITE_CONSTRUCT, col_t, &comps));
                    }
                    return Ok(self.ins_id(OP_COMPOSITE_CONSTRUCT, t, &cols));
                }
                let mut vals = Vec::new();
                for a in args {
                    vals.push(self.value(*a)?);
                }
                if args.len() as u32 == c as u32 {
                    return Ok(self.ins_id(OP_COMPOSITE_CONSTRUCT, t, &vals));
                }
                if args.len() as u32 == c as u32 * r as u32 {
                    let mut cols = Vec::new();
                    for k in 0..c as usize {
                        let part = &vals[k * r as usize..(k + 1) * r as usize];
                        cols.push(self.ins_id(OP_COMPOSITE_CONSTRUCT, col_t, part));
                    }
                    return Ok(self.ins_id(OP_COMPOSITE_CONSTRUCT, t, &cols));
                }
                err("matrix constructor with the wrong number of arguments")
            }
            IrTy::Struct(_) | IrTy::Array(..) => {
                let mut vals = Vec::new();
                for a in args {
                    vals.push(self.value(*a)?);
                }
                Ok(self.ins_id(OP_COMPOSITE_CONSTRUCT, t, &vals))
            }
            _ => err("cannot construct this type"),
        }
    }

    /// Component-wise builtin arguments: each converted to `s` and
    /// splatted to `n` lanes.
    fn uniform_args(&mut self, args: &[ExprId], s: IrScalar, n: u32) -> Result<Vec<u32>, String> {
        let mut out = Vec::new();
        for a in args {
            let v = self.value(*a)?;
            let at = self.ety(*a);
            let al = self.lanes(at);
            let as_ = self.scalar_of(at);
            let v = if as_ != s { self.convert_scalar(v, as_, s, al.max(1))? } else { v };
            out.push(if al == 1 && n > 1 { self.splat(s, n, v) } else { v });
        }
        Ok(out)
    }

    fn builtin(&mut self, op: Builtin, args: &[ExprId], ty: TyId) -> Result<u32, String> {
        let rs = self.scalar_of(ty);
        let rn = self.lanes(ty).max(1);
        let t = self.ty(ty);
        let gl_float = match op {
            Builtin::Sin => Some(GL_SIN),
            Builtin::Cos => Some(GL_COS),
            Builtin::Tan => Some(GL_TAN),
            Builtin::Asin => Some(GL_ASIN),
            Builtin::Acos => Some(GL_ACOS),
            Builtin::Atan => Some(GL_ATAN),
            Builtin::Sinh => Some(GL_SINH),
            Builtin::Cosh => Some(GL_COSH),
            Builtin::Tanh => Some(GL_TANH),
            Builtin::Asinh => Some(GL_ASINH),
            Builtin::Acosh => Some(GL_ACOSH),
            Builtin::Atanh => Some(GL_ATANH),
            Builtin::Atan2 => Some(GL_ATAN2),
            Builtin::Radians => Some(GL_RADIANS),
            Builtin::Degrees => Some(GL_DEGREES),
            Builtin::Exp => Some(GL_EXP),
            Builtin::Exp2 => Some(GL_EXP2),
            Builtin::Log => Some(GL_LOG),
            Builtin::Log2 => Some(GL_LOG2),
            Builtin::Pow => Some(GL_POW),
            Builtin::Sqrt => Some(GL_SQRT),
            Builtin::InverseSqrt => Some(GL_INVERSE_SQRT),
            Builtin::Floor => Some(GL_FLOOR),
            Builtin::Ceil => Some(GL_CEIL),
            Builtin::Fract => Some(GL_FRACT),
            Builtin::Trunc => Some(GL_TRUNC),
            Builtin::Round => Some(GL_ROUND_EVEN),
            Builtin::Fma => Some(GL_FMA),
            Builtin::Step => Some(GL_STEP),
            Builtin::Smoothstep => Some(GL_SMOOTH_STEP),
            Builtin::Normalize => Some(GL_NORMALIZE),
            Builtin::Cross => Some(GL_CROSS),
            // naga back/spv/block.rs: a scalar mix selector is splatted.
            Builtin::Mix => Some(GL_F_MIX),
            _ => None,
        };
        if let Some(gl) = gl_float {
            let ids = self.uniform_args(args, rs, rn)?;
            return Ok(self.ext_id(gl, t, &ids));
        }
        Ok(match op {
            Builtin::Abs => {
                let ids = self.uniform_args(args, rs, rn)?;
                match rs {
                    IrScalar::I32 => self.ext_id(GL_S_ABS, t, &ids),
                    // naga back/spv/block.rs: abs of an unsigned is a copy.
                    IrScalar::U32 => self.ins_id(OP_COPY_OBJECT, t, &ids),
                    _ => self.ext_id(GL_F_ABS, t, &ids),
                }
            }
            Builtin::Min | Builtin::Max => {
                let ids = self.uniform_args(args, rs, rn)?;
                let gl = match (op, rs) {
                    (Builtin::Min, IrScalar::I32) => GL_S_MIN,
                    (Builtin::Min, IrScalar::U32) => GL_U_MIN,
                    (Builtin::Min, _) => GL_F_MIN,
                    (_, IrScalar::I32) => GL_S_MAX,
                    (_, IrScalar::U32) => GL_U_MAX,
                    _ => GL_F_MAX,
                };
                self.ext_id(gl, t, &ids)
            }
            Builtin::Clamp => {
                let ids = self.uniform_args(args, rs, rn)?;
                if ids.len() != 3 {
                    return err("clamp takes 3 arguments");
                }
                // naga back/spv/block.rs: integer clamp as max then min.
                match rs {
                    IrScalar::I32 => {
                        let x = self.ext_id(GL_S_MAX, t, &[ids[0], ids[1]]);
                        self.ext_id(GL_S_MIN, t, &[x, ids[2]])
                    }
                    IrScalar::U32 => {
                        let x = self.ext_id(GL_U_MAX, t, &[ids[0], ids[1]]);
                        self.ext_id(GL_U_MIN, t, &[x, ids[2]])
                    }
                    _ => self.ext_id(GL_F_CLAMP, t, &ids),
                }
            }
            Builtin::Sign => {
                let ids = self.uniform_args(args, rs, rn)?;
                let gl = if rs == IrScalar::I32 { GL_S_SIGN } else { GL_F_SIGN };
                self.ext_id(gl, t, &ids)
            }
            Builtin::Length | Builtin::Distance => {
                let at = self.ety(args[0]);
                let n = self.lanes(at).max(1);
                let s = self.scalar_of(at);
                let ids = self.uniform_args(args, s, n)?;
                let gl = if op == Builtin::Length { GL_LENGTH } else { GL_DISTANCE };
                self.ext_id(gl, t, &ids)
            }
            Builtin::Dot => {
                let at = self.ety(args[0]);
                let n = self.lanes(at).max(1);
                let s = self.scalar_of(at);
                let ids = self.uniform_args(args, s, n)?;
                if s.is_float() {
                    self.ins_id(OP_DOT, t, &ids)
                } else {
                    let vt = self.vec_id(s, n);
                    let prod = self.ins_id(OP_I_MUL, vt, &ids);
                    let mut sum = self.ins_id(OP_COMPOSITE_EXTRACT, t, &[prod, 0]);
                    for k in 1..n {
                        let c = self.ins_id(OP_COMPOSITE_EXTRACT, t, &[prod, k]);
                        sum = self.ins_id(OP_I_ADD, t, &[sum, c]);
                    }
                    sum
                }
            }
            Builtin::FMod => {
                let ids = self.uniform_args(args, rs, rn)?;
                self.ins_id(OP_F_REM, t, &ids)
            }
            Builtin::Inverse => {
                let v = self.value(args[0])?;
                self.ext_id(GL_MATRIX_INVERSE, t, &[v])
            }
            Builtin::Dpdx | Builtin::Dpdy => {
                let ids = self.uniform_args(args, rs, rn)?;
                let o = if op == Builtin::Dpdx { OP_DPDX } else { OP_DPDY };
                self.ins_id(o, t, &ids)
            }
            Builtin::Unpack2f16 | Builtin::Unpack4u8 => {
                let v = self.value(args[0])?;
                let at = self.ety(args[0]);
                let u = if self.scalar_of(at) == IrScalar::U32 {
                    v
                } else {
                    let ut = self.b.type_u32();
                    self.ins_id(OP_BITCAST, ut, &[v])
                };
                let gl = if op == Builtin::Unpack2f16 { GL_UNPACK_HALF_2X16 } else { GL_UNPACK_UNORM_4X8 };
                self.ext_id(gl, t, &[u])
            }
            Builtin::Unpack2x16Float | Builtin::Unpack2x16Unorm | Builtin::Unpack2x16Snorm | Builtin::Unpack4x8Unorm | Builtin::Unpack4x8Snorm => {
                let v = self.value(args[0])?;
                let gl = match op {
                    Builtin::Unpack2x16Float => GL_UNPACK_HALF_2X16,
                    Builtin::Unpack2x16Unorm => GL_UNPACK_UNORM_2X16,
                    Builtin::Unpack2x16Snorm => GL_UNPACK_SNORM_2X16,
                    Builtin::Unpack4x8Unorm => GL_UNPACK_UNORM_4X8,
                    _ => GL_UNPACK_SNORM_4X8,
                };
                self.ext_id(gl, t, &[v])
            }
            _ => return err("unsupported builtin"),
        })
    }

    // ---- textures ----

    fn sampled_image(&mut self, tex: ExprId, sampler: u32) -> Result<u32, String> {
        let img = self.value(tex)?;
        let tex_ty = self.ety(tex);
        let it = self.ty(tex_ty);
        let sg = match self.find_global(IrGlobalKind::Sampler(sampler)) {
            Some(g) => g,
            None => return err("a texture sample without its sampler"),
        };
        let sp = self.global_ptr(sg);
        let smp = self.load(sp);
        let st = self.b.type_sampled_image(it);
        Ok(self.ins_id(OP_SAMPLED_IMAGE, st, &[img, smp]))
    }

    fn sample(&mut self, s: &IrSample, ty: TyId) -> Result<u32, String> {
        let si = self.sampled_image(s.tex, s.sampler)?;
        let coord = self.value(s.coord)?;
        let (_, _, arrayed) = image_shape(s.tex_type);
        let coord = if arrayed {
            // The layer is the coordinate's last lane truncated (WGSL's
            // `i32(coord.z)`), then exact as a float.
            let ct = self.ety(s.coord);
            let n = self.lanes(ct);
            let f = self.b.type_f32();
            let i = self.b.type_i32();
            let layer = self.ins_id(OP_COMPOSITE_EXTRACT, f, &[coord, n - 1]);
            let li = self.ins_id(OP_CONVERT_F_TO_S, i, &[layer]);
            let lf = self.ins_id(OP_CONVERT_S_TO_F, f, &[li]);
            let mut args = Vec::new();
            for k in 0..n - 1 {
                args.push(self.ins_id(OP_COMPOSITE_EXTRACT, f, &[coord, k]));
            }
            args.push(lf);
            let vt = self.vec_id(IrScalar::F32, n);
            self.ins_id(OP_COMPOSITE_CONSTRUCT, vt, &args)
        } else {
            coord
        };
        let v4 = self.vec_id(IrScalar::F32, 4);
        let f32t = self.b.type_f32();
        let r = match s.kind {
            SampleKind::Implicit => {
                if self.stage == IrStage::Vertex {
                    let zero = self.b.const_f32(0.0);
                    self.ins_id(OP_IMAGE_SAMPLE_EXPLICIT_LOD, v4, &[si, coord, IMAGE_OPERAND_LOD, zero])
                } else {
                    self.ins_id(OP_IMAGE_SAMPLE_IMPLICIT_LOD, v4, &[si, coord])
                }
            }
            SampleKind::Lod => {
                let lod = self.value(s.lod)?;
                self.ins_id(OP_IMAGE_SAMPLE_EXPLICIT_LOD, v4, &[si, coord, IMAGE_OPERAND_LOD, lod])
            }
            SampleKind::Video => {
                let zero = self.b.const_f32(0.0);
                self.ins_id(OP_IMAGE_SAMPLE_EXPLICIT_LOD, v4, &[si, coord, IMAGE_OPERAND_LOD, zero])
            }
            SampleKind::Grad => {
                let dx = self.value(s.dx)?;
                let dy = self.value(s.dy)?;
                self.ins_id(OP_IMAGE_SAMPLE_EXPLICIT_LOD, v4, &[si, coord, IMAGE_OPERAND_GRAD, dx, dy])
            }
            SampleKind::CompareLevel0 => {
                let reference = self.value(s.reference)?;
                let zero = self.b.const_f32(0.0);
                return Ok(self.ins_id(OP_IMAGE_SAMPLE_DREF_EXPLICIT_LOD, f32t, &[si, coord, reference, IMAGE_OPERAND_LOD, zero]));
            }
        };
        if ty == TY_F32 {
            return Ok(self.ins_id(OP_COMPOSITE_EXTRACT, f32t, &[r, 0]));
        }
        Ok(r)
    }

    fn tex_size(&mut self, tex: ExprId, ty: TyId) -> Result<u32, String> {
        self.b.capability(CAP_IMAGE_QUERY);
        let img = self.value(tex)?;
        let tex_ty = self.ety(tex);
        let tt = match self.m.get(tex_ty) {
            IrTy::Texture(t) => *t,
            _ => return err("size() of a non-texture"),
        };
        let (dim, _, arrayed) = image_shape(tt);
        let n = if dim == DIM_3D || arrayed { 3 } else { 2 };
        let qt = self.vec_id(IrScalar::U32, n);
        let lod = self.b.const_u32(0);
        let size = self.ins_id(OP_IMAGE_QUERY_SIZE_LOD, qt, &[img, lod]);
        let size = if n == 3 {
            let u2 = self.vec_id(IrScalar::U32, 2);
            self.ins_id(OP_VECTOR_SHUFFLE, u2, &[size, size, 0, 1])
        } else {
            size
        };
        if self.scalar_of(ty) == IrScalar::U32 {
            return Ok(size);
        }
        let t = self.ty(ty);
        Ok(self.ins_id(OP_CONVERT_U_TO_F, t, &[size]))
    }

    fn tex_load(&mut self, tex: ExprId, coord: ExprId, layer: ExprId, lod: ExprId, ty: TyId) -> Result<u32, String> {
        let img = self.value(tex)?;
        let c = self.value(coord)?;
        let c = if layer != NO_EXPR {
            let l = self.value(layer)?;
            let lt = self.ety(layer);
            let l = self.convert(l, lt, TY_I32)?;
            let i = self.b.type_i32();
            let x = self.ins_id(OP_COMPOSITE_EXTRACT, i, &[c, 0]);
            let y = self.ins_id(OP_COMPOSITE_EXTRACT, i, &[c, 1]);
            let v3 = self.vec_id(IrScalar::I32, 3);
            self.ins_id(OP_COMPOSITE_CONSTRUCT, v3, &[x, y, l])
        } else {
            c
        };
        let lod = if lod == NO_EXPR { self.b.const_i32(0) } else { self.value(lod)? };
        let v4 = self.vec_id(IrScalar::F32, 4);
        let r = self.ins_id(OP_IMAGE_FETCH, v4, &[img, c, IMAGE_OPERAND_LOD, lod]);
        if ty == TY_F32 {
            let f = self.b.type_f32();
            return Ok(self.ins_id(OP_COMPOSITE_EXTRACT, f, &[r, 0]));
        }
        Ok(r)
    }

    /// The pass matrices: the draw pass uniform buffer's field, in XR the
    /// `_r` one for the right view.
    fn pass_matrix(&mut self, which: PassMatrix, ty: TyId) -> Result<u32, String> {
        let (name, name_r) = match which {
            PassMatrix::CameraProjection => (id!(camera_projection), id!(camera_projection_r)),
            PassMatrix::CameraView => (id!(camera_view), id!(camera_view_r)),
            PassMatrix::DepthProjection => (id!(depth_projection), id!(depth_projection_r)),
            PassMatrix::DepthView => (id!(depth_view), id!(depth_view_r)),
            PassMatrix::CameraInv => (id!(camera_inv), id!(camera_inv_r)),
        };
        let mut pass = None;
        for (i, g) in self.m.globals.iter().enumerate() {
            if g.kind == IrGlobalKind::Io(IrIo::UniformBuffer) && g.name == id!(draw_pass) {
                pass = Some(i as GlobalId);
            }
        }
        let Some(pass) = pass else {
            return err("the pass matrices need the draw pass uniforms");
        };
        let pty = self.m.globals[pass as usize].ty;
        let IrTy::Struct(si) = self.m.get(pty).clone() else {
            return err("the draw pass uniforms are not a struct");
        };
        let mut idx = None;
        let mut idx_r = None;
        for (k, f) in self.m.structs[si as usize].fields.iter().enumerate() {
            if f.name == name {
                idx = Some(k as u32);
            }
            if f.name == name_r {
                idx_r = Some(k as u32);
            }
        }
        let Some(idx) = idx else {
            return err("the draw pass uniforms lack a matrix");
        };
        let p = self.global_ptr(pass);
        let load_field = |g: &mut Gen, k: u32| -> u32 {
            let c = g.b.const_u32(k);
            let pt = g.ptr_ty(p.sc, ty);
            let fp = g.ins_id(OP_ACCESS_CHAIN, pt, &[p.id, c]);
            g.ins(OP_LOAD, ty, &[fp])
        };
        let base = load_field(self, idx);
        let (true, Some(idx_r)) = (self.xr, idx_r) else {
            return Ok(base);
        };
        let right = load_field(self, idx_r);
        let view = match self.find_global(IrGlobalKind::ViewId) {
            Some(g) => {
                let vp = self.global_ptr(g);
                self.load(vp)
            }
            None => return Ok(base),
        };
        let zero = self.b.const_i32(0);
        let bt = self.b.type_bool();
        let is_right = self.ins_id(OP_I_NOT_EQUAL, bt, &[view, zero]);
        // OpSelect of a matrix needs SPIR-V 1.4: select column by column.
        let (c, r) = match self.m.get(ty) {
            IrTy::Mat(c, r) => (*c as u32, *r as u32),
            _ => return Ok(base),
        };
        let col = self.vec_id(IrScalar::F32, r);
        let bv = self.vec_id(IrScalar::Bool, r);
        let cond = self.splat(IrScalar::Bool, r, is_right);
        let _ = bv;
        let mut cols = Vec::new();
        for k in 0..c {
            let a = self.ins_id(OP_COMPOSITE_EXTRACT, col, &[right, k]);
            let b = self.ins_id(OP_COMPOSITE_EXTRACT, col, &[base, k]);
            cols.push(self.ins_id(OP_SELECT, col, &[cond, a, b]));
        }
        let t = self.ty(ty);
        Ok(self.ins_id(OP_COMPOSITE_CONSTRUCT, t, &cols))
    }
}

fn image_shape(tt: TextureType) -> (u32, bool, bool) {
    match tt {
        TextureType::Texture1d | TextureType::Texture2d | TextureType::TextureVideo => (DIM_2D, false, false),
        TextureType::Texture1dArray | TextureType::Texture2dArray => (DIM_2D, false, true),
        TextureType::Texture3d | TextureType::Texture3dArray => (DIM_3D, false, false),
        TextureType::TextureCube => (DIM_CUBE, false, false),
        TextureType::TextureCubeArray => (DIM_CUBE, false, true),
        TextureType::TextureDepth => (DIM_2D, true, false),
        TextureType::TextureDepthArray => (DIM_2D, true, true),
    }
}

/// An f32 as IEEE half bits (round to nearest even).
fn f32_to_f16_bits(v: f32) -> u32 {
    let x = v.to_bits();
    let sign = (x >> 16) & 0x8000;
    let exp = ((x >> 23) & 0xff) as i32;
    let mant = x & 0x7f_ffff;
    if exp == 0xff {
        return sign | 0x7c00 | if mant != 0 { 0x200 } else { 0 };
    }
    let e = exp - 127 + 15;
    if e >= 0x1f {
        return sign | 0x7c00;
    }
    if e <= 0 {
        if e < -10 {
            return sign;
        }
        let m = (mant | 0x80_0000) >> (1 - e);
        let round = if (m & 0x1fff) > 0x1000 || ((m & 0x1fff) == 0x1000 && (m & 0x2000) != 0) { 1 } else { 0 };
        return sign | ((m >> 13) + round);
    }
    let round = if (mant & 0x1fff) > 0x1000 || ((mant & 0x1fff) == 0x1000 && (mant & 0x2000) != 0) { 1 } else { 0 };
    sign | (((e as u32) << 10) | (mant >> 13)) + round
}

/// A full-screen triangle sampling one texture: the vertex stage places
/// vertex 0..2 by `vertex_index` (uv (0,0), (2,0), (0,2)), the fragment
/// stage samples texture binding 0 with sampler binding 1 (set 0) at the
/// interpolated uv. Entry points `vertex_main` / `fragment_main`.
pub fn fullscreen_blit_spirv() -> (Vec<u32>, Vec<u32>) {
    // Vertex stage.
    let mut b = SpvBuilder::new();
    let void = b.type_void();
    let fn_ty = b.type_function(void, &[]);
    let u32_ty = b.type_u32();
    let f32_ty = b.type_f32();
    let v2 = b.type_vector(f32_ty, 2);
    let v4 = b.type_vector(f32_ty, 4);
    let in_u32 = b.type_pointer(SC_INPUT, u32_ty);
    let out_v4 = b.type_pointer(SC_OUTPUT, v4);
    let out_v2 = b.type_pointer(SC_OUTPUT, v2);
    let vertex_index = b.global_variable(in_u32, SC_INPUT, 0);
    b.decorate(vertex_index, DECO_BUILT_IN, &[BUILTIN_VERTEX_INDEX]);
    let position = b.global_variable(out_v4, SC_OUTPUT, 0);
    b.decorate(position, DECO_BUILT_IN, &[BUILTIN_POSITION]);
    let uv_out = b.global_variable(out_v2, SC_OUTPUT, 0);
    b.decorate(uv_out, DECO_LOCATION, &[0]);
    let one_u = b.const_u32(1);
    let two_u = b.const_u32(2);
    let zero_f = b.const_f32(0.0);
    let one_f = b.const_f32(1.0);
    let two_f = b.const_f32(2.0);
    let ones = b.const_composite(v2, &[one_f, one_f]);
    let main = b.id();
    let mut ids = Vec::new();
    for _ in 0..10 {
        ids.push(b.id());
    }
    let label = b.id();
    let f = &mut b.functions;
    spv_inst(f, OP_FUNCTION, &[void, main, 0, fn_ty]);
    spv_inst(f, OP_LABEL, &[label]);
    spv_inst(f, OP_LOAD, &[u32_ty, ids[0], vertex_index]);
    spv_inst(f, OP_SHIFT_LEFT_LOGICAL, &[u32_ty, ids[1], ids[0], one_u]);
    spv_inst(f, OP_BITWISE_AND, &[u32_ty, ids[2], ids[1], two_u]);
    spv_inst(f, OP_CONVERT_U_TO_F, &[f32_ty, ids[3], ids[2]]);
    spv_inst(f, OP_BITWISE_AND, &[u32_ty, ids[4], ids[0], two_u]);
    spv_inst(f, OP_CONVERT_U_TO_F, &[f32_ty, ids[5], ids[4]]);
    spv_inst(f, OP_COMPOSITE_CONSTRUCT, &[v2, ids[6], ids[3], ids[5]]);
    spv_inst(f, OP_STORE, &[uv_out, ids[6]]);
    spv_inst(f, OP_VECTOR_TIMES_SCALAR, &[v2, ids[7], ids[6], two_f]);
    spv_inst(f, OP_F_SUB, &[v2, ids[8], ids[7], ones]);
    spv_inst(f, OP_COMPOSITE_CONSTRUCT, &[v4, ids[9], ids[8], zero_f, one_f]);
    spv_inst(f, OP_STORE, &[position, ids[9]]);
    spv_inst(f, OP_RETURN, &[]);
    spv_inst(f, OP_FUNCTION_END, &[]);
    b.entry_point(EXEC_MODEL_VERTEX, main, "vertex_main", &[vertex_index, position, uv_out]);
    let vertex = b.finish();

    // Fragment stage.
    let mut b = SpvBuilder::new();
    let void = b.type_void();
    let fn_ty = b.type_function(void, &[]);
    let f32_ty = b.type_f32();
    let v2 = b.type_vector(f32_ty, 2);
    let v4 = b.type_vector(f32_ty, 4);
    let image = b.type_image(f32_ty, DIM_2D, false, false);
    let sampler = b.type_sampler();
    let sampled = b.type_sampled_image(image);
    let image_ptr = b.type_pointer(SC_UNIFORM_CONSTANT, image);
    let sampler_ptr = b.type_pointer(SC_UNIFORM_CONSTANT, sampler);
    let in_v2 = b.type_pointer(SC_INPUT, v2);
    let out_v4 = b.type_pointer(SC_OUTPUT, v4);
    let source = b.global_variable(image_ptr, SC_UNIFORM_CONSTANT, 0);
    b.decorate(source, DECO_DESCRIPTOR_SET, &[0]);
    b.decorate(source, DECO_BINDING, &[0]);
    let source_sampler = b.global_variable(sampler_ptr, SC_UNIFORM_CONSTANT, 0);
    b.decorate(source_sampler, DECO_DESCRIPTOR_SET, &[0]);
    b.decorate(source_sampler, DECO_BINDING, &[1]);
    let uv_in = b.global_variable(in_v2, SC_INPUT, 0);
    b.decorate(uv_in, DECO_LOCATION, &[0]);
    let color = b.global_variable(out_v4, SC_OUTPUT, 0);
    b.decorate(color, DECO_LOCATION, &[0]);
    let main = b.id();
    let mut ids = Vec::new();
    for _ in 0..5 {
        ids.push(b.id());
    }
    let label = b.id();
    let f = &mut b.functions;
    spv_inst(f, OP_FUNCTION, &[void, main, 0, fn_ty]);
    spv_inst(f, OP_LABEL, &[label]);
    spv_inst(f, OP_LOAD, &[image, ids[0], source]);
    spv_inst(f, OP_LOAD, &[sampler, ids[1], source_sampler]);
    spv_inst(f, OP_SAMPLED_IMAGE, &[sampled, ids[2], ids[0], ids[1]]);
    spv_inst(f, OP_LOAD, &[v2, ids[3], uv_in]);
    spv_inst(f, OP_IMAGE_SAMPLE_IMPLICIT_LOD, &[v4, ids[4], ids[2], ids[3]]);
    spv_inst(f, OP_STORE, &[color, ids[4]]);
    spv_inst(f, OP_RETURN, &[]);
    spv_inst(f, OP_FUNCTION_END, &[]);
    b.entry_point(EXEC_MODEL_FRAGMENT, main, "fragment_main", &[uv_in, color]);
    b.execution_mode(main, EXEC_MODE_ORIGIN_UPPER_LEFT);
    (vertex, b.finish())
}

/// Test support: runs `spirv-val` (Vulkan 1.1 rules) on a module when the
/// tool is installed. None when it is not.
pub fn spirv_val(words: &[u32]) -> Option<Result<(), String>> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static SEQ: AtomicUsize = AtomicUsize::new(0);
    let path = std::env::temp_dir().join(format!(
        "makepad-spirv-val-{}-{}.spv",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let mut bytes = Vec::with_capacity(words.len() * 4);
    for w in words {
        bytes.extend_from_slice(&w.to_le_bytes());
    }
    if std::fs::write(&path, &bytes).is_err() {
        return None;
    }
    let out = std::process::Command::new("spirv-val")
        .arg("--target-env")
        .arg("vulkan1.1")
        .arg(&path)
        .output();
    let _ = std::fs::remove_file(&path);
    match out {
        Err(_) => None,
        Ok(o) if o.status.success() => Some(Ok(())),
        Ok(o) => Some(Err(format!(
            "{}{}",
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        ))),
    }
}
