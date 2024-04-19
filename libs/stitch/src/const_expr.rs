use crate::{
    decode::{Decode, DecodeError, Decoder},
    func::Func,
    func_ref::FuncRef,
    global::{Global, Mut},
    module::ModuleBuilder,
    ref_::{Ref, RefType},
    store::Store,
    val::{Val, ValType},
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

    pub(crate) fn func_idx(&self) -> Option<u32> {
        match self.instr {
            ConstInstr::RefFunc(func_idx) => Some(func_idx),
            _ => None,
        }
    }

    pub(crate) fn validate(&self, module: &ModuleBuilder) -> Result<ValType, DecodeError> {
        match self.instr {
            ConstInstr::I32Const(_) => Ok(ValType::I32),
            ConstInstr::I64Const(_) => Ok(ValType::I64),
            ConstInstr::F32Const(_) => Ok(ValType::F32),
            ConstInstr::F64Const(_) => Ok(ValType::F64),
            ConstInstr::RefNull(type_) => Ok(type_.into()),
            ConstInstr::RefFunc(func_idx) => {
                module.func(func_idx)?;
                Ok(ValType::FuncRef)
            }
            ConstInstr::GlobalGet(global_idx) => {
                let type_ = module.imported_global(global_idx)?;
                if type_.mut_ != Mut::Const {
                    return Err(DecodeError::new("global is immutable"));
                }
                Ok(type_.val)
            }
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

impl Decode for ConstExpr {
    fn decode(decoder: &mut Decoder<'_>) -> Result<Self, DecodeError> {
        let instr = decoder.decode()?;
        if decoder.read_byte()? != 0x0B {
            return Err(DecodeError::new("expected end opcode"));
        }
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

impl Decode for ConstInstr {
    fn decode(decoder: &mut Decoder<'_>) -> Result<Self, DecodeError> {
        match decoder.read_byte()? {
            0x23 => Ok(Self::GlobalGet(decoder.decode()?)),
            0x41 => Ok(Self::I32Const(decoder.decode()?)),
            0x42 => Ok(Self::I64Const(decoder.decode()?)),
            0x43 => Ok(Self::F32Const(decoder.decode()?)),
            0x44 => Ok(Self::F64Const(decoder.decode()?)),
            0xD0 => Ok(Self::RefNull(decoder.decode()?)),
            0xD2 => Ok(Self::RefFunc(decoder.decode()?)),
            _ => Err(DecodeError::new("illegal const opcode")),
        }
    }
}
