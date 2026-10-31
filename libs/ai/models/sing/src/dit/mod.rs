//! Cantor DiT: the score-native flow-matching singing voice (feature `dit`).
//!
//! A scored line (notes and the words they sing) -> [`front`] (word-level
//! note tokens and the frame -> token map) -> the note encoder (embeddings,
//! four ConvNeXt-V2 blocks, expanded to 50 frames per second) -> a 22-layer,
//! 1024-wide DiT that turns noise into a 128-bin log-mel in `steps` Euler
//! steps of the flow (classifier-free guidance with a std rescale, the
//! voice's own sung clip in context for its timbre) -> a Vocos-style vocoder
//! (30 ConvNeXt blocks, an STFT head, inverse STFT) -> 24 kHz audio.
//!
//! Everything heavy runs on the device tensor surface (`makepad_ai_common::
//! gpu`, CUDA today, Metal where its kernels exist): f16 weights, f16 GEMM
//! operands with f32 accumulation, f32 activations. The host does the
//! front end, the solver arithmetic and the overlap-add.
//!
//! Metal: the same calls resolve to `makepad_ai_common::gpu`'s Metal side
//! on macOS. Of the ops used here it still lacks `gpu_dwconv1d`,
//! `gpu_swiglu_value_gate`, `gpu_rms_norm_mod_indexed` and
//! `gpu_attention_packed_flash2_d64` (they return "unavailable" there);
//! writing those kernels is the Metal port, no change to this module.
//!
//! Weights come from the reference checkpoint through [`convert`] into an
//! MKSING file (`kind=dit`); a voice is a sung clip's mel and its note
//! tokens, stored in the same file (`voice.<name>.*`).

pub mod front;

use crate::nn::{Params, Tensor};
use crate::weights::{self, Dtype};
use front::{DitNote, Tokens};
use makepad_ai_common::gpu::{self as g, GemmPrecision, GpuLinearPart, GpuTensor};
use makepad_ai_common::quant::{f32_to_f16, GGML_TYPE_F16};
use std::collections::HashMap;
use std::path::Path;

pub const SR: u32 = 24_000;
pub const HOP: usize = 480;
pub const N_FFT: usize = 1920;
pub const BINS: usize = N_FFT / 2 + 1;
pub const MELS: usize = 128;
pub const ENC: usize = 512;
pub const DIM: usize = 1024;
pub const FFN: usize = 4096;
pub const HEADS: usize = 16;
pub const HEAD_DIM: usize = DIM / HEADS;
pub const LAYERS: usize = 22;
pub const PRE_LAYERS: usize = 4;
pub const VOC_DIM: usize = 1024;
pub const VOC_FFN: usize = 4096;
pub const VOC_LAYERS: usize = 30;
pub const KERNEL: usize = 7;
const RMS_EPS: f32 = 1e-6;
const LN_EPS: f32 = 1e-6;
const ROPE_BASE: f64 = 10_000.0;
const MEL_MEAN: f32 = -4.92;
const MEL_VAR: f32 = 8.14;
/// AdaLN sites: per layer the input and post-attention norms, then the final norm.
const SITES: usize = 2 * LAYERS + 1;
const NS: &str = "cantor_dit";
/// f16 weights and GEMM operands, f32 accumulation and outputs.
const PRECISION: GemmPrecision = GemmPrecision { f16_accumulate: false, f16_activations: false };

fn io_err(m: impl std::fmt::Display) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, m.to_string())
}

#[derive(Clone, Debug)]
pub struct DitOpts {
    /// Euler steps of the flow (each one cond + uncond pass, batched).
    pub steps: usize,
    /// Classifier-free guidance scale.
    pub cfg: f32,
    pub seed: u64,
    /// Seconds per onset consonant a word starts before its first note.
    pub lead: f64,
}

impl Default for DitOpts {
    fn default() -> Self {
        DitOpts { steps: 32, cfg: 3.0, seed: 1, lead: 0.06 }
    }
}

/// A voice: a sung clip's normalised mel (frames x 128) and its note tokens.
#[derive(Clone, Debug)]
pub struct Voice {
    pub name: String,
    pub mel: Vec<f32>,
    pub tokens: Tokens,
}

/// What a render made, for inspection.
pub struct DitRender {
    /// Target frames x 128, normalised log-mel.
    pub mel: Vec<f32>,
    pub audio: Vec<f32>,
}

/// The AdaLN modulation of one render: per site a device table of rows
/// `[w - 1 | 0]`, one row per step.
struct StepTables {
    tables: Vec<GpuTensor>,
}

pub struct CantorDit {
    pub params: Params,
    pub phones: HashMap<String, u32>,
    pub voices: Vec<Voice>,
    /// Device-resident f32 tensors (depthwise kernels, iSTFT/STFT bases).
    res: HashMap<String, GpuTensor>,
    ones: GpuTensor,
    /// `cond_mlp(0)`: the unconditional pass's condition row.
    uncond_row: Vec<f32>,
}

fn f16_bytes(v: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len() * 2);
    for &x in v {
        out.extend_from_slice(&f32_to_f16(x).to_le_bytes());
    }
    out
}

