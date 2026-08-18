//! Exact HY-Motion latent/wooden decoder validation against oracle dumps.
//!
//! Usage:
//!   hy-motion-decode-validate <latest.ckpt> <oracle-npy-dir> <dump_wooden-dir>

use std::path::{Path, PathBuf};
use std::time::Instant;

use makepad_diffusion::hy_motion::{HY_MOTION_BODY_JOINTS, HY_MOTION_INPUT_DIM};
use makepad_diffusion::hy_motion_decode::{
    hy_motion_denormalize, HyMotionWoodenModel, HY_MOTION_WOODEN_JOINTS,
};
use makepad_diffusion::hy_motion_weights::HyMotionCheckpoint;

struct Npy {
    shape: Vec<usize>,
    descr: String,
    data: Vec<u8>,
}

fn load_npy(path: &Path) -> Result<Npy, String> {
    let bytes = std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    if bytes.len() < 12 || &bytes[..6] != b"\x93NUMPY" {
        return Err(format!("{}: not an npy file", path.display()));
    }
    let major = bytes[6];
    let (header_len, header_start) = match major {
        1 => (u16::from_le_bytes([bytes[8], bytes[9]]) as usize, 10),
        2 | 3 => (
            u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize,
            12,
        ),
        _ => return Err(format!("{}: unsupported npy version {major}", path.display())),
    };
    let data_start = header_start + header_len;
    if data_start > bytes.len() {
        return Err(format!("{}: truncated npy header", path.display()));
    }
    let header = String::from_utf8_lossy(&bytes[header_start..data_start]);
    if header.contains("'fortran_order': True") {
        return Err(format!("{}: Fortran-order npy is unsupported", path.display()));
    }
    let descr = header
        .split("'descr':")
        .nth(1)
        .and_then(|rest| rest.split('\'').nth(1))
        .ok_or_else(|| format!("{}: no dtype", path.display()))?
        .to_string();
    let shape_text = header
        .split("'shape':")
        .nth(1)
        .and_then(|rest| rest.split('(').nth(1))
        .and_then(|rest| rest.split(')').next())
        .ok_or_else(|| format!("{}: no shape", path.display()))?;
    let shape: Vec<usize> = shape_text
        .split(',')
        .filter_map(|part| part.trim().parse::<usize>().ok())
        .collect();
    let data = bytes[data_start..].to_vec();
    let expected_bytes = shape.iter().product::<usize>() * 4;
    if data.len() != expected_bytes {
        return Err(format!(
            "{}: {} payload bytes for shape {:?}, expected {expected_bytes}",
            path.display(),
            data.len(),
            shape
        ));
    }
    Ok(Npy { shape, descr, data })
}

