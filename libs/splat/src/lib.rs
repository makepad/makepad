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
    use super::{load_splat_from_bytes, load_splat_from_path};
    use std::path::PathBuf;

    fn local_sample(name: &str) -> Option<PathBuf> {
        // `local/` lives in the main checkout; worktrees reach it four levels up.
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let mut candidates = Vec::new();
        if let Ok(dir) = std::env::var("MAKEPAD_LOCAL_DIR") {
            candidates.push(PathBuf::from(dir));
        }
        candidates.push(manifest.join("../../local"));
        candidates.push(manifest.join("../../../../../local"));
        candidates
            .into_iter()
            .map(|dir| dir.join(name))
            .find(|path| path.exists())
    }

    #[test]
    fn loads_sample_ply_if_available() {
        let Some(path) = local_sample("biker.ply") else {
            return;
        };
        let scene = load_splat_from_path(&path).expect("sample ply should load");
        assert!(!scene.splats.is_empty());
    }

    #[test]
    fn loads_sample_sog_if_available() {
        let Some(path) = local_sample("toy-cat.sog") else {
            return;
        };
        let scene = load_splat_from_path(&path).expect("sample sog should load");
        assert!(!scene.splats.is_empty());
    }

    /// Binary rows with mixed scalar types, extra (skipped) properties and a
    /// property order that differs from the field order decode to the same
    /// splats as the ascii encoding of the same values.
    #[test]
    fn binary_and_ascii_ply_decode_identically() {
        let header = |format: &str| {
            format!(
                "ply\nformat {format} 1.0\ncomment test\nelement vertex 3\n\
                 property float x\nproperty double y\nproperty float z\n\
                 property uchar junk\nproperty float f_dc_0\nproperty float f_dc_1\n\
                 property float f_dc_2\nproperty float f_rest_0\nproperty float opacity\n\
                 property float scale_0\nproperty float scale_1\nproperty float scale_2\n\
                 property float rot_0\nproperty float rot_1\nproperty float rot_2\n\
                 property float rot_3\nend_header\n"
            )
        };
        let rows: [[f64; 16]; 3] = [
            [0.5, -1.25, 3.0, 7.0, 0.1, 0.2, 0.3, 9.0, 1.5, -3.0, -2.0, -1.0, 1.0, 0.0, 0.0, 0.0],
            [-2.0, 0.0, 1e-3, 255.0, -0.4, 0.9, 0.0, 0.0, -4.0, -5.0, -5.5, -6.0, 0.7, 0.1, 0.2, 0.3],
            [1e3, 2e3, -3e3, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, -7.0, -7.0, -7.0, 0.0, 0.0, 0.0, 0.0],
        ];
        let mut ascii = header("ascii").into_bytes();
        let mut binary = header("binary_little_endian").into_bytes();
        for row in &rows {
            let line: Vec<String> = row.iter().map(|v| format!("{v}")).collect();
            ascii.extend_from_slice(line.join(" ").as_bytes());
            ascii.push(b'\n');
            for (i, v) in row.iter().enumerate() {
                match i {
                    1 => binary.extend_from_slice(&v.to_le_bytes()),
                    3 => binary.push(*v as u8),
                    _ => binary.extend_from_slice(&(*v as f32).to_le_bytes()),
                }
            }
        }
        let a = load_splat_from_bytes(&ascii, None).expect("ascii");
        let b = load_splat_from_bytes(&binary, None).expect("binary");
        assert_eq!(a.splats.len(), 3);
        assert_eq!(b.splats.len(), 3);
        for (sa, sb) in a.splats.iter().zip(&b.splats) {
            assert_eq!(sa.position, sb.position);
            assert_eq!(sa.scale, sb.scale);
            assert_eq!(sa.rotation, sb.rotation);
            assert_eq!(sa.color, sb.color);
        }
        // Spot-check the decode itself (not just agreement).
        assert_eq!(b.splats[0].position, [0.5, -1.25, 3.0]);
        assert!((b.splats[0].scale[0] - (-3.0f32).exp()).abs() < 1e-7);
        assert_eq!(b.splats[0].rotation, [0.0, 0.0, 0.0, 1.0]);
        assert!((b.splats[1].color[3] - 1.0 / (1.0 + 4.0f32.exp())).abs() < 1e-6);
        assert_eq!(b.bounds_min, [-2.0, -1.25, -3e3]);
        assert_eq!(b.bounds_max, [1e3, 2e3, 3.0]);
    }

    #[test]
    fn truncated_binary_ply_is_an_error() {
        let mut bytes = b"ply\nformat binary_little_endian 1.0\nelement vertex 2\n\
            property float x\nproperty float y\nproperty float z\nend_header\n"
            .to_vec();
        bytes.extend_from_slice(&[0u8; 20]); // 24 needed
        assert!(load_splat_from_bytes(&bytes, None).is_err());
    }

    /// cargo test -p makepad-splat --release loader_bench -- --ignored --nocapture
    #[test]
    #[ignore]
    fn loader_bench() {
        for name in ["biker.ply", "coastal_world.ply", "toy-cat.sog"] {
            let Some(path) = local_sample(name) else {
                println!("{name}: not present, skipped");
                continue;
            };
            let bytes = std::fs::read(&path).expect("read sample");
            // Warm once, then time.
            let _ = load_splat_from_bytes(&bytes, Some(&path));
            let runs = 3;
            let started = std::time::Instant::now();
            let mut count = 0;
            for _ in 0..runs {
                count = load_splat_from_bytes(&bytes, Some(&path))
                    .expect("load")
                    .splats
                    .len();
            }
            println!(
                "LOADER_BENCH {name}: splats={count} load_ms={:.1} ({:.1} MB)",
                started.elapsed().as_secs_f64() * 1000.0 / runs as f64,
                bytes.len() as f64 / 1e6
            );
        }
    }
}
