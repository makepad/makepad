#![allow(unused)]
use makepad_relax_api::relax_api;
use crate::heap::*;
use crate::mod_pod::*;
use crate::trap::*;
use crate::value::*;
use crate::value::*;
use makepad_live_id::*;

#[relax_api(handle)]
#[derive(Clone, PartialEq, Eq)]
pub struct ScriptPodField {
    pub name: LiveId,
    pub default: ScriptValue,
    pub ty: ScriptPodTypeInline,
}

#[derive(Clone, PartialEq, Eq)]
pub struct ScriptPodEnum {
    pub name: LiveId,
    pub variant: ScriptPodEnumVariant,
}

#[derive(Clone, PartialEq, Eq)]
pub enum ScriptPodEnumVariant {
    Bare,
    Tuple { items: Vec<ScriptPodTypeInline> },
    Named { fields: Vec<ScriptPodField> },
}

// we're going to try to follow std140 datamapping for wgsl
#[relax_api(handle)]
#[derive(Default, Clone, PartialEq, Eq)]
pub struct ScriptPodTypeData {
    pub name: Option<LiveId>,
    pub object: ScriptObject,
    pub default: ScriptValue,
    //pub cached_align_of2: usize,
    pub ty: ScriptPodTy,
}

#[relax_api(handle)]
#[derive(Default, Clone, PartialEq, Eq)]
pub struct ScriptPodTypeInline {
    pub self_ref: ScriptPodType,
    pub data: ScriptPodTypeData,
}

/// A pod type's header (ScriptPodTypeData::view).
#[relax_api(plain)]
#[repr(C)]
pub struct ScriptPodTypeDataView<'a> {
    pub name: LiveId,
    pub has_name: bool,
    pub object: ScriptObject,
    pub default: ScriptValue,
    pub ty: &'a ScriptPodTy,
}

/// An inline pod type (ScriptPodTypeInline::view).
#[relax_api(plain)]
#[repr(C)]
pub struct ScriptPodTypeInlineView<'a> {
    pub self_ref: ScriptPodType,
    pub data: &'a ScriptPodTypeData,
}

/// A struct field (ScriptPodField::view).
#[relax_api(plain)]
#[repr(C)]
pub struct ScriptPodFieldView<'a> {
    pub name: LiveId,
    pub default: ScriptValue,
    pub ty: &'a ScriptPodTypeInline,
}

#[relax_api(marked)]
impl ScriptPodTypeData {
    #[relax_api]
    pub fn view(&self) -> ScriptPodTypeDataView<'_> {
        ScriptPodTypeDataView { name: self.name.unwrap_or(LiveId(0)), has_name: self.name.is_some(), object: self.object, default: self.default, ty: &self.ty }
    }
    #[inline]
    pub fn ty(&self) -> &ScriptPodTy { self.view().ty }
    #[inline]
    pub fn name(&self) -> Option<LiveId> { let v = self.view(); v.has_name.then_some(v.name) }
    #[inline]
    pub fn object(&self) -> ScriptObject { self.view().object }
    #[inline]
    pub fn default_value(&self) -> ScriptValue { self.view().default }
}

#[relax_api(marked)]
impl ScriptPodTypeInline {
    #[relax_api]
    pub fn view(&self) -> ScriptPodTypeInlineView<'_> { ScriptPodTypeInlineView { self_ref: self.self_ref, data: &self.data } }
    #[inline]
    pub fn data(&self) -> &ScriptPodTypeData { self.view().data }
    #[inline]
    pub fn self_ref(&self) -> ScriptPodType { self.view().self_ref }
}

#[relax_api(marked)]
impl ScriptPodField {
    #[relax_api]
    pub fn view(&self) -> ScriptPodFieldView<'_> { ScriptPodFieldView { name: self.name, default: self.default, ty: &self.ty } }
    #[inline]
    pub fn name(&self) -> LiveId { self.view().name }
    #[inline]
    pub fn ty(&self) -> &ScriptPodTypeInline { self.view().ty }
    #[inline]
    pub fn default_value(&self) -> ScriptValue { self.view().default }
}

#[relax_api(plain)]
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum ScriptPodVec {
    Vec2f,
    Vec3f,
    Vec4f,
    Vec2h,
    Vec3h,
    Vec4h,
    Vec2u,
    Vec3u,
    Vec4u,
    Vec2i,
    Vec3i,
    Vec4i,
    Vec2b,
    Vec3b,
    Vec4b,
}
#[relax_api(plain)]
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum ScriptPodMat {
    Mat2x2f,
    Mat3x2f,
    Mat4x2f,
    Mat2x3f,
    Mat3x3f,
    Mat4x3f,
    Mat2x4f,
    Mat3x4f,
    Mat4x4f,
}

