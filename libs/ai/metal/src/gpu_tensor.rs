//! Device-resident shared-buffer tensors on macOS. GPU operations append to
//! one ordered process batch; host operations borrow mapped storage after
//! flushing its writer. Row views share immutable contents until a host write.

use makepad_ai_loader::{WeightKey, WeightOwner, WeightOwnerTrace};
use crate::gpu_types::{fresh_tensor_id, GpuLinearPart, GpuTensor, TensorStorage};
pub use crate::shim::{DecAttnRef, DecLinearRef, TwoWayLayerRef, VitLayerRef, VitLinearRef};
use crate::shim;
use crate::shim::{try_two_way_layer_resident_f32, try_vit_backbone_resident_f32};
use std::cell::{Cell, Ref, RefCell, RefMut};
use std::sync::Arc;
use makepad_ai_loader::quant::GGML_TYPE_F32;

fn extent(rows: usize, cols: usize) -> Result<usize, String> {
    rows.checked_mul(cols).and_then(|n| n.checked_mul(4))
        .ok_or_else(|| "metal tensor extent overflow".into())
}

fn from_buffer(rows: usize, cols: usize, buffer: Arc<shim::TensorBuffer>, offset: usize, writer: u64) -> GpuTensor {
    GpuTensor { rows, cols, storage: RefCell::new(TensorStorage { buffer, offset }),
        writer_batch: Cell::new(writer), u32s: RefCell::new(Vec::new()), id: Cell::new(fresh_tensor_id()) }
}

fn empty(rows: usize, cols: usize, host: bool) -> Result<GpuTensor, String> {
    Ok(from_buffer(rows, cols, shim::tensor_buffer(extent(rows, cols)?, host)?, 0, 0))
}

/// The RefCell borrow guards the mapped view, including against in-place writes.
pub(crate) fn data(t: &GpuTensor) -> Result<Ref<'_, [f32]>, String> {
    shim::tensor_thread_guard()?;
    let storage = t.storage.try_borrow().map_err(|_| "metal GpuTensor already borrowed".to_string())?;
    shim::tensor_flush_until(t.writer_batch.get())?;
    let ptr = storage.buffer.contents()?;
    let offset = storage.offset / 4;
    Ok(Ref::map(storage, |_| unsafe { std::slice::from_raw_parts(ptr.add(offset), t.rows * t.cols) }))
}

pub(crate) fn data_mut(t: &GpuTensor) -> Result<RefMut<'_, [f32]>, String> {
    shim::tensor_thread_guard()?;
    let mut storage = t.storage.try_borrow_mut().map_err(|_| "metal GpuTensor already borrowed".to_string())?;
    if Arc::strong_count(&storage.buffer) > 1 {
        shim::tensor_flush_until(t.writer_batch.get())?;
        let replacement = shim::tensor_buffer(extent(t.rows, t.cols)?, true)?;
        unsafe {
            std::ptr::copy_nonoverlapping(storage.buffer.contents()?.add(storage.offset / 4),
                replacement.contents()?, t.rows * t.cols);
        }
        *storage = TensorStorage { buffer: replacement, offset: 0 };
        shim::tensor_cow_record();
    }
    // A unique host handle may still be referenced by a command buffer.
    shim::tensor_flush_until(storage.buffer.last_batch())?;
    let ptr = storage.buffer.contents()?;
    let offset = storage.offset / 4;
    shim::tensor_host_written(&storage.buffer)?;
    t.writer_batch.set(0);
    t.id.set(fresh_tensor_id());
    Ok(RefMut::map(storage, |_| unsafe { std::slice::from_raw_parts_mut(ptr.add(offset), t.rows * t.cols) }))
}

/// Owned mapped output used by host loops; moving it into a tensor copies no bytes.
pub(crate) struct HostOutput {
    buffer: Arc<shim::TensorBuffer>,
    ptr: *mut f32,
    len: usize,
    capacity: usize,
}

impl HostOutput {
    pub(crate) fn zeroed(len: usize) -> Result<Self, String> {
        let mut out = Self::with_capacity(len)?;
        unsafe { std::ptr::write_bytes(out.ptr, 0, len); }
        out.len = len;
        Ok(out)
    }
    pub(crate) fn with_capacity(capacity: usize) -> Result<Self, String> {
        let buffer = shim::tensor_buffer(extent(1, capacity)?, true)?;
        let ptr = buffer.contents()?;
        Ok(Self { buffer, ptr, len: 0, capacity })
    }
    pub(crate) fn copy(values: &[f32]) -> Result<Self, String> {
        let mut out = Self::with_capacity(values.len())?;
        out.extend_from_slice(values);
        Ok(out)
    }
    pub(crate) fn push(&mut self, value: f32) {
        assert!(self.len < self.capacity, "mapped output extent");
        unsafe { self.ptr.add(self.len).write(value); }
        self.len += 1;
    }
    pub(crate) fn extend(&mut self, values: impl IntoIterator<Item=f32>) {
        for value in values { self.push(value); }
    }
    pub(crate) fn extend_from_slice(&mut self, values: &[f32]) {
        assert!(values.len() <= self.capacity - self.len, "mapped output extent");
        unsafe { std::ptr::copy_nonoverlapping(values.as_ptr(), self.ptr.add(self.len), values.len()); }
        self.len += values.len();
    }
}
impl std::ops::Deref for HostOutput {
    type Target = [f32];
    fn deref(&self) -> &[f32] { unsafe { std::slice::from_raw_parts(self.ptr, self.len) } }
}
impl std::ops::DerefMut for HostOutput {
    fn deref_mut(&mut self) -> &mut [f32] { unsafe { std::slice::from_raw_parts_mut(self.ptr, self.len) } }
}

pub(crate) fn tensor(rows: usize, cols: usize, out: HostOutput) -> GpuTensor {
    assert_eq!(rows * cols, out.len, "mapped output shape");
    from_buffer(rows, cols, out.buffer, 0, 0)
}

pub fn available() -> bool { shim::tensor_available() }

/// Lock hold high-water in ns and cumulative COW copies; subtract snapshots per forward.
pub fn residency_profile() -> (u64, u64) { shim::tensor_profile() }

pub fn upload(values: &[f32], rows: usize, cols: usize) -> Result<GpuTensor, String> {
    if extent(rows, cols)? / 4 != values.len() { return Err("metal upload size mismatch".into()); }
    Ok(tensor(rows, cols, HostOutput::copy(values)?))
}
pub fn upload_u32(values: &[u32]) -> Result<GpuTensor, String> {
    let tensor = tensor(values.len().max(1), 1, HostOutput::zeroed(values.len().max(1))?);
    *tensor.u32s.borrow_mut() = values.to_vec();
    Ok(tensor)
}
pub fn download(t: &GpuTensor) -> Result<Vec<f32>, String> { Ok(data(t)?.to_vec()) }
pub fn upload_into(t: &GpuTensor, values: &[f32]) -> Result<(), String> {
    if values.len() != extent(t.rows, t.cols)? / 4 { return Err("metal upload_into size mismatch".into()); }
    data_mut(t)?.copy_from_slice(values);
    Ok(())
}
pub fn copy_into(src: &GpuTensor, dst: &GpuTensor) -> Result<(), String> {
    if std::ptr::eq(src, dst) { drop(data_mut(dst)?); return Ok(()); }
    if extent(src.rows, src.cols)? != extent(dst.rows, dst.cols)? { return Err("metal copy_into shape mismatch".into()); }
    let input = data(src)?;
    data_mut(dst)?.copy_from_slice(&input);
    Ok(())
}
pub fn to_f32(t: &GpuTensor) -> Result<GpuTensor, String> {
    Ok(tensor(t.rows, t.cols, HostOutput::copy(&data(t)?)?))
}

