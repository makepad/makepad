//! Native SkinTokens boundary validator.
//!
//! ```text
//! skin-tokens-validate inventory <tokenrig.safetensors>
//! skin-tokens-validate mesh <input.glb> <official-oracle-dir> [seed]
//! ```

use makepad_diffusion::skin_tokens::SkinTokensWeights;
use makepad_diffusion::skin_tokens_condition::{
    embed_condition_rows, select_condition_rows, SkinTokensConditionKind,
};
use makepad_diffusion::skin_tokens_convert::convert_skin_tokens_checkpoint;
use makepad_diffusion::skin_tokens_mesh::SkinTokensMesh;
use makepad_diffusion::skin_tokens_neural::{
    encode_mesh_prefix_tapped, encode_vae_condition, encode_vae_condition_tapped,
    project_condition,
};
use makepad_diffusion::torch_pth::{PthDType, PthStateDict};
use makepad_diffusion::backend::{
    gpu_attention_packed_bf16, gpu_attention_packed_cross_bf16,
    gpu_attention_packed_cross_composite_bf16, gpu_bf16_round, gpu_download,
    gpu_gelu_erf, gpu_layer_norm_pytorch, gpu_linear_nt_cached_bf16_bias_epilogue,
    gpu_linear_nt_cached_bf16_mm, gpu_perf_stats, gpu_skintokens_michelangelo_fourier,
    gpu_slice_cols, gpu_upload, gpu_weight_cache_ensure, GpuLinearPart,
};
use makepad_ggml::quant::GGML_TYPE_BF16;
use std::path::Path;

struct Npy {
    shape: Vec<usize>,
    descr: String,
    data: Vec<u8>,
}

fn load_npy(path: &Path) -> Result<Npy, String> {
    let bytes = std::fs::read(path).map_err(|err| format!("{}: {err}", path.display()))?;
    if bytes.len() < 12 || &bytes[..6] != b"\x93NUMPY" {
        return Err(format!("{}: not an npy file", path.display()));
    }
    let major = bytes[6];
    let (header_len, header_start) = match major {
        1 => (u16::from_le_bytes([bytes[8], bytes[9]]) as usize, 10usize),
        2 | 3 => (
            u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize,
            12usize,
        ),
        _ => return Err(format!("{}: unsupported npy version {major}", path.display())),
    };
    let data_start = header_start
        .checked_add(header_len)
        .filter(|end| *end <= bytes.len())
        .ok_or_else(|| format!("{}: truncated npy header", path.display()))?;
    let header = String::from_utf8_lossy(&bytes[header_start..data_start]);
    if header.contains("'fortran_order': True") {
        return Err(format!("{}: Fortran-order npy is unsupported", path.display()));
    }
    let descr = header
        .split("'descr':")
        .nth(1)
        .and_then(|rest| rest.split('\'').nth(1))
        .ok_or_else(|| format!("{}: npy has no descr", path.display()))?
        .to_string();
    let shape_text = header
        .split("'shape':")
        .nth(1)
        .and_then(|rest| rest.split('(').nth(1))
        .and_then(|rest| rest.split(')').next())
        .ok_or_else(|| format!("{}: npy has no shape", path.display()))?;
    let shape = shape_text
        .split(',')
        .filter_map(|part| part.trim().parse::<usize>().ok())
        .collect();
    Ok(Npy {
        shape,
        descr,
        data: bytes[data_start..].to_vec(),
    })
}

impl Npy {
    fn f32(&self) -> Result<Vec<f32>, String> {
        match self.descr.as_str() {
            "<f4" => Ok(self
                .data
                .chunks_exact(4)
                .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                .collect()),
            "<f8" => Ok(self
                .data
                .chunks_exact(8)
                .map(|c| {
                    f64::from_le_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]])
                        as f32
                })
                .collect()),
            other => Err(format!("npy dtype {other} is not float-convertible")),
        }
    }

    fn i32(&self) -> Result<Vec<i32>, String> {
        match self.descr.as_str() {
            "<i4" => Ok(self
                .data
                .chunks_exact(4)
                .map(|c| i32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                .collect()),
            other => Err(format!("npy dtype {other} is not i32")),
        }
    }

    fn i64(&self) -> Result<Vec<i64>, String> {
        match self.descr.as_str() {
            "<i8" => Ok(self
                .data
                .chunks_exact(8)
                .map(|c| {
                    i64::from_le_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]])
                })
                .collect()),
            other => Err(format!("npy dtype {other} is not i64")),
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Difference {
    max_abs: f64,
    mean_abs: f64,
    rms: f64,
    cosine: f64,
    max_index: usize,
    above_1e5: usize,
}

fn compare(name: &str, ours: &[f32], reference: &[f32]) -> Result<Difference, String> {
    if ours.len() != reference.len() {
        return Err(format!(
            "{name}: element count differs: native={} oracle={}",
            ours.len(),
            reference.len()
        ));
    }
    let mut max_abs = 0.0f64;
    let mut max_index = 0usize;
    let mut sum_abs = 0.0f64;
    let mut sum_sq = 0.0f64;
    let mut dot = 0.0f64;
    let mut ours_sq = 0.0f64;
    let mut reference_sq = 0.0f64;
    let mut above_1e5 = 0usize;
    for (index, (&a, &b)) in ours.iter().zip(reference).enumerate() {
        let diff = (a as f64 - b as f64).abs();
        sum_abs += diff;
        sum_sq += diff * diff;
        dot += a as f64 * b as f64;
        ours_sq += (a as f64) * (a as f64);
        reference_sq += (b as f64) * (b as f64);
        above_1e5 += usize::from(diff > 1e-5);
        if diff > max_abs {
            max_abs = diff;
            max_index = index;
        }
    }
    let count = ours.len().max(1) as f64;
    let stats = Difference {
        max_abs,
        mean_abs: sum_abs / count,
        rms: (sum_sq / count).sqrt(),
        cosine: if ours_sq == 0.0 || reference_sq == 0.0 {
            f64::NAN
        } else {
            dot / (ours_sq.sqrt() * reference_sq.sqrt())
        },
        max_index,
        above_1e5,
    };
    println!(
        "[{name}] cosine={:.9} max_abs={:.9e} mean_abs={:.9e} rms={:.9e} max_i={} >1e-5={}/{}",
        stats.cosine,
        stats.max_abs,
        stats.mean_abs,
        stats.rms,
        stats.max_index,
        stats.above_1e5,
        ours.len(),
    );
    if !ours.is_empty() {
        let row = stats.max_index / 3;
        let start = (row * 3).min(ours.len().saturating_sub(1));
        let end = (start + 3).min(ours.len());
        println!(
            "[{name}] max_row={row} native={:?} oracle={:?}",
            &ours[start..end],
            &reference[start..end],
        );
    }
    Ok(stats)
}

fn flatten(values: &[[f32; 3]]) -> Vec<f32> {
    values.iter().flat_map(|value| value.iter().copied()).collect()
}

fn require_shape(name: &str, found: &[usize], expected: &[usize]) -> Result<(), String> {
    if found != expected {
        return Err(format!("{name}: shape {found:?}, expected {expected:?}"));
    }
    Ok(())
}

