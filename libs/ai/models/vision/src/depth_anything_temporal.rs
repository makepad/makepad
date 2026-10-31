//! Video-Depth-Anything's temporal head, streaming mode.
//!
//! Video-Depth-Anything (VDA) is Depth-Anything-V2 plus four "motion
//! modules" inside the DPT head (after `layer_3`, `layer_4`, `refinenet4` and
//! `refinenet3`). Each one lets every pixel attend over that pixel's own
//! history, which removes the frame-to-frame flicker of a per-frame model.
//!
//! Streaming follows the reference `infer_video_depth_one` exactly: a module
//! keeps the layer-normed inputs of its attention blocks per frame, and each
//! new frame attends over a 32-frame window = the first frame (an anchor),
//! one older frame, the 29 most recent frames and itself, with the
//! sinusoidal position encoding indexed by window slot. Only those cached
//! inputs are kept; keys and values are recomputed per frame because the
//! position encoding moves with the window.
//!
//! Module structure (`TemporalModule` with one `TemporalTransformerBlock`):
//! GroupNorm(32, eps 1e-6) -> tokens -> proj_in -> 2 x [LayerNorm ->
//! temporal attention (8 heads, bias-free q/k/v, to_out with bias) ->
//! residual] -> LayerNorm -> GEGLU feed-forward (x4) -> residual ->
//! proj_out -> + input.

use crate::backend::{
    gpu_add, gpu_birefnet_tokens_to_planar, gpu_concat_rows_many, gpu_gather_rows_colblock,
    gpu_gelu_erf, gpu_group_norm_planar, gpu_layer_norm_mod, gpu_linear_f32_resident, gpu_mul,
    gpu_slice_cols, gpu_upload_u32, GpuTensor,
};
use crate::da3::{norm_mods, tensor, upload, CacheScope, Planar, TensorSource};
use crate::{DiffusionError, Result};
use makepad_ai_common::gpu::gpu_paint_attn_batched_self;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Mutex;

const HEADS: usize = 8;
const GROUPS: usize = 32;
const MAX_LEN: usize = 32;
/// Reference streaming constants (`video_depth_stream.py`).
const INFER_LEN: usize = 32;
const GAP: i64 = 41;

/// Cached attention inputs of one frame: `[module * 2 + block]`, each
/// `[pixels, channels]`.
pub(crate) type FrameCache = Vec<GpuTensor>;

struct Linear {
    w: GpuTensor,
    b: Option<GpuTensor>,
}

impl Linear {
    fn load(weights: &dyn TensorSource, name: &str, out: usize, inn: usize, bias: bool) -> Result<Self> {
        let w = tensor(weights, &format!("{name}.weight"), out * inn)?;
        let b = if bias {
            Some(upload(&tensor(weights, &format!("{name}.bias"), out)?, 1, out)?)
        } else {
            None
        };
        Ok(Self {
            w: upload(&w, out, inn)?,
            b,
        })
    }

    fn forward(&self, x: &GpuTensor) -> Result<GpuTensor> {
        gpu_linear_f32_resident(x, &self.w, self.b.as_ref()).map_err(DiffusionError::model)
    }
}

struct AttentionBlock {
    norm: GpuTensor,
    to_q: Linear,
    to_k: Linear,
    to_v: Linear,
    to_out: Linear,
    /// Sinusoidal position encoding, `[MAX_LEN, channels]`.
    pe: Vec<f32>,
}

pub(crate) struct MotionModule {
    channels: usize,
    gn_gamma: Vec<f32>,
    gn_beta: Vec<f32>,
    gn_key: String,
    namespace: &'static str,
    proj_in: Linear,
    blocks: [AttentionBlock; 2],
    ff_norm: GpuTensor,
    ff_in: Linear,
    ff_out: Linear,
    proj_out: Linear,
    /// Per (window length, pixels): the position encoding expanded to the
    /// time-major sequence, and the gathers to pixel-major and back.
    shapes: Mutex<HashMap<(usize, usize), Rc<ShapeTables>>>,
}

