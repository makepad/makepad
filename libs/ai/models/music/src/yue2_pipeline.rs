//! YuE2 song pipeline: plan (ABC score) -> semantic (codec tokens) ->
//! synthesize (flow-matching latents) -> decode (VAE audio).
//!
//! Stage order, seeds and chunking follow `yue2.pipeline` / `yue2.nar`:
//! each AR stage resets its own sampler to the request seed; the acoustic
//! noise is ONE torch-CPU `randn([frames, 64])` draw for the whole song,
//! viewed per original context chunk.

use crate::yue2::{
    yue2_chunk_ranges, yue2_negative_prefix, yue2_prefix, Yue2Phase, Yue2Request, Yue2Sampling,
    Yue2Weights, CODEC_OFFSET, CODEC_SIZE, MUSIC_END, YUE2_CONTEXT, YUE2_LATENT_DIM,
    YUE2_ODE_STEPS, YUE2_LAYERS,
};
use crate::oobleck::{OobleckDecoder, YUE2_OOBLECK};
use crate::yue2_lm::{Yue2DecodeMode, Yue2KvCache, Yue2Model, Yue2NarChunk};
use crate::yue2_graph::Yue2GraphModel;
use crate::yue2_graph_vae::Yue2GraphVae;
use crate::yue2_sample::{Yue2DeviceSampler, Yue2Sampler};
use crate::backend::{
    gpu_alloc_f32, gpu_download, gpu_graph_capture, gpu_graph_launch, gpu_increment_u32,
    gpu_profile_kernels,
    gpu_runtime_trim, gpu_scaled_sub_bf16_into, gpu_slice_rows, gpu_upload, gpu_upload_u32,
    GpuKernelProfile, GpuStepGraph, GpuTensor,
};
use crate::yue2_tokenizer::Yue2Tokenizer;
use crate::{DiffusionError, Result};
use std::path::Path;
use std::time::Instant;

/// Fine-grained progress for the hub's progress sink.
#[derive(Clone, Copy, Debug)]
pub enum Yue2Progress {
    /// AR score tokens: `done` of at most `max`.
    Score { done: usize, max: usize },
    /// AR codec tokens: `done` of at most `max` (25 per second of audio).
    Codec { done: usize, max: usize },
    /// Flow-matching midpoint steps over all chunks.
    Flow { done: usize, total: usize },
    /// VAE tiles.
    Decode { done: usize, total: usize },
}

#[derive(Clone, Debug, Default)]
pub struct Yue2StageTiming {
    pub seconds: f64,
    pub prefill_seconds: f64,
    pub tokens: usize,
    pub prefix_tokens: usize,
}

impl Yue2StageTiming {
    pub fn tokens_per_second(&self) -> f64 {
        let decode = self.seconds - self.prefill_seconds;
        if decode > 0.0 {
            self.tokens as f64 / decode
        } else {
            0.0
        }
    }
}

/// Planner output: the exact ABC ids (never re-tokenized from text unless
/// the user supplied/edited the score) and the semantic-stage prefix.
#[derive(Clone, Debug)]
pub struct Yue2Plan {
    pub abc: Option<String>,
    pub abc_ids: Vec<u32>,
    pub prefix: Vec<u32>,
    pub truncated: bool,
    pub timing: Yue2StageTiming,
}

#[derive(Clone, Debug)]
pub struct Yue2Semantic {
    /// Codec ids in `[0, 32768)`.
    pub tokens: Vec<u32>,
    pub truncated: bool,
    pub timing: Yue2StageTiming,
}

pub struct Yue2Pipeline {
    pub model: Yue2Model,
    pub graph_model: Option<Yue2GraphModel>,
    pub tokenizer: Yue2Tokenizer,
}

/// The imperative CUDA store remains the default where it is available.
/// MAKEPAD_AI_GRAPH_BACKEND chooses the executor only after this selection.
pub fn yue2_graph_backend_requested() -> bool {
    // The imperative gpu_* store is CUDA only; on Apple targets its names
    // resolve to a Metal-backed shim that cannot run YuE2, so Macs always
    // take the graph path.
    cfg!(not(any(target_os = "linux", target_os = "windows")))
        || std::env::var("MAKEPAD_YUE2_BACKEND").is_ok_and(|v| v.trim().eq_ignore_ascii_case("graph"))
        || !crate::backend::gpu_device_available()
}

