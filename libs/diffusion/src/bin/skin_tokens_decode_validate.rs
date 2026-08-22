//! Official-oracle parity and warm benchmark for native SkinTokens FSQ/decoder.
//!
//! Usage:
//! `skin-tokens-decode-validate <tokenrig.safetensors> <condition-oracle-dir> <decode-oracle-dir> [warm-runs]`

use makepad_diffusion::backend::{gpu_download, gpu_upload};
use makepad_diffusion::skin_tokens::{SkinTokensWeights, SKIN_TOKENS_SAMPLE_COUNT};
use makepad_diffusion::skin_tokens_decode::{
    decode_skin_tokens_joint_tapped, decode_skin_tokens_weights,
    fsq_indices_to_normalized_codes, replay_skin_tokens_decoder_block0_attention,
    replay_skin_tokens_decoder_block0_attention_composite,
    replay_skin_tokens_decoder_block0_operator, replay_skin_tokens_decoder_bf16_residual,
    replay_skin_tokens_decoder_cross_attention,
    replay_skin_tokens_decoder_cross_attention_composite, SkinTokensDecoderBlock0Replay,
};
use makepad_diffusion::skin_tokens_mesh::SkinTokensMesh;
use makepad_diffusion::skin_tokens_neural::encode_vae_condition;
use makepad_diffusion::skin_tokens_output::skin_tokens_rig_glb;
use makepad_diffusion::skin_tokens_pipeline::{SkinTokensPipeline, SkinTokensPipelineParams};
use makepad_diffusion::skin_tokens_qwen::SkinTokensGenerationGrammar;
use makepad_diffusion::skin_tokens_tokenizer::skin_tokens_detokenize_skeleton;
use makepad_gltf::parse_glb_bytes;
use makepad_micro_serde::{DeJson, JsonValue};
use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::time::Instant;

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
    let (header_len, header_start) = match bytes[6] {
        1 => (u16::from_le_bytes([bytes[8], bytes[9]]) as usize, 10usize),
        2 | 3 => (
            u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize,
            12usize,
        ),
        version => {
            return Err(format!(
                "{}: unsupported npy version {version}",
                path.display()
            ))
        }
    };
    let data_start = header_start
        .checked_add(header_len)
        .filter(|&end| end <= bytes.len())
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
    fn f32(self) -> Result<Vec<f32>, String> {
        match self.descr.as_str() {
            "<f4" => Ok(self
                .data
                .chunks_exact(4)
                .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
                .collect()),
            other => Err(format!("npy dtype {other} is not f32")),
        }
    }

    fn i64(self) -> Result<Vec<i64>, String> {
        match self.descr.as_str() {
            "<i8" => Ok(self
                .data
                .chunks_exact(8)
                .map(|chunk| {
                    i64::from_le_bytes([
                        chunk[0], chunk[1], chunk[2], chunk[3], chunk[4], chunk[5], chunk[6],
                        chunk[7],
                    ])
                })
                .collect()),
            other => Err(format!("npy dtype {other} is not i64")),
        }
    }

    fn f64(self) -> Result<Vec<f64>, String> {
        match self.descr.as_str() {
            "<f8" => Ok(self
                .data
                .chunks_exact(8)
                .map(|chunk| {
                    f64::from_le_bytes([
                        chunk[0], chunk[1], chunk[2], chunk[3], chunk[4], chunk[5], chunk[6],
                        chunk[7],
                    ])
                })
                .collect()),
            other => Err(format!("npy dtype {other} is not f64")),
        }
    }
}

fn load_f32(path: &Path, expected: &[usize]) -> Result<Vec<f32>, String> {
    let npy = load_npy(path)?;
    if npy.shape != expected {
        return Err(format!(
            "{}: shape {:?}, expected {expected:?}",
            path.display(),
            npy.shape,
        ));
    }
    npy.f32()
}

fn json_u64(value: &JsonValue, label: &str) -> Result<u64, String> {
    match value {
        JsonValue::U64(value) => Ok(*value),
        JsonValue::U128(value) => {
            u64::try_from(*value).map_err(|_| format!("{label} overflows u64"))
        }
        JsonValue::I64(value) if *value >= 0 => Ok(*value as u64),
        JsonValue::I128(value) if *value >= 0 => {
            u64::try_from(*value).map_err(|_| format!("{label} overflows u64"))
        }
        JsonValue::F64(value)
            if value.is_finite() && *value >= 0.0 && value.fract() == 0.0 =>
        {
            Ok(*value as u64)
        }
        _ => Err(format!("{label} is not a nonnegative integer")),
    }
}

fn json_array<'a>(
    root: &'a HashMap<String, JsonValue>,
    key: &str,
) -> Result<&'a [JsonValue], String> {
    match root.get(key) {
        Some(JsonValue::Array(values)) => Ok(values),
        _ => Err(format!("strict generation artifact has no {key} array")),
    }
}

