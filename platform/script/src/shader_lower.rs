//! Lowering of a Splash shader function's bytecode to the shader IR
//! (shader_ir.rs). One backend-blind pass: the same walk the text compiler
//! (`ShaderFnCompiler`) does, with IR expressions on the stack instead of
//! target-language text. Typing (`ShaderType`, the type tables), IO
//! collection on `ShaderOutput`, scope uniforms, table constants and live
//! literals, function memoisation and the loop-cost accounting are the
//! text compiler's, unchanged.

use crate::makepad_error_log::*;
use crate::mod_shader::*;
use crate::opcode::*;
use crate::pod::ScriptPodTy;
use crate::shader::ShaderType;
use crate::shader_ir::*;
use crate::shader_output::*;
use crate::suggest::*;
use crate::trap::*;
use crate::value::*;
use crate::vm::*;
use crate::*;
use makepad_live_id::*;
use makepad_math::*;

#[derive(Clone, Copy, Debug)]
pub(crate) enum IrVar {
    Param(u32, ScriptPodType),
    Local(LocalId, ScriptPodType, bool),
    IoSelf(ScriptObject),
    ScopeObject(ScriptObject),
    PodType(ScriptPodType),
}

#[derive(Default)]
pub(crate) struct IrScope {
    pub scopes: Vec<Vec<(LiveId, IrVar)>>,
}

impl IrScope {
    pub fn enter(&mut self) {
        self.scopes.push(Vec::new());
    }
    pub fn exit(&mut self) {
        self.scopes.pop();
    }
    pub fn define(&mut self, id: LiveId, var: IrVar) {
        if self.scopes.is_empty() {
            self.scopes.push(Vec::new());
        }
        let scope = self.scopes.last_mut().unwrap();
        for entry in scope.iter_mut() {
            if entry.0 == id {
                entry.1 = var;
                return;
            }
        }
        scope.push((id, var));
    }
    pub fn find(&self, id: LiveId) -> Option<IrVar> {
        let mut i = self.scopes.len();
        while i > 0 {
            i -= 1;
            let scope = &self.scopes[i];
            let mut j = scope.len();
            while j > 0 {
                j -= 1;
                if scope[j].0 == id {
                    return Some(scope[j].1);
                }
            }
        }
        None
    }
    pub fn all_names(&self) -> Vec<LiveId> {
        let mut out = Vec::new();
        for scope in &self.scopes {
            for (id, _) in scope {
                if !out.contains(id) {
                    out.push(*id);
                }
            }
        }
        out
    }
}

impl IrVar {
    pub fn pod(&self) -> ScriptPodType {
        match self {
            IrVar::Param(_, t) | IrVar::Local(_, t, _) | IrVar::PodType(t) => *t,
            _ => ScriptPodType::VOID,
        }
    }
}

#[derive(Debug)]
pub(crate) struct IrPodArg {
    pub name: Option<LiveId>,
    pub ty: ShaderType,
    pub e: ExprId,
}

#[derive(Debug)]
pub(crate) enum IrMe {
    FnBody {
        ret: Option<ScriptPodType>,
        escaped: bool,
        stack_depth: usize,
    },
    LoopBody {
        stack_depth: usize,
    },
    ForLoop {
        var: LocalId,
        start: ExprId,
        end: ExprId,
        stack_depth: usize,
    },
    IfBody {
        target_ip: u32,
        stack_depth: usize,
        cond: ExprId,
        /// The then branch, once the else branch is open.
        then_block: Option<Vec<Stmt>>,
        phi: Option<LocalId>,
        phi_type: Option<ShaderType>,
        has_return: bool,
        if_branch_returned: bool,
        phi_assigned_by_inner: bool,
        created_unreachable: bool,
    },
    LogicOp {
        target_ip: u32,
        op: BinOp,
        first: ExprId,
        first_type: ShaderType,
    },
    BuiltinCall {
        name: LiveId,
        args: Vec<(ShaderType, ExprId)>,
    },
    PodBuiltinMethod {
        name: LiveId,
        args: Vec<(ShaderType, ExprId)>,
    },
    ScriptCall {
        name: LiveId,
        fnobj: ScriptObject,
        sself: ShaderType,
        /// A pod method's receiver (a place passed by reference).
        self_arg: ExprId,
        args: Vec<(ShaderType, ExprId)>,
    },
    Pod {
        pod_ty: ScriptPodType,
        args: Vec<IrPodArg>,
    },
    ArrayConstruct {
        args: Vec<(ShaderType, ExprId)>,
        elem_ty: Option<ScriptPodType>,
    },
    TextureBuiltin {
        method_id: LiveId,
        tex_type: TextureType,
        tex: ExprId,
        args: Vec<(ShaderType, ExprId)>,
    },
}

pub(crate) struct IrLoopFrame {
    pub literal: Option<u64>,
    /// The `_mp_iter` charge literal, patched when the loop closes.
    pub charge: ExprId,
    pub body: u64,
}

#[derive(Default)]
pub(crate) struct IrStack {
    pub types: Vec<ShaderType>,
    pub exprs: Vec<ExprId>,
}

pub struct IrFnCompiler {
    pub(crate) f: IrFunction,
    pub(crate) stack: IrStack,
    pub(crate) script_scope: ScriptObject,
    pub(crate) scope: IrScope,
    pub(crate) mes: Vec<IrMe>,
    pub(crate) trap: ScriptTrapInner,
    /// Open statement lists: the function body, then each open branch or
    /// loop body.
    pub(crate) blocks: Vec<Vec<Stmt>>,
    pub(crate) skip_next_pop_to_me: bool,
    pub(crate) skip_next_neg: bool,
    pub(crate) loop_frames: Vec<IrLoopFrame>,
    pub(crate) fn_cost: u64,
}

impl IrFnCompiler {
    pub fn new(script_scope: ScriptObject) -> Self {
        IrFnCompiler {
            f: IrFunction::new(String::new(), TY_VOID),
            stack: IrStack::default(),
            script_scope,
            scope: IrScope { scopes: vec![Vec::new()] },
            mes: Vec::new(),
            trap: ScriptTrapInner::default(),
            blocks: vec![Vec::new()],
            skip_next_pop_to_me: false,
            skip_next_neg: false,
            loop_frames: Vec::new(),
            fn_cost: 0,
        }
    }

    // ---- stack ----

    pub(crate) fn push(&mut self, ty: ShaderType, e: ExprId) {
        if self.stack.types.len() > 1_000_000 {
            script_err_stack!(self.trap.pass(), "shader stack overflow");
            return;
        }
        self.stack.types.push(ty);
        self.stack.exprs.push(e);
    }

    pub(crate) fn pop(&mut self) -> (ShaderType, ExprId) {
        match self.stack.types.pop() {
            Some(t) => (t, self.stack.exprs.pop().unwrap()),
            None => {
                script_err_stack!(self.trap.pass(), "shader stack underflow");
                (ShaderType::Error(NIL), NO_EXPR)
            }
        }
    }

    pub(crate) fn emit(&mut self, s: Stmt) {
        if let Some(b) = self.blocks.last_mut() {
            b.push(s);
        }
    }

    pub(crate) fn expr(&mut self, kind: ExprKind, ty: TyId) -> ExprId {
        self.f.expr(kind, ty)
    }

    pub(crate) fn nop(&mut self) -> ExprId {
        self.f.expr(ExprKind::Nop, TY_VOID)
    }

    pub(crate) fn err_expr(&mut self) -> ExprId {
        self.f.expr(ExprKind::Error, TY_VOID)
    }