/// One phase's graph decode buffers. A single graph contains both
/// branches when CFG is enabled. Drop the executable before its inputs.
pub struct Yue2DecodeSession<'a> {
    graph: Option<GpuStepGraph>,
    model: &'a Yue2Model,
    cache: Yue2KvCache,
    negative: Option<Yue2KvCache>,
    positions: GpuTensor,
    position: GpuTensor,
    sequence: GpuTensor,
    negative_position: Option<GpuTensor>,
    negative_sequence: Option<GpuTensor>,
    sampler: Yue2DeviceSampler,
    logits: GpuTensor,
    unconditional: Option<GpuTensor>,
    phase: Yue2Phase,
    warmed_up: bool,
}

impl<'a> Yue2DecodeSession<'a> {
    /// Prefill and first draw are outside capture. The first decode warms
    /// the exact shapes; the second is captured and executed exactly once.
    pub fn new(
        model: &'a Yue2Model,
        prefix: &[u32],
        negative: Option<&[u32]>,
        phase: Yue2Phase,
        sampling: &Yue2Sampling,
        seed: u64,
        guidance: f32,
        legacy_off: bool,
    ) -> Result<(Self, u32)> {
        if prefix.is_empty() || sampling.max_tokens == 0
            || prefix.len().saturating_add(sampling.max_tokens) > YUE2_CONTEXT
            || negative.is_some_and(|ids| ids.is_empty() || ids.len().saturating_add(sampling.max_tokens) > YUE2_CONTEXT)
            || (guidance != 1.0 && negative.is_none())
        {
            return Err(DiffusionError::workflow("yue2 decode session prefix/budget/guidance mismatch"));
        }
        let mut cache = Yue2KvCache::new(prefix.len() + sampling.max_tokens, model.ar_weights())?;
        let hidden = model.ar_forward(&mut cache, prefix)?;
        let (_, logits) = model.head_window_device(&hidden, phase, false)?;
        drop(hidden);
        let (negative, unconditional) = if let Some(ids) = negative {
            let mut cache = Yue2KvCache::new(ids.len() + sampling.max_tokens, model.ar_weights())?;
            let hidden = model.ar_forward(&mut cache, ids)?;
            let (_, logits) = model.head_window_device(&hidden, phase, false)?;
            (Some(cache), Some(logits))
        } else {
            (None, None)
        };
        let mut words = vec![prefix.len() as u32, (prefix.len() + 1) as u32];
        if let Some(cache) = &negative {
            words.extend_from_slice(&[cache.len as u32, (cache.len + 1) as u32]);
        }
        let positions = gpu_upload_u32(&words).map_err(DiffusionError::model)?;
        let position = gpu_slice_rows(&positions, 0, 1).map_err(DiffusionError::model)?;
        let sequence = gpu_slice_rows(&positions, 1, 1).map_err(DiffusionError::model)?;
        let (negative_position, negative_sequence) = if negative.is_some() {
            (
                Some(gpu_slice_rows(&positions, 2, 1).map_err(DiffusionError::model)?),
                Some(gpu_slice_rows(&positions, 3, 1).map_err(DiffusionError::model)?),
            )
        } else { (None, None) };
        let sampler = Yue2DeviceSampler::new(seed, phase, sampling, legacy_off, guidance)?;
        sampler.sample(&logits, unconditional.as_ref())?;
        let token = sampler.read_token()?;
        Ok((Self {
            graph: None, model, cache, negative, positions, position, sequence,
            negative_position, negative_sequence, sampler, logits, unconditional, phase,
            warmed_up: false,
        }, token))
    }

    fn run_step(&self) -> Result<(GpuTensor, Option<GpuTensor>)> {
        self.sampler.accept_previous()?;
        let (hidden, normed) = self.model.ar_decode_device(
            &self.cache, self.sampler.token(), &self.position, &self.sequence,
        )?;
        drop(hidden);
        let logits = self.model.head_from_normed(&normed, self.phase, true)?;
        drop(normed);
        let unconditional = if let Some(cache) = &self.negative {
            let (hidden, normed) = self.model.ar_decode_device(
                cache, self.sampler.token(),
                self.negative_position.as_ref().expect("negative position"),
                self.negative_sequence.as_ref().expect("negative length"),
            )?;
            drop(hidden);
            Some(self.model.head_from_normed(&normed, self.phase, true)?)
        } else { None };
        self.sampler.sample(&logits, unconditional.as_ref())?;
        gpu_increment_u32(&self.positions, 1).map_err(DiffusionError::model)?;
        Ok((logits, unconditional))
    }