fn ge(e: String) -> String {
    format!("cantor dit: {e}")
}

fn silu(x: f32) -> f32 {
    x / (1.0 + (-x).exp())
}

/// `y = x W^T + b` on the host (small, one-off products).
fn host_linear(x: &[f32], w: &Tensor, b: &Tensor) -> Vec<f32> {
    let (n, k) = (w.rows, w.cols);
    let rows = x.len() / k;
    let mut y = vec![0f32; rows * n];
    for r in 0..rows {
        let xr = &x[r * k..(r + 1) * k];
        for o in 0..n {
            let wr = &w.data[o * k..(o + 1) * k];
            let mut acc = 0f64;
            for i in 0..k {
                acc += (xr[i] * wr[i]) as f64;
            }
            y[r * n + o] = acc as f32 + b.data[o];
        }
    }
    y
}

/// The periodic Hann window of the STFT.
fn hann() -> Vec<f32> {
    (0..N_FFT).map(|n| (0.5 - 0.5 * (2.0 * std::f64::consts::PI * n as f64 / N_FFT as f64).cos()) as f32).collect()
}

impl CantorDit {
    pub fn load(path: &Path) -> std::io::Result<CantorDit> {
        let wf = weights::read(path)?;
        let cfg = |k: &str| wf.config.iter().find(|(a, _)| a == k).map(|(_, v)| v.clone());
        if cfg("kind").as_deref() != Some("dit") {
            return Err(io_err(format!("{}: not a Cantor DiT voice (kind=dit)", path.display())));
        }
        let phones: HashMap<String, u32> = cfg("phones").unwrap_or_default().split(' ').enumerate().map(|(i, p)| (p.to_string(), i as u32)).collect();
        let mut voices = Vec::new();
        for name in cfg("voices").unwrap_or_default().split(',').filter(|v| !v.is_empty()) {
            let get = |k: &str| -> std::io::Result<Vec<f32>> {
                let n = format!("voice.{name}.{k}");
                if !wf.params.has(&n) {
                    return Err(io_err(format!("missing {n}")));
                }
                Ok(wf.params.get(&n).data.clone())
            };
            let ints = |k: &str| -> std::io::Result<Vec<u32>> { Ok(get(k)?.iter().map(|v| *v as u32).collect()) };
            voices.push(Voice {
                name: name.to_string(),
                mel: get("mel")?,
                tokens: Tokens { phoneme: ints("phoneme")?, pitch: ints("pitch")?, kind: ints("kind")?, mel2note: ints("mel2note")? },
            });
        }
        let mut dit = CantorDit::from_params(wf.params, phones).map_err(io_err)?;
        dit.voices = voices;
        Ok(dit)
    }

    /// Upload the weights (f16 GEMM weights into the device cache, f32
    /// resident tensors) and derive the host constants.
    pub fn from_params(params: Params, phones: HashMap<String, u32>) -> Result<CantorDit, String> {
        if !g::gpu_device_available() {
            return Err(ge("no GPU device backend in this build".into()));
        }
        for (name, t) in params.names.iter().zip(&params.vals) {
            if t.rows > 1 && t.cols > 1 && !name.starts_with("voice.") && !name.starts_with("enc.") && !name.ends_with(".dw") && name != "mel_basis" {
                g::gpu_weight_cache_ensure(NS, name, GGML_TYPE_F16, t.rows, t.cols, false, || Ok(f16_bytes(&t.data))).map_err(ge)?;
            }
        }
        let mut res = HashMap::new();
        for (name, t) in params.names.iter().zip(&params.vals) {
            if name.ends_with(".dw") {
                res.insert(name.clone(), g::gpu_upload(&t.data, t.rows, t.cols).map_err(ge)?);
                let b = format!("{name}_b");
                let bt = params.get(&b);
                res.insert(b, g::gpu_upload(&bt.data, 1, bt.cols).map_err(ge)?);
            }
        }
        // iSTFT basis: frame[n] = window[n] / N * (Re X0 + Re X_{N/2} (-1)^n
        // + 2 sum_k (Re X_k cos - Im X_k sin)), columns [re 0..BINS | im 0..BINS].
        let win = hann();
        let mut basis = vec![0f32; N_FFT * 2 * BINS];
        for n in 0..N_FFT {
            for k in 0..BINS {
                let a = 2.0 * std::f64::consts::PI * (k * n % N_FFT) as f64 / N_FFT as f64;
                let edge = k == 0 || k == N_FFT / 2;
                let scale = if edge { 1.0 } else { 2.0 } * win[n] as f64 / N_FFT as f64;
                basis[n * 2 * BINS + k] = (scale * a.cos()) as f32;
                basis[n * 2 * BINS + BINS + k] = if edge { 0.0 } else { (-scale * a.sin()) as f32 };
            }
        }
        res.insert("istft".into(), g::gpu_upload(&basis, N_FFT, 2 * BINS).map_err(ge)?);
        // STFT basis for the prompt mel: rows [re k | im k] over the windowed frame.
        let mut fwd = vec![0f32; 2 * BINS * N_FFT];
        for k in 0..BINS {
            for n in 0..N_FFT {
                let a = 2.0 * std::f64::consts::PI * (k * n % N_FFT) as f64 / N_FFT as f64;
                fwd[k * N_FFT + n] = (win[n] as f64 * a.cos()) as f32;
                fwd[(BINS + k) * N_FFT + n] = (-(win[n] as f64) * a.sin()) as f32;
            }
        }
        res.insert("stft".into(), g::gpu_upload(&fwd, 2 * BINS, N_FFT).map_err(ge)?);
        let ones = g::gpu_upload(&vec![1.0; DIM], 1, DIM).map_err(ge)?;
        let h = host_linear(&vec![0.0; DIM], params.get("dit.cond_mlp.0"), params.get("dit.cond_mlp.0_b"));
        let h: Vec<f32> = h.into_iter().map(silu).collect();
        let uncond_row = host_linear(&h, params.get("dit.cond_mlp.2"), params.get("dit.cond_mlp.2_b"));
        Ok(CantorDit { params, phones, voices: Vec::new(), res, ones, uncond_row })
    }

