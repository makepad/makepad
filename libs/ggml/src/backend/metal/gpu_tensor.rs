//! Metal-backed GpuTensor ops for SAM3 / later Klein / RealESRGAN on macOS.
//! Heavy GEMM / flash / conv / norm / elementwise go through the existing
//! Metal try_* kernels. Small addressing ops stay on the host. No oracle;
//! goal is a working Metal path we can then keep cutting copies.

use crate::backend::cuda::{GpuLinearPart, GpuTensor};
use crate::backend::metal::{
    try_add_f32, try_conv2d_planar_f32, try_flash_attn_f32_packed, try_gelu_f32,
    try_group_norm_planar_f32, try_layer_norm_mul_add_f32, try_matmul_nt_f32, try_mul_f32,
    try_silu_f32,
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Mutex;

fn tensor(rows: usize, cols: usize, data: Vec<f32>) -> GpuTensor {
    GpuTensor {
        rows,
        cols,
        data: RefCell::new(data),
        u32s: RefCell::new(Vec::new()),
    }
}

fn data(t: &GpuTensor) -> Result<std::cell::Ref<'_, Vec<f32>>, String> {
    t.data
        .try_borrow()
        .map_err(|_| "metal GpuTensor already borrowed".to_string())
}

fn unpack_bf16(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(2)
        .map(|c| {
            let w = u16::from_le_bytes([c[0], c[1]]);
            f32::from_bits((w as u32) << 16)
        })
        .collect()
}

fn unpack_f16(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(2)
        .map(|c| {
            let w = u16::from_le_bytes([c[0], c[1]]);
            half_to_f32(w)
        })
        .collect()
}

fn half_to_f32(word: u16) -> f32 {
    let sign = ((word >> 15) & 1) as u32;
    let exp = ((word >> 10) & 0x1f) as u32;
    let frac = (word & 0x3ff) as u32;
    let bits = if exp == 0 {
        if frac == 0 {
            sign << 31
        } else {
            let mut exp32 = 127 - 15 + 1;
            let mut frac32 = frac;
            while frac32 & 0x400 == 0 {
                frac32 <<= 1;
                exp32 -= 1;
            }
            frac32 &= 0x3ff;
            (sign << 31) | (exp32 << 23) | (frac32 << 13)
        }
    } else if exp == 31 {
        (sign << 31) | (0xff << 23) | (frac << 13)
    } else {
        (sign << 31) | ((exp + 127 - 15) << 23) | (frac << 13)
    };
    f32::from_bits(bits)
}