    pub(crate) fn ir_ty(vm: &ScriptVm, output: &mut ShaderOutput, pod: ScriptPodType) -> TyId {
        output.ir.pod_ty(&vm.bx.heap, pod)
    }

    /// The expression a scope variable reads.
    pub(crate) fn var_expr(&mut self, vm: &ScriptVm, output: &mut ShaderOutput, var: IrVar) -> ExprId {
        match var {
            IrVar::Param(i, pod) => {
                let ty = Self::ir_ty(vm, output, pod);
                let p = self.expr(ExprKind::Param(i), ty);
                if self.f.params.get(i as usize).map(|p| p.inout).unwrap_or(false) {
                    self.expr(ExprKind::Deref(p), ty)
                } else {
                    p
                }
            }
            IrVar::Local(l, pod, _) => {
                let ty = Self::ir_ty(vm, output, pod);
                self.expr(ExprKind::Local(l), ty)
            }
            _ => NO_EXPR,
        }
    }

    // ---- abstract numbers ----

    /// Gives an abstract literal (or a fold of them) the scalar `s`.
    pub(crate) fn concretize(&mut self, e: ExprId, s: IrScalar) {
        if e == NO_EXPR {
            return;
        }
        let ty = self.f.exprs[e as usize].ty;
        if ty != TY_AINT && ty != TY_AFLOAT {
            return;
        }
        let lit = match self.f.exprs[e as usize].kind {
            ExprKind::Lit(l) => l,
            _ => return,
        };
        let v = match lit {
            IrLit::AbstractInt(i) => i as f64,
            IrLit::AbstractFloat(f) => f,
            _ => return,
        };
        let int = match lit {
            IrLit::AbstractInt(i) => i,
            IrLit::AbstractFloat(f) => f as i64,
            _ => 0,
        };
        let (nl, nt) = match s {
            IrScalar::F32 => (IrLit::F32(v as f32), TY_F32),
            IrScalar::F16 => (IrLit::F16(v as f32), TY_F16),
            IrScalar::I32 => (IrLit::I32(int as i32), TY_I32),
            IrScalar::U32 => (IrLit::U32(int as u32), TY_U32),
            IrScalar::Bool => (IrLit::Bool(v != 0.0), TY_BOOL),
        };
        self.f.exprs[e as usize] = Expr { kind: ExprKind::Lit(nl), ty: nt };
    }

    /// Abstract to its default (i32 / f32).
    pub(crate) fn concretize_default(&mut self, e: ExprId) {
        if e == NO_EXPR {
            return;
        }
        let ty = self.f.exprs[e as usize].ty;
        if ty == TY_AINT {
            self.concretize(e, IrScalar::I32);
        } else if ty == TY_AFLOAT {
            self.concretize(e, IrScalar::F32);
        }
    }

    /// Makes `e` (of stack type `ty`) a value of pod type `to`: an abstract
    /// literal takes `to`'s scalar (splatted for a vector), a concrete value
    /// of another numeric type of the same shape is converted.
    pub(crate) fn coerce(&mut self, vm: &ScriptVm, output: &mut ShaderOutput, e: ExprId, to: ScriptPodType) -> ExprId {
        if e == NO_EXPR {
            return e;
        }
        let to_ty = Self::ir_ty(vm, output, to);
        let from_ty = self.f.exprs[e as usize].ty;
        if from_ty == to_ty {
            return e;
        }
        if from_ty == TY_AINT || from_ty == TY_AFLOAT {
            if let Some(s) = output.ir.scalar_of(to_ty) {
                self.concretize(e, s);
                if output.ir.lanes(to_ty) > 1 {
                    return self.expr(ExprKind::Construct(vec![e]), to_ty);
                }
            }
            return e;
        }
        let (fl, tl) = (output.ir.lanes(from_ty), output.ir.lanes(to_ty));
        if fl > 0 && fl == tl {
            return self.expr(ExprKind::Convert(e), to_ty);
        }
        e
    }

    // ---- compile ----

    pub fn compile_fn(&mut self, vm: &mut ScriptVm, output: &mut ShaderOutput, fnip: ScriptIp) -> ScriptPodType {
        self.mes.push(IrMe::FnBody {
            ret: None,
            escaped: false,
            stack_depth: self.stack.types.len(),
        });
        self.trap.ip = fnip;

        let fn_end_index = {
            let bodies = vm.bx.code.bodies.borrow();
            let body = &bodies[self.trap.ip.body as usize];
            let fn_body_opcode = body.parser.opcodes[(fnip.index - 1) as usize];
            if let Some((_opcode, args)) = fn_body_opcode.as_opcode() {
                (fnip.index - 1) + args.to_u32()
            } else {
                body.parser.opcodes.len() as u32
            }
        };

        while self.trap.ip.index < fn_end_index {
            let opcode = {
                let bodies = vm.bx.code.bodies.borrow();
                let body = &bodies[self.trap.ip.body as usize];
                body.parser.opcodes[self.trap.ip.index as usize]
            };

            if self.skip_next_pop_to_me {
                let next_is_pop_to_me = matches!(opcode.as_opcode(), Some((Opcode::POP_TO_ME, _)));
                if !next_is_pop_to_me {
                    self.skip_next_pop_to_me = false;
                }
            }

            if self.is_unreachable() {
                if let Some((op, args)) = opcode.as_opcode() {
                    match op {
                        Opcode::IF_TEST => self.handle_if_test_unreachable(args),
                        Opcode::IF_ELSE => {
                            if self.is_parent_scope_unreachable() {
                                self.handle_if_else_unreachable(args);
                            } else {
                                self.handle_if_else(vm, output, args);
                            }
                        }
                        _ => {}
                    }
                }
                self.trap.goto_next();
                self.handle_if_else_phi_unreachable();
            } else {
                self.handle_logic_phi(vm, output);
                if let Some((opcode, args)) = opcode.as_opcode() {
                    self.opcode(vm, output, opcode, args);
                    self.trap.goto_next();
                    self.handle_logic_phi(vm, output);
                    self.handle_if_else_phi(vm, output);
                } else {
                    let lifts = output.const_table || output.live_literals.is_some();
                    if !(lifts && self.try_push_table_const(vm, output, opcode)) {
                        self.push_immediate(vm, output, opcode);
                    }
                    self.trap.goto_next();
                    self.handle_logic_phi(vm, output);
                    self.handle_if_else_phi(vm, output);
                }
            }
            if let Some(err) = self.trap.err_pop_front() {
                let at = err
                    .value
                    .as_err()
                    .and_then(|ptr| vm.bx.code.ip_to_loc(ptr.ip))
                    .map(|loc| format!(" at {}:{}:{}", loc.file, loc.line, loc.col))
                    .unwrap_or_default();
                output.push_error(format!("{} ({}:{}){}", err.message, err.origin_file, err.origin_line, at));
                if let Some(ptr) = err.value.as_err() {
                    if let Some(loc2) = vm.bx.code.ip_to_loc(ptr.ip) {
                        log_with_level(
                            &loc2.file,
                            loc2.line,
                            loc2.col,
                            loc2.line,
                            loc2.col,
                            format!("{} ({}:{})", err.message, err.origin_file, err.origin_line),
                            LogLevel::Error,
                        );
                    }
                }
            }
            self.trap.take_on();
        }
        let cost = self.static_cost();
        if cost > crate::shader_control::SHADER_ITERATION_BUDGET {
            output.push_error(format!(
                "shader loops too costly: literal-bounded loops need {} passes per invocation, over the budget of {} (reduce loop bounds or nesting)",
                cost,
                crate::shader_control::SHADER_ITERATION_BUDGET
            ));
        }
        // Expressions grow with inlined call graphs the way emitted text did.
        output.emitted_bytes += self.f.exprs.len() * 16;
        if output.emitted_bytes > crate::shader_output::MAX_EMITTED_BYTES && !output.size_exceeded {
            output.size_exceeded = true;
            output.push_error(format!(
                "shader too large: emitted source exceeded {} bytes (deeply nested or heavily branching function calls inline exponentially)",
                crate::shader_output::MAX_EMITTED_BYTES
            ));
        }
        let value = self.mes.pop();
        if let Some(IrMe::FnBody { ret, .. }) = value {
            return ret.unwrap_or(vm.bx.code.builtins.pod.pod_void);
        }
        output.push_error("shader function did not close (after the errors above): an open expression".to_string());
        vm.bx.code.builtins.pod.pod_void
    }