fn validate_mesh(input: &Path, oracle: &Path, seed: u32) -> Result<(), String> {
    let bytes = std::fs::read(input).map_err(|err| format!("{}: {err}", input.display()))?;
    let mesh = SkinTokensMesh::from_glb(&bytes).map_err(|err| err.to_string())?;
    println!(
        "[mesh] vertices={} faces={} parts={} center={:?} scale={:.9}",
        mesh.positions.len(),
        mesh.indices.len() / 3,
        mesh.parts.len(),
        mesh.normalization.center,
        mesh.normalization.scale,
    );

    let reference_faces = load_npy(&oracle.join("normalized_faces.npy"))?;
    require_shape(
        "faces",
        &reference_faces.shape,
        &[mesh.indices.len() / 3, 3],
    )?;
    let reference_faces = reference_faces.i32()?;
    let first_face_mismatch = mesh
        .indices
        .iter()
        .zip(&reference_faces)
        .position(|(&native, &official)| native as i32 != official);
    if let Some(index) = first_face_mismatch {
        let scalar_start = index.saturating_sub(index % 3).saturating_sub(9);
        let scalar_end = (scalar_start + 30).min(mesh.indices.len());
        return Err(format!(
            "faces: first mismatch at scalar {index}: native={} oracle={}; native_window={:?}; oracle_window={:?}",
            mesh.indices[index],
            reference_faces[index],
            &mesh.indices[scalar_start..scalar_end],
            &reference_faces[scalar_start..scalar_end],
        ));
    }
    println!("[faces] exact ({} indices)", mesh.indices.len());

    let cases: [(&str, Vec<f32>, f64); 4] = [
        (
            "source_positions",
            flatten(&mesh.source_positions),
            2e-6,
        ),
        ("normalized_positions", flatten(&mesh.positions), 2e-6),
        ("face_normals", flatten(&mesh.face_normals), 2e-5),
        ("vertex_normals", flatten(&mesh.vertex_normals), 2e-5),
    ];
    let oracle_names = [
        "source_vertices.npy",
        "normalized_vertices.npy",
        "normalized_face_normals.npy",
        "normalized_vertex_normals.npy",
    ];
    for ((name, native, tolerance), oracle_name) in cases.into_iter().zip(oracle_names) {
        let npy = load_npy(&oracle.join(oracle_name))?;
        require_shape(name, &npy.shape, &[native.len() / 3, 3])?;
        let stats = compare(name, &native, &npy.f32()?)?;
        if stats.max_abs > tolerance {
            return Err(format!(
                "{name}: max_abs {:.9e} exceeds gate {tolerance:.1e}",
                stats.max_abs
            ));
        }
    }

    let samples = mesh.sample(seed).map_err(|err| err.to_string())?;
    let sample_cases = [
        (
            "sampled_positions",
            flatten(&samples.positions),
            "sampled_positions.npy",
            3e-6,
        ),
        (
            "sampled_normals",
            flatten(&samples.normals),
            "sampled_normals.npy",
            2e-5,
        ),
    ];
    for (name, native, oracle_name, tolerance) in sample_cases {
        let npy = load_npy(&oracle.join(oracle_name))?;
        require_shape(name, &npy.shape, &[native.len() / 3, 3])?;
        let stats = compare(name, &native, &npy.f32()?)?;
        if stats.max_abs > tolerance {
            return Err(format!(
                "{name}: max_abs {:.9e} exceeds gate {tolerance:.1e}",
                stats.max_abs
            ));
        }
    }
    println!("[mesh] PASS official predict-transform boundary, seed={seed}");
    Ok(())
}

fn exact_indices(name: &str, native: &[usize], oracle: &Npy) -> Result<(), String> {
    require_shape(name, &oracle.shape, &[native.len()])?;
    let reference = oracle.i64()?;
    if let Some(index) = native
        .iter()
        .zip(&reference)
        .position(|(&left, &right)| left as i64 != right)
    {
        return Err(format!(
            "{name}: first mismatch at {index}: native={} oracle={}",
            native[index], reference[index],
        ));
    }
    println!("[{name}] exact ({} indices)", native.len());
    Ok(())
}

fn validate_condition(oracle: &Path, seed: u64) -> Result<(), String> {
    let cond_npy = load_npy(&oracle.join("cond_input.npy"))?;
    require_shape("condition", &cond_npy.shape, &[1, 54_000, 6])?;
    let condition = cond_npy.f32()?;
    for (kind, prefix, tolerance) in [
        (SkinTokensConditionKind::SkinVae, "vae", 3.0e-6f64),
        (
            SkinTokensConditionKind::Michelangelo,
            "mesh",
            2.0e-5f64,
        ),
    ] {
        let selection = select_condition_rows(&condition, seed, kind)
            .map_err(|error| error.to_string())?;
        exact_indices(
            &format!("{prefix}_candidate_indices"),
            &selection.candidate_indices,
            &load_npy(&oracle.join(format!("{prefix}_candidate_indices.npy")))?,
        )?;
        exact_indices(
            &format!("{prefix}_fps_indices"),
            &selection.fps_indices,
            &load_npy(&oracle.join(format!("{prefix}_fps_indices.npy")))?,
        )?;
        let selected_reference = load_npy(&oracle.join(format!("{prefix}_selected_cond.npy")))?;
        require_shape(
            &format!("{prefix}_selected_cond"),
            &selected_reference.shape,
            &[1, kind.tokens(), 6],
        )?;
        let stats = compare(
            &format!("{prefix}_selected_cond"),
            &selection.selected,
            &selected_reference.f32()?,
        )?;
        if stats.max_abs != 0.0 {
            return Err(format!(
                "{prefix}_selected_cond: expected exact gather, max_abs={}",
                stats.max_abs,
            ));
        }

        let kv = embed_condition_rows(&condition, kind).map_err(|error| error.to_string())?;
        let q = embed_condition_rows(&selection.selected, kind)
            .map_err(|error| error.to_string())?;
        for (name, native, expected_shape) in [
            (
                match kind {
                    SkinTokensConditionKind::SkinVae => "vae_kv_embed",
                    SkinTokensConditionKind::Michelangelo => "mesh_kv_fourier",
                },
                kv,
                vec![1, 54_000, 54],
            ),
            (
                match kind {
                    SkinTokensConditionKind::SkinVae => "vae_q_embed",
                    SkinTokensConditionKind::Michelangelo => "mesh_q_fourier",
                },
                q,
                vec![1, kind.tokens(), 54],
            ),
        ] {
            // The first oracle captured Michelangelo only after input_proj;
            // keep validating its exact selection and gate raw Fourier rows
            // when the optional operator-level dump is present.
            let path = oracle.join(format!("{name}.npy"));
            if !path.is_file() {
                println!("[{name}] oracle tensor absent; selection gate retained");
                continue;
            }
            let reference = load_npy(&path)?;
            require_shape(&name, &reference.shape, &expected_shape)?;
            let stats = compare(&name, &native, &reference.f32()?)?;
            if stats.max_abs > tolerance {
                return Err(format!(
                    "{name}: max_abs {:.9e} exceeds gate {tolerance:.1e}",
                    stats.max_abs,
                ));
            }
        }
    }
    println!("[condition] PASS NumPy choice/FPS/Fourier boundaries, seed={seed}");
    Ok(())
}