    pub fn next_token(&mut self) -> Result<u32> {
        if self.cache.len >= self.cache.capacity
            || self.negative.as_ref().is_some_and(|c| c.len >= c.capacity)
        {
            return Err(DiffusionError::workflow("yue2 decode cache is full"));
        }
        if let Some(graph) = &self.graph {
            gpu_graph_launch(graph).map_err(DiffusionError::model)?;
        } else if self.warmed_up {
            let (graph, (logits, unconditional)) = gpu_graph_capture(|| {
                self.run_step().map_err(|e| e.to_string())
            }).map_err(DiffusionError::model)?;
            gpu_graph_launch(&graph).map_err(DiffusionError::model)?;
            self.logits = logits;
            self.unconditional = unconditional;
            self.graph = Some(graph);
        } else {
            let (logits, unconditional) = self.run_step()?;
            self.logits = logits;
            self.unconditional = unconditional;
            self.warmed_up = true;
        }
        let token = self.sampler.read_token()?;
        self.cache.len += 1;
        if let Some(cache) = &mut self.negative { cache.len += 1; }
        Ok(token)
    }

    pub fn is_captured(&self) -> bool {
        self.graph.is_some()
    }

    /// Advance one warm row with an event-instrumented graph replay. Discard
    /// the old graph so a later next_token captures the new output buffers.
    /// KV, sampler history and positions advance exactly once.
    pub fn profile_next_token(&mut self) -> Result<(u32, GpuKernelProfile)> {
        if !self.warmed_up || self.model.ar_weights() != crate::yue2_lm::Yue2ArWeights::Q8 {
            return Err(DiffusionError::workflow("yue2 kernel profile requires warm Q8 decode"));
        }
        if self.cache.len >= self.cache.capacity
            || self.negative.as_ref().is_some_and(|c| c.len >= c.capacity)
        {
            return Err(DiffusionError::workflow("yue2 decode cache is full"));
        }
        self.graph = None;
        let branches = 1 + self.negative.is_some() as usize;
        let ((logits, unconditional), profile) = gpu_profile_kernels(
            branches * YUE2_LAYERS * 16 + 64,
            || self.run_step().map_err(|e| e.to_string()),
        ).map_err(DiffusionError::model)?;
        self.logits = logits;
        self.unconditional = unconditional;
        let token = self.sampler.read_token()?;
        self.cache.len += 1;
        if let Some(cache) = &mut self.negative { cache.len += 1; }
        Ok((token, profile))
    }

    /// Exposes the exact graph output to the oracle driver without another
    /// forward pass. Normal generation only reads the sampled u32.
    pub fn logits(&self) -> (&GpuTensor, Option<&GpuTensor>) {
        (&self.logits, self.unconditional.as_ref())
    }

    pub fn force_token(&self, token: u32) -> Result<()> {
        if token as usize >= crate::yue2::YUE2_VOCAB {
            return Err(DiffusionError::model("yue2 teacher token out of range"));
        }
        self.sampler.force_token(token)
    }
}

/// YuE2-Vae (Oobleck, 48 kHz stereo, 1920 samples per latent).
pub enum Yue2Vae {
    Device(OobleckDecoder),
    Graph(Yue2GraphVae),
}

// Bound the late, high-rate activations: a full core + halo needs about
// 0.5 GiB for the largest im2col slab, versus about 1.8 GiB at core=1024.
// The halo is unchanged; splitting never drops or blends audio.
pub const YUE2_VAE_CORE_FRAMES: usize = 256;
pub const YUE2_VAE_HALO_FRAMES: usize = 16;

impl Yue2Vae {
    pub fn load(dir: impl AsRef<Path>) -> Result<Self> {
        if yue2_graph_backend_requested() {
            return Ok(Self::Graph(Yue2GraphVae::load(dir)?));
        }
        Ok(Self::Device(OobleckDecoder::load(dir.as_ref().join("model.safetensors"), YUE2_OOBLECK)?))
    }