impl ScriptPodVec {
    pub fn elem_size(&self) -> usize {
        match self {
            Self::Vec2h | Self::Vec3h | Self::Vec4h => 2,
            _ => 4,
        }
    }

    pub fn elem_ty(&self) -> ScriptPodTy {
        match self {
            Self::Vec2f | Self::Vec3f | Self::Vec4f => ScriptPodTy::F32,
            Self::Vec2h | Self::Vec3h | Self::Vec4h => ScriptPodTy::F16,
            Self::Vec2u | Self::Vec3u | Self::Vec4u => ScriptPodTy::U32,
            Self::Vec2i | Self::Vec3i | Self::Vec4i => ScriptPodTy::I32,
            Self::Vec2b | Self::Vec3b | Self::Vec4b => ScriptPodTy::Bool,
        }
    }

    pub fn name(&self) -> LiveId {
        match self {
            Self::Vec2f => id!(vec2f),
            Self::Vec3f => id!(vec3f),
            Self::Vec4f => id!(vec4f),
            Self::Vec2h => id!(vec2h),
            Self::Vec3h => id!(vec3h),
            Self::Vec4h => id!(vec4h),
            Self::Vec2u => id!(vec2u),
            Self::Vec3u => id!(vec3u),
            Self::Vec4u => id!(vec4u),
            Self::Vec2i => id!(vec2i),
            Self::Vec3i => id!(vec3i),
            Self::Vec4i => id!(vec4i),
            Self::Vec2b => id!(vec2b),
            Self::Vec3b => id!(vec3b),
            Self::Vec4b => id!(vec4b),
        }
    }

    pub fn swizzle_type(&self, lanes: usize, builtin: &ScriptPodBuiltins) -> ScriptPodType {
        match self {
            Self::Vec2f => match lanes {
                2 => builtin.pod_vec2f,
                3 => builtin.pod_vec3f,
                4 => builtin.pod_vec4f,
                _ => builtin.pod_f32,
            },
            Self::Vec3f => match lanes {
                2 => builtin.pod_vec2f,
                3 => builtin.pod_vec3f,
                4 => builtin.pod_vec4f,
                _ => builtin.pod_f32,
            },
            Self::Vec4f => match lanes {
                2 => builtin.pod_vec2f,
                3 => builtin.pod_vec3f,
                4 => builtin.pod_vec4f,
                _ => builtin.pod_f32,
            },
            Self::Vec2h => match lanes {
                2 => builtin.pod_vec2h,
                3 => builtin.pod_vec3h,
                4 => builtin.pod_vec4h,
                _ => builtin.pod_f16,
            },
            Self::Vec3h => match lanes {
                2 => builtin.pod_vec2h,
                3 => builtin.pod_vec3h,
                4 => builtin.pod_vec4h,
                _ => builtin.pod_f16,
            },
            Self::Vec4h => match lanes {
                2 => builtin.pod_vec2h,
                3 => builtin.pod_vec3h,
                4 => builtin.pod_vec4h,
                _ => builtin.pod_f16,
            },
            Self::Vec2u => match lanes {
                2 => builtin.pod_vec2u,
                3 => builtin.pod_vec3u,
                4 => builtin.pod_vec4u,
                _ => builtin.pod_u32,
            },
            Self::Vec3u => match lanes {
                2 => builtin.pod_vec2u,
                3 => builtin.pod_vec3u,
                4 => builtin.pod_vec4u,
                _ => builtin.pod_u32,
            },
            Self::Vec4u => match lanes {
                2 => builtin.pod_vec2u,
                3 => builtin.pod_vec3u,
                4 => builtin.pod_vec4u,
                _ => builtin.pod_u32,
            },
            Self::Vec2i => match lanes {
                2 => builtin.pod_vec2i,
                3 => builtin.pod_vec3i,
                4 => builtin.pod_vec4i,
                _ => builtin.pod_i32,
            },
            Self::Vec3i => match lanes {
                2 => builtin.pod_vec2i,
                3 => builtin.pod_vec3i,
                4 => builtin.pod_vec4i,
                _ => builtin.pod_i32,
            },
            Self::Vec4i => match lanes {
                2 => builtin.pod_vec2i,
                3 => builtin.pod_vec3i,
                4 => builtin.pod_vec4i,
                _ => builtin.pod_i32,
            },
            Self::Vec2b => match lanes {
                2 => builtin.pod_vec2b,
                3 => builtin.pod_vec3b,
                4 => builtin.pod_vec4b,
                _ => builtin.pod_bool,
            },
            Self::Vec3b => match lanes {
                2 => builtin.pod_vec2b,
                3 => builtin.pod_vec3b,
                4 => builtin.pod_vec4b,
                _ => builtin.pod_bool,
            },
            Self::Vec4b => match lanes {
                2 => builtin.pod_vec2b,
                3 => builtin.pod_vec3b,
                4 => builtin.pod_vec4b,
                _ => builtin.pod_bool,
            },
        }
    }

