//! GLB inspection for the importer: MEASURED metrics only, never guesses.
//!
//! Skinned models (the generated dancers) report vertex/joint/clip counts
//! straight from `SkinnedModel`; static props report triangle counts from
//! their index buffer via `StaticModel`. Vertex counts come from the GLB's
//! own POSITION accessors (summed over triangle primitives) — a measured
//! number, not a stride guess. The content contract refuses a published mesh
//! with `vertices < 3` or `triangles == 0`, so a count we cannot measure is
//! a publish failure, never a fabricated value.

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

/// Measured topology straight from the glTF document: vertices = sum of the
/// POSITION accessor counts of every triangle primitive, triangles = index
/// count / 3 (or vertex count / 3 for unindexed primitives).
fn measure_topology(glb: &[u8]) -> Option<(u32, u32)> {
    let loaded = makepad_gltf::load_gltf_from_bytes(glb, None).ok()?;
    let doc = &loaded.document;
    let accessors = doc.accessors_slice();
    let (mut vertices, mut triangles) = (0usize, 0usize);
    for mesh in doc.meshes_slice() {
        for prim in &mesh.primitives {
            if prim.mode() != makepad_gltf::GLTF_MODE_TRIANGLES {
                continue;
            }
            let Some(count) = prim
                .attributes
                .get("POSITION")
                .and_then(|&i| accessors.get(i))
                .map(|a| a.count)
            else {
                continue;
            };
            vertices += count;
            triangles += match prim.indices.and_then(|i| accessors.get(i)) {
                Some(index) => index.count / 3,
                None => count / 3,
            };
        }
    }
    Some((
        vertices.min(u32::MAX as usize) as u32,
        triangles.min(u32::MAX as usize) as u32,
    ))
}

pub fn inspect_glb(bytes: &[u8]) -> Result<GlbStats, String> {
    // Skinned first: generated dancers parse here with real joints/clips.
    if let Ok(model) = SkinnedModel::parse_glb(bytes) {
        let joints = model.joint_count();
        let clips = model.clips.len();
        if joints > 0 && clips > 0 {
            let (_, triangles) = measure_topology(bytes).unwrap_or((0, 0));
            return Ok(GlbStats {
                skinned: true,
                triangles,
                vertices: model.vertex_count().min(u32::MAX as usize) as u32,
                joints: joints.min(u16::MAX as usize) as u16,
                clips: clips.min(u16::MAX as usize) as u16,
                base_color: extract_base_color(bytes),
            });
        }
    }
    // Static prop: triangle count from the index buffer, vertex count from
    // the document's POSITION accessors.
    let model = StaticModel::parse_glb(bytes)?;
    let base_color = model.texture_png.clone().or_else(|| extract_base_color(bytes));
    let (vertices, triangles) = measure_topology(bytes).ok_or("glb: unreadable document")?;
    Ok(GlbStats {
        skinned: false,
        // A generic driven rigid part (vehicle wheel, door-like mechanism)
        // intentionally leaves StaticModel's flattened stream. Manifest
        // metrics still describe the whole source asset, so count topology
        // from the glTF document rather than only the static remainder.
        triangles,
        vertices,
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

    /// A generated static GLB must publish: the content contract needs
    /// measured `vertices >= 3` and `triangles > 0`.
    #[test]
    fn static_glb_reports_measured_vertices_and_triangles() {
        let glb = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../local/ai_content_library/lib-6.glb");
        let Ok(bytes) = std::fs::read(&glb) else {
            return; // fixture only present on a dev box with the library
        };
        let stats = inspect_glb(&bytes).expect("generated mesh inspects");
        assert!(!stats.skinned);
        assert!(stats.vertices >= 3, "vertices {}", stats.vertices);
        assert!(stats.triangles > 0, "triangles {}", stats.triangles);
    }
}
