use makepad_ggml::backend::cuda::CudaRuntime;

const QK_Q8_1: usize = 32;
const Q8_1_BLOCK_BYTES: usize = 36;
const Q8_1_MMQ_BLOCK_BYTES: usize = 144;
const QK_NVFP4: usize = 64;
const NVFP4_BLOCK_BYTES: usize = 36;
const QK_NVFP4_SUB: usize = 16;
const NVFP4_VALUES: [f32; 16] = [
    0.0, 1.0, 2.0, 3.0, 4.0, 6.0, 8.0, 12.0, 0.0, -1.0, -2.0, -3.0, -4.0, -6.0, -8.0, -12.0,
];

fn f32s_to_bytes(values: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(values.len() * std::mem::size_of::<f32>());
    for &value in values {
        bytes.extend_from_slice(&value.to_ne_bytes());
    }
    bytes
}

fn ue4m3_to_fp32(x: u8) -> f32 {
    if x == 0 || x == 0x7F || x == 0xFF {
        return 0.0;
    }
    let exp = ((x >> 3) & 0xF) as i32;
    let man = (x & 0x7) as i32;
    let raw = if exp == 0 {
        (man as f32) * 2f32.powi(-9)
    } else {
        (1.0 + man as f32 / 8.0) * 2f32.powi(exp - 7)
    };
    raw * 0.5
}

fn f16_to_f32(bits: u16) -> f32 {
    let sign = ((bits >> 15) & 0x1) as u32;
    let exp = ((bits >> 10) & 0x1f) as u32;
    let frac = (bits & 0x03ff) as u32;
    let out_bits = if exp == 0 {
        if frac == 0 {
            sign << 31
        } else {
            let mut frac_norm = frac;
            let mut exp_shift = -1i32;
            while (frac_norm & 0x0400) == 0 {
                frac_norm <<= 1;
                exp_shift += 1;
            }
            frac_norm &= 0x03ff;
            (sign << 31) | (((127 - 15 - exp_shift) as u32) << 23) | (frac_norm << 13)
        }
    } else if exp == 0x1f {
        (sign << 31) | 0x7f800000 | (frac << 13)
    } else {
        (sign << 31) | ((exp + 112) << 23) | (frac << 13)
    };
    f32::from_bits(out_bits)
}

fn dequantize_q8_1_mmq_row(
    bytes: &[u8],
    hidden_size: usize,
    row_index: usize,
    row_count: usize,
) -> Vec<f32> {
    let block_groups = hidden_size / (4 * QK_Q8_1);
    let mut out = vec![0.0f32; hidden_size];
    for block_group in 0..block_groups {
        let block_offset = (block_group * row_count + row_index) * Q8_1_MMQ_BLOCK_BYTES;
        let block = &bytes[block_offset..block_offset + Q8_1_MMQ_BLOCK_BYTES];
        let mut d4 = [0.0f32; 4];
        for (sub, dst) in d4.iter_mut().enumerate() {
            let start = sub * 4;
            *dst = f32::from_ne_bytes(block[start..start + 4].try_into().unwrap());
        }
        let qs = &block[16..];
        for sub in 0..4 {
            for lane in 0..QK_Q8_1 {
                let q = qs[sub * QK_Q8_1 + lane] as i8;
                out[block_group * 4 * QK_Q8_1 + sub * QK_Q8_1 + lane] = d4[sub] * q as f32;
            }
        }
    }
    out
}

fn dequantize_q8_1_row(
    bytes: &[u8],
    hidden_size: usize,
    row_index: usize,
    row_count: usize,
) -> Vec<f32> {
    let blocks_per_row = hidden_size / QK_Q8_1;
    let mut out = vec![0.0f32; hidden_size];
    for block_idx in 0..blocks_per_row {
        let block_offset = (row_index * blocks_per_row + block_idx) * Q8_1_BLOCK_BYTES;
        let block = &bytes[block_offset..block_offset + Q8_1_BLOCK_BYTES];
        let d = f16_to_f32(u16::from_ne_bytes(block[0..2].try_into().unwrap()));
        let qs = &block[4..];
        for lane in 0..QK_Q8_1 {
            out[block_idx * QK_Q8_1 + lane] = d * (qs[lane] as i8) as f32;
        }
    }
    let _ = row_count;
    out
}