    pub fn builtin(&self, builtin: &ScriptPodBuiltins) -> ScriptPodType {
        match self {
            Self::Vec2f => builtin.pod_vec2f,
            Self::Vec3f => builtin.pod_vec3f,
            Self::Vec4f => builtin.pod_vec4f,
            Self::Vec2h => builtin.pod_vec2h,
            Self::Vec3h => builtin.pod_vec3h,
            Self::Vec4h => builtin.pod_vec4h,
            Self::Vec2u => builtin.pod_vec2u,
            Self::Vec3u => builtin.pod_vec3u,
            Self::Vec4u => builtin.pod_vec4u,
            Self::Vec2i => builtin.pod_vec2i,
            Self::Vec3i => builtin.pod_vec3i,
            Self::Vec4i => builtin.pod_vec4i,
            Self::Vec2b => builtin.pod_vec2b,
            Self::Vec3b => builtin.pod_vec3b,
            Self::Vec4b => builtin.pod_vec4b,
        }
    }

    pub fn dims(&self) -> usize {
        match self {
            Self::Vec2f | Self::Vec2h | Self::Vec2u | Self::Vec2i | Self::Vec2b => 2,
            Self::Vec3f | Self::Vec3h | Self::Vec3u | Self::Vec3i | Self::Vec3b => 3,
            Self::Vec4f | Self::Vec4h | Self::Vec4u | Self::Vec4i | Self::Vec4b => 4,
        }
    }
    pub fn align_of(&self) -> usize {
        match self {
            Self::Vec2f => 8,
            Self::Vec3f => 16,
            Self::Vec4f => 8,
            Self::Vec2h => 4,
            Self::Vec3h => 8,
            Self::Vec4h => 16,
            Self::Vec2u => 8,
            Self::Vec3u => 16,
            Self::Vec4u => 16,
            Self::Vec2i => 8,
            Self::Vec3i => 16,
            Self::Vec4i => 16,
            Self::Vec2b => 8,
            Self::Vec3b => 16,
            Self::Vec4b => 16,
        }
    }
    pub fn size_of(&self) -> usize {
        match self {
            Self::Vec2f => 8,
            Self::Vec3f => 12,
            Self::Vec4f => 16,
            Self::Vec2h => 4,
            Self::Vec3h => 6,
            Self::Vec4h => 8,
            Self::Vec2u => 8,
            Self::Vec3u => 12,
            Self::Vec4u => 16,
            Self::Vec2i => 8,
            Self::Vec3i => 12,
            Self::Vec4i => 16,
            Self::Vec2b => 8,
            Self::Vec3b => 12,
            Self::Vec4b => 16,
        }
    }
}
impl ScriptPodMat {
    pub fn elem_size(&self) -> usize {
        match self {
            _ => 4,
        }
    }
    pub fn name(&self) -> LiveId {
        match self {
            Self::Mat2x2f => id!(mat2x2f),
            Self::Mat3x2f => id!(mat3x2f),
            Self::Mat4x2f => id!(mat4x2f),
            Self::Mat2x3f => id!(mat2x3f),
            Self::Mat3x3f => id!(mat3x3f),
            Self::Mat4x3f => id!(mat4x3f),
            Self::Mat2x4f => id!(mat2x4f),
            Self::Mat3x4f => id!(mat3x4f),
            Self::Mat4x4f => id!(mat4x4f),
        }
    }

    pub fn builtin(&self, builtin: &ScriptPodBuiltins) -> ScriptPodType {
        match self {
            Self::Mat2x2f => builtin.pod_mat2x2f,
            Self::Mat3x2f => builtin.pod_mat3x2f,
            Self::Mat4x2f => builtin.pod_mat4x2f,
            Self::Mat2x3f => builtin.pod_mat2x3f,
            Self::Mat3x3f => builtin.pod_mat3x3f,
            Self::Mat4x3f => builtin.pod_mat4x3f,
            Self::Mat2x4f => builtin.pod_mat2x4f,
            Self::Mat3x4f => builtin.pod_mat3x4f,
            Self::Mat4x4f => builtin.pod_mat4x4f,
        }
    }

    pub fn dim(&self) -> usize {
        let (x, y) = self.dims();
        x * y
    }