    /// Latents `[frames, 64]` -> interleaved stereo `[samples, 2]`, clamped
    /// to [-1, 1] (exact core crops, with the reference's 16-latent halo).
    pub fn decode(&self, latents: &[f32], progress: &mut dyn FnMut(Yue2Progress) -> Result<()>) -> Result<Vec<f32>> {
        self.decode_prefix(latents, latents.len() / YUE2_LATENT_DIM, progress)
    }

    /// Decode the first `output_frames` cores. Following input latents remain
    /// available as right halo but do not cause extra output tiles to run.
    pub fn decode_prefix(&self, latents: &[f32], output_frames: usize, progress: &mut dyn FnMut(Yue2Progress) -> Result<()>) -> Result<Vec<f32>> {
        let frames = latents.len() / YUE2_LATENT_DIM;
        if latents.len() % YUE2_LATENT_DIM != 0 || output_frames == 0 || output_frames > frames {
            return Err(DiffusionError::workflow("yue2 VAE prefix latent shape"));
        }
        let decoder = match self {
            Self::Graph(graph) => return graph.decode_prefix(latents, output_frames, progress),
            Self::Device(decoder) => decoder,
        };
        // Idle flow activations must not overlap the VAE working set.
        gpu_runtime_trim().map_err(|e| DiffusionError::model(format!("yue2 VAE trim: {e}")))?;
        let mut audio = decoder.decode_tiled_device(
            latents,
            frames,
            output_frames,
            YUE2_VAE_CORE_FRAMES,
            YUE2_VAE_HALO_FRAMES,
            &mut |done, total| progress(Yue2Progress::Decode { done, total }),
        )?;
        for v in &mut audio {
            *v = v.clamp(-1.0, 1.0);
        }
        Ok(audio)
    }
}

/// torch CPU `randn` (float32, contiguous, numel >= 16): MT19937 uniforms
/// (24-bit), then `normal_fill_16` Box-Muller over 16-wide blocks (j, j+8),
/// with a recomputed final block when numel is not a multiple of 16.
pub fn torch_cpu_randn(seed: u64, n: usize) -> Vec<f32> {
    use makepad_ai_common::torch_rng::Mt19937;
    let mut rng = Mt19937::new(seed);
    let mut uniform = || ((rng.next_u32() & ((1 << 24) - 1)) as f32) * (1.0 / 16_777_216.0);
    let mut data: Vec<f32> = (0..n).map(|_| uniform()).collect();
    fn fill16(d: &mut [f32]) {
        for j in 0..8 {
            let u1 = 1.0 - d[j];
            let u2 = d[j + 8];
            let radius = (-2.0 * u1.ln()).sqrt();
            let theta = 2.0 * std::f32::consts::PI * u2;
            d[j] = radius * theta.cos();
            d[j + 8] = radius * theta.sin();
        }
    }
    if n < 16 {
        // torch uses the serial normal path below 16; not reached (frames*64).
        return data;
    }
    let mut i = 0;
    while i + 16 <= n {
        fill16(&mut data[i..i + 16]);
        i += 16;
    }
    if n % 16 != 0 {
        let tail = n - 16;
        for v in &mut data[tail..] {
            *v = uniform();
        }
        fill16(&mut data[tail..]);
    }
    data
}

impl Yue2Pipeline {
    pub fn load(model_dir: impl AsRef<Path>) -> Result<Self> {
        let dir = model_dir.as_ref();
        let tokenizer = Yue2Tokenizer::load(dir.join("qwen.tiktoken"))?;
        let model = Yue2Model::load(Yue2Weights::load(dir)?)?;
        let graph_model = if yue2_graph_backend_requested() {
            Some(Yue2GraphModel::load(Yue2Weights::load(dir)?)?)
        } else { None };
        Ok(Self { model, graph_model, tokenizer })
    }

    pub fn text_ids(&self, request: &Yue2Request) -> Vec<u32> {
        self.tokenizer.encode(&request.text())
    }