    /// Pops a value, resolving a bare name through the shader scope, then the
    /// script scope (scope uniforms, uniform buffers, textures, objects).
    pub(crate) fn pop_resolved(&mut self, vm: &mut ScriptVm, output: &mut ShaderOutput) -> (ShaderType, ExprId) {
        let (ty, e) = self.pop();
        match ty {
            ShaderType::Id(id) => self.resolve_id(vm, output, id),
            _ => (ty, e),
        }
    }

    pub(crate) fn resolve_id(&mut self, vm: &mut ScriptVm, output: &mut ShaderOutput, id: LiveId) -> (ShaderType, ExprId) {
        if let Some(var) = self.scope.find(id) {
            return match var {
                IrVar::IoSelf(obj) => (ShaderType::IoSelf(obj), NO_EXPR),
                IrVar::ScopeObject(obj) => (ShaderType::ScopeObject(obj), NO_EXPR),
                _ => {
                    let e = self.var_expr(vm, output, var);
                    (ShaderType::Pod(var.pod()), e)
                }
            };
        }

        let value = vm.bx.heap.scope_value(self.script_scope, id.into(), self.trap.pass());
        if !value.is_nil() && self.trap.err_is_empty() {
            if let Some(value_obj) = value.as_object() {
                if let Some(io_type) = vm.bx.heap.as_shader_io(value_obj) {
                    if io_type == SHADER_IO_UNIFORM_BUFFER {
                        let proto_value = vm.bx.heap.proto(value_obj);
                        let pod_ty = vm.bx.heap.pod_type(proto_value).or_else(|| proto_value.as_pod_type());
                        if let Some(pod_ty) = pod_ty {
                            return (ShaderType::ScopeUniformBuffer { obj: value_obj, pod_ty }, NO_EXPR);
                        }
                    }
                    if let Some(tex_type) = texture_type_of(io_type) {
                        let shader_name = self.register_scope_texture(output, id, value_obj, tex_type);
                        let e = self.texture_expr(output, shader_name, tex_type);
                        return (ShaderType::ScopeTexture { obj: value_obj, tex_type, shader_name }, e);
                    }
                    script_err_shader!(self.trap, "opcode not supported");
                    return (ShaderType::Error(NIL), NO_EXPR);
                }
                return (ShaderType::ScopeObject(value_obj), NO_EXPR);
            }

            if let Some(c) = shader_math_const_value(id) {
                let e = self.expr(ExprKind::Lit(IrLit::F32(c as f32)), TY_F32);
                return (ShaderType::Pod(vm.bx.code.builtins.pod.pod_f32), e);
            }

            if let Some(pod_ty) = get_scope_value_pod_type(vm, value) {
                let source = self.script_scope;
                let shader_name = self.register_scope_uniform(vm, output, source, id, pod_ty);
                let e = self.scope_uniform_expr(vm, output, shader_name, pod_ty);
                return (ShaderType::Pod(pod_ty), e);
            }
        }

        self.trap.err_take();
        script_err_not_found!(
            self.trap,
            "shader variable {:?} not found{}",
            id,
            suggest_from_live_ids(id, &self.scope.all_names())
        );
        (ShaderType::Error(NIL), NO_EXPR)
    }

    /// A stack value with any bare name resolved, keeping abstract types
    /// abstract (call and constructor arguments).
    pub(crate) fn resolve_value(&mut self, vm: &mut ScriptVm, output: &mut ShaderOutput, ty: ShaderType, e: ExprId) -> (ShaderType, ExprId) {
        match ty {
            ShaderType::Id(id) => match self.scope.find(id) {
                Some(var) => match var {
                    IrVar::IoSelf(obj) => (ShaderType::IoSelf(obj), NO_EXPR),
                    IrVar::ScopeObject(obj) => (ShaderType::ScopeObject(obj), NO_EXPR),
                    _ => {
                        let e = if e != NO_EXPR { e } else { self.var_expr(vm, output, var) };
                        (ShaderType::Pod(var.pod()), e)
                    }
                },
                None => self.resolve_id(vm, output, id),
            },
            _ => (ty, e),
        }
    }

    pub(crate) fn register_scope_uniform(
        &mut self,
        vm: &mut ScriptVm,
        output: &mut ShaderOutput,
        source: ScriptObject,
        key: LiveId,
        pod_ty: ScriptPodType,
    ) -> LiveId {
        for su in &output.scope_uniforms {
            if su.source_obj == source && su.key == key {
                return su.shader_name;
            }
        }
        let shader_name = generate_scope_uniform_name(output, key, source);
        output.scope_uniforms.push(ScopeUniformSource {
            source_obj: source,
            key,
            shader_name,
            ty: pod_ty,
            table_const: None,
        });
        let mut exists = false;
        for io in &output.io {
            if io.name == shader_name && matches!(io.kind, ShaderIoKind::ScopeUniform) {
                exists = true;
            }
        }
        if !exists {
            vm.bx.heap.pod_type_name_if_not_set(pod_ty, shader_name);
            output.io.push(ShaderIo {
                kind: ShaderIoKind::ScopeUniform,
                name: shader_name,
                ty: pod_ty,
                buffer_index: None,
            });
        }
        shader_name
    }

    pub(crate) fn scope_uniform_expr(&mut self, vm: &ScriptVm, output: &mut ShaderOutput, shader_name: LiveId, pod_ty: ScriptPodType) -> ExprId {
        let ty = Self::ir_ty(vm, output, pod_ty);
        let g = output.ir.global(IrGlobalKind::Io(IrIo::ScopeUniform), shader_name, ty);
        self.expr(ExprKind::Global(g), ty)
    }

    pub(crate) fn register_scope_texture(
        &mut self,
        output: &mut ShaderOutput,
        base: LiveId,
        obj: ScriptObject,
        tex_type: TextureType,
    ) -> LiveId {
        for st in &output.scope_textures {
            if st.obj == obj {
                return st.shader_name;
            }
        }
        let shader_name = generate_scope_texture_name(output, base, obj);
        output.scope_textures.push(ScopeTextureSource { obj, tex_type, shader_name });
        let mut exists = false;
        for io in &output.io {
            if io.name == shader_name && matches!(io.kind, ShaderIoKind::Texture(_)) {
                exists = true;
            }
        }
        if !exists {
            output.io.push(ShaderIo {
                kind: ShaderIoKind::Texture(tex_type),
                name: shader_name,
                ty: ScriptPodType::VOID,
                buffer_index: None,
            });
        }
        shader_name
    }