    pub fn voice(&self, name: &str) -> Option<&Voice> {
        self.voices.iter().find(|v| v.name == name)
    }

    fn bias(&self, name: &str) -> &[f32] {
        &self.params.get(&format!("{name}_b")).data
    }

    /// `x W^T + b` with the cached f16 weight `name` (bias `name_b`).
    fn lin(&self, x: &GpuTensor, name: &str) -> Result<GpuTensor, String> {
        let n = self.params.get(name).rows;
        let part = GpuLinearPart { bt_ggml_type: GGML_TYPE_F16, n, cache_key: name, bytes: &[] };
        g::gpu_linear_nt_cached_with_precision(x, NS, &[part], self.bias(name), PRECISION).map_err(ge)
    }

    /// Several bias-free weights side by side (`[x W1^T | x W2^T ..]`).
    fn lin_parts(&self, x: &GpuTensor, names: &[&str]) -> Result<GpuTensor, String> {
        let parts: Vec<GpuLinearPart<'_>> = names.iter().map(|n| GpuLinearPart { bt_ggml_type: GGML_TYPE_F16, n: self.params.get(n).rows, cache_key: n, bytes: &[] }).collect();
        g::gpu_linear_nt_cached_with_precision(x, NS, &parts, &[], PRECISION).map_err(ge)
    }

    /// Linear, SiLU, linear (`<name>.0`, `<name>.2`).
    fn mlp(&self, x: &GpuTensor, name: &str) -> Result<GpuTensor, String> {
        let h = self.lin(x, &format!("{name}.0"))?;
        let h = g::gpu_silu(&h).map_err(ge)?;
        self.lin(&h, &format!("{name}.2"))
    }

    fn layer_norm(&self, x: &GpuTensor, name: &str) -> Result<GpuTensor, String> {
        g::gpu_layer_norm_mul_add(x, &self.params.get(&format!("{name}_w")).data, &self.params.get(&format!("{name}_b")).data, LN_EPS).map_err(ge)
    }

    fn dwconv(&self, x: &GpuTensor, name: &str) -> Result<GpuTensor, String> {
        g::gpu_dwconv1d(x, &self.res[name], Some(&self.res[&format!("{name}_b")])).map_err(ge)
    }

    /// The note encoder over prompt + target tokens: `[frames, 1024]`
    /// conditions (prompt frames first).
    pub fn encode(&self, prompt: &Tokens, target: &Tokens) -> Result<GpuTensor, String> {
        let n_tok = prompt.phoneme.len() + target.phoneme.len();
        let (text, pitch, kind) = (self.params.get("enc.text"), self.params.get("enc.pitch"), self.params.get("enc.type"));
        let mut feats = vec![0f32; n_tok * ENC];
        let toks = prompt.phoneme.iter().zip(&prompt.pitch).zip(&prompt.kind).chain(target.phoneme.iter().zip(&target.pitch).zip(&target.kind));
        for (i, ((p, q), k)) in toks.enumerate() {
            let row = &mut feats[i * ENC..(i + 1) * ENC];
            for c in 0..ENC {
                // The reference's association: pitch + type + text.
                row[c] = pitch.data[*q as usize * ENC + c] + kind.data[*k as usize * ENC + c] + text.data[*p as usize * ENC + c];
            }
        }
        let mut x = g::gpu_upload(&feats, n_tok, ENC).map_err(ge)?;
        for l in 0..PRE_LAYERS {
            let p = format!("pre.{l}");
            let h = self.dwconv(&x, &format!("{p}.dw"))?;
            let h = self.layer_norm(&h, &format!("{p}.ln"))?;
            let h = self.lin(&h, &format!("{p}.pw1"))?;
            let h = g::gpu_gelu_erf(&h).map_err(ge)?;
            // GRN over time: x * (1 + gamma * N) + beta, N = |x|_t / mean_c |x|_t.
            let mut hv = g::gpu_download(&h).map_err(ge)?;
            let w = 2 * ENC;
            let mut gx = vec![0f64; w];
            for r in 0..n_tok {
                for c in 0..w {
                    let v = hv[r * w + c] as f64;
                    gx[c] += v * v;
                }
            }
            let gx: Vec<f32> = gx.iter().map(|v| v.sqrt() as f32).collect();
            let mean = gx.iter().map(|v| *v as f64).sum::<f64>() as f32 / w as f32;
            let (gamma, beta) = (&self.params.get(&format!("{p}.grn_g")).data, &self.params.get(&format!("{p}.grn_b")).data);
            for r in 0..n_tok {
                for c in 0..w {
                    let v = hv[r * w + c];
                    let nx = gx[c] / (mean + 1e-6);
                    hv[r * w + c] = gamma[c] * (v * nx) + beta[c] + v;
                }
            }
            let h = g::gpu_upload(&hv, n_tok, w).map_err(ge)?;
            let h = self.lin(&h, &format!("{p}.pw2"))?;
            x = g::gpu_add(&x, &h).map_err(ge)?;
        }
        let off = prompt.phoneme.len() as u32;
        let idx: Vec<u32> = prompt.mel2note.iter().copied().chain(target.mel2note.iter().map(|m| m + off)).map(|m| m.min(n_tok as u32 - 1)).collect();
        let idx = g::gpu_upload_u32(&idx).map_err(ge)?;
        let frames = g::gpu_gather_rows_colblock(&x, &idx, None, ENC).map_err(ge)?;
        // cond_emb with the f0 embedding's unvoiced row folded into its bias.
        self.lin(&frames, "cond_emb")
    }

