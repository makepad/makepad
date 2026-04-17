use makepad_mlx::{fnv1a64_u32_words, gemma4_qproj_case_input_bf16_words, MlxSafetensorsHeader};
use std::error::Error;
use std::path::PathBuf;

const Q_WEIGHT_NAME: &str = "language_model.model.layers.0.self_attn.q_proj.weight";
const Q_SCALES_NAME: &str = "language_model.model.layers.0.self_attn.q_proj.scales";
const Q_BIASES_NAME: &str = "language_model.model.layers.0.self_attn.q_proj.biases";
const RMS_WEIGHT_NAME: &str = "language_model.model.layers.0.input_layernorm.weight";
const DIRECT_EXPECTED_HASH: u64 = 0xDA70_9B59_F4F7_0892;
const COMPOSED_EXPECTED_HASH: u64 = 0xA2FC_CDC3_E3B9_9A86;

#[derive(Clone, Copy, Debug)]
enum DeqMode {
    FloatAffine,
    Bf16MulAdd,
    LoaderLike,
}

fn bf16_word_to_f32(word: u16) -> f32 {
    f32::from_bits((word as u32) << 16)
}

fn bf16_round_to_f32(value: f32) -> f32 {
    let bits = value.to_bits();
    let lsb = (bits >> 16) & 1;
    let rounded = bits.wrapping_add(0x7FFF + lsb) & 0xFFFF0000;
    f32::from_bits(rounded)
}

fn default_model_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../local/models/gemma-4-26b-mlx/model-00001-of-00003.safetensors")
}

fn qmm_variant(
    x: &[f32],
    weights: &[u32],
    scales: &[u16],
    biases: &[u16],
    rows: usize,
    weight_words_per_row: usize,
    qparams_per_row: usize,
    deq_mode: DeqMode,
    round_prod: bool,
    round_sum: bool,
) -> Vec<f32> {
    let mut out = Vec::with_capacity(rows);
    for row in 0..rows {
        let weight_base = row * weight_words_per_row;
        let qparam_base = row * qparams_per_row;
        let mut sum = 0.0f32;
        for i in 0..x.len() {
            let group_idx = i >> 6;
            let word_idx = i >> 3;
            let shift = (i & 7) << 2;
            let q = (weights[weight_base + word_idx] >> shift) & 0xF;
            let scale = bf16_word_to_f32(scales[qparam_base + group_idx]);
            let bias = bf16_word_to_f32(biases[qparam_base + group_idx]);

            let deq = match deq_mode {
                DeqMode::FloatAffine => q as f32 * scale + bias,
                DeqMode::Bf16MulAdd => {
                    let deq_mul = bf16_round_to_f32(q as f32 * scale);
                    bf16_round_to_f32(deq_mul + bias)
                }
                DeqMode::LoaderLike => {
                    if (i & 1) == 0 {
                        bf16_round_to_f32(scale * bf16_round_to_f32(q as f32) + bias)
                    } else {
                        let scale_hi = bf16_round_to_f32(scale * 0.0625f32);
                        let q_hi = bf16_round_to_f32((q << 4) as f32);
                        bf16_round_to_f32(scale_hi * q_hi + bias)
                    }
                }
            };

            let mut prod = x[i] * deq;
            if round_prod {
                prod = bf16_round_to_f32(prod);
            }
            sum += prod;
            if round_sum {
                sum = bf16_round_to_f32(sum);
            }
        }
        out.push(bf16_round_to_f32(sum));
    }
    out
}

fn main() -> Result<(), Box<dyn Error>> {
    let header = MlxSafetensorsHeader::load(default_model_path())?;
    let q_weight_entry = header
        .tensor(Q_WEIGHT_NAME)
        .ok_or("missing q_proj weight entry")?;
    let q_scales_entry = header
        .tensor(Q_SCALES_NAME)
        .ok_or("missing q_proj scales entry")?;
    let weights = header.read_u32_tensor_words(Q_WEIGHT_NAME)?;
    let scales = header.read_bf16_tensor_words(Q_SCALES_NAME)?;
    let biases = header.read_bf16_tensor_words(Q_BIASES_NAME)?;

    let x_direct = gemma4_qproj_case_input_bf16_words(2816)
        .into_iter()
        .map(bf16_word_to_f32)
        .collect::<Vec<_>>();
    let x_composed = header.rms_norm_weighted_f32(
        &gemma4_qproj_case_input_bf16_words(2816),
        RMS_WEIGHT_NAME,
        1e-6,
    )?;

    for deq_mode in [
        DeqMode::FloatAffine,
        DeqMode::Bf16MulAdd,
        DeqMode::LoaderLike,
    ] {
        for round_prod in [false, true] {
            for round_sum in [false, true] {
                let direct = qmm_variant(
                    &x_direct,
                    &weights,
                    &scales,
                    &biases,
                    q_weight_entry.shape[0] as usize,
                    q_weight_entry.shape[1] as usize,
                    q_scales_entry.shape[1] as usize,
                    deq_mode,
                    round_prod,
                    round_sum,
                );
                let composed = qmm_variant(
                    &x_composed,
                    &weights,
                    &scales,
                    &biases,
                    q_weight_entry.shape[0] as usize,
                    q_weight_entry.shape[1] as usize,
                    q_scales_entry.shape[1] as usize,
                    deq_mode,
                    round_prod,
                    round_sum,
                );

                let direct_hash =
                    fnv1a64_u32_words(&direct.iter().map(|v| v.to_bits()).collect::<Vec<_>>());
                let composed_hash =
                    fnv1a64_u32_words(&composed.iter().map(|v| v.to_bits()).collect::<Vec<_>>());
                println!(
                    "deq={deq_mode:?} round_prod={round_prod} round_sum={round_sum} direct=0x{direct_hash:016X} composed=0x{composed_hash:016X} direct_ok={} composed_ok={}",
                    direct_hash == DIRECT_EXPECTED_HASH,
                    composed_hash == COMPOSED_EXPECTED_HASH
                );
            }
        }
    }

    Ok(())
}