    pub(crate) fn texture_expr(&mut self, output: &mut ShaderOutput, name: LiveId, tex_type: TextureType) -> ExprId {
        let ty = output.ir.ty(IrTy::Texture(tex_type));
        let g = output.ir.global(IrGlobalKind::Io(IrIo::Texture(tex_type)), name, ty);
        self.expr(ExprKind::Global(g), ty)
    }

    // ---- literals ----

    fn push_immediate(&mut self, vm: &mut ScriptVm, output: &mut ShaderOutput, value: ScriptValue) {
        let builtins = &vm.bx.code.builtins.pod;
        if let Some(v) = value.as_f64() {
            let e = self.expr(ExprKind::Lit(IrLit::AbstractFloat(v)), TY_AFLOAT);
            return self.push(ShaderType::AbstractFloat, e);
        }
        if let Some(v) = value.as_u40() {
            let e = self.expr(ExprKind::Lit(IrLit::AbstractInt(v as i64)), TY_AINT);
            return self.push(ShaderType::AbstractInt, e);
        }
        if let Some(id) = value.as_id() {
            let e = match self.scope.find(id) {
                Some(var @ (IrVar::Param(..) | IrVar::Local(..))) => self.var_expr(vm, output, var),
                _ => NO_EXPR,
            };
            return self.push(ShaderType::Id(id), e);
        }
        if let Some(v) = value.as_f32() {
            let e = self.expr(ExprKind::Lit(IrLit::F32(v)), TY_F32);
            return self.push(ShaderType::Pod(builtins.pod_f32), e);
        }
        if let Some(v) = value.as_f16() {
            let e = self.expr(ExprKind::Lit(IrLit::F16(v as f32)), TY_F16);
            return self.push(ShaderType::Pod(builtins.pod_f16), e);
        }
        if let Some(v) = value.as_u32() {
            let e = self.expr(ExprKind::Lit(IrLit::U32(v)), TY_U32);
            return self.push(ShaderType::Pod(builtins.pod_u32), e);
        }
        if let Some(v) = value.as_i32() {
            let e = self.expr(ExprKind::Lit(IrLit::I32(v)), TY_I32);
            return self.push(ShaderType::Pod(builtins.pod_i32), e);
        }
        if let Some(v) = value.as_bool() {
            let e = self.expr(ExprKind::Lit(IrLit::Bool(v)), TY_BOOL);
            return self.push(ShaderType::Pod(builtins.pod_bool), e);
        }
        if let Some(v) = value.as_color() {
            let pod_vec4f = builtins.pod_vec4f;
            let v = Vec4f::from_u32(v);
            let mut parts = Vec::new();
            for c in [v.x, v.y, v.z, v.w] {
                parts.push(self.expr(ExprKind::Lit(IrLit::F32(c)), TY_F32));
            }
            let ty = output.ir.vec_ty(4, IrScalar::F32);
            let e = self.expr(ExprKind::Construct(parts), ty);
            return self.push(ShaderType::Pod(pod_vec4f), e);
        }
        script_err_shader!(self.trap, "no matching shader type");
    }

    /// Const-table / live-literal mode (the text compiler's
    /// `try_push_table_const`): a lifted literal reads its scope-uniform
    /// slot.
    fn try_push_table_const(&mut self, vm: &ScriptVm, output: &mut ShaderOutput, value: ScriptValue) -> bool {
        let ip = self.trap.ip;
        let is_int = value.as_u40().is_some();
        let color = value.as_color();
        let float = value.as_f64();
        if float.is_none() && color.is_none() {
            if is_int && output.const_table {
                warn_annotated_int(vm, ip);
            }
            if is_int && output.live_literals.is_some() {
                if let Some(site) = ip_token(vm, ip).and_then(|tok| live_site(vm, output, ip.body, tok)) {
                    output.folded_sites.push(site);
                }
            }
            return false;
        }
        let Some(tok) = ip_token(vm, ip) else {
            return false;
        };
        let site = live_site(vm, output, ip.body, tok);
        if output.const_table {
            if let Some(v) = float {
                let annotated = {
                    let bodies = vm.bx.code.bodies.borrow();
                    bodies.get(ip.body as usize).and_then(|body| crate::docs::value_name_at(&body.tokenizer, tok))
                };
                if let Some((doc, negated)) = annotated {
                    let v = if negated { -v } else { v };
                    self.skip_next_neg = negated;
                    let e = self.register_table_const(vm, output, doc, v, None, ip, site);
                    if let Some(tc) = output.table_consts.last_mut() {
                        tc.negated = negated;
                    }
                    self.push(ShaderType::Pod(vm.bx.code.builtins.pod.pod_f32), e);
                    return true;
                }
            }
        }
        if site.is_none() {
            return false;
        }
        let pods = &vm.bx.code.builtins.pod;
        let (ty, rgba) = match color {
            Some(c) => {
                let c = Vec4f::from_u32(c);
                (pods.pod_vec4f, Some([c.x, c.y, c.z, c.w]))
            }
            None => (pods.pod_f32, None),
        };
        let e = self.register_table_const(vm, output, String::new(), float.unwrap_or(0.0), rgba, ip, site);
        self.push(ShaderType::Pod(ty), e);
        true
    }

    /// The literal operand packed into an arithmetic or comparison opcode.
    pub(crate) fn packed_operand(&mut self, vm: &ScriptVm, output: &mut ShaderOutput, opargs: OpcodeArgs) -> (ShaderType, ExprId) {
        if output.const_table || output.live_literals.is_some() {
            let ip = self.trap.ip;
            let pod_f32 = vm.bx.code.builtins.pod.pod_f32;
            let found = {
                let bodies = vm.bx.code.bodies.borrow();
                bodies.get(ip.body as usize).and_then(|body| {
                    let op_tok = body.parser.source_map.get(ip.index as usize).copied().flatten()?;
                    let lit_tok = op_tok + 1;
                    let doc = if output.const_table {
                        crate::docs::value_name_at(&body.tokenizer, lit_tok).map(|(doc, _)| doc)
                    } else {
                        None
                    };
                    Some((lit_tok, doc, crate::docs::float_literal_at(&body.tokenizer, lit_tok)))
                })
            };
            if let Some((lit_tok, doc, v)) = found {
                let site = match v {
                    Some(_) => live_site(vm, output, ip.body, lit_tok),
                    None => None,
                };
                match (doc, v) {
                    (Some(doc), Some(v)) => {
                        let e = self.register_table_const(vm, output, doc, v, None, ip, site);
                        return (ShaderType::Pod(pod_f32), e);
                    }
                    (Some(_), None) => warn_annotated_int_at(vm, ip),
                    (None, None) => {
                        if let Some(site) = live_site(vm, output, ip.body, lit_tok) {
                            output.folded_sites.push(site);
                        }
                    }
                    (None, Some(v)) if site.is_some() => {
                        let e = self.register_table_const(vm, output, String::new(), v, None, ip, site);
                        return (ShaderType::Pod(pod_f32), e);
                    }
                    _ => {}
                }
            }
        }
        let e = self.expr(ExprKind::Lit(IrLit::AbstractInt(opargs.to_u32() as i64)), TY_AINT);
        (ShaderType::AbstractInt, e)
    }