    pub fn dims(&self) -> (usize, usize) {
        match self {
            Self::Mat2x2f => (2, 2),
            Self::Mat3x2f => (3, 2),
            Self::Mat4x2f => (4, 2),
            Self::Mat2x3f => (2, 3),
            Self::Mat3x3f => (3, 3),
            Self::Mat4x3f => (4, 3),
            Self::Mat2x4f => (2, 4),
            Self::Mat3x4f => (3, 4),
            Self::Mat4x4f => (4, 4),
        }
    }

    pub fn align_of(&self) -> usize {
        match self {
            Self::Mat2x2f => 8,
            Self::Mat3x2f => 8,
            Self::Mat4x2f => 8,
            Self::Mat2x3f => 16,
            Self::Mat3x3f => 16,
            Self::Mat4x3f => 16,
            Self::Mat2x4f => 16,
            Self::Mat3x4f => 16,
            Self::Mat4x4f => 16,
        }
    }
    pub fn size_of(&self) -> usize {
        match self {
            Self::Mat2x2f => 16,
            Self::Mat3x2f => 24,
            Self::Mat4x2f => 32,
            Self::Mat2x3f => 32,
            Self::Mat3x3f => 48,
            Self::Mat4x3f => 64,
            Self::Mat2x4f => 32,
            Self::Mat3x4f => 48,
            Self::Mat4x4f => 64,
        }
    }
}

#[relax_api(handle, clone, debug, default)]
#[derive(Default, Clone, PartialEq, Eq)]
pub enum ScriptPodTy {
    #[default]
    Void,
    ArrayBuilder,
    UndefinedStruct,
    // limited to the types WGSL supports
    F32,
    F16,
    U32,
    I32,
    Bool,
    AtomicU32,
    AtomicI32,
    Vec(ScriptPodVec),
    Mat(ScriptPodMat),
    Struct {
        align_of: usize,
        size_of: usize,
        fields: Vec<ScriptPodField>,
    },
    Enum {
        align_of: usize,
        size_of: usize,
        variants: Vec<ScriptPodEnum>,
    },
    FixedArray {
        align_of: usize,
        size_of: usize,
        len: usize,
        ty: Box<ScriptPodTypeInline>,
    },
    VariableArray {
        align_of: usize,
        ty: Box<ScriptPodTypeInline>,
    },
    /// Physical vertex/instance fetch format. Logical shader type is
    /// `vec2f`/`vec4f`; fetch converts and normalizes where the name says.
    Packed(ScriptPodPacked),
}

/// Compact GPU fetch formats used in `#[repr(C)]` vertex/instance structs.
#[relax_api(plain)]
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum ScriptPodPacked {
    F16x2,
    F16x4,
    U16x2,
    I16x2,
    U16x2Norm,
    I16x2Norm,
    U8x4Norm,
    I8x4Norm,
}

impl ScriptPodPacked {
    pub fn name(&self) -> LiveId {
        match self {
            Self::F16x2 => id!(f16x2),
            Self::F16x4 => id!(f16x4),
            Self::U16x2 => id!(u16x2),
            Self::I16x2 => id!(i16x2),
            Self::U16x2Norm => id!(unorm16x2),
            Self::I16x2Norm => id!(snorm16x2),
            Self::U8x4Norm => id!(unorm8x4),
            Self::I8x4Norm => id!(snorm8x4),
        }
    }

    pub fn size_of(&self) -> usize {
        match self {
            Self::F16x4 => 8,
            _ => 4,
        }
    }

    pub fn align_of(&self) -> usize {
        match self {
            Self::U8x4Norm | Self::I8x4Norm => 1,
            Self::F16x2 | Self::F16x4 | Self::U16x2 | Self::I16x2 | Self::U16x2Norm | Self::I16x2Norm => {
                2
            }
        }
    }

    /// Logical shader component count (`vec2f` = 2, `vec4f` = 4).
    pub fn logical_slots(&self) -> usize {
        match self {
            Self::F16x4 | Self::U8x4Norm | Self::I8x4Norm => 4,
            _ => 2,
        }
    }

    pub fn is_vec4(&self) -> bool {
        self.logical_slots() == 4
    }
}

/// The data-free pod types, for building one without a variant literal (provided-crate
/// boundary, loader-core.md section 8).
#[relax_api(plain)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ScriptPodScalar {
    Void,
    F32,
    F16,
    U32,
    I32,
    Bool,
    AtomicU32,
    AtomicI32,
}

impl ScriptPodTy {
    /// A data-free pod type.
    pub fn scalar(s: ScriptPodScalar) -> ScriptPodTy {
        match s {
            ScriptPodScalar::Void => Self::Void,
            ScriptPodScalar::F32 => Self::F32,
            ScriptPodScalar::F16 => Self::F16,
            ScriptPodScalar::U32 => Self::U32,
            ScriptPodScalar::I32 => Self::I32,
            ScriptPodScalar::Bool => Self::Bool,
            ScriptPodScalar::AtomicU32 => Self::AtomicU32,
            ScriptPodScalar::AtomicI32 => Self::AtomicI32,
        }
    }
}