fn validate_projection(weights: &Path, oracle: &Path, seed: u64) -> Result<(), String> {
    let weights = SkinTokensWeights::load(weights).map_err(|error| error.to_string())?;
    let condition_npy = load_npy(&oracle.join("cond_input.npy"))?;
    require_shape("condition", &condition_npy.shape, &[1, 54_000, 6])?;
    let condition = condition_npy.f32()?;
    for (kind, prefix, width) in [
        (SkinTokensConditionKind::SkinVae, "vae", 768usize),
        (
            SkinTokensConditionKind::Michelangelo,
            "mesh",
            512usize,
        ),
    ] {
        let projected = project_condition(&weights, &condition, seed, kind)
            .map_err(|error| error.to_string())?;
        for (suffix, native, expected_shape) in [
            (
                "q_projected",
                projected.query_f32().map_err(|error| error.to_string())?,
                vec![1, kind.tokens(), width],
            ),
            (
                "kv_projected",
                projected
                    .key_value_f32()
                    .map_err(|error| error.to_string())?,
                vec![1, 54_000, width],
            ),
        ] {
            let name = format!("{prefix}_{suffix}");
            let reference = load_npy(&oracle.join(format!("{name}.npy")))?;
            require_shape(&name, &reference.shape, &expected_shape)?;
            let stats = compare(&name, &native, &reference.f32()?)?;
            if stats.max_abs > 1.0e-2 || stats.mean_abs > 8.0e-4 {
                return Err(format!(
                    "{name}: projection parity gate failed (max {:.9e}, mean {:.9e})",
                    stats.max_abs, stats.mean_abs,
                ));
            }
        }
    }
    println!("[projection] PASS native BF16 54-channel projections, seed={seed}");
    Ok(())
}

/// Direct operator gate for the cuBLASLt BF16+bias epilogue path. Keeping
/// this separate from `project_condition` lets the backend contract be
/// proven against the pinned PyTorch oracle before production routing.
fn validate_projection_lt(weights: &Path, oracle: &Path, seed: u64) -> Result<(), String> {
    const NAMESPACE: &str = "skin-tokens-lt-projection-validate";
    let weights = SkinTokensWeights::load(weights).map_err(|error| error.to_string())?;
    let condition_npy = load_npy(&oracle.join("cond_input.npy"))?;
    require_shape("condition", &condition_npy.shape, &[1, 54_000, 6])?;
    let condition = condition_npy.f32()?;
    for (kind, prefix, width, weight_name, bias_name) in [
        (
            SkinTokensConditionKind::SkinVae,
            "vae",
            768usize,
            "vae.model.cond_encoder.proj_in.weight",
            "vae.model.cond_encoder.proj_in.bias",
        ),
        (
            SkinTokensConditionKind::Michelangelo,
            "mesh",
            512usize,
            "mesh_encoder.encoder.input_proj.weight",
            "mesh_encoder.encoder.input_proj.bias",
        ),
    ] {
        gpu_weight_cache_ensure(
            NAMESPACE,
            weight_name,
            GGML_TYPE_BF16,
            width,
            54,
            false,
            || weights.tensor_bytes(weight_name).map_err(|error| error.to_string()),
        )?;
        let part = GpuLinearPart {
            bt_ggml_type: GGML_TYPE_BF16,
            n: width,
            cache_key: weight_name,
            bytes: &[],
        };
        let bias = weights
            .tensor_f32(bias_name)
            .map_err(|error| error.to_string())?;
        let selected = select_condition_rows(&condition, seed, kind)
            .map_err(|error| error.to_string())?;
        let query = embed_condition_rows(&selected.selected, kind)
            .map_err(|error| error.to_string())?;
        let key_value = embed_condition_rows(&condition, kind)
            .map_err(|error| error.to_string())?;

        for (suffix, input, input_oracle, rows) in [
            (
                "q_projected",
                query,
                match kind {
                    SkinTokensConditionKind::SkinVae => "vae_q_embed.npy",
                    SkinTokensConditionKind::Michelangelo => "mesh_q_fourier.npy",
                },
                kind.tokens(),
            ),
            (
                "kv_projected",
                key_value,
                match kind {
                    SkinTokensConditionKind::SkinVae => "vae_kv_embed.npy",
                    SkinTokensConditionKind::Michelangelo => "mesh_kv_fourier.npy",
                },
                54_000usize,
            ),
        ] {
            let input_reference = load_npy(&oracle.join(input_oracle))?;
            require_shape(input_oracle, &input_reference.shape, &[1, rows, 54])?;
            let input_reference = input_reference.f32()?;
            let _ = compare(
                &format!("{prefix}_{suffix}_native_input"),
                &input,
                &input_reference,
            )?;
            let input = gpu_upload(&input_reference, rows, 54)?;
            let native = gpu_linear_nt_cached_bf16_bias_epilogue(
                &input,
                NAMESPACE,
                std::slice::from_ref(&part),
                &bias,
            )?;
            let native = gpu_download(&native)?;
            let name = format!("{prefix}_{suffix}");
            let reference = load_npy(&oracle.join(format!("{name}.npy")))?;
            require_shape(&name, &reference.shape, &[1, rows, width])?;
            let stats = compare(&format!("{name}_lt"), &native, &reference.f32()?)?;
            if stats.max_abs != 0.0 {
                return Err(format!(
                    "{name}: cuBLASLt BF16+bias direct gate is not bit-exact (max {:.9e}, mean {:.9e})",
                    stats.max_abs, stats.mean_abs,
                ));
            }
        }
    }
    println!("[projection-lt] PASS bit-exact cuBLASLt BF16+bias projections, seed={seed}");
    Ok(())
}

/// Direct replay of the pinned Torch bias-free BF16 `mm` boundaries in the
/// Michelangelo cross-attention block. Official LayerNorm outputs are fed in
/// so input-projection and PMPE differences cannot contaminate this gate.
fn validate_mesh_mm(weights: &Path, oracle: &Path) -> Result<(), String> {
    const NAMESPACE: &str = "skin-tokens-mesh-mm-validate";
    let weights = SkinTokensWeights::load(weights).map_err(|error| error.to_string())?;
    for (name, input_name, output_name, weight_name, rows, width) in [
        (
            "mesh_cross_q_mm",
            "mesh_cross_ln1",
            "mesh_cross_q",
            "mesh_encoder.encoder.cross_attn.attn.c_q.weight",
            512usize,
            512usize,
        ),
        (
            "mesh_cross_kv_mm",
            "mesh_cross_ln2",
            "mesh_cross_kv",
            "mesh_encoder.encoder.cross_attn.attn.c_kv.weight",
            54_000usize,
            1_024usize,
        ),
    ] {
        gpu_weight_cache_ensure(
            NAMESPACE,
            weight_name,
            GGML_TYPE_BF16,
            width,
            512,
            false,
            || weights.tensor_bytes(weight_name).map_err(|error| error.to_string()),
        )?;
        let part = GpuLinearPart {
            bt_ggml_type: GGML_TYPE_BF16,
            n: width,
            cache_key: weight_name,
            bytes: &[],
        };
        let input = load_npy(&oracle.join(format!("{input_name}.npy")))?;
        require_shape(input_name, &input.shape, &[1, rows, 512])?;
        let input = gpu_upload(&input.f32()?, rows, 512)?;
        let native = gpu_download(&gpu_linear_nt_cached_bf16_mm(
            &input,
            NAMESPACE,
            std::slice::from_ref(&part),
        )?)?;
        let reference = load_npy(&oracle.join(format!("{output_name}.npy")))?;
        require_shape(output_name, &reference.shape, &[1, rows, width])?;
        let stats = compare(name, &native, &reference.f32()?)?;
        if stats.max_abs != 0.0 {
            return Err(format!(
                "{name}: PyTorch BF16 mm replay is not bit-exact (max {:.9e}, mean {:.9e})",
                stats.max_abs, stats.mean_abs,
            ));
        }
    }
    println!("[mesh-mm] PASS bit-exact Torch 2.7 bias-free BF16 projections");
    Ok(())
}

