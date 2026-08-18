use makepad_diffusion::backend::{
    gpu_attention_gqa_decode_bf16, gpu_beam_cache_reorder_append, gpu_download, gpu_upload,
};

fn bf16(value: f32) -> f32 {
    let bits = value.to_bits();
    f32::from_bits(bits.wrapping_add(0x7fff + ((bits >> 16) & 1)) & 0xffff_0000)
}

fn compare(name: &str, native: &[f32], reference: &[f32], tolerance: f32) -> Result<(), String> {
    let mut max = 0.0f32;
    let mut mean = 0.0f64;
    for (&a, &b) in native.iter().zip(reference) {
        max = max.max((a - b).abs());
        mean += (a - b).abs() as f64;
    }
    mean /= native.len().max(1) as f64;
    println!("[{name}] max_abs={max:.9e} mean_abs={mean:.9e}");
    if max > tolerance {
        Err(format!("{name} max {max} exceeds {tolerance}"))
    } else {
        Ok(())
    }
}

fn main() -> Result<(), String> {
    let prior_beams = 2;
    let sequence = 3;
    let cols = 4;
    let prior: Vec<f32> = (0..prior_beams * sequence * cols)
        .map(|value| value as f32)
        .collect();
    let step = vec![100.0f32, 101.0, 102.0, 103.0, 200.0, 201.0, 202.0, 203.0];
    let parents = [1u32, 0];
    let prior_gpu = gpu_upload(&prior, prior_beams * sequence, cols)?;
    let step_gpu = gpu_upload(&step, parents.len(), cols)?;
    let output = gpu_beam_cache_reorder_append(
        &prior_gpu,
        &step_gpu,
        &parents,
        prior_beams,
        sequence,
    )?;
    let output = gpu_download(&output)?;
    let mut reference = Vec::new();
    reference.extend_from_slice(&prior[sequence * cols..2 * sequence * cols]);
    reference.extend_from_slice(&step[..cols]);
    reference.extend_from_slice(&prior[..sequence * cols]);
    reference.extend_from_slice(&step[cols..]);
    compare("beam_cache_reorder_append", &output, &reference, 0.0)?;

    let beams = 2;
    let query_heads = 4;
    let kv_heads = 2;
    let head_dim = 8;
    let sequence = 7;
    let q_width = query_heads * head_dim;
    let kv_width = kv_heads * head_dim;
    let q: Vec<f32> = (0..beams * q_width)
        .map(|i| bf16(((i * 17 % 29) as f32 - 14.0) / 16.0))
        .collect();
    let k: Vec<f32> = (0..beams * sequence * kv_width)
        .map(|i| bf16(((i * 13 % 31) as f32 - 15.0) / 16.0))
        .collect();
    let v: Vec<f32> = (0..beams * sequence * kv_width)
        .map(|i| bf16(((i * 11 % 37) as f32 - 18.0) / 16.0))
        .collect();
    let scale = 1.0 / (head_dim as f32).sqrt();
    let mut reference = vec![0.0f32; beams * q_width];
    for beam in 0..beams {
        for head in 0..query_heads {
            let kv_head = head / (query_heads / kv_heads);
            let mut scores = vec![0.0f32; sequence];
            for position in 0..sequence {
                for dim in 0..head_dim {
                    scores[position] += q[beam * q_width + head * head_dim + dim]
                        * k[(beam * sequence + position) * kv_width + kv_head * head_dim + dim];
                }
                scores[position] *= scale;
            }
            let maximum = scores.iter().copied().fold(f32::NEG_INFINITY, f32::max);
            let denominator: f32 = scores.iter().map(|score| (*score - maximum).exp()).sum();
            for position in 0..sequence {
                let probability = bf16((scores[position] - maximum).exp() / denominator);
                for dim in 0..head_dim {
                    reference[beam * q_width + head * head_dim + dim] += probability
                        * v[(beam * sequence + position) * kv_width + kv_head * head_dim + dim];
                }
            }
        }
    }
    let q_gpu = gpu_upload(&q, beams, q_width)?;
    let k_gpu = gpu_upload(&k, beams * sequence, kv_width)?;
    let v_gpu = gpu_upload(&v, beams * sequence, kv_width)?;
    let output = gpu_attention_gqa_decode_bf16(
        &q_gpu,
        &k_gpu,
        &v_gpu,
        query_heads,
        kv_heads,
        scale,
    )?;
    compare("beam_gqa_decode", &gpu_download(&output)?, &reference, 2.0e-5)?;
    println!("[beam-ops] PASS");
    Ok(())
}