struct ShapeTables {
    pe: GpuTensor,
    to_pixel_major: GpuTensor,
    last_of_each: GpuTensor,
}

/// `PositionalEncoding` of the reference: sin on even, cos on odd channels.
fn sinusoid(channels: usize) -> Vec<f32> {
    let mut pe = vec![0.0f32; MAX_LEN * channels];
    for pos in 0..MAX_LEN {
        for i in (0..channels).step_by(2) {
            let div = (-(i as f64) * (10000.0f64).ln() / channels as f64).exp();
            let angle = pos as f64 * div;
            pe[pos * channels + i] = angle.sin() as f32;
            if i + 1 < channels {
                pe[pos * channels + i + 1] = angle.cos() as f32;
            }
        }
    }
    pe
}

impl MotionModule {
    /// `prefix` = `...head.motion_modules.N`.
    pub(crate) fn load(
        scope: &CacheScope,
        weights: &dyn TensorSource,
        prefix: &str,
        channels: usize,
    ) -> Result<Self> {
        let c = channels;
        let tt = format!("{prefix}.temporal_transformer");
        let block = |a: usize| -> Result<AttentionBlock> {
            let ab = format!("{tt}.transformer_blocks.0.attention_blocks.{a}");
            let pe_name = format!("{ab}.pos_encoder.pe");
            let pe = weights
                .tensor_f32(&pe_name)
                .ok()
                .filter(|pe| pe.len() == MAX_LEN * c)
                .unwrap_or_else(|| sinusoid(c));
            Ok(AttentionBlock {
                norm: norm_mods(weights, &format!("{tt}.transformer_blocks.0.norms.{a}"), c)?,
                to_q: Linear::load(weights, &format!("{ab}.to_q"), c, c, false)?,
                to_k: Linear::load(weights, &format!("{ab}.to_k"), c, c, false)?,
                to_v: Linear::load(weights, &format!("{ab}.to_v"), c, c, false)?,
                to_out: Linear::load(weights, &format!("{ab}.to_out.0"), c, c, true)?,
                pe,
            })
        };
        let ff = format!("{tt}.transformer_blocks.0.ff");
        Ok(Self {
            channels: c,
            gn_gamma: tensor(weights, &format!("{tt}.norm.weight"), c)?,
            gn_beta: tensor(weights, &format!("{tt}.norm.bias"), c)?,
            gn_key: scope.key(&format!("{tt}.norm")),
            namespace: scope.namespace,
            proj_in: Linear::load(weights, &format!("{tt}.proj_in"), c, c, true)?,
            blocks: [block(0)?, block(1)?],
            ff_norm: norm_mods(weights, &format!("{tt}.transformer_blocks.0.ff_norm"), c)?,
            ff_in: Linear::load(weights, &format!("{ff}.net.0.proj"), 8 * c, c, true)?,
            ff_out: Linear::load(weights, &format!("{ff}.net.2"), c, 4 * c, true)?,
            proj_out: Linear::load(weights, &format!("{tt}.proj_out"), c, c, true)?,
            shapes: Mutex::new(HashMap::new()),
        })
    }

    fn tables(&self, block: usize, len: usize, pixels: usize) -> Result<Rc<ShapeTables>> {
        let mut shapes = self
            .shapes
            .lock()
            .map_err(|_| DiffusionError::workflow("motion module shape cache poisoned"))?;
        // Both blocks share the gathers; the encoding differs per block.
        let key = (len * 2 + block, pixels);
        if let Some(tables) = shapes.get(&key) {
            return Ok(tables.clone());
        }
        let c = self.channels;
        let pe_src = &self.blocks[block].pe;
        let mut pe = vec![0.0f32; len * pixels * c];
        for t in 0..len {
            for p in 0..pixels {
                let dst = (t * pixels + p) * c;
                pe[dst..dst + c].copy_from_slice(&pe_src[t * c..(t + 1) * c]);
            }
        }
        // Time-major row t*pixels+p  ->  pixel-major row p*len+t.
        let mut to_pixel_major = Vec::with_capacity(len * pixels);
        for p in 0..pixels {
            for t in 0..len {
                to_pixel_major.push((t * pixels + p) as u32);
            }
        }
        let last_of_each: Vec<u32> = (0..pixels).map(|p| (p * len + len - 1) as u32).collect();
        let tables = Rc::new(ShapeTables {
            pe: upload(&pe, len * pixels, c)?,
            to_pixel_major: gpu_upload_u32(&to_pixel_major).map_err(DiffusionError::model)?,
            last_of_each: gpu_upload_u32(&last_of_each).map_err(DiffusionError::model)?,
        });
        shapes.insert(key, tables.clone());
        Ok(tables)
    }