/// Direct replay of Michelangelo's biased `nn.Linear` calls using official
/// inputs. Torch folds these contiguous 3D tensors to 2D `addmm`, whose BF16
/// bias epilogue is represented by the narrowly scoped cuBLASLt helper.
fn validate_mesh_biased_linears(weights: &Path, oracle: &Path) -> Result<(), String> {
    const NAMESPACE: &str = "skin-tokens-mesh-biased-linear-validate";
    let weights = SkinTokensWeights::load(weights).map_err(|error| error.to_string())?;
    for (name, input_name, output_name, weight_name, bias_name, input_width, output_width) in [
        (
            "mesh_cross_to_out_lt",
            "mesh_cross_flash",
            "mesh_cross_to_out",
            "mesh_encoder.encoder.cross_attn.attn.c_proj.weight",
            "mesh_encoder.encoder.cross_attn.attn.c_proj.bias",
            512usize,
            512usize,
        ),
        (
            "mesh_cross_ff_in_lt",
            "mesh_cross_ln3",
            "mesh_cross_ff_in",
            "mesh_encoder.encoder.cross_attn.mlp.c_fc.weight",
            "mesh_encoder.encoder.cross_attn.mlp.c_fc.bias",
            512usize,
            2_048usize,
        ),
        (
            "mesh_cross_ff_out_lt",
            "mesh_cross_gelu",
            "mesh_cross_ff_out",
            "mesh_encoder.encoder.cross_attn.mlp.c_proj.weight",
            "mesh_encoder.encoder.cross_attn.mlp.c_proj.bias",
            2_048usize,
            512usize,
        ),
        (
            "mesh_output_linear_lt",
            "mesh_ln_post",
            "mesh_output_linear",
            "output_proj.0.weight",
            "output_proj.0.bias",
            512usize,
            896usize,
        ),
    ] {
        gpu_weight_cache_ensure(
            NAMESPACE,
            weight_name,
            GGML_TYPE_BF16,
            output_width,
            input_width,
            false,
            || weights.tensor_bytes(weight_name).map_err(|error| error.to_string()),
        )?;
        let part = GpuLinearPart {
            bt_ggml_type: GGML_TYPE_BF16,
            n: output_width,
            cache_key: weight_name,
            bytes: &[],
        };
        let bias = weights
            .tensor_f32(bias_name)
            .map_err(|error| error.to_string())?;
        let input = load_npy(&oracle.join(format!("{input_name}.npy")))?;
        require_shape(input_name, &input.shape, &[1, 512, input_width])?;
        let input = gpu_upload(&input.f32()?, 512, input_width)?;
        let native = gpu_download(&gpu_linear_nt_cached_bf16_bias_epilogue(
            &input,
            NAMESPACE,
            std::slice::from_ref(&part),
            &bias,
        )?)?;
        let reference = load_npy(&oracle.join(format!("{output_name}.npy")))?;
        require_shape(output_name, &reference.shape, &[1, 512, output_width])?;
        let stats = compare(name, &native, &reference.f32()?)?;
        if stats.max_abs != 0.0 {
            return Err(format!(
                "{name}: cuBLASLt BF16+bias replay is not bit-exact (max {:.9e}, mean {:.9e})",
                stats.max_abs, stats.mean_abs,
            ));
        }
    }

    let input = load_npy(&oracle.join("mesh_cross_ff_in.npy"))?;
    require_shape("mesh_cross_ff_in", &input.shape, &[1, 512, 2_048])?;
    let input = gpu_upload(&input.f32()?, 512, 2_048)?;
    let activated = gpu_gelu_erf(&input)?;
    let reference = load_npy(&oracle.join("mesh_cross_gelu.npy"))?;
    require_shape("mesh_cross_gelu", &reference.shape, &[1, 512, 2_048])?;
    let reference = reference.f32()?;
    compare(
        "mesh_cross_gelu_raw",
        &gpu_download(&activated)?,
        &reference,
    )?;
    compare(
        "mesh_cross_gelu_bf16",
        &gpu_download(&gpu_bf16_round(&activated)?)?,
        &reference,
    )?;
    println!("[mesh-biased-linears] PASS bit-exact Michelangelo biased linears");
    Ok(())
}

fn validate_michelangelo_fourier_cuda(oracle: &Path) -> Result<(), String> {
    for (name, input_name, reference_name, rows) in [
        (
            "mesh_q_fourier_cuda",
            "mesh_selected_cond",
            "mesh_q_fourier",
            512usize,
        ),
        (
            "mesh_kv_fourier_cuda",
            "cond_input",
            "mesh_kv_fourier",
            54_000usize,
        ),
    ] {
        let input = load_npy(&oracle.join(format!("{input_name}.npy")))?;
        require_shape(input_name, &input.shape, &[1, rows, 6])?;
        let input = gpu_upload(&input.f32()?, rows, 6)?;
        let native = gpu_download(&gpu_skintokens_michelangelo_fourier(&input)?)?;
        let reference = load_npy(&oracle.join(format!("{reference_name}.npy")))?;
        require_shape(reference_name, &reference.shape, &[1, rows, 54])?;
        let stats = compare(name, &native, &reference.f32()?)?;
        if stats.max_abs != 0.0 {
            return Err(format!(
                "{name}: CUDA Michelangelo Fourier is not bit-exact (max {:.9e}, mean {:.9e})",
                stats.max_abs, stats.mean_abs,
            ));
        }
    }
    println!("[fourier-cuda] PASS bit-exact Michelangelo CUDA embedding");
    Ok(())
}

fn validate_mesh_attention_direct(oracle: &Path) -> Result<(), String> {
    let q = load_npy(&oracle.join("mesh_cross_q.npy"))?;
    require_shape("mesh_cross_q", &q.shape, &[1, 512, 512])?;
    let official_kv = load_npy(&oracle.join("mesh_cross_kv.npy"))?;
    require_shape("mesh_cross_kv", &official_kv.shape, &[1, 54_000, 1_024])?;
    let official_kv = official_kv.f32()?;
    let mut standard_kv = vec![0.0f32; official_kv.len()];
    for row in 0..54_000 {
        for standard_col in 0..1_024 {
            let stream = standard_col / 512;
            let within_stream = standard_col % 512;
            let head = within_stream / 64;
            let dim = within_stream % 64;
            let official_col = head * 2 * 64 + stream * 64 + dim;
            standard_kv[row * 1_024 + standard_col] =
                official_kv[row * 1_024 + official_col];
        }
    }
    let q = gpu_upload(&q.f32()?, 512, 512)?;
    let kv = gpu_upload(&standard_kv, 54_000, 1_024)?;
    let k = gpu_slice_cols(&kv, 0, 512)?;
    let v = gpu_slice_cols(&kv, 512, 512)?;
    let reference = load_npy(&oracle.join("mesh_cross_flash.npy"))?;
    require_shape("mesh_cross_flash", &reference.shape, &[1, 512, 512])?;
    let reference = reference.f32()?;
    for (name, attention) in [
        (
            "mesh_cross_flash_direct",
            gpu_attention_packed_cross_bf16(&q, &k, &v, 8, 1.0 / 8.0)?,
        ),
        (
            "mesh_cross_composite_direct",
            gpu_attention_packed_cross_composite_bf16(&q, &k, &v, 8, 1.0 / 8.0)?,
        ),
    ] {
        let native = gpu_download(&gpu_bf16_round(&attention)?)?;
        compare(name, &native, &reference)?;
    }

    println!("[mesh-attention-direct] PASS flash/composite comparison complete");
    Ok(())
}

