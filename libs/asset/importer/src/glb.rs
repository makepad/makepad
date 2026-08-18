//! GLB inspection for the importer: MEASURED metrics only, never guesses.
//!
//! Skinned models (the generated dancers) report vertex/joint/clip counts
//! straight from `SkinnedModel`; static props report triangle counts from
//! their index buffer via `StaticModel`. Fields that cannot be measured for
//! a given model class stay zero — the manifest metrics block treats zero
//! as "unmeasured", and fabricating numbers would poison the catalog.

use makepad_render::skin::SkinnedModel;
use makepad_render::StaticModel;

pub struct GlbStats {
    /// Skinned + animated (joints and at least one clip).
    pub skinned: bool,
    pub triangles: u32,
    pub vertices: u32,
    pub joints: u16,
    pub clips: u16,
    /// Embedded base-color image bytes (PNG/JPEG) when the GLB carries one —
    /// the thumbnail fallback for meshes without a rendered preview.
    pub base_color: Option<Vec<u8>>,
}

/// Embedded base-color image out of a GLB: material 0's baseColorTexture
/// source, else image 0.
fn extract_base_color(glb: &[u8]) -> Option<Vec<u8>> {
    let loaded = makepad_gltf::load_gltf_from_bytes(glb, None).ok()?;
    let doc = &loaded.document;
    let image_index = doc
        .materials_slice()
        .first()
        .and_then(|m| m.pbr_metallic_roughness.as_ref())
        .and_then(|pbr| pbr.base_color_texture.as_ref())
        .and_then(|info| doc.textures_slice().get(info.index))
        .and_then(|tex| tex.source)
        .or(if doc.images_slice().is_empty() { None } else { Some(0) })?;
    makepad_gltf::load_image_bytes(&loaded, image_index).ok()
}

pub fn inspect_glb(bytes: &[u8]) -> Result<GlbStats, String> {
    // Skinned first: generated dancers parse here with real joints/clips.
    if let Ok(model) = SkinnedModel::parse_glb(bytes) {
        let joints = model.joint_count();
        let clips = model.clips.len();
        if joints > 0 && clips > 0 {
            return Ok(GlbStats {
                skinned: true,
                // Triangle topology is not exposed by the skinned parser:
                // honestly unmeasured.
                triangles: 0,
                vertices: model.vertex_count().min(u32::MAX as usize) as u32,
                joints: joints.min(u16::MAX as usize) as u16,
                clips: clips.min(u16::MAX as usize) as u16,
                base_color: extract_base_color(bytes),
            });
        }
    }
    // Static prop: triangle count from the index buffer.
    let model = StaticModel::parse_glb(bytes)?;
    let base_color = model.texture_png.clone().or_else(|| extract_base_color(bytes));
    Ok(GlbStats {
        skinned: false,
        triangles: (model.indices.len() / 3).min(u32::MAX as usize) as u32,
        // The packed-vertex stride is a renderer detail; vertex count stays
        // unmeasured here rather than derived from an assumed layout.
        vertices: 0,
        joints: 0,
        clips: 0,
        base_color,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn garbage_refuses_instead_of_reporting_zero_stats() {
        assert!(inspect_glb(b"not a glb at all").is_err());
        assert!(inspect_glb(b"glTFxx").is_err());
    }
}