impl ScriptPodTy {
    #[inline]
    pub fn as_vec(&self) -> Option<ScriptPodVec> {
        let v = self.view(POD_TY_VIEW_BASIC);
        (v.tag == ScriptPodTyTag::Vec).then_some(v.vec)
    }
    #[inline]
    pub fn as_mat(&self) -> Option<ScriptPodMat> {
        let v = self.view(POD_TY_VIEW_BASIC);
        (v.tag == ScriptPodTyTag::Mat).then_some(v.mat)
    }
    #[inline]
    pub fn struct_fields(&self) -> Option<&[ScriptPodField]> {
        let v = self.view(POD_TY_VIEW_BASIC);
        (v.tag == ScriptPodTyTag::Struct).then_some(v.fields)
    }
    pub fn is_number(&self) -> bool {
        match self {
            Self::F32 | Self::F16 | Self::U32 | Self::I32 => true,
            _ => false,
        }
    }

    pub fn is_float_type(&self) -> bool {
        match self {
            Self::F32 | Self::F16 => true,
            Self::Vec(v) => matches!(v.elem_ty(), ScriptPodTy::F32 | ScriptPodTy::F16),
            Self::Mat(_) => true, // Matrices are float-based
            Self::Packed(_) => true,
            _ => false,
        }
    }

    #[inline]
    pub fn packed(&self) -> Option<ScriptPodPacked> {
        let v = self.view(POD_TY_VIEW_BASIC);
        (v.tag == ScriptPodTyTag::Packed).then_some(v.packed)
    }

    /// True when this type (or any struct field) uses a compact fetch format.
    #[inline]
    pub fn has_compact_format(&self) -> bool {
        self.view(POD_TY_VIEW_COMPACT).compact
    }

    fn has_compact_format_impl(&self) -> bool {
        match self {
            Self::Packed(_) => true,
            Self::Struct { fields, .. } => fields.iter().any(|f| f.ty.data.ty.has_compact_format_impl()),
            Self::FixedArray { ty, .. } | Self::VariableArray { ty, .. } => {
                ty.data.ty.has_compact_format_impl()
            }
            _ => false,
        }
    }

    /// Compact fetch supports a compact leaf, or one flat struct containing
    /// direct scalar/float-vector leaves. Aggregate location/path lowering is
    /// deliberately rejected until every backend implements it.
    pub fn vertex_layout_supported(&self) -> bool {
        fn direct_leaf(ty: &ScriptPodTy) -> bool {
            matches!(
                ty,
                ScriptPodTy::F32
                    | ScriptPodTy::F16
                    | ScriptPodTy::U32
                    | ScriptPodTy::I32
                    | ScriptPodTy::Bool
                    | ScriptPodTy::Packed(_)
            ) || matches!(ty, ScriptPodTy::Vec(v) if matches!(v.elem_ty(), ScriptPodTy::F32 | ScriptPodTy::F16))
        }

        match self {
            ty if direct_leaf(ty) => true,
            Self::Struct { fields, .. } => {
                !fields.is_empty() && fields.iter().all(|field| direct_leaf(&field.ty.data.ty))
            }
            _ => false,
        }
    }

    /// All leaf fields are f32-family (the packed-lane GLSL/HLSL path).
    pub fn is_f32_fetch_tree(&self) -> bool {
        match self {
            Self::F32 | Self::F16 => true,
            Self::Vec(v) => matches!(
                v.elem_ty(),
                ScriptPodTy::F32 | ScriptPodTy::F16
            ),
            Self::Mat(_) => true,
            Self::Packed(_) => false,
            Self::U32 | Self::I32 | Self::Bool | Self::AtomicU32 | Self::AtomicI32 => false,
            Self::Struct { fields, .. } => {
                !fields.is_empty() && fields.iter().all(|f| f.ty.data.ty.is_f32_fetch_tree())
            }
            Self::FixedArray { ty, .. } | Self::VariableArray { ty, .. } => {
                ty.data.ty.is_f32_fetch_tree()
            }
            _ => false,
        }
    }

