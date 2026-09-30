//! Layout reflection: `layout_of(draw_shader, Instance | Vertex, packing)`
//! answers where each field of a draw shader's instance or vertex record
//! lives in memory, so a kernel can write the record in place (the kernel's
//! output record *is* the draw shader's struct).
//!
//! Two packings are reflected from one field list:
//!
//! - [`LayoutPacking::VertexFetch`]: the buffer the draw reads. Instance
//!   records are the draw list's f32 slots (`DrawShaderInputs`' logical
//!   offsets, with the integer-vector lane padding); vertex records follow
//!   the geometry fetch (one blob per vertex-buffer field whose members the
//!   backends decode as a tight scalar sequence, or per-field physical
//!   offsets when the geometry uses compact formats).
//! - [`LayoutPacking::Storage`]: a storage buffer for GPU compute, laid out
//!   with the WGSL/std430 rules the pod types use (`align_of`, `size_of`).
//!
//! Integers are words read as integers, never floats. Backend limits
//! (attribute count, stride) are checked here, when the layout is
//! reflected, not at draw time.

use {
    crate::{
        cx::Cx,
        draw_shader::{CxDrawShaderMapping, DrawShaderAttrFormat, DrawShaderId, DrawShaderInputs},
        makepad_live_id::*,
        makepad_script::heap::ScriptHeap,
        makepad_script::pod::{ScriptPodMat, ScriptPodTy, ScriptPodVec},
        makepad_script::shader::*,
    },
};

/// Which record of a draw shader.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LayoutKind {
    Instance,
    Vertex,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LayoutPacking {
    VertexFetch,
    Storage,
}

/// A field's type in the record. Every type is a whole number of 32-bit
/// words; `Packed` fields hold a compact format the kernel writes as raw
/// words (f16 pairs, unorm8 quads).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PodType {
    F32,
    I32,
    U32,
    Vec2,
    Vec3,
    Vec4,
    IVec2,
    IVec3,
    IVec4,
    UVec2,
    UVec3,
    UVec4,
    Mat4,
    Packed(DrawShaderAttrFormat),
}

impl PodType {
    /// Words the field occupies.
    pub fn words(self) -> u32 {
        match self {
            PodType::F32 | PodType::I32 | PodType::U32 => 1,
            PodType::Vec2 | PodType::IVec2 | PodType::UVec2 => 2,
            PodType::Vec3 | PodType::IVec3 | PodType::UVec3 => 3,
            PodType::Vec4 | PodType::IVec4 | PodType::UVec4 => 4,
            PodType::Mat4 => 16,
            PodType::Packed(f) => (f.byte_size() as u32).div_ceil(4),
        }
    }

