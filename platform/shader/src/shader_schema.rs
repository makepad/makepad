//! Owned GPU type schemas, layouts and swizzles.
use makepad_relax_api::relax_api;
use crate::{shader_ids::*};
use makepad_live_id::*;

/// Values that can be stored in a shader declaration without a VM allocation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ShaderConstant {
    F32(f32),
    U32(u32),
    I32(i32),
    Bool(bool),
    Vec(ShaderVec, [f32; 4]),
    Mat(ShaderMat, [f32; 16]),
}

/// A swizzle field name of a pod vector (`x` .. `wwww`, `r` .. `aaaa`): how
/// many components it reads and which lane each one comes from.
#[relax_api(plain)]
#[derive(Clone, Copy)]
pub struct PodSwizzle {
    pub len: u8,
    pub lanes: [u8; 4],
}

const POD_SWIZZLE_SLOTS: usize = 2048;

/// The 680 swizzle names by id, open-addressed on the id's low bits (a third
/// full). Built at compile time from the two lane alphabets, so a lookup is a
/// probe or two instead of a 680-arm `match` per call site.
static POD_SWIZZLES: [(LiveId, PodSwizzle); POD_SWIZZLE_SLOTS] = pod_swizzle_table();

const fn pod_swizzle_table() -> [(LiveId, PodSwizzle); POD_SWIZZLE_SLOTS] {
    let mut table = [(LiveId(0), PodSwizzle { len: 0, lanes: [0; 4] }); POD_SWIZZLE_SLOTS];
    let alphabets: [&[u8; 4]; 2] = [b"xyzw", b"rgba"];
    let mut alphabet = 0;
    while alphabet < alphabets.len() {
        let mut len = 1;
        while len <= 4 {
            // Every lane combination of this length, first lane most significant.
            let mut code = 0;
            while code < 1 << (2 * len) {
                let mut name = [0u8; 4];
                let mut lanes = [0u8; 4];
                let mut i = 0;
                while i < len {
                    lanes[i] = ((code >> (2 * (len - 1 - i))) & 3) as u8;
                    name[i] = alphabets[alphabet][lanes[i] as usize];
                    i += 1;
                }
                let id = LiveId::from_bytes(LiveId::SEED, &name, 0, len, 0);
                let mut slot = id.0 as usize & (POD_SWIZZLE_SLOTS - 1);
                while table[slot].0 .0 != 0 {
                    slot = (slot + 1) & (POD_SWIZZLE_SLOTS - 1);
                }
                table[slot] = (
                    id,
                    PodSwizzle {
                        len: len as u8,
                        lanes,
                    },
                );
                code += 1;
            }
            len += 1;
        }
        alphabet += 1;
    }
    table
}

/// The swizzle a field name of a pod vector stands for, if it is one.
pub fn pod_swizzle(id: LiveId) -> Option<PodSwizzle> {
    let mut slot = id.0 as usize & (POD_SWIZZLE_SLOTS - 1);
    loop {
        let (key, swizzle) = POD_SWIZZLES[slot];
        if key.0 == 0 {
            return None;
        }
        if key == id {
            return Some(swizzle);
        }
        slot = (slot + 1) & (POD_SWIZZLE_SLOTS - 1);
    }
}

#[derive(Clone, PartialEq)]
pub struct ShaderField {
    pub name: LiveId,
    pub default: Option<ShaderConstant>,
    pub ty: ShaderTypeInline,
}

#[derive(Clone, PartialEq)]
pub struct ShaderEnum {
    pub name: LiveId,
    pub variant: ShaderEnumVariant,
}

#[derive(Clone, PartialEq)]
pub enum ShaderEnumVariant {
    Bare,
    Tuple { items: Vec<ShaderTypeInline> },
    Named { fields: Vec<ShaderField> },
}

// we're going to try to follow std140 datamapping for wgsl
#[derive(Default, Clone, PartialEq)]
pub struct ShaderTypeData {
    pub name: Option<LiveId>,
    pub def: Option<ShaderDef>,
    //pub cached_align_of2: usize,
    pub ty: ShaderTypeKind,
}

#[derive(Default, Clone, PartialEq)]
pub struct ShaderTypeInline {
    pub id: ShaderTypeId,
    pub data: ShaderTypeData,
}