    fn register_table_const(
        &mut self,
        vm: &ScriptVm,
        output: &mut ShaderOutput,
        doc: String,
        v: f64,
        color: Option<[f32; 4]>,
        ip: ScriptIp,
        site: Option<crate::literal::LiteralSite>,
    ) -> ExprId {
        let index = output.table_consts.len();
        let mut n = index;
        let shader_name = loop {
            let text = format!("ct{}", n);
            let name = LiveId::from_str_with_lut(&text).unwrap_or_else(|_| LiveId::from_str(&text));
            let mut used = false;
            for io in &output.io {
                if io.name == name {
                    used = true;
                }
            }
            if !used {
                break name;
            }
            n += 1000;
        };
        let ty = match color {
            Some(_) => vm.bx.code.builtins.pod.pod_vec4f,
            None => vm.bx.code.builtins.pod.pod_f32,
        };
        output.table_consts.push(ShaderTableConst {
            shader_name,
            doc,
            value: v,
            ip,
            site,
            color,
            negated: false,
        });
        output.scope_uniforms.push(ScopeUniformSource {
            source_obj: ScriptObject::ZERO,
            key: shader_name,
            shader_name,
            ty,
            table_const: Some(index),
        });
        output.io.push(ShaderIo {
            kind: ShaderIoKind::ScopeUniform,
            name: shader_name,
            ty,
            buffer_index: None,
        });
        self.scope_uniform_expr(vm, output, shader_name, ty)
    }

    pub(crate) fn ensure_struct_name(&self, vm: &mut ScriptVm, output: &mut ShaderOutput, pod_ty: ScriptPodType, used_name: LiveId) -> LiveId {
        if let ScriptPodTy::Struct { .. } = vm.bx.heap.pod_type_ref(pod_ty).ty {
            output.structs.insert(pod_ty);
        }
        if let Some(name) = vm.bx.heap.pod_type_name(pod_ty) {
            let alias_ok = (name == id!(f32) && used_name == id!(float))
                || (name == id!(u32) && used_name == id!(uint))
                || (name == id!(i32) && used_name == id!(int));
            if name != used_name
                && !alias_ok
                && used_name != id!(self)
                && used_name != id!(vec2)
                && used_name != id!(vec3)
                && used_name != id!(vec4)
            {
                script_err_inconsistent!(self.trap, "struct name not consistent");
            }
            return name;
        }
        vm.bx.heap.pod_type_name_set(pod_ty, used_name);
        used_name
    }