    /// Per-step AdaLN tables for `steps` midpoint times.
    fn step_tables(&self, steps: usize) -> Result<StepTables, String> {
        let rows = steps.max(2);
        let half = DIM / 2;
        let scale = (ROPE_BASE.ln()) / (half - 1) as f64;
        let mut temb = vec![0f32; rows * DIM];
        for s in 0..steps {
            let t = ((s as f64 + 0.5) / steps as f64) as f32;
            for i in 0..half {
                let f = (-(i as f64) * scale).exp() as f32;
                temb[s * DIM + i] = (t * f).sin();
                temb[s * DIM + half + i] = (t * f).cos();
            }
        }
        let temb = g::gpu_upload(&temb, rows, DIM).map_err(ge)?;
        let c = self.mlp(&temb, "dit.step_mlp")?;
        let mut tables = Vec::with_capacity(SITES);
        for site in 0..SITES {
            let name = if site == 2 * LAYERS { "dit.norm".to_string() } else { format!("dit.{}.norm{}", site / 2, site % 2 + 1) };
            let w = g::gpu_download(&self.lin(&c, &name)?).map_err(ge)?;
            let mut t = vec![0f32; steps * 2 * DIM];
            for s in 0..steps {
                for i in 0..DIM {
                    t[s * 2 * DIM + i] = w[s * DIM + i] - 1.0;
                }
            }
            tables.push(g::gpu_upload(&t, steps, 2 * DIM).map_err(ge)?);
        }
        Ok(StepTables { tables })
    }

    /// Render a target's tokens in a voice: the flow from `noise` (target
    /// frames x 128; drawn from the seed when `None`) to a mel, then audio.
    pub fn render_tokens(&self, voice: &Voice, target: &Tokens, opts: &DitOpts, noise: Option<&[f32]>) -> Result<DitRender, String> {
        let mel = self.sample(voice, target, opts, noise)?;
        let audio = self.vocode(&mel, target.frames())?;
        Ok(DitRender { mel, audio })
    }

