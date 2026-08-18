//! Release validator/benchmark for the native SkinTokens sampled-weight
//! transfer host boundary.
//!
//! ```text
//! skin-tokens-output-validate <mesh.glb> <sampled-weight-column.npy> [transferred-oracle.npy]
//! ```

use makepad_diffusion::skin_tokens_mesh::SkinTokensMesh;
use makepad_diffusion::skin_tokens_output::skin_tokens_rig_glb;
use makepad_diffusion::skin_tokens_tokenizer::SkinTokensSkeleton;
use std::path::Path;
use std::time::Instant;

fn npy_f32(path: &Path) -> Result<(Vec<usize>, Vec<f32>), String> {
    let bytes = std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    if bytes.len() < 12 || &bytes[..6] != b"\x93NUMPY" {
        return Err(format!("{} is not NPY", path.display()));
    }
    let (header_start, header_len) = match bytes[6] {
        1 => (10, u16::from_le_bytes([bytes[8], bytes[9]]) as usize),
        2 | 3 => (
            12,
            u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize,
        ),
        version => return Err(format!("unsupported NPY version {version}")),
    };
    let data_start = header_start + header_len;
    let header = std::str::from_utf8(
        bytes
            .get(header_start..data_start)
            .ok_or_else(|| "truncated NPY header".to_string())?,
    )
    .map_err(|error| format!("NPY header: {error}"))?;
    let shape_text = header
        .split("'shape':")
        .nth(1)
        .and_then(|value| value.split('(').nth(1))
        .and_then(|value| value.split(')').next())
        .ok_or_else(|| "NPY header has no shape".to_string())?;
    let shape = shape_text
        .split(',')
        .filter_map(|value| value.trim().parse().ok())
        .collect::<Vec<_>>();
    let data = bytes
        .get(data_start..)
        .ok_or_else(|| "truncated NPY data".to_string())?;
    let values = if header.contains("'descr': '<f4'") {
        data.chunks_exact(4)
            .map(|chunk| f32::from_le_bytes(chunk.try_into().unwrap()))
            .collect()
    } else if header.contains("'descr': '<f8'") {
        data.chunks_exact(8)
            .map(|chunk| f64::from_le_bytes(chunk.try_into().unwrap()) as f32)
            .collect()
    } else {
        return Err(format!("{} is not little-endian f32/f64 NPY", path.display()));
    };
    Ok((shape, values))
}

fn npy_i64(path: &Path) -> Result<(Vec<usize>, Vec<i64>), String> {
    let bytes = std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    if bytes.len() < 12 || &bytes[..6] != b"\x93NUMPY" {
        return Err(format!("{} is not NPY", path.display()));
    }
    let (header_start, header_len) = match bytes[6] {
        1 => (10, u16::from_le_bytes([bytes[8], bytes[9]]) as usize),
        2 | 3 => (
            12,
            u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize,
        ),
        version => return Err(format!("unsupported NPY version {version}")),
    };
    let data_start = header_start + header_len;
    let header = std::str::from_utf8(
        bytes
            .get(header_start..data_start)
            .ok_or_else(|| "truncated NPY header".to_string())?,
    )
    .map_err(|error| format!("NPY header: {error}"))?;
    if !header.contains("'descr': '<i8'") {
        return Err(format!("{} is not little-endian i64 NPY", path.display()));
    }
    let shape_text = header
        .split("'shape':")
        .nth(1)
        .and_then(|value| value.split('(').nth(1))
        .and_then(|value| value.split(')').next())
        .ok_or_else(|| "NPY header has no shape".to_string())?;
    let shape = shape_text
        .split(',')
        .filter_map(|value| value.trim().parse().ok())
        .collect::<Vec<_>>();
    let values = bytes[data_start..]
        .chunks_exact(8)
        .map(|chunk| i64::from_le_bytes(chunk.try_into().unwrap()))
        .collect();
    Ok((shape, values))
}

