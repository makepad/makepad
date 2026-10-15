//! Deterministic weight encoders for the block layouts in `quant`.
//! Scale searches and their MIT attribution are documented in `libs/ai/NOTICE`.
//! This offline path does not change the runtime activation quantizers.

use crate::quant::{block_elements, block_size, f16_to_f32, f32_to_f16_rn, TensorType};

#[inline]
fn nearest(x: f32) -> i32 {
    x.round_ties_even() as i32
}

fn half(out: &mut [u8], x: f32) -> f32 {
    let h = f32_to_f16_rn(x);
    out[..2].copy_from_slice(&h.to_le_bytes());
    f16_to_f32(h)
}

/// Encodes whole blocks in row order. Callers must not join partial rows.
/// Non-finite inputs and values outside the block scale range are errors.
pub fn encode(ty: TensorType, input: &[f32], out: &mut [u8]) -> Result<(), String> {
    let kind = ty.ggml_type();
    let elements = block_elements(kind);
    let bytes = block_size(kind);
    if !matches!(
        ty,
        TensorType::F32
            | TensorType::F16
            | TensorType::Q8_0
            | TensorType::Q4K
            | TensorType::Q5K
            | TensorType::Q6K
    ) {
        return Err(format!("unsupported output type {ty:?}"));
    }
    if input.len() % elements != 0 || out.len() != input.len() / elements * bytes {
        return Err("input/output block lengths disagree".into());
    }
    if input.iter().any(|x| !x.is_finite()) {
        return Err("non-finite source weight".into());
    }
    for (x, b) in input
        .chunks_exact(elements)
        .zip(out.chunks_exact_mut(bytes))
    {
        match ty {
            TensorType::F32 => b.copy_from_slice(&x[0].to_le_bytes()),
            TensorType::F16 => {
                half(b, x[0]);
            }
            TensorType::Q8_0 => quantize_q8_0(x, b),
            TensorType::Q4K => quantize_affine(x, b, 15),
            TensorType::Q5K => quantize_affine(x, b, 31),
            TensorType::Q6K => quantize_q6_k(x, b),
            _ => unreachable!(),
        }
        let scale_offsets: &[usize] = match ty {
            TensorType::F32 => &[],
            TensorType::Q4K | TensorType::Q5K => &[0, 2],
            TensorType::Q6K => &[208],
            _ => &[0],
        };
        if scale_offsets
            .iter()
            .any(|&at| !f16_to_f32(u16::from_le_bytes([b[at], b[at + 1]])).is_finite())
        {
            return Err("weight exceeds finite f16 scale range".into());
        }
    }
    Ok(())
}

fn quantize_q8_0(x: &[f32], out: &mut [u8]) {
    let amax = x.iter().fold(0.0f32, |a, x| a.max(x.abs()));
    let d = amax / 127.0;
    let inv = if d != 0.0 { 1.0 / d } else { 0.0 };
    half(out, d);
    for (q, &x) in out[2..].iter_mut().zip(x) {
        *q = (x * inv).round().clamp(-127.0, 127.0) as i8 as u8;
    }
}