impl Npy {
    fn f32(self) -> Result<Vec<f32>, String> {
        if self.descr != "<f4" && self.descr != "=f4" {
            return Err(format!("expected little-endian f32 npy, got {}", self.descr));
        }
        Ok(self
            .data
            .chunks_exact(4)
            .map(|bytes| f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
            .collect())
    }
}

#[derive(Clone, Copy)]
struct Metrics {
    max_abs: f64,
    mean_abs: f64,
    cosine: f64,
}

fn compare(name: &str, actual: &[f32], expected: &[f32]) -> Result<Metrics, String> {
    if actual.len() != expected.len() {
        return Err(format!(
            "{name}: length {} != {}",
            actual.len(),
            expected.len()
        ));
    }
    let mut max_abs = 0.0f64;
    let mut sum_abs = 0.0f64;
    let mut dot = 0.0f64;
    let mut norm_actual = 0.0f64;
    let mut norm_expected = 0.0f64;
    for (&actual, &expected) in actual.iter().zip(expected) {
        let actual = actual as f64;
        let expected = expected as f64;
        let difference = (actual - expected).abs();
        max_abs = max_abs.max(difference);
        sum_abs += difference;
        dot += actual * expected;
        norm_actual += actual * actual;
        norm_expected += expected * expected;
    }
    let metrics = Metrics {
        max_abs,
        mean_abs: sum_abs / actual.len().max(1) as f64,
        cosine: dot / (norm_actual.sqrt() * norm_expected.sqrt()).max(1.0e-30),
    };
    println!(
        "{name}: n={} max_abs={:.9e} mean_abs={:.9e} cosine={:.12}",
        actual.len(),
        metrics.max_abs,
        metrics.mean_abs,
        metrics.cosine
    );
    Ok(metrics)
}

fn load_f32(path: &Path, expected_shape: &[usize]) -> Result<Vec<f32>, String> {
    let npy = load_npy(path)?;
    if npy.shape != expected_shape {
        return Err(format!(
            "{}: shape {:?}, expected {:?}",
            path.display(),
            npy.shape,
            expected_shape
        ));
    }
    npy.f32()
}

fn main() {
    if let Err(error) = run() {
        eprintln!("HY-MOTION-DECODE-VALIDATE-FAILED: {error}");
        std::process::exit(1);
    }
    println!("HY-MOTION-DECODE-VALIDATE-DONE");
}

fn run() -> Result<(), String> {
    let mut arguments = std::env::args().skip(1);
    let usage = "usage: hy-motion-decode-validate <latest.ckpt> <oracle-npy-dir> <dump_wooden-dir>";
    let checkpoint_path = PathBuf::from(arguments.next().ok_or(usage)?);
    let oracle_dir = PathBuf::from(arguments.next().ok_or(usage)?);
    let wooden_dir = PathBuf::from(arguments.next().ok_or(usage)?);
    if arguments.next().is_some() {
        return Err(usage.to_string());
    }

    let sampled_npy = load_npy(&oracle_dir.join("sampled_trimmed.npy"))?;
    let frames = match sampled_npy.shape.as_slice() {
        [frames, HY_MOTION_INPUT_DIM] => *frames,
        [1, frames, HY_MOTION_INPUT_DIM] => *frames,
        _ => {
            return Err(format!(
                "sampled_trimmed.npy: shape {:?}, expected [frames, {HY_MOTION_INPUT_DIM}] or [1, frames, {HY_MOTION_INPUT_DIM}]",
                sampled_npy.shape
            ));
        }
    };
    if frames == 0 {
        return Err(format!(
            "sampled_trimmed.npy has an empty frame axis: {:?}",
            sampled_npy.shape
        ));
    }
    let sampled = sampled_npy.f32()?;

    let checkpoint_started = Instant::now();
    let mut checkpoint =
        HyMotionCheckpoint::open(&checkpoint_path).map_err(|error| error.to_string())?;
    let mean = checkpoint.f32("mean").map_err(|error| error.to_string())?;
    let std = checkpoint.f32("std").map_err(|error| error.to_string())?;
    println!(
        "normalization_load_s={:.6}",
        checkpoint_started.elapsed().as_secs_f64()
    );

    let denormalize_started = Instant::now();
    let latent_denorm = hy_motion_denormalize(&sampled, frames, &mean, &std)
        .map_err(|error| error.to_string())?;
    let denormalize_s = denormalize_started.elapsed().as_secs_f64();
    let expected_denorm = load_f32(
        &oracle_dir.join("decoded.latent_denorm.npy"),
        &[1, frames, HY_MOTION_INPUT_DIM],
    )?;
    let denormalize = compare("decoded.latent_denorm", &latent_denorm, &expected_denorm)?;

    let wooden_load_started = Instant::now();
    let wooden = HyMotionWoodenModel::load(&wooden_dir).map_err(|error| error.to_string())?;
    let wooden_load_s = wooden_load_started.elapsed().as_secs_f64();
    println!(
        "wooden_vertices={} wooden_joints={} wooden_load_s={wooden_load_s:.6}",
        wooden.vertex_count(),
        wooden.joint_count()
    );

    let decode_started = Instant::now();
    let decoded = wooden
        .decode_denormalized(&latent_denorm, frames, true)
        .map_err(|error| error.to_string())?;
    let decode_s = decode_started.elapsed().as_secs_f64();
    let rotations = compare(
        "decoded.rot6d",
        &decoded.rotations_6d,
        &load_f32(
            &oracle_dir.join("decoded.rot6d.npy"),
            &[1, frames, HY_MOTION_BODY_JOINTS, 6],
        )?,
    )?;
    let translations = compare(
        "decoded.transl",
        &decoded.translations,
        &load_f32(&oracle_dir.join("decoded.transl.npy"), &[1, frames, 3])?,
    )?;
    let roots = compare(
        "decoded.root_rotations_mat",
        &decoded.root_rotation_matrices,
        &load_f32(
            &oracle_dir.join("decoded.root_rotations_mat.npy"),
            &[1, frames, 3, 3],
        )?,
    )?;
    if decoded.local_rotation_matrices.len() != frames * HY_MOTION_BODY_JOINTS * 9 {
        return Err(format!(
            "decoded local matrix shape mismatch: {} values",
            decoded.local_rotation_matrices.len()
        ));
    }
    for frame in 0..frames {
        if decoded.local_rotation_matrices
            [frame * HY_MOTION_BODY_JOINTS * 9..frame * HY_MOTION_BODY_JOINTS * 9 + 9]
            != decoded.root_rotation_matrices[frame * 9..frame * 9 + 9]
        {
            return Err(format!(
                "decoded local/root matrix mismatch at frame {frame}"
            ));
        }
    }
    println!(
        "decoded.local_rotation_matrices: n={} root_track=bit_exact",
        decoded.local_rotation_matrices.len()
    );
    let keypoints = compare(
        "decoded.keypoints3d",
        &decoded.keypoints_3d,
        &load_f32(
            &oracle_dir.join("decoded.keypoints3d.npy"),
            &[1, frames, HY_MOTION_WOODEN_JOINTS, 3],
        )?,
    )?;
    println!(
        "frames={frames} denormalize_s={denormalize_s:.6} decode_s={decode_s:.6} total_cpu_s={:.6}",
        denormalize_s + decode_s
    );

    let minimum_cosine = [
        denormalize.cosine,
        rotations.cosine,
        translations.cosine,
        roots.cosine,
        keypoints.cosine,
    ]
    .into_iter()
    .fold(1.0f64, f64::min);
    if minimum_cosine < 0.999_99
        || [
            denormalize.max_abs,
            rotations.max_abs,
            translations.max_abs,
            roots.max_abs,
            keypoints.max_abs,
        ]
        .into_iter()
        .any(|value| !value.is_finite())
    {
        return Err(format!(
            "decoder parity below gate: minimum cosine {minimum_cosine:.12}"
        ));
    }
    Ok(())
}