    /// The flow solve: `steps` midpoint Euler steps, each one batched device
    /// pass of the conditional (prompt + target) and unconditional (target
    /// alone, no condition) estimates, combined as the reference does.
    pub fn sample(&self, voice: &Voice, target: &Tokens, opts: &DitOpts, noise: Option<&[f32]>) -> Result<Vec<f32>, String> {
        let lp = voice.tokens.frames();
        let t = target.frames();
        if voice.mel.len() != lp * MELS || t == 0 {
            return Err(ge(format!("bad sizes: prompt {lp} frames ({} mel values), target {t} frames", voice.mel.len())));
        }
        let lc = lp + t;
        let rows = lc + t;
        let steps = opts.steps.max(1);
        let mut x: Vec<f32> = match noise {
            Some(z) if z.len() == t * MELS => z.to_vec(),
            Some(z) => return Err(ge(format!("noise has {} values, want {}", z.len(), t * MELS))),
            None => {
                let mut rng = crate::dsp::Rng::new(opts.seed);
                (0..t * MELS).map(|_| rng.normal()).collect()
            }
        };
        // Step-constant device state: the condition rows of both passes
        // through cond_mlp, rope tables, per-step norm tables and indices.
        let cond = self.encode(&voice.tokens, target)?;
        let cond = self.mlp(&cond, "dit.cond_mlp")?;
        let mut un = Vec::with_capacity(t * DIM);
        for _ in 0..t {
            un.extend_from_slice(&self.uncond_row);
        }
        let un = g::gpu_upload(&un, t, DIM).map_err(ge)?;
        let cond_all = g::gpu_concat_rows(&cond, &un).map_err(ge)?;
        let half = HEAD_DIM / 2;
        let mut cos = vec![0f32; rows * half];
        let mut sin = vec![0f32; rows * half];
        for r in 0..rows {
            let pos = if r < lc { r } else { r - lc } as f64;
            for i in 0..half {
                let inv = 1.0 / ROPE_BASE.powf((2 * i) as f64 / HEAD_DIM as f64);
                let a = (pos * inv as f32 as f64) as f32 as f64;
                cos[r * half + i] = a.cos() as f32;
                sin[r * half + i] = a.sin() as f32;
            }
        }
        let cos = g::gpu_upload(&cos, rows, half).map_err(ge)?;
        let sin = g::gpu_upload(&sin, rows, half).map_err(ge)?;
        let tables = self.step_tables(steps)?;
        let h = 1.0f32 / steps as f32;
        let scale = 1.0 / (HEAD_DIM as f32).sqrt();
        let mut x_in = vec![0f32; rows * MELS];
        x_in[..lp * MELS].copy_from_slice(&voice.mel);
        for s in 0..steps {
            x_in[lp * MELS..lc * MELS].copy_from_slice(&x);
            x_in[lc * MELS..].copy_from_slice(&x);
            let idx = g::gpu_upload_u32(&vec![s as u32; rows]).map_err(ge)?;
            let xd = g::gpu_upload(&x_in, rows, MELS).map_err(ge)?;
            let mut hs = self.mlp(&xd, "dit.mel_mlp")?;
            hs = g::gpu_add(&hs, &cond_all).map_err(ge)?;
            for l in 0..LAYERS {
                let p = format!("dit.{l}");
                let n = self.adaln(&hs, &tables.tables[2 * l], &idx)?;
                let qkv = self.lin_parts(&n, &[&format!("{p}.q"), &format!("{p}.k"), &format!("{p}.v")])?;
                let q = g::gpu_slice_cols(&qkv, 0, DIM).map_err(ge)?;
                let k = g::gpu_slice_cols(&qkv, DIM, DIM).map_err(ge)?;
                let v = g::gpu_slice_cols(&qkv, 2 * DIM, DIM).map_err(ge)?;
                let q = g::gpu_rope_half(&q, HEADS, half, &cos, &sin).map_err(ge)?;
                let k = g::gpu_rope_half(&k, HEADS, half, &cos, &sin).map_err(ge)?;
                let mut parts = Vec::with_capacity(2);
                for (a, len) in [(0, lc), (lc, t)] {
                    let qs = g::gpu_slice_rows(&q, a, len).map_err(ge)?;
                    let ks = g::gpu_slice_rows(&k, a, len).map_err(ge)?;
                    let vs = g::gpu_slice_rows(&v, a, len).map_err(ge)?;
                    parts.push(g::gpu_attention_packed_flash2_d64(&qs, &ks, &vs, HEADS, scale).map_err(ge)?);
                }
                let attn = g::gpu_concat_rows(&parts[0], &parts[1]).map_err(ge)?;
                let o = self.lin_parts(&attn, &[&format!("{p}.o")])?;
                hs = g::gpu_add(&hs, &o).map_err(ge)?;
                let n = self.adaln(&hs, &tables.tables[2 * l + 1], &idx)?;
                let f = self.lin_parts(&n, &[&format!("{p}.up"), &format!("{p}.gate")])?;
                let f = g::gpu_swiglu_value_gate(&f).map_err(ge)?;
                let d = self.lin_parts(&f, &[&format!("{p}.down")])?;
                hs = g::gpu_add(&hs, &d).map_err(ge)?;
            }
            let n = self.adaln(&hs, &tables.tables[2 * LAYERS], &idx)?;
            let out = self.mlp(&n, "dit.out_mlp")?;
            let out = g::gpu_download(&out).map_err(ge)?;
            let (vc, vu) = (&out[lp * MELS..lc * MELS], &out[lc * MELS..]);
            let std = |v: &mut dyn Iterator<Item = f32>, n: usize| -> f32 {
                let vals: Vec<f64> = v.map(|x| x as f64).collect();
                let m = vals.iter().sum::<f64>() / n as f64;
                (vals.iter().map(|x| (x - m) * (x - m)).sum::<f64>() / (n - 1).max(1) as f64).sqrt() as f32
            };
            let n_el = t * MELS;
            let pos_std = std(&mut vc.iter().copied(), n_el);
            let guided: Vec<f32> = vc.iter().zip(vu).map(|(c, u)| c + opts.cfg * (c - u)).collect();
            let g_std = std(&mut guided.iter().copied(), n_el);
            let r = pos_std / g_std.max(1e-12);
            for (xv, gv) in x.iter_mut().zip(&guided) {
                let v = 0.75 * (gv * r) + 0.25 * gv;
                *xv += v * h;
            }
        }
        Ok(x)
    }

    fn adaln(&self, x: &GpuTensor, table: &GpuTensor, idx: &GpuTensor) -> Result<GpuTensor, String> {
        g::gpu_rms_norm_mod_indexed(x, &self.ones, table, idx, 2 * DIM, 0, DIM, RMS_EPS, false).map_err(ge)
    }