    fn opcode(&mut self, vm: &mut ScriptVm, output: &mut ShaderOutput, opcode: Opcode, opargs: OpcodeArgs) {
        match opcode {
            Opcode::NOT => self.handle_not(vm, output),
            Opcode::NEG if self.skip_next_neg => {
                self.skip_next_neg = false;
            }
            Opcode::NEG => self.handle_neg(vm, output),
            Opcode::MUL => self.handle_arithmetic(vm, output, opargs, BinOp::Mul, false),
            Opcode::DIV => self.handle_arithmetic(vm, output, opargs, BinOp::Div, false),
            Opcode::MOD => self.handle_arithmetic(vm, output, opargs, BinOp::Rem, false),
            Opcode::ADD => self.handle_arithmetic(vm, output, opargs, BinOp::Add, false),
            Opcode::SUB => self.handle_arithmetic(vm, output, opargs, BinOp::Sub, false),
            Opcode::SHL => self.handle_arithmetic(vm, output, opargs, BinOp::Shl, true),
            Opcode::SHR => self.handle_arithmetic(vm, output, opargs, BinOp::Shr, true),
            Opcode::AND => self.handle_arithmetic(vm, output, opargs, BinOp::BitAnd, true),
            Opcode::OR => self.handle_arithmetic(vm, output, opargs, BinOp::BitOr, true),
            Opcode::XOR => self.handle_arithmetic(vm, output, opargs, BinOp::BitXor, true),

            Opcode::SLOTS_FRAME | Opcode::ARGS_TO_SLOTS => {}
            Opcode::NIL_ARM => {}
            Opcode::PUSH_SLOT => {
                let name = {
                    let bodies = vm.bx.code.bodies.borrow();
                    let body = &bodies[self.trap.ip.body as usize];
                    let ip = self.trap.ip.index;
                    let mut found = None;
                    for (frame_ip, names) in body.parser.slot_frames.iter().rev() {
                        if *frame_ip < ip {
                            found = names.get(opargs.to_u32() as usize).copied();
                            break;
                        }
                    }
                    found
                };
                if let Some(name) = name {
                    self.push_immediate(vm, output, ScriptValue::from_id(name));
                } else {
                    script_err_shader!(self.trap, "PUSH_SLOT: no slot name table for shader");
                }
            }
            Opcode::LET_SLOT => self.handle_let_dyn(vm, output, OpcodeArgs::NONE, false),
            Opcode::VAR_SLOT => self.handle_let_dyn(vm, output, OpcodeArgs::NONE, true),
            Opcode::STORE_SLOT => self.handle_assign(vm, output),
            Opcode::ASSIGN_SLOT_ADD => self.handle_arithmetic_assign(vm, output, OpcodeArgs::NONE, BinOp::Add, false),
            Opcode::ASSIGN_SLOT_SUB => self.handle_arithmetic_assign(vm, output, OpcodeArgs::NONE, BinOp::Sub, false),
            Opcode::ASSIGN_SLOT_MUL => self.handle_arithmetic_assign(vm, output, OpcodeArgs::NONE, BinOp::Mul, false),
            Opcode::ASSIGN_SLOT_DIV => self.handle_arithmetic_assign(vm, output, OpcodeArgs::NONE, BinOp::Div, false),
            Opcode::ASSIGN_SLOT_MOD => self.handle_arithmetic_assign(vm, output, OpcodeArgs::NONE, BinOp::Rem, false),

            Opcode::ASSIGN => self.handle_assign(vm, output),
            Opcode::ASSIGN_ADD => self.handle_arithmetic_assign(vm, output, opargs, BinOp::Add, false),
            Opcode::ASSIGN_SUB => self.handle_arithmetic_assign(vm, output, opargs, BinOp::Sub, false),
            Opcode::ASSIGN_MUL => self.handle_arithmetic_assign(vm, output, opargs, BinOp::Mul, false),
            Opcode::ASSIGN_DIV => self.handle_arithmetic_assign(vm, output, opargs, BinOp::Div, false),
            Opcode::ASSIGN_MOD => self.handle_arithmetic_assign(vm, output, opargs, BinOp::Rem, false),
            Opcode::ASSIGN_AND => self.handle_arithmetic_assign(vm, output, opargs, BinOp::BitAnd, true),
            Opcode::ASSIGN_OR => self.handle_arithmetic_assign(vm, output, opargs, BinOp::BitOr, true),
            Opcode::ASSIGN_XOR => self.handle_arithmetic_assign(vm, output, opargs, BinOp::BitXor, true),
            Opcode::ASSIGN_SHL => self.handle_arithmetic_assign(vm, output, opargs, BinOp::Shl, true),
            Opcode::ASSIGN_SHR => self.handle_arithmetic_assign(vm, output, opargs, BinOp::Shr, true),
            Opcode::ASSIGN_IFNIL => {
                script_err_not_impl!(self.trap, "ASSIGN_IFNIL: null-coalescing assignment `x ??= default` not supported in shaders");
            }
            Opcode::ASSIGN_FIELD => self.handle_assign_field(vm, output),
            Opcode::ASSIGN_FIELD_ADD => self.handle_arithmetic_field_assign(vm, output, opargs, BinOp::Add, false),
            Opcode::ASSIGN_FIELD_SUB => self.handle_arithmetic_field_assign(vm, output, opargs, BinOp::Sub, false),
            Opcode::ASSIGN_FIELD_MUL => self.handle_arithmetic_field_assign(vm, output, opargs, BinOp::Mul, false),
            Opcode::ASSIGN_FIELD_DIV => self.handle_arithmetic_field_assign(vm, output, opargs, BinOp::Div, false),
            Opcode::ASSIGN_FIELD_MOD => self.handle_arithmetic_field_assign(vm, output, opargs, BinOp::Rem, false),
            Opcode::ASSIGN_FIELD_AND => self.handle_arithmetic_field_assign(vm, output, opargs, BinOp::BitAnd, true),
            Opcode::ASSIGN_FIELD_OR => self.handle_arithmetic_field_assign(vm, output, opargs, BinOp::BitOr, true),
            Opcode::ASSIGN_FIELD_XOR => self.handle_arithmetic_field_assign(vm, output, opargs, BinOp::BitXor, true),
            Opcode::ASSIGN_FIELD_SHL => self.handle_arithmetic_field_assign(vm, output, opargs, BinOp::Shl, true),
            Opcode::ASSIGN_FIELD_SHR => self.handle_arithmetic_field_assign(vm, output, opargs, BinOp::Shr, true),
            Opcode::ASSIGN_FIELD_IFNIL => {
                script_err_not_impl!(self.trap, "ASSIGN_FIELD_IFNIL: null-coalescing field assignment `obj.x ??= default` not supported in shaders");
            }
            Opcode::ASSIGN_INDEX => self.handle_assign_index(vm, output),
            Opcode::ASSIGN_INDEX_ADD => self.handle_arithmetic_index_assign(vm, output, opargs, BinOp::Add, false),
            Opcode::ASSIGN_INDEX_SUB => self.handle_arithmetic_index_assign(vm, output, opargs, BinOp::Sub, false),
            Opcode::ASSIGN_INDEX_MUL => self.handle_arithmetic_index_assign(vm, output, opargs, BinOp::Mul, false),
            Opcode::ASSIGN_INDEX_DIV => self.handle_arithmetic_index_assign(vm, output, opargs, BinOp::Div, false),
            Opcode::ASSIGN_INDEX_MOD => self.handle_arithmetic_index_assign(vm, output, opargs, BinOp::Rem, false),
            Opcode::ASSIGN_INDEX_AND => self.handle_arithmetic_index_assign(vm, output, opargs, BinOp::BitAnd, true),
            Opcode::ASSIGN_INDEX_OR => self.handle_arithmetic_index_assign(vm, output, opargs, BinOp::BitOr, true),
            Opcode::ASSIGN_INDEX_XOR => self.handle_arithmetic_index_assign(vm, output, opargs, BinOp::BitXor, true),
            Opcode::ASSIGN_INDEX_SHL => self.handle_arithmetic_index_assign(vm, output, opargs, BinOp::Shl, true),
            Opcode::ASSIGN_INDEX_SHR => self.handle_arithmetic_index_assign(vm, output, opargs, BinOp::Shr, true),
            Opcode::ASSIGN_INDEX_IFNIL => {
                script_err_not_impl!(self.trap, "ASSIGN_INDEX_IFNIL: null-coalescing index assignment `arr[i] ??= default` not supported in shaders");
            }
            Opcode::ASSIGN_ME => self.handle_assign_me(vm, output),
            Opcode::ASSIGN_ME_VEC => {
                script_err_shader!(self.trap, "ASSIGN_ME_VEC: vec assignment `:=` not supported in shaders");
            }
            Opcode::ASSIGN_ME_BEFORE | Opcode::ASSIGN_ME_AFTER => {
                script_err_shader!(self.trap, "ASSIGN_ME_BEFORE/AFTER: `++x` / `x++` not supported in shaders, use `x += 1`");
            }
            Opcode::ASSIGN_ME_BEGIN => {
                script_err_shader!(self.trap, "ASSIGN_ME_BEGIN: compound assignment not supported in shaders");
            }
            Opcode::CONCAT => {
                script_err_shader!(self.trap, "CONCAT: string concatenation `a + b` not supported in shaders");
            }
            Opcode::EQ => self.handle_eq(vm, output, opargs, BinOp::Eq),
            Opcode::NEQ => self.handle_eq(vm, output, opargs, BinOp::Ne),
            Opcode::LT => self.handle_eq(vm, output, opargs, BinOp::Lt),
            Opcode::GT => self.handle_eq(vm, output, opargs, BinOp::Gt),
            Opcode::LEQ => self.handle_eq(vm, output, opargs, BinOp::Le),
            Opcode::GEQ => self.handle_eq(vm, output, opargs, BinOp::Ge),
            Opcode::LOGIC_AND_TEST => self.handle_logic_test(vm, output, opargs, BinOp::LogicAnd),
            Opcode::LOGIC_OR_TEST => self.handle_logic_test(vm, output, opargs, BinOp::LogicOr),
            Opcode::NIL_OR_TEST => {
                script_err_shader!(self.trap, "NIL_OR_TEST: null-coalescing `a |? b` not supported in shaders");
            }
            Opcode::SHALLOW_EQ => {
                script_err_shader!(self.trap, "SHALLOW_EQ: shallow equality `===` not supported in shaders");
            }
            Opcode::SHALLOW_NEQ => {
                script_err_shader!(self.trap, "SHALLOW_NEQ: shallow inequality `!==` not supported in shaders");
            }
            Opcode::BEGIN_PROTO | Opcode::END_PROTO => {
                script_err_shader!(self.trap, "BEGIN_PROTO: object literal `{{...}}` not supported in shaders");
            }
            Opcode::PROTO_INHERIT_READ | Opcode::PROTO_INHERIT_WRITE => {
                script_err_shader!(self.trap, "PROTO_INHERIT: prototype inheritance not supported in shaders");
            }
            Opcode::SCOPE_INHERIT_READ | Opcode::SCOPE_INHERIT_WRITE => {
                script_err_shader!(self.trap, "SCOPE_INHERIT: scope inheritance not supported in shaders");
            }
            Opcode::FIELD_INHERIT_READ | Opcode::FIELD_INHERIT_WRITE => {
                script_err_shader!(self.trap, "FIELD_INHERIT: field inheritance not supported in shaders");
            }
            Opcode::INDEX_INHERIT_READ | Opcode::INDEX_INHERIT_WRITE => {
                script_err_shader!(self.trap, "INDEX_INHERIT: index inheritance not supported in shaders");
            }
            Opcode::BEGIN_BARE | Opcode::END_BARE => {
                script_err_shader!(self.trap, "BEGIN_BARE: bare object `{{..}}` not supported in shaders");
            }
            Opcode::BEGIN_ARRAY | Opcode::END_ARRAY => {
                script_err_shader!(self.trap, "BEGIN_ARRAY: dynamic array `[...]` not supported in shaders, use fixed-size arrays");
            }
            Opcode::CALL_ARGS => self.handle_call_args(vm, output, opargs),
            Opcode::CALL_EXEC | Opcode::METHOD_CALL_EXEC => self.handle_call_exec(vm, output),
            Opcode::METHOD_CALL_ARGS => self.handle_method_call_args(vm, output, opargs),
            Opcode::FN_ARGS | Opcode::FN_LET_ARGS | Opcode::FN_ARG_DYN | Opcode::FN_ARG_TYPED | Opcode::FN_BODY_DYN | Opcode::FN_BODY_TYPED => {
                script_err_not_impl!(self.trap, "nested function definitions are not supported in shaders");
            }
            Opcode::RETURN => self.handle_return(vm, output, opargs),
            Opcode::RETURN_IF_ERR => {
                script_err_shader!(self.trap, "RETURN_IF_ERR: error propagation `?` not supported in shaders");
            }
            Opcode::IF_TEST => self.handle_if_test(vm, output, opargs),
            Opcode::IF_ELSE => self.handle_if_else(vm, output, opargs),
            Opcode::USE => {
                script_err_shader!(self.trap, "USE: `use` imports not supported in shaders");
            }
            Opcode::FIELD | Opcode::PROTO_FIELD => self.handle_field(vm, output),
            Opcode::FIELD_NIL => {
                script_err_shader!(self.trap, "FIELD_NIL: optional chaining `obj?.field` not supported in shaders");
            }
            Opcode::ME_FIELD => {
                script_err_not_impl!(self.trap, "ME_FIELD: `me.field` access not supported in shaders");
            }
            Opcode::POP_TO_ME => self.pop_to_me(vm, output),
            Opcode::ARRAY_INDEX => self.handle_array_index(vm, output),
            Opcode::LET_DYN => self.handle_let_dyn(vm, output, opargs, false),
            Opcode::LET_TYPED => {
                script_err_not_impl!(self.trap, "LET_TYPED: typed let `let x: Type = ...` not yet supported in shaders, use `let x = Type(...)`");
            }
            Opcode::VAR_DYN => self.handle_let_dyn(vm, output, opargs, true),
            Opcode::VAR_TYPED => {
                script_err_not_impl!(self.trap, "VAR_TYPED: typed var `var x: Type = ...` not yet supported in shaders, use `var x = Type(...)`");
            }
            Opcode::SEARCH_TREE => {
                script_err_shader!(self.trap, "SEARCH_TREE: tree search `#identifier` not supported in shaders");
            }
            Opcode::LOG => {}
            Opcode::ME => {
                script_err_shader!(self.trap, "ME: `me` keyword not supported in shaders");
            }
            Opcode::SCOPE => {
                script_err_shader!(self.trap, "SCOPE: `scope` keyword not supported in shaders");
            }
            Opcode::FOR_1 => self.handle_for_1(vm, output),
            Opcode::FOR_2 | Opcode::FOR_3 => {
                script_err_shader!(self.trap, "`for k, v in obj` iteration not supported in shaders, use `for i in 0..n`");
            }
            Opcode::LOOP => self.handle_loop(vm, output),
            Opcode::FOR_END => self.handle_for_end(),
            Opcode::BREAK => self.emit(Stmt::Break),
            Opcode::BREAKIFNOT => self.handle_breakifnot(vm, output),
            Opcode::CONTINUE => self.emit(Stmt::Continue),
            Opcode::RANGE => self.handle_range(vm),
            Opcode::IS => {
                script_err_shader!(self.trap, "IS: type check `x is Type` not supported in shaders");
            }
            Opcode::OK_TEST | Opcode::OK_END | Opcode::TRY_TEST | Opcode::TRY_ERR | Opcode::TRY_OK => {
                script_err_shader!(self.trap, "`ok` / `try` blocks are not supported in shaders");
            }
            opcode => {
                script_err_shader!(self.trap, "unknown opcode {:?} not supported in shaders", opcode);
                self.trap.goto_next();
            }
        }
        if opargs.is_pop_to_me() {
            self.pop_to_me(vm, output);
        }
    }