fn rig_from_oracle(mesh_path: &str, oracle_dir: &str, output_path: &str) -> Result<(), String> {
    let glb = std::fs::read(mesh_path).map_err(|error| format!("{mesh_path}: {error}"))?;
    let mesh = SkinTokensMesh::from_glb(&glb).map_err(|error| error.to_string())?;
    let samples = mesh.sample(424_242).map_err(|error| error.to_string())?;
    let oracle = Path::new(oracle_dir);
    let (joint_shape, joint_values) = npy_f32(&oracle.join("decoded_joints.npy"))?;
    if joint_shape.len() != 2 || joint_shape[1] != 3 {
        return Err(format!("decoded joints have shape {joint_shape:?}, expected [J,3]"));
    }
    let joint_count = joint_shape[0];
    let joints = joint_values
        .chunks_exact(3)
        .map(|row| [row[0], row[1], row[2]])
        .collect::<Vec<_>>();
    let (parent_shape, parent_values) = npy_i64(&oracle.join("decoded_parents.npy"))?;
    if parent_shape != [joint_count] {
        return Err(format!(
            "decoded parents have shape {parent_shape:?}, expected [{joint_count}]",
        ));
    }
    let parents = parent_values
        .into_iter()
        .map(|parent| {
            if parent < 0 {
                Ok(None)
            } else {
                usize::try_from(parent)
                    .map(Some)
                    .map_err(|_| format!("invalid parent {parent}"))
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    let (skin_shape, sample_weights) = npy_f32(&oracle.join("decoded_skin.npy"))?;
    if skin_shape != [samples.positions.len(), joint_count] {
        return Err(format!(
            "decoded skin has shape {skin_shape:?}, expected [{}, {joint_count}]",
            samples.positions.len(),
        ));
    }
    let skeleton = SkinTokensSkeleton {
        joints,
        parents,
        class_token: None,
        parts: Vec::new(),
    };
    let started = Instant::now();
    let output = skin_tokens_rig_glb(&glb, &mesh, &samples, &skeleton, &sample_weights)
        .map_err(|error| error.to_string())?;
    let elapsed = started.elapsed();
    if let Some(parent) = Path::new(output_path).parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("{}: {error}", parent.display()))?;
    }
    std::fs::write(output_path, &output).map_err(|error| format!("{output_path}: {error}"))?;
    println!(
        "[rig] joints={joint_count} vertices={} bytes={} transfer+augment={:.3}ms output={output_path}",
        mesh.positions.len(),
        output.len(),
        elapsed.as_secs_f64() * 1e3,
    );
    Ok(())
}

fn run() -> Result<(), String> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if let [command, mesh_path, oracle_dir, output_path] = args.as_slice() {
        if command == "rig" {
            return rig_from_oracle(mesh_path, oracle_dir, output_path);
        }
    }
    let [mesh_path, sampled_path, rest @ ..] = args.as_slice() else {
        return Err("usage: skin-tokens-output-validate <mesh.glb> <sampled-weight-column.npy> [transferred-oracle.npy]\n       skin-tokens-output-validate rig <mesh.glb> <generation-oracle-dir> <output.glb>".into());
    };
    if rest.len() > 1 {
        return Err("too many arguments".into());
    }
    let glb = std::fs::read(mesh_path).map_err(|error| format!("{mesh_path}: {error}"))?;
    let mesh = SkinTokensMesh::from_glb(&glb).map_err(|error| error.to_string())?;
    let samples = mesh.sample(424_242).map_err(|error| error.to_string())?;
    let (sample_shape, sampled_weights) = npy_f32(Path::new(sampled_path))?;
    if sampled_weights.len() != samples.positions.len() {
        return Err(format!(
            "sampled weights shape {sample_shape:?} contains {}, expected {}",
            sampled_weights.len(),
            samples.positions.len(),
        ));
    }

    let started = Instant::now();
    let first = mesh
        .transfer_sample_weights(&samples, &sampled_weights, 1)
        .map_err(|error| error.to_string())?;
    let cold = started.elapsed();
    let mut timings = Vec::new();
    for _ in 0..7 {
        let started = Instant::now();
        let output = mesh
            .transfer_sample_weights(&samples, &sampled_weights, 1)
            .map_err(|error| error.to_string())?;
        timings.push(started.elapsed());
        if output != first {
            return Err("native transfer is not deterministic".into());
        }
    }
    timings.sort_unstable();
    println!(
        "[transfer] vertices={} samples={} cold={:.3}ms warm_median={:.3}ms range={:.6}..{:.6}",
        mesh.positions.len(),
        samples.positions.len(),
        cold.as_secs_f64() * 1e3,
        timings[timings.len() / 2].as_secs_f64() * 1e3,
        first.iter().copied().fold(f64::INFINITY, f64::min),
        first.iter().copied().fold(f64::NEG_INFINITY, f64::max),
    );

    if let Some(oracle_path) = rest.first() {
        let (shape, oracle) = npy_f32(Path::new(oracle_path))?;
        if oracle.len() != first.len() {
            return Err(format!(
                "transfer oracle shape {shape:?} contains {}, expected {}",
                oracle.len(),
                first.len(),
            ));
        }
        let mut max_abs = 0.0f64;
        let mut sum_abs = 0.0f64;
        for (&native, &reference) in first.iter().zip(&oracle) {
            // The official .npy fixture is the final f32 cast; native keeps
            // the transfer in f64 until top-four export. Compare at the same
            // observable boundary here.
            let difference = (native as f32 - reference).abs() as f64;
            max_abs = max_abs.max(difference);
            sum_abs += difference;
        }
        let mean_abs = sum_abs / first.len() as f64;
        println!("[oracle] max_abs={max_abs:.9e} mean_abs={mean_abs:.9e}");
        if max_abs > 2.0e-6 || mean_abs > 1.0e-7 {
            return Err("native transferred weights exceed oracle tolerance".into());
        }
    }
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("skin-tokens-output-validate: {error}");
        std::process::exit(1);
    }
}