    /// The vocoder: a normalised log-mel (frames x 128) -> 24 kHz audio,
    /// `frames * 480` samples.
    pub fn vocode(&self, mel: &[f32], frames: usize) -> Result<Vec<f32>, String> {
        let pad = KERNEL / 2;
        let m = g::gpu_upload(mel, frames, MELS).map_err(ge)?;
        // Embedding conv (k 7, zero padded): 7 shifted gathers, one GEMM.
        let mut taps = Vec::with_capacity(KERNEL);
        for j in 0..KERNEL {
            let idx: Vec<u32> = (0..frames).map(|t| {
                let s = t as isize + j as isize - pad as isize;
                if s < 0 || s >= frames as isize { u32::MAX } else { s as u32 }
            }).collect();
            let idx = g::gpu_upload_u32(&idx).map_err(ge)?;
            taps.push(g::gpu_gather_rows_colblock(&m, &idx, None, MELS).map_err(ge)?);
        }
        let refs: Vec<&GpuTensor> = taps.iter().collect();
        let cat = g::gpu_concat_cols(&refs).map_err(ge)?;
        let mut x = self.lin(&cat, "voc.embed")?;
        x = self.layer_norm(&x, "voc.ln")?;
        for l in 0..VOC_LAYERS {
            let p = format!("voc.{l}");
            let h = self.dwconv(&x, &format!("{p}.dw"))?;
            let h = self.layer_norm(&h, &format!("{p}.ln"))?;
            let h = self.lin(&h, &format!("{p}.pw1"))?;
            let h = g::gpu_gelu_erf(&h).map_err(ge)?;
            let h = self.lin(&h, &format!("{p}.pw2"))?;
            x = g::gpu_add(&x, &h).map_err(ge)?;
        }
        x = self.layer_norm(&x, "voc.final_ln")?;
        let head = g::gpu_download(&self.lin(&x, "voc.head")?).map_err(ge)?;
        let w = 2 * BINS;
        let mut spec = vec![0f32; frames * w];
        for t in 0..frames {
            for k in 0..BINS {
                let mag = head[t * w + k].exp().min(1e2);
                let ph = head[t * w + BINS + k];
                spec[t * w + k] = mag * ph.cos();
                spec[t * w + BINS + k] = mag * ph.sin();
            }
        }
        let spec = g::gpu_upload(&spec, frames, w).map_err(ge)?;
        let fr = g::gpu_download(&g::gpu_linear_f32_resident(&spec, &self.res["istft"], None).map_err(ge)?).map_err(ge)?;
        // Overlap-add, "same" padding trim, window envelope.
        let win = hann();
        let total = (frames - 1) * HOP + N_FFT;
        let mut y = vec![0f64; total];
        let mut env = vec![0f64; total];
        for t in 0..frames {
            for n in 0..N_FFT {
                y[t * HOP + n] += fr[t * N_FFT + n] as f64;
                env[t * HOP + n] += (win[n] * win[n]) as f64;
            }
        }
        let trim = (N_FFT - HOP) / 2;
        Ok((trim..total - trim).map(|i| (y[i] / env[i]) as f32).collect())
    }

    /// A 24 kHz clip's normalised log-mel (the prompt features), frames =
    /// samples / 480.
    pub fn mel_of(&self, wav: &[f32]) -> Result<Vec<f32>, String> {
        let p = (N_FFT - HOP) / 2;
        let n = wav.len();
        if n < p + 1 {
            return Err(ge("clip too short".into()));
        }
        let at = |i: isize| -> f32 {
            let j = if i < 0 { -i } else if i >= n as isize { 2 * (n as isize - 1) - i } else { i };
            wav[j as usize]
        };
        let frames = (n + 2 * p - N_FFT) / HOP + 1;
        let mut fr = vec![0f32; frames * N_FFT];
        for t in 0..frames {
            for k in 0..N_FFT {
                fr[t * N_FFT + k] = at((t * HOP + k) as isize - p as isize);
            }
        }
        let fr = g::gpu_upload(&fr, frames, N_FFT).map_err(ge)?;
        let spec = g::gpu_download(&g::gpu_linear_f32_resident(&fr, &self.res["stft"], None).map_err(ge)?).map_err(ge)?;
        let basis = &self.params.get("mel_basis").data;
        let mut out = vec![0f32; frames * MELS];
        let sd = MEL_VAR.sqrt();
        for t in 0..frames {
            let mag: Vec<f32> = (0..BINS).map(|k| {
                let (re, im) = (spec[t * 2 * BINS + k], spec[t * 2 * BINS + BINS + k]);
                (re * re + im * im + 1e-9).sqrt()
            }).collect();
            for m in 0..MELS {
                let mut acc = 0f64;
                for k in 0..BINS {
                    acc += (basis[m * BINS + k] * mag[k]) as f64;
                }
                out[t * MELS + m] = ((acc as f32).max(1e-5).ln() - MEL_MEAN) / sd;
            }
        }
        Ok(out)
    }

    /// Word-level notes -> tokens with this model's phone set.
    pub fn tokens(&self, notes: &[DitNote]) -> Result<Tokens, String> {
        front::tokens(notes, &self.phones)
    }
}