    /// std430 alignment in words.
    fn storage_align(self) -> u32 {
        match self {
            PodType::Vec2 | PodType::IVec2 | PodType::UVec2 => 2,
            PodType::Vec3 | PodType::IVec3 | PodType::UVec3 | PodType::Vec4 | PodType::IVec4 | PodType::UVec4 | PodType::Mat4 => 4,
            _ => 1,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct LayoutField {
    pub name: LiveId,
    pub ty: PodType,
    pub offset_words: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    pub kind: LayoutKind,
    pub packing: LayoutPacking,
    /// Words per record, padding included.
    pub stride_words: u32,
    pub fields: Vec<LayoutField>,
    /// Changes whenever the record's physical layout does (a new layout id
    /// after a shader edit: hot reload swaps buffers on it).
    pub id: u64,
}

impl Layout {
    pub fn field(&self, name: LiveId) -> Option<&LayoutField> {
        self.fields.iter().find(|f| f.name == name)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum LayoutError {
    /// The shader has no record of that kind.
    Empty,
    /// A field type a kernel cannot write (named, with the type).
    Unsupported { field: LiveId, ty: String },
    /// Compact instance formats are converted on upload: not writable yet.
    CompactInstance(LiveId),
    /// The record needs more vertex attributes than the backends guarantee.
    TooManyAttributes { need: usize, limit: usize },
    StrideTooLarge { bytes: usize, limit: usize },
    /// A struct with padding between or after its fields (a vec3 before a
    /// wider-aligned field): the fetch decoders read it as a tight scalar
    /// sequence but size it differently per backend, so no kernel may
    /// write it. Reorder the fields or widen the vec3.
    Padding(LiveId),
    /// The reflection disagrees with the draw system's own packing (a bug).
    Mismatch { reflected: usize, packed: usize },
    NoSuchShader,
}

impl std::fmt::Display for LayoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LayoutError::Empty => write!(f, "the shader has no such record"),
            LayoutError::Unsupported { field, ty } => write!(f, "field `{}` has type {}, which kernels cannot write", field, ty),
            LayoutError::CompactInstance(field) => write!(f, "instance field `{}` uses a compact format; kernels write f32/i32/u32 words", field),
            LayoutError::TooManyAttributes { need, limit } => write!(f, "the record needs {} vertex attributes; the limit is {}", need, limit),
            LayoutError::StrideTooLarge { bytes, limit } => write!(f, "the record is {} bytes; the limit is {}", bytes, limit),
            LayoutError::Padding(field) => write!(f, "`{}` has padding inside its record; reorder its fields or use vec4 so it packs tight", field),
            LayoutError::Mismatch { reflected, packed } => write!(f, "internal: reflected {} words but the draw packs {}", reflected, packed),
            LayoutError::NoSuchShader => write!(f, "no such draw shader"),
        }
    }
}

/// Vertex attributes (vec4 each) the backend provides: Metal 31; GL ES 3,
/// WebGL 2, Vulkan and D3D11 guarantee 16.
#[cfg(any(target_os = "macos", target_os = "ios", target_os = "tvos"))]
pub const MAX_VERTEX_ATTRIBUTES: usize = 31;
#[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "tvos")))]
pub const MAX_VERTEX_ATTRIBUTES: usize = 16;
/// Vertex stride every backend accepts (Metal and Vulkan minimum).
pub const MAX_VERTEX_STRIDE_BYTES: usize = 2048;

/// One instance or vertex-buffer io of a compiled shader with its pod type,
/// captured when the shader is compiled (the type is not kept elsewhere).
#[derive(Clone, Debug, Default)]
pub struct ReflectedIo {
    pub name: LiveId,
    pub ty: ScriptPodTy,
}

/// The typed record fields of a compiled draw shader.
#[derive(Clone, Debug, Default)]
pub struct DrawShaderReflection {
    /// Dyn instance fields first, then Rust instance fields (the draw's
    /// memory order).
    pub instance: Vec<ReflectedIo>,
    pub vertex: Vec<ReflectedIo>,
}

impl DrawShaderReflection {
    pub fn from_output(output: &ShaderOutput, heap: &ScriptHeap) -> Self {
        let pick = |kind: fn(&ShaderIoKind) -> bool| -> Vec<ReflectedIo> {
            output.io.iter().filter(|io| kind(&io.kind)).map(|io| ReflectedIo { name: io.name, ty: heap.pod_type_ref(io.ty).ty.clone() }).collect()
        };
        let mut instance = pick(|k| matches!(k, ShaderIoKind::DynInstance));
        instance.extend(pick(|k| matches!(k, ShaderIoKind::RustInstance)));
        DrawShaderReflection { instance, vertex: pick(|k| matches!(k, ShaderIoKind::VertexBuffer)) }
    }
}

fn unsupported(name: LiveId, ty: &ScriptPodTy) -> LayoutError {
    LayoutError::Unsupported { field: name, ty: format!("{:?}", ty) }
}

/// A leaf pod type as a record field type.
fn leaf(name: LiveId, ty: &ScriptPodTy) -> Result<PodType, LayoutError> {
    Ok(match ty {
        ScriptPodTy::F32 => PodType::F32,
        ScriptPodTy::I32 | ScriptPodTy::AtomicI32 => PodType::I32,
        ScriptPodTy::U32 | ScriptPodTy::AtomicU32 | ScriptPodTy::Bool => PodType::U32,
        ScriptPodTy::Vec(v) => match v {
            ScriptPodVec::Vec2f => PodType::Vec2,
            ScriptPodVec::Vec3f => PodType::Vec3,
            ScriptPodVec::Vec4f => PodType::Vec4,
            ScriptPodVec::Vec2i => PodType::IVec2,
            ScriptPodVec::Vec3i => PodType::IVec3,
            ScriptPodVec::Vec4i => PodType::IVec4,
            ScriptPodVec::Vec2u | ScriptPodVec::Vec2b => PodType::UVec2,
            ScriptPodVec::Vec3u | ScriptPodVec::Vec3b => PodType::UVec3,
            ScriptPodVec::Vec4u | ScriptPodVec::Vec4b => PodType::UVec4,
            _ => return Err(unsupported(name, ty)),
        },
        ScriptPodTy::Mat(ScriptPodMat::Mat4x4f) => PodType::Mat4,
        _ => return Err(unsupported(name, ty)),
    })
}

/// Fields of one io, laid tight from `at` (the fetch decode: members are
/// read as one scalar sequence).
fn tight(name: LiveId, ty: &ScriptPodTy, at: &mut u32, out: &mut Vec<LayoutField>) -> Result<(), LayoutError> {
    if let ScriptPodTy::Struct { fields, .. } = ty {
        for f in fields {
            tight(f.name, &f.ty.data.ty, at, out)?;
        }
        return Ok(());
    }
    let ty = leaf(name, ty)?;
    out.push(LayoutField { name, ty, offset_words: *at });
    *at += ty.words();
    Ok(())
}

/// Fields of one io at std430 offsets from `base`; returns the end.
fn storage(name: LiveId, ty: &ScriptPodTy, base: u32, out: &mut Vec<LayoutField>) -> Result<u32, LayoutError> {
    if let ScriptPodTy::Struct { fields, align_of, size_of } = ty {
        let start = base.next_multiple_of((*align_of as u32 / 4).max(1));
        let mut at = start;
        for f in fields {
            at = storage(f.name, &f.ty.data.ty, at, out)?;
        }
        return Ok((start + *size_of as u32 / 4).max(at));
    }
    let ty = leaf(name, ty)?;
    let at = base.next_multiple_of(ty.storage_align());
    out.push(LayoutField { name, ty, offset_words: at });
    Ok(at + ty.words())
}

fn signature(kind: LayoutKind, packing: LayoutPacking, stride: u32, fields: &[LayoutField]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    let mut mix = |bytes: &[u8]| {
        for b in bytes {
            h = (h ^ *b as u64).wrapping_mul(0x100_0000_01b3);
        }
    };
    mix(&[kind as u8, packing as u8]);
    mix(&stride.to_le_bytes());
    for f in fields {
        mix(&f.name.0.to_le_bytes());
        mix(format!("{:?}", f.ty).as_bytes());
        mix(&f.offset_words.to_le_bytes());
    }
    h
}

impl CxDrawShaderMapping {
    /// The layout of this shader's instance or vertex record.
    pub fn layout_of(&self, kind: LayoutKind, packing: LayoutPacking) -> Result<Layout, LayoutError> {
        layout_from(&self.reflection, &self.instances, &self.geometries, kind, packing)
    }
}

/// Vertex attributes a draw binds: vec4-packed slots, or one per compact
/// geometry input.
fn vertex_attributes(instances: &DrawShaderInputs, geometries: &DrawShaderInputs) -> usize {
    let geometry = if geometries.has_compact() { geometries.inputs.len() } else { geometries.total_slots.div_ceil(4) };
    geometry + instances.total_slots.div_ceil(4)
}

/// `layout_of` over the reflected fields and the draw's own packing of them.
pub fn layout_from(
    reflection: &DrawShaderReflection,
    instances: &DrawShaderInputs,
    geometries: &DrawShaderInputs,
    kind: LayoutKind,
    packing: LayoutPacking,
) -> Result<Layout, LayoutError> {
    let ios = match kind {
        LayoutKind::Instance => &reflection.instance,
        LayoutKind::Vertex => &reflection.vertex,
    };
    if ios.is_empty() {
        return Err(LayoutError::Empty);
    }
    let mut fields = Vec::new();
    let stride = match packing {
        LayoutPacking::Storage => {
            let mut at = 0;
            let mut align = 1;
            for io in ios {
                at = storage(io.name, &io.ty, at, &mut fields)?;
                align = align.max((io.ty.align_of() as u32 / 4).max(1));
            }
            at.next_multiple_of(align)
        }
        LayoutPacking::VertexFetch => match kind {
            LayoutKind::Instance => {
                // The draw list's slots: DrawShaderInputs' logical
                // offsets, integer vectors padded to a vec4 boundary.
                let mut at = 0u32;
                for io in ios {
                    if io.ty.has_compact_format() {
                        return Err(LayoutError::CompactInstance(io.name));
                    }
                    let slots = io.ty.slots() as u32;
                    let int_lanes = CxDrawShaderMapping::attr_format_from_pod_type(&io.ty).is_integer_fetch() && slots > 1;
                    if int_lanes {
                        at = at.next_multiple_of(4);
                    }
                    let mut inner = at;
                    tight(io.name, &io.ty, &mut inner, &mut fields)?;
                    if inner != at + slots {
                        return Err(LayoutError::Padding(io.name));
                    }
                    at += slots;
                    if int_lanes {
                        at = at.next_multiple_of(4);
                    }
                }
                if at as usize != instances.total_slots {
                    return Err(LayoutError::Mismatch { reflected: at as usize, packed: instances.total_slots });
                }
                at
            }
            LayoutKind::Vertex if geometries.has_compact() => {
                // Per-field physical inputs at their byte offsets.
                for input in &geometries.inputs {
                    let ty = match input.attr_format {
                        f if f.is_compact() => PodType::Packed(f),
                        DrawShaderAttrFormat::U32x1 => [PodType::U32, PodType::UVec2, PodType::UVec3, PodType::UVec4][input.slots.clamp(1, 4) - 1],
                        DrawShaderAttrFormat::I32x1 => [PodType::I32, PodType::IVec2, PodType::IVec3, PodType::IVec4][input.slots.clamp(1, 4) - 1],
                        _ => [PodType::F32, PodType::Vec2, PodType::Vec3, PodType::Vec4][input.slots.clamp(1, 4) - 1],
                    };
                    if input.byte_offset % 4 != 0 {
                        return Err(LayoutError::Unsupported { field: input.id, ty: format!("{:?} at byte {}", input.attr_format, input.byte_offset) });
                    }
                    fields.push(LayoutField { name: input.id, ty, offset_words: input.byte_offset as u32 / 4 });
                }
                (geometries.stride_bytes as u32).div_ceil(4)
            }
            LayoutKind::Vertex => {
                // One blob per vertex-buffer io, decoded as a tight
                // scalar sequence from the blob's slot offset.
                for (io, input) in ios.iter().zip(&geometries.inputs) {
                    let mut at = input.offset as u32;
                    tight(io.name, &io.ty, &mut at, &mut fields)?;
                    if at != (input.offset + input.slots) as u32 {
                        return Err(LayoutError::Padding(io.name));
                    }
                }
                (if geometries.stride_bytes != 0 { geometries.stride_bytes } else { geometries.total_slots * 4 }) as u32 / 4
            }
        },
    };
    if packing == LayoutPacking::VertexFetch {
        // Instance and vertex attributes share the backend's budget.
        let need = vertex_attributes(instances, geometries);
        if need > MAX_VERTEX_ATTRIBUTES {
            return Err(LayoutError::TooManyAttributes { need, limit: MAX_VERTEX_ATTRIBUTES });
        }
        if stride as usize * 4 > MAX_VERTEX_STRIDE_BYTES {
            return Err(LayoutError::StrideTooLarge { bytes: stride as usize * 4, limit: MAX_VERTEX_STRIDE_BYTES });
        }
    }
    let id = signature(kind, packing, stride, &fields);
    Ok(Layout { kind, packing, stride_words: stride, fields, id })
}

impl Cx {
    /// `layout_of(draw_shader, Instance | Vertex)`: see the module docs.
    pub fn layout_of(&self, shader: DrawShaderId, kind: LayoutKind, packing: LayoutPacking) -> Result<Layout, LayoutError> {
        if shader.index >= self.draw_shaders.shaders.len() {
            return Err(LayoutError::NoSuchShader);
        }
        self.draw_shaders[shader.index].mapping.layout_of(kind, packing)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::draw_shader::DrawShaderInputPacking;
    use crate::makepad_script::pod::{ScriptPodField, ScriptPodPacked, ScriptPodTypeData, ScriptPodTypeInline};
    use crate::makepad_script::value::NIL;

    fn field(name: LiveId, ty: ScriptPodTy) -> ScriptPodField {
        ScriptPodField { name, default: NIL, ty: ScriptPodTypeInline { data: ScriptPodTypeData { ty, ..Default::default() }, ..Default::default() } }
    }

    fn vec(v: ScriptPodVec) -> ScriptPodTy {
        ScriptPodTy::Vec(v)
    }

    /// Reflection plus the draw's own packing of the same ios, built the
    /// way `CxDrawShaderMapping::from_shader_output` builds them.
    fn shader(instance: &[(LiveId, ScriptPodTy)], vertex: &[(LiveId, ScriptPodTy)]) -> (DrawShaderReflection, DrawShaderInputs, DrawShaderInputs) {
        let mut instances = DrawShaderInputs::new(DrawShaderInputPacking::Attribute);
        let mut geometries = DrawShaderInputs::new(DrawShaderInputPacking::Attribute);
        let mut refl = DrawShaderReflection::default();
        for (name, ty) in instance {
            CxDrawShaderMapping::push_pod_fields(&mut instances, ty, *name);
            refl.instance.push(ReflectedIo { name: *name, ty: ty.clone() });
        }
        for (name, ty) in vertex {
            CxDrawShaderMapping::push_pod_fields(&mut geometries, ty, *name);
            refl.vertex.push(ReflectedIo { name: *name, ty: ty.clone() });
        }
        instances.finalize();
        geometries.finalize();
        (refl, instances, geometries)
    }

    fn offsets(l: &Layout) -> Vec<(LiveId, PodType, u32)> {
        l.fields.iter().map(|f| (f.name, f.ty, f.offset_words)).collect()
    }

    #[test]
    fn instance_records_follow_the_draw_lists_slots() {
        let (r, i, g) = shader(
            &[
                (id!(color), vec(ScriptPodVec::Vec4f)),
                (id!(pos), vec(ScriptPodVec::Vec3f)),
                (id!(id), ScriptPodTy::U32),
                (id!(xf), ScriptPodTy::Mat(ScriptPodMat::Mat4x4f)),
                (id!(seeds), vec(ScriptPodVec::Vec2u)),
                (id!(phase), ScriptPodTy::F32),
            ],
            &[],
        );
        let l = layout_from(&r, &i, &g, LayoutKind::Instance, LayoutPacking::VertexFetch).unwrap();
        // Integer vectors sit on a vec4 boundary (the fetch's int lanes).
        assert_eq!(
            offsets(&l),
            vec![
                (id!(color), PodType::Vec4, 0),
                (id!(pos), PodType::Vec3, 4),
                (id!(id), PodType::U32, 7),
                (id!(xf), PodType::Mat4, 8),
                (id!(seeds), PodType::UVec2, 24),
                (id!(phase), PodType::F32, 28),
            ]
        );
        assert_eq!(l.stride_words as usize, i.total_slots);
        assert_eq!(l.stride_words, 29);
    }

    #[test]
    fn storage_packing_uses_std430_alignment() {
        let (r, i, g) = shader(&[(id!(a), ScriptPodTy::F32), (id!(b), vec(ScriptPodVec::Vec3f)), (id!(c), vec(ScriptPodVec::Vec2f))], &[]);
        let fetch = layout_from(&r, &i, &g, LayoutKind::Instance, LayoutPacking::VertexFetch).unwrap();
        let store = layout_from(&r, &i, &g, LayoutKind::Instance, LayoutPacking::Storage).unwrap();
        assert_eq!(offsets(&fetch), vec![(id!(a), PodType::F32, 0), (id!(b), PodType::Vec3, 1), (id!(c), PodType::Vec2, 4)]);
        assert_eq!(fetch.stride_words, 6);
        assert_eq!(offsets(&store), vec![(id!(a), PodType::F32, 0), (id!(b), PodType::Vec3, 4), (id!(c), PodType::Vec2, 8)]);
        assert_eq!(store.stride_words, 12);
        assert_ne!(fetch.id, store.id);
    }

    #[test]
    fn vertex_structs_are_fetched_as_one_tight_blob() {
        let vertex = ScriptPodTy::new_struct(vec![
            field(id!(pos), vec(ScriptPodVec::Vec3f)),
            field(id!(u), ScriptPodTy::F32),
            field(id!(normal), vec(ScriptPodVec::Vec4f)),
            field(id!(uv), vec(ScriptPodVec::Vec2f)),
            field(id!(uv2), vec(ScriptPodVec::Vec2f)),
        ]);
        let (r, i, g) = shader(&[], &[(id!(geom), vertex)]);
        let l = layout_from(&r, &i, &g, LayoutKind::Vertex, LayoutPacking::VertexFetch).unwrap();
        // The backends decode the blob as one scalar sequence.
        assert_eq!(
            offsets(&l),
            vec![(id!(pos), PodType::Vec3, 0), (id!(u), PodType::F32, 3), (id!(normal), PodType::Vec4, 4), (id!(uv), PodType::Vec2, 8), (id!(uv2), PodType::Vec2, 10)]
        );
        assert_eq!(l.stride_words, 12);
        let s = layout_from(&r, &i, &g, LayoutKind::Vertex, LayoutPacking::Storage).unwrap();
        assert_eq!(offsets(&s), offsets(&l), "no padding: the two packings agree");
        // With padding (vec3 then vec3) the backends size the record
        // differently (GLSL 48 bytes, Metal's packed struct 32): refused.
        let padded = ScriptPodTy::new_struct(vec![
            field(id!(pos), vec(ScriptPodVec::Vec3f)),
            field(id!(normal), vec(ScriptPodVec::Vec3f)),
            field(id!(uv), vec(ScriptPodVec::Vec2f)),
        ]);
        let (r, i, g) = shader(&[], &[(id!(geom), padded)]);
        assert_eq!(layout_from(&r, &i, &g, LayoutKind::Vertex, LayoutPacking::VertexFetch), Err(LayoutError::Padding(id!(geom))));
        let s = layout_from(&r, &i, &g, LayoutKind::Vertex, LayoutPacking::Storage).unwrap();
        assert_eq!(offsets(&s), vec![(id!(pos), PodType::Vec3, 0), (id!(normal), PodType::Vec3, 4), (id!(uv), PodType::Vec2, 8)]);
        assert_eq!(s.stride_words, 12);
    }

    #[test]
    fn compact_vertex_fields_keep_their_physical_offsets() {
        let vertex = ScriptPodTy::new_struct(vec![
            field(id!(pos), vec(ScriptPodVec::Vec3f)),
            field(id!(uv), ScriptPodTy::Packed(ScriptPodPacked::F16x2)),
            field(id!(color), ScriptPodTy::Packed(ScriptPodPacked::U8x4Norm)),
        ]);
        let (r, i, g) = shader(&[], &[(id!(geom), vertex)]);
        assert!(g.has_compact());
        let l = layout_from(&r, &i, &g, LayoutKind::Vertex, LayoutPacking::VertexFetch).unwrap();
        assert_eq!(
            offsets(&l),
            vec![
                (id!(pos), PodType::Vec3, 0),
                (id!(uv), PodType::Packed(DrawShaderAttrFormat::F16x2), 3),
                (id!(color), PodType::Packed(DrawShaderAttrFormat::U8x4Norm), 4),
            ]
        );
        assert_eq!(l.stride_words as usize * 4, g.stride_bytes);
    }

    #[test]
    fn what_kernels_cannot_write_is_refused_by_name() {
        let (r, i, g) = shader(&[(id!(uv), ScriptPodTy::Packed(ScriptPodPacked::F16x2))], &[]);
        assert_eq!(layout_from(&r, &i, &g, LayoutKind::Instance, LayoutPacking::VertexFetch), Err(LayoutError::CompactInstance(id!(uv))));
        let (r, i, g) = shader(&[(id!(h), vec(ScriptPodVec::Vec3h))], &[]);
        assert!(matches!(layout_from(&r, &i, &g, LayoutKind::Instance, LayoutPacking::Storage), Err(LayoutError::Unsupported { field, .. }) if field == id!(h)));
        let (r, i, g) = shader(&[(id!(a), ScriptPodTy::F32)], &[]);
        assert_eq!(layout_from(&r, &i, &g, LayoutKind::Vertex, LayoutPacking::VertexFetch), Err(LayoutError::Empty));
    }

    #[test]
    fn backend_limits_are_checked_at_reflection() {
        let many: Vec<(LiveId, ScriptPodTy)> = (0..40).map(|k| (LiveId(k + 1), ScriptPodTy::Mat(ScriptPodMat::Mat4x4f))).collect();
        let (r, i, g) = shader(&many, &[]);
        assert!(matches!(layout_from(&r, &i, &g, LayoutKind::Instance, LayoutPacking::VertexFetch), Err(LayoutError::TooManyAttributes { .. })));
        // A storage buffer has no attribute limit.
        assert!(layout_from(&r, &i, &g, LayoutKind::Instance, LayoutPacking::Storage).is_ok());
    }

    #[test]
    fn the_layout_id_follows_the_physical_layout() {
        let a = shader(&[(id!(p), vec(ScriptPodVec::Vec3f)), (id!(s), ScriptPodTy::F32)], &[]);
        let b = shader(&[(id!(p), vec(ScriptPodVec::Vec3f)), (id!(s), ScriptPodTy::F32)], &[]);
        let c = shader(&[(id!(s), ScriptPodTy::F32), (id!(p), vec(ScriptPodVec::Vec3f))], &[]);
        let id = |x: &(DrawShaderReflection, DrawShaderInputs, DrawShaderInputs)| layout_from(&x.0, &x.1, &x.2, LayoutKind::Instance, LayoutPacking::VertexFetch).unwrap().id;
        assert_eq!(id(&a), id(&b));
        assert_ne!(id(&a), id(&c));
    }
}