fn strict_generation_artifact(path: &Path) -> Result<(u64, Vec<u32>, Vec<usize>), String> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| format!("strict generation artifact {}: {error}", path.display()))?;
    let root = HashMap::<String, JsonValue>::deserialize_json(&text)
        .map_err(|error| format!("strict generation artifact {}: {error:?}", path.display()))?;
    match root.get("policy") {
        Some(JsonValue::String(policy)) | Some(JsonValue::BareIdent(policy))
            if policy == "Strict" => {}
        _ => return Err("generation artifact policy is not Strict".to_string()),
    }
    let seed = json_u64(
        root.get("seed")
            .ok_or_else(|| "strict generation artifact has no seed".to_string())?,
        "strict generation seed",
    )?;
    let skeleton_ids = json_array(&root, "skeleton_ids")?
        .iter()
        .enumerate()
        .map(|(index, value)| {
            u32::try_from(json_u64(value, &format!("skeleton_ids[{index}]"))?)
                .map_err(|_| format!("skeleton_ids[{index}] overflows u32"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let groups = json_array(&root, "fsq_indices")?;
    let mut fsq_indices = Vec::with_capacity(groups.len() * 4);
    for (group_index, group) in groups.iter().enumerate() {
        let JsonValue::Array(group) = group else {
            return Err(format!("fsq_indices[{group_index}] is not an array"));
        };
        if group.len() != 4 {
            return Err(format!(
                "fsq_indices[{group_index}] has {} lanes, expected 4",
                group.len(),
            ));
        }
        for (lane, value) in group.iter().enumerate() {
            let index = usize::try_from(json_u64(
                value,
                &format!("fsq_indices[{group_index}][{lane}]"),
            )?)
            .map_err(|_| format!("fsq_indices[{group_index}][{lane}] overflows usize"))?;
            if index >= 32_768 {
                return Err(format!(
                    "fsq_indices[{group_index}][{lane}]={index} is outside 0..32768",
                ));
            }
            fsq_indices.push(index);
        }
    }
    Ok((seed, skeleton_ids, fsq_indices))
}

struct StrictWeightQuality {
    raw_min: f64,
    raw_max: f64,
    top4_used_joints: usize,
    top1_used_joints: usize,
    max_normalized_sum_error: f64,
    dominance_min: f64,
    dominance_mean: f64,
    dominance_max: f64,
}

fn strict_weight_quality(
    transferred: &[f64],
    vertices: usize,
    joint_count: usize,
) -> Result<StrictWeightQuality, String> {
    if transferred.len() != vertices * joint_count {
        return Err(format!(
            "strict transferred skin has {} values, expected {vertices} x {joint_count}",
            transferred.len(),
        ));
    }
    let mut raw_min = f64::INFINITY;
    let mut raw_max = f64::NEG_INFINITY;
    let mut top4_used = vec![false; joint_count];
    let mut top1_used = vec![false; joint_count];
    let mut order = Vec::with_capacity(joint_count);
    let mut max_normalized_sum_error = 0.0f64;
    let mut dominance_min = f64::INFINITY;
    let mut dominance_max = f64::NEG_INFINITY;
    let mut dominance_sum = 0.0f64;
    for (vertex, row) in transferred.chunks_exact(joint_count).enumerate() {
        if row.iter().any(|value| !value.is_finite() || *value < 0.0) {
            return Err(format!(
                "strict transferred skin row {vertex} contains a non-finite or negative value",
            ));
        }
        for &value in row {
            raw_min = raw_min.min(value);
            raw_max = raw_max.max(value);
        }
        order.clear();
        order.extend(0..joint_count);
        order.sort_unstable_by(|left, right| {
            row[*right]
                .total_cmp(&row[*left])
                .then_with(|| left.cmp(right))
        });
        let kept = 4.min(joint_count);
        let sum = order[..kept].iter().map(|joint| row[*joint]).sum::<f64>();
        if !sum.is_finite() || sum <= 0.0 {
            return Err(format!("strict transferred skin row {vertex} has zero top-four sum"));
        }
        let mut normalized_sum = 0.0f32;
        for &joint in &order[..kept] {
            top4_used[joint] = true;
            normalized_sum += (row[joint] / sum) as f32;
        }
        top1_used[order[0]] = true;
        max_normalized_sum_error =
            max_normalized_sum_error.max((normalized_sum as f64 - 1.0).abs());
        let dominance = row[order[0]] / sum;
        dominance_min = dominance_min.min(dominance);
        dominance_max = dominance_max.max(dominance);
        dominance_sum += dominance;
    }
    let top4_used_joints = top4_used.iter().filter(|used| **used).count();
    let top1_used_joints = top1_used.iter().filter(|used| **used).count();
    if joint_count > 1 && (top4_used_joints < 2 || top1_used_joints < 2) {
        return Err(format!(
            "strict rig has degenerate joint use: top4={top4_used_joints}, top1={top1_used_joints}",
        ));
    }
    Ok(StrictWeightQuality {
        raw_min,
        raw_max,
        top4_used_joints,
        top1_used_joints,
        max_normalized_sum_error,
        dominance_min,
        dominance_mean: dominance_sum / vertices.max(1) as f64,
        dominance_max,
    })
}

fn validate_strict_rigged_glb(
    input: &[u8],
    output: &[u8],
    mesh: &SkinTokensMesh,
    joint_count: usize,
) -> Result<(usize, usize), String> {
    let before = parse_glb_bytes(input).map_err(|error| format!("input GLB: {error}"))?;
    let after = parse_glb_bytes(output).map_err(|error| format!("rigged GLB: {error}"))?;
    for (label, before_count, after_count) in [
        (
            "materials",
            before.document.materials.as_ref().map_or(0, Vec::len),
            after.document.materials.as_ref().map_or(0, Vec::len),
        ),
        (
            "textures",
            before.document.textures.as_ref().map_or(0, Vec::len),
            after.document.textures.as_ref().map_or(0, Vec::len),
        ),
        (
            "images",
            before.document.images.as_ref().map_or(0, Vec::len),
            after.document.images.as_ref().map_or(0, Vec::len),
        ),
        (
            "meshes",
            before.document.meshes.as_ref().map_or(0, Vec::len),
            after.document.meshes.as_ref().map_or(0, Vec::len),
        ),
    ] {
        if before_count != after_count {
            return Err(format!(
                "rigged GLB changed {label} count from {before_count} to {after_count}",
            ));
        }
    }
    let before_nodes = before.document.nodes_slice().len();
    let after_nodes = after.document.nodes_slice().len();
    if after_nodes != before_nodes + joint_count {
        return Err(format!(
            "rigged GLB has {after_nodes} nodes, expected {before_nodes}+{joint_count}",
        ));
    }
    let skins = after
        .document
        .skins
        .as_ref()
        .ok_or_else(|| "rigged GLB has no skins".to_string())?;
    let expected_skin_nodes = mesh
        .parts
        .iter()
        .filter_map(|part| part.node_index)
        .collect::<BTreeSet<_>>();
    if skins.len() != expected_skin_nodes.len() {
        return Err(format!(
            "rigged GLB has {} skins, expected {} skinned mesh nodes",
            skins.len(),
            expected_skin_nodes.len(),
        ));
    }
    for (skin_index, skin) in skins.iter().enumerate() {
        let JsonValue::Object(fields) = skin else {
            return Err(format!("rigged GLB skin {skin_index} is not an object"));
        };
        let Some(JsonValue::Array(joints)) = fields.get("joints") else {
            return Err(format!("rigged GLB skin {skin_index} has no joints array"));
        };
        if joints.len() != joint_count {
            return Err(format!(
                "rigged GLB skin {skin_index} has {} joints, expected {joint_count}",
                joints.len(),
            ));
        }
        if !fields.contains_key("inverseBindMatrices") || !fields.contains_key("skeleton") {
            return Err(format!(
                "rigged GLB skin {skin_index} lacks inverse bind matrices or skeleton root",
            ));
        }
    }
    for part in &mesh.parts {
        let primitive = &after.document.meshes_slice()[part.mesh_index].primitives
            [part.primitive_index];
        for semantic in ["JOINTS_0", "WEIGHTS_0"] {
            let accessor = *primitive.attributes.get(semantic).ok_or_else(|| {
                format!(
                    "rigged GLB mesh {}/{} has no {semantic}",
                    part.mesh_index, part.primitive_index,
                )
            })?;
            let accessor = after
                .document
                .accessors_slice()
                .get(accessor)
                .ok_or_else(|| format!("rigged GLB {semantic} accessor is out of range"))?;
            if accessor.count != part.vertex_count || accessor.accessor_type != "VEC4" {
                return Err(format!(
                    "rigged GLB {semantic} accessor is {} {}, expected {} VEC4",
                    accessor.count, accessor.accessor_type, part.vertex_count,
                ));
            }
        }
        let node = part
            .node_index
            .ok_or_else(|| "strict mesh part has no node".to_string())?;
        if after.document.nodes_slice()[node].skin.is_none() {
            return Err(format!("rigged GLB mesh node {node} has no skin"));
        }
    }
    Ok((skins.len(), mesh.parts.len()))
}

fn run_strict_rig(
    checkpoint: &Path,
    artifact: &Path,
    mesh_path: &Path,
    output_path: &Path,
) -> Result<(), String> {
    let total_started = Instant::now();
    let (seed, skeleton_ids, fsq_indices) = strict_generation_artifact(artifact)?;
    let skeleton = skin_tokens_detokenize_skeleton(&skeleton_ids)
        .map_err(|error| format!("strict skeleton: {error}"))?;
    if fsq_indices.len() != skeleton.joints.len() * 4 {
        return Err(format!(
            "strict generation has {} joints but {} FSQ indices",
            skeleton.joints.len(),
            fsq_indices.len(),
        ));
    }
    let root_count = skeleton.parents.iter().filter(|parent| parent.is_none()).count();
    let mut joint_min = [f32::INFINITY; 3];
    let mut joint_max = [f32::NEG_INFINITY; 3];
    let mut nonzero_bones = 0usize;
    for (joint, head) in skeleton.joints.iter().enumerate() {
        for axis in 0..3 {
            joint_min[axis] = joint_min[axis].min(head[axis]);
            joint_max[axis] = joint_max[axis].max(head[axis]);
        }
        if let Some(parent) = skeleton.parents[joint] {
            let length2 = head
                .iter()
                .zip(skeleton.joints[parent])
                .map(|(value, parent)| (value - parent) * (value - parent))
                .sum::<f32>();
            nonzero_bones += usize::from(length2 > 1.0e-12);
        }
    }
    if root_count != 1 || nonzero_bones == 0 {
        return Err(format!(
            "strict skeleton is degenerate: roots={root_count}, nonzero_bones={nonzero_bones}",
        ));
    }
    println!(
        "[strict-rig] artifact={} policy=Strict seed={seed} joints={} fsq={} roots={root_count} nonzero_bones={nonzero_bones} bounds={joint_min:?}..{joint_max:?}",
        artifact.display(),
        skeleton.joints.len(),
        fsq_indices.len(),
    );

    let input = std::fs::read(mesh_path)
        .map_err(|error| format!("strict input mesh {}: {error}", mesh_path.display()))?;
    let mesh_started = Instant::now();
    let mesh = SkinTokensMesh::from_glb(&input).map_err(|error| error.to_string())?;
    let samples = mesh.sample(seed as u32).map_err(|error| error.to_string())?;
    let condition = samples.condition_f32().map_err(|error| error.to_string())?;
    println!(
        "[strict-rig] mesh vertices={} parts={} samples={} parse+sample={:.6}s",
        mesh.positions.len(),
        mesh.parts.len(),
        samples.positions.len(),
        mesh_started.elapsed().as_secs_f64(),
    );

    let load_started = Instant::now();
    let weights = SkinTokensWeights::load(checkpoint).map_err(|error| error.to_string())?;
    println!("[strict-rig] load={:.6}s", load_started.elapsed().as_secs_f64());
    let condition_started = Instant::now();
    let vae = encode_vae_condition(&weights, &condition, seed)
        .map_err(|error| format!("strict native VAE condition: {error}"))?;
    println!(
        "[strict-rig] native_condition={:.6}s",
        condition_started.elapsed().as_secs_f64(),
    );
    let decode_started = Instant::now();
    let sample_weights = decode_skin_tokens_weights(
        &weights,
        &fsq_indices,
        &condition,
        &vae.latents,
        None,
    )
    .map_err(|error| format!("strict native decoder: {error}"))?;
    let decode_seconds = decode_started.elapsed().as_secs_f64();
    let expected = SKIN_TOKENS_SAMPLE_COUNT * skeleton.joints.len();
    if sample_weights.len() != expected {
        return Err(format!(
            "strict decoder returned {} weights, expected {expected}",
            sample_weights.len(),
        ));
    }
    if sample_weights
        .iter()
        .any(|value| !value.is_finite() || !(0.0..=1.0).contains(value))
    {
        return Err("strict decoder returned a non-finite or out-of-range sigmoid weight".into());
    }
    let sampled_min = sample_weights.iter().copied().fold(f32::INFINITY, f32::min);
    let sampled_max = sample_weights
        .iter()
        .copied()
        .fold(f32::NEG_INFINITY, f32::max);
    println!(
        "[strict-rig] decode={decode_seconds:.6}s per_joint={:.6}s sampled_range={sampled_min:.9}..{sampled_max:.9}",
        decode_seconds / skeleton.joints.len() as f64,
    );

    let transfer_started = Instant::now();
    let transferred = mesh
        .transfer_sample_weights(&samples, &sample_weights, skeleton.joints.len())
        .map_err(|error| format!("strict native transfer: {error}"))?;
    let transfer_seconds = transfer_started.elapsed().as_secs_f64();
    let quality = strict_weight_quality(
        &transferred,
        mesh.positions.len(),
        skeleton.joints.len(),
    )?;
    println!(
        "[strict-rig] transfer={transfer_seconds:.6}s raw_range={:.9}..{:.9} top4_used={}/{} top1_used={}/{} normalized_sum_max_error={:.3e} dominance={:.6}..{:.6} mean={:.6}",
        quality.raw_min,
        quality.raw_max,
        quality.top4_used_joints,
        skeleton.joints.len(),
        quality.top1_used_joints,
        skeleton.joints.len(),
        quality.max_normalized_sum_error,
        quality.dominance_min,
        quality.dominance_max,
        quality.dominance_mean,
    );

    let export_started = Instant::now();
    let output = skin_tokens_rig_glb(&input, &mesh, &samples, &skeleton, &sample_weights)
        .map_err(|error| format!("strict native GLB export: {error}"))?;
    let export_seconds = export_started.elapsed().as_secs_f64();
    let (skins, primitives) =
        validate_strict_rigged_glb(&input, &output, &mesh, skeleton.joints.len())?;
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("strict output directory {}: {error}", parent.display()))?;
    }
    std::fs::write(output_path, &output)
        .map_err(|error| format!("strict output {}: {error}", output_path.display()))?;
    println!(
        "[strict-rig] PASS output={} bytes={} skins={skins} skinned_primitives={primitives} export={export_seconds:.6}s total={:.6}s",
        output_path.display(),
        output.len(),
        total_started.elapsed().as_secs_f64(),
    );
    Ok(())
}

fn skeleton_quality(
    skeleton: &makepad_diffusion::skin_tokens_tokenizer::SkinTokensSkeleton,
) -> Result<(usize, usize, [f32; 3], [f32; 3]), String> {
    let root_count = skeleton.parents.iter().filter(|parent| parent.is_none()).count();
    let mut joint_min = [f32::INFINITY; 3];
    let mut joint_max = [f32::NEG_INFINITY; 3];
    let mut nonzero_bones = 0usize;
    for (joint, head) in skeleton.joints.iter().enumerate() {
        if head.iter().any(|value| !value.is_finite()) {
            return Err(format!("strict skeleton joint {joint} is non-finite"));
        }
        for axis in 0..3 {
            joint_min[axis] = joint_min[axis].min(head[axis]);
            joint_max[axis] = joint_max[axis].max(head[axis]);
        }
        if let Some(parent) = skeleton.parents[joint] {
            if parent >= joint {
                return Err(format!(
                    "strict skeleton joint {joint} has non-prior parent {parent}",
                ));
            }
            let length2 = head
                .iter()
                .zip(skeleton.joints[parent])
                .map(|(value, parent)| (value - parent) * (value - parent))
                .sum::<f32>();
            nonzero_bones += usize::from(length2 > 1.0e-12);
        }
    }
    if root_count != 1 || nonzero_bones == 0 {
        return Err(format!(
            "strict skeleton is degenerate: roots={root_count}, nonzero_bones={nonzero_bones}",
        ));
    }
    Ok((root_count, nonzero_bones, joint_min, joint_max))
}

fn run_production_rig(
    checkpoint: &Path,
    mesh_path: &Path,
    output_path: &Path,
    runs: usize,
    seed: u64,
) -> Result<(), String> {
    if runs == 0 {
        return Err("production-rig runs must be positive".to_string());
    }
    let input = std::fs::read(mesh_path)
        .map_err(|error| format!("production input mesh {}: {error}", mesh_path.display()))?;
    let mesh = SkinTokensMesh::from_glb(&input).map_err(|error| error.to_string())?;
    let load_started = Instant::now();
    let pipeline = SkinTokensPipeline::load(checkpoint)
        .map_err(|error| format!("native SkinTokens pipeline load: {error}"))?;
    println!(
        "[production-rig] load={:.6}s seed={seed} vertices={} parts={} runs={runs}",
        load_started.elapsed().as_secs_f64(),
        mesh.positions.len(),
        mesh.parts.len(),
    );
    let mut params = SkinTokensPipelineParams::default();
    params.seed = seed;
    if params.generation.grammar != SkinTokensGenerationGrammar::Strict {
        return Err("production SkinTokens defaults are not Strict".to_string());
    }
    let mut first = None;
    let mut timings = Vec::with_capacity(runs);
    for run in 0..runs {
        let started = Instant::now();
        let result = pipeline
            .rig_glb(&input, &params, None, None)
            .map_err(|error| format!("native production rig run {}: {error}", run + 1))?;
        let seconds = started.elapsed().as_secs_f64();
        timings.push(seconds);
        if seconds > 33.6 {
            return Err(format!(
                "native production rig run {} took {seconds:.6}s, exceeding 33.6s",
                run + 1,
            ));
        }
        if result.generation.grammar != SkinTokensGenerationGrammar::Strict
            || result.generation.fsq_indices.len() != result.skeleton.joints.len()
        {
            return Err(format!(
                "production run {} violated strict output contract: grammar={:?} joints={} fsq_groups={}",
                run + 1,
                result.generation.grammar,
                result.skeleton.joints.len(),
                result.generation.fsq_indices.len(),
            ));
        }
        let (roots, nonzero_bones, joint_min, joint_max) =
            skeleton_quality(&result.skeleton)?;
        let (skins, primitives) =
            validate_strict_rigged_glb(&input, &result.glb, &mesh, result.skeleton.joints.len())?;
        println!(
            "[production-rig] run={} seconds={seconds:.6} generated={} skeleton_ids={} joints={} genuine_fsq={} roots={roots} nonzero_bones={nonzero_bones} bounds={joint_min:?}..{joint_max:?} bytes={} skins={skins} skinned_primitives={primitives}",
            run + 1,
            result.generation.generated_ids.len(),
            result.generation.skeleton_ids.len(),
            result.skeleton.joints.len(),
            result.generation.fsq_indices.len() * 4,
            result.glb.len(),
        );
        if let Some(first) = first.as_ref() {
            if first != &result.glb {
                return Err(format!(
                    "native production rig run {} is not byte-deterministic",
                    run + 1,
                ));
            }
        } else {
            first = Some(result.glb);
        }
    }
    let output = first.expect("runs is positive");
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("production output directory {}: {error}", parent.display()))?;
    }
    std::fs::write(output_path, &output)
        .map_err(|error| format!("production output {}: {error}", output_path.display()))?;
    timings.sort_by(f64::total_cmp);
    println!(
        "[production-rig] PASS output={} bytes={} median={:.6}s min={:.6}s max={:.6}s deterministic=true",
        output_path.display(),
        output.len(),
        timings[timings.len() / 2],
        timings[0],
        timings[timings.len() - 1],
    );
    Ok(())
}