/// Convert the reference checkpoint (`model.pt`, fp32 state dict) and its
/// phone set (one phone per entry, in id order) into the MKSING form: GEMM
/// weights as stored matrices (gamma and the f0 row folded in), depthwise
/// kernels `[ch, 7]`, the vocoder's embedding conv packed tap-major.
pub fn convert(pt: &Path, phone_set: &[String]) -> Result<(Vec<(String, String)>, Params), String> {
    use makepad_ai_common::torch_pth::PthStateDict;
    let mut sd = PthStateDict::load(pt).map_err(|e| format!("{}: {e}", pt.display()))?;
    let mut p = Params::new();
    let mut get = |n: &str| -> Result<(Vec<usize>, Vec<f32>), String> {
        let shape = sd.shape(n).map_err(|e| format!("{n}: {e}"))?;
        let v = sd.f32(n).map_err(|e| format!("{n}: {e}"))?;
        Ok((shape, v))
    };
    fn put(p: &mut Params, name: &str, rows: usize, cols: usize, v: Vec<f32>) {
        p.insert(name, Tensor::new(rows, cols, v));
    }
    // A linear: weight [out, in] and bias.
    let linear = |p: &mut Params, get: &mut dyn FnMut(&str) -> Result<(Vec<usize>, Vec<f32>), String>, src: &str, dst: &str, bias: bool| -> Result<(), String> {
        let (s, w) = get(&format!("{src}.weight"))?;
        put(p, dst, s[0], s[1], w);
        if bias {
            let (_, b) = get(&format!("{src}.bias"))?;
            put(p, &format!("{dst}_b"), 1, b.len(), b);
        }
        Ok(())
    };
    let vec1 = |p: &mut Params, get: &mut dyn FnMut(&str) -> Result<(Vec<usize>, Vec<f32>), String>, src: &str, dst: &str| -> Result<(), String> {
        let (_, v) = get(src)?;
        put(p, dst, 1, v.len(), v);
        Ok(())
    };
    for (src, dst) in [("note_text_encoder", "enc.text"), ("note_pitch_encoder", "enc.pitch"), ("note_type_encoder", "enc.type")] {
        let (s, w) = get(&format!("{src}.weight"))?;
        put(&mut p, dst, s[0], s[1], w);
    }
    for l in 0..PRE_LAYERS {
        let (src, dst) = (format!("preflow.{l}"), format!("pre.{l}"));
        let (s, w) = get(&format!("{src}.dwconv.weight"))?;
        put(&mut p, &format!("{dst}.dw"), s[0], s[2], w);
        vec1(&mut p, &mut get, &format!("{src}.dwconv.bias"), &format!("{dst}.dw_b"))?;
        vec1(&mut p, &mut get, &format!("{src}.norm.weight"), &format!("{dst}.ln_w"))?;
        vec1(&mut p, &mut get, &format!("{src}.norm.bias"), &format!("{dst}.ln_b"))?;
        linear(&mut p, &mut get, &format!("{src}.pwconv1"), &format!("{dst}.pw1"), true)?;
        vec1(&mut p, &mut get, &format!("{src}.grn.gamma"), &format!("{dst}.grn_g"))?;
        vec1(&mut p, &mut get, &format!("{src}.grn.beta"), &format!("{dst}.grn_b"))?;
        linear(&mut p, &mut get, &format!("{src}.pwconv2"), &format!("{dst}.pw2"), true)?;
    }
    // cond_emb(x + f0_emb[0]) = W x + (b + W f0_emb[0]): score mode feeds the unvoiced row.
    {
        let (s, w) = get("cfm_decoder.model.cond_emb.weight")?;
        let (_, mut b) = get("cfm_decoder.model.cond_emb.bias")?;
        let (_, f0) = get("f0_encoder.weight")?;
        for o in 0..s[0] {
            let mut acc = 0f64;
            for i in 0..s[1] {
                acc += (w[o * s[1] + i] * f0[i]) as f64;
            }
            b[o] += acc as f32;
        }
        put(&mut p, "cond_emb", s[0], s[1], w);
        put(&mut p, "cond_emb_b", 1, b.len(), b);
    }
    let de = "cfm_decoder.model.diff_estimator";
    for (src, dst) in [("cond_mlp", "dit.cond_mlp"), ("mel_mlp", "dit.mel_mlp"), ("diff_step_mlp", "dit.step_mlp"), ("mel_out_mlp", "dit.out_mlp")] {
        for k in ["0", "2"] {
            linear(&mut p, &mut get, &format!("{de}.{src}.{k}"), &format!("{dst}.{k}"), true)?;
        }
    }
    for l in 0..LAYERS {
        let (src, dst) = (format!("{de}.layers.{l}"), format!("dit.{l}"));
        for (a, b) in [("self_attn.q_proj", "q"), ("self_attn.k_proj", "k"), ("self_attn.v_proj", "v"), ("self_attn.o_proj", "o"), ("mlp.gate_proj", "gate"), ("mlp.up_proj", "up"), ("mlp.down_proj", "down")] {
            linear(&mut p, &mut get, &format!("{src}.{a}"), &format!("{dst}.{b}"), false)?;
        }
        linear(&mut p, &mut get, &format!("{src}.input_layernorm.to_weight"), &format!("{dst}.norm1"), true)?;
        linear(&mut p, &mut get, &format!("{src}.post_attention_layernorm.to_weight"), &format!("{dst}.norm2"), true)?;
    }
    linear(&mut p, &mut get, &format!("{de}.norm.to_weight"), "dit.norm", true)?;
    let vb = "vocoder.model.backbone";
    {
        // Embedding conv [1024, 128, 7] -> [1024, 7 * 128], tap-major.
        let (s, w) = get(&format!("{vb}.embed.weight"))?;
        let (o, i, k) = (s[0], s[1], s[2]);
        let mut packed = vec![0f32; o * k * i];
        for oo in 0..o {
            for kk in 0..k {
                for ii in 0..i {
                    packed[oo * k * i + kk * i + ii] = w[(oo * i + ii) * k + kk];
                }
            }
        }
        put(&mut p, "voc.embed", o, k * i, packed);
        vec1(&mut p, &mut get, &format!("{vb}.embed.bias"), "voc.embed_b")?;
    }
    vec1(&mut p, &mut get, &format!("{vb}.norm.weight"), "voc.ln_w")?;
    vec1(&mut p, &mut get, &format!("{vb}.norm.bias"), "voc.ln_b")?;
    for l in 0..VOC_LAYERS {
        let (src, dst) = (format!("{vb}.convnext.{l}"), format!("voc.{l}"));
        let (s, w) = get(&format!("{src}.dwconv.weight"))?;
        put(&mut p, &format!("{dst}.dw"), s[0], s[2], w);
        vec1(&mut p, &mut get, &format!("{src}.dwconv.bias"), &format!("{dst}.dw_b"))?;
        vec1(&mut p, &mut get, &format!("{src}.norm.weight"), &format!("{dst}.ln_w"))?;
        vec1(&mut p, &mut get, &format!("{src}.norm.bias"), &format!("{dst}.ln_b"))?;
        linear(&mut p, &mut get, &format!("{src}.pwconv1"), &format!("{dst}.pw1"), true)?;
        // Layer scale folded into pwconv2.
        let (_, gamma) = get(&format!("{src}.gamma"))?;
        let (s, mut w2) = get(&format!("{src}.pwconv2.weight"))?;
        let (_, mut b2) = get(&format!("{src}.pwconv2.bias"))?;
        for o in 0..s[0] {
            for i in 0..s[1] {
                w2[o * s[1] + i] *= gamma[o];
            }
            b2[o] *= gamma[o];
        }
        put(&mut p, &format!("{dst}.pw2"), s[0], s[1], w2);
        put(&mut p, &format!("{dst}.pw2_b"), 1, b2.len(), b2);
    }
    vec1(&mut p, &mut get, &format!("{vb}.final_layer_norm.weight"), "voc.final_ln_w")?;
    vec1(&mut p, &mut get, &format!("{vb}.final_layer_norm.bias"), "voc.final_ln_b")?;
    linear(&mut p, &mut get, "vocoder.model.head.out", "voc.head", true)?;
    let (s, mb) = get("mel.model.mel_basis")?;
    put(&mut p, "mel_basis", s[0], s[1], mb);
    let config = vec![
        ("kind".to_string(), "dit".to_string()),
        ("phones".to_string(), phone_set.join(" ")),
        ("voices".to_string(), String::new()),
    ];
    Ok((config, p))
}

