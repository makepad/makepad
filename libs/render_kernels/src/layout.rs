//! A draw shader's reflected record as a kernel layout: the kernel's
//! `output(Name)` / `emit_buffer(Name, n)` writes the draw's own struct in
//! place, field by field, at the offsets the reflection reports
//! (`Cx::layout_of(shader, Instance | Vertex, VertexFetch)`).
//!
//! Integers stay integer words. Field types a kernel cannot write yet
//! (integer vectors, packed formats) are refused here with the field named,
//! so a mismatch is a diagnostic at compile time, never a wrong draw.

use makepad_platform::draw_shader_layout::{Layout as ShaderLayout, PodType};
use makepad_script_compute::kernel::{FieldTy, Layout, LayoutField};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LayoutMapError {
    /// A field type the kernel language cannot write yet.
    Unsupported { field: String, ty: String },
}

impl std::fmt::Display for LayoutMapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LayoutMapError::Unsupported { field, ty } => write!(f, "field `{}` is a {}, which a kernel cannot write yet", field, ty),
        }
    }
}

/// The kernel layout named `name` for a reflected draw record.
pub fn kernel_layout(name: &str, layout: &ShaderLayout) -> Result<Layout, LayoutMapError> {
    let mut fields = Vec::with_capacity(layout.fields.len());
    for f in &layout.fields {
        let ty = match f.ty {
            PodType::F32 => FieldTy::F32,
            PodType::I32 | PodType::U32 => FieldTy::I32,
            PodType::Vec2 => FieldTy::Vec2,
            PodType::Vec3 => FieldTy::Vec3,
            PodType::Vec4 => FieldTy::Vec4,
            PodType::Mat4 => FieldTy::Mat4,
            other => return Err(LayoutMapError::Unsupported { field: f.name.to_string(), ty: format!("{:?}", other) }),
        };
        fields.push(LayoutField { name: f.name.to_string(), ty, offset: f.offset_words });
    }
    Ok(Layout { name: name.to_string(), stride: layout.stride_words, fields })
}
