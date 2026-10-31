//! The legacy VM's type-table boundary. Compiler schemas never retain values or heap objects.
use crate::{heap::ScriptHeap, value::*, pod::*,
    shader_ids::*, shader_schema::*, shader_types::ShaderTypes};

impl From<ScriptPodType> for ShaderTypeId {
    fn from(ty: ScriptPodType) -> Self { Self(ty.index) }
}
impl From<ShaderTypeId> for ScriptPodType {
    fn from(ty: ShaderTypeId) -> Self { Self { index: ty.0 } }
}
impl ShaderTypes for ScriptHeap {
    fn type_data(&self, ty: ShaderTypeId) -> &ShaderTypeData {
        self.shader_types[ty.0 as usize].get_or_init(|| type_data(self, self.pod_type_ref(ty.into())))
    }
}

pub fn type_data(heap: &ScriptHeap, data: &ScriptPodTypeData) -> ShaderTypeData {
    ShaderTypeData { name: data.name, def: (data.object != ScriptObject::ZERO).then_some(ShaderDef(data.object.index)), ty: type_kind(heap, &data.ty) }
}
fn inline(heap: &ScriptHeap, ty: &ScriptPodTypeInline) -> ShaderTypeInline {
    ShaderTypeInline { id: ty.self_ref.into(), data: type_data(heap, &ty.data) }
}
fn field(heap: &ScriptHeap, f: &ScriptPodField) -> ShaderField {
    ShaderField { name: f.name, default: constant(heap, f.default), ty: inline(heap, &f.ty) }
}
fn variant(heap: &ScriptHeap, v: &ScriptPodEnumVariant) -> ShaderEnumVariant {
    match v {
        ScriptPodEnumVariant::Bare => ShaderEnumVariant::Bare,
        ScriptPodEnumVariant::Tuple { items } => ShaderEnumVariant::Tuple { items: items.iter().map(|v| inline(heap, v)).collect() },
        ScriptPodEnumVariant::Named { fields } => ShaderEnumVariant::Named { fields: fields.iter().map(|v| field(heap, v)).collect() },
    }
}
pub fn constant(heap: &ScriptHeap, value: ScriptValue) -> Option<ShaderConstant> {
    if let Some(v) = value.as_f64() { return Some(ShaderConstant::F32(v as f32)); }
    if let Some(v) = value.as_f32() { return Some(ShaderConstant::F32(v)); }
    if let Some(v) = value.as_f16() { return Some(ShaderConstant::F32(v as f32)); }
    if let Some(v) = value.as_u32() { return Some(ShaderConstant::U32(v)); }
    if let Some(v) = value.as_u40() { return Some(ShaderConstant::U32(v as u32)); }
    if let Some(v) = value.as_i32() { return Some(ShaderConstant::I32(v)); }
    if let Some(v) = value.as_bool() { return Some(ShaderConstant::Bool(v)); }
    if let Some(v) = value.as_color() {
        let v = makepad_math::Vec4f::from_u32(v);
        return Some(ShaderConstant::Vec(ShaderVec::Vec4f, [v.x, v.y, v.z, v.w]));
    }
    if let Some(pod) = value.as_pod() {
        let (ty, words) = heap.pod_data(pod);
        match ty.ty {
            ScriptPodTy::Vec(v) => {
                let mut data = [0.0; 4];
                for i in 0..v.dims() {
                    data[i] = match v.elem_ty() {
                        ScriptPodTy::F16 => crate::pod_heap::f16_to_f32((words[i / 2] >> ((i % 2) * 16)) as u16),
                        ScriptPodTy::U32 => words[i] as f32,
                        ScriptPodTy::I32 => (words[i] as i32) as f32,
                        ScriptPodTy::Bool => (words[i] != 0) as u8 as f32,
                        _ => f32::from_bits(words[i]),
                    };
                }
                return Some(ShaderConstant::Vec(v.into(), data));
            }
            ScriptPodTy::Mat(v) => {
                let mut data = [0.0; 16];
                let (columns, rows) = v.dims();
                let stride = if rows == 2 { 2 } else { 4 };
                for column in 0..columns {
                    for row in 0..rows {
                        data[column * rows + row] = f32::from_bits(words[column * stride + row]);
                    }
                }
                return Some(ShaderConstant::Mat(v.into(), data));
            }
            _ => {}
        }
    }
    None
}