    /// Calculates the struct layout (align_of and size_of) from a slice of fields.
    /// Returns (align_of, size_of) tuple.
    ///
    /// The alignment is the maximum alignment of all fields.
    /// The size is calculated by walking through fields with proper alignment,
    /// then aligning the final size to the struct alignment.
    pub fn calculate_struct_layout(fields: &[ScriptPodField]) -> (usize, usize) {
        // Calculate max alignment from all field types
        let mut align_of = 0usize;
        for field in fields {
            align_of = align_of.max(field.ty.data.ty.align_of());
        }

        // Calculate size by walking through fields with proper alignment
        let mut offset_of = 0usize;
        for field in fields {
            let field_align = field.ty.data.ty.align_of();
            let field_size = field.ty.data.ty.size_of();

            // Align the offset to field alignment
            let rem = offset_of % field_align;
            if rem != 0 {
                offset_of += field_align - rem;
            }

            offset_of += field_size;
        }

        // Align final size to struct alignment
        if align_of > 0 {
            let rem = offset_of % align_of;
            if rem != 0 {
                offset_of += align_of - rem;
            }
        }

        (align_of, offset_of)
    }

    /// Creates a new Struct variant with layout calculated from fields.
    pub fn new_struct(fields: Vec<ScriptPodField>) -> Self {
        let (align_of, size_of) = Self::calculate_struct_layout(&fields);
        Self::Struct {
            align_of,
            size_of,
            fields,
        }
    }
}

#[relax_api(plain)]
#[derive(Debug, Default, Copy, Clone)]
pub struct ScriptPodOffset {
    pub offset_of: usize,
    pub field_index: usize,
}

/// ScriptPodTy::view's mode: POD_TY_VIEW_COMPACT also works out `compact`.
pub const POD_TY_VIEW_BASIC: u32 = 0;
pub const POD_TY_VIEW_COMPACT: u32 = 1;

/// What a pod type reads as (ScriptPodTy::view): its kind, the payload of a vec, mat or packed
/// type (the others' fields are only meaningful for their kind), its layout and its fields.
#[relax_api(plain)]
#[repr(C)]
pub struct ScriptPodTyView<'a> {
    pub tag: ScriptPodTyTag,
    pub vec: ScriptPodVec,
    pub mat: ScriptPodMat,
    pub packed: ScriptPodPacked,
    pub compact: bool,
    pub align_of: usize,
    pub size_of: usize,
    pub slots: usize,
    pub fields: &'a [ScriptPodField],
}

#[relax_api(marked)]
impl ScriptPodTy {
    #[relax_api]
    pub fn view(&self, mode: u32) -> ScriptPodTyView<'_> {
        ScriptPodTyView {
            tag: self.kind_impl(),
            vec: if let Self::Vec(v) = self { *v } else { ScriptPodVec::Vec2f },
            mat: if let Self::Mat(m) = self { *m } else { ScriptPodMat::Mat2x2f },
            packed: if let Self::Packed(p) = self { *p } else { ScriptPodPacked::F16x2 },
            compact: mode == POD_TY_VIEW_COMPACT && self.has_compact_format_impl(),
            align_of: self.align_of_impl(),
            size_of: self.size_of_impl(),
            slots: self.slots_impl(),
            fields: if let Self::Struct { fields, .. } = self { fields } else { &[] },
        }
    }

    #[inline]
    pub fn align_of(&self) -> usize { self.view(POD_TY_VIEW_BASIC).align_of }
    #[inline]
    pub fn size_of(&self) -> usize { self.view(POD_TY_VIEW_BASIC).size_of }
    #[inline]
    pub fn slots(&self) -> usize { self.view(POD_TY_VIEW_BASIC).slots }
    #[inline]
    pub fn kind(&self) -> ScriptPodTyTag { self.view(POD_TY_VIEW_BASIC).tag }

    fn align_of_impl(&self) -> usize {
        match self {
            Self::Void | Self::ArrayBuilder | Self::UndefinedStruct => 0,
            Self::F32 => 4,
            Self::F16 => 2,
            Self::U32 => 4,
            Self::I32 => 4,
            Self::Bool => 4,
            Self::AtomicU32 => 4,
            Self::AtomicI32 => 4,
            Self::Vec(bt) => bt.align_of(),
            Self::Mat(bt) => bt.align_of(),
            Self::Struct { align_of, .. } => *align_of,
            Self::Enum { align_of, .. } => *align_of,
            Self::FixedArray { align_of, .. } => *align_of,
            Self::VariableArray { align_of, .. } => *align_of,
            Self::Packed(p) => p.align_of(),
        }
    }

    pub(crate) fn size_of_impl(&self) -> usize {
        match self {
            Self::Void | Self::ArrayBuilder | Self::UndefinedStruct => 0,
            Self::F32 => 4,
            Self::F16 => 2,
            Self::U32 => 4,
            Self::I32 => 4,
            Self::Bool => 4,
            Self::AtomicU32 => 4,
            Self::AtomicI32 => 4,
            Self::Vec(bt) => bt.size_of(),
            Self::Mat(bt) => bt.size_of(),
            Self::Struct { size_of, .. } => *size_of,
            Self::Enum { size_of, .. } => *size_of,
            Self::FixedArray { size_of, .. } => *size_of,
            Self::VariableArray { .. } => 0,
            Self::Packed(p) => p.size_of(),
        }
    }

    /// Returns the number of float32-sized slots this type occupies.
    /// This is used for shader attribute/uniform layout calculations.
    /// Compact fetch formats report their *logical* f32 component count so
    /// reconstructed shader values stay vec2/vec4.
    fn slots_impl(&self) -> usize {
        if let Self::Packed(p) = self {
            return p.logical_slots();
        }
        // Each slot is 4 bytes (size of f32)
        // For f16, we round up to 1 slot
        let size = self.size_of_impl();
        (size + 3) / 4
    }
}