    /// Current frame's normed input attends over `window` (older frames'
    /// inputs, oldest first) plus itself; returns `[pixels, channels]`.
    fn attend(&self, block: usize, normed: &GpuTensor, window: &[&GpuTensor], pixels: usize) -> Result<GpuTensor> {
        let c = self.channels;
        let a = &self.blocks[block];
        let len = window.len() + 1;
        let tables = self.tables(block, len, pixels)?;
        let mut parts: Vec<&GpuTensor> = window.to_vec();
        parts.push(normed);
        let seq = gpu_concat_rows_many(&parts).map_err(DiffusionError::model)?;
        let seq = gpu_add(&seq, &tables.pe).map_err(DiffusionError::model)?;
        let seq = gpu_gather_rows_colblock(&seq, &tables.to_pixel_major, None, c)
            .map_err(DiffusionError::model)?;
        let q = a.to_q.forward(&seq)?;
        let k = a.to_k.forward(&seq)?;
        let v = a.to_v.forward(&seq)?;
        let scale = 1.0 / ((c / HEADS) as f32).sqrt();
        let out = gpu_paint_attn_batched_self(&q, &k, &v, pixels, HEADS, scale)
            .map_err(DiffusionError::model)?;
        // Only the current frame's query is the reference's output.
        let last = gpu_gather_rows_colblock(&out, &tables.last_of_each, None, c)
            .map_err(DiffusionError::model)?;
        a.to_out.forward(&last)
    }

    /// One frame through the module. `window[b]` holds block `b`'s cached
    /// inputs of the window frames; returns the output plane and this
    /// frame's two cache entries.
    pub(crate) fn forward(
        &self,
        x: Planar,
        window: [&[&GpuTensor]; 2],
    ) -> Result<(Planar, [GpuTensor; 2])> {
        let c = self.channels;
        let pixels = x.width * x.height;
        let normed = gpu_group_norm_planar(
            &x.tensor,
            x.width,
            x.height,
            GROUPS,
            self.namespace,
            &self.gn_key,
            &self.gn_gamma,
            &self.gn_beta,
            1e-6,
        )
        .map_err(DiffusionError::model)?;
        let tokens = gpu_birefnet_tokens_to_planar(&normed).map_err(DiffusionError::model)?;
        let mut h = self.proj_in.forward(&tokens)?;
        let mut cache = Vec::with_capacity(2);
        for (block, window) in window.into_iter().enumerate() {
            let nh = gpu_layer_norm_mod(&h, &self.blocks[block].norm, 0, c, 1e-5)
                .map_err(DiffusionError::model)?;
            let att = self.attend(block, &nh, window, pixels)?;
            h = gpu_add(&h, &att).map_err(DiffusionError::model)?;
            cache.push(nh);
        }
        let x_ff = gpu_layer_norm_mod(&h, &self.ff_norm, 0, c, 1e-5).map_err(DiffusionError::model)?;
        let proj = self.ff_in.forward(&x_ff)?;
        let value = gpu_slice_cols(&proj, 0, 4 * c).map_err(DiffusionError::model)?;
        let gate = gpu_slice_cols(&proj, 4 * c, 4 * c).map_err(DiffusionError::model)?;
        let gate = gpu_gelu_erf(&gate).map_err(DiffusionError::model)?;
        let ff = self.ff_out.forward(&gpu_mul(&value, &gate).map_err(DiffusionError::model)?)?;
        h = gpu_add(&h, &ff).map_err(DiffusionError::model)?;
        let out = self.proj_out.forward(&h)?;
        let out = gpu_birefnet_tokens_to_planar(&out).map_err(DiffusionError::model)?;
        let out = gpu_add(&out, &x.tensor).map_err(DiffusionError::model)?;
        let mut cache = cache.into_iter();
        let entries = [cache.next().expect("block 0"), cache.next().expect("block 1")];
        Ok((
            Planar {
                tensor: out,
                width: x.width,
                height: x.height,
            },
            entries,
        ))
    }
}