    /// The AR sampling loop for one phase (`sampling.generate_tokens`).
    /// Returns (ids without the end token, truncated, timing).
    pub fn generate(
        &self,
        prefix: &[u32],
        sampling: &Yue2Sampling,
        seed: u64,
        phase: Yue2Phase,
        guidance: f32,
        negative: Option<&[u32]>,
        legacy_off: bool,
        progress: &mut dyn FnMut(Yue2Progress) -> Result<()>,
    ) -> Result<(Vec<u32>, bool, Yue2StageTiming)> {
        if prefix.len() + sampling.max_tokens > YUE2_CONTEXT {
            return Err(DiffusionError::workflow(format!(
                "yue2: prefix {} + budget {} exceeds the {YUE2_CONTEXT}-token context",
                prefix.len(),
                sampling.max_tokens
            )));
        }
        let negative = if guidance != 1.0 {
            let negative = negative.ok_or_else(|| {
                DiffusionError::workflow("yue2: text guidance needs the negative prefix")
            })?;
            if negative.len() + sampling.max_tokens > YUE2_CONTEXT {
                return Err(DiffusionError::workflow("yue2: negative prefix + budget exceeds context"));
            }
            Some(negative)
        } else {
            None
        };
        if sampling.max_tokens == 0 {
            return Ok((Vec::new(), true, Yue2StageTiming { prefix_tokens: prefix.len(), ..Default::default() }));
        }
        if let Some(model) = &self.graph_model {
            let start = Instant::now();
            let mut session = model.ar_session(prefix, negative, sampling.max_tokens, phase)?;
            let mut timing = Yue2StageTiming {
                prefix_tokens: prefix.len(), prefill_seconds: start.elapsed().as_secs_f64(),
                ..Default::default()
            };
            let mut sampler = Yue2Sampler::new(seed);
            let mut history = Vec::with_capacity(sampling.max_tokens);
            let mut eos = false;
            for step in 0..sampling.max_tokens {
                let (lo, logits, uncond) = session.logits();
                let guided: Option<Vec<f32>> = uncond.map(|u| logits.iter().zip(u)
                    .map(|(&c, &u)| u + guidance * (c - u)).collect());
                let token = sampler.sample(guided.as_deref().unwrap_or(logits), lo, phase,
                    sampling, &history, step, legacy_off);
                match phase {
                    Yue2Phase::Abc => progress(Yue2Progress::Score { done: step + 1, max: sampling.max_tokens })?,
                    Yue2Phase::Semantic => progress(Yue2Progress::Codec { done: step + 1, max: sampling.max_tokens })?,
                }
                if token == phase.end_token() { eos = true; break; }
                history.push(token);
                if step + 1 < sampling.max_tokens { session.decode(token)?; }
            }
            timing.seconds = start.elapsed().as_secs_f64();
            timing.tokens = history.len() + eos as usize;
            return Ok((history, !eos, timing));
        }
        if self.model.decode_mode() == Yue2DecodeMode::Graph {
            return self.generate_graph(prefix, sampling, seed, phase, guidance, negative, legacy_off, progress);
        }
        let start = Instant::now();
        let mut sampler = Yue2Sampler::new(seed);
        let mut cache = Yue2KvCache::new(prefix.len() + sampling.max_tokens, self.model.ar_weights())?;
        let mut hidden = self.model.ar_forward(&mut cache, prefix)?;
        // CFG: the negative branch runs as its own sequence with its own cache.
        let mut neg = match negative {
            Some(ids) => {
                let mut c = Yue2KvCache::new(ids.len() + sampling.max_tokens, self.model.ar_weights())?;
                let h = self.model.ar_forward(&mut c, ids)?;
                Some((c, h))
            }
            None => None,
        };
        let mut timing = Yue2StageTiming { prefix_tokens: prefix.len(), ..Default::default() };
        let end = phase.end_token();
        let mut history: Vec<u32> = Vec::with_capacity(sampling.max_tokens);
        let mut eos = false;
        for step in 0..sampling.max_tokens {
            let (lo, mut logits) = self.model.head_window(&hidden, phase, step != 0)?;
            if let Some((_, nh)) = &neg {
                let (_, uncond) = self.model.head_window(nh, phase, step != 0)?;
                for (c, u) in logits.iter_mut().zip(&uncond) {
                    *c = u + guidance * (*c - u);
                }
            }
            if step == 0 {
                timing.prefill_seconds = start.elapsed().as_secs_f64();
            }
            let token = sampler.sample(&logits, lo, phase, sampling, &history, step, legacy_off);
            match phase {
                Yue2Phase::Abc => progress(Yue2Progress::Score { done: step + 1, max: sampling.max_tokens })?,
                Yue2Phase::Semantic => progress(Yue2Progress::Codec { done: step + 1, max: sampling.max_tokens })?,
            }
            if token == end {
                eos = true;
                break;
            }
            history.push(token);
            if step + 1 < sampling.max_tokens {
                hidden = self.model.ar_forward(&mut cache, &[token])?;
                if let Some((nc, nh)) = &mut neg {
                    *nh = self.model.ar_forward(nc, &[token])?;
                }
            }
        }
        timing.seconds = start.elapsed().as_secs_f64();
        timing.tokens = history.len() + eos as usize;
        Ok((history, !eos, timing))
    }

