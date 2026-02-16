use crate::{error::GltfError, loader::LoadedGltf};
use makepad_base64::base64_decode;
use std::{fs, path::Path};

/// Resolve image payload bytes for a glTF image entry.
/// Supports external files, `data:` URIs, and GLB bufferView-backed images.
pub fn load_image_bytes(loaded: &LoadedGltf, image_index: usize) -> Result<Vec<u8>, GltfError> {
    let image = loaded
        .document
        .images_slice()
        .get(image_index)
        .ok_or_else(|| {
            GltfError::Validation(format!(
                "image index {image_index} is out of bounds (len {})",
                loaded.document.images_slice().len()
            ))
        })?;

    if let Some(uri) = &image.uri {
        return load_uri(uri, loaded.base_dir.as_deref());
    }

    if let Some(buffer_view_index) = image.buffer_view {
        let buffer_view = loaded
            .document
            .buffer_views_slice()
            .get(buffer_view_index)
            .ok_or_else(|| {
                GltfError::Validation(format!(
                    "image[{image_index}] references bufferView {buffer_view_index} out of bounds (len {})",
                    loaded.document.buffer_views_slice().len()
                ))
            })?;

        let buffer = loaded
            .buffers
            .get(buffer_view.buffer)
            .ok_or(GltfError::MissingBuffer {
                index: buffer_view.buffer,
            })?;

        let start = buffer_view.byte_offset.unwrap_or(0);
        let end = start + buffer_view.byte_length;
        if end > buffer.len() {
            return Err(GltfError::Validation(format!(
                "image[{image_index}] bufferView {buffer_view_index} overruns buffer bounds ({end} > {})",
                buffer.len()
            )));
        }
        return Ok(buffer[start..end].to_vec());
    }

    Err(GltfError::Validation(format!(
        "image[{image_index}] has neither uri nor bufferView"
    )))
}

fn load_uri(uri: &str, base_dir: Option<&Path>) -> Result<Vec<u8>, GltfError> {
    if uri.starts_with("data:") {
        return decode_data_uri(uri);
    }

    if let Some(file_uri) = uri.strip_prefix("file://") {
        return Ok(fs::read(Path::new(file_uri))?);
    }

    if uri.contains("://") {
        return Err(GltfError::UnsupportedUri(uri.to_string()));
    }

    let base_dir = base_dir.ok_or_else(|| GltfError::UnsupportedUri(uri.to_string()))?;
    let path = base_dir.join(uri);
    Ok(fs::read(path)?)
}

fn decode_data_uri(uri: &str) -> Result<Vec<u8>, GltfError> {
    let payload = uri
        .strip_prefix("data:")
        .ok_or_else(|| GltfError::DataUri("missing data: prefix".to_string()))?;
    let (meta, data) = payload
        .split_once(',')
        .ok_or_else(|| GltfError::DataUri("missing comma separator".to_string()))?;

    if !meta.ends_with(";base64") {
        return Err(GltfError::DataUri(
            "only base64 data uris are currently supported".to_string(),
        ));
    }

    base64_decode(data.as_bytes())
        .map_err(|_| GltfError::DataUri("base64 decode failed".to_string()))
}

#[cfg(test)]
mod tests {
    use crate::{image::load_image_bytes, loader::load_gltf_from_path};
    use std::path::PathBuf;

    fn damaged_helmet_path(subpath: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!(
            "../../examples/gltf/resources/glTF-Sample-Models/2.0/DamagedHelmet/{subpath}"
        ))
    }

    #[test]
    fn loads_damaged_helmet_external_image_bytes_if_available() {
        let path = damaged_helmet_path("glTF/DamagedHelmet.gltf");
        if !path.exists() {
            return;
        }

        let loaded = load_gltf_from_path(path).expect("must load");
        let bytes = load_image_bytes(&loaded, 0).expect("must load image bytes");
        assert!(!bytes.is_empty());
    }

    #[test]
    fn loads_damaged_helmet_glb_image_bytes_if_available() {
        let path = damaged_helmet_path("glTF-Binary/DamagedHelmet.glb");
        if !path.exists() {
            return;
        }

        let loaded = load_gltf_from_path(path).expect("must load");
        let bytes = load_image_bytes(&loaded, 0).expect("must load image bytes");
        assert!(!bytes.is_empty());
    }
}
