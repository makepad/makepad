use crate::{decode_mesh_primitive, load_gltf_from_path, GltfError, GLTF_MODE_TRIANGLES};
use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
};

fn sample_models_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/gltf/resources/glTF-Sample-Models")
}

fn collect_gltf_paths(root: &Path) -> io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    collect_gltf_paths_rec(root, &mut out)?;
    out.sort();
    Ok(out)
}

fn collect_gltf_paths_rec(dir: &Path, out: &mut Vec<PathBuf>) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_gltf_paths_rec(&path, out)?;
        } else if let Some(ext) = path.extension().and_then(|v| v.to_str()) {
            if ext.eq_ignore_ascii_case("gltf") || ext.eq_ignore_ascii_case("glb") {
                out.push(path);
            }
        }
    }
    Ok(())
}

fn is_legacy_gltf_1_0(path: &Path) -> bool {
    path.components().any(|c| c.as_os_str() == "1.0")
}

fn categorize_unsupported_reason(msg: &str) -> String {
    if msg.contains("EXT_meshopt_compression") {
        "required extension EXT_meshopt_compression".to_string()
    } else if msg.contains("sparse accessors are not yet supported") {
        "sparse accessors".to_string()
    } else if msg.contains("has no bufferView") {
        "accessor without bufferView".to_string()
    } else if msg.contains("f32x3 requires FLOAT") {
        "non-float POSITION/NORMAL (quantized vec3)".to_string()
    } else if msg.contains("f32x2 requires FLOAT") {
        "non-float TEXCOORD_0 (quantized vec2)".to_string()
    } else if msg.contains("uses unsupported component type") {
        "unsupported accessor component type".to_string()
    } else if msg.contains("uses unsupported accessor type") {
        "unsupported accessor type".to_string()
    } else if msg.contains("component type") && msg.contains("unsupported") {
        "unsupported component type".to_string()
    } else {
        format!("other: {msg}")
    }
}

fn reason_counts_desc(reason_counts: &BTreeMap<String, usize>) -> Vec<(&str, usize)> {
    let mut out: Vec<(&str, usize)> = reason_counts
        .iter()
        .map(|(reason, count)| (reason.as_str(), *count))
        .collect();
    out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
    out
}

#[test]
fn loads_all_gltf_sample_models_if_available() {
    let root = sample_models_root();
    if !root.exists() {
        return;
    }

    let files = collect_gltf_paths(&root).expect("must walk sample-model directory");
    assert!(
        !files.is_empty(),
        "no .gltf or .glb files found under {}",
        root.display()
    );

    let mut loaded_ok = 0_usize;
    let mut skipped_legacy_1_0 = 0_usize;
    let mut skipped_unsupported_version = 0_usize;
    let mut skipped_unsupported_feature = 0_usize;
    let mut failures: Vec<(PathBuf, String)> = Vec::new();

    for path in files {
        if is_legacy_gltf_1_0(&path) {
            skipped_legacy_1_0 += 1;
            continue;
        }

        match load_gltf_from_path(&path) {
            Ok(_) => {
                loaded_ok += 1;
            }
            Err(GltfError::Validation(msg)) if msg.starts_with("unsupported glTF asset version") => {
                skipped_unsupported_version += 1;
            }
            Err(GltfError::Unsupported(_)) => {
                skipped_unsupported_feature += 1;
            }
            Err(err) => {
                failures.push((path, err.to_string()));
            }
        }
    }

    eprintln!(
        "gltf sample scan: loaded_ok={loaded_ok}, skipped_legacy_1_0={skipped_legacy_1_0}, skipped_unsupported_version={skipped_unsupported_version}, skipped_unsupported_feature={skipped_unsupported_feature}, failures={}",
        failures.len()
    );

    if !failures.is_empty() {
        let mut summary = String::new();
        for (i, (path, err)) in failures.iter().take(30).enumerate() {
            summary.push_str(&format!("{:02}. {} => {}\n", i + 1, path.display(), err));
        }
        panic!(
            "failed to load {} sample files (showing first {}):\n{}",
            failures.len(),
            failures.len().min(30),
            summary
        );
    }
}

