use crate::{
    binary::Instr,
    decode::DecodeError,
    func::Func,
    func_ref::FuncRef,
    global::Global,
    ref_::{Ref, RefType},
    store::Store,
    val::Val,
};

#[derive(Clone, Debug)]
pub(crate) struct ConstExpr {
    instr: ConstInstr,
}

impl ConstExpr {
    pub(crate) fn new_ref_func(func_idx: u32) -> Self {
        Self {
            instr: ConstInstr::RefFunc(func_idx),
        }
    }

    pub(crate) fn evaluate(&self, store: &Store, context: &impl EvaluationContext) -> Val {
        match self.instr {
            ConstInstr::I32Const(val) => val.into(),
            ConstInstr::I64Const(val) => val.into(),
            ConstInstr::F32Const(val) => val.into(),
            ConstInstr::F64Const(val) => val.into(),
            ConstInstr::RefNull(ref_ty) => Ref::null(ref_ty).into(),
            ConstInstr::RefFunc(func_idx) => FuncRef::new(context.func(func_idx).unwrap()).into(),
            ConstInstr::GlobalGet(global_idx) => {
                context.global(global_idx).unwrap().get(store).into()
            }
        }
    }
}

impl ConstExpr {
    /// The engine's form of a validated constant expression. It runs the
    /// single-instruction ones; extended constant expressions and `v128`
    /// constants are refused.
    pub(crate) fn from_expr(expr: &[Instr]) -> Result<Self, DecodeError> {
        let instr = match expr {
            [instr, Instr::End] => match instr {
                Instr::I32Const(val) => ConstInstr::I32Const(*val),
                Instr::I64Const(val) => ConstInstr::I64Const(*val),
                Instr::F32Const(bits) => ConstInstr::F32Const(f32::from_bits(*bits)),
                Instr::F64Const(bits) => ConstInstr::F64Const(f64::from_bits(*bits)),
                Instr::RefNull(type_) => ConstInstr::RefNull(type_.to_ref().unwrap()),
                Instr::RefFunc(func_idx) => ConstInstr::RefFunc(*func_idx),
                Instr::GlobalGet(global_idx) => ConstInstr::GlobalGet(*global_idx),
                _ => return Err(DecodeError::new("unsupported constant expression")),
            },
            _ => return Err(DecodeError::new("unsupported constant expression")),
        };
        Ok(Self { instr })
    }
}

pub(crate) trait EvaluationContext {
    fn func(&self, idx: u32) -> Option<Func>;
    fn global(&self, idx: u32) -> Option<Global>;
}

#[derive(Clone, Copy, Debug)]
enum ConstInstr {
    I32Const(i32),
    I64Const(i64),
    F32Const(f32),
    F64Const(f64),
    RefNull(RefType),
    RefFunc(u32),
    GlobalGet(u32),
}