// Weighted affine least squares over candidate code assignments. The search
// grid is part of the quantization scheme, not a limit on the input size.
fn affine_group(x: &[f32], codes: &mut [u8], nmax: i32) -> (f32, f32) {
    let rms = (x.iter().map(|x| x * x).sum::<f32>() / 32.0).sqrt();
    let mut weights = [0.0; 32];
    let mut min = x[0];
    let mut max = x[0];
    let mut sw = 0.0;
    let mut sx = 0.0;
    for i in 0..32 {
        min = min.min(x[i]);
        max = max.max(x[i]);
        weights[i] = rms + x[i].abs();
        sw += weights[i];
        sx += weights[i] * x[i];
    }
    min = min.min(0.0);
    if max == min {
        codes.fill(0);
        return (0.0, -min);
    }
    let inv = nmax as f32 / (max - min);
    let mut scale = 1.0 / inv;
    let mut best_error = 0.0;
    for i in 0..32 {
        codes[i] = nearest(inv * (x[i] - min)).clamp(0, nmax) as u8;
        let diff = scale * codes[i] as f32 + min - x[i];
        best_error += weights[i] * (diff * diff);
    }
    let (start, steps) = if nmax == 15 { (-1.0, 20) } else { (-0.5, 15) };
    for step in 0..=steps {
        let inv = (start + 0.1 * step as f32 + nmax as f32) / (max - min);
        let mut aux = [0u8; 32];
        let (mut sl, mut sl2, mut sxl) = (0.0, 0.0, 0.0);
        for i in 0..32 {
            let l = nearest(inv * (x[i] - min)).clamp(0, nmax);
            aux[i] = l as u8;
            let l = l as f32;
            sl += weights[i] * l;
            sl2 += weights[i] * l * l;
            sxl += weights[i] * l * x[i];
        }
        let det = sw * sl2 - sl * sl;
        if det <= 0.0 {
            continue;
        }
        let mut candidate_scale = (sw * sxl - sx * sl) / det;
        let mut candidate_min = (sl2 * sx - sl * sxl) / det;
        if candidate_min > 0.0 {
            candidate_min = 0.0;
            candidate_scale = sxl / sl2;
        }
        let mut error = 0.0;
        for i in 0..32 {
            let diff = candidate_scale * aux[i] as f32 + candidate_min - x[i];
            error += weights[i] * (diff * diff);
        }
        if error < best_error {
            codes.copy_from_slice(&aux);
            best_error = error;
            scale = candidate_scale;
            min = candidate_min;
        }
    }
    (scale, -min)
}

// The first fit uses real-valued group scales. Rounding those to six-bit
// integers changes the optimum. Descend over neighbouring representable
// scale/min pairs using the actual decoded weighted error. Keep the same
// weighting as the first fit so scale rounding does not discard its objective.
// Strict improvement and the
// finite code space give a deterministic termination condition.
fn refine_affine_group(
    x: &[f32],
    codes: &mut [u8],
    d: f32,
    dm: f32,
    mut sc: i32,
    mut m: i32,
    nmax: i32,
) -> (u8, u8) {
    let rms = (x.iter().map(|v| v * v).sum::<f32>() / 32.0).sqrt();
    let weights: [f32; 32] = std::array::from_fn(|i| rms + x[i].abs());
    let evaluate = |sc: i32, m: i32, q: &mut [u8]| {
        let scale = d * sc as f32;
        let min = dm * m as f32;
        let mut error = 0.0f32;
        for i in 0..32 {
            q[i] = if scale != 0.0 {
                nearest((x[i] + min) / scale).clamp(0, nmax) as u8
            } else {
                0
            };
            let diff = scale * q[i] as f32 - min - x[i];
            error += weights[i] * (diff * diff);
        }
        error
    };
    let mut best = evaluate(sc, m, codes);
    loop {
        let (mut best_sc, mut best_m) = (sc, m);
        for ds in -1..=1 {
            for delta_min in -1..=1 {
                let (s, minimum) = (sc + ds, m + delta_min);
                if !(0..=63).contains(&s)
                    || !(0..=63).contains(&minimum)
                    || (ds == 0 && delta_min == 0)
                {
                    continue;
                }
                let mut candidate = [0u8; 32];
                let error = evaluate(s, minimum, &mut candidate);
                if error < best {
                    best = error;
                    best_sc = s;
                    best_m = minimum;
                    codes.copy_from_slice(&candidate);
                }
            }
        }
        if (best_sc, best_m) == (sc, m) {
            return (sc as u8, m as u8);
        }
        (sc, m) = (best_sc, best_m);
    }
}