#[derive(Clone, Copy)]
struct Metrics {
    cosine: f64,
    max_abs: f64,
    mean_abs: f64,
    rms: f64,
    max_index: usize,
}

fn compare(name: &str, native: &[f32], oracle: &[f32]) -> Result<Metrics, String> {
    if native.len() != oracle.len() {
        return Err(format!(
            "{name}: native/oracle lengths differ: {}/{}",
            native.len(),
            oracle.len(),
        ));
    }
    let mut dot = 0.0f64;
    let mut norm_native = 0.0f64;
    let mut norm_oracle = 0.0f64;
    let mut max_abs = 0.0f64;
    let mut max_index = 0usize;
    let mut sum_abs = 0.0f64;
    let mut sum_sq = 0.0f64;
    for (index, (&native, &oracle)) in native.iter().zip(oracle).enumerate() {
        if !native.is_finite() {
            return Err(format!("{name}: non-finite native value at {index}"));
        }
        let native = native as f64;
        let oracle = oracle as f64;
        dot += native * oracle;
        norm_native += native * native;
        norm_oracle += oracle * oracle;
        let difference = (native - oracle).abs();
        sum_abs += difference;
        sum_sq += difference * difference;
        if difference > max_abs {
            max_abs = difference;
            max_index = index;
        }
    }
    let count = native.len().max(1) as f64;
    let metrics = Metrics {
        cosine: dot / (norm_native.sqrt() * norm_oracle.sqrt()).max(f64::MIN_POSITIVE),
        max_abs,
        mean_abs: sum_abs / count,
        rms: (sum_sq / count).sqrt(),
        max_index,
    };
    println!(
        "[{name}] cosine={:.10} max_abs={:.9e} mean_abs={:.9e} rms={:.9e} max_i={} native={:.9e} oracle={:.9e}",
        metrics.cosine,
        metrics.max_abs,
        metrics.mean_abs,
        metrics.rms,
        metrics.max_index,
        native[metrics.max_index],
        oracle[metrics.max_index],
    );
    Ok(metrics)
}

