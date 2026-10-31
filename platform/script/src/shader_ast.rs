//! First source-AST slice of the Splash shader frontend.
//!
//! A complete literal-return body is typed from Splash tokens and lowered
//! directly to shared IR. Unsupported bodies use the bytecode frontend. The
//! source-map anchor identifies the function; no bytecode body is traversed.
//! Live/const-table modes remain on the frontend that owns their provenance.
use crate::{shader_ids::ShaderTypeId, shader_ir::*, shader_lower::IrFnCompiler,
    shader_output::ShaderOutput, tokenizer::ScriptToken, value::*, vm::ScriptVm};
use makepad_live_id::{id, LiveId};

/// An already typed expression; abstract source numbers take Splash's
/// return defaults (f32/i32), just as the existing return lowering does.
enum Literal {
    Scalar(IrLit),
    Color([f32; 4]),
}

struct TypedReturn {
    ty: ShaderTypeId,
    value: Literal,
}

impl TypedReturn {
    fn from_source(vm: &ScriptVm, output: &ShaderOutput, ip: ScriptIp) -> Option<Self> {
        if output.const_table || output.live_literals.is_some() { return None; }
        let bodies = vm.bx.code.bodies.borrow();
        let body = bodies.get(ip.body as usize)?;
        let token = body.parser.source_map.get(ip.index.checked_sub(1)? as usize)?.as_ref()?;
        let tokens = &body.tokenizer.tokens;
        let mut at = *token as usize;
        if !matches!(tokens.get(at)?.token, ScriptToken::OpenCurly) { return None; }
        at += 1;
        let explicit_return = matches!(tokens.get(at)?.token, ScriptToken::Identifier(name) if name == id!(return));
        if explicit_return { at += 1; }
        let pods = &vm.bx.code.builtins.pod;
        let (ty, value, immediate) = match tokens.get(at)?.token {
            ScriptToken::Color(value) => {
                let c = makepad_math::Vec4f::from_u32(value);
                (pods.pod_vec4f, Literal::Color([c.x, c.y, c.z, c.w]), ScriptValue::from_color(value))
            }
            ScriptToken::F64(value) => (pods.pod_f32, Literal::Scalar(IrLit::F32(value as f32)), ScriptValue::from_f64(value)),
            ScriptToken::F32(value) => (pods.pod_f32, Literal::Scalar(IrLit::F32(value)), ScriptValue::from_f32(value)),
            ScriptToken::U40(value) => (pods.pod_i32, Literal::Scalar(IrLit::I32(value as i32)), ScriptValue::from_u40(value)),
            ScriptToken::U32(value) => (pods.pod_u32, Literal::Scalar(IrLit::U32(value)), ScriptValue::from_u32(value)),
            ScriptToken::I32(value) => (pods.pod_i32, Literal::Scalar(IrLit::I32(value)), ScriptValue::from_i32(value)),
            ScriptToken::Identifier(name) if name == id!(true) || name == id!(false) => {
                let value = name == id!(true);
                (pods.pod_bool, Literal::Scalar(IrLit::Bool(value)), ScriptValue::from_bool(value))
            }
            _ => return None,
        };
        at += 1;
        // A semicolon after a tail expression returns void; only an explicit
        // return may have one in this subset. Extra statements never match.
        if explicit_return && matches!(tokens.get(at)?.token, ScriptToken::Separator(name) if name == id!(;)) { at += 1; }
        if !matches!(tokens.get(at)?.token, ScriptToken::CloseCurly) { return None; }
        // A live patch can change an immediate without replacing the source
        // tokens. In that case keep using the authoritative bytecode path.
        if body.parser.opcodes.get(ip.index as usize).copied()? != immediate { return None; }
        Some(Self { ty: ty.into(), value })
    }

    fn lower(self, compiler: &mut IrFnCompiler, vm: &ScriptVm, output: &mut ShaderOutput) -> ScriptPodType {
        let ty = output.compiled.ir.pod_ty(&vm.bx.heap, self.ty);
        let value = match self.value {
            Literal::Scalar(value) => compiler.expr(ExprKind::Lit(value), ty),
            Literal::Color(rgba) => {
                let parts = rgba.into_iter().map(|v| compiler.expr(ExprKind::Lit(IrLit::F32(v)), TY_F32)).collect();
                compiler.expr(ExprKind::Construct(parts), ty)
            }
        };
        compiler.emit(Stmt::Return(value));
        self.ty.into()
    }
}

impl IrFnCompiler {
    pub(crate) fn compile_source_ast(&mut self, vm: &ScriptVm, output: &mut ShaderOutput, ip: ScriptIp) -> Option<ScriptPodType> {
        let body = TypedReturn::from_source(vm, output, ip)?;
        let ret = body.lower(self, vm, output);
        output.ast_functions += 1;
        Some(ret)
    }
}