fn dequantize_nvfp4_row(bytes: &[u8], hidden_size: usize, row_index: usize) -> Vec<f32> {
    let blocks_per_row = hidden_size / QK_NVFP4;
    let mut out = vec![0.0f32; hidden_size];
    for block_idx in 0..blocks_per_row {
        let block_offset = (row_index * blocks_per_row + block_idx) * NVFP4_BLOCK_BYTES;
        let block = &bytes[block_offset..block_offset + NVFP4_BLOCK_BYTES];
        let d = &block[..QK_NVFP4 / QK_NVFP4_SUB];
        let qs = &block[QK_NVFP4 / QK_NVFP4_SUB..];
        for sub in 0..(QK_NVFP4 / QK_NVFP4_SUB) {
            let scale = ue4m3_to_fp32(d[sub]);
            for byte_idx in 0..(QK_NVFP4_SUB / 2) {
                let packed = qs[sub * (QK_NVFP4_SUB / 2) + byte_idx];
                let lo = (packed & 0x0F) as usize;
                let hi = (packed >> 4) as usize;
                let base = block_idx * QK_NVFP4 + sub * QK_NVFP4_SUB + byte_idx;
                out[base] = scale * NVFP4_VALUES[lo];
                out[base + QK_NVFP4_SUB / 2] = scale * NVFP4_VALUES[hi];
            }
        }
    }
    out
}

fn cpu_mmq_reference(
    q8_mmq_bytes: &[u8],
    weights_nvfp4_bytes: &[u8],
    hidden_size: usize,
    out_rows: usize,
    input_rows: usize,
) -> Vec<f32> {
    let mut out = vec![0.0f32; input_rows * out_rows];
    let dequant_inputs: Vec<Vec<f32>> = (0..input_rows)
        .map(|row| dequantize_q8_1_mmq_row(q8_mmq_bytes, hidden_size, row, input_rows))
        .collect();
    let dequant_weights: Vec<Vec<f32>> = (0..out_rows)
        .map(|row| dequantize_nvfp4_row(weights_nvfp4_bytes, hidden_size, row))
        .collect();

    for row in 0..input_rows {
        for col in 0..out_rows {
            let mut sum = 0.0f32;
            for k in 0..hidden_size {
                sum += dequant_inputs[row][k] * dequant_weights[col][k];
            }
            out[row * out_rows + col] = sum;
        }
    }
    out
}

fn cpu_q8_reference(
    q8_bytes: &[u8],
    weights_nvfp4_bytes: &[u8],
    hidden_size: usize,
    out_rows: usize,
    input_rows: usize,
) -> Vec<f32> {
    let mut out = vec![0.0f32; input_rows * out_rows];
    let dequant_inputs: Vec<Vec<f32>> = (0..input_rows)
        .map(|row| dequantize_q8_1_row(q8_bytes, hidden_size, row, input_rows))
        .collect();
    let dequant_weights: Vec<Vec<f32>> = (0..out_rows)
        .map(|row| dequantize_nvfp4_row(weights_nvfp4_bytes, hidden_size, row))
        .collect();

    for row in 0..input_rows {
        for col in 0..out_rows {
            let mut sum = 0.0f32;
            for k in 0..hidden_size {
                sum += dequant_inputs[row][k] * dequant_weights[col][k];
            }
            out[row * out_rows + col] = sum;
        }
    }
    out
}