fn compare_f64(name: &str, native: &[f64], oracle: &[f64]) -> Result<Metrics, String> {
    if native.len() != oracle.len() {
        return Err(format!(
            "{name}: native/oracle lengths differ: {}/{}",
            native.len(),
            oracle.len(),
        ));
    }
    let mut dot = 0.0f64;
    let mut norm_native = 0.0f64;
    let mut norm_oracle = 0.0f64;
    let mut max_abs = 0.0f64;
    let mut max_index = 0usize;
    let mut sum_abs = 0.0f64;
    let mut sum_sq = 0.0f64;
    for (index, (&native, &oracle)) in native.iter().zip(oracle).enumerate() {
        if !native.is_finite() {
            return Err(format!("{name}: non-finite native value at {index}"));
        }
        dot += native * oracle;
        norm_native += native * native;
        norm_oracle += oracle * oracle;
        let difference = (native - oracle).abs();
        sum_abs += difference;
        sum_sq += difference * difference;
        if difference > max_abs {
            max_abs = difference;
            max_index = index;
        }
    }
    let count = native.len().max(1) as f64;
    let metrics = Metrics {
        cosine: dot / (norm_native.sqrt() * norm_oracle.sqrt()).max(f64::MIN_POSITIVE),
        max_abs,
        mean_abs: sum_abs / count,
        rms: (sum_sq / count).sqrt(),
        max_index,
    };
    println!(
        "[{name}] cosine={:.10} max_abs={:.9e} mean_abs={:.9e} rms={:.9e} max_i={} native={:.9e} oracle={:.9e}",
        metrics.cosine,
        metrics.max_abs,
        metrics.mean_abs,
        metrics.rms,
        metrics.max_index,
        native[metrics.max_index],
        oracle[metrics.max_index],
    );
    Ok(metrics)
}

fn require_gate(
    name: &str,
    metrics: Metrics,
    minimum_cosine: f64,
    maximum_abs: f64,
) -> Result<(), String> {
    if metrics.cosine < minimum_cosine || metrics.max_abs > maximum_abs {
        return Err(format!(
            "{name}: parity gate failed: cosine {:.10} < {minimum_cosine:.10} or max_abs {:.9e} > {maximum_abs:.9e}",
            metrics.cosine, metrics.max_abs,
        ));
    }
    Ok(())
}

fn print_attention_diff_distribution(
    name: &str,
    native: &[f32],
    oracle: &[f32],
    rows: usize,
    heads: usize,
    head_dim: usize,
) -> Result<(), String> {
    let expected = rows
        .checked_mul(heads)
        .and_then(|value| value.checked_mul(head_dim))
        .ok_or_else(|| format!("{name}: attention shape overflow"))?;
    if native.len() != expected || oracle.len() != expected {
        return Err(format!(
            "{name}: attention distribution shape mismatch native={} oracle={} expected={expected}",
            native.len(),
            oracle.len(),
        ));
    }
    let mut differing = Vec::new();
    let mut differing_rows = vec![0usize; rows];
    let mut differing_heads = vec![0usize; heads];
    for (index, (&got, &want)) in native.iter().zip(oracle).enumerate() {
        let difference = (got - want).abs();
        if difference != 0.0 {
            let row = index / (heads * head_dim);
            let head = (index / head_dim) % heads;
            differing_rows[row] += 1;
            differing_heads[head] += 1;
            differing.push((difference, index, got, want));
        }
    }
    differing.sort_unstable_by(|left, right| right.0.total_cmp(&left.0));
    let row_count = differing_rows.iter().filter(|&&count| count != 0).count();
    let head_summary = differing_heads
        .iter()
        .enumerate()
        .filter(|(_, count)| **count != 0)
        .map(|(head, count)| format!("{head}:{count}"))
        .collect::<Vec<_>>()
        .join(",");
    let top = differing
        .iter()
        .take(8)
        .map(|&(difference, index, got, want)| {
            let row = index / (heads * head_dim);
            let col = index % (heads * head_dim);
            let head = col / head_dim;
            let lane = col % head_dim;
            format!("r{row}/h{head}/d{lane}:{got:.9e}/{want:.9e}/{difference:.9e}")
        })
        .collect::<Vec<_>>()
        .join(",");
    println!(
        "[attention-diff {name}] elements={}/{} rows={row_count}/{rows} heads=[{head_summary}] top=[{top}]",
        differing.len(),
        expected,
    );
    Ok(())
}

