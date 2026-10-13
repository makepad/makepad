//! A draw shader's reflected record as a kernel layout: the kernel's
//! `output(Name)` / `emit_buffer(Name, n)` writes the draw's own struct in
//! place, field by field, at the offsets the reflection reports
//! (`Cx::layout_of(shader, Instance | Vertex, VertexFetch)`).
//!
//! Integers stay integer words. Field types a kernel cannot write yet
//! (integer vectors, packed formats) are refused here with the field named,
//! so a mismatch is a diagnostic at compile time, never a wrong draw.

use makepad_platform::draw_shader_layout::{Layout as ShaderLayout, PodType};
use makepad_script_compute::boundary::{LayoutFieldView, LayoutView};
use makepad_script_compute::kernel::FieldTy;

/// A record layout the host owns and builds at run time (a draw shader's
/// reflected record, a tool's JSON, a document's `Sim` state). Fixed
/// layouts are `const` [`LayoutView`] tables instead; a kernel compile takes
/// views of either ([`RecordLayout::field_views`], [`views`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordLayout {
    pub name: String,
    /// Words per record (including padding).
    pub stride: u32,
    pub fields: Vec<RecordField>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordField {
    pub name: String,
    pub ty: FieldTy,
    /// Word offset inside the record.
    pub offset: u32,
}

impl RecordLayout {
    /// An owned copy of a fixed table.
    pub fn of(view: &LayoutView<'_>) -> RecordLayout {
        RecordLayout {
            name: view.name.to_string(),
            stride: view.stride,
            fields: view.fields.iter().map(|f| RecordField { name: f.name.to_string(), ty: f.ty, offset: f.offset }).collect(),
        }
    }

    /// The fields as views, for [`RecordLayout::view`].
    pub fn field_views(&self) -> Vec<LayoutFieldView<'_>> {
        self.fields.iter().map(|f| LayoutFieldView { name: &f.name, ty: f.ty, offset: f.offset }).collect()
    }

    /// This layout as a view over `fields` (its [`RecordLayout::field_views`]).
    pub fn view<'a>(&'a self, fields: &'a [LayoutFieldView<'a>]) -> LayoutView<'a> {
        LayoutView { name: &self.name, stride: self.stride, fields }
    }
}

/// Views of `layouts` over their `fields` (one [`RecordLayout::field_views`]
/// each, in order), for a kernel compile.
pub fn views<'a>(layouts: &'a [RecordLayout], fields: &'a [Vec<LayoutFieldView<'a>>]) -> Vec<LayoutView<'a>> {
    layouts.iter().zip(fields).map(|(layout, fields)| layout.view(fields)).collect()
}

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
pub fn kernel_layout(name: &str, layout: &ShaderLayout) -> Result<RecordLayout, LayoutMapError> {
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
        fields.push(RecordField { name: f.name.to_string(), ty, offset: f.offset_words });
    }
    Ok(RecordLayout { name: name.to_string(), stride: layout.stride_words, fields })
}