fn validate_vae_encoder(weights: &Path, oracle: &Path, seed: u64) -> Result<(), String> {
    let weights = SkinTokensWeights::load(weights).map_err(|error| error.to_string())?;
    let condition_npy = load_npy(&oracle.join("cond_input.npy"))?;
    require_shape("condition", &condition_npy.shape, &[1, 54_000, 6])?;
    let encoded = encode_vae_condition(&weights, &condition_npy.f32()?, seed)
        .map_err(|error| error.to_string())?;
    let tensors = [
        ("vae_block0", encoded.block_f32(0).map_err(|error| error.to_string())?),
        ("vae_block1", encoded.block_f32(1).map_err(|error| error.to_string())?),
        ("vae_block2", encoded.block_f32(2).map_err(|error| error.to_string())?),
        (
            "vae_norm_out",
            encoded.normalized_f32().map_err(|error| error.to_string())?,
        ),
        (
            "vae_cond_latents",
            encoded.latents_f32().map_err(|error| error.to_string())?,
        ),
    ];
    let mut failures = Vec::new();
    for (name, native) in tensors {
        let reference = load_npy(&oracle.join(format!("{name}.npy")))?;
        let expected_width = if name == "vae_cond_latents" { 512 } else { 768 };
        require_shape(name, &reference.shape, &[1, 384, expected_width])?;
        let stats = compare(name, &native, &reference.f32()?)?;
        let mean_gate = if name == "vae_norm_out" { 2.0e-4 } else { 2.0e-3 };
        let max_gate = if name == "vae_norm_out" { 2.0e-3 } else { 2.0e-2 };
        if stats.max_abs > max_gate || stats.mean_abs > mean_gate {
            failures.push(format!(
                "{name}: VAE encoder parity failed (max {:.9e}, mean {:.9e})",
                stats.max_abs, stats.mean_abs,
            ));
        }
    }
    if !failures.is_empty() {
        return Err(failures.join("; "));
    }
    println!("[vae-encoder] PASS three SkinVAE condition blocks and latent projection");
    Ok(())
}

fn validate_mesh_encoder(weights: &Path, oracle: &Path) -> Result<(), String> {
    let weights = SkinTokensWeights::load(weights).map_err(|error| error.to_string())?;
    let condition_npy = load_npy(&oracle.join("cond_input.npy"))?;
    require_shape("condition", &condition_npy.shape, &[1, 54_000, 6])?;

    // Gate the Torch 2.7 LayerNorm primitive independently of the upstream
    // projection. This feeds the official input into the native Welford
    // kernel, so any error reported here is the operator itself rather than
    // accumulated GEMM drift.
    for (name, input_name, prefix, rows) in [
        (
            "mesh_cross_ln1_direct",
            "mesh_q_projected",
            "mesh_encoder.encoder.cross_attn.ln_1",
            512usize,
        ),
        (
            "mesh_cross_ln2_direct",
            "mesh_kv_projected",
            "mesh_encoder.encoder.cross_attn.ln_2",
            54_000usize,
        ),
    ] {
        let input = load_npy(&oracle.join(format!("{input_name}.npy")))?;
        require_shape(input_name, &input.shape, &[1, rows, 512])?;
        let input_gpu = gpu_upload(&input.f32()?, rows, 512)?;
        let scale = weights
            .tensor_f32(&format!("{prefix}.weight"))
            .map_err(|error| error.to_string())?;
        let bias = weights
            .tensor_f32(&format!("{prefix}.bias"))
            .map_err(|error| error.to_string())?;
        let native = gpu_download(&gpu_layer_norm_pytorch(
            &input_gpu, &scale, &bias, 1.0e-5,
        )?)?;
        let reference_name = name.strip_suffix("_direct").expect("direct suffix");
        let reference = load_npy(&oracle.join(format!("{reference_name}.npy")))?;
        require_shape(reference_name, &reference.shape, &[1, rows, 512])?;
        let stats = compare(name, &native, &reference.f32()?)?;
        if stats.max_abs > 2.0e-6 || stats.mean_abs > 1.0e-8 {
            return Err(format!(
                "{name}: Torch 2.7 Welford LayerNorm parity failed (max {:.9e}, mean {:.9e})",
                stats.max_abs, stats.mean_abs,
            ));
        }
    }
    let started = std::time::Instant::now();
    let (encoded, taps) = encode_mesh_prefix_tapped(&weights, &condition_npy.f32()?)
        .map_err(|error| error.to_string())?;
    let elapsed = started.elapsed();

    exact_indices(
        "mesh_candidate_indices",
        &encoded.selection.candidate_indices,
        &load_npy(&oracle.join("mesh_candidate_indices.npy"))?,
    )?;
    exact_indices(
        "mesh_fps_indices",
        &encoded.selection.fps_indices,
        &load_npy(&oracle.join("mesh_fps_indices.npy"))?,
    )?;

    let official_kv = load_npy(&oracle.join("mesh_cross_kv.npy"))?;
    require_shape("mesh_cross_kv", &official_kv.shape, &[1, 54_000, 1_024])?;
    let official_kv = official_kv.f32()?;
    let mut standard_kv = vec![0.0f32; official_kv.len()];
    // The official Tripo module emits packed columns head-first
    // `[K_head0,V_head0,...]`, while the native attention boundary is
    // deliberately stream-major `[K_all_heads,V_all_heads]`.
    for row in 0..54_000 {
        for head in 0..8 {
            for stream in 0..2 {
                for dim in 0..64 {
                    let official_col = (head * 2 + stream) * 64 + dim;
                    let standard_col = stream * 512 + head * 64 + dim;
                    standard_kv[row * 1_024 + standard_col] =
                        official_kv[row * 1_024 + official_col];
                }
            }
        }
    }
    let operator_tensors = [
        ("mesh_cross_ln1", taps.cross.ln1, vec![1, 512, 512]),
        ("mesh_cross_ln2", taps.cross.ln2, vec![1, 54_000, 512]),
        ("mesh_cross_q", taps.cross.q, vec![1, 512, 512]),
        ("mesh_cross_kv", taps.cross.kv, vec![1, 54_000, 1_024]),
        ("mesh_cross_flash", taps.cross.flash, vec![1, 512, 512]),
        (
            "mesh_cross_to_out",
            taps.cross.to_out,
            vec![1, 512, 512],
        ),
        ("mesh_cross_ln3", taps.cross.ln3, vec![1, 512, 512]),
        (
            "mesh_cross_ff_in",
            taps.cross.ff_in,
            vec![1, 512, 2_048],
        ),
        (
            "mesh_cross_gelu",
            taps.cross.gelu,
            vec![1, 512, 2_048],
        ),
        (
            "mesh_cross_ff_out",
            taps.cross.ff_out,
            vec![1, 512, 512],
        ),
    ];

    let mut failures = Vec::new();
    for (name, native, expected_shape) in operator_tensors {
        let reference = load_npy(&oracle.join(format!("{name}.npy")))?;
        require_shape(name, &reference.shape, &expected_shape)?;
        let stats = if name == "mesh_cross_kv" {
            compare(name, &native, &standard_kv)?
        } else {
            compare(name, &native, &reference.f32()?)?
        };
        // This gate is intentionally diagnostic and tight: it marks the
        // first operator at which native execution diverges materially.
        // The accumulated production prefix keeps its independent gate.
        if stats.cosine < 0.999_99 || stats.max_abs > 1.0e-2 {
            failures.push(format!(
                "{name}: Michelangelo operator parity failed (cosine {:.9}, max {:.9e})",
                stats.cosine, stats.max_abs,
            ));
        }
    }

    let mut tensors = Vec::with_capacity(12);
    tensors.push(("mesh_cross_attn".to_string(), taps.cross_attention, 512usize));
    for (index, block) in taps.blocks.into_iter().enumerate() {
        tensors.push((format!("mesh_block{index}"), block, 512));
    }
    tensors.push(("mesh_ln_post".to_string(), taps.normalized, 512));
    tensors.push((
        "mesh_output_linear".to_string(),
        taps.output_linear,
        896,
    ));
    tensors.push(("mesh_prefix".to_string(), taps.prefix, 896));

    for (name, native, width) in tensors {
        let reference = load_npy(&oracle.join(format!("{name}.npy")))?;
        require_shape(&name, &reference.shape, &[1, 512, width])?;
        let stats = compare(&name, &native, &reference.f32()?)?;
        // Block activations are BF16 and accumulate small cuBLAS/SDPA
        // reduction-order differences. The two final normalization tensors
        // have materially tighter expected parity.
        let (max_gate, mean_gate) = match name.as_str() {
            "mesh_ln_post" => (3.0e-3, 3.0e-4),
            "mesh_prefix" => (2.0e-2, 1.5e-3),
            _ => (3.0e-2, 3.0e-3),
        };
        if stats.max_abs > max_gate || stats.mean_abs > mean_gate {
            failures.push(format!(
                "{name}: Michelangelo parity failed (max {:.9e}, mean {:.9e})",
                stats.max_abs, stats.mean_abs,
            ));
        }
    }
    println!(
        "[mesh-encoder] elapsed={:.3}s output=512x896",
        elapsed.as_secs_f64(),
    );
    if !failures.is_empty() {
        return Err(failures.join("; "));
    }
    println!("[mesh-encoder] PASS Michelangelo cross/self tower and Qwen prefix");
    Ok(())
}