fn metadata_median(path: &Path) -> Result<f64, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    let suffix = text
        .split("\"benchmark_median_seconds\":")
        .nth(1)
        .ok_or_else(|| format!("{}: benchmark median is missing", path.display()))?;
    suffix
        .trim_start()
        .split([',', '\n', '\r'])
        .next()
        .ok_or_else(|| format!("{}: benchmark median is empty", path.display()))?
        .trim()
        .parse::<f64>()
        .map_err(|error| format!("{}: invalid benchmark median: {error}", path.display()))
}

fn median(mut values: Vec<f64>) -> f64 {
    values.sort_by(f64::total_cmp);
    let middle = values.len() / 2;
    if values.len() % 2 == 0 {
        (values[middle - 1] + values[middle]) * 0.5
    } else {
        values[middle]
    }
}

fn compare_transferred_top4(
    transferred: &[f64],
    vertices: usize,
    joints: usize,
    oracle_dir: &Path,
) -> Result<(), String> {
    if transferred.len() != vertices * joints {
        return Err(format!(
            "native transfer has {} values, expected {vertices}x{joints}",
            transferred.len(),
        ));
    }
    let transferred_npy = load_npy(&oracle_dir.join("transferred_skin.npy"))?;
    if transferred_npy.shape != [vertices, joints] {
        return Err(format!(
            "transferred oracle shape {:?}, expected [{vertices}, {joints}]",
            transferred_npy.shape,
        ));
    }
    let transferred_oracle = transferred_npy.f64()?;
    let transfer_metrics = compare_f64("transferred_skin_34j", transferred, &transferred_oracle)?;

    let joints_npy = load_npy(&oracle_dir.join("top4_joints.npy"))?;
    if joints_npy.shape != [vertices, 4] {
        return Err(format!(
            "top-four joint oracle shape {:?}, expected [{vertices}, 4]",
            joints_npy.shape,
        ));
    }
    let oracle_joints = joints_npy.i64()?;
    let weights_npy = load_npy(&oracle_dir.join("top4_weights.npy"))?;
    if weights_npy.shape != [vertices, 4] {
        return Err(format!(
            "top-four weight oracle shape {:?}, expected [{vertices}, 4]",
            weights_npy.shape,
        ));
    }
    let oracle_weights = weights_npy.f64()?;

    let mut native_joints = Vec::with_capacity(vertices * 4);
    let mut native_weights = Vec::with_capacity(vertices * 4);
    let mut order = Vec::with_capacity(joints);
    for row in transferred.chunks_exact(joints) {
        order.clear();
        order.extend(0..joints);
        // This is the production GLB contract. The official full-J oracle has
        // no equal values inside or at the top-four boundary, so NumPy's
        // unstable quicksort tie order cannot affect this acceptance asset.
        order.sort_unstable_by(|left, right| {
            row[*right]
                .total_cmp(&row[*left])
                .then_with(|| left.cmp(right))
        });
        let sum = order[..4].iter().map(|&joint| row[joint]).sum::<f64>();
        for &joint in &order[..4] {
            native_joints.push(joint as i64);
            native_weights.push(row[joint] / sum);
        }
    }
    let differing_lanes = native_joints
        .iter()
        .zip(&oracle_joints)
        .filter(|(native, oracle)| native != oracle)
        .count();
    let differing_rows = native_joints
        .chunks_exact(4)
        .zip(oracle_joints.chunks_exact(4))
        .filter(|(native, oracle)| native != oracle)
        .count();
    let weight_metrics = compare_f64(
        "transferred_top4_weights_34j",
        &native_weights,
        &oracle_weights,
    )?;
    println!(
        "[task-top4] matching_rows={}/{} differing_lanes={}/{}",
        vertices - differing_rows,
        vertices,
        differing_lanes,
        vertices * 4,
    );
    require_gate("transferred_skin_34j", transfer_metrics, 0.999_9, 2.0e-3)?;
    if differing_rows != 0 {
        return Err(format!(
            "transferred top-four joints differ on {differing_rows}/{vertices} rows",
        ));
    }
    require_gate(
        "transferred_top4_weights_34j",
        weight_metrics,
        0.999_999,
        2.0e-3,
    )
}

