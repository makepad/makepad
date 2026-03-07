mod buffer;
mod document;
mod error;
mod image;
mod loader;
mod parser;
#[cfg(test)]
mod sample_tests;

pub use crate::buffer::{
    decode_mesh_primitive, read_accessor_f32x2, read_accessor_f32x3, read_accessor_indices_u32,
    GLTF_COMPONENT_TYPE_BYTE, GLTF_COMPONENT_TYPE_FLOAT,
    GLTF_COMPONENT_TYPE_SHORT, GLTF_COMPONENT_TYPE_UNSIGNED_BYTE, GLTF_COMPONENT_TYPE_UNSIGNED_INT,
    GLTF_COMPONENT_TYPE_UNSIGNED_SHORT,
};
pub use crate::document::*;
pub use crate::error::GltfError;
pub use crate::image::load_image_bytes;
pub use crate::loader::{load_gltf_from_bytes, load_gltf_from_path, GltfContainerKind, LoadedGltf};
pub use crate::parser::{is_glb_bytes, parse_glb_bytes, parse_gltf_json, GlbChunk, ParsedGlb};
pub use makepad_math::DecodedPrimitive;