fn validate_vae_operators(weights: &Path, oracle: &Path, seed: u64) -> Result<(), String> {
    let weights = SkinTokensWeights::load(weights).map_err(|error| error.to_string())?;
    let condition_npy = load_npy(&oracle.join("cond_input.npy"))?;
    require_shape("condition", &condition_npy.shape, &[1, 54_000, 6])?;
    let started = std::time::Instant::now();
    let (encoded, taps) = encode_vae_condition_tapped(&weights, &condition_npy.f32()?, seed)
        .map_err(|error| error.to_string())?;
    let elapsed = started.elapsed();
    let tensors = [
        ("vae_block0_norm2", taps.norm2, vec![1, 384, 768]),
        (
            "vae_block0_norm_cross",
            taps.norm_cross,
            vec![1, 54_000, 768],
        ),
        ("vae_block0_q", taps.q, vec![1, 384, 768]),
        ("vae_block0_k", taps.k, vec![1, 54_000, 768]),
        ("vae_block0_v", taps.v, vec![1, 54_000, 768]),
        (
            "vae_block0_attn_out",
            taps.attention_out,
            vec![1, 384, 768],
        ),
        ("vae_block0_norm3", taps.norm3, vec![1, 384, 768]),
        ("vae_block0_ff_in", taps.ff_in, vec![1, 384, 3_072]),
        ("vae_block0_ff_out", taps.ff_out, vec![1, 384, 768]),
        (
            "vae_block0",
            encoded.block_f32(0).map_err(|error| error.to_string())?,
            vec![1, 384, 768],
        ),
    ];
    let mut failures = Vec::new();
    for (name, native, shape) in tensors {
        let reference = load_npy(&oracle.join(format!("{name}.npy")))?;
        require_shape(name, &reference.shape, &shape)?;
        let stats = compare(name, &native, &reference.f32()?)?;
        let (max_gate, mean_gate) = if name.contains("norm") {
            (4.0e-3, 4.0e-4)
        } else {
            (2.0e-2, 2.0e-3)
        };
        if stats.max_abs > max_gate || stats.mean_abs > mean_gate {
            failures.push(format!(
                "{name}: operator parity failed (max {:.9e}, mean {:.9e})",
                stats.max_abs, stats.mean_abs,
            ));
        }
    }
    println!("[vae-operators] elapsed={:.3}s", elapsed.as_secs_f64());
    if !failures.is_empty() {
        return Err(failures.join("; "));
    }
    println!("[vae-operators] PASS first cross block boundary taps");
    Ok(())
}

fn mib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

fn used_gpu_bytes(total: u64, free: u64) -> u64 {
    total.saturating_sub(free)
}

fn validate_encoders(
    weights: &Path,
    oracle: &Path,
    seed: u64,
    warm_runs: usize,
) -> Result<(), String> {
    let weights = SkinTokensWeights::load(weights).map_err(|error| error.to_string())?;
    let condition_npy = load_npy(&oracle.join("cond_input.npy"))?;
    require_shape("condition", &condition_npy.shape, &[1, 54_000, 6])?;
    let condition = condition_npy.f32()?;
    let vae_reference = load_npy(&oracle.join("vae_cond_latents.npy"))?;
    let mesh_reference = load_npy(&oracle.join("mesh_prefix.npy"))?;
    require_shape("vae_cond_latents", &vae_reference.shape, &[1, 384, 512])?;
    require_shape("mesh_prefix", &mesh_reference.shape, &[1, 512, 896])?;
    let vae_reference = vae_reference.f32()?;
    let mesh_reference = mesh_reference.f32()?;

    let baseline = gpu_perf_stats(true);
    let run_once = || -> Result<(Vec<f32>, Vec<f32>, f64), String> {
        let started = std::time::Instant::now();
        let vae = encode_vae_condition(&weights, &condition, seed)
            .map_err(|error| error.to_string())?;
        let mesh = makepad_diffusion::skin_tokens_neural::encode_mesh_prefix(&weights, &condition)
            .map_err(|error| error.to_string())?;
        // Both downloads synchronize the shared CUDA stream, so elapsed time
        // includes every encoder kernel rather than only enqueue latency.
        let vae = vae.latents_f32().map_err(|error| error.to_string())?;
        let mesh = gpu_download(&mesh.prefix)?;
        Ok((vae, mesh, started.elapsed().as_secs_f64()))
    };

    let (mut vae, mut mesh, cold_seconds) = run_once()?;
    let after_cold = gpu_perf_stats(false);
    let mut warm_seconds = Vec::with_capacity(warm_runs);
    for _ in 0..warm_runs {
        let (next_vae, next_mesh, seconds) = run_once()?;
        vae = next_vae;
        mesh = next_mesh;
        warm_seconds.push(seconds);
    }
    let after_warm = gpu_perf_stats(false);
    let vae_stats = compare("vae_cond_latents", &vae, &vae_reference)?;
    let mesh_stats = compare("mesh_prefix", &mesh, &mesh_reference)?;
    let warm_mean = if warm_seconds.is_empty() {
        f64::NAN
    } else {
        warm_seconds.iter().sum::<f64>() / warm_seconds.len() as f64
    };
    let baseline_used = used_gpu_bytes(baseline.mem_total_bytes, baseline.mem_free_bytes);
    let cold_used = used_gpu_bytes(after_cold.mem_total_bytes, after_cold.mem_free_bytes);
    let warm_used = used_gpu_bytes(after_warm.mem_total_bytes, after_warm.mem_free_bytes);
    println!(
        "[encoders] cold={cold_seconds:.3}s warm={warm_seconds:?} warm_mean={warm_mean:.3}s",
    );
    println!(
        "[encoders] VRAM baseline={:.1}MiB cold_resident={:.1}MiB warm_resident={:.1}MiB delta={:.1}MiB total={:.1}MiB",
        mib(baseline_used),
        mib(cold_used),
        mib(warm_used),
        mib(warm_used.saturating_sub(baseline_used)),
        mib(after_warm.mem_total_bytes),
    );
    println!(
        "[encoders] streamed_weights={} ({:.1}MiB) evictions={} pool_fresh={:.1}MiB",
        after_warm.weight_stream_count,
        mib(after_warm.weight_stream_bytes),
        after_warm.weight_evict_events,
        mib(after_warm.pool_fresh_alloc_bytes),
    );
    if vae_stats.cosine < 0.999_9 || vae_stats.max_abs > 3.0e-2 {
        return Err(format!(
            "vae_cond_latents: production parity failed (cosine {:.9}, max {:.9e})",
            vae_stats.cosine, vae_stats.max_abs,
        ));
    }
    if mesh_stats.cosine < 0.999_9 || mesh_stats.max_abs > 4.0e-2 {
        return Err(format!(
            "mesh_prefix: production parity failed (cosine {:.9}, max {:.9e})",
            mesh_stats.cosine, mesh_stats.max_abs,
        ));
    }
    println!("[encoders] PASS production no-tap condition and mesh prefix APIs");
    Ok(())
}