fn quantize_affine(x: &[f32], out: &mut [u8], nmax: i32) {
    out.fill(0);
    let mut codes = [0u8; 256];
    let mut scales = [0.0f32; 8];
    let mut mins = [0.0f32; 8];
    let (mut max_scale, mut max_min) = (0.0f32, 0.0f32);
    for j in 0..8 {
        (scales[j], mins[j]) = affine_group(
            &x[j * 32..j * 32 + 32],
            &mut codes[j * 32..j * 32 + 32],
            nmax,
        );
        max_scale = max_scale.max(scales[j]);
        max_min = max_min.max(mins[j]);
    }
    let inv_scale = if max_scale > 0.0 {
        63.0 / max_scale
    } else {
        0.0
    };
    let inv_min = if max_min > 0.0 { 63.0 / max_min } else { 0.0 };
    let d = half(&mut out[0..], max_scale / 63.0);
    let dm = half(&mut out[2..], max_min / 63.0);
    for j in 0..8 {
        let sc = nearest(inv_scale * scales[j]).clamp(0, 63);
        let m = nearest(inv_min * mins[j]).clamp(0, 63);
        let (sc, m) = refine_affine_group(
            &x[j * 32..j * 32 + 32],
            &mut codes[j * 32..j * 32 + 32],
            d,
            dm,
            sc,
            m,
            nmax,
        );
        if j < 4 {
            out[4 + j] = sc;
            out[8 + j] = m;
        } else {
            out[8 + j] = (sc & 15) | ((m & 15) << 4);
            out[j] |= (sc >> 4) << 6;
            out[4 + j] |= (m >> 4) << 6;
        }
    }
    let lo = if nmax == 31 { 48 } else { 16 };
    for g in 0..4 {
        for i in 0..32 {
            let a = codes[g * 64 + i];
            let b = codes[g * 64 + 32 + i];
            out[lo + g * 32 + i] = (a & 15) | ((b & 15) << 4);
            if nmax == 31 {
                out[16 + i] |= ((a >> 4) << (2 * g)) | ((b >> 4) << (2 * g + 1));
            }
        }
    }
}

fn signed_group(x: &[f32], codes: &mut [u8]) -> f32 {
    let mut max = 0.0f32;
    for &v in x {
        if v.abs() > max.abs() {
            max = v;
        }
    }
    if max.abs() < 1e-15 {
        codes.fill(0);
        return 0.0;
    }
    let sums = |inv: f32| {
        let (mut sx, mut sl) = (0.0f32, 0.0f32);
        for &v in x {
            let l = nearest(inv * v).clamp(-32, 31) as f32;
            let w = v * v;
            sx += w * v * l;
            sl += w * l * l;
        }
        (sx, sl)
    };
    let mut inv = -32.0 / max;
    let (sx, sl) = sums(inv);
    let mut scale = if sl != 0.0 { sx / sl } else { 0.0 };
    let mut best = scale * sx;
    for step in -9..=9 {
        if step == 0 {
            continue;
        }
        let candidate = -(32.0 + 0.1 * step as f32) / max;
        let (sx, sl) = sums(candidate);
        if sl > 0.0 && sx * sx > best * sl {
            inv = candidate;
            scale = sx / sl;
            best = scale * sx;
        }
    }
    for (code, &v) in codes.iter_mut().zip(x) {
        *code = (nearest(inv * v).clamp(-32, 31) + 32) as u8;
    }
    scale
}

fn quantize_q6_k(x: &[f32], out: &mut [u8]) {
    out.fill(0);
    let mut codes = [0u8; 256];
    let mut scales = [0.0f32; 16];
    let mut max_scale = 0.0f32;
    for j in 0..16 {
        scales[j] = signed_group(&x[j * 16..j * 16 + 16], &mut codes[j * 16..j * 16 + 16]);
        if scales[j].abs() > max_scale.abs() {
            max_scale = scales[j];
        }
    }
    if max_scale.abs() < 1e-15 {
        return;
    }
    let inv = -128.0 / max_scale;
    let d = half(&mut out[208..], 1.0 / inv);
    for j in 0..16 {
        let sc = nearest(inv * scales[j]).clamp(-128, 127) as i8;
        out[192 + j] = sc as u8;
        let ds = d * sc as f32;
        if ds != 0.0 {
            for i in 0..16 {
                codes[j * 16 + i] = (nearest(x[j * 16 + i] / ds).clamp(-32, 31) + 32) as u8;
            }
        }
    }
    for n in 0..2 {
        for i in 0..32 {
            let a = codes[n * 128 + i];
            let b = codes[n * 128 + 32 + i];
            let c = codes[n * 128 + 64 + i];
            let e = codes[n * 128 + 96 + i];
            out[n * 64 + i] = (a & 15) | ((c & 15) << 4);
            out[n * 64 + 32 + i] = (b & 15) | ((e & 15) << 4);
            out[128 + n * 32 + i] = (a >> 4) | ((b >> 4) << 2) | ((c >> 4) << 4) | ((e >> 4) << 6);
        }
    }
}
