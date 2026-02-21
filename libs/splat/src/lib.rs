mod error;
mod model;
mod ply;
mod sog;

pub use crate::error::SplatError;
pub use crate::model::{Splat, SplatFileFormat, SplatHigherOrderSh, SplatScene};

use std::{
    fs,
    path::{Path, PathBuf},
};

pub fn load_splat_from_path(path: impl AsRef<Path>) -> Result<SplatScene, SplatError> {
    let path = path.as_ref();
    let bytes = fs::read(path)?;
    load_splat_from_bytes(&bytes, Some(path))
}

pub fn load_splat_from_bytes(
    bytes: &[u8],
    source_path_hint: Option<&Path>,
) -> Result<SplatScene, SplatError> {
    let format = detect_format(bytes, source_path_hint)?;
    match format {
        SplatFileFormat::Ply => ply::load_ply_from_bytes(bytes),
        SplatFileFormat::Sog => sog::load_sog_from_bytes(bytes),
    }
}

fn detect_format(
    bytes: &[u8],
    source_path_hint: Option<&Path>,
) -> Result<SplatFileFormat, SplatError> {
    if let Some(path) = source_path_hint {
        if let Some(ext) = path.extension().and_then(|ext| ext.to_str()) {
            let ext = ext.to_ascii_lowercase();
            if ext == "ply" {
                return Ok(SplatFileFormat::Ply);
            }
            if ext == "sog" {
                return Ok(SplatFileFormat::Sog);
            }
        }

        // Handle compressed.ply naming.
        let path_name = PathBuf::from(path)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if path_name.ends_with(".ply") {
            return Ok(SplatFileFormat::Ply);
        }
    }

    if bytes.starts_with(b"ply\n") || bytes.starts_with(b"ply\r\n") {
        return Ok(SplatFileFormat::Ply);
    }
    if bytes.starts_with(b"PK\x03\x04") {
        return Ok(SplatFileFormat::Sog);
    }

    Err(SplatError::Unsupported(
        "could not detect splat format (expected .ply or .sog)".to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::load_splat_from_path;
    use std::path::PathBuf;

    fn local_sample(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../local")
            .join(name)
    }

    #[test]
    fn loads_sample_ply_if_available() {
        let path = local_sample("biker.ply");
        if !path.exists() {
            return;
        }
        let scene = load_splat_from_path(&path).expect("sample ply should load");
        assert!(!scene.splats.is_empty());
    }

    #[test]
    fn loads_sample_sog_if_available() {
        let path = local_sample("toy-cat.sog");
        if !path.exists() {
            return;
        }
        let scene = load_splat_from_path(&path).expect("sample sog should load");
        assert!(!scene.splats.is_empty());
    }
}