    fn generate_graph(
        &self,
        prefix: &[u32],
        sampling: &Yue2Sampling,
        seed: u64,
        phase: Yue2Phase,
        guidance: f32,
        negative: Option<&[u32]>,
        legacy_off: bool,
        progress: &mut dyn FnMut(Yue2Progress) -> Result<()>,
    ) -> Result<(Vec<u32>, bool, Yue2StageTiming)> {
        let start = Instant::now();
        let (mut session, mut token) = Yue2DecodeSession::new(
            &self.model, prefix, negative, phase, sampling, seed, guidance, legacy_off,
        )?;
        let mut timing = Yue2StageTiming {
            prefix_tokens: prefix.len(), prefill_seconds: start.elapsed().as_secs_f64(),
            ..Default::default()
        };
        let mut history = Vec::with_capacity(sampling.max_tokens);
        let mut eos = false;
        for step in 0..sampling.max_tokens {
            if step != 0 { token = session.next_token()?; }
            match phase {
                Yue2Phase::Abc => progress(Yue2Progress::Score { done: step + 1, max: sampling.max_tokens })?,
                Yue2Phase::Semantic => progress(Yue2Progress::Codec { done: step + 1, max: sampling.max_tokens })?,
            }
            if token == phase.end_token() { eos = true; break; }
            history.push(token);
        }
        timing.seconds = start.elapsed().as_secs_f64();
        timing.tokens = history.len() + eos as usize;
        Ok((history, !eos, timing))
    }

    pub fn plan(
        &self,
        request: &Yue2Request,
        sampling: &Yue2Sampling,
        progress: &mut dyn FnMut(Yue2Progress) -> Result<()>,
    ) -> Result<Yue2Plan> {
        let text = self.text_ids(request);
        if request.cot == crate::yue2::Yue2Cot::Off {
            return Ok(Yue2Plan {
                abc: None,
                abc_ids: vec![],
                prefix: yue2_prefix(&text, request.cot, None),
                truncated: false,
                timing: Yue2StageTiming::default(),
            });
        }
        if let Some(abc) = &request.abc {
            let ids = self.tokenizer.encode(abc);
            return Ok(Yue2Plan {
                abc: Some(abc.clone()),
                prefix: yue2_prefix(&text, request.cot, Some(&ids)),
                abc_ids: ids,
                truncated: false,
                timing: Yue2StageTiming::default(),
            });
        }
        let prefix = yue2_prefix(&text, request.cot, None);
        let (ids, truncated, timing) =
            self.generate(&prefix, sampling, request.seed, Yue2Phase::Abc, 1.0, None, false, progress)?;
        Ok(Yue2Plan {
            abc: Some(self.tokenizer.decode(&ids)),
            prefix: yue2_prefix(&text, request.cot, Some(&ids)),
            abc_ids: ids,
            truncated,
            timing,
        })
    }

    pub fn semantic(
        &self,
        request: &Yue2Request,
        plan: &Yue2Plan,
        sampling: &Yue2Sampling,
        progress: &mut dyn FnMut(Yue2Progress) -> Result<()>,
    ) -> Result<Yue2Semantic> {
        let guidance = request.guidance();
        let negative = if guidance != 1.0 {
            let instr = self.tokenizer.encode(request.cot.instruction());
            Some(yue2_negative_prefix(&instr, request.cot, &plan.abc_ids))
        } else {
            None
        };
        let legacy_off = request.cot == crate::yue2::Yue2Cot::Off;
        let (ids, truncated, timing) = self.generate(
            &plan.prefix,
            sampling,
            request.seed,
            Yue2Phase::Semantic,
            guidance,
            negative.as_deref(),
            legacy_off,
            progress,
        )?;
        Ok(Yue2Semantic {
            tokens: ids.into_iter().map(|t| t - CODEC_OFFSET).collect(),
            truncated,
            timing,
        })
    }

