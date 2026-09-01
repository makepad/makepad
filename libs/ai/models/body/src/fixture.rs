//! Optional oracle fixture reader used only by tests.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::mhr::MhrRig;
use crate::weights::BodyWeights;

pub fn oracle_dir() -> Option<PathBuf> {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .map(|root| root.join("local/agent_state/sam3dbody/oracle"))
        .find(|candidate| candidate.is_dir())
}

/// Load `<name>.f32` and its shape from the oracle manifest.
pub fn load(name: &str) -> Option<(Vec<usize>, Vec<f32>)> {
    let root = oracle_dir()?;
    let manifest = std::fs::read_to_string(root.join("manifest.json")).ok()?;
    let shape = manifest_shape(&manifest, name)?;
    let bytes = std::fs::read(root.join(format!("{name}.f32"))).ok()?;
    if bytes.len() % 4 != 0 {
        return None;
    }
    let values: Vec<f32> = bytes
        .chunks_exact(4)
        .map(|bytes| f32::from_le_bytes(bytes.try_into().unwrap()))
        .collect();
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

pub fn rig() -> Option<&'static MhrRig> {
    static RIG: OnceLock<Option<MhrRig>> = OnceLock::new();
    RIG.get_or_init(|| {
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
    })
    .as_ref()
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
