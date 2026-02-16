use crate::{
    document::GltfDocument,
    error::GltfError,
    parser::{is_glb_bytes, parse_glb_bytes, parse_gltf_json},
};
use makepad_base64::base64_decode;
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GltfContainerKind {
    Gltf,
    Glb,
}

#[derive(Clone, Debug)]
pub struct LoadedGltf {
    pub kind: GltfContainerKind,
    pub document: GltfDocument,
    pub buffers: Vec<Vec<u8>>,
    pub source_path: Option<PathBuf>,
    pub base_dir: Option<PathBuf>,
}

pub fn load_gltf_from_path(path: impl AsRef<Path>) -> Result<LoadedGltf, GltfError> {
    let path = path.as_ref();
    let bytes = fs::read(path)?;
    let base_dir = path.parent().map(|p| p.to_path_buf());

    let mut loaded = load_gltf_from_bytes(&bytes, base_dir.as_deref())?;
    loaded.source_path = Some(path.to_path_buf());
    loaded.base_dir = base_dir;
    Ok(loaded)
}

pub fn load_gltf_from_bytes(bytes: &[u8], base_dir: Option<&Path>) -> Result<LoadedGltf, GltfError> {
    if is_glb_bytes(bytes) {
        let parsed = parse_glb_bytes(bytes)?;
        let buffers = resolve_buffers(&parsed.document, base_dir, parsed.bin_chunk.as_deref())?;
        return Ok(LoadedGltf {
            kind: GltfContainerKind::Glb,
            document: parsed.document,
            buffers,
            source_path: None,
            base_dir: base_dir.map(|p| p.to_path_buf()),
        });
    }

    let json = std::str::from_utf8(bytes)?;
    let document = parse_gltf_json(json)?;
    let buffers = resolve_buffers(&document, base_dir, None)?;
    Ok(LoadedGltf {
        kind: GltfContainerKind::Gltf,
        document,
        buffers,
        source_path: None,
        base_dir: base_dir.map(|p| p.to_path_buf()),
    })
}

fn resolve_buffers(
    document: &GltfDocument,
    base_dir: Option<&Path>,
    glb_bin_chunk: Option<&[u8]>,
) -> Result<Vec<Vec<u8>>, GltfError> {
    let mut out = Vec::with_capacity(document.buffers_slice().len());
    for (i, buffer) in document.buffers_slice().iter().enumerate() {
        let bytes = match &buffer.uri {
            Some(uri) => load_uri(uri, base_dir)?,
            None => {
                if i == 0 {
                    if let Some(bin) = glb_bin_chunk {
                        bin.to_vec()
                    } else {
                        return Err(GltfError::MissingBuffer { index: i });
                    }
                } else {
                    return Err(GltfError::MissingBuffer { index: i });
                }
            }
        };
        if bytes.len() < buffer.byte_length {
            return Err(GltfError::Validation(format!(
                "buffer[{i}] byteLength={} but loaded {} bytes",
                buffer.byte_length,
                bytes.len()
            )));
        }
        out.push(bytes);
    }
    Ok(out)
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
    use super::{load_gltf_from_path, GltfContainerKind};
    use std::path::PathBuf;

    fn damaged_helmet_path(subpath: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(
            format!(
                "../../examples/gltf/resources/glTF-Sample-Models/2.0/DamagedHelmet/{subpath}"
            ),
        )
    }

    #[test]
    fn loads_damaged_helmet_gltf_if_available() {
        let path = damaged_helmet_path("glTF/DamagedHelmet.gltf");
        if !path.exists() {
            return;
        }

        let loaded = load_gltf_from_path(&path).expect("must parse gltf");
        assert_eq!(loaded.kind, GltfContainerKind::Gltf);
        assert_eq!(loaded.document.asset.version, "2.0");
        assert_eq!(loaded.buffers.len(), 1);
        assert!(!loaded.buffers[0].is_empty());
        assert_eq!(loaded.document.meshes_slice().len(), 1);
        assert_eq!(loaded.document.meshes_slice()[0].primitives[0].mode(), 4);
    }

    #[test]
    fn loads_damaged_helmet_glb_if_available() {
        let path = damaged_helmet_path("glTF-Binary/DamagedHelmet.glb");
        if !path.exists() {
            return;
        }

        let loaded = load_gltf_from_path(&path).expect("must parse glb");
        assert_eq!(loaded.kind, GltfContainerKind::Glb);
        assert_eq!(loaded.document.asset.version, "2.0");
        assert_eq!(loaded.buffers.len(), 1);
        assert!(!loaded.buffers[0].is_empty());
        assert_eq!(loaded.document.meshes_slice().len(), 1);
        assert_eq!(loaded.document.meshes_slice()[0].primitives[0].mode(), 4);
    }
}
