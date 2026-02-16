use crate::{
    document::GltfDocument,
    error::GltfError,
};
use makepad_micro_serde::DeJson;

const GLB_MAGIC: u32 = 0x4654_6C67;
const GLB_JSON_CHUNK: u32 = 0x4E4F_534A;
const GLB_BIN_CHUNK: u32 = 0x004E_4942;

#[derive(Clone, Debug)]
pub struct GlbChunk {
    pub chunk_type: u32,
    pub data: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct ParsedGlb {
    pub version: u32,
    pub document: GltfDocument,
    pub bin_chunk: Option<Vec<u8>>,
    pub extra_chunks: Vec<GlbChunk>,
}

pub fn is_glb_bytes(bytes: &[u8]) -> bool {
    if bytes.len() < 4 {
        return false;
    }
    let mut magic = [0_u8; 4];
    magic.copy_from_slice(&bytes[0..4]);
    u32::from_le_bytes(magic) == GLB_MAGIC
}

pub fn parse_gltf_json(json: &str) -> Result<GltfDocument, GltfError> {
    let document = <GltfDocument as DeJson>::deserialize_json_lenient(json).map_err(GltfError::Json)?;
    validate_document(&document)?;
    Ok(document)
}

pub fn parse_glb_bytes(bytes: &[u8]) -> Result<ParsedGlb, GltfError> {
    if bytes.len() < 12 {
        return Err(GltfError::InvalidGlb("header is shorter than 12 bytes".to_string()));
    }
    let magic = read_u32(bytes, 0)?;
    if magic != GLB_MAGIC {
        return Err(GltfError::InvalidGlb("bad magic, expected 'glTF'".to_string()));
    }

    let version = read_u32(bytes, 4)?;
    if version != 2 {
        return Err(GltfError::InvalidGlb(format!(
            "unsupported glb version {version}, only version 2 is supported"
        )));
    }

    let declared_length = read_u32(bytes, 8)? as usize;
    if declared_length != bytes.len() {
        return Err(GltfError::InvalidGlb(format!(
            "declared length {declared_length} does not match actual length {}",
            bytes.len()
        )));
    }

    let mut offset = 12;
    let json_len = read_u32(bytes, offset)? as usize;
    let json_type = read_u32(bytes, offset + 4)?;
    if json_type != GLB_JSON_CHUNK {
        return Err(GltfError::InvalidGlb(
            "first chunk must be JSON for glTF 2.0".to_string(),
        ));
    }
    offset += 8;
    let json_end = offset + json_len;
    if json_end > bytes.len() {
        return Err(GltfError::InvalidGlb(
            "json chunk overruns file bounds".to_string(),
        ));
    }

    let json_raw = std::str::from_utf8(&bytes[offset..json_end])?;
    let json_trimmed = json_raw.trim_end_matches(['\0', ' ', '\n', '\r', '\t']);
    let document = parse_gltf_json(json_trimmed)?;
    offset = json_end;

    let mut bin_chunk = None;
    let mut extra_chunks = Vec::new();
    while offset < bytes.len() {
        if offset + 8 > bytes.len() {
            return Err(GltfError::InvalidGlb(
                "trailing chunk header overruns file bounds".to_string(),
            ));
        }
        let chunk_len = read_u32(bytes, offset)? as usize;
        let chunk_type = read_u32(bytes, offset + 4)?;
        offset += 8;

        let chunk_end = offset + chunk_len;
        if chunk_end > bytes.len() {
            return Err(GltfError::InvalidGlb(
                "trailing chunk data overruns file bounds".to_string(),
            ));
        }

        let data = bytes[offset..chunk_end].to_vec();
        if chunk_type == GLB_BIN_CHUNK && bin_chunk.is_none() {
            bin_chunk = Some(data);
        } else {
            extra_chunks.push(GlbChunk { chunk_type, data });
        }

        offset = chunk_end;
    }

    Ok(ParsedGlb {
        version,
        document,
        bin_chunk,
        extra_chunks,
    })
}

pub fn validate_document(document: &GltfDocument) -> Result<(), GltfError> {
    if let Some(required_extensions) = &document.extensions_required {
        for required in required_extensions {
            if required == "EXT_meshopt_compression" {
                return Err(GltfError::Unsupported(
                    "required extension 'EXT_meshopt_compression' is not yet supported".to_string(),
                ));
            }
        }
    }

    if !document.asset.version.starts_with("2.") {
        return Err(GltfError::Validation(format!(
            "unsupported glTF asset version '{}'",
            document.asset.version
        )));
    }

    let scene_count = document.scenes_slice().len();
    let node_count = document.nodes_slice().len();
    let mesh_count = document.meshes_slice().len();
    let accessor_count = document.accessors_slice().len();
    let buffer_view_count = document.buffer_views_slice().len();
    let buffer_count = document.buffers_slice().len();
    let material_count = document.materials_slice().len();
    let texture_count = document.textures_slice().len();
    let image_count = document.images_slice().len();
    let sampler_count = document.samplers_slice().len();
    let camera_count = document.cameras_slice().len();
    let skin_count = document.skins.as_deref().unwrap_or(&[]).len();

    if let Some(scene_index) = document.scene {
        ensure_index("scene", scene_index, scene_count)?;
    }

    for (scene_i, scene) in document.scenes_slice().iter().enumerate() {
        if let Some(nodes) = &scene.nodes {
            for &node_index in nodes {
                ensure_index(&format!("scenes[{scene_i}].nodes"), node_index, node_count)?;
            }
        }
    }

    for (node_i, node) in document.nodes_slice().iter().enumerate() {
        if let Some(mesh_index) = node.mesh {
            ensure_index(&format!("nodes[{node_i}].mesh"), mesh_index, mesh_count)?;
        }
        if let Some(camera_index) = node.camera {
            ensure_index(&format!("nodes[{node_i}].camera"), camera_index, camera_count)?;
        }
        if let Some(skin_index) = node.skin {
            ensure_index(&format!("nodes[{node_i}].skin"), skin_index, skin_count)?;
        }
        if let Some(children) = &node.children {
            for &child_index in children {
                ensure_index(&format!("nodes[{node_i}].children"), child_index, node_count)?;
            }
        }
    }

    for (mesh_i, mesh) in document.meshes_slice().iter().enumerate() {
        for (primitive_i, primitive) in mesh.primitives.iter().enumerate() {
            if let Some(indices_index) = primitive.indices {
                ensure_index(
                    &format!("meshes[{mesh_i}].primitives[{primitive_i}].indices"),
                    indices_index,
                    accessor_count,
                )?;
            }
            if let Some(material_index) = primitive.material {
                ensure_index(
                    &format!("meshes[{mesh_i}].primitives[{primitive_i}].material"),
                    material_index,
                    material_count,
                )?;
            }
            for (semantic, accessor_index) in &primitive.attributes {
                ensure_index(
                    &format!("meshes[{mesh_i}].primitives[{primitive_i}].attributes.{semantic}"),
                    *accessor_index,
                    accessor_count,
                )?;
            }
            if let Some(targets) = &primitive.targets {
                for (target_i, target) in targets.iter().enumerate() {
                    for (semantic, accessor_index) in target {
                        ensure_index(
                            &format!(
                                "meshes[{mesh_i}].primitives[{primitive_i}].targets[{target_i}].{semantic}"
                            ),
                            *accessor_index,
                            accessor_count,
                        )?;
                    }
                }
            }
        }
    }

    for (accessor_i, accessor) in document.accessors_slice().iter().enumerate() {
        if let Some(buffer_view_index) = accessor.buffer_view {
            ensure_index(
                &format!("accessors[{accessor_i}].bufferView"),
                buffer_view_index,
                buffer_view_count,
            )?;
        }
        if let Some(sparse) = &accessor.sparse {
            ensure_index(
                &format!("accessors[{accessor_i}].sparse.indices.bufferView"),
                sparse.indices.buffer_view,
                buffer_view_count,
            )?;
            ensure_index(
                &format!("accessors[{accessor_i}].sparse.values.bufferView"),
                sparse.values.buffer_view,
                buffer_view_count,
            )?;
        }
    }

    for (buffer_view_i, buffer_view) in document.buffer_views_slice().iter().enumerate() {
        ensure_index(
            &format!("bufferViews[{buffer_view_i}].buffer"),
            buffer_view.buffer,
            buffer_count,
        )?;
    }

    for (material_i, material) in document.materials_slice().iter().enumerate() {
        if let Some(pbr) = &material.pbr_metallic_roughness {
            if let Some(info) = &pbr.base_color_texture {
                ensure_index(
                    &format!(
                        "materials[{material_i}].pbrMetallicRoughness.baseColorTexture.index"
                    ),
                    info.index,
                    texture_count,
                )?;
            }
            if let Some(info) = &pbr.metallic_roughness_texture {
                ensure_index(
                    &format!(
                        "materials[{material_i}].pbrMetallicRoughness.metallicRoughnessTexture.index"
                    ),
                    info.index,
                    texture_count,
                )?;
            }
        }
        if let Some(info) = &material.normal_texture {
            ensure_index(
                &format!("materials[{material_i}].normalTexture.index"),
                info.index,
                texture_count,
            )?;
        }
        if let Some(info) = &material.occlusion_texture {
            ensure_index(
                &format!("materials[{material_i}].occlusionTexture.index"),
                info.index,
                texture_count,
            )?;
        }
        if let Some(info) = &material.emissive_texture {
            ensure_index(
                &format!("materials[{material_i}].emissiveTexture.index"),
                info.index,
                texture_count,
            )?;
        }
    }

    for (texture_i, texture) in document.textures_slice().iter().enumerate() {
        if let Some(sampler_index) = texture.sampler {
            ensure_index(
                &format!("textures[{texture_i}].sampler"),
                sampler_index,
                sampler_count,
            )?;
        }
        if let Some(source_index) = texture.source {
            ensure_index(
                &format!("textures[{texture_i}].source"),
                source_index,
                image_count,
            )?;
        }
    }

    for (image_i, image) in document.images_slice().iter().enumerate() {
        if let Some(buffer_view_index) = image.buffer_view {
            ensure_index(
                &format!("images[{image_i}].bufferView"),
                buffer_view_index,
                buffer_view_count,
            )?;
        }
    }

    Ok(())
}

fn ensure_index(label: &str, index: usize, len: usize) -> Result<(), GltfError> {
    if index >= len {
        return Err(GltfError::Validation(format!(
            "{label} index {index} is out of bounds (len {len})"
        )));
    }
    Ok(())
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, GltfError> {
    if offset + 4 > bytes.len() {
        return Err(GltfError::InvalidGlb(format!(
            "u32 read at byte offset {offset} overruns file bounds"
        )));
    }
    let mut v = [0_u8; 4];
    v.copy_from_slice(&bytes[offset..offset + 4]);
    Ok(u32::from_le_bytes(v))
}

#[cfg(test)]
mod tests {
    use super::{parse_glb_bytes, GLB_BIN_CHUNK, GLB_JSON_CHUNK, GLB_MAGIC};

    #[test]
    fn parses_minimal_glb() {
        let json = br#"{"asset":{"version":"2.0"},"buffers":[{"byteLength":4}]}"#;
        let json_padded_len = (json.len() + 3) & !3;
        let bin = [1_u8, 2, 3, 4];

        let total_len = 12 + 8 + json_padded_len + 8 + bin.len();
        let mut glb = Vec::with_capacity(total_len);
        glb.extend_from_slice(&GLB_MAGIC.to_le_bytes());
        glb.extend_from_slice(&2_u32.to_le_bytes());
        glb.extend_from_slice(&(total_len as u32).to_le_bytes());

        glb.extend_from_slice(&(json_padded_len as u32).to_le_bytes());
        glb.extend_from_slice(&GLB_JSON_CHUNK.to_le_bytes());
        glb.extend_from_slice(json);
        glb.extend(std::iter::repeat(b' ').take(json_padded_len - json.len()));

        glb.extend_from_slice(&(bin.len() as u32).to_le_bytes());
        glb.extend_from_slice(&GLB_BIN_CHUNK.to_le_bytes());
        glb.extend_from_slice(&bin);

        let parsed = parse_glb_bytes(&glb).expect("must parse");
        assert_eq!(parsed.version, 2);
        assert_eq!(parsed.document.asset.version, "2.0");
        assert_eq!(parsed.bin_chunk.as_ref().expect("bin").len(), 4);
        assert!(parsed.extra_chunks.is_empty());
    }
}
