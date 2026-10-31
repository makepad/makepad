//! Whisper on Vulkan. The complete encoder and decoder entries keep their
//! activations resident across all layers; the smaller `try_*` entries also
//! implement the existing accelerator seam. Weights belong to a model identity,
//! KV buffers to a decoder-session identity, never to a sampled host pointer.
//!
//! f16 weights remain f16; activations, attention and accumulators are f32.
//! Q8_0/Q5_K/Q4_K use the shared integer-dot kernels and int8 activations;
//! weights stay quantised on the GPU. Calls run
//! on the existing transcription worker. Thread-local streams share the
//! process-wide device; only queue submission takes its lock. No GPU wait or
//! work is added to the UI/audio threads.

#[path = "ops.rs"]
mod ops;
#[cfg(test)]
#[path = "tests.rs"]
mod tests;

use crate::whisper::decoder::KvCache;
use crate::whisper::model::{DecoderLayer, EncoderLayer, WhisperModel};
use crate::whisper::quant::{GGML_TYPE_F16, GGML_TYPE_F32};
use crate::whisper::tensor::Tensor;
use makepad_ai_vulkan::{Buffer, BufferKind, Result};
use ops::*;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Weak};

pub(crate) fn is_selected() -> bool {
    #[cfg(test)]
    if TEST_VULKAN.with(|v| v.get()) { return true; }
    match std::env::var("MAKEPAD_WHISPER_BACKEND").as_deref() {
        Ok("vulkan") => true,
        Ok(_) => false,
        Err(_) => cfg!(target_os = "android"),
    }
}

enum Runtime {
    New,
    Ready(Box<Context>),
    Failed,
}