/// The reference's streaming window (`frame_cache_list` and its pruning).
#[derive(Default)]
pub(crate) struct TemporalStream {
    shape: (usize, usize),
    frame_id: i64,
    caches: Vec<Rc<FrameCache>>,
}

impl TemporalStream {
    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }

    /// Window for the next frame at this input shape (empty on a first frame).
    pub(crate) fn window(&mut self, shape: (usize, usize)) -> Vec<Rc<FrameCache>> {
        if self.shape != shape {
            self.reset();
            self.shape = shape;
        }
        if self.caches.is_empty() {
            return Vec::new();
        }
        let n = self.caches.len();
        let mut window: Vec<Rc<FrameCache>> = self.caches[0..2].to_vec();
        window.extend(self.caches[n - (INFER_LEN - 3)..].iter().cloned());
        window
    }

    /// Record the frame just run.
    pub(crate) fn push(&mut self, cache: FrameCache) {
        self.frame_id += 1;
        let cache = Rc::new(cache);
        if self.caches.is_empty() {
            // First frame: simulate a full window of itself.
            self.caches = vec![cache; INFER_LEN];
        } else {
            self.caches.push(cache);
        }
        let id = self.frame_id - 1;
        if id + INFER_LEN as i64 > GAP + 1 {
            self.caches.remove(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// THE WINDOW (the reference's `frame_cache_list`): a first frame runs
    /// alone and fills the cache with copies of itself; every later frame
    /// attends over the anchor (the first frame), one older frame and the 29
    /// most recent ones. The older frame is the first frame's copies until
    /// they are pruned, then the one 41 frames back, and the cache holds 42.
    /// A new input size, or a reset, starts a new stream.
    #[test]
    fn the_window_is_the_anchor_one_older_frame_and_the_29_most_recent() {
        let shape = (308, 182);
        let mut stream = TemporalStream::default();
        let mut frames: Vec<Rc<FrameCache>> = Vec::new();
        for t in 0..120usize {
            let window = stream.window(shape);
            if t == 0 {
                assert!(window.is_empty(), "the first frame runs alone");
            } else {
                assert_eq!(window.len(), INFER_LEN - 1, "frame {t}: with itself, a window of 32");
                assert!(Rc::ptr_eq(&window[0], &frames[0]), "frame {t}: the anchor is the first frame");
                let older = t.saturating_sub(GAP as usize);
                assert!(Rc::ptr_eq(&window[1], &frames[older]), "frame {t}: the older frame is {older}");
                for (k, recent) in window[2..].iter().enumerate() {
                    let at = (t + k).saturating_sub(INFER_LEN - 3);
                    assert!(Rc::ptr_eq(recent, &frames[at]), "frame {t}: slot {} is frame {at}", k + 2);
                }
            }
            stream.push(Vec::new());
            frames.push(stream.caches.last().expect("a cache").clone());
            assert_eq!(stream.caches.len(), if t <= 10 { INFER_LEN + t } else { 42 }, "after frame {t}");
        }
        assert!(stream.window((154, 91)).is_empty(), "a new input size starts a new stream");
        stream.push(Vec::new());
        assert_eq!(stream.window((154, 91)).len(), INFER_LEN - 1);
        stream.reset();
        assert!(stream.window((154, 91)).is_empty(), "a reset starts a new stream");
    }
}