    pub(crate) fn maybe_pop_to_me(&mut self, vm: &mut ScriptVm, output: &mut ShaderOutput, opargs: OpcodeArgs) {
        if opargs.is_pop_to_me() {
            self.pop_to_me(vm, output);
        }
    }

    pub(crate) fn pop_to_me(&mut self, vm: &mut ScriptVm, output: &mut ShaderOutput) {
        if self.skip_next_pop_to_me {
            self.skip_next_pop_to_me = false;
            return;
        }
        let depth = match self.mes.last() {
            Some(IrMe::FnBody { stack_depth, .. })
            | Some(IrMe::ForLoop { stack_depth, .. })
            | Some(IrMe::LoopBody { stack_depth })
            | Some(IrMe::IfBody { stack_depth, .. }) => Some(*stack_depth),
            _ => None,
        };
        if let Some(depth) = depth {
            if self.stack.types.len() <= depth {
                return;
            }
            // A value nothing consumes: an expression statement, or an
            // if/else whose branches ended in values (its phi).
            let (_ty, e) = self.pop();
            self.discard_value(e);
            return;
        }
        let (ty, e) = self.pop();
        match self.mes.last() {
            Some(IrMe::Pod { .. }) => {
                if let Some(IrMe::Pod { args, .. }) = self.mes.last() {
                    if let Some(last) = args.last() {
                        if last.name.is_some() {
                            script_err_pod!(self.trap, "mixing named and ordered fields");
                        }
                    }
                }
                let (ty, e) = self.resolve_value(vm, output, ty, e);
                if let Some(IrMe::Pod { args, .. }) = self.mes.last_mut() {
                    args.push(IrPodArg { name: None, ty, e });
                }
            }
            Some(IrMe::ArrayConstruct { .. }) => {
                let (ty, e) = self.resolve_value(vm, output, ty, e);
                let arg_ty = match ty.make_concrete(&vm.bx.code.builtins.pod) {
                    Some(t) => t,
                    None => {
                        script_err_shader!(self.trap, "no matching shader type");
                        vm.bx.code.builtins.pod.pod_void
                    }
                };
                let mut mismatch = None;
                if let Some(IrMe::ArrayConstruct { args, elem_ty }) = self.mes.last_mut() {
                    match elem_ty {
                        Some(et) if *et != arg_ty => mismatch = Some(*et),
                        Some(_) => {}
                        None => *elem_ty = Some(arg_ty),
                    }
                    args.push((ty, e));
                }
                if let Some(et) = mismatch {
                    script_err_pod!(
                        self.trap,
                        "array element type mismatch: expected {}, got {}",
                        format_pod_type_name(&vm.bx.heap, et),
                        format_pod_type_name(&vm.bx.heap, arg_ty)
                    );
                }
            }
            Some(IrMe::TextureBuiltin { .. }) => {
                let (ty, e) = self.resolve_value(vm, output, ty, e);
                if let Some(IrMe::TextureBuiltin { args, .. }) = self.mes.last_mut() {
                    args.push((ty, e));
                }
            }
            Some(IrMe::ScriptCall { .. }) | Some(IrMe::BuiltinCall { .. }) | Some(IrMe::PodBuiltinMethod { .. }) => {
                let (ty, e) = self.resolve_value(vm, output, ty, e);
                match self.mes.last_mut() {
                    Some(IrMe::ScriptCall { args, .. })
                    | Some(IrMe::BuiltinCall { args, .. })
                    | Some(IrMe::PodBuiltinMethod { args, .. }) => args.push((ty, e)),
                    _ => {}
                }
            }
            _ => {
                script_err_shader!(self.trap, "shader: unexpected value position");
            }
        }
    }

    /// A value used as a statement: kept for its effects.
    pub(crate) fn discard_value(&mut self, e: ExprId) {
        if e == NO_EXPR {
            return;
        }
        match self.f.exprs[e as usize].kind {
            ExprKind::Nop | ExprKind::Error => {}
            ExprKind::Lit(_) | ExprKind::Local(_) | ExprKind::Param(_) | ExprKind::Global(_) => {}
            _ => {
                self.concretize_default(e);
                self.emit(Stmt::Expr(e));
            }
        }
    }
}