fn bf16_round(value: f32) -> f32 {
    let bits = value.to_bits();
    let rounding_bias = 0x7fff + ((bits >> 16) & 1);
    f32::from_bits(bits.wrapping_add(rounding_bias) & 0xffff_0000)
}

#[allow(clippy::too_many_arguments)]
fn cpu_attention_bf16(
    q: &[f32],
    k: &[f32],
    v: &[f32],
    q_len: usize,
    kv_len: usize,
    hidden: usize,
    heads: usize,
    scale: f32,
) -> Vec<f32> {
    let head_dim = hidden / heads;
    let q = q.iter().copied().map(bf16_round).collect::<Vec<_>>();
    let k = k.iter().copied().map(bf16_round).collect::<Vec<_>>();
    let v = v.iter().copied().map(bf16_round).collect::<Vec<_>>();
    let mut output = vec![0.0f32; q_len * hidden];
    let mut scores = vec![0.0f32; kv_len];
    for head in 0..heads {
        for query in 0..q_len {
            let q_offset = query * hidden + head * head_dim;
            let mut max_score = f32::NEG_INFINITY;
            for key in 0..kv_len {
                let k_offset = key * hidden + head * head_dim;
                let mut dot = 0.0f32;
                for dim in 0..head_dim {
                    dot += q[q_offset + dim] * k[k_offset + dim];
                }
                scores[key] = dot * scale;
                max_score = max_score.max(scores[key]);
            }
            let mut denominator = 0.0f32;
            for score in &mut scores {
                *score = (*score - max_score).exp();
                denominator += *score;
            }
            for score in &mut scores {
                *score = bf16_round(*score / denominator);
            }
            for dim in 0..head_dim {
                let mut sum = 0.0f32;
                for key in 0..kv_len {
                    let v_offset = key * hidden + head * head_dim;
                    sum += scores[key] * v[v_offset + dim];
                }
                output[q_offset + dim] = sum;
            }
        }
    }
    output
}

fn synthetic_tensor(rows: usize, cols: usize, phase: f32) -> Vec<f32> {
    (0..rows * cols)
        .map(|index| {
            let x = index as f32 * 0.017_123 + phase;
            0.55 * x.sin() + 0.31 * (x * 0.37 + 0.2).cos()
        })
        .collect()
}

fn validate_cross_attention_case(
    name: &str,
    q_len: usize,
    kv_len: usize,
    hidden: usize,
    heads: usize,
) -> Result<(), String> {
    let q = synthetic_tensor(q_len, hidden, 0.13);
    let k = synthetic_tensor(kv_len, hidden, 0.71);
    let v = synthetic_tensor(kv_len, hidden, 1.37);
    let scale = 1.0 / ((hidden / heads) as f32).sqrt();
    let reference = cpu_attention_bf16(&q, &k, &v, q_len, kv_len, hidden, heads, scale);
    let q_gpu = gpu_upload(&q, q_len, hidden)?;
    let k_gpu = gpu_upload(&k, kv_len, hidden)?;
    let v_gpu = gpu_upload(&v, kv_len, hidden)?;
    let output = gpu_attention_packed_cross_bf16(&q_gpu, &k_gpu, &v_gpu, heads, scale)?;
    let native = gpu_download(&output)?;
    let stats = compare(name, &native, &reference)?;
    if stats.max_abs > 2.0e-3 || stats.mean_abs > 1.0e-4 {
        return Err(format!(
            "{name}: BF16 attention parity failed (max {:.9e}, mean {:.9e})",
            stats.max_abs, stats.mean_abs,
        ));
    }
    Ok(())
}

fn validate_attention() -> Result<(), String> {
    // Decode-style single query against a long KV cache.
    validate_cross_attention_case("cross_q1_kv515", 1, 515, 32, 4)?;
    // Encoder-style cross attention exercises q offsets and batched heads.
    validate_cross_attention_case("cross_q7_kv13", 7, 13, 32, 4)?;
    // SkinTokens fused BF16 path: d64 with non-tile-aligned query/KV lengths.
    validate_cross_attention_case("flash_d64_cross_q67_kv131", 67, 131, 256, 4)?;

    let seq = 7usize;
    let hidden = 32usize;
    let heads = 4usize;
    let values = synthetic_tensor(seq, hidden, 0.43);
    let scale = 1.0 / ((hidden / heads) as f32).sqrt();
    let reference = cpu_attention_bf16(
        &values, &values, &values, seq, seq, hidden, heads, scale,
    );
    let gpu = gpu_upload(&values, seq, hidden)?;
    let native = gpu_download(&gpu_attention_packed_bf16(
        &gpu, &gpu, &gpu, heads, scale,
    )?)?;
    let stats = compare("self_q7", &native, &reference)?;
    if stats.max_abs > 2.0e-3 || stats.mean_abs > 1.0e-4 {
        return Err(format!(
            "self_q7: BF16 attention parity failed (max {:.9e}, mean {:.9e})",
            stats.max_abs, stats.mean_abs,
        ));
    }
    println!("[attention] PASS explicit BF16 self/cross CUDA contracts");
    Ok(())
}

fn inventory(path: &Path) -> Result<(), String> {
    let weights = SkinTokensWeights::load(path).map_err(|err| err.to_string())?;
    let found = weights.inventory();
    println!("SkinTokens checkpoint: {}", path.display());
    println!(
        "all: tensors={} params={} bytes={}",
        found.all.tensors, found.all.parameters, found.all.bytes
    );
    println!(
        "qwen: tensors={} params={} bytes={}",
        found.qwen.tensors, found.qwen.parameters, found.qwen.bytes
    );
    println!(
        "vae: tensors={} params={} bytes={}",
        found.vae.tensors, found.vae.parameters, found.vae.bytes
    );
    println!(
        "mesh: tensors={} params={} bytes={}",
        found.mesh_encoder.tensors, found.mesh_encoder.parameters, found.mesh_encoder.bytes
    );
    println!(
        "projection: tensors={} params={} bytes={}",
        found.output_projection.tensors,
        found.output_projection.parameters,
        found.output_projection.bytes
    );
    println!("[inventory] PASS");
    Ok(())
}