#[relax_api(plain)]
#[derive(Default, Debug)]
pub struct ScriptPodTag(u64);

impl ScriptPodTag {
    const MARK: u64 = 0x1 << 60;
    const ALLOCED: u64 = 0x2 << 60;
    const STATIC: u64 = 0x8 << 60;
    const ARRAY_BUILDER: u64 = 0x4 << 60;
    pub fn set_array_builder(&mut self, ty: ScriptPodType) {
        self.0 |= Self::ARRAY_BUILDER;
        self.0 |= (ty.index as u64);
    }

    pub fn as_array_builder(&self) -> Option<ScriptPodType> {
        if self.0 & Self::ARRAY_BUILDER != 0 {
            return Some(ScriptPodType {
                index: (self.0 & 0xffff_ffff) as u32,
            });
        }
        None
    }

    pub fn is_marked(&self) -> bool {
        self.0 & Self::MARK != 0
    }

    pub fn set_mark(&mut self) {
        self.0 |= Self::MARK
    }

    pub fn clear_mark(&mut self) {
        self.0 &= !Self::MARK
    }

    pub fn is_alloced(&self) -> bool {
        return self.0 & Self::ALLOCED != 0;
    }

    pub fn set_alloced(&mut self) {
        self.0 |= Self::ALLOCED
    }

    pub fn set_static(&mut self) {
        self.0 |= Self::STATIC
    }

    pub fn is_static(&self) -> bool {
        self.0 & Self::STATIC != 0
    }

    pub fn clear(&mut self) {
        self.0 = 0;
    }
}

#[relax_api(handle)]
#[derive(Default)]
pub struct ScriptPodData {
    pub tag: ScriptPodTag,
    pub ty: ScriptPodType,
    pub data: Vec<u32>,
}

impl ScriptPodData {
    pub fn clear(&mut self) {
        self.tag.clear();
        self.data.clear();
    }
}

#[relax_api(plain)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScriptPodTyTag { Void, ArrayBuilder, UndefinedStruct, F32, F16, U32, I32, Bool, AtomicU32, AtomicI32, Vec, Mat, Struct, Enum, FixedArray, VariableArray, Packed }
impl ScriptPodTy {
    fn kind_impl(&self) -> ScriptPodTyTag {
        match self {
            Self::Void => ScriptPodTyTag::Void,
            Self::ArrayBuilder => ScriptPodTyTag::ArrayBuilder,
            Self::UndefinedStruct => ScriptPodTyTag::UndefinedStruct,
            Self::F32 => ScriptPodTyTag::F32,
            Self::F16 => ScriptPodTyTag::F16,
            Self::U32 => ScriptPodTyTag::U32,
            Self::I32 => ScriptPodTyTag::I32,
            Self::Bool => ScriptPodTyTag::Bool,
            Self::AtomicU32 => ScriptPodTyTag::AtomicU32,
            Self::AtomicI32 => ScriptPodTyTag::AtomicI32,
            Self::Vec(_) => ScriptPodTyTag::Vec,
            Self::Mat(_) => ScriptPodTyTag::Mat,
            Self::Struct { .. } => ScriptPodTyTag::Struct,
            Self::Enum { .. } => ScriptPodTyTag::Enum,
            Self::FixedArray { .. } => ScriptPodTyTag::FixedArray,
            Self::VariableArray { .. } => ScriptPodTyTag::VariableArray,
            Self::Packed(_) => ScriptPodTyTag::Packed,
        }
    }
}