pub(crate) fn shader_math_const_value(id: LiveId) -> Option<f64> {
    match id {
        id!(PI) => Some(3.141592653589793),
        id!(E) => Some(2.718281828459045),
        id!(LN2) => Some(0.6931471805599453),
        id!(LN10) => Some(2.302585092994046),
        id!(LOG2E) => Some(1.4426950408889634),
        id!(LOG10E) => Some(0.4342944819032518),
        id!(SQRT1_2) => Some(0.70710678118654757),
        id!(TORAD) => Some(0.017453292519943295),
        id!(GOLDEN) => Some(1.618033988749895),
        _ => None,
    }
}

pub(crate) fn get_scope_value_pod_type(vm: &ScriptVm, value: ScriptValue) -> Option<ScriptPodType> {
    if let Some(pod_ty) = vm.bx.code.builtins.pod.value_to_exact_type(value) {
        return Some(pod_ty);
    }
    if value.is_color() {
        return Some(vm.bx.code.builtins.pod.pod_vec4f);
    }
    if let Some(pod) = value.as_pod() {
        let pod = &vm.bx.heap.pods[pod];
        return Some(pod.ty);
    }
    None
}

pub(crate) fn texture_type_of(io_type: ShaderIoType) -> Option<TextureType> {
    Some(match io_type {
        SHADER_IO_TEXTURE_1D => TextureType::Texture1d,
        SHADER_IO_TEXTURE_1D_ARRAY => TextureType::Texture1dArray,
        SHADER_IO_TEXTURE_2D => TextureType::Texture2d,
        SHADER_IO_TEXTURE_2D_ARRAY => TextureType::Texture2dArray,
        SHADER_IO_TEXTURE_3D => TextureType::Texture3d,
        SHADER_IO_TEXTURE_3D_ARRAY => TextureType::Texture3dArray,
        SHADER_IO_TEXTURE_CUBE => TextureType::TextureCube,
        SHADER_IO_TEXTURE_CUBE_ARRAY => TextureType::TextureCubeArray,
        SHADER_IO_TEXTURE_DEPTH => TextureType::TextureDepth,
        SHADER_IO_TEXTURE_DEPTH_ARRAY => TextureType::TextureDepthArray,
        SHADER_IO_TEXTURE_VIDEO => TextureType::TextureVideo,
        _ => return None,
    })
}

fn name_used(output: &ShaderOutput, name: LiveId) -> bool {
    for io in &output.io {
        if io.name == name {
            return true;
        }
    }
    for su in &output.scope_uniforms {
        if su.shader_name == name {
            return true;
        }
    }
    false
}

fn lut_id(text: &str) -> LiveId {
    LiveId::from_str_with_lut(text).unwrap_or_else(|_| LiveId::from_str(text))
}

pub(crate) fn generate_scope_uniform_name(output: &ShaderOutput, base_name: LiveId, source_obj: ScriptObject) -> LiveId {
    let base_name_str = base_name
        .as_string(|s| s.map(|s| s.to_string()))
        .unwrap_or_else(|| "scope_uni".to_string());
    let base_name = LiveId::from_str_with_lut(&base_name_str).unwrap_or_else(|_| id!(scope_uni));
    if !name_used(output, base_name) {
        return base_name;
    }
    let unique_name_str = format!("{}_obj{}", base_name_str, source_obj.index);
    let unique_name = lut_id(&unique_name_str);
    if !name_used(output, unique_name) {
        return unique_name;
    }
    for i in 1..100 {
        let new_name = lut_id(&format!("{}_obj{}_{}", base_name_str, source_obj.index, i));
        if !name_used(output, new_name) {
            return new_name;
        }
    }
    unique_name
}

pub(crate) fn generate_scope_uniform_buffer_names(output: &ShaderOutput, obj: ScriptObject) -> (LiveId, LiveId) {
    let shader_name = lut_id(&format!("scopebuf_{}", obj.index));
    let struct_name = lut_id(&format!("IoScopeUniformBuf{}", obj.index));
    let mut used = false;
    for io in &output.io {
        if io.name == shader_name {
            used = true;
        }
    }
    for sub in &output.scope_uniform_buffers {
        if sub.shader_name == shader_name {
            used = true;
        }
    }
    if !used {
        return (shader_name, struct_name);
    }
    for i in 0..100 {
        let new_shader_name = lut_id(&format!("scopebuf_{}_{}", obj.index, i));
        let new_struct_name = lut_id(&format!("IoScopeUniformBuf{}_{}", obj.index, i));
        let mut used = false;
        for io in &output.io {
            if io.name == new_shader_name {
                used = true;
            }
        }
        for sub in &output.scope_uniform_buffers {
            if sub.shader_name == new_shader_name {
                used = true;
            }
        }
        if !used {
            return (new_shader_name, new_struct_name);
        }
    }
    (shader_name, struct_name)
}

fn texture_name_used(output: &ShaderOutput, name: LiveId) -> bool {
    for io in &output.io {
        if io.name == name && matches!(io.kind, ShaderIoKind::Texture(_)) {
            return true;
        }
    }
    for st in &output.scope_textures {
        if st.shader_name == name {
            return true;
        }
    }
    false
}

pub(crate) fn generate_scope_texture_name(output: &ShaderOutput, base_name: LiveId, obj: ScriptObject) -> LiveId {
    let base_name_str = base_name
        .as_string(|s| s.map(|s| s.to_string()))
        .unwrap_or_else(|| format!("scope_tex_{}", obj.index));
    let base_name = LiveId::from_str_with_lut(&base_name_str).unwrap_or_else(|_| id!(scope_tex));
    if !texture_name_used(output, base_name) {
        return base_name;
    }
    let unique_name = lut_id(&format!("{}_obj{}", base_name_str, obj.index));
    if !texture_name_used(output, unique_name) {
        return unique_name;
    }
    for i in 1..100 {
        let new_name = lut_id(&format!("{}_obj{}_{}", base_name_str, obj.index, i));
        if !texture_name_used(output, new_name) {
            return new_name;
        }
    }
    unique_name
}

fn ip_token(vm: &ScriptVm, ip: ScriptIp) -> Option<u32> {
    let bodies = vm.bx.code.bodies.borrow();
    bodies.get(ip.body as usize)?.parser.source_map.get(ip.index as usize).copied().flatten()
}

fn live_site(vm: &ScriptVm, output: &ShaderOutput, body: u16, tok: u32) -> Option<crate::literal::LiteralSite> {
    let live = output.live_literals.as_ref()?;
    let bodies = vm.bx.code.bodies.borrow();
    let body = bodies.get(body as usize)?;
    let ScriptSource::Mod(m) = &body.source else {
        return None;
    };
    if !live.sources.contains_key(&m.file) {
        return None;
    }
    let (row, col) = body.tokenizer.token_start_row_col(tok)?;
    live.site(&m.file, row, col)
}

fn warn_annotated_int(vm: &ScriptVm, ip: ScriptIp) {
    let annotated = {
        let bodies = vm.bx.code.bodies.borrow();
        bodies.get(ip.body as usize).is_some_and(|body| {
            body.parser
                .source_map
                .get(ip.index as usize)
                .copied()
                .flatten()
                .and_then(|tok| crate::docs::value_name_at(&body.tokenizer, tok))
                .is_some()
        })
    };
    if annotated {
        warn_annotated_int_at(vm, ip);
    }
}

fn warn_annotated_int_at(vm: &ScriptVm, ip: ScriptIp) {
    if let Some(loc) = vm.bx.code.ip_to_loc(ip) {
        log_with_level(
            &loc.file,
            loc.line,
            loc.col,
            loc.line,
            loc.col,
            "shader constant table: an annotated INT literal stays folded (only float literals become hot-patchable constants) — write it as a float (`6.`) if it should be tweakable".to_string(),
            LogLevel::Warning,
        );
    }
}