fn run_case(
    cuda: &CudaRuntime,
    hidden_size: usize,
    out_rows: usize,
    input_rows: usize,
) -> Result<(), String> {
    let input: Vec<f32> = (0..input_rows * hidden_size)
        .map(|i| ((i % 19) as f32 - 9.0) * 0.0625f32)
        .collect();
    let weights: Vec<f32> = (0..out_rows * hidden_size)
        .map(|i| (((i * 7) % 23) as f32 - 11.0) * 0.03125f32)
        .collect();

    let input_f32 = cuda.load_bytes(&f32s_to_bytes(&input))?;
    let weights_f32 = cuda.load_bytes(&f32s_to_bytes(&weights))?;

    let q8_bytes = input_rows
        .checked_mul(hidden_size / QK_Q8_1)
        .and_then(|blocks| blocks.checked_mul(Q8_1_BLOCK_BYTES))
        .ok_or_else(|| "q8 byte size overflow".to_string())?;
    let nvfp4_bytes = out_rows
        .checked_mul(hidden_size / QK_NVFP4)
        .and_then(|blocks| blocks.checked_mul(NVFP4_BLOCK_BYTES))
        .ok_or_else(|| "nvfp4 byte size overflow".to_string())?;
    let output_len = input_rows
        .checked_mul(out_rows)
        .ok_or_else(|| "output length overflow".to_string())?;

    let q8_ref = cuda.alloc_bytes(q8_bytes)?;
    let q8_mmq = cuda.alloc_bytes(q8_bytes)?;
    let weights_nvfp4 = cuda.alloc_bytes(nvfp4_bytes)?;
    let out_ref = cuda.alloc_f32(output_len)?;
    let out_mmq = cuda.alloc_f32(output_len)?;
    let mmq_fixup_len = cuda.nvfp4_q8_1_mmq_fixup_f32_len()?;
    let mmq_fixup = cuda.alloc_f32(mmq_fixup_len)?;

    println!("case input_rows={input_rows} quantize_weights");
    cuda.quantize_nvfp4_f32(&weights_f32, 1.0, &weights_nvfp4, out_rows * hidden_size)?;
    println!("case input_rows={input_rows} quantize_ref_input");
    cuda.quantize_q8_1_f32(&input_f32, &q8_ref, input_rows * hidden_size)?;
    println!("case input_rows={input_rows} ref_matmul");
    cuda.nvfp4_q8_1_matmul_batched(
        &q8_ref,
        &weights_nvfp4,
        &out_ref,
        hidden_size / QK_Q8_1,
        out_rows,
        input_rows,
    )?;
    println!("case input_rows={input_rows} quantize_mmq_input");
    cuda.quantize_q8_1_mmq_f32(&input_f32, &q8_mmq, hidden_size, input_rows)?;
    println!("case input_rows={input_rows} mmq_matmul");
    cuda.nvfp4_q8_1_mmq_matmul_batched(
        &q8_mmq,
        &weights_nvfp4,
        &out_mmq,
        &mmq_fixup,
        mmq_fixup_len,
        hidden_size,
        out_rows,
        input_rows,
    )?;
    println!("case input_rows={input_rows} readback");
    let ref_vals = cuda.read_f32s(&out_ref, output_len)?;
    let mmq_vals = cuda.read_f32s(&out_mmq, output_len)?;
    let q8_ref_bytes = cuda.read_bytes(&q8_ref, q8_bytes)?;
    let q8_mmq_bytes = cuda.read_bytes(&q8_mmq, q8_bytes)?;
    let weights_nvfp4_bytes = cuda.read_bytes(&weights_nvfp4, nvfp4_bytes)?;
    let cpu_ref_vals = cpu_q8_reference(
        &q8_ref_bytes,
        &weights_nvfp4_bytes,
        hidden_size,
        out_rows,
        input_rows,
    );
    let cpu_vals = cpu_mmq_reference(
        &q8_mmq_bytes,
        &weights_nvfp4_bytes,
        hidden_size,
        out_rows,
        input_rows,
    );

    let mut max_abs_diff = 0.0f32;
    let mut max_idx = 0usize;
    for (idx, (&lhs, &rhs)) in ref_vals.iter().zip(mmq_vals.iter()).enumerate() {
        let diff = (lhs - rhs).abs();
        if diff > max_abs_diff {
            max_abs_diff = diff;
            max_idx = idx;
        }
    }

    let mut max_cpu_diff = 0.0f32;
    let mut max_cpu_idx = 0usize;
    for (idx, (&lhs, &rhs)) in cpu_vals.iter().zip(mmq_vals.iter()).enumerate() {
        let diff = (lhs - rhs).abs();
        if diff > max_cpu_diff {
            max_cpu_diff = diff;
            max_cpu_idx = idx;
        }
    }

    let mut max_ref_cpu_diff = 0.0f32;
    let mut max_ref_cpu_idx = 0usize;
    for (idx, (&lhs, &rhs)) in cpu_ref_vals.iter().zip(ref_vals.iter()).enumerate() {
        let diff = (lhs - rhs).abs();
        if diff > max_ref_cpu_diff {
            max_ref_cpu_diff = diff;
            max_ref_cpu_idx = idx;
        }
    }

    let ref_q_row = dequantize_q8_1_row(&q8_ref_bytes, hidden_size, 0, input_rows);
    let mmq_q_row = dequantize_q8_1_mmq_row(&q8_mmq_bytes, hidden_size, 0, input_rows);
    let mut max_q_diff = 0.0f32;
    let mut max_q_idx = 0usize;
    for (idx, (&lhs, &rhs)) in ref_q_row.iter().zip(mmq_q_row.iter()).enumerate() {
        let diff = (lhs - rhs).abs();
        if diff > max_q_diff {
            max_q_diff = diff;
            max_q_idx = idx;
        }
    }

    println!(
        "case input_rows={input_rows} ok max_abs_diff={:.6} idx={} ref={:.6} mmq={:.6}",
        max_abs_diff, max_idx, ref_vals[max_idx], mmq_vals[max_idx]
    );
    println!(
        "case input_rows={input_rows} ref cpu max_abs_diff={:.6} idx={} cpu_ref={:.6} ref={:.6}",
        max_ref_cpu_diff, max_ref_cpu_idx, cpu_ref_vals[max_ref_cpu_idx], ref_vals[max_ref_cpu_idx]
    );
    println!(
        "case input_rows={input_rows} cpu max_abs_diff={:.6} idx={} cpu={:.6} mmq={:.6}",
        max_cpu_diff, max_cpu_idx, cpu_vals[max_cpu_idx], mmq_vals[max_cpu_idx]
    );
    println!(
        "case input_rows={input_rows} q max_abs_diff={:.6} idx={} ref_q={:.6} mmq_q={:.6}",
        max_q_diff, max_q_idx, ref_q_row[max_q_idx], mmq_q_row[max_q_idx]
    );
    if input_rows == 14 {
        for mmq_block in 0..(hidden_size / QK_Q8_1) {
            let mmq_slice = &mmq_q_row[mmq_block * QK_Q8_1..(mmq_block + 1) * QK_Q8_1];
            let mut best_ref_block = 0usize;
            let mut best_err = f32::INFINITY;
            for ref_block in 0..(hidden_size / QK_Q8_1) {
                let ref_slice = &ref_q_row[ref_block * QK_Q8_1..(ref_block + 1) * QK_Q8_1];
                let err: f32 = ref_slice
                    .iter()
                    .zip(mmq_slice.iter())
                    .map(|(&lhs, &rhs)| (lhs - rhs).abs())
                    .sum();
                if err < best_err {
                    best_err = err;
                    best_ref_block = ref_block;
                }
            }
            println!(
                "case input_rows={input_rows} q block_map mmq_block={} best_ref_block={} l1={:.6}",
                mmq_block, best_ref_block, best_err
            );
        }
    }
    Ok(())
}

fn main() -> Result<(), String> {
    let hidden_size = 256usize;
    let out_rows = 128usize;
    let cuda = CudaRuntime::load()?;
    run_case(&cuda, hidden_size, out_rows, 14)?;
    run_case(&cuda, hidden_size, out_rows, 32)?;
    Ok(())
}