/// Add (or replace) a voice in a converted model's params and config.
pub fn add_voice(config: &mut Vec<(String, String)>, params: &mut Params, voice: &Voice) {
    let f = |v: &[u32]| v.iter().map(|x| *x as f32).collect::<Vec<f32>>();
    let n = &voice.name;
    let frames = voice.tokens.frames();
    params.insert(&format!("voice.{n}.mel"), Tensor::new(frames, MELS, voice.mel.clone()));
    params.insert(&format!("voice.{n}.phoneme"), Tensor::new(1, voice.tokens.phoneme.len(), f(&voice.tokens.phoneme)));
    params.insert(&format!("voice.{n}.pitch"), Tensor::new(1, voice.tokens.pitch.len(), f(&voice.tokens.pitch)));
    params.insert(&format!("voice.{n}.kind"), Tensor::new(1, voice.tokens.kind.len(), f(&voice.tokens.kind)));
    params.insert(&format!("voice.{n}.mel2note"), Tensor::new(1, frames, f(&voice.tokens.mel2note)));
    let entry = config.iter_mut().find(|(k, _)| k == "voices");
    let mut names: Vec<String> = entry.as_ref().map(|(_, v)| v.split(',').filter(|s| !s.is_empty()).map(String::from).collect()).unwrap_or_default();
    if !names.contains(n) {
        names.push(n.clone());
    }
    match entry {
        Some(e) => e.1 = names.join(","),
        None => config.push(("voices".into(), names.join(","))),
    }
}

/// Save a converted model: GEMM matrices f16, vectors, voices and the mel
/// filters f32.
pub fn save(path: &Path, config: &[(String, String)], params: &Params) -> std::io::Result<()> {
    weights::write_with(path, config, params, |name, t| {
        if t.rows > 1 && t.cols > 1 && !name.starts_with("voice.") && name != "mel_basis" {
            Dtype::F16
        } else {
            Dtype::F32
        }
    })
}