    /// Flow matching: `[frames, 64]` latents (host f32, row-major).
    pub fn synthesize(
        &self,
        prefix: &[u32],
        codec: &[u32],
        seed: u64,
        steps: usize,
        progress: &mut dyn FnMut(Yue2Progress) -> Result<()>,
    ) -> Result<Vec<f32>> {
        if codec.is_empty() || codec.iter().any(|&c| c >= CODEC_SIZE) || steps == 0 {
            return Err(DiffusionError::workflow("yue2 synthesize: empty codec/steps or out-of-range codec"));
        }
        let ranges = yue2_chunk_ranges(codec.len(), prefix.len());
        let noise = torch_cpu_randn(seed, codec.len() * YUE2_LATENT_DIM);
        let total = ranges.len() * steps;
        let mut out = Vec::with_capacity(noise.len());
        let dt = 1.0f64 / steps as f64;
        let raw_times: Vec<f32> = (0..steps).flat_map(|step| {
            let t = 1.0 - step as f64 * dt;
            [logit_clamped(t), logit_clamped(t - dt / 2.0)]
        }).collect();
        let device_error = |e| DiffusionError::model(format!("yue2 ODE: {e}"));
        for (ci, &(a, b)) in ranges.iter().enumerate() {
            let mut ar = prefix.to_vec();
            ar.extend(codec[a..b].iter().map(|&c| c + CODEC_OFFSET));
            ar.push(MUSIC_END);
            if let Some(model) = &self.graph_model {
                let mut chunk = model.nar_chunk(&ar, b - a, &raw_times)?;
                let mut state: Vec<f32> = noise[a * YUE2_LATENT_DIM..b * YUE2_LATENT_DIM]
                    .iter().map(|&v| bf16(v)).collect();
                let mut mid = vec![0.0; state.len()];
                for step in 0..steps {
                    let first = chunk.velocity(&state, 2 * step)?;
                    for ((m, &s), &v) in mid.iter_mut().zip(&state).zip(&first) {
                        *m = bf16(s - bf16(bf16(v) * (dt / 2.0) as f32));
                    }
                    let second = chunk.velocity(&mid, 2 * step + 1)?;
                    for (s, &v) in state.iter_mut().zip(&second) {
                        *s = bf16(*s - bf16(bf16(v) * dt as f32));
                    }
                    progress(Yue2Progress::Flow { done: ci * steps + step + 1, total })?;
                }
                out.extend(state);
                continue;
            }
            let chunk = Yue2NarChunk::prefill(&self.model, &ar, b - a, &raw_times)?;
            let initial: Vec<f32> = noise[a * YUE2_LATENT_DIM..b * YUE2_LATENT_DIM]
                .iter()
                .map(|&v| bf16(v))
                .collect();
            let state = gpu_upload(&initial, b - a, YUE2_LATENT_DIM).map_err(device_error)?;
            let mid = gpu_alloc_f32(b - a, YUE2_LATENT_DIM).map_err(device_error)?;
            for step in 0..steps {
                let first = chunk.velocity_device(&state, 2 * step)?;
                // Exactly bf16(s - bf16(bf16(v) * h)), without an FMA.
                // Consume the first velocity before its workspace is reused.
                gpu_scaled_sub_bf16_into(&state, first, (dt / 2.0) as f32, &mid).map_err(device_error)?;
                let second = chunk.velocity_device(&mid, 2 * step + 1)?;
                gpu_scaled_sub_bf16_into(&state, second, dt as f32, &state).map_err(device_error)?;
                progress(Yue2Progress::Flow { done: ci * steps + step + 1, total })?;
            }
            out.extend(gpu_download(&state).map_err(device_error)?);
        }
        Ok(out)
    }

    pub fn default_steps() -> usize {
        YUE2_ODE_STEPS
    }
}

fn bf16(v: f32) -> f32 {
    if !v.is_finite() {
        return v;
    }
    let bits = v.to_bits();
    f32::from_bits(bits.wrapping_add(0x7fff + ((bits >> 16) & 1)) & 0xffff_0000)
}

/// `torch.logit(t, f64).clamp(-20, 20)` as the python float the reference
/// passes on.
fn logit_clamped(t: f64) -> f32 {
    let v = (t / (1.0 - t)).ln();
    let v = if v.is_nan() { 0.0 } else { v };
    v.clamp(-20.0, 20.0) as f32
}