fn binary(a: &GpuTensor, b: &GpuTensor, multiply: bool) -> Result<GpuTensor, String> {
    let out = empty(a.rows, a.cols, false)?;
    let ad = a.storage.try_borrow().map_err(|_| "metal binary borrow".to_string())?;
    let bd = b.storage.try_borrow().map_err(|_| "metal binary borrow".to_string())?;
    let output = out.storage.borrow();
    out.writer_batch.set(shim::tensor_binary(&ad.buffer, ad.offset, &shim::shape4_from_row_major(&[a.rows, a.cols], 4)?,
        &bd.buffer, bd.offset, &shim::shape4_from_row_major(&[b.rows, b.cols], 4)?, &output.buffer, multiply)?);
    drop(output);
    Ok(out)
}
pub fn add(a: &GpuTensor, b: &GpuTensor) -> Result<GpuTensor, String> { binary(a, b, false) }
pub fn mul(a: &GpuTensor, b: &GpuTensor) -> Result<GpuTensor, String> { binary(a, b, true) }
pub fn add_cols_broadcast(a: &GpuTensor, b: &GpuTensor) -> Result<GpuTensor, String> {
    if b.rows * b.cols != a.cols { return Err("metal broadcast width mismatch".into()); }
    let bias = reshape(b, 1, a.cols)?;
    binary(a, &bias, false)
}
fn unary(x: &GpuTensor, op: i16) -> Result<GpuTensor, String> {
    let out = empty(x.rows, x.cols, false)?;
    let input = x.storage.try_borrow().map_err(|_| "metal unary borrow".to_string())?;
    let output = out.storage.borrow();
    out.writer_batch.set(shim::tensor_unary(&input.buffer, input.offset, &shim::shape4_from_row_major(&[x.rows, x.cols], 4)?, &output.buffer, op)?);
    drop(output);
    Ok(out)
}
pub fn silu(x: &GpuTensor) -> Result<GpuTensor, String> { unary(x, 106) }
pub fn gelu(x: &GpuTensor) -> Result<GpuTensor, String> { unary(x, 103) }
// Preserve step 2's public arithmetic: this entry used the tanh GELU kernel.
pub fn gelu_erf(x: &GpuTensor) -> Result<GpuTensor, String> { gelu(x) }
pub fn relu(x: &GpuTensor) -> Result<GpuTensor, String> { unary(x, 101) }

pub fn slice_rows(x: &GpuTensor, start: usize, count: usize) -> Result<GpuTensor, String> {
    shim::tensor_thread_guard()?;
    if start.checked_add(count).is_none_or(|end| end > x.rows) { return Err("metal slice_rows oob".into()); }
    let input = x.storage.try_borrow().map_err(|_| "metal slice_rows borrow".to_string())?;
    Ok(from_buffer(count, x.cols, input.buffer.clone(), input.offset + start * x.cols * 4, x.writer_batch.get()))
}

/// Encode one strided copy; only shape metadata is handled on the host.
fn copy_strided(
    src: &GpuTensor, src_offset: usize, src_ne: [usize; 4], src_nb: [usize; 4],
    dst: &GpuTensor, dst_offset: usize, dst_ne: [usize; 4], dst_nb: [usize; 4],
) -> Result<(), String> {
    let input = src.storage.try_borrow().map_err(|_| "metal copy input borrow".to_string())?;
    let output = dst.storage.try_borrow().map_err(|_| "metal copy output borrow".to_string())?;
    let writer = shim::tensor_copy_strided(&input.buffer, input.offset + src_offset, src_ne, src_nb,
        &output.buffer, output.offset + dst_offset, dst_ne, dst_nb)?;
    dst.writer_batch.set(dst.writer_batch.get().max(writer));
    Ok(())
}

pub fn slice_cols(x: &GpuTensor, start: usize, count: usize) -> Result<GpuTensor, String> {
    if start.checked_add(count).is_none_or(|end| end > x.cols) {
        return Err("metal slice_cols oob".into());
    }
    let out = empty(x.rows, count, false)?;
    let ne = [count, x.rows, 1, 1];
    copy_strided(x, start * 4, ne, [4, x.cols * 4, 0, 0],
        &out, 0, ne, [4, count * 4, 0, 0])?;
    Ok(out)
}

pub fn concat_rows(a: &GpuTensor, b: &GpuTensor) -> Result<GpuTensor, String> {
    if a.cols != b.cols { return Err("metal concat_rows col mismatch".into()); }
    let rows = a.rows.checked_add(b.rows).ok_or("metal concat_rows overflow")?;
    let out = empty(rows, a.cols, false)?;
    let mut offset = 0;
    for part in [a, b] {
        let ne = [part.cols, part.rows, 1, 1];
        let nb = [4, part.cols * 4, 0, 0];
        copy_strided(part, 0, ne, nb, &out, offset, ne, nb)?;
        offset += extent(part.rows, part.cols)?;
    }
    Ok(out)
}

pub fn concat_cols(parts: &[&GpuTensor]) -> Result<GpuTensor, String> {
    let rows = parts.first().ok_or("metal concat_cols empty")?.rows;
    let cols = parts.iter().try_fold(0usize, |cols, part| {
        if part.rows != rows { return Err("metal concat_cols row mismatch"); }
        cols.checked_add(part.cols).ok_or("metal concat_cols overflow")
    })?;
    let out = empty(rows, cols, false)?;
    let mut offset = 0;
    for part in parts {
        let ne = [part.cols, rows, 1, 1];
        copy_strided(part, 0, ne, [4, part.cols * 4, 0, 0],
            &out, offset, ne, [4, cols * 4, 0, 0])?;
        offset += part.cols * 4;
    }
    Ok(out)
}

pub fn reshape(x: &GpuTensor, rows: usize, cols: usize) -> Result<GpuTensor, String> {
    shim::tensor_thread_guard()?;
    if extent(rows, cols)? != extent(x.rows, x.cols)? { return Err("metal reshape size".into()); }
    let input = x.storage.try_borrow().map_err(|_| "metal reshape borrow".to_string())?;
    Ok(from_buffer(rows, cols, input.buffer.clone(), input.offset, x.writer_batch.get()))
}