impl ScriptPodTypeData {
    pub fn new(name: Option<LiveId>, ty: ScriptPodTy) -> Self { Self { name, ty, ..Self::default() } }
}
impl ScriptPodTypeInline {
    pub fn new(self_ref: ScriptPodType, data: ScriptPodTypeData) -> Self { Self { self_ref, data } }
}
impl ScriptPodField {
    pub fn new(name: LiveId, default: ScriptValue, ty: ScriptPodTypeInline) -> Self { Self { name, default, ty } }
}
impl ScriptPodEnum { pub fn name(&self) -> LiveId { self.name } }
impl ScriptPodTy {
    pub fn vector(v: ScriptPodVec) -> Self { Self::Vec(v) }
    pub fn matrix(v: ScriptPodMat) -> Self { Self::Mat(v) }
    pub fn packed_type(v: ScriptPodPacked) -> Self { Self::Packed(v) }
    pub fn structure(align_of: usize, size_of: usize) -> Self { Self::Struct { align_of, size_of, fields: Vec::new() } }
    pub fn push_field(&mut self, field: ScriptPodField) {
        if let Self::Struct { fields, .. } = self { fields.push(field); } else { panic!("expected a struct pod type"); }
    }
    pub fn fixed_array(align_of: usize, size_of: usize, len: usize, ty: ScriptPodTypeInline) -> Self { Self::FixedArray { align_of, size_of, len, ty: Box::new(ty) } }
    pub fn variable_array(align_of: usize, ty: ScriptPodTypeInline) -> Self { Self::VariableArray { align_of, ty: Box::new(ty) } }
    pub fn enum_variants(&self) -> Option<&[ScriptPodEnum]> { if let Self::Enum { variants, .. } = self { Some(variants) } else { None } }
    pub fn array_element(&self) -> Option<&ScriptPodTypeInline> { match self { Self::FixedArray { ty, .. } | Self::VariableArray { ty, .. } => Some(ty), _ => None } }
    pub fn array_len(&self) -> Option<usize> { if let Self::FixedArray { len, .. } = self { Some(*len) } else { None } }
}

impl ScriptPodEnum {
    pub fn variant(&self) -> &ScriptPodEnumVariant { &self.variant }
}
impl ScriptPodEnumVariant {
    pub fn tuple_items(&self) -> Option<&[ScriptPodTypeInline]> { if let Self::Tuple { items } = self { Some(items) } else { None } }
    pub fn named_fields(&self) -> Option<&[ScriptPodField]> { if let Self::Named { fields } = self { Some(fields) } else { None } }
}
impl std::fmt::Debug for ScriptPodTy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use ScriptPodTyTag::*;
        match self.kind() {
            Void => f.write_str("Void"), ArrayBuilder => f.write_str("ArrayBuilder"), UndefinedStruct => f.write_str("UndefinedStruct"),
            F32 => f.write_str("F32"), F16 => f.write_str("F16"), U32 => f.write_str("U32"), I32 => f.write_str("I32"),
            Bool => f.write_str("Bool"), AtomicU32 => f.write_str("AtomicU32"), AtomicI32 => f.write_str("AtomicI32"),
            Vec => f.debug_tuple("Vec").field(&self.as_vec().unwrap()).finish(),
            Mat => f.debug_tuple("Mat").field(&self.as_mat().unwrap()).finish(),
            Packed => f.debug_tuple("Packed").field(&self.packed().unwrap()).finish(),
            Struct => f.debug_struct("Struct").field("align_of", &self.align_of()).field("size_of", &self.size_of()).field("fields", &self.struct_fields().unwrap()).finish(),
            Enum => f.debug_struct("Enum").field("align_of", &self.align_of()).field("size_of", &self.size_of()).field("variants", &self.enum_variants().unwrap()).finish(),
            FixedArray => f.debug_struct("FixedArray").field("align_of", &self.align_of()).field("size_of", &self.size_of()).field("len", &self.array_len().unwrap()).field("ty", self.array_element().unwrap()).finish(),
            VariableArray => f.debug_struct("VariableArray").field("align_of", &self.align_of()).field("ty", self.array_element().unwrap()).finish(),
        }
    }
}
impl std::fmt::Debug for ScriptPodTypeData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScriptPodTypeData").field("name", &self.name()).field("object", &self.object()).field("default", &self.default_value()).field("ty", self.ty()).finish()
    }
}
impl std::fmt::Debug for ScriptPodTypeInline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.debug_struct("ScriptPodTypeInline").field("self_ref", &self.self_ref()).field("data", self.data()).finish() }
}
impl std::fmt::Debug for ScriptPodField {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.debug_struct("ScriptPodField").field("name", &self.name()).field("default", &self.default_value()).field("ty", self.ty()).finish() }
}
impl std::fmt::Debug for ScriptPodEnum {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.debug_struct("ScriptPodEnum").field("name", &self.name()).field("variant", self.variant()).finish() }
}
impl std::fmt::Debug for ScriptPodEnumVariant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(items) = self.tuple_items() { f.debug_struct("Tuple").field("items", &items).finish() }
        else if let Some(fields) = self.named_fields() { f.debug_struct("Named").field("fields", &fields).finish() }
        else { f.write_str("Bare") }
    }
}