fn weight_f32(part: &GpuLinearPart<'_>) -> Vec<f32> {
    // SAM3 packs bf16. Accept f32 / f16 too.
    if part.bytes.len() == part.n.saturating_mul(part.bytes.len() / part.n.max(1))
        && part.bytes.len() % 2 == 0
        && part.bytes.len() / 2 >= part.n
    {
        if part.bt_ggml_type == 1 {
            // GGML_TYPE_F16
            return unpack_f16(part.bytes);
        }
        return unpack_bf16(part.bytes);
    }
    part.bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

fn cache_weight(namespace: &str, parts: &[GpuLinearPart<'_>]) -> Vec<f32> {
    static CACHE: Mutex<Option<HashMap<String, Vec<f32>>>> = Mutex::new(None);
    let key = parts
        .iter()
        .map(|p| format!("{namespace}/{}:{}", p.cache_key, p.n))
        .collect::<Vec<_>>()
        .join("|");
    let mut guard = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let map = guard.get_or_insert_with(HashMap::new);
    if let Some(hit) = map.get(&key) {
        return hit.clone();
    }
    let mut packed = Vec::new();
    for part in parts {
        packed.extend(weight_f32(part));
    }
    map.insert(key, packed.clone());
    packed
}

pub fn available() -> bool {
    crate::backend::metal::is_available()
}

pub fn upload(values: &[f32], rows: usize, cols: usize) -> Result<GpuTensor, String> {
    if values.len() != rows * cols {
        return Err(format!(
            "metal upload len {} != {}x{}",
            values.len(),
            rows,
            cols
        ));
    }
    Ok(tensor(rows, cols, values.to_vec()))
}

pub fn upload_u32(values: &[u32]) -> Result<GpuTensor, String> {
    Ok(GpuTensor {
        rows: values.len().max(1),
        cols: 1,
        data: RefCell::new(Vec::new()),
        u32s: RefCell::new(values.to_vec()),
    })
}

pub fn download(t: &GpuTensor) -> Result<Vec<f32>, String> {
    Ok(data(t)?.clone())
}

pub fn upload_into(t: &GpuTensor, values: &[f32]) -> Result<(), String> {
    let mut slot = t
        .data
        .try_borrow_mut()
        .map_err(|_| "metal upload_into borrow".to_string())?;
    if values.len() != slot.len() && slot.len() != 0 {
        if values.len() != t.rows * t.cols {
            return Err("metal upload_into size mismatch".to_string());
        }
    }
    *slot = values.to_vec();
    Ok(())
}

pub fn copy_into(src: &GpuTensor, dst: &GpuTensor) -> Result<(), String> {
    let src_data = data(src)?.clone();
    let mut dst_data = dst
        .data
        .try_borrow_mut()
        .map_err(|_| "metal copy_into borrow".to_string())?;
    *dst_data = src_data;
    Ok(())
}

pub fn to_f32(t: &GpuTensor) -> Result<GpuTensor, String> {
    Ok(tensor(t.rows, t.cols, data(t)?.clone()))
}

pub fn add(a: &GpuTensor, b: &GpuTensor) -> Result<GpuTensor, String> {
    let ad = data(a)?;
    let bd = data(b)?;
    let out = try_add_f32(&ad, &[a.rows, a.cols], &bd, &[b.rows, b.cols])
        .unwrap_or_else(|| {
            ad.iter()
                .zip(bd.iter())
                .map(|(x, y)| x + y)
                .collect::<Vec<_>>()
        });
    Ok(tensor(a.rows, a.cols, out))
}

pub fn mul(a: &GpuTensor, b: &GpuTensor) -> Result<GpuTensor, String> {
    let ad = data(a)?;
    let bd = data(b)?;
    let out = try_mul_f32(&ad, &[a.rows, a.cols], &bd, &[b.rows, b.cols]).unwrap_or_else(|| {
        ad.iter()
            .zip(bd.iter())
            .map(|(x, y)| x * y)
            .collect::<Vec<_>>()
    });
    Ok(tensor(a.rows, a.cols, out))
}

pub fn silu(x: &GpuTensor) -> Result<GpuTensor, String> {
    let xd = data(x)?;
    let out = try_silu_f32(&xd).unwrap_or_else(|| {
        xd.iter()
            .map(|&v| v / (1.0 + (-v).exp()))
            .collect::<Vec<_>>()
    });
    Ok(tensor(x.rows, x.cols, out))
}

pub fn gelu(x: &GpuTensor) -> Result<GpuTensor, String> {
    let xd = data(x)?;
    let out = try_gelu_f32(&xd, &[x.rows, x.cols]).unwrap_or_else(|| {
        xd.iter()
            .map(|&v| 0.5 * v * (1.0 + (0.79788456 * (v + 0.044715 * v * v * v)).tanh()))
            .collect()
    });
    Ok(tensor(x.rows, x.cols, out))
}

pub fn gelu_erf(x: &GpuTensor) -> Result<GpuTensor, String> {
    // SAM3 GELU-erf; Metal gelu is tanh approx — close enough for no-oracle.
    gelu(x)
}

pub fn relu(x: &GpuTensor) -> Result<GpuTensor, String> {
    let out = data(x)?.iter().map(|&v| v.max(0.0)).collect();
    Ok(tensor(x.rows, x.cols, out))
}

pub fn slice_rows(x: &GpuTensor, start: usize, count: usize) -> Result<GpuTensor, String> {
    let xd = data(x)?;
    let end = start.saturating_add(count);
    if end > x.rows {
        return Err("metal slice_rows oob".into());
    }
    let off = start * x.cols;
    Ok(tensor(count, x.cols, xd[off..off + count * x.cols].to_vec()))
}

pub fn slice_cols(x: &GpuTensor, start: usize, count: usize) -> Result<GpuTensor, String> {
    let xd = data(x)?;
    if start + count > x.cols {
        return Err("metal slice_cols oob".into());
    }
    let mut out = vec![0.0; x.rows * count];
    for r in 0..x.rows {
        let s = r * x.cols + start;
        out[r * count..(r + 1) * count].copy_from_slice(&xd[s..s + count]);
    }
    Ok(tensor(x.rows, count, out))
}

pub fn concat_rows(a: &GpuTensor, b: &GpuTensor) -> Result<GpuTensor, String> {
    if a.cols != b.cols {
        return Err("metal concat_rows col mismatch".into());
    }
    let mut out = data(a)?.clone();
    out.extend_from_slice(&data(b)?);
    Ok(tensor(a.rows + b.rows, a.cols, out))
}

pub fn concat_cols(parts: &[&GpuTensor]) -> Result<GpuTensor, String> {
    if parts.is_empty() {
        return Err("metal concat_cols empty".into());
    }
    let rows = parts[0].rows;
    let cols: usize = parts.iter().map(|p| p.cols).sum();
    let mut out = vec![0.0; rows * cols];
    for r in 0..rows {
        let mut c0 = 0;
        for p in parts {
            let pd = data(p)?;
            let src = &pd[r * p.cols..(r + 1) * p.cols];
            out[r * cols + c0..r * cols + c0 + p.cols].copy_from_slice(src);
            c0 += p.cols;
        }
    }
    Ok(tensor(rows, cols, out))
}

pub fn reshape(x: &GpuTensor, rows: usize, cols: usize) -> Result<GpuTensor, String> {
    if rows * cols != x.rows * x.cols {
        return Err("metal reshape size".into());
    }
    Ok(tensor(rows, cols, data(x)?.clone()))
}

pub fn gather_cols(x: &GpuTensor, indices: &[u32]) -> Result<GpuTensor, String> {
    let xd = data(x)?;
    let mut out = vec![0.0; x.rows * indices.len()];
    for r in 0..x.rows {
        for (j, &idx) in indices.iter().enumerate() {
            out[r * indices.len() + j] = xd[r * x.cols + idx as usize];
        }
    }
    Ok(tensor(x.rows, indices.len(), out))
}

pub fn gather_rows_colblock(
    src: &GpuTensor,
    row_idx: &GpuTensor,
    colblock_idx: Option<&GpuTensor>,
    block_cols: usize,
) -> Result<GpuTensor, String> {
    let src_d = data(src)?;
    let rows = row_idx.u32s.borrow();
    let blocks = colblock_idx.map(|t| t.u32s.borrow());
    let out_rows = rows.len();
    let mut out = vec![0.0; out_rows * block_cols];
    let nblocks = src.cols / block_cols;
    for i in 0..out_rows {
        let r = rows[i];
        if r == u32::MAX {
            continue;
        }
        let b = blocks.as_ref().map(|bb| bb[i] as usize).unwrap_or(0);
        if (r as usize) >= src.rows || b >= nblocks {
            continue;
        }
        let src_off = (r as usize) * src.cols + b * block_cols;
        out[i * block_cols..(i + 1) * block_cols]
            .copy_from_slice(&src_d[src_off..src_off + block_cols]);
    }
    Ok(tensor(out_rows, block_cols, out))
}

pub fn layer_norm_mod(
    x: &GpuTensor,
    mods: &GpuTensor,
    scale_off: usize,
    shift_off: usize,
    eps: f32,
) -> Result<GpuTensor, String> {
    let xd = data(x)?;
    let md = data(mods)?;
    let scale = &md[scale_off..scale_off + x.cols];
    let shift = &md[shift_off..shift_off + x.cols];
    let out = try_layer_norm_mul_add_f32(
        &xd,
        &[x.rows, x.cols],
        scale,
        &[x.cols],
        shift,
        &[x.cols],
        eps,
    )
    .unwrap_or_else(|| {
        let mut out = vec![0.0; xd.len()];
        for r in 0..x.rows {
            let row = &xd[r * x.cols..(r + 1) * x.cols];
            let mean = row.iter().sum::<f32>() / x.cols as f32;
            let var = row.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / x.cols as f32;
            let inv = (var + eps).sqrt().recip();
            for c in 0..x.cols {
                out[r * x.cols + c] = (row[c] - mean) * inv * scale[c] + shift[c];
            }
        }
        out
    });
    Ok(tensor(x.rows, x.cols, out))
}

pub fn linear_nt(
    x: &GpuTensor,
    namespace: &str,
    parts: &[GpuLinearPart<'_>],
    bias: &[f32],
) -> Result<GpuTensor, String> {
    let w = cache_weight(namespace, parts);
    let n: usize = parts.iter().map(|p| p.n).sum();
    let k = x.cols;
    if w.len() != n * k {
        return Err(format!(
            "metal linear weight {} != n {n} * k {k}",
            w.len()
        ));
    }
    let xd = data(x)?;
    let mut out = try_matmul_nt_f32(&xd, &w, x.rows, k, n)
        .ok_or_else(|| "metal matmul_nt failed".to_string())?;
    if !bias.is_empty() {
        for r in 0..x.rows {
            for c in 0..n {
                out[r * n + c] += bias[c];
            }
        }
    }
    Ok(tensor(x.rows, n, out))
}

pub fn linear_f32_resident(
    x: &GpuTensor,
    w: &GpuTensor,
    bias: Option<&GpuTensor>,
) -> Result<GpuTensor, String> {
    let xd = data(x)?;
    let wd = data(w)?;
    let n = w.rows;
    let k = w.cols;
    let mut out = try_matmul_nt_f32(&xd, &wd, x.rows, k, n)
        .ok_or_else(|| "metal resident matmul failed".to_string())?;
    if let Some(bias) = bias {
        let bd = data(bias)?;
        for r in 0..x.rows {
            for c in 0..n {
                out[r * n + c] += bd[c];
            }
        }
    }
    Ok(tensor(x.rows, n, out))
}

pub fn attention_packed(
    q: &GpuTensor,
    k: &GpuTensor,
    v: &GpuTensor,
    heads: usize,
    scale: f32,
) -> Result<GpuTensor, String> {
    if heads == 0 || q.cols % heads != 0 {
        return Err("metal attention head mismatch".into());
    }
    let d = q.cols / heads;
    let qd = data(q)?;
    let kd = data(k)?;
    let vd = data(v)?;
    let out = try_flash_attn_f32_packed(&qd, &kd, &vd, q.rows, k.rows, heads, d, scale)
        .ok_or_else(|| "metal flash attn failed".to_string())?;
    Ok(tensor(q.rows, q.cols, out))
}

pub fn attention_packed_cross_bias(
    q: &GpuTensor,
    k: &GpuTensor,
    v: &GpuTensor,
    heads: usize,
    scale: f32,
    bias: &GpuTensor,
) -> Result<GpuTensor, String> {
    // Biased decoder path is small; stay on host so the additive RPB is exact.
    let d = q.cols / heads;
    let qd = data(q)?;
    let kd = data(k)?;
    let vd = data(v)?;
    let bd = data(bias)?;
    let q_len = q.rows;
    let kv_len = k.rows;
    let mut out = vec![0.0; q_len * q.cols];
    for h in 0..heads {
        for qi in 0..q_len {
            let mut scores = vec![0.0; kv_len];
            let mut max = f32::NEG_INFINITY;
            for ki in 0..kv_len {
                let mut dot = 0.0;
                for c in 0..d {
                    dot += qd[qi * q.cols + h * d + c] * kd[ki * k.cols + h * d + c];
                }
                let s = dot * scale + bd[h * q_len * kv_len + qi * kv_len + ki];
                scores[ki] = s;
                max = max.max(s);
            }
            let mut sum = 0.0;
            for s in &mut scores {
                *s = (*s - max).exp();
                sum += *s;
            }
            let inv = if sum > 0.0 { 1.0 / sum } else { 0.0 };
            for c in 0..d {
                let mut acc = 0.0;
                for ki in 0..kv_len {
                    acc += scores[ki] * inv * vd[ki * v.cols + h * d + c];
                }
                out[qi * q.cols + h * d + c] = acc;
            }
        }
    }
    Ok(tensor(q_len, q.cols, out))
}

pub fn conv2d_planar(
    x: &GpuTensor,
    width: usize,
    height: usize,
    weights: &[f32],
    bias: &[f32],
    out_channels: usize,
    kw: usize,
    kh: usize,
    pad_x: usize,
    pad_y: usize,
) -> Result<GpuTensor, String> {
    let xd = data(x)?;
    let out = try_conv2d_planar_f32(
        &xd,
        width,
        height,
        x.rows,
        weights,
        bias,
        out_channels,
        kw,
        kh,
        pad_x,
        pad_y,
    )
    .ok_or_else(|| "metal conv2d_planar failed".to_string())?;
    Ok(tensor(out_channels, width * height, out))
}

pub fn image_to_patches(
    image: &GpuTensor,
    image_width: usize,
    image_height: usize,
    out_width: usize,
    out_height: usize,
) -> Result<GpuTensor, String> {
    let xd = data(image)?;
    let patch_w = image_width / out_width;
    let patch_h = image_height / out_height;
    // SAM3 passes patch size as out_w/out_h (14) so gh = 1008/14 = 72.
    let gh = image_height / out_height;
    let gw = image_width / out_width;
    let dim = out_width * out_height;
    let ch = image.rows;
    let mut out = vec![0.0; ch * gh * gw * dim];
    for c in 0..ch {
        let plane = c * image_width * image_height;
        for gy in 0..gh {
            for gx in 0..gw {
                let row = c * gh * gw + gy * gw + gx;
                let mut d = 0;
                for py in 0..out_height {
                    for px in 0..out_width {
                        let y = gy * out_height + py;
                        let x = gx * out_width + px;
                        out[row * dim + d] = xd[plane + y * image_width + x];
                        d += 1;
                    }
                }
            }
        }
    }
    let _ = (patch_w, patch_h);
    Ok(tensor(ch * gh * gw, dim, out))
}

pub fn tokens_to_planar(tokens: &GpuTensor) -> Result<GpuTensor, String> {
    // [hw, c] -> [c, hw]
    let td = data(tokens)?;
    let hw = tokens.rows;
    let c = tokens.cols;
    let mut out = vec![0.0; c * hw];
    for i in 0..hw {
        for j in 0..c {
            out[j * hw + i] = td[i * c + j];
        }
    }
    Ok(tensor(c, hw, out))
}

pub fn resize_bilinear(
    x: &GpuTensor,
    in_w: usize,
    in_h: usize,
    out_w: usize,
    out_h: usize,
    align_corners: bool,
) -> Result<GpuTensor, String> {
    let xd = data(x)?;
    let ch = x.rows;
    let mut out = vec![0.0; ch * out_w * out_h];
    for c in 0..ch {
        let src = &xd[c * in_w * in_h..(c + 1) * in_w * in_h];
        let dst = &mut out[c * out_w * out_h..(c + 1) * out_w * out_h];
        for y in 0..out_h {
            let fy = if align_corners {
                if out_h == 1 {
                    0.0
                } else {
                    y as f32 * (in_h - 1) as f32 / (out_h - 1) as f32
                }
            } else {
                (y as f32 + 0.5) * in_h as f32 / out_h as f32 - 0.5
            };
            let y0 = fy.floor().max(0.0) as usize;
            let y1 = (y0 + 1).min(in_h - 1);
            let wy = fy - y0 as f32;
            for x in 0..out_w {
                let fx = if align_corners {
                    if out_w == 1 {
                        0.0
                    } else {
                        x as f32 * (in_w - 1) as f32 / (out_w - 1) as f32
                    }
                } else {
                    (x as f32 + 0.5) * in_w as f32 / out_w as f32 - 0.5
                };
                let x0 = fx.floor().max(0.0) as usize;
                let x1 = (x0 + 1).min(in_w - 1);
                let wx = fx - x0 as f32;
                let v00 = src[y0 * in_w + x0];
                let v01 = src[y0 * in_w + x1];
                let v10 = src[y1 * in_w + x0];
                let v11 = src[y1 * in_w + x1];
                dst[y * out_w + x] = (1.0 - wy) * ((1.0 - wx) * v00 + wx * v01)
                    + wy * ((1.0 - wx) * v10 + wx * v11);
            }
        }
    }
    Ok(tensor(ch, out_w * out_h, out))
}

pub fn pixel_shuffle(x: &GpuTensor, width: usize, height: usize, scale: usize) -> Result<GpuTensor, String> {
    let xd = data(x)?;
    let out_c = x.rows / (scale * scale);
    let out_w = width * scale;
    let out_h = height * scale;
    let mut out = vec![0.0; out_c * out_w * out_h];
    for oc in 0..out_c {
        for y in 0..height {
            for xcol in 0..width {
                for sy in 0..scale {
                    for sx in 0..scale {
                        let ic = oc * scale * scale + sy * scale + sx;
                        let v = xd[ic * width * height + y * width + xcol];
                        out[oc * out_w * out_h + (y * scale + sy) * out_w + (xcol * scale + sx)] = v;
                    }
                }
            }
        }
    }
    Ok(tensor(out_c, out_w * out_h, out))
}

pub fn upsample_nearest2x(x: &GpuTensor, width: usize, height: usize) -> Result<GpuTensor, String> {
    resize_bilinear(x, width, height, width * 2, height * 2, false)
}

pub fn rope_interleaved(
    x: &GpuTensor,
    heads: usize,
    cos: &GpuTensor,
    sin: &GpuTensor,
) -> Result<GpuTensor, String> {
    let xd = data(x)?;
    let cd = data(cos)?;
    let sd = data(sin)?;
    let dim = x.cols / heads;
    let half = dim / 2;
    let mut out = xd.clone();
    for r in 0..x.rows {
        for h in 0..heads {
            for i in 0..half {
                let base = r * x.cols + h * dim + i * 2;
                let c = cd[r * half + i];
                let s = sd[r * half + i];
                let a = xd[base];
                let b = xd[base + 1];
                out[base] = a * c - b * s;
                out[base + 1] = a * s + b * c;
            }
        }
    }
    Ok(tensor(x.rows, x.cols, out))
}

pub fn rope_half(
    x: &GpuTensor,
    heads: usize,
    cos: &GpuTensor,
    sin: &GpuTensor,
) -> Result<GpuTensor, String> {
    rope_interleaved(x, heads, cos, sin)
}

pub fn rpb_expand(
    ry: &GpuTensor,
    rx: &GpuTensor,
    height: usize,
    width: usize,
    queries: usize,
    heads: usize,
) -> Result<GpuTensor, String> {
    let ryd = data(ry)?;
    let rxd = data(rx)?;
    let q1 = queries + 1;
    let hw = height * width;
    let mut out = vec![0.0; heads * q1 * hw];
    for q in 0..queries {
        for y in 0..height {
            for x in 0..width {
                for h in 0..heads {
                    let v = ryd[(q * height + y) * heads + h] + rxd[(q * width + x) * heads + h];
                    out[(h * q1 + (q + 1)) * hw + y * width + x] = v;
                }
            }
        }
    }
    Ok(tensor(heads * q1, hw, out))
}

pub fn sam3_sine_embed(ref_points: &GpuTensor, num_pos_feats: usize) -> Result<GpuTensor, String> {
    let rp = data(ref_points)?;
    let n = ref_points.rows;
    let mut out = vec![0.0; n * num_pos_feats * 2];
    let scale = 2.0 * std::f32::consts::PI;
    for i in 0..n {
        let y = rp[i * 4];
        let x = rp[i * 4 + 1];
        for k in 0..num_pos_feats / 2 {
            let freq = (k as f32 / (num_pos_feats as f32 / 2.0)).exp2();
            let yv = y * scale / freq;
            let xv = x * scale / freq;
            out[i * num_pos_feats * 2 + k] = yv.sin();
            out[i * num_pos_feats * 2 + num_pos_feats / 2 + k] = yv.cos();
            out[i * num_pos_feats * 2 + num_pos_feats + k] = xv.sin();
            out[i * num_pos_feats * 2 + num_pos_feats + num_pos_feats / 2 + k] = xv.cos();
        }
    }
    Ok(tensor(n, num_pos_feats * 2, out))
}

pub fn sam3_rpb_axial(
    ref_points: &GpuTensor,
    width: usize,
    height: usize,
) -> Result<(GpuTensor, GpuTensor), String> {
    let rp = data(ref_points)?;
    let q = ref_points.rows;
    let mut dx = vec![0.0; q * width];
    let mut dy = vec![0.0; q * height];
    for i in 0..q {
        let cy = rp[i * 4];
        let cx = rp[i * 4 + 1];
        for x in 0..width {
            dx[i * width + x] = (x as f32 + 0.5) / width as f32 - cx;
        }
        for y in 0..height {
            dy[i * height + y] = (y as f32 + 0.5) / height as f32 - cy;
        }
    }
    Ok((tensor(q, width, dx), tensor(q, height, dy)))
}

pub fn sam3_refine_boxes(ref_points: &GpuTensor, delta: &GpuTensor) -> Result<GpuTensor, String> {
    let rp = data(ref_points)?;
    let d = data(delta)?;
    let mut out = rp.clone();
    for i in 0..ref_points.rows {
        for c in 0..4 {
            let x = rp[i * 4 + c];
            let logit = (x.clamp(1e-4, 1.0 - 1e-4)).ln() - (1.0 - x.clamp(1e-4, 1.0 - 1e-4)).ln();
            out[i * 4 + c] = 1.0 / (1.0 + (-(logit + d[i * 4 + c])).exp());
        }
    }
    Ok(tensor(ref_points.rows, 4, out))
}

pub fn group_norm_planar(
    x: &GpuTensor,
    width: usize,
    height: usize,
    groups: usize,
    gamma: &[f32],
    beta: &[f32],
    eps: f32,
) -> Result<GpuTensor, String> {
    let xd = data(x)?;
    let out = try_group_norm_planar_f32(&xd, width, height, x.rows, groups, gamma, beta, eps)
        .ok_or_else(|| "metal group_norm failed".to_string())?;
    Ok(tensor(x.rows, width * height, out))
}

pub fn evict_prefix(_prefix: &str) -> Result<usize, String> {
    Ok(0)
}

pub fn pool_clear() {}