pub fn gather_cols(x: &GpuTensor, indices: &[u32]) -> Result<GpuTensor, String> {
    let xd = data(x)?;
    let mut out = HostOutput::zeroed(x.rows * indices.len())?;
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
    let mut out = HostOutput::zeroed(out_rows * block_cols)?;
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

pub fn layer_norm_mod(x: &GpuTensor, mods: &GpuTensor, scale_off: usize, shift_off: usize, eps: f32) -> Result<GpuTensor, String> {
    let out = empty(x.rows, x.cols, false)?;
    let input = x.storage.try_borrow().map_err(|_| "metal norm borrow".to_string())?;
    let affine = mods.storage.try_borrow().map_err(|_| "metal norm borrow".to_string())?;
    if scale_off.checked_add(x.cols).is_none_or(|end| end > mods.rows * mods.cols)
        || shift_off.checked_add(x.cols).is_none_or(|end| end > mods.rows * mods.cols) {
        return Err("metal norm affine extent".into());
    }
    let output = out.storage.borrow();
    out.writer_batch.set(shim::tensor_norm(&input.buffer, input.offset, &shim::shape4_from_row_major(&[x.rows, x.cols], 4)?,
        &affine.buffer, affine.offset + scale_off * 4, &shim::shape4_from_row_major(&[x.cols], 4)?,
        &affine.buffer, affine.offset + shift_off * 4, &shim::shape4_from_row_major(&[x.cols], 4)?, &output.buffer, eps, 3)?);
    drop(output);
    Ok(out)
}

pub fn linear_nt(x: &GpuTensor, owner: &WeightOwner, parts: &[GpuLinearPart<'_>], bias: &[f32]) -> Result<GpuTensor, String> {
    let n: usize = parts.iter().map(|p| p.n).sum();
    if parts.is_empty() || (!bias.is_empty() && bias.len() != n) { return Err("metal linear parts/bias shape".into()); }
    let input = x.storage.try_borrow().map_err(|_| "metal linear borrow".to_string())?;
    let mut outputs = Vec::with_capacity(parts.len());
    for part in parts {
        let weight = shim::tensor_named_weight(owner, part.cache_key, || Ok(std::borrow::Cow::Borrowed(part.bytes)))?;
        let out = empty(x.rows, part.n, false)?;
        let output = out.storage.borrow();
        out.writer_batch.set(shim::tensor_matmul(&input.buffer, input.offset, &weight, 0,
            &output.buffer, x.rows, x.cols, part.n, part.bt_ggml_type)?);
        drop(output);
        outputs.push(out);
    }
    let output = if outputs.len() == 1 { outputs.remove(0) }
        else { concat_cols(&outputs.iter().collect::<Vec<_>>())? };
    if bias.is_empty() { Ok(output) }
    else { add_cols_broadcast(&output, &upload(bias, 1, n)?) }
}

/// F32 weights bind their existing tensor buffer, including row-view offsets.
pub fn linear_f32_resident(x: &GpuTensor, w: &GpuTensor, bias: Option<&GpuTensor>) -> Result<GpuTensor, String> {
    if x.cols != w.cols { return Err("metal resident linear k mismatch".into()); }
    let out = empty(x.rows, w.rows, false)?;
    let input = x.storage.try_borrow().map_err(|_| "metal linear borrow".to_string())?;
    let weight = w.storage.try_borrow().map_err(|_| "metal linear borrow".to_string())?;
    let output = out.storage.borrow();
    out.writer_batch.set(shim::tensor_matmul(&input.buffer, input.offset, &weight.buffer, weight.offset,
        &output.buffer, x.rows, x.cols, w.rows, GGML_TYPE_F32)?);
    drop(output);
    if let Some(bias) = bias { add_cols_broadcast(&out, bias) } else { Ok(out) }
}

pub fn attention_packed(q: &GpuTensor, k: &GpuTensor, v: &GpuTensor, heads: usize, scale: f32) -> Result<GpuTensor, String> {
    if heads == 0 || q.cols % heads != 0 || k.cols != q.cols || v.cols != q.cols || k.rows != v.rows {
        return Err("metal attention head/shape mismatch".into());
    }
    let out = empty(q.rows, q.cols, false)?;
    let qd = q.storage.try_borrow().map_err(|_| "metal attention borrow".to_string())?;
    let kd = k.storage.try_borrow().map_err(|_| "metal attention borrow".to_string())?;
    let vd = v.storage.try_borrow().map_err(|_| "metal attention borrow".to_string())?;
    let output = out.storage.borrow();
    out.writer_batch.set(shim::tensor_attention(&qd.buffer, qd.offset, &kd.buffer, kd.offset,
        &vd.buffer, vd.offset, &output.buffer, q.rows, k.rows, heads, q.cols / heads, scale)?);
    drop(output);
    Ok(out)
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
    let mut out = HostOutput::zeroed(q_len * q.cols)?;
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

/// Gate-first SwiGLU over a slab of each row, using CUDA's f32 arithmetic.
pub fn swiglu_gate_first(x: &GpuTensor, gate_offset: usize, n: usize) -> Result<GpuTensor, String> {
    if n == 0 || gate_offset.checked_add(n.saturating_mul(2)).is_none_or(|end| end > x.cols) {
        return Err(format!(
            "gpu_swiglu_gate_first slab {gate_offset}+2*{n} outside {} cols", x.cols
        ));
    }
    let xd = data(x)?;
    let mut out = HostOutput::with_capacity(x.rows * n)?;
    for r in 0..x.rows {
        let base = r * x.cols + gate_offset;
        for c in 0..n {
            let gate = xd[base + c];
            let value = xd[base + n + c];
            let s = gate / (1.0f32 + (-gate).exp());
            out.push(value * s);
        }
    }
    Ok(tensor(x.rows, n, out))
}

pub fn global_avg_pool(x: &GpuTensor) -> Result<GpuTensor, String> {
    if x.cols == 0 {
        return Err("gpu_birefnet_global_avg_pool shape mismatch".into());
    }
    let xd = data(x)?;
    let mut out = HostOutput::with_capacity(x.rows)?;
    out.extend(xd.chunks_exact(x.cols).map(|row| row.iter().copied().sum::<f32>() / x.cols as f32));
    Ok(tensor(x.rows, 1, out))
}

pub fn bias_lrelu_inplace(x: &GpuTensor, bias: &[f32], slope: f32) -> Result<(), String> {
    if bias.len() != x.rows {
        return Err("gpu_realesrgan_bias_lrelu_f32 bias length mismatch".into());
    }
    let mut xd = data_mut(x)?;
    for r in 0..x.rows {
        for c in 0..x.cols {
            let v = xd[r * x.cols + c] + bias[r];
            xd[r * x.cols + c] = if v > 0.0 { v } else { v * slope };
        }
    }
    x.id.set(fresh_tensor_id());
    Ok(())
}

/// Bytes of im2col scratch per band. A larger convolution runs in more
/// bands (one output row at least), never refused.
const CONV2D_IM2COL_BAND_BYTES: usize = 256 * 1024 * 1024;

pub fn conv2d_planar(
    x: &GpuTensor, width: usize, height: usize, weights: &[f32], bias: &[f32],
    out_channels: usize, kw: usize, kh: usize, pad_x: usize, pad_y: usize,
) -> Result<GpuTensor, String> {
    conv2d_planar_banded(x, width, height, weights, bias, out_channels, kw, kh, pad_x, pad_y, CONV2D_IM2COL_BAND_BYTES)
}

pub(crate) fn conv2d_planar_banded(
    x: &GpuTensor, width: usize, height: usize, weights: &[f32], bias: &[f32],
    out_channels: usize, kw: usize, kh: usize, pad_x: usize, pad_y: usize, band_bytes: usize,
) -> Result<GpuTensor, String> {
    let overflow = || "metal conv2d extent overflow".to_string();
    let plane = width.checked_mul(height).ok_or_else(overflow)?;
    let k = x.rows.checked_mul(kh).and_then(|n| n.checked_mul(kw)).ok_or_else(overflow)?;
    let ow = pad_x.checked_mul(2).and_then(|n| width.checked_add(n)).and_then(|n| n.checked_sub(kw))
        .and_then(|n| n.checked_add(1)).ok_or_else(overflow)?;
    let oh = pad_y.checked_mul(2).and_then(|n| height.checked_add(n)).and_then(|n| n.checked_sub(kh))
        .and_then(|n| n.checked_add(1)).ok_or_else(overflow)?;
    if plane == 0 || k == 0 || ow == 0 || oh == 0 || out_channels == 0 || x.cols != plane
        || weights.len() != out_channels.checked_mul(k).ok_or_else(overflow)?
        || (!bias.is_empty() && bias.len() != out_channels) { return Err("metal conv2d shape mismatch".into()); }
    let pixels = ow.checked_mul(oh).ok_or_else(overflow)?;
    let weight = upload(weights, out_channels, k)?;
    let row_bytes = extent(ow, k)?;
    let band_rows = (band_bytes / row_bytes).max(1).min(oh);
    let scratch = empty(band_rows * ow, k, false)?;
    let input = x.storage.try_borrow().map_err(|_| "metal conv borrow".to_string())?;
    let scratch_storage = scratch.storage.borrow();
    let mut bands = Vec::new();
    for r0 in (0..oh).step_by(band_rows) {
        let rows = band_rows.min(oh - r0);
        let origin = r0.saturating_sub(pad_y).min(height);
        scratch.writer_batch.set(shim::tensor_im2col(&input.buffer, input.offset + origin * width * 4,
            &scratch_storage.buffer, x.rows, width, height - origin, plane, kw, kh,
            pad_x, pad_y.saturating_sub(r0), ow, rows)?);
        // This scratch is private to the convolution and is reused only by ordered GPU ops.
        let slab = from_buffer(rows * ow, k, scratch_storage.buffer.clone(), 0, scratch.writer_batch.get());
        let values = linear_f32_resident(&slab, &weight, None)?;
        bands.push((r0, rows * ow, values));
    }
    let mut out = HostOutput::zeroed(out_channels.checked_mul(pixels).ok_or_else(overflow)?)?;
    for (r0, count, values) in bands {
        let values = data(&values)?;
        for c in 0..out_channels {
            let b = bias.get(c).copied().unwrap_or(0.0);
            for p in 0..count { out[c * pixels + r0 * ow + p] = values[p * out_channels + c] + b; }
        }
    }
    Ok(tensor(out_channels, pixels, out))
}


pub fn image_to_patches(
    image: &GpuTensor, image_width: usize, image_height: usize,
    out_width: usize, out_height: usize,
) -> Result<GpuTensor, String> {
    if out_width == 0 || out_height == 0
        || image_width.checked_mul(image_height) != Some(image.cols) {
        return Err("metal image_to_patches shape mismatch".into());
    }
    let gh = image_height / out_height;
    let gw = image_width / out_width;
    let dim = out_width.checked_mul(out_height).ok_or("metal patch extent overflow")?;
    let rows = image.rows.checked_mul(gh).and_then(|n| n.checked_mul(gw))
        .ok_or("metal patch extent overflow")?;
    let out = empty(rows, dim, false)?;
    let ne = [out_width, out_height, gw, gh];
    // Each channel is a 4-D permutation: px, py, grid-x, grid-y.
    for channel in 0..image.rows {
        copy_strided(image, channel * image.cols * 4, ne,
            [4, image_width * 4, out_width * 4, out_height * image_width * 4],
            &out, channel * gh * gw * dim * 4, ne,
            [4, out_width * 4, dim * 4, gw * dim * 4])?;
    }
    Ok(out)
}

pub fn tokens_to_planar(tokens: &GpuTensor) -> Result<GpuTensor, String> {
    let out = empty(tokens.cols, tokens.rows, false)?;
    let ne = [tokens.rows, tokens.cols, 1, 1];
    copy_strided(tokens, 0, ne, [tokens.cols * 4, 4, 0, 0],
        &out, 0, ne, [4, tokens.rows * 4, 0, 0])?;
    Ok(out)
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
    let xd: &[f32] = &xd;
    let ch = x.rows;
    let prof_t0 = std::time::Instant::now();
    let mut out = HostOutput::zeroed(ch * out_w * out_h)?;
    // Channel-parallel: RIFE resizes many-channel tensors every block and
    // the scalar single-thread loop was a top-3 cost in its profile.
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    let small = ch * out_w * out_h < 1_000_000;
    let per = if threads <= 1 || ch < 2 || small { ch } else { ch.div_ceil(threads.min(ch)) };
    std::thread::scope(|scope| {
    let mut chunks = out.chunks_mut(per * out_w * out_h);
    let mut c0 = 0usize;
    while let Some(chunk) = chunks.next() {
        let chunk_ch = chunk.len() / (out_w * out_h);
        let start = c0;
        c0 += chunk_ch;
        scope.spawn(move || {
        for (ci, dst) in chunk.chunks_mut(out_w * out_h).enumerate() {
        let c = start + ci;
        let src = &xd[c * in_w * in_h..(c + 1) * in_w * in_h];
        for y in 0..out_h {
            let fy = if align_corners {
                if out_h == 1 {
                    0.0
                } else {
                    y as f32 * (in_h - 1) as f32 / (out_h - 1) as f32
                }
            } else {
                // torch half-pixel: source coord clamps to 0 BEFORE the
                // weight is taken, so the edge row repeats instead of
                // extrapolating (weight-after-clamp gave negative weights
                // at the border — RIFE's per-block resizes amplified that
                // edge error through the whole pyramid).
                ((y as f32 + 0.5) * in_h as f32 / out_h as f32 - 0.5).max(0.0)
            };
            let y0 = (fy.floor() as usize).min(in_h - 1);
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
                    ((x as f32 + 0.5) * in_w as f32 / out_w as f32 - 0.5).max(0.0)
                };
                let x0 = (fx.floor() as usize).min(in_w - 1);
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
        });
    }
    });
    crate::rife::prof_add(4, prof_t0);
    Ok(tensor(ch, out_w * out_h, out))
}

pub fn pixel_shuffle(x: &GpuTensor, width: usize, height: usize, scale: usize) -> Result<GpuTensor, String> {
    let square = scale.checked_mul(scale).filter(|&n| n != 0)
        .ok_or("metal pixel_shuffle scale")?;
    if width.checked_mul(height) != Some(x.cols) {
        return Err("metal pixel_shuffle shape mismatch".into());
    }
    let out_c = x.rows / square;
    let out_w = width.checked_mul(scale).ok_or("metal pixel_shuffle extent overflow")?;
    let out_h = height.checked_mul(scale).ok_or("metal pixel_shuffle extent overflow")?;
    let plane = out_w.checked_mul(out_h).ok_or("metal pixel_shuffle extent overflow")?;
    let out = empty(out_c, plane, false)?;
    let ne = [scale, width, scale, height];
    // sx is contiguous in the output and strided across source channels.
    for channel in 0..out_c {
        copy_strided(x, channel * square * x.cols * 4, ne,
            [x.cols * 4, 4, scale * x.cols * 4, width * 4],
            &out, channel * plane * 4, ne,
            [4, scale * 4, out_w * 4, scale * out_w * 4])?;
    }
    Ok(out)
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
    let mut out = HostOutput::copy(&xd)?;
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

/// Rotate-half rope (the DINOv3 / LLaMA layout): within each head the first
/// `rot_half` lanes pair with the next `rot_half`, `out1 = x1 c - x2 s`,
/// `out2 = x2 c + x1 s`, with one `[rows, rot_half]` cos/sin table shared by
/// both halves; lanes past `2 * rot_half` pass through. Same contract as the
/// CUDA `rope_half` kernel.
pub fn rope_half(
    x: &GpuTensor,
    heads: usize,
    rot_half: usize,
    cos: &GpuTensor,
    sin: &GpuTensor,
) -> Result<GpuTensor, String> {
    if heads == 0 || x.cols % heads != 0 {
        return Err("metal rope_half head mismatch".to_string());
    }
    let dim = x.cols / heads;
    if rot_half * 2 > dim {
        return Err("metal rope_half rotary span exceeds head dim".to_string());
    }
    if cos.rows != x.rows || cos.cols != rot_half || sin.rows != x.rows || sin.cols != rot_half {
        return Err("metal rope_half table mismatch".to_string());
    }
    let xd = data(x)?;
    let cd = data(cos)?;
    let sd = data(sin)?;
    let mut out = HostOutput::copy(&xd)?;
    for r in 0..x.rows {
        for h in 0..heads {
            let base = r * x.cols + h * dim;
            for i in 0..rot_half {
                let c = cd[r * rot_half + i];
                let s = sd[r * rot_half + i];
                let x1 = xd[base + i];
                let x2 = xd[base + rot_half + i];
                out[base + i] = x1 * c - x2 * s;
                out[base + rot_half + i] = x2 * c + x1 * s;
            }
        }
    }
    Ok(tensor(x.rows, x.cols, out))
}

/// Per-row layer norm with an affine: `(x - mean) / sqrt(var + eps) * mul + add`
/// (biased variance), `mul`/`add` one value per column.
/// A whole pre-norm ViT stack device-resident (see `shim::VitLayerRef`):
/// `x` `[seq, dim]` goes up once, every layer encodes into one command
/// buffer, and the final layer-normed activations come back.
#[allow(clippy::too_many_arguments)]
pub fn vit_backbone_resident(
    x: &GpuTensor,
    n_head: usize,
    rot_half: usize,
    cos: &GpuTensor,
    sin: &GpuTensor,
    layers: &[VitLayerRef<'_>],
    final_norm_w: &[f32],
    final_norm_b: &[f32],
    eps: f32,
) -> Result<GpuTensor, String> {
    let xd = data(x)?;
    let cd = data(cos)?;
    let sd = data(sin)?;
    let out = try_vit_backbone_resident_f32(
        &xd,
        x.rows,
        x.cols,
        n_head,
        rot_half,
        &cd,
        &sd,
        layers,
        final_norm_w,
        final_norm_b,
        eps,
    )
    .ok_or_else(|| "metal resident vit backbone failed".to_string())?;
    upload(&out, x.rows, x.cols)
}

/// One two-way decoder layer device-resident (see `shim::TwoWayLayerRef`);
/// returns `(hidden, ln_final(hidden))`.
pub fn two_way_layer_resident(
    hidden: &GpuTensor,
    token_pe: &GpuTensor,
    context: &GpuTensor,
    context_pe: &GpuTensor,
    layer: &TwoWayLayerRef<'_>,
) -> Result<(GpuTensor, GpuTensor), String> {
    if token_pe.rows != hidden.rows
        || token_pe.cols != hidden.cols
        || context_pe.rows != context.rows
        || context_pe.cols != context.cols
    {
        return Err("metal two-way layer: PE shapes do not match their tensors".to_string());
    }
    let h = data(hidden)?;
    let t = data(token_pe)?;
    let c = data(context)?;
    let cp = data(context_pe)?;
    let (out, normed) = try_two_way_layer_resident_f32(
        &h,
        &t,
        &c,
        &cp,
        hidden.rows,
        hidden.cols,
        context.rows,
        context.cols,
        layer,
    )
    .ok_or_else(|| "metal resident two-way layer failed".to_string())?;
    Ok((
        upload(&out, hidden.rows, hidden.cols)?,
        upload(&normed, hidden.rows, hidden.cols)?,
    ))
}

pub fn layer_norm_mul_add(x: &GpuTensor, mul: &[f32], add: &[f32], eps: f32) -> Result<GpuTensor, String> {
    if mul.len() != x.cols || add.len() != x.cols { return Err("metal norm affine width mismatch".into()); }
    let scale = upload(mul, 1, x.cols)?;
    let shift = upload(add, 1, x.cols)?;
    let out = empty(x.rows, x.cols, false)?;
    let input = x.storage.try_borrow().map_err(|_| "metal norm borrow".to_string())?;
    let scale = scale.storage.borrow();
    let shift = shift.storage.borrow();
    let output = out.storage.borrow();
    out.writer_batch.set(shim::tensor_norm(&input.buffer, input.offset, &shim::shape4_from_row_major(&[x.rows, x.cols], 4)?,
        &scale.buffer, 0, &shim::shape4_from_row_major(&[x.cols], 4)?,
        &shift.buffer, 0, &shim::shape4_from_row_major(&[x.cols], 4)?, &output.buffer, eps, 3)?);
    drop(output);
    Ok(out)
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
    let mut out = HostOutput::zeroed(heads * q1 * hw)?;
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
    let mut out = HostOutput::zeroed(n * num_pos_feats * 2)?;
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
    let mut dx = HostOutput::zeroed(q * width)?;
    let mut dy = HostOutput::zeroed(q * height)?;
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
    let mut out = HostOutput::copy(&rp)?;
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
    _x: &GpuTensor, _width: usize, _height: usize, _groups: usize,
    _gamma: &[f32], _beta: &[f32], _eps: f32,
) -> Result<GpuTensor, String> {
    shim::tensor_thread_guard()?;
    Err("metal group_norm has no device implementation".into())
}

/// Resident named weight entries and bytes after retiring dropped owners.
/// Like tensor operations, this query refuses the main thread.
pub fn resident_weights(owner: &WeightOwnerTrace) -> Result<(usize, usize), String> {
    shim::tensor_resident_weights(owner)
}

/// Evict every representation belonging to an owner; return released bytes.
pub fn evict_owner(owner: &WeightOwner) -> Result<usize, String> { shim::tensor_evict_owner(owner) }
/// Evict qualified keys starting with the given key; return released bytes.
pub fn evict_keys(prefix: &WeightKey) -> Result<usize, String> { shim::tensor_evict_keys(prefix) }

pub fn pool_clear() -> Result<(), String> { shim::tensor_pool_clear() }

pub fn alloc_bytes(bytes: usize) -> usize {
    shim::tensor_alloc_bytes(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weight_owners_share_lazily_retire_and_preserve_held_buffers() {
        crate::run_model_worker("weight-owner-test", || {
            use crate::WeightOwner;
            use std::borrow::Cow;
            use std::sync::atomic::{AtomicUsize, Ordering};
            let a = [1.0; 32];
            let w1: Vec<u8> = [1.0f32; 32].iter().flat_map(|v| v.to_le_bytes()).collect();
            let w2: Vec<u8> = [2.0f32; 32].iter().flat_map(|v| v.to_le_bytes()).collect();
            let product = |owner: &WeightOwner, bytes: &[u8]| {
                shim::try_matmul_nt_ggml_bytes_keyed(&a, GGML_TYPE_F32, 4, 8, 4,
                    &owner, "w", || Ok(Cow::Borrowed(bytes))).unwrap()
            };
            let first = WeightOwner::new("reload");
            assert_eq!(product(&first, &w1), [8.0; 16], "first owner uses W1");
            drop(first);
            let second = WeightOwner::new("reload");
            assert_eq!(product(&second, &w2), [16.0; 16], "reused label loads W2 after old owner drops");
            let simultaneous = WeightOwner::new("reload");
            assert_eq!(product(&simultaneous, &w1), [8.0; 16], "two live owners with the same key do not collide");
            assert_eq!(product(&second, &w1), [16.0; 16], "existing second owner's immutable entry survives other owner");

            let shared = WeightOwner::new("shared");
            let calls = AtomicUsize::new(0);
            let lookup = || shim::tensor_named_weight(&shared, "w", || {
                calls.fetch_add(1, Ordering::Relaxed);
                Ok(Cow::Borrowed(w1.as_slice()))
            }).unwrap();
            let first_handle = lookup();
            // Warm once, then verify hits on both threads. Concurrent cold
            // misses may both load outside the lock; insertion keeps one winner.
            std::thread::scope(|scope| {
                for _ in 0..2 {
                    let lookup = &lookup;
                    let first_handle = &first_handle;
                    scope.spawn(move || {
                        for _ in 0..3 {
                            assert_eq!(lookup().test_identity(), first_handle.test_identity(),
                                "threads bind the same stored weight");
                        }
                    });
                }
            });
            assert_eq!(calls.load(Ordering::Relaxed), 1, "warm calls across two threads invoke loader once");
            assert_eq!(evict_owner(&shared).unwrap(), w1.len(), "explicit eviction frees the map retain");
            drop(lookup());
            assert_eq!(calls.load(Ordering::Relaxed), 2, "first lookup after eviction reloads once");

            let held = shim::tensor_named_weight(&second, "w", || panic!("cached weight reloaded")).unwrap();
            std::thread::scope(|scope| {
                let second = &second;
                scope.spawn(move || {
                    assert_eq!(evict_owner(&second).unwrap(), w2.len(), "another thread evicts held weight");
                }).join().unwrap();
            });
            let input = upload(&a, 4, 8).unwrap();
            let out = empty(4, 4, false).unwrap();
            let batch = shim::tensor_matmul(&input.storage.borrow().buffer, 0, &held, 0,
                &out.storage.borrow().buffer, 4, 8, 4, GGML_TYPE_F32).unwrap();
            out.writer_batch.set(batch);
            assert_eq!(download(&out).unwrap(), [16.0; 16], "held weight remains usable after cross-thread eviction");

            let prefix = simultaneous.trace();
            shim::tensor_test_drop_owner_locked(simultaneous).unwrap();
            pool_clear().unwrap();
            assert_eq!(resident_weights(&prefix).unwrap(), (0, 0), "drop inside tensor lock drains without deadlock");
            let prefix = shared.trace();
            std::thread::spawn(move || drop(shared)).join().unwrap();
            pool_clear().unwrap();
            assert_eq!(resident_weights(&prefix).unwrap(), (0, 0), "drop on a thread without context retires weights");
            let fresh = WeightOwner::new("unused");
            assert_eq!(evict_owner(&fresh).unwrap(), 0, "fresh owner has nothing to evict");
            drop((fresh, second, held, first_handle, input, out));
            pool_clear().unwrap();
        });
    }

    #[test]
    fn raw_ggml_matmul_reads_reused_weight_address() {
        crate::run_model_worker("raw-matmul-address-reuse-test", || {
            let (m, k, n) = (4, 8, 4);
            let a = vec![1.0; m * k];
            let mut weight = vec![1.0f32; n * k];
            let ptr = weight.as_ptr();
            fn bytes(values: &[f32]) -> &[u8] {
                unsafe {
                    std::slice::from_raw_parts(values.as_ptr().cast::<u8>(), values.len() * 4)
                }
            }
            let first = shim::try_matmul_nt_ggml_bytes(&a, bytes(&weight), GGML_TYPE_F32, m, k, n)
                .expect("first raw matmul");
            assert_eq!(first, [8.0; 16], "first call uses original contents");
            weight.fill(2.0);
            assert_eq!(weight.as_ptr(), ptr, "weight allocation must be reused");
            let second = shim::try_matmul_nt_ggml_bytes(&a, bytes(&weight), GGML_TYPE_F32, m, k, n)
                .expect("second raw matmul");
            assert_eq!(second, [16.0; 16], "second call uses overwritten contents");
        });
    }

    #[test]
    fn f32_weights_share_host_pages_and_retire() {
        crate::run_model_worker("f32-weight-test", || {
            use makepad_ai_loader::{F32Weight, WeightOwner};
            let (m, k, n) = (4, 32, 8);
            let owner = Arc::new(WeightOwner::new("f32-resident"));
            let a: Vec<f32> = (0..m * k).map(|i| (i as i32 % 23 - 11) as f32 * 0.0317).collect();
            let w = F32Weight::new(&owner, "matrix", n * k, |out| {
                for (i, value) in out.iter_mut().enumerate() {
                    *value = (i as i32 % 19 - 9) as f32 * 0.0731;
                }
                Ok::<_, ()>(())
            }).unwrap();
            let weak = Arc::downgrade(w.storage());
            let legacy = shim::try_matmul_nt_f32(&a, w.as_slice(), m, k, n).unwrap();
            let bits = |values: &[f32]| values.iter().map(|v| v.to_bits()).collect::<Vec<_>>();
            let first = shim::try_matmul_nt_f32_weight(&a, &w, m, k, n).unwrap();
            assert_eq!(bits(&first), bits(&legacy), "resident f32 preserves legacy output bits");
            let buffer = shim::tensor_f32_weight(&w).unwrap();
            assert_eq!(buffer.contents().unwrap().cast::<u8>() as *const u8, w.storage().as_ptr(), "Metal shares the host allocation");
            drop(buffer);
            assert_eq!(resident_weights(&owner.trace()).unwrap().0, 1, "one entry after first call");
            let second = shim::try_matmul_nt_f32_weight(&a, &w, m, k, n).unwrap();
            assert_eq!(bits(&second), bits(&legacy), "reuse preserves output bits");
            assert_eq!(resident_weights(&owner.trace()).unwrap().0, 1, "reuse does not add an entry");
            drop(w);
            drop(owner);
            pool_clear().unwrap();
            assert!(weak.upgrade().is_none(), "retirement releases the shared allocation");
        });
    }

    #[test]
    fn keyed_matmuls_match_resident_and_legacy_bits() {
        crate::run_model_worker("keyed-matmul-test", || {
            use makepad_ai_loader::quant::{f32_to_f16, GGML_TYPE_F16, GGML_TYPE_Q8_0, GGML_TYPE_Q4_0};
            use std::borrow::Cow;
            let (m, k, n) = (4, 32, 8);
            let a: Vec<f32> = (0..m * k).map(|i| (i as i32 % 23 - 11) as f32 * 0.0317).collect();
            let w: Vec<f32> = (0..n * k).map(|i| (i as i32 % 19 - 9) as f32 * 0.0731).collect();
            let mut q8 = Vec::new();
            let mut q4 = Vec::new();
            for row in 0..n {
                q8.extend(f32_to_f16(0.125).to_le_bytes());
                q8.extend((0..32).map(|i| ((row * 7 + i) as i32 % 31 - 15) as i8 as u8));
                q4.extend(f32_to_f16(0.25).to_le_bytes());
                q4.extend((0..16).map(|i| ((row + i) % 16) as u8 | (((row * 3 + i) % 16) as u8) << 4));
            }
            let weights = [
                (GGML_TYPE_F32, w.iter().flat_map(|v| v.to_le_bytes()).collect()),
                (GGML_TYPE_F16, w.iter().flat_map(|v| f32_to_f16(*v).to_le_bytes()).collect()),
                (GGML_TYPE_Q8_0, q8), (GGML_TYPE_Q4_0, q4),
            ];
            let legacy = shim::tensor_test_legacy_matmuls(&a, &weights, m, k, n).unwrap();
            let owner = crate::WeightOwner::new("keyed-bits");
            let keys = ["f32", "f16", "q8_0", "q4_0"];
            let input = upload(&a, m, k).unwrap();
            let bits = |values: &[f32]| values.iter().map(|v| v.to_bits()).collect::<Vec<_>>();
            for (index, (ty, bytes)) in weights.iter().enumerate() {
                let weight = shim::tensor_buffer(bytes.len(), true).unwrap();
                unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), weight.contents().unwrap().cast::<u8>(), bytes.len()); }
                let out = empty(m, n, false).unwrap();
                let batch = shim::tensor_matmul(&input.storage.borrow().buffer, 0, &weight, 0,
                    &out.storage.borrow().buffer, m, k, n, *ty).unwrap();
                out.writer_batch.set(batch);
                let resident = download(&out).unwrap();
                let keyed = shim::try_matmul_nt_ggml_bytes_keyed(&a, *ty, m, k, n,
                    &owner, keys[index], || Ok(Cow::Borrowed(bytes.as_slice()))).unwrap();
                assert_eq!(bits(&keyed), bits(&resident), "{} {m}x{k} by {n}x{k}: keyed equals upload + tensor_matmul", keys[index]);
                assert_eq!(bits(&keyed), bits(&legacy[index]), "{} {m}x{k} by {n}x{k}: keyed equals old matmul_nt_ggml_bytes_impl", keys[index]);
            }
            let matrices: Vec<_> = weights.iter().enumerate().map(|(i, (ty, _))| shim::MatmulNtGgmlBytesKeyedMatrix {
                bt_ggml_type: *ty, n, owner: &owner, cache_key: keys[i],
            }).collect();
            let multi = shim::try_matmul_nt_ggml_bytes_keyed_multi(&a, m, k, &matrices,
                |_, _| panic!("multi must share single's named weights")).unwrap();
            for (index, values) in multi.iter().enumerate() {
                assert_eq!(bits(values), bits(&legacy[index]), "{} multi equals old path", keys[index]);
            }
            assert!(shim::try_matmul_nt_ggml_bytes_keyed(&a[..a.len()-1], GGML_TYPE_F32,
                m, k, n, &owner, "bad", || panic!("invalid input loaded weight")).is_none(), "invalid input still declines");
            assert!(shim::try_matmul_nt_ggml_bytes_keyed_multi(&[], 0, 0, &[],
                |_, _| panic!("empty multi loaded weight")).unwrap().is_empty(), "empty multi still returns empty");
            drop((input, owner));
            pool_clear().unwrap();
        });
    }

    #[test]
    fn addressing_copies_and_model_worker_match_host() {
        let token = String::from("worker value");
        assert_eq!(crate::run_model_worker("copy-test", move || token), "worker value",
            "worker returns the closure's owned value");
        let panic = std::panic::catch_unwind(|| crate::run_model_worker("panic-test", || {
            std::panic::panic_any(73u32);
        })).expect_err("worker re-raises its panic");
        assert_eq!(panic.downcast_ref::<u32>(), Some(&73), "worker preserves panic payload");
        let check = |name: &str, actual: GpuTensor, rows, cols, expected: Vec<f32>| {
            assert_eq!((actual.rows(), actual.cols()), (rows, cols), "{name}: shape");
            assert_eq!(download(&actual).unwrap().iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
                expected.iter().map(|v| v.to_bits()).collect::<Vec<_>>(), "{name}: bit-identical copy");
        };
        // Include signed zero, infinities and NaN payloads: equality of floats
        // would neither prove a copy nor compare the NaNs correctly.
        let values: Vec<f32> = (0..40).map(|i| match i % 8 {
            0 => -0.0, 1 => f32::INFINITY, 2 => f32::NEG_INFINITY,
            3 => f32::from_bits(0x7fc01234), _ => i as f32 - 17.25,
        }).collect();
        for (rows, cols) in [(3, 5), (7, 3)] {
            let parent = upload(&values[..(rows + 1) * cols], rows + 1, cols).unwrap();
            let x = slice_rows(&parent, 1, rows).unwrap();
            assert_ne!(x.storage.borrow().offset % 16, 0, "unaligned row view exercised");
            let host = &values[cols..(rows + 1) * cols];
            check("slice_cols", slice_cols(&x, 1, cols - 1).unwrap(), rows, cols - 1,
                host.chunks_exact(cols).flat_map(|row| row[1..].iter().copied()).collect());
            check("concat_rows", concat_rows(&x, &x).unwrap(), rows * 2, cols,
                host.iter().chain(host).copied().collect());
            let narrow = slice_cols(&x, 1, 1).unwrap();
            check("concat_cols", concat_cols(&[&x, &narrow, &x]).unwrap(), rows, cols * 2 + 1,
                host.chunks_exact(cols).flat_map(|row| row.iter().chain(&row[1..2]).chain(row).copied()).collect());
            let mut expected = vec![0.0; rows * cols];
            for row in 0..rows { for col in 0..cols { expected[col * rows + row] = host[row * cols + col]; } }
            check("tokens_to_planar", tokens_to_planar(&x).unwrap(), cols, rows, expected);
        }
        let (channels, width, height, pw, ph) = (3, 7, 5, 3, 2);
        let pixels: Vec<f32> = (0..(channels + 1) * width * height).map(|i| values[i % values.len()]).collect();
        let image = upload(&pixels, channels + 1, width * height).unwrap();
        let image = slice_rows(&image, 1, channels).unwrap();
        let host = &pixels[width * height..];
        let mut expected = Vec::new();
        for c in 0..channels { for gy in 0..height / ph { for gx in 0..width / pw {
            for py in 0..ph { for px in 0..pw {
                expected.push(host[c * width * height + (gy * ph + py) * width + gx * pw + px]);
            } }
        } } }
        check("image_to_patches with cropped edges", image_to_patches(&image, width, height, pw, ph).unwrap(),
            channels * (height / ph) * (width / pw), pw * ph, expected);
        let (channels, width, height, scale) = (3, 5, 3, 3);
        let count = (channels * scale * scale + 1) * width * height;
        let pixels: Vec<f32> = (0..count).map(|i| values[i % values.len()]).collect();
        let image = upload(&pixels, channels * scale * scale + 1, width * height).unwrap();
        let image = slice_rows(&image, 1, channels * scale * scale).unwrap();
        let host = &pixels[width * height..];
        let mut expected = vec![0.0; channels * width * height * scale * scale];
        for c in 0..channels { for y in 0..height { for x in 0..width {
            for sy in 0..scale { for sx in 0..scale {
                expected[c * width * height * scale * scale + (y * scale + sy) * width * scale + x * scale + sx] =
                    host[(c * scale * scale + sy * scale + sx) * width * height + y * width + x];
            } }
        } } }
        check("pixel_shuffle", pixel_shuffle(&image, width, height, scale).unwrap(), channels,
            width * height * scale * scale, expected);
        let (hold, copies) = residency_profile();
        eprintln!("addressing copies: lock_hold_high_water_ns={hold}, cumulative_cow_copies={copies}");
    }

    #[test]
    fn resident_storage_order_views_threads_and_eviction() {
        // This is one scenario so the device and tiny buffers are prepared once.
        let copies_before = residency_profile().1;
        pool_clear().unwrap();
        let a = upload(&[1.0; 51], 17, 3).expect("reuse input upload");
        let b = upload(&[2.0; 51], 17, 3).expect("reuse second input");
        let c = add(&a, &b).expect("deferred add");
        let identity = a.storage.borrow().buffer.test_identity();
        drop(a);
        let d = mul(&c, &b).expect("ordered reuse multiply");
        assert_eq!(d.storage.borrow().buffer.test_identity(), identity, "multiply reuses the dropped input's device buffer");
        assert_eq!(download(&c).unwrap(), [3.0; 51], "ordering: first output survives reuse");
        assert_eq!(download(&d).unwrap(), [6.0; 51], "ordering: second output");
        let b = upload(&[5.0, 6.0, 7.0, 8.0], 2, 2).unwrap();
        let c = upload(&[6.0, 8.0, 10.0, 12.0], 2, 2).unwrap();
        let pending = add(&c, &b).expect("cross-thread pending output");
        std::thread::spawn(move || {
            assert_eq!(download(&pending).unwrap(), [11.0, 14.0, 17.0, 20.0], "thread B commits A's open batch");
            let result = add(&pending, &pending).expect("GPU operation after moving tensor");
            let host = slice_cols(&result, 1, 1).expect("host operation after GPU");
            assert_eq!(download(&host).unwrap(), [28.0, 40.0], "host op sees GPU writes on new thread");
        }).join().unwrap();

        let parent = upload(&[9.0, 8.0, 7.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0], 3, 3).unwrap();
        let view = slice_rows(&parent, 1, 2).unwrap();
        let copied = upload(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0], 2, 3).unwrap();
        assert_eq!(view.storage.borrow().offset, 12, "view carries an unaligned byte offset");
        assert!(Arc::ptr_eq(&parent.storage.borrow().buffer, &view.storage.borrow().buffer), "row slice shares storage");
        assert_eq!(download(&add(&view, &copied).unwrap()).unwrap(), download(&add(&copied, &copied).unwrap()).unwrap(), "view add matches copied slice");
        let weights = upload(&[1.0, 0.5, -1.0, 0.25, 2.0, 1.0], 2, 3).unwrap();
        assert_eq!(download(&linear_f32_resident(&view, &weights, None).unwrap()).unwrap(),
            download(&linear_f32_resident(&copied, &weights, None).unwrap()).unwrap(), "view linear matches copied slice");
        upload_into(&parent, &[0.0; 9]).unwrap();
        assert_eq!(download(&parent).unwrap(), [0.0; 9], "COW parent changes");
        assert_eq!(download(&view).unwrap(), [1.0, 2.0, 3.0, 4.0, 5.0, 6.0], "COW preserves view");
        std::thread::scope(|scope| {
            scope.spawn(move || assert_eq!(download(&view).unwrap(), [1.0, 2.0, 3.0, 4.0, 5.0, 6.0], "alias reads safely on second thread"));
            upload_into(&parent, &[2.0; 9]).unwrap();
        });
        let guard = data(&parent).unwrap();
        assert!(upload_into(&parent, &[3.0; 9]).is_err(), "host read guard excludes an in-place write");
        drop(guard);

        let pending = add(&b, &b).unwrap();
        std::thread::spawn(|| pool_clear().expect("pool clear with another thread's open batch")).join().unwrap();
        assert_eq!(download(&pending).unwrap(), [10.0, 12.0, 14.0, 16.0], "pool clear preserves pending work");
        shim::tensor_test_error_and_guard().expect("error-path drop, UI guard, and batch failure recovery");

        let ns = WeightOwner::new("resident-eviction");
        let bytes: Vec<u8> = [1.0f32, 2.0, 3.0, 4.0].iter().flat_map(|v| v.to_le_bytes()).collect();
        let parts = [GpuLinearPart { bt_ggml_type: GGML_TYPE_F32, n: 2, cache_key: "weight", bytes: &bytes }];
        let result = linear_nt(&b, &ns, &parts, &[]).unwrap();
        download(&result).unwrap();
        assert_eq!(evict_owner(&ns).unwrap(), bytes.len(), "eviction returns named weight bytes");
        let calls = std::cell::Cell::new(0);
        let weight = shim::tensor_named_weight(&ns, "weight", || { calls.set(calls.get() + 1); Ok(std::borrow::Cow::Owned(bytes.clone())) }).unwrap();
        assert_eq!(calls.get(), 1, "evicted key invokes loader again");
        drop(weight);
        assert_eq!(evict_owner(&ns).unwrap(), bytes.len(), "reloaded key evicts again");
        let values: Vec<f32> = (0..96).map(|i| (i as f32 - 48.0) / 64.0).collect();
        let q_parent = upload(&values, 3, 32).unwrap();
        let q = slice_rows(&q_parent, 1, 2).unwrap();
        let expected = shim::try_flash_attn_f32_packed(&values[32..], &values, &values, 2, 3, 1, 32, 0.125).unwrap();
        let actual = attention_packed(&q, &q_parent, &q_parent, 1, 0.125).unwrap();
        assert_eq!(download(&actual).unwrap(), expected, "attention on a row-view operand matches the host-slice entry");
        let mods = upload(&[99.0, 1.0, 2.0, 3.0, 4.0, -1.0, -2.0, -3.0, -4.0], 1, 9).unwrap();
        let norm_x = upload(&values[..8], 2, 4).unwrap();
        let expected = shim::try_layer_norm_mul_add_f32(&values[..8], &[2, 4], &[1.0, 2.0, 3.0, 4.0], &[4],
            &[-1.0, -2.0, -3.0, -4.0], &[4], 1e-5).unwrap();
        let actual = download(&layer_norm_mod(&norm_x, &mods, 1, 5, 1e-5).unwrap()).unwrap();
        for (index, (got, expected)) in actual.iter().zip(&expected).enumerate() {
            assert!((got - expected).abs() < 1e-5, "unaligned norm affine element {index}: {got} != {expected}");
        }
        let expected = shim::try_gelu_f32(&values[..8], &[2, 4]).unwrap();
        assert_eq!(download(&gelu(&norm_x).unwrap()).unwrap(), expected, "resident GELU uses existing arithmetic");
        let activated = download(&silu(&norm_x).unwrap()).unwrap();
        for (&got, &value) in activated.iter().zip(&values[..8]) {
            assert!((got - value / (1.0 + (-value).exp())).abs() < 1e-6, "resident SiLU arithmetic");
        }
        assert_eq!(download(&relu(&norm_x).unwrap()).unwrap(), vec![0.0; 8], "resident ReLU arithmetic");
        let image = upload(&values[..27], 3, 9).unwrap();
        let image = slice_rows(&image, 1, 2).unwrap();
        let weights: Vec<f32> = (0..36).map(|i| i as f32 / 32.0).collect();
        let expected = shim::try_conv2d_planar_f32(&values[9..27], 3, 3, 2, &weights, &[0.25, -0.5], 2, 3, 3, 1, 1).unwrap();
        let actual = conv2d_planar(&image, 3, 3, &weights, &[0.25, -0.5], 2, 3, 3, 1, 1).unwrap();
        assert_eq!(download(&actual).unwrap(), expected, "convolution of a row-view image matches the host-slice entry");
        let (hold, copies) = residency_profile();
        assert_eq!(copies - copies_before, 1, "only the deliberate shared-view write copies");
        eprintln!("tensor residency: lock_hold_high_water_ns={hold}, scenario_cow_copies={}", copies - copies_before);
    }

    #[test]
    fn linear_nt_keyed_parts_match_host() {
        use makepad_ai_loader::quant::{f16_to_f32, f32_to_f16, GGML_TYPE_BF16, GGML_TYPE_F16};

        let values = [1.0, -0.5, 2.0, 0.25, -1.0, 3.0, 0.5, 2.0, 0.75, 1.5, -2.0, 1.0];
        let x = upload(&values, 3, 4).expect("linear input upload");
        let weights = [
            0.1f32, 0.2, -0.3, 0.4, 1.25, -0.75, 0.6, 0.8,
            -0.9, 0.3, 1.1, -0.2, 0.5, -1.5, 0.7, 0.1, 1.2, 0.4, -0.6, 0.9,
        ];
        let bias = [0.25, -0.5, 0.75, 1.0, -0.25];
        let words: Vec<u16> = weights.iter().map(|v| {
            let bits = v.to_bits();
            ((bits + 0x7fff + ((bits >> 16) & 1)) >> 16) as u16
        }).collect();
        let rounded: Vec<f32> = words.iter().map(|&w| f32::from_bits(u32::from(w) << 16)).collect();
        let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        let owner = WeightOwner::new("linear_nt_test_bf16");
        let mut parts = [
            GpuLinearPart { bt_ggml_type: GGML_TYPE_BF16, n: 2, cache_key: "first", bytes: &bytes[..16] },
            GpuLinearPart { bt_ggml_type: GGML_TYPE_BF16, n: 3, cache_key: "second", bytes: &bytes[16..] },
        ];
        let check = |case: &str, output: GpuTensor, weights: &[f32], bias: &[f32]| {
            assert_eq!((output.rows(), output.cols()), (3, 5), "{case}: output shape");
            let actual = download(&output).unwrap_or_else(|err| panic!("{case}: {err}"));
            for r in 0..3 {
                for c in 0..5 {
                    let expected = (0..4).map(|k| values[r * 4 + k] * weights[c * 4 + k]).sum::<f32>()
                        + bias.get(c).copied().unwrap_or(0.0);
                    let got = actual[r * 5 + c];
                    assert!((got - expected).abs() <= 1e-5 * expected.abs(),
                        "{case}: row {r}, col {c}: {got} != {expected}");
                }
            }
        };
        check("bf16 first call with bias", linear_nt(&x, &owner, &parts, &bias)
            .expect("bf16 first call with bias"), &rounded, &bias);
        // Changed host bytes must not replace weights already cached under these keys.
        let zeros = [0u8; 40];
        parts[0].bytes = &zeros[..16];
        parts[1].bytes = &zeros[16..];
        check("bf16 cached call with bias", linear_nt(&x, &owner, &parts, &bias)
            .expect("bf16 cached call with bias"), &rounded, &bias);

        let f32_bytes: Vec<u8> = weights[..8].iter().flat_map(|w| w.to_le_bytes()).collect();
        let f16_words: Vec<u16> = weights[8..].iter().map(|&w| f32_to_f16(w)).collect();
        let f16_bytes: Vec<u8> = f16_words.iter().flat_map(|w| w.to_le_bytes()).collect();
        let mut mixed_weights = weights[..8].to_vec();
        mixed_weights.extend(f16_words.iter().map(|&w| f16_to_f32(w)));
        let mixed_parts = [
            GpuLinearPart { bt_ggml_type: GGML_TYPE_F32, n: 2, cache_key: "first", bytes: &f32_bytes },
            GpuLinearPart { bt_ggml_type: GGML_TYPE_F16, n: 3, cache_key: "second", bytes: &f16_bytes },
        ];
        let mixed_namespace = WeightOwner::new("linear_nt_test_mixed");
        check("f32 and f16 separate namespace without bias",
            linear_nt(&x, &mixed_namespace, &mixed_parts, &[]).expect("f32 and f16 call"),
            &mixed_weights, &[]);
    }

    #[test]
    fn gate_pool_and_bias_match_cuda_formulas() {
        for (cols, offset, values) in [
            (4, 0, vec![-2.0f32, 0.5, 3.0, -4.0, 1.0, -0.25, 2.0, 0.75]),
            (6, 2, vec![9.0f32, 8.0, -2.0, 0.5, 3.0, -4.0, 7.0, 6.0, 1.0, -0.25, 2.0, 0.75]),
        ] {
            let x = upload(&values, 2, cols).unwrap();
            let expected: Vec<f32> = values.chunks_exact(cols).flat_map(|row| {
                (0..2).map(move |c| {
                    let gate = row[offset + c];
                    let s = gate / (1.0f32 + (-gate).exp());
                    row[offset + 2 + c] * s
                })
            }).collect();
            let out = swiglu_gate_first(&x, offset, 2).unwrap();
            assert_eq!((out.rows(), out.cols()), (2, 2), "SwiGLU {cols} cols: shape");
            assert_eq!(download(&out).unwrap(), expected, "SwiGLU {cols} cols: gate order and arithmetic");
            let expected: Vec<f32> = values.chunks_exact(cols)
                .map(|row| row.iter().copied().sum::<f32>() / cols as f32).collect();
            let out = global_avg_pool(&x).unwrap();
            assert_eq!((out.rows(), out.cols()), (2, 1), "pool {cols} cols: shape");
            assert_eq!(download(&out).unwrap(), expected, "pool {cols} cols: f32 mean");
            let bias = [0.25, -0.5];
            for slope in [0.1f32, 1.0] {
                upload_into(&x, &values).unwrap();
                let id = x.id.get();
                let expected: Vec<f32> = values.iter().enumerate().map(|(i, v)| {
                    let value = v + bias[i / cols];
                    if value > 0.0 { value } else { value * slope }
                }).collect();
                bias_lrelu_inplace(&x, &bias, slope).unwrap();
                assert_eq!(download(&x).unwrap(), expected, "bias {cols} cols, slope {slope}: in-place arithmetic");
                assert_ne!(x.id.get(), id, "bias {cols} cols, slope {slope}: invalidate cached contents");
            }
        }
    }
}