impl ShaderTypeData {
    pub fn ty(&self) -> &ShaderTypeKind { &self.ty }
    pub fn name(&self) -> Option<LiveId> { self.name }
    pub fn definition(&self) -> Option<ShaderDef> { self.def }
}

impl ShaderTypeInline {
    pub fn data(&self) -> &ShaderTypeData { &self.data }
    pub fn id(&self) -> ShaderTypeId { self.id }
}

impl ShaderField {
    pub fn name(&self) -> LiveId { self.name }
    pub fn ty(&self) -> &ShaderTypeInline { &self.ty }
    pub fn default_value(&self) -> Option<ShaderConstant> { self.default }
}

#[relax_api(plain)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum ShaderVec {
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
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum ShaderMat {
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

impl ShaderVec {
    pub fn elem_size(&self) -> usize {
        match self {
            Self::Vec2h | Self::Vec3h | Self::Vec4h => 2,
            _ => 4,
        }
    }

    pub fn elem_ty(&self) -> ShaderTypeKind {
        match self {
            Self::Vec2f | Self::Vec3f | Self::Vec4f => ShaderTypeKind::F32,
            Self::Vec2h | Self::Vec3h | Self::Vec4h => ShaderTypeKind::F16,
            Self::Vec2u | Self::Vec3u | Self::Vec4u => ShaderTypeKind::U32,
            Self::Vec2i | Self::Vec3i | Self::Vec4i => ShaderTypeKind::I32,
            Self::Vec2b | Self::Vec3b | Self::Vec4b => ShaderTypeKind::Bool,
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

    pub fn swizzle_type(&self, lanes: usize, builtin: &ShaderBuiltins) -> ShaderTypeId {
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

    pub fn builtin(&self, builtin: &ShaderBuiltins) -> ShaderTypeId {
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
impl ShaderMat {
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

    pub fn builtin(&self, builtin: &ShaderBuiltins) -> ShaderTypeId {
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

#[derive(Default, Clone, PartialEq)]
pub enum ShaderTypeKind {
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
    Vec(ShaderVec),
    Mat(ShaderMat),
    Struct {
        align_of: usize,
        size_of: usize,
        fields: Vec<ShaderField>,
    },
    Enum {
        align_of: usize,
        size_of: usize,
        variants: Vec<ShaderEnum>,
    },
    FixedArray {
        align_of: usize,
        size_of: usize,
        len: usize,
        ty: Box<ShaderTypeInline>,
    },
    VariableArray {
        align_of: usize,
        ty: Box<ShaderTypeInline>,
    },
    /// Physical vertex/instance fetch format. Logical shader type is
    /// `vec2f`/`vec4f`; fetch converts and normalizes where the name says.
    Packed(ShaderPacked),
}

/// Compact GPU fetch formats used in `#[repr(C)]` vertex/instance structs.
#[relax_api(plain)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum ShaderPacked {
    F16x2,
    F16x4,
    U16x2,
    I16x2,
    U16x2Norm,
    I16x2Norm,
    U8x4Norm,
    I8x4Norm,
}

impl ShaderPacked {
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
pub enum ShaderScalar {
    Void,
    F32,
    F16,
    U32,
    I32,
    Bool,
    AtomicU32,
    AtomicI32,
}

impl ShaderTypeKind {
    /// A data-free pod type.
    pub fn scalar(s: ShaderScalar) -> ShaderTypeKind {
        match s {
            ShaderScalar::Void => Self::Void,
            ShaderScalar::F32 => Self::F32,
            ShaderScalar::F16 => Self::F16,
            ShaderScalar::U32 => Self::U32,
            ShaderScalar::I32 => Self::I32,
            ShaderScalar::Bool => Self::Bool,
            ShaderScalar::AtomicU32 => Self::AtomicU32,
            ShaderScalar::AtomicI32 => Self::AtomicI32,
        }
    }
}

impl ShaderTypeKind {
    pub fn as_vec(&self) -> Option<ShaderVec> {
        if let Self::Vec(v) = self { Some(*v) } else { None }
    }
    pub fn as_mat(&self) -> Option<ShaderMat> {
        if let Self::Mat(v) = self { Some(*v) } else { None }
    }
    pub fn struct_fields(&self) -> Option<&[ShaderField]> {
        if let Self::Struct { fields, .. } = self { Some(fields) } else { None }
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
            Self::Vec(v) => matches!(v.elem_ty(), ShaderTypeKind::F32 | ShaderTypeKind::F16),
            Self::Mat(_) => true, // Matrices are float-based
            Self::Packed(_) => true,
            _ => false,
        }
    }

    pub fn packed(&self) -> Option<ShaderPacked> {
        match self {
            Self::Packed(p) => Some(*p),
            _ => None,
        }
    }

    /// True when this type (or any struct field) uses a compact fetch format.
    pub fn has_compact_format(&self) -> bool {
        match self {
            Self::Packed(_) => true,
            Self::Struct { fields, .. } => fields.iter().any(|f| f.ty.data.ty.has_compact_format()),
            Self::FixedArray { ty, .. } | Self::VariableArray { ty, .. } => {
                ty.data.ty.has_compact_format()
            }
            _ => false,
        }
    }

    /// Compact fetch supports a compact leaf, or one flat struct containing
    /// direct scalar/float-vector leaves. Aggregate location/path lowering is
    /// deliberately rejected until every backend implements it.
    pub fn vertex_layout_supported(&self) -> bool {
        fn direct_leaf(ty: &ShaderTypeKind) -> bool {
            matches!(
                ty,
                ShaderTypeKind::F32
                    | ShaderTypeKind::F16
                    | ShaderTypeKind::U32
                    | ShaderTypeKind::I32
                    | ShaderTypeKind::Bool
                    | ShaderTypeKind::Packed(_)
            ) || matches!(ty, ShaderTypeKind::Vec(v) if matches!(v.elem_ty(), ShaderTypeKind::F32 | ShaderTypeKind::F16))
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
                ShaderTypeKind::F32 | ShaderTypeKind::F16
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
    pub fn calculate_struct_layout(fields: &[ShaderField]) -> (usize, usize) {
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
    pub fn new_struct(fields: Vec<ShaderField>) -> Self {
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
pub struct ShaderOffset {
    pub offset_of: usize,
    pub field_index: usize,
}

impl ShaderTypeKind {
    pub fn align_of(&self) -> usize {
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

    pub fn size_of(&self) -> usize {
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
    pub fn slots(&self) -> usize {
        if let Self::Packed(p) = self {
            return p.logical_slots();
        }
        // Each slot is 4 bytes (size of f32)
        // For f16, we round up to 1 slot
        let size = self.size_of();
        (size + 3) / 4
    }
}

#[relax_api(plain)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShaderTypeTag { Void, ArrayBuilder, UndefinedStruct, F32, F16, U32, I32, Bool, AtomicU32, AtomicI32, Vec, Mat, Struct, Enum, FixedArray, VariableArray, Packed }
impl ShaderTypeKind {
    pub fn kind(&self) -> ShaderTypeTag {
        match self {
            Self::Void => ShaderTypeTag::Void,
            Self::ArrayBuilder => ShaderTypeTag::ArrayBuilder,
            Self::UndefinedStruct => ShaderTypeTag::UndefinedStruct,
            Self::F32 => ShaderTypeTag::F32,
            Self::F16 => ShaderTypeTag::F16,
            Self::U32 => ShaderTypeTag::U32,
            Self::I32 => ShaderTypeTag::I32,
            Self::Bool => ShaderTypeTag::Bool,
            Self::AtomicU32 => ShaderTypeTag::AtomicU32,
            Self::AtomicI32 => ShaderTypeTag::AtomicI32,
            Self::Vec(_) => ShaderTypeTag::Vec,
            Self::Mat(_) => ShaderTypeTag::Mat,
            Self::Struct { .. } => ShaderTypeTag::Struct,
            Self::Enum { .. } => ShaderTypeTag::Enum,
            Self::FixedArray { .. } => ShaderTypeTag::FixedArray,
            Self::VariableArray { .. } => ShaderTypeTag::VariableArray,
            Self::Packed(_) => ShaderTypeTag::Packed,
        }
    }
}

impl ShaderTypeData {
    pub fn new(name: Option<LiveId>, ty: ShaderTypeKind) -> Self { Self { name, ty, ..Self::default() } }
}
impl ShaderTypeInline {
    pub fn new(id: ShaderTypeId, data: ShaderTypeData) -> Self { Self { id, data } }
}
impl ShaderField {
    pub fn new(name: LiveId, default: Option<ShaderConstant>, ty: ShaderTypeInline) -> Self { Self { name, default, ty } }
}
impl ShaderEnum { pub fn name(&self) -> LiveId { self.name } }
impl ShaderTypeKind {
    pub fn vector(v: ShaderVec) -> Self { Self::Vec(v) }
    pub fn matrix(v: ShaderMat) -> Self { Self::Mat(v) }
    pub fn packed_type(v: ShaderPacked) -> Self { Self::Packed(v) }
    pub fn structure(align_of: usize, size_of: usize) -> Self { Self::Struct { align_of, size_of, fields: Vec::new() } }
    pub fn push_field(&mut self, field: ShaderField) {
        if let Self::Struct { fields, .. } = self { fields.push(field); } else { panic!("expected a struct pod type"); }
    }
    pub fn fixed_array(align_of: usize, size_of: usize, len: usize, ty: ShaderTypeInline) -> Self { Self::FixedArray { align_of, size_of, len, ty: Box::new(ty) } }
    pub fn variable_array(align_of: usize, ty: ShaderTypeInline) -> Self { Self::VariableArray { align_of, ty: Box::new(ty) } }
    pub fn enum_variants(&self) -> Option<&[ShaderEnum]> { if let Self::Enum { variants, .. } = self { Some(variants) } else { None } }
    pub fn array_element(&self) -> Option<&ShaderTypeInline> { match self { Self::FixedArray { ty, .. } | Self::VariableArray { ty, .. } => Some(ty), _ => None } }
    pub fn array_len(&self) -> Option<usize> { if let Self::FixedArray { len, .. } = self { Some(*len) } else { None } }
}

impl ShaderEnum {
    pub fn variant(&self) -> &ShaderEnumVariant { &self.variant }
}
impl ShaderEnumVariant {
    pub fn tuple_items(&self) -> Option<&[ShaderTypeInline]> { if let Self::Tuple { items } = self { Some(items) } else { None } }
    pub fn named_fields(&self) -> Option<&[ShaderField]> { if let Self::Named { fields } = self { Some(fields) } else { None } }
}
impl std::fmt::Debug for ShaderTypeKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use ShaderTypeTag::*;
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
impl std::fmt::Debug for ShaderTypeData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShaderTypeData").field("name", &self.name()).field("def", &self.definition()).field("ty", self.ty()).finish()
    }
}
impl std::fmt::Debug for ShaderTypeInline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.debug_struct("ShaderTypeInline").field("id", &self.id()).field("data", self.data()).finish() }
}
impl std::fmt::Debug for ShaderField {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.debug_struct("ShaderField").field("name", &self.name()).field("default", &self.default_value()).field("ty", self.ty()).finish() }
}
impl std::fmt::Debug for ShaderEnum {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.debug_struct("ShaderEnum").field("name", &self.name()).field("variant", self.variant()).finish() }
}
impl std::fmt::Debug for ShaderEnumVariant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(items) = self.tuple_items() { f.debug_struct("Tuple").field("items", &items).finish() }
        else if let Some(fields) = self.named_fields() { f.debug_struct("Named").field("fields", &fields).finish() }
        else { f.write_str("Bare") }
    }
}

#[relax_api(plain)]
#[derive(Default, Clone)]
#[repr(C)]
pub struct ShaderBuiltins {
    pub pod_void: ShaderTypeId,
    pub pod_struct: ShaderTypeId,
    pub pod_array: ShaderTypeId,
    pub pod_bool: ShaderTypeId,
    pub pod_f32: ShaderTypeId,
    pub pod_f16: ShaderTypeId,
    pub pod_u32: ShaderTypeId,
    pub pod_i32: ShaderTypeId,
    pub pod_atomic_u32: ShaderTypeId,
    pub pod_atomic_i32: ShaderTypeId,
    pub pod_vec2f: ShaderTypeId,
    pub pod_vec3f: ShaderTypeId,
    pub pod_vec4f: ShaderTypeId,
    pub pod_vec2h: ShaderTypeId,
    pub pod_vec3h: ShaderTypeId,
    pub pod_vec4h: ShaderTypeId,
    pub pod_vec2u: ShaderTypeId,
    pub pod_vec3u: ShaderTypeId,
    pub pod_vec4u: ShaderTypeId,
    pub pod_vec2i: ShaderTypeId,
    pub pod_vec3i: ShaderTypeId,
    pub pod_vec4i: ShaderTypeId,
    pub pod_vec2b: ShaderTypeId,
    pub pod_vec3b: ShaderTypeId,
    pub pod_vec4b: ShaderTypeId,
    pub pod_mat2x2f: ShaderTypeId,
    pub pod_mat2x3f: ShaderTypeId,
    pub pod_mat2x4f: ShaderTypeId,
    pub pod_mat3x2f: ShaderTypeId,
    pub pod_mat3x3f: ShaderTypeId,
    pub pod_mat3x4f: ShaderTypeId,
    pub pod_mat4x2f: ShaderTypeId,
    pub pod_mat4x3f: ShaderTypeId,
    pub pod_mat4x4f: ShaderTypeId,
    pub pod_f16x2: ShaderTypeId,
    pub pod_f16x4: ShaderTypeId,
    pub pod_u16x2: ShaderTypeId,
    pub pod_i16x2: ShaderTypeId,
    pub pod_unorm16x2: ShaderTypeId,
    pub pod_snorm16x2: ShaderTypeId,
    pub pod_unorm8x4: ShaderTypeId,
    pub pod_snorm8x4: ShaderTypeId,
}

pub fn format_pod_type_from_builtins(
    pod_ty: ShaderTypeId,
    builtins: &ShaderBuiltins,
) -> String {
    // Check against known builtin types
    if pod_ty == builtins.pod_void {
        return "void".to_string();
    }
    if pod_ty == builtins.pod_f32 {
        return "f32".to_string();
    }
    if pod_ty == builtins.pod_f16 {
        return "f16".to_string();
    }
    if pod_ty == builtins.pod_u32 {
        return "u32".to_string();
    }
    if pod_ty == builtins.pod_i32 {
        return "i32".to_string();
    }
    if pod_ty == builtins.pod_vec2f {
        return "vec2f".to_string();
    }
    if pod_ty == builtins.pod_vec3f {
        return "vec3f".to_string();
    }
    if pod_ty == builtins.pod_vec4f {
        return "vec4f".to_string();
    }
    if pod_ty == builtins.pod_vec2h {
        return "vec2h".to_string();
    }
    if pod_ty == builtins.pod_vec3h {
        return "vec3h".to_string();
    }
    if pod_ty == builtins.pod_vec4h {
        return "vec4h".to_string();
    }
    if pod_ty == builtins.pod_vec2u {
        return "vec2u".to_string();
    }
    if pod_ty == builtins.pod_vec3u {
        return "vec3u".to_string();
    }
    if pod_ty == builtins.pod_vec4u {
        return "vec4u".to_string();
    }
    if pod_ty == builtins.pod_vec2i {
        return "vec2i".to_string();
    }
    if pod_ty == builtins.pod_vec3i {
        return "vec3i".to_string();
    }
    if pod_ty == builtins.pod_vec4i {
        return "vec4i".to_string();
    }
    if pod_ty == builtins.pod_mat2x2f {
        return "mat2x2f".to_string();
    }
    if pod_ty == builtins.pod_mat3x3f {
        return "mat3x3f".to_string();
    }
    if pod_ty == builtins.pod_mat4x4f {
        return "mat4x4f".to_string();
    }
    if pod_ty == builtins.pod_mat2x3f {
        return "mat2x3f".to_string();
    }
    if pod_ty == builtins.pod_mat2x4f {
        return "mat2x4f".to_string();
    }
    if pod_ty == builtins.pod_mat3x2f {
        return "mat3x2f".to_string();
    }
    if pod_ty == builtins.pod_mat3x4f {
        return "mat3x4f".to_string();
    }
    if pod_ty == builtins.pod_mat4x2f {
        return "mat4x2f".to_string();
    }
    if pod_ty == builtins.pod_mat4x3f {
        return "mat4x3f".to_string();
    }

    // Unknown type - just show index
    format!("type#{}", pod_ty.0)
}
