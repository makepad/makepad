//! Optional oracle fixture reader used only by tests.

use std::path::{Path, PathBuf};

use crate::mhr::MhrRig;
use crate::weights::BodyWeights;

pub fn oracle_dir() -> Option<PathBuf> {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .map(|root| root.join("local/agent_state/sam3dbody/oracle"))
        .find(|candidate| candidate.is_dir())
}

/// Load `<name>.f32` (or `<name>.u8`, widened) and its shape from the
/// oracle manifest.
pub fn load(name: &str) -> Option<(Vec<usize>, Vec<f32>)> {
    let root = oracle_dir()?;
    let manifest = std::fs::read_to_string(root.join("manifest.json")).ok()?;
    let shape = manifest_shape(&manifest, name)?;
    let f32_path = root.join(format!("{name}.f32"));
    let values: Vec<f32> = if f32_path.is_file() {
        let bytes = std::fs::read(f32_path).ok()?;
        if bytes.len() % 4 != 0 {
            return None;
        }
        bytes
            .chunks_exact(4)
            .map(|bytes| f32::from_le_bytes(bytes.try_into().unwrap()))
            .collect()
    } else {
        std::fs::read(root.join(format!("{name}.u8")))
            .ok()?
            .into_iter()
            .map(f32::from)
            .collect()
    };
    let expected = shape.iter().try_fold(1usize, |count, &dimension| {
        count.checked_mul(dimension)
    })?;
    (values.len() == expected).then_some((shape, values))
}

pub fn weights_path() -> Option<PathBuf> {
    let root = oracle_dir()?;
    let value = std::fs::read_to_string(root.join("weights_path.txt")).ok()?;
    let path = PathBuf::from(value.trim());
    let path = if path.is_absolute() { path } else { root.join(path) };
    path.is_file().then_some(path)
}

/// The rig loaded from the weights (a fresh load each call: the rig holds
/// GPU tensors, which cannot sit in a static).
pub fn rig() -> Option<MhrRig> {
    let path = weights_path()?;
    let weights = match BodyWeights::load(path) {
        Ok(weights) => weights,
        Err(err) => {
            eprintln!("fixture rig: weights failed to load: {err:?}");
            return None;
        }
    };
    match MhrRig::load(&weights) {
        Ok(rig) => Some(rig),
        Err(err) => {
            eprintln!("fixture rig: MHR rig failed to load: {err:?}");
            None
        }
    }
}

/// The GPU tests need a device AND the layer-norm op family; a build without
/// either skips them.
pub fn gpu_required_ops_available() -> bool {
    let Ok(input) = crate::backend::gpu_upload(&[0.0], 1, 1) else {
        return false;
    };
    crate::backend::gpu_layer_norm_mul_add(&input, &[1.0], &[0.0], 1e-5).is_ok()
}

fn manifest_shape(manifest: &str, name: &str) -> Option<Vec<usize>> {
    let key = format!("\"{name}\"");
    let entry = manifest.get(manifest.find(&key)? + key.len()..)?;
    let entry = entry.get(entry.find(':')? + 1..)?.trim_start();

    // Accepted oracle encodings are `[[dims], "f32"]` and
    // `{"shape":[dims], "dtype":"f32"}`.
    let shape_text = if entry.starts_with('[') {
        let nested = entry.get(1..)?.trim_start();
        nested.get(nested.find('[')? + 1..)?
    } else {
        let shape_key = entry.find("\"shape\"")?;
        let after_key = entry.get(shape_key + "\"shape\"".len()..)?;
        after_key.get(after_key.find('[')? + 1..)?
    };
    let shape_text = shape_text.get(..shape_text.find(']')?)?;
    if shape_text.trim().is_empty() {
        return Some(Vec::new());
    }
    shape_text
        .split(',')
        .map(|value| value.trim().parse::<usize>().ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_both_supported_manifest_encodings() {
        let pairs = r#"{"x":[[1, 45],"f32"]}"#;
        assert_eq!(manifest_shape(pairs, "x"), Some(vec![1, 45]));
        let object = r#"{"x":{"shape":[2,3,4],"dtype":"float32"}}"#;
        assert_eq!(manifest_shape(object, "x"), Some(vec![2, 3, 4]));
    }
}