fn main() -> Result<(), String> {
    const USAGE: &str = "usage: skin-tokens-decode-validate <tokenrig.safetensors> <condition-oracle-dir> <decode-oracle-dir> [warm-runs] [generation-oracle-dir] [mesh.glb transfer-oracle-dir]\n       skin-tokens-decode-validate strict-rig <tokenrig.safetensors> <native-strict-generation.json> <mesh.glb> <output.glb>\n       skin-tokens-decode-validate production-rig <tokenrig.safetensors> <mesh.glb> <output.glb> [runs] [seed]";
    let collected = std::env::args().skip(1).collect::<Vec<_>>();
    if let [command, checkpoint, artifact, mesh, output] = collected.as_slice() {
        if command == "strict-rig" {
            return run_strict_rig(
                Path::new(checkpoint),
                Path::new(artifact),
                Path::new(mesh),
                Path::new(output),
            );
        }
    }
    if matches!(collected.first().map(String::as_str), Some("production-rig")) {
        if !(4..=6).contains(&collected.len()) {
            return Err(USAGE.to_string());
        }
        let runs = collected
            .get(4)
            .map(|value| value.parse::<usize>())
            .transpose()
            .map_err(|error| format!("invalid production-rig runs: {error}"))?
            .unwrap_or(2);
        let seed = collected
            .get(5)
            .map(|value| value.parse::<u64>())
            .transpose()
            .map_err(|error| format!("invalid production-rig seed: {error}"))?
            .unwrap_or(424_242);
        return run_production_rig(
            Path::new(&collected[1]),
            Path::new(&collected[2]),
            Path::new(&collected[3]),
            runs,
            seed,
        );
    }
    let mut args = collected.into_iter();
    let checkpoint = args.next().ok_or_else(|| USAGE.to_string())?;
    let condition_oracle = args.next().ok_or_else(|| USAGE.to_string())?;
    let decode_oracle = args.next().ok_or_else(|| USAGE.to_string())?;
    let warm_runs = args
        .next()
        .map(|value| value.parse::<usize>())
        .transpose()
        .map_err(|error| format!("invalid warm-runs: {error}"))?
        .unwrap_or(5);
    let generation_oracle = args.next();
    let task_mesh = args.next();
    let transfer_oracle = args.next();
    if args.next().is_some() || task_mesh.is_some() != transfer_oracle.is_some() {
        return Err(format!(
            "mesh.glb and transfer-oracle-dir must be supplied together\n{USAGE}",
        ));
    }
    if task_mesh.is_some() && generation_oracle.is_none() {
        return Err(format!(
            "full transfer validation requires generation-oracle-dir\n{USAGE}",
        ));
    }
    if warm_runs == 0 {
        return Err("warm-runs must be positive".to_string());
    }
    let checkpoint = Path::new(&checkpoint);
    let condition_oracle = Path::new(&condition_oracle);
    let decode_oracle = Path::new(&decode_oracle);

    let condition = load_f32(
        &condition_oracle.join("cond_input.npy"),
        &[1, SKIN_TOKENS_SAMPLE_COUNT, 6],
    )?;
    let condition_latents = load_f32(
        &condition_oracle.join("vae_cond_latents.npy"),
        &[1, 384, 512],
    )?;
    let fsq_npy = load_npy(&decode_oracle.join("fsq_indices.npy"))?;
    if fsq_npy.shape != [1, 4] {
        return Err(format!(
            "fsq_indices shape {:?}, expected [1, 4]",
            fsq_npy.shape,
        ));
    }
    // The first oracle capture's generic tensor saver promoted this integer
    // tap to f32. Accept both encodings but require exact nonnegative integers;
    // all meaningful parity is subsequently gated against the FSQ code tensor.
    let fsq_values: Vec<f64> = match fsq_npy.descr.as_str() {
        "<i8" => fsq_npy.i64()?.into_iter().map(|value| value as f64).collect(),
        "<f4" => fsq_npy.f32()?.into_iter().map(|value| value as f64).collect(),
        other => return Err(format!("fsq_indices dtype {other} is unsupported")),
    };
    let fsq_indices = fsq_values
        .into_iter()
        .map(|index| {
            if !index.is_finite() || index < 0.0 || index.fract() != 0.0 {
                return Err(format!("invalid FSQ index {index}"));
            }
            usize::try_from(index as u64).map_err(|_| format!("FSQ index {index} overflows usize"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let normalized = fsq_indices_to_normalized_codes(&fsq_indices)
        .map_err(|error| error.to_string())?;
    let normalized_oracle = load_f32(
        &decode_oracle.join("fsq_normalized_codes.npy"),
        &[1, 4, 5],
    )?;
    require_gate(
        "fsq_normalized_codes",
        compare("fsq_normalized_codes", &normalized, &normalized_oracle)?,
        1.0,
        0.0,
    )?;

    let load_started = Instant::now();
    let weights = SkinTokensWeights::load(checkpoint).map_err(|error| error.to_string())?;
    let condition_latents_device = gpu_upload(&condition_latents, 384, 512)?;
    println!("[setup] {:.6}s", load_started.elapsed().as_secs_f64());

    let tap_started = Instant::now();
    let tap = decode_skin_tokens_joint_tapped(
        &weights,
        &fsq_indices,
        &condition,
        &condition_latents_device,
        None,
    )
    .map_err(|error| error.to_string())?;
    let sampled_weights = tap.sampled_weights_f32().map_err(|error| error.to_string())?;
    println!("[parity-pass] {:.6}s", tap_started.elapsed().as_secs_f64());

    let stages = [
        (
            "indices_to_codes",
            tap.indices_to_codes_f32().map_err(|error| error.to_string())?,
            vec![1, 4, 512],
            0.999_999_9,
            3.90625e-3,
        ),
        (
            "post_quant",
            tap.post_quant_f32().map_err(|error| error.to_string())?,
            vec![1, 388, 768],
            0.999_99,
            1.5625e-2,
        ),
        (
            "decoder_block0",
            tap.block_f32(0).map_err(|error| error.to_string())?,
            vec![1, 388, 768],
            0.999_9,
            6.25e-2,
        ),
        (
            "decoder_block4",
            tap.block_f32(4).map_err(|error| error.to_string())?,
            vec![1, 388, 768],
            0.999_9,
            6.25e-2,
        ),
        (
            "decoder_block9",
            tap.block_f32(9).map_err(|error| error.to_string())?,
            vec![1, 388, 768],
            0.999_9,
            1.25e-1,
        ),
        (
            "sampled_weight_column",
            sampled_weights,
            vec![1, SKIN_TOKENS_SAMPLE_COUNT, 1],
            0.999_9,
            1.5625e-2,
        ),
    ];
    let operator_stages = [
        ("decoder_block0_norm1", "norm1", 768, 2.0e-3),
        ("decoder_block0_q", "q", 768, 0.0),
        ("decoder_block0_k", "k", 768, 0.0),
        ("decoder_block0_v", "v", 768, 0.0),
        (
            "decoder_block0_attention",
            "attention",
            768,
            7.8125e-3,
        ),
        (
            "decoder_block0_attention_out",
            "attention_out",
            768,
            7.8125e-3,
        ),
        (
            "decoder_block0_attention_residual",
            "attention_residual",
            768,
            7.8125e-3,
        ),
        ("decoder_block0_norm3", "norm3", 768, 7.8125e-3),
        ("decoder_block0_ff_in", "ff_in", 3072, 3.125e-2),
        ("decoder_block0_gelu", "gelu", 3072, 3.125e-2),
        ("decoder_block0_ff_out", "ff_out", 768, 7.8125e-3),
    ];
    let mut parity_failures = Vec::new();

    // Direct official-input replays distinguish the operator itself from
    // propagated error. In particular, end-to-end `attention_out` consumes
    // the already-divergent native flash result, while this replay feeds the
    // official attention tensor into only the output addmm.
    let direct_q = load_f32(
        &decode_oracle.join("decoder_block0_q.npy"),
        &[1, 388, 768],
    )?;
    let direct_k = load_f32(
        &decode_oracle.join("decoder_block0_k.npy"),
        &[1, 388, 768],
    )?;
    let direct_v = load_f32(
        &decode_oracle.join("decoder_block0_v.npy"),
        &[1, 388, 768],
    )?;
    let direct_q = gpu_upload(&direct_q, 388, 768)?;
    let direct_k = gpu_upload(&direct_k, 388, 768)?;
    let direct_v = gpu_upload(&direct_v, 388, 768)?;
    let direct_attention = replay_skin_tokens_decoder_block0_attention(
        &direct_q,
        &direct_k,
        &direct_v,
    )
    .map_err(|error| error.to_string())?;
    let direct_attention = gpu_download(&direct_attention)?;
    let direct_attention_oracle = load_f32(
        &decode_oracle.join("decoder_block0_attention.npy"),
        &[1, 388, 768],
    )?;
    let metrics = compare(
        "direct_decoder_block0_attention",
        &direct_attention,
        &direct_attention_oracle,
    )?;
    print_attention_diff_distribution(
        "direct_decoder_block0_attention",
        &direct_attention,
        &direct_attention_oracle,
        388,
        12,
        64,
    )?;
    if let Err(error) = require_gate(
        "direct_decoder_block0_attention",
        metrics,
        0.999_99,
        7.8125e-3,
    ) {
        println!("[parity-warning] {error}");
        parity_failures.push(error);
    }
    let direct_attention_composite = replay_skin_tokens_decoder_block0_attention_composite(
        &direct_q,
        &direct_k,
        &direct_v,
    )
    .map_err(|error| error.to_string())?;
    let direct_attention_composite = gpu_download(&direct_attention_composite)?;
    let composite_metrics = compare(
        "direct_decoder_block0_attention_composite",
        &direct_attention_composite,
        &direct_attention_oracle,
    )?;
    print_attention_diff_distribution(
        "direct_decoder_block0_attention_composite",
        &direct_attention_composite,
        &direct_attention_oracle,
        388,
        12,
        64,
    )?;
    if let Err(error) = require_gate(
        "direct_decoder_block0_attention_composite",
        composite_metrics,
        1.0,
        0.0,
    ) {
        println!("[parity-diagnostic] {error}");
    }
    let direct_cross_q = load_f32(
        &decode_oracle.join("decoder_cross_q_rows.npy"),
        &[1, 5, 768],
    )?;
    let direct_cross_k = load_f32(
        &decode_oracle.join("decoder_cross_k.npy"),
        &[1, 388, 768],
    )?;
    let direct_cross_v = load_f32(
        &decode_oracle.join("decoder_cross_v.npy"),
        &[1, 388, 768],
    )?;
    let direct_cross_q = gpu_upload(&direct_cross_q, 5, 768)?;
    let direct_cross_k = gpu_upload(&direct_cross_k, 388, 768)?;
    let direct_cross_v = gpu_upload(&direct_cross_v, 388, 768)?;
    let direct_cross_attention = replay_skin_tokens_decoder_cross_attention(
        &direct_cross_q,
        &direct_cross_k,
        &direct_cross_v,
    )
    .map_err(|error| error.to_string())?;
    let direct_cross_attention = gpu_download(&direct_cross_attention)?;
    let direct_cross_attention_oracle = load_f32(
        &decode_oracle.join("decoder_cross_attention_rows.npy"),
        &[1, 5, 768],
    )?;
    let cross_metrics = compare(
        "direct_decoder_cross_attention_rows",
        &direct_cross_attention,
        &direct_cross_attention_oracle,
    )?;
    print_attention_diff_distribution(
        "direct_decoder_cross_attention_rows",
        &direct_cross_attention,
        &direct_cross_attention_oracle,
        5,
        12,
        64,
    )?;
    if let Err(error) = require_gate(
        "direct_decoder_cross_attention_rows",
        cross_metrics,
        0.999_99,
        7.8125e-3,
    ) {
        println!("[parity-warning] {error}");
        parity_failures.push(error);
    }
    let direct_cross_attention_composite =
        replay_skin_tokens_decoder_cross_attention_composite(
            &direct_cross_q,
            &direct_cross_k,
            &direct_cross_v,
        )
        .map_err(|error| error.to_string())?;
    let direct_cross_attention_composite = gpu_download(&direct_cross_attention_composite)?;
    let cross_composite_metrics = compare(
        "direct_decoder_cross_attention_rows_composite",
        &direct_cross_attention_composite,
        &direct_cross_attention_oracle,
    )?;
    print_attention_diff_distribution(
        "direct_decoder_cross_attention_rows_composite",
        &direct_cross_attention_composite,
        &direct_cross_attention_oracle,
        5,
        12,
        64,
    )?;
    if let Err(error) = require_gate(
        "direct_decoder_cross_attention_rows_composite",
        cross_composite_metrics,
        1.0,
        0.0,
    ) {
        println!("[parity-diagnostic] {error}");
    }
    for (name, left_name, right_name, output_name) in [
        (
            "direct_decoder_block0_attention_residual",
            "post_quant",
            "decoder_block0_attention_out",
            "decoder_block0_attention_residual",
        ),
        (
            "direct_decoder_block0_final_residual",
            "decoder_block0_attention_residual",
            "decoder_block0_ff_out",
            "decoder_block0",
        ),
    ] {
        let left = load_f32(
            &decode_oracle.join(format!("{left_name}.npy")),
            &[1, 388, 768],
        )?;
        let right = load_f32(
            &decode_oracle.join(format!("{right_name}.npy")),
            &[1, 388, 768],
        )?;
        let oracle = load_f32(
            &decode_oracle.join(format!("{output_name}.npy")),
            &[1, 388, 768],
        )?;
        let left = gpu_upload(&left, 388, 768)?;
        let right = gpu_upload(&right, 388, 768)?;
        let native = replay_skin_tokens_decoder_bf16_residual(&left, &right)
            .map_err(|error| error.to_string())?;
        let native = gpu_download(&native)?;
        let metrics = compare(name, &native, &oracle)?;
        if let Err(error) = require_gate(name, metrics, 1.0, 0.0) {
            println!("[parity-warning] {error}");
            parity_failures.push(error);
        }
    }
    for (operator, input_name, input_cols, output_name, output_cols) in [
        (
            SkinTokensDecoderBlock0Replay::AttentionOut,
            "decoder_block0_attention",
            768,
            "decoder_block0_attention_out",
            768,
        ),
        (
            SkinTokensDecoderBlock0Replay::Norm3,
            "decoder_block0_attention_residual",
            768,
            "decoder_block0_norm3",
            768,
        ),
        (
            SkinTokensDecoderBlock0Replay::FfIn,
            "decoder_block0_norm3",
            768,
            "decoder_block0_ff_in",
            3072,
        ),
        (
            SkinTokensDecoderBlock0Replay::Gelu,
            "decoder_block0_ff_in",
            3072,
            "decoder_block0_gelu",
            3072,
        ),
        (
            SkinTokensDecoderBlock0Replay::FfOut,
            "decoder_block0_gelu",
            3072,
            "decoder_block0_ff_out",
            768,
        ),
        (
            SkinTokensDecoderBlock0Replay::FfOutBiasEpilogue,
            "decoder_block0_gelu",
            3072,
            "decoder_block0_ff_out",
            768,
        ),
    ] {
        let input = load_f32(
            &decode_oracle.join(format!("{input_name}.npy")),
            &[1, 388, input_cols],
        )?;
        let input = gpu_upload(&input, 388, input_cols)?;
        let native = replay_skin_tokens_decoder_block0_operator(&weights, &input, operator)
            .map_err(|error| error.to_string())?;
        let native = gpu_download(&native)?;
        let oracle = load_f32(
            &decode_oracle.join(format!("{output_name}.npy")),
            &[1, 388, output_cols],
        )?;
        let name = match operator {
            SkinTokensDecoderBlock0Replay::FfOutBiasEpilogue => {
                format!("direct_{output_name}_bias_epilogue")
            }
            _ => format!("direct_{output_name}"),
        };
        let metrics = compare(&name, &native, &oracle)?;
        if let Err(error) = require_gate(&name, metrics, 0.999_999, 0.0) {
            if matches!(operator, SkinTokensDecoderBlock0Replay::FfOut) {
                // The production self-block FF-down uses the bias-epilogue
                // replay immediately below. Keep the legacy GEMM result as a
                // root-cause diagnostic without making it a production gate.
                println!("[parity-diagnostic] {error}");
            } else {
                println!("[parity-warning] {error}");
                parity_failures.push(error);
            }
        }
    }

    for (oracle_name, tap_name, cols, maximum_abs) in operator_stages {
        let native = tap
            .block0_operator_f32(tap_name)
            .map_err(|error| error.to_string())?;
        let oracle = load_f32(
            &decode_oracle.join(format!("{oracle_name}.npy")),
            &[1, 388, cols],
        )?;
        let metrics = compare(oracle_name, &native, &oracle)?;
        if let Err(error) = require_gate(oracle_name, metrics, 0.999_99, maximum_abs) {
            println!("[parity-warning] {error}");
            parity_failures.push(error);
        }
    }
    // The 54k-query cross block is the first boundary after the compact
    // decoder cache and the dominant production kernel. Keep these operator
    // taps separate from the end-to-end rows so a final-weight failure cannot
    // be hidden by a high aggregate cosine over 54,000 samples.
    for (oracle_name, tap_name, shape, minimum_cosine, maximum_abs) in [
        (
            "decoder_cross_norm2_rows",
            "norm2_rows",
            vec![1, 5, 768],
            0.999_999,
            1.953_125e-3,
        ),
        (
            "decoder_cross_norm_cache",
            "norm_cache",
            vec![1, 388, 768],
            0.999_999,
            1.953_125e-3,
        ),
        (
            "decoder_cross_q_rows",
            "q_rows",
            vec![1, 5, 768],
            0.999_999,
            3.906_25e-3,
        ),
        (
            "decoder_cross_k",
            "k",
            vec![1, 388, 768],
            0.999_999,
            3.906_25e-3,
        ),
        (
            "decoder_cross_v",
            "v",
            vec![1, 388, 768],
            0.999_999,
            3.906_25e-3,
        ),
        (
            "decoder_cross_attention_rows",
            "attention_rows",
            vec![1, 5, 768],
            0.999_999,
            3.906_25e-3,
        ),
        (
            "decoder_cross_attention_out_rows",
            "attention_out_rows",
            vec![1, 5, 768],
            0.999_999,
            3.906_25e-3,
        ),
        (
            "decoder_cross_norm3_rows",
            "norm3_rows",
            vec![1, 5, 768],
            0.999_999,
            1.953_125e-3,
        ),
        (
            "decoder_cross_ff_in_rows",
            "ff_in_rows",
            vec![1, 5, 3072],
            0.999_999,
            3.906_25e-3,
        ),
        (
            "decoder_cross_ff_out_rows",
            "ff_out_rows",
            vec![1, 5, 768],
            0.999_999,
            3.906_25e-3,
        ),
    ] {
        let native = tap
            .cross_operator_f32(tap_name)
            .map_err(|error| error.to_string())?;
        let oracle = load_f32(&decode_oracle.join(format!("{oracle_name}.npy")), &shape)?;
        let metrics = compare(oracle_name, &native, &oracle)?;
        if let Err(error) = require_gate(oracle_name, metrics, minimum_cosine, maximum_abs) {
            println!("[parity-warning] {error}");
            parity_failures.push(error);
        }
    }
    for (name, native, shape, minimum_cosine, maximum_abs) in stages {
        let oracle = load_f32(&decode_oracle.join(format!("{name}.npy")), &shape)?;
        let metrics = compare(name, &native, &oracle)?;
        if let Err(error) = require_gate(name, metrics, minimum_cosine, maximum_abs) {
            println!("[parity-warning] {error}");
            parity_failures.push(error);
        }
    }
    for (name, shape, minimum_cosine, maximum_abs) in [
        ("query_projection_rows", vec![1, 5, 768], 0.999_999, 3.90625e-3),
        ("decoder_cross_rows", vec![1, 5, 768], 0.999_9, 6.25e-2),
        ("decoder_norm_rows", vec![1, 5, 768], 0.999_9, 6.25e-2),
        (
            "raw_weight_logits",
            vec![1, SKIN_TOKENS_SAMPLE_COUNT, 1],
            0.999_9,
            1.25e-1,
        ),
    ] {
        let native = tap
            .output_stage_f32(name)
            .map_err(|error| error.to_string())?;
        let oracle = load_f32(&decode_oracle.join(format!("{name}.npy")), &shape)?;
        let metrics = compare(name, &native, &oracle)?;
        if let Err(error) = require_gate(name, metrics, minimum_cosine, maximum_abs) {
            println!("[parity-warning] {error}");
            parity_failures.push(error);
        }
    }
    if parity_failures.is_empty() {
        println!("[parity] PASS all staged BF16 gates");
    }

    if makepad_ai_common::backend::prof::enabled() {
        // Discard parity-pass counters so every following report describes
        // one tap-free production decode, including its synchronization cost.
        let _ = makepad_ai_common::backend::prof::report_and_reset("");
    }
    let mut timings = Vec::with_capacity(warm_runs);
    for run in 0..warm_runs {
        let started = Instant::now();
        let values = decode_skin_tokens_weights(
            &weights,
            &fsq_indices,
            &condition,
            &condition_latents_device,
            None,
        )
        .map_err(|error| error.to_string())?;
        if values.len() != SKIN_TOKENS_SAMPLE_COUNT {
            return Err(format!("benchmark output has {} values", values.len()));
        }
        let elapsed = started.elapsed().as_secs_f64();
        println!("[benchmark] run={} {:.6}s", run + 1, elapsed);
        if makepad_ai_common::backend::prof::enabled() {
            print!(
                "{}",
                makepad_ai_common::backend::prof::report_and_reset(&format!(
                    "[benchmark-prof run={}] ",
                    run + 1
                ))
            );
        }
        timings.push(elapsed);
    }
    let native_median = median(timings);
    let official_median = metadata_median(&decode_oracle.join("metadata.json"))?;
    let speedup = official_median / native_median;
    println!(
        "[benchmark] native_median={native_median:.6}s official_torch_median={official_median:.6}s speedup={speedup:.3}x",
    );
    if native_median > official_median {
        let error = format!(
            "native decoder is {:.3}x slower than official Torch oracle",
            native_median / official_median,
        );
        println!("[benchmark-warning] {error}");
        parity_failures.push(error);
    } else {
        println!("[benchmark] PASS native is equal or faster than Torch oracle");
    }

    if let Some(generation_oracle) = generation_oracle {
        let generation_oracle = Path::new(&generation_oracle);
        let token_npy = load_npy(&generation_oracle.join("fsq_tokens.npy"))?;
        if token_npy.shape != [136] {
            return Err(format!("generation FSQ shape {:?}, expected [136]", token_npy.shape));
        }
        let token_ids = token_npy.i64()?;
        let mut generated_indices = Vec::with_capacity(token_ids.len());
        for (position, token) in token_ids.into_iter().enumerate() {
            let index = if token == 33_035 {
                if position + 1 != 136 {
                    return Err(format!("unexpected EOS skin token at {position}"));
                }
                // Released generation has an upstream off-by-one: the final
                // required FSQ is EOS and official FSQ modulo-wraps it to 0.
                // This mapping exists only to validate official compatibility;
                // the production decoder rejects 32768 and strict generation
                // emits all 136 real symbols before EOS.
                0usize
            } else {
                let shifted = token - 267;
                if !(0..32_768).contains(&shifted) {
                    return Err(format!("invalid generated skin token {token} at {position}"));
                }
                shifted as usize
            };
            generated_indices.push(index);
        }
        let full_started = Instant::now();
        let native = decode_skin_tokens_weights(
            &weights,
            &generated_indices,
            &condition,
            &condition_latents_device,
            None,
        )
        .map_err(|error| error.to_string())?;
        let full_seconds = full_started.elapsed().as_secs_f64();
        let oracle = load_f32(
            &generation_oracle.join("decoded_skin.npy"),
            &[SKIN_TOKENS_SAMPLE_COUNT, 34],
        )?;
        let metrics = compare("generated_skin_34j", &native, &oracle)?;
        println!(
            "[full-j] joints=34 seconds={full_seconds:.6} per_joint={:.6}",
            full_seconds / 34.0,
        );
        require_gate("generated_skin_34j", metrics, 0.999, 6.25e-2)?;
        println!("[full-j] PASS parity gate");
        if let (Some(mesh_path), Some(transfer_oracle)) = (&task_mesh, &transfer_oracle) {
            let input = std::fs::read(mesh_path)
                .map_err(|error| format!("full-J task mesh {mesh_path}: {error}"))?;
            let mesh = SkinTokensMesh::from_glb(&input).map_err(|error| error.to_string())?;
            let samples = mesh.sample(424_242).map_err(|error| error.to_string())?;
            let transfer_started = Instant::now();
            let transferred = mesh
                .transfer_sample_weights(&samples, &native, 34)
                .map_err(|error| error.to_string())?;
            println!(
                "[task-transfer] vertices={} seconds={:.6}",
                mesh.positions.len(),
                transfer_started.elapsed().as_secs_f64(),
            );
            compare_transferred_top4(
                &transferred,
                mesh.positions.len(),
                34,
                Path::new(transfer_oracle),
            )?;
        }
    }
    if !parity_failures.is_empty() {
        return Err(format!("{} staged parity gate(s) failed", parity_failures.len()));
    }
    Ok(())
}