#[test]
fn decodes_triangle_primitives_from_all_gltf_2_samples_if_available() {
    let root = sample_models_root();
    if !root.exists() {
        return;
    }

    let files = collect_gltf_paths(&root).expect("must walk sample-model directory");
    assert!(
        !files.is_empty(),
        "no .gltf or .glb files found under {}",
        root.display()
    );

    let mut loaded_ok = 0_usize;
    let mut skipped_legacy_1_0 = 0_usize;
    let mut skipped_unsupported_container = 0_usize;
    let mut unsupported_container_reasons: BTreeMap<String, usize> = BTreeMap::new();
    let mut decoded_ok = 0_usize;
    let mut skipped_non_triangles = 0_usize;
    let mut skipped_unsupported_decode = 0_usize;
    let mut unsupported_decode_reasons: BTreeMap<String, usize> = BTreeMap::new();
    let mut failures: Vec<(PathBuf, String)> = Vec::new();

    for path in files {
        if is_legacy_gltf_1_0(&path) {
            skipped_legacy_1_0 += 1;
            continue;
        }

        let loaded = match load_gltf_from_path(&path) {
            Ok(loaded) => loaded,
            Err(GltfError::Unsupported(msg)) => {
                skipped_unsupported_container += 1;
                let key = categorize_unsupported_reason(&msg);
                *unsupported_container_reasons.entry(key).or_insert(0) += 1;
                continue;
            }
            Err(err) => {
                failures.push((path.clone(), format!("load failed: {err}")));
                continue;
            }
        };

        loaded_ok += 1;
        for mesh_index in 0..loaded.document.meshes_slice().len() {
            let primitive_count = loaded.document.meshes_slice()[mesh_index].primitives.len();
            for primitive_index in 0..primitive_count {
                let primitive = &loaded.document.meshes_slice()[mesh_index].primitives[primitive_index];
                if primitive.mode() != GLTF_MODE_TRIANGLES {
                    skipped_non_triangles += 1;
                    continue;
                }

                match decode_mesh_primitive(&loaded, mesh_index, primitive_index) {
                    Ok(decoded) => {
                        // Sanity check for decoded streams.
                        if decoded.positions.is_empty() {
                            failures.push((
                                path.clone(),
                                format!(
                                    "decoded empty POSITION stream at mesh {mesh_index} primitive {primitive_index}"
                                ),
                            ));
                        } else {
                            decoded_ok += 1;
                        }
                    }
                    Err(GltfError::Unsupported(msg)) => {
                        skipped_unsupported_decode += 1;
                        let key = categorize_unsupported_reason(&msg);
                        *unsupported_decode_reasons.entry(key).or_insert(0) += 1;
                    }
                    Err(err) => {
                        failures.push((
                            path.clone(),
                            format!(
                                "decode failed at mesh {mesh_index} primitive {primitive_index}: {err}"
                            ),
                        ));
                    }
                }
            }
        }
    }

    eprintln!(
        "gltf decode sweep: loaded_ok={loaded_ok}, skipped_legacy_1_0={skipped_legacy_1_0}, skipped_unsupported_container={skipped_unsupported_container}, decoded_ok={decoded_ok}, skipped_non_triangles={skipped_non_triangles}, skipped_unsupported_decode={skipped_unsupported_decode}, failures={}",
        failures.len()
    );
    if !unsupported_container_reasons.is_empty() {
        eprintln!("unsupported container reasons:");
        for (reason, count) in reason_counts_desc(&unsupported_container_reasons) {
            eprintln!("  {count:>5}  {reason}");
        }
    }
    if !unsupported_decode_reasons.is_empty() {
        eprintln!("unsupported decode reasons:");
        for (reason, count) in reason_counts_desc(&unsupported_decode_reasons) {
            eprintln!("  {count:>5}  {reason}");
        }
    }

    if !failures.is_empty() {
        let mut summary = String::new();
        for (i, (path, err)) in failures.iter().take(30).enumerate() {
            summary.push_str(&format!("{:02}. {} => {}\n", i + 1, path.display(), err));
        }
        panic!(
            "failed to decode {} triangle primitive cases (showing first {}):\n{}",
            failures.len(),
            failures.len().min(30),
            summary
        );
    }

    assert!(
        decoded_ok > 0,
        "decode sweep did not decode any triangle primitives successfully"
    );
}