fn checkpoint_inventory(path: &Path) -> Result<(), String> {
    let state = PthStateDict::load(path).map_err(|err| err.to_string())?;
    let mut names = state.names().cloned().collect::<Vec<_>>();
    names.sort();
    let mut parameters = 0u64;
    let mut dtype_counts = [0usize; 3];
    let mut dtype_params = [0u64; 3];
    for name in &names {
        let count = state
            .shape(name)
            .map_err(|err| err.to_string())?
            .iter()
            .map(|value| *value as u64)
            .product::<u64>();
        parameters += count;
        let dtype_index = match state.dtype(name).map_err(|err| err.to_string())? {
            PthDType::F32 => 0,
            PthDType::F16 => 1,
            PthDType::BF16 => 2,
        };
        dtype_counts[dtype_index] += 1;
        dtype_params[dtype_index] += count;
    }
    println!(
        "official checkpoint: tensors={} params={} first={:?} last={:?}",
        names.len(),
        parameters,
        names.first(),
        names.last(),
    );
    println!(
        "dtypes: F32={}/{} F16={}/{} BF16={}/{} tensors/params",
        dtype_counts[0],
        dtype_params[0],
        dtype_counts[1],
        dtype_params[1],
        dtype_counts[2],
        dtype_params[2],
    );
    Ok(())
}

fn convert_checkpoint(source: &Path, output: &Path) -> Result<(), String> {
    let mut last_percent = usize::MAX;
    let mut progress = |stage: &str, fraction: f64| {
        let percent = (fraction * 100.0).floor() as usize;
        if percent != last_percent {
            eprintln!("[{percent:3}%] {stage}");
            last_percent = percent;
        }
        Ok(())
    };
    let report = convert_skin_tokens_checkpoint(source, output, Some(&mut progress))
        .map_err(|err| err.to_string())?;
    println!(
        "converted: tensors={} params={} bytes={} output={}",
        report.tensors,
        report.parameters,
        report.bytes,
        report.output.display(),
    );
    Ok(())
}

fn run() -> Result<(), String> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    match args.as_slice() {
        [command, path] if command == "inventory" => inventory(Path::new(path)),
        [command, path] if command == "checkpoint" => checkpoint_inventory(Path::new(path)),
        [command, source, output] if command == "convert" => {
            convert_checkpoint(Path::new(source), Path::new(output))
        }
        [command, input, oracle] if command == "mesh" => {
            validate_mesh(Path::new(input), Path::new(oracle), 424242)
        }
        [command, input, oracle, seed] if command == "mesh" => {
            let seed = seed
                .parse::<u32>()
                .map_err(|err| format!("invalid seed {seed:?}: {err}"))?;
            validate_mesh(Path::new(input), Path::new(oracle), seed)
        }
        [command, oracle] if command == "condition" => {
            validate_condition(Path::new(oracle), 424242)
        }
        [command, oracle, seed] if command == "condition" => {
            let seed = seed
                .parse::<u64>()
                .map_err(|err| format!("invalid seed {seed:?}: {err}"))?;
            validate_condition(Path::new(oracle), seed)
        }
        [command, weights, oracle] if command == "projection" => {
            validate_projection(Path::new(weights), Path::new(oracle), 424242)
        }
        [command, weights, oracle, seed] if command == "projection" => {
            let seed = seed
                .parse::<u64>()
                .map_err(|err| format!("invalid seed {seed:?}: {err}"))?;
            validate_projection(Path::new(weights), Path::new(oracle), seed)
        }
        [command, weights, oracle] if command == "projection-lt" => {
            validate_projection_lt(Path::new(weights), Path::new(oracle), 424242)
        }
        [command, weights, oracle, seed] if command == "projection-lt" => {
            let seed = seed
                .parse::<u64>()
                .map_err(|err| format!("invalid seed {seed:?}: {err}"))?;
            validate_projection_lt(Path::new(weights), Path::new(oracle), seed)
        }
        [command, weights, oracle] if command == "mesh-mm" => {
            validate_mesh_mm(Path::new(weights), Path::new(oracle))
        }
        [command, weights, oracle] if command == "mesh-biased-linears" => {
            validate_mesh_biased_linears(Path::new(weights), Path::new(oracle))
        }
        [command, oracle] if command == "fourier-cuda" => {
            validate_michelangelo_fourier_cuda(Path::new(oracle))
        }
        [command, oracle] if command == "mesh-attention-direct" => {
            validate_mesh_attention_direct(Path::new(oracle))
        }
        [command, weights, oracle] if command == "vae-encoder" => {
            validate_vae_encoder(Path::new(weights), Path::new(oracle), 424242)
        }
        [command, weights, oracle, seed] if command == "vae-encoder" => {
            let seed = seed
                .parse::<u64>()
                .map_err(|err| format!("invalid seed {seed:?}: {err}"))?;
            validate_vae_encoder(Path::new(weights), Path::new(oracle), seed)
        }
        [command, weights, oracle] if command == "vae-operators" => {
            validate_vae_operators(Path::new(weights), Path::new(oracle), 424242)
        }
        [command, weights, oracle, seed] if command == "vae-operators" => {
            let seed = seed
                .parse::<u64>()
                .map_err(|err| format!("invalid seed {seed:?}: {err}"))?;
            validate_vae_operators(Path::new(weights), Path::new(oracle), seed)
        }
        [command, weights, oracle] if command == "mesh-encoder" => {
            validate_mesh_encoder(Path::new(weights), Path::new(oracle))
        }
        [command, weights, oracle] if command == "encoders" => {
            validate_encoders(Path::new(weights), Path::new(oracle), 424242, 2)
        }
        [command, weights, oracle, warm_runs] if command == "encoders" => {
            let warm_runs = warm_runs
                .parse::<usize>()
                .map_err(|err| format!("invalid warm run count {warm_runs:?}: {err}"))?;
            validate_encoders(Path::new(weights), Path::new(oracle), 424242, warm_runs)
        }
        [command] if command == "attention" => validate_attention(),
        _ => Err(
            "usage: skin-tokens-validate inventory <tokenrig.safetensors>\n       skin-tokens-validate checkpoint <grpo_1400.ckpt>\n       skin-tokens-validate convert <grpo_1400.ckpt> <tokenrig.safetensors>\n       skin-tokens-validate mesh <input.glb> <official-oracle-dir> [seed]\n       skin-tokens-validate condition <official-neural-oracle-dir> [seed]\n       skin-tokens-validate projection <tokenrig.safetensors> <official-neural-oracle-dir> [seed]\n       skin-tokens-validate projection-lt <tokenrig.safetensors> <official-neural-oracle-dir> [seed]\n       skin-tokens-validate mesh-mm <tokenrig.safetensors> <official-neural-oracle-dir>\n       skin-tokens-validate mesh-biased-linears <tokenrig.safetensors> <official-neural-oracle-dir>\n       skin-tokens-validate fourier-cuda <official-neural-oracle-dir>\n       skin-tokens-validate mesh-attention-direct <official-neural-oracle-dir>\n       skin-tokens-validate vae-encoder <tokenrig.safetensors> <official-neural-oracle-dir> [seed]\n       skin-tokens-validate vae-operators <tokenrig.safetensors> <official-neural-oracle-dir> [seed]\n       skin-tokens-validate mesh-encoder <tokenrig.safetensors> <official-neural-oracle-dir>\n       skin-tokens-validate encoders <tokenrig.safetensors> <official-neural-oracle-dir> [warm-runs]\n       skin-tokens-validate attention"
                .to_string(),
        ),
    }
}

fn main() {
    if let Err(err) = run() {
        eprintln!("skin-tokens-validate: {err}");
        std::process::exit(1);
    }
}