pub fn type_kind(heap: &ScriptHeap, ty: &ScriptPodTy) -> ShaderTypeKind {
    match ty {
        ScriptPodTy::Void => ShaderTypeKind::Void,
        ScriptPodTy::ArrayBuilder => ShaderTypeKind::ArrayBuilder,
        ScriptPodTy::UndefinedStruct => ShaderTypeKind::UndefinedStruct,
        ScriptPodTy::F32 => ShaderTypeKind::F32,
        ScriptPodTy::F16 => ShaderTypeKind::F16,
        ScriptPodTy::U32 => ShaderTypeKind::U32,
        ScriptPodTy::I32 => ShaderTypeKind::I32,
        ScriptPodTy::Bool => ShaderTypeKind::Bool,
        ScriptPodTy::AtomicU32 => ShaderTypeKind::AtomicU32,
        ScriptPodTy::AtomicI32 => ShaderTypeKind::AtomicI32,
        ScriptPodTy::Vec(v) => ShaderTypeKind::Vec((*v).into()),
        ScriptPodTy::Mat(v) => ShaderTypeKind::Mat((*v).into()),
        ScriptPodTy::Packed(v) => ShaderTypeKind::Packed((*v).into()),
        ScriptPodTy::Struct { align_of, size_of, fields } => ShaderTypeKind::Struct { align_of: *align_of, size_of: *size_of, fields: fields.iter().map(|v| field(heap, v)).collect() },
        ScriptPodTy::Enum { align_of, size_of, variants } => ShaderTypeKind::Enum { align_of: *align_of, size_of: *size_of, variants: variants.iter().map(|v| ShaderEnum { name: v.name, variant: variant(heap, &v.variant) }).collect() },
        ScriptPodTy::FixedArray { align_of, size_of, len, ty } => ShaderTypeKind::FixedArray { align_of: *align_of, size_of: *size_of, len: *len, ty: Box::new(inline(heap, ty)) },
        ScriptPodTy::VariableArray { align_of, ty } => ShaderTypeKind::VariableArray { align_of: *align_of, ty: Box::new(inline(heap, ty)) },
    }
}
impl From<ScriptPodVec> for ShaderVec {
    fn from(value: ScriptPodVec) -> Self {
        match value {
            ScriptPodVec::Vec2f => Self::Vec2f,
            ScriptPodVec::Vec3f => Self::Vec3f,
            ScriptPodVec::Vec4f => Self::Vec4f,
            ScriptPodVec::Vec2h => Self::Vec2h,
            ScriptPodVec::Vec3h => Self::Vec3h,
            ScriptPodVec::Vec4h => Self::Vec4h,
            ScriptPodVec::Vec2u => Self::Vec2u,
            ScriptPodVec::Vec3u => Self::Vec3u,
            ScriptPodVec::Vec4u => Self::Vec4u,
            ScriptPodVec::Vec2i => Self::Vec2i,
            ScriptPodVec::Vec3i => Self::Vec3i,
            ScriptPodVec::Vec4i => Self::Vec4i,
            ScriptPodVec::Vec2b => Self::Vec2b,
            ScriptPodVec::Vec3b => Self::Vec3b,
            ScriptPodVec::Vec4b => Self::Vec4b,
        }
    }
}
impl From<ShaderVec> for ScriptPodVec {
    fn from(value: ShaderVec) -> Self {
        match value {
            ShaderVec::Vec2f => Self::Vec2f,
            ShaderVec::Vec3f => Self::Vec3f,
            ShaderVec::Vec4f => Self::Vec4f,
            ShaderVec::Vec2h => Self::Vec2h,
            ShaderVec::Vec3h => Self::Vec3h,
            ShaderVec::Vec4h => Self::Vec4h,
            ShaderVec::Vec2u => Self::Vec2u,
            ShaderVec::Vec3u => Self::Vec3u,
            ShaderVec::Vec4u => Self::Vec4u,
            ShaderVec::Vec2i => Self::Vec2i,
            ShaderVec::Vec3i => Self::Vec3i,
            ShaderVec::Vec4i => Self::Vec4i,
            ShaderVec::Vec2b => Self::Vec2b,
            ShaderVec::Vec3b => Self::Vec3b,
            ShaderVec::Vec4b => Self::Vec4b,
        }
    }
}
impl From<ScriptPodMat> for ShaderMat {
    fn from(value: ScriptPodMat) -> Self {
        match value {
            ScriptPodMat::Mat2x2f => Self::Mat2x2f,
            ScriptPodMat::Mat3x2f => Self::Mat3x2f,
            ScriptPodMat::Mat4x2f => Self::Mat4x2f,
            ScriptPodMat::Mat2x3f => Self::Mat2x3f,
            ScriptPodMat::Mat3x3f => Self::Mat3x3f,
            ScriptPodMat::Mat4x3f => Self::Mat4x3f,
            ScriptPodMat::Mat2x4f => Self::Mat2x4f,
            ScriptPodMat::Mat3x4f => Self::Mat3x4f,
            ScriptPodMat::Mat4x4f => Self::Mat4x4f,
        }
    }
}
impl From<ShaderMat> for ScriptPodMat {
    fn from(value: ShaderMat) -> Self {
        match value {
            ShaderMat::Mat2x2f => Self::Mat2x2f,
            ShaderMat::Mat3x2f => Self::Mat3x2f,
            ShaderMat::Mat4x2f => Self::Mat4x2f,
            ShaderMat::Mat2x3f => Self::Mat2x3f,
            ShaderMat::Mat3x3f => Self::Mat3x3f,
            ShaderMat::Mat4x3f => Self::Mat4x3f,
            ShaderMat::Mat2x4f => Self::Mat2x4f,
            ShaderMat::Mat3x4f => Self::Mat3x4f,
            ShaderMat::Mat4x4f => Self::Mat4x4f,
        }
    }
}
impl From<ScriptPodPacked> for ShaderPacked {
    fn from(value: ScriptPodPacked) -> Self {
        match value {
            ScriptPodPacked::F16x2 => Self::F16x2,
            ScriptPodPacked::F16x4 => Self::F16x4,
            ScriptPodPacked::U16x2 => Self::U16x2,
            ScriptPodPacked::I16x2 => Self::I16x2,
            ScriptPodPacked::U16x2Norm => Self::U16x2Norm,
            ScriptPodPacked::I16x2Norm => Self::I16x2Norm,
            ScriptPodPacked::U8x4Norm => Self::U8x4Norm,
            ScriptPodPacked::I8x4Norm => Self::I8x4Norm,
        }
    }
}
impl From<ShaderPacked> for ScriptPodPacked {
    fn from(value: ShaderPacked) -> Self {
        match value {
            ShaderPacked::F16x2 => Self::F16x2,
            ShaderPacked::F16x4 => Self::F16x4,
            ShaderPacked::U16x2 => Self::U16x2,
            ShaderPacked::I16x2 => Self::I16x2,
            ShaderPacked::U16x2Norm => Self::U16x2Norm,
            ShaderPacked::I16x2Norm => Self::I16x2Norm,
            ShaderPacked::U8x4Norm => Self::U8x4Norm,
            ShaderPacked::I8x4Norm => Self::I8x4Norm,
        }
    }
}
impl crate::shader_types::ShaderTypes for crate::vm::ScriptVm<'_> {
    fn type_data(&self, ty: ShaderTypeId) -> &ShaderTypeData { self.bx.heap.type_data(ty) }
}