thread_local! {
    static RUNTIME: RefCell<Runtime> = const { RefCell::new(Runtime::New) };
    #[cfg(test)]
    static TEST_VULKAN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn run<T>(what: &str, f: impl FnOnce(&mut Context) -> Result<T>) -> Option<T> {
    if !is_selected() { return None; }
    RUNTIME.with(|state| {
        let mut state = state.try_borrow_mut().ok()?;
        if matches!(*state, Runtime::New) {
            match Context::new() {
                Ok(ctx) => *state = Runtime::Ready(Box::new(ctx)),
                Err(e) => {
                    eprintln!("[voice] Vulkan unavailable: {e}; using CPU");
                    *state = Runtime::Failed;
                }
            }
        }
        let Runtime::Ready(ctx) = &mut *state else { return None };
        match f(ctx) {
            Ok(out) => Some(out),
            Err(e) => {
                eprintln!("[voice] Vulkan disabled after {what}: {e}; using CPU");
                *state = Runtime::Failed;
                None
            }
        }
    })
}

pub(crate) fn is_available() -> bool { run("initialization", |_| Ok(())).is_some() }

pub(crate) fn clear_decoder_kv_cache() {
    // Clear even when selection changed. Do not initialize a device here.
    RUNTIME.with(|s| {
        if let Ok(mut s) = s.try_borrow_mut() {
            if let Runtime::Ready(ctx) = &mut *s { ctx.clear_session(); }
        }
    });
}

struct DeviceKv {
    k: Buffer,
    v: Buffer,
    capacity: usize,
    rows: usize,
}

struct CrossKv {
    k: Buffer,
    v: Buffer,
    rows: usize,
    host: (usize, usize),
}

struct Context {
    model: Option<Rc<ModelWeights>>,
    model_owner: Weak<()>,
    session_owner: Weak<()>,
    self_kv: Vec<DeviceKv>,
    cross_kv: Vec<CrossKv>,
    // Release the shared device reference after its buffers and pipelines.
    ops: Ops,
}

impl Context {
    fn new() -> Result<Self> {
        Ok(Self { model: None, model_owner: Weak::new(), session_owner: Weak::new(),
            self_kv: Vec::new(), cross_kv: Vec::new(), ops: Ops::new()? })
    }

    fn clear_session(&mut self) {
        self.self_kv.clear();
        self.cross_kv.clear();
        self.session_owner = Weak::new();
    }

    fn model(&mut self, model: &WhisperModel) -> Result<Rc<ModelWeights>> {
        if !self.model_owner.upgrade().is_some_and(|id| Arc::ptr_eq(&id, &model.vulkan_id)) {
            self.clear_session();
            self.model = None;
            let start = std::time::Instant::now();
            self.model = Some(Rc::new(ModelWeights::new(&self.ops.device, model, self.ops.int8_encoder)?));
            self.model_owner = Arc::downgrade(&model.vulkan_id);
            // The GPU holds the weights now: the mapped file pages go.
            model.release_mapped_weights();
            eprintln!("[voice] Vulkan weights resident in {:.3}s", start.elapsed().as_secs_f64());
            let sizes = self.model.as_ref().unwrap().resident_bytes();
            eprintln!("[voice] Vulkan weight buffers: {} bytes {sizes:?}", sizes.values().sum::<u64>());
        }
        Ok(self.model.as_ref().unwrap().clone())
    }

    fn session(&mut self, cache: &KvCache, cross: &[(Tensor, Tensor)], width: usize, new_rows: usize) -> Result<()> {
        let needed = cache.n_past.checked_add(new_rows).ok_or("Whisper KV size overflow")?;
        count(needed, width)?;
        if !self.session_owner.upgrade().is_some_and(|id| Arc::ptr_eq(&id, &cache.vulkan_id)) {
            self.clear_session();
            self.session_owner = Arc::downgrade(&cache.vulkan_id);
        }
        for (i, (key, val)) in cross.iter().enumerate() {
            if cache.k[i].len() != cache.n_past * width || cache.v[i].len() != cache.n_past * width {
                return Err("Whisper host KV position mismatch".into());
            }
            if i == self.self_kv.len() || self.self_kv[i].capacity < needed || self.self_kv[i].rows != cache.n_past {
                // Geometric growth is a capacity policy, not a token limit.
                // The authoritative host prefix also restores a cache after
                // a CPU-only alignment step or a switch of decoder sessions.
                let old_capacity = self.self_kv.get(i).map_or(0, |kv| kv.capacity);
                let capacity = needed.max(old_capacity.saturating_mul(2));
                let size = count(capacity, width)? as u64 * 4;
                let k = self.ops.device.buffer(size, BufferKind::Device)?;
                let v = self.ops.device.buffer(size, BufferKind::Device)?;
                if cache.n_past != 0 {
                    self.ops.device.upload(&k, 0, bytes(&cache.k[i]))?;
                    self.ops.device.upload(&v, 0, bytes(&cache.v[i]))?;
                }
                let entry = DeviceKv { k, v, capacity, rows: cache.n_past };
                if i == self.self_kv.len() { self.self_kv.push(entry); } else { self.self_kv[i] = entry; }
            }
            let host = (key.data.as_ptr() as usize, val.data.as_ptr() as usize);
            let rows = key.data.len() / width;
            if rows == 0 || key.data.len() != rows * width || val.data.len() != key.data.len() {
                return Err("Whisper cross-attention shape mismatch".into());
            }
            if i == self.cross_kv.len() || self.cross_kv[i].host != host || self.cross_kv[i].rows != rows {
                // Cross tensors are immutable for a decoder session. The
                // owning session identity prevents reuse after allocation ABA.
                let entry = CrossKv { k: upload(&self.ops.device, &key.data)?, v: upload(&self.ops.device, &val.data)?, rows, host };
                if i == self.cross_kv.len() { self.cross_kv.push(entry); } else { self.cross_kv[i] = entry; }
            }
        }
        Ok(())
    }
}

fn head_shape(width: usize, heads: usize) -> bool {
    heads > 0 && width.checked_div(heads) == Some(64) && width % heads == 0
}

/// Complete front end, positional addition, encoder stack and final norm.
pub(crate) fn try_encode(model: &WhisperModel, mel: &[f32], frames: usize) -> Option<Vec<f32>> {
    let width = model.hparams.n_audio_state as usize;
    let heads = model.hparams.n_audio_head as usize;
    let mels = model.hparams.n_mels as usize;
    if !head_shape(width, heads) || frames == 0 || frames > model.hparams.n_audio_ctx as usize
        || mel.len() != frames.checked_mul(2)?.checked_mul(mels)? { return None; }
    run("encoder", |ctx| {
        let w = ctx.model(model)?;
        let mut plan = Layout::default();
        let input = plan.take(mel.len())?;
        let col1 = plan.take(count(frames * 2, mels * 3)?)?;
        let conv1 = plan.take(count(frames * 2, width)?)?;
        let col2 = plan.take(count(frames, width * 3)?)?;
        let x = plan.take(count(frames, width)?)?;
        let s = Scratch::new(&mut plan, frames, width)?;
        let op = &mut ctx.ops;
        op.reserve(&plan)?;
        op.write(input, mel)?;
        op.begin()?;
        op.im2col(input, col1, mels, frames * 2, 3, 1, 1, false);
        op.linear(col1, conv1, frames * 2, &w.conv1)?;
        op.gelu(conv1);
        op.im2col(conv1, col2, width, frames * 2, 3, 2, 1, true);
        op.linear(col2, x, frames, &w.conv2)?;
        op.gelu(x);
        op.element(op.addr(x), w.pos.address, op.addr(x), x.len, x.len, 0);
        for l in &w.enc { op.encoder_layer(x, &s, frames, heads, l)?; }
        op.norm(x, s.norm, width, Some(&w.enc_norm), 1e-5);
        op.finish()?;
        op.read(s.norm)
    })
}

/// Cross projections use the already resident model weights. The host copies
/// satisfy the CPU/alignment contract; decode uploads each pair once per session.
pub(crate) fn try_cross_kv(model: &WhisperModel, input: &Tensor) -> Option<Vec<(Tensor, Tensor)>> {
    if input.shape.len() != 2 || input.shape[1] != model.hparams.n_audio_state as usize { return None; }
    let rows = input.shape[0];
    let width = input.shape[1];
    if input.data.len() != rows.checked_mul(width)? || rows == 0 { return None; }
    run("cross projections", |ctx| {
        let w = ctx.model(model)?;
        let mut plan = Layout::default();
        let x = plan.take(input.data.len())?;
        let out = (0..w.dec.len()).map(|_| Ok((plan.take(x.len)?, plan.take(x.len)?))).collect::<Result<Vec<_>>>()?;
        let op = &mut ctx.ops;
        op.reserve(&plan)?;
        op.write(x, &input.data)?;
        op.begin()?;
        for (l, &(k, v)) in w.dec.iter().zip(&out) {
            op.linear(x, k, rows, &l.cross.k)?;
            op.linear(x, v, rows, &l.cross.v)?;
        }
        op.finish()?;
        out.into_iter().map(|(k, v)| Ok((Tensor { data: op.read(k)?, shape: vec![rows, width] },
            Tensor { data: op.read(v)?, shape: vec![rows, width] }))).collect()
    })
}

/// All decoder layers, causal prompt/decode attention and vocabulary logits in
/// one submission. Host KV is committed only after every readback succeeds;
/// returning None leaves the original CPU decoder a complete, unmodified prefix.
pub(crate) fn try_decode(model: &WhisperModel, tokens: &[i32], positions: &[i32], cache: &mut KvCache, cross: &[(Tensor, Tensor)]) -> Option<Vec<f32>> {
    let rows = tokens.len();
    let width = model.hparams.n_text_state as usize;
    let heads = model.hparams.n_text_head as usize;
    let vocab = model.hparams.n_vocab as usize;
    if !head_shape(width, heads) || rows == 0 || positions.len() != rows
        || cross.len() != model.decoder_layers.len() || cache.k.len() != cross.len() || cache.v.len() != cross.len()
        || tokens.iter().any(|&t| t < 0 || t as usize >= vocab)
        || positions.iter().any(|&p| p < 0 || p as usize >= model.hparams.n_text_ctx as usize) { return None; }
    run("decoder step", |ctx| {
        let w = ctx.model(model)?;
        ctx.session(cache, cross, width, rows)?;
        let mut plan = Layout::default();
        let x = plan.take(count(rows, width)?)?;
        let s = Scratch::new(&mut plan, rows, width)?;
        let logits = plan.take(count(rows, vocab)?)?;
        let new_kv = plan.take(count(count(rows, width)?, w.dec.len() * 2)?)?;
        let mut embedding = Vec::with_capacity(x.len);
        for (&token, &position) in tokens.iter().zip(positions) {
            for d in 0..width {
                embedding.push(model.d_te.data[token as usize * width + d] + model.d_pe.data[position as usize * width + d]);
            }
        }
        let op = &mut ctx.ops;
        op.reserve(&plan)?;
        op.write(x, &embedding)?;
        op.begin()?;
        let nk = cache.n_past + rows;
        for (i, l) in w.dec.iter().enumerate() {
            let kv = &ctx.self_kv[i];
            let cross = &ctx.cross_kv[i];
            op.qkv(x, &s, rows, &l.self_attn)?;
            let size = (rows * width * 4) as u64;
            let offset = (cache.n_past * width * 4) as u64;
            op.stream.copy(&op.arena, s.k.offset, &kv.k, offset, size);
            op.stream.copy(&op.arena, s.v.offset, &kv.v, offset, size);
            op.stream.copy(&op.arena, s.k.offset, &op.arena, new_kv.offset + i as u64 * 2 * size, size);
            op.stream.copy(&op.arena, s.v.offset, &op.arena, new_kv.offset + (i as u64 * 2 + 1) * size, size);
            op.stream.barrier();
            op.attention(op.addr(s.q), kv.k.address, kv.v.address, op.addr(s.att), rows, nk, heads, Some(cache.n_past), 0.125);
            op.linear(s.att, s.proj, rows, &l.self_attn.out)?;
            op.add(s.proj, x, x);
            op.cross_ffn(x, &s, rows, heads, &cross.k, &cross.v, cross.rows, l)?;
        }
        op.norm(x, s.norm, width, Some(&w.dec_norm), 1e-5);
        op.matmul(op.addr(s.norm), op.addr(logits), rows, &w.logits, None)?;
        op.finish()?;
        let result = op.read(logits)?;
        let appended = op.read(new_kv)?;
        let len = rows * width;
        for (i, kv) in ctx.self_kv.iter_mut().enumerate() {
            cache.k[i].extend_from_slice(&appended[i * 2 * len..(i * 2 + 1) * len]);
            cache.v[i].extend_from_slice(&appended[(i * 2 + 1) * len..(i * 2 + 2) * len]);
            kv.rows = nk;
        }
        // The existing caller advances cache.n_past, exactly as on the CPU.
        Ok(result)
    })
}

pub(crate) fn try_matmul_nt_ggml_bytes_add_bias(a: &[f32], w: &[u8], ty: u32, m: usize, k: usize, n: usize, bias: &[f32]) -> Option<Vec<f32>> {
    matmul(a, w, ty, m, k, n, Some(bias))
}

pub(crate) fn try_matmul_nt_ggml_bytes(a: &[f32], w: &[u8], ty: u32, m: usize, k: usize, n: usize) -> Option<Vec<f32>> {
    matmul(a, w, ty, m, k, n, None)
}

pub(crate) fn try_matmul_nt_f16_bytes(a: &[f32], w: &[u8], m: usize, k: usize, n: usize) -> Option<Vec<f32>> {
    matmul(a, w, GGML_TYPE_F16, m, k, n, None)
}

pub(crate) fn try_matmul_nt_f32_bytes(a: &[f32], w: &[u8], m: usize, k: usize, n: usize) -> Option<Vec<f32>> {
    matmul(a, w, GGML_TYPE_F32, m, k, n, None)
}

pub(crate) fn try_matmul_nt_f32(a: &[f32], w: &[f32], m: usize, k: usize, n: usize) -> Option<Vec<f32>> {
    matmul(a, bytes(w), GGML_TYPE_F32, m, k, n, None)
}

pub(crate) fn try_matmul_nn_f32(a: &[f32], b: &[f32], m: usize, k: usize, n: usize) -> Option<Vec<f32>> {
    if !is_selected() || b.len() != k.checked_mul(n)? { return None; }
    let mut wt = vec![0.0; b.len()];
    for i in 0..k { for j in 0..n { wt[j * k + i] = b[i * n + j]; } }
    try_matmul_nt_f32(a, &wt, m, k, n)
}

fn matmul(a: &[f32], w: &[u8], ty: u32, m: usize, k: usize, n: usize, bias: Option<&[f32]>) -> Option<Vec<f32>> {
    if m == 0 || k == 0 || n == 0 || a.len() != m.checked_mul(k)? || bias.is_some_and(|b| b.len() != n) { return None; }
    run("matrix multiply", |ctx| {
        let op = &mut ctx.ops;
        let w = Linear::raw(&op.device, w, ty, n, k, bias)?;
        let mut plan = Layout::default();
        let x = plan.take(a.len())?;
        let out = plan.take(count(m, n)?)?;
        op.reserve(&plan)?;
        op.write(x, a)?;
        op.begin()?;
        op.linear(x, out, m, &w)?;
        op.finish()?;
        op.read(out)
    })
}

fn valid_shape(data: &[f32], shape: &[usize]) -> bool {
    !shape.is_empty() && !shape.contains(&0) && shape.iter().try_fold(1usize, |n, &v| n.checked_mul(v)) == Some(data.len())
}

fn element(a: &[f32], ashape: &[usize], b: &[f32], bshape: &[usize], kind: u32) -> Option<Vec<f32>> {
    if !valid_shape(a, ashape) || !valid_shape(b, bshape) || bshape.len() > ashape.len() { return None; }
    // The common scalar, suffix-row and equal-shape broadcasts. Other
    // broadcasts use the established CPU implementation.
    let tail = &ashape[ashape.len() - bshape.len()..];
    if b.len() != 1 && bshape != tail { return None; }
    run("elementwise", |ctx| {
        let op = &mut ctx.ops;
        let mut plan = Layout::default();
        let x = plan.take(a.len())?;
        let y = plan.take(b.len())?;
        op.reserve(&plan)?;
        op.write(x, a)?;
        op.write(y, b)?;
        op.begin()?;
        op.element(op.addr(x), op.addr(y), op.addr(x), x.len, y.len, kind);
        op.finish()?;
        op.read(x)
    })
}

pub(crate) fn try_add_f32(a: &[f32], ashape: &[usize], b: &[f32], bshape: &[usize]) -> Option<Vec<f32>> { element(a, ashape, b, bshape, 0) }
pub(crate) fn try_mul_f32(a: &[f32], ashape: &[usize], b: &[f32], bshape: &[usize]) -> Option<Vec<f32>> { element(a, ashape, b, bshape, 1) }
pub(crate) fn try_gelu_f32(a: &[f32], shape: &[usize]) -> Option<Vec<f32>> { element(a, shape, &[0.0], &[1], 2) }

pub(crate) fn try_layer_norm_f32(x: &[f32], shape: &[usize], eps: f32) -> Option<Vec<f32>> {
    norm(x, shape, None, eps)
}

pub(crate) fn try_layer_norm_mul_add_f32(x: &[f32], shape: &[usize], mul: &[f32], mul_shape: &[usize], add: &[f32], add_shape: &[usize], eps: f32) -> Option<Vec<f32>> {
    let &cols = shape.last()?;
    if !valid_shape(mul, mul_shape) || !valid_shape(add, add_shape) || mul.len() != cols || add.len() != cols { return None; }
    norm(x, shape, Some((mul, add)), eps)
}

fn norm(x: &[f32], shape: &[usize], affine: Option<(&[f32], &[f32])>, eps: f32) -> Option<Vec<f32>> {
    if !valid_shape(x, shape) || !eps.is_finite() || eps <= 0.0 { return None; }
    let cols = *shape.last()?;
    run("layer norm", |ctx| {
        let op = &mut ctx.ops;
        let norm = affine.map(|(w, b)| Norm::new(&op.device, w, b)).transpose()?;
        let mut plan = Layout::default();
        let input = plan.take(x.len())?;
        let out = plan.take(x.len())?;
        op.reserve(&plan)?;
        op.write(input, x)?;
        op.begin()?;
        op.norm(input, out, cols, norm.as_ref(), eps);
        op.finish()?;
        op.read(out)
    })
}

pub(crate) fn try_im2col_1d_f32(input: &[f32], ic: usize, iw: usize, kw: usize, stride: usize, pad: usize) -> Option<Vec<f32>> {
    if ic == 0 || iw == 0 || kw == 0 || stride == 0 || input.len() != ic.checked_mul(iw)? { return None; }
    let extent = iw.checked_add(pad.checked_mul(2)?)?;
    if extent < kw { return Some(Vec::new()); }
    let ow = (extent - kw) / stride + 1;
    run("convolution packing", |ctx| {
        let op = &mut ctx.ops;
        let mut plan = Layout::default();
        let x = plan.take(input.len())?;
        let out = plan.take(count(ow, count(ic, kw)?)?)?;
        op.reserve(&plan)?;
        op.write(x, input)?;
        op.begin()?;
        op.im2col(x, out, ic, iw, kw, stride, pad, false);
        op.finish()?;
        op.read(out)
    })
}

pub(crate) fn try_flash_attn_f32_packed(q: &[f32], k: &[f32], v: &[f32], nq: usize, nk: usize, heads: usize, d: usize, scale: f32) -> Option<Vec<f32>> {
    if d != 64 || nq == 0 || nk == 0 || heads == 0 || !scale.is_finite()
        || q.len() != nq.checked_mul(heads)?.checked_mul(d)?
        || k.len() != nk.checked_mul(heads)?.checked_mul(d)? || k.len() != v.len() { return None; }
    run("attention", |ctx| {
        let op = &mut ctx.ops;
        let mut plan = Layout::default();
        let query = plan.take(q.len())?;
        let key = plan.take(k.len())?;
        let val = plan.take(v.len())?;
        let out = plan.take(q.len())?;
        op.reserve(&plan)?;
        op.write(query, q)?;
        op.write(key, k)?;
        op.write(val, v)?;
        op.begin()?;
        op.attention(op.addr(query), op.addr(key), op.addr(val), op.addr(out), nq, nk, heads, None, scale);
        op.finish()?;
        op.read(out)
    })
}

pub(crate) fn try_flash_attn_f32_self_kv_cache(_layer: usize, q: &[f32], k: &[f32], v: &[f32], nk: usize, heads: usize, d: usize, scale: f32) -> Option<Vec<f32>> {
    try_flash_attn_f32_packed(q, k, v, 1, nk, heads, d, scale)
}

pub(crate) fn try_flash_attn_f32_cross_kv_cache(_layer: usize, q: &[f32], k: &[f32], v: &[f32], nq: usize, padded_nk: usize, heads: usize, d: usize, scale: f32) -> Option<Vec<f32>> {
    let width = heads.checked_mul(d)?;
    if width == 0 || k.len() % width != 0 { return None; }
    let nk = k.len() / width;
    if nk > padded_nk { return None; }
    // The old seam passes a padded capacity; attend only to real rows.
    try_flash_attn_f32_packed(q, k, v, nq, nk, heads, d, scale)
}

fn attention_weights(device: &makepad_ai_vulkan::Device, width: usize, ln_w: &[f32], ln_b: &[f32], q: &[u8], qt: u32, qb: &[f32], k: &[u8], kt: u32, v: &[u8], vt: u32, vb: &[f32], out: &[u8], ot: u32, ob: &[f32]) -> Result<Attention> {
    if ln_w.len() != width || ln_b.len() != width { return Err("Whisper attention norm width mismatch".into()); }
    Ok(Attention {
        norm: Norm::new(device, ln_w, ln_b)?,
        q: Linear::raw(device, q, qt, width, width, Some(qb))?,
        k: Linear::raw(device, k, kt, width, width, None)?,
        v: Linear::raw(device, v, vt, width, width, Some(vb))?,
        out: Linear::raw(device, out, ot, width, width, Some(ob))?,
    })
}

fn ffn_weights(device: &makepad_ai_vulkan::Device, width: usize, ln_w: &[f32], ln_b: &[f32], up: &[u8], ut: u32, ub: &[f32], down: &[u8], dt: u32, db: &[f32]) -> Result<Ffn> {
    if ln_w.len() != width || ln_b.len() != width { return Err("Whisper FFN norm width mismatch".into()); }
    Ok(Ffn { norm: Norm::new(device, ln_w, ln_b)?,
        up: Linear::raw(device, up, ut, count(width, 4)?, width, Some(ub))?,
        down: Linear::raw(device, down, dt, width, count(width, 4)?, Some(db))? })
}

fn block_input(op: &mut Ops, values: &[f32], rows: usize, width: usize) -> Result<(Span, Scratch)> {
    if values.len() != count(rows, width)? { return Err("Whisper block shape mismatch".into()); }
    let mut plan = Layout::default();
    let x = plan.take(values.len())?;
    let s = Scratch::new(&mut plan, rows, width)?;
    op.reserve(&plan)?;
    op.write(x, values)?;
    Ok((x, s))
}

pub(crate) fn try_encoder_attn_block_f32(x: &[f32], rows: usize, width: usize, heads: usize,
    ln_w: &[f32], ln_b: &[f32], q: &[u8], qt: u32, qb: &[f32], k: &[u8], kt: u32,
    v: &[u8], vt: u32, vb: &[f32], out: &[u8], ot: u32, ob: &[f32]) -> Option<Vec<f32>> {
    if !head_shape(width, heads) { return None; }
    run("encoder attention block", |ctx| {
        let op = &mut ctx.ops;
        let weights = attention_weights(&op.device, width, ln_w, ln_b, q, qt, qb, k, kt, v, vt, vb, out, ot, ob)?;
        let (input, s) = block_input(op, x, rows, width)?;
        op.begin()?;
        op.encoder_attention(input, &s, rows, heads, &weights)?;
        op.finish()?;
        op.read(input)
    })
}

pub(crate) fn try_encoder_ffn_block_f32(x: &[f32], rows: usize, width: usize, ln_w: &[f32], ln_b: &[f32],
    up: &[u8], ut: u32, ub: &[f32], down: &[u8], dt: u32, db: &[f32]) -> Option<Vec<f32>> {
    run("encoder FFN block", |ctx| {
        let op = &mut ctx.ops;
        let weights = ffn_weights(&op.device, width, ln_w, ln_b, up, ut, ub, down, dt, db)?;
        let (input, s) = block_input(op, x, rows, width)?;
        op.begin()?;
        op.ffn(input, &s, rows, &weights)?;
        op.finish()?;
        op.read(input)
    })
}

pub(crate) fn try_encoder_layer_f32(x: &[f32], rows: usize, width: usize, heads: usize,
    ln_w: &[f32], ln_b: &[f32], q: &[u8], qt: u32, qb: &[f32], k: &[u8], kt: u32,
    v: &[u8], vt: u32, vb: &[f32], out: &[u8], ot: u32, ob: &[f32],
    fln_w: &[f32], fln_b: &[f32], up: &[u8], ut: u32, ub: &[f32], down: &[u8], dt: u32, db: &[f32]) -> Option<Vec<f32>> {
    if !head_shape(width, heads) { return None; }
    run("encoder layer", |ctx| {
        let op = &mut ctx.ops;
        let weights = EncoderWeights {
            attn: attention_weights(&op.device, width, ln_w, ln_b, q, qt, qb, k, kt, v, vt, vb, out, ot, ob)?,
            ffn: ffn_weights(&op.device, width, fln_w, fln_b, up, ut, ub, down, dt, db)?,
        };
        let (input, s) = block_input(op, x, rows, width)?;
        op.begin()?;
        op.encoder_layer(input, &s, rows, heads, &weights)?;
        op.finish()?;
        op.read(input)
    })
}

pub(crate) fn try_encoder_stack_f32(x: &[f32], rows: usize, width: usize, heads: usize, layers: &[EncoderLayer], ln_w: &[f32], ln_b: &[f32]) -> Option<Vec<f32>> {
    if !head_shape(width, heads) || ln_w.len() != width || ln_b.len() != width { return None; }
    run("encoder stack", |ctx| {
        let op = &mut ctx.ops;
        let weights = layers.iter().map(|l| EncoderWeights::new(&op.device, l, op.int8_encoder)).collect::<Result<Vec<_>>>()?;
        let norm = Norm::new(&op.device, ln_w, ln_b)?;
        let (input, s) = block_input(op, x, rows, width)?;
        op.begin()?;
        for l in &weights { op.encoder_layer(input, &s, rows, heads, l)?; }
        op.norm(input, s.norm, width, Some(&norm), 1e-5);
        op.finish()?;
        op.read(s.norm)
    })
}

pub(crate) fn try_decoder_self_qkv_step_f32(x: &[f32], width: usize, ln_w: &[f32], ln_b: &[f32], q: &[u8], qt: u32, qb: &[f32], k: &[u8], kt: u32, v: &[u8], vt: u32, vb: &[f32]) -> Option<(Vec<f32>, Vec<f32>, Vec<f32>)> {
    if x.len() != width || ln_w.len() != width || ln_b.len() != width { return None; }
    run("decoder QKV", |ctx| {
        let op = &mut ctx.ops;
        let norm = Norm::new(&op.device, ln_w, ln_b)?;
        let q = Linear::raw(&op.device, q, qt, width, width, Some(qb))?;
        let k = Linear::raw(&op.device, k, kt, width, width, None)?;
        let v = Linear::raw(&op.device, v, vt, width, width, Some(vb))?;
        let (input, s) = block_input(op, x, 1, width)?;
        op.begin()?;
        op.norm(input, s.norm, width, Some(&norm), 1e-5);
        op.linear(s.norm, s.q, 1, &q)?;
        op.linear(s.norm, s.k, 1, &k)?;
        op.linear(s.norm, s.v, 1, &v)?;
        op.finish()?;
        Ok((op.read(s.q)?, op.read(s.k)?, op.read(s.v)?))
    })
}

pub(crate) fn try_decoder_cross_ffn_step_f32(_layer_idx: usize, x: &[f32], width: usize, heads: usize, k: &[f32], v: &[f32], padded_nk: usize, layer: &DecoderLayer) -> Option<Vec<f32>> {
    decoder_block(x, width, heads, None, k, v, padded_nk, layer)
}

pub(crate) fn try_decoder_self_cross_ffn_step_f32(_layer_idx: usize, x: &[f32], q: &[f32], k: &[f32], v: &[f32], nk: usize, width: usize, heads: usize, cross_k: &[f32], cross_v: &[f32], padded_cross_nk: usize, layer: &DecoderLayer) -> Option<Vec<f32>> {
    if q.len() != width || k.len() != nk.checked_mul(width)? || k.len() != v.len() || nk == 0 { return None; }
    decoder_block(x, width, heads, Some((q, k, v, nk)), cross_k, cross_v, padded_cross_nk, layer)
}

fn decoder_block(x: &[f32], width: usize, heads: usize, self_attn: Option<(&[f32], &[f32], &[f32], usize)>, k: &[f32], v: &[f32], padded_nk: usize, layer: &DecoderLayer) -> Option<Vec<f32>> {
    if !head_shape(width, heads) || x.len() != width || k.is_empty() || k.len() % width != 0 || v.len() != k.len() || k.len() / width > padded_nk { return None; }
    let nk = k.len() / width;
    run("decoder block", |ctx| {
        let op = &mut ctx.ops;
        let weights = DecoderWeights::new(&op.device, layer)?;
        let cross_k = upload(&op.device, k)?;
        let cross_v = upload(&op.device, v)?;
        let self_buffers = self_attn.map(|(q, k, v, nk)| -> Result<_> {
            Ok((upload(&op.device, q)?, upload(&op.device, k)?, upload(&op.device, v)?, nk))
        }).transpose()?;
        let (input, s) = block_input(op, x, 1, width)?;
        op.begin()?;
        if let Some((q, k, v, nk)) = &self_buffers {
            op.attention(q.address, k.address, v.address, op.addr(s.att), 1, *nk, heads, None, 0.125);
            op.linear(s.att, s.proj, 1, &weights.self_attn.out)?;
            op.add(s.proj, input, input);
        }
        op.cross_ffn(input, &s, 1, heads, &cross_k, &cross_v, nk, &weights)?;
        op.finish()?;
        op.read(input)
    })
}
