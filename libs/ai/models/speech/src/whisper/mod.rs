mod accel;
#[path = "cpu/align.rs"]
mod align;
#[path = "cuda/backend.rs"]
mod cuda_backend;
#[path = "cpu/decode_loop.rs"]
mod decode_loop;
#[path = "cpu/decoder.rs"]
mod decoder;
#[path = "cpu/encoder.rs"]
mod encoder;
#[path = "cpu/mel.rs"]
mod mel;
#[path = "metal/backend.rs"]
mod metal_backend;
#[path = "cpu/model.rs"]
mod model;
#[path = "cpu/quant.rs"]
mod quant;
mod settings;
#[path = "cpu/tensor.rs"]
mod tensor;

pub use accel::backend_name as accel_backend_name;
pub use align::{AlignmentHeads, WordSpan, AUDIO_FRAME_MS};
pub use decode_loop::{AlignedSegment, Segment, WhisperParams, WhisperState};
pub use model::WhisperModel;

use std::sync::atomic::{AtomicU64, Ordering};

/// GPU pacing, for a host that renders on the same GPU while it transcribes.
///
/// By default the Metal encoder submits all its layers as one command buffer:
/// the fastest way through, and a render pass queued behind it waits for the
/// whole stack (tens of milliseconds, a dropped frame or several). With
/// `pieces_per_layer` set, the stack is committed in that many pieces a layer, so
/// other queues' work runs between the pieces; with `duty` under 1, the
/// thread transcribing waits for each piece and then rests so the encoder
/// takes at most that share of the GPU's time. Process-wide; the waiting
/// happens on the calling (worker) thread, never a render thread.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GpuPacing {
    /// Command buffers per encoder layer: 0 = the whole stack in one (the
    /// default), 1 = one per layer, 2 = attention and feed-forward apart.
    pub pieces_per_layer: usize,
    /// The encoder's share of GPU time, 0..=1 (1 = no rest).
    pub duty: f32,
}

static PACING_LAYERS: AtomicU64 = AtomicU64::new(0);
static PACING_DUTY: AtomicU64 = AtomicU64::new(0);

pub fn set_gpu_pacing(pacing: GpuPacing) {
    PACING_LAYERS.store(pacing.pieces_per_layer as u64, Ordering::Relaxed);
    PACING_DUTY.store((pacing.duty.clamp(0.05, 1.0) as f64).to_bits(), Ordering::Relaxed);
}

pub fn gpu_pacing() -> GpuPacing {
    let duty = f64::from_bits(PACING_DUTY.load(Ordering::Relaxed));
    GpuPacing { pieces_per_layer: PACING_LAYERS.load(Ordering::Relaxed) as usize, duty: if duty > 0.0 { duty as f32 } else { 1.0 } }
}

pub(crate) static PROF_MATMUL_RAW: AtomicU64 = AtomicU64::new(0);
pub(crate) static PROF_MATMUL_RAW_CALLS: AtomicU64 = AtomicU64::new(0);
pub(crate) static PROF_MATMUL_T: AtomicU64 = AtomicU64::new(0);
pub(crate) static PROF_MATMUL_T_CALLS: AtomicU64 = AtomicU64::new(0);
pub(crate) static PROF_ENCODER: AtomicU64 = AtomicU64::new(0);
pub(crate) static PROF_ENC_ATTN: AtomicU64 = AtomicU64::new(0);
pub(crate) static PROF_ENC_CONV: AtomicU64 = AtomicU64::new(0);
pub(crate) static PROF_ENC_ELEM: AtomicU64 = AtomicU64::new(0);
pub(crate) static PROF_DECODER: AtomicU64 = AtomicU64::new(0);
pub(crate) static PROF_DECODER_CALLS: AtomicU64 = AtomicU64::new(0);
pub(crate) static PROF_CROSS_KV: AtomicU64 = AtomicU64::new(0);
pub(crate) static PROF_MEL: AtomicU64 = AtomicU64::new(0);

pub fn reset_profiling() {
    PROF_MATMUL_RAW.store(0, Ordering::Relaxed);
    PROF_MATMUL_RAW_CALLS.store(0, Ordering::Relaxed);
    PROF_MATMUL_T.store(0, Ordering::Relaxed);
    PROF_MATMUL_T_CALLS.store(0, Ordering::Relaxed);
    PROF_ENCODER.store(0, Ordering::Relaxed);
    PROF_ENC_ATTN.store(0, Ordering::Relaxed);
    PROF_ENC_CONV.store(0, Ordering::Relaxed);
    PROF_ENC_ELEM.store(0, Ordering::Relaxed);
    PROF_DECODER.store(0, Ordering::Relaxed);
    PROF_DECODER_CALLS.store(0, Ordering::Relaxed);
    PROF_CROSS_KV.store(0, Ordering::Relaxed);
    PROF_MEL.store(0, Ordering::Relaxed);
}

pub fn print_profiling() {
    let to_ms = |v: &AtomicU64| v.load(Ordering::Relaxed) as f64 / 1_000_000.0;
    eprintln!("--- profiling ---");
    eprintln!("  mel:            {:.1}ms", to_ms(&PROF_MEL));
    eprintln!("  encoder:        {:.1}ms", to_ms(&PROF_ENCODER));
    eprintln!("    enc_conv:     {:.1}ms", to_ms(&PROF_ENC_CONV));
    eprintln!("    enc_attn:     {:.1}ms", to_ms(&PROF_ENC_ATTN));
    eprintln!("    enc_elem:     {:.1}ms", to_ms(&PROF_ENC_ELEM));
    eprintln!("  cross_kv:       {:.1}ms", to_ms(&PROF_CROSS_KV));
    eprintln!(
        "  decoder:        {:.1}ms ({} calls)",
        to_ms(&PROF_DECODER),
        PROF_DECODER_CALLS.load(Ordering::Relaxed)
    );
    eprintln!(
        "  matmul_raw:     {:.1}ms ({} calls)",
        to_ms(&PROF_MATMUL_RAW),
        PROF_MATMUL_RAW_CALLS.load(Ordering::Relaxed)
    );
    eprintln!(
        "  matmul_t:       {:.1}ms ({} calls)",
        to_ms(&PROF_MATMUL_T),
        PROF_MATMUL_T_CALLS.load(Ordering::Relaxed)
    );
}

/// The encoder's audio context, in encoder positions (20 ms each; the model's
/// own is 1500 = 30 s), as whisper.cpp's `audio_ctx`: each call encodes only
/// that much audio, so a caller transcribing short windows pays for the
/// window, not for 30 s of padding. The encoder's cost scales with it. A
/// window longer than the context is transcribed in several chunks, as ever.
/// 0 (the default) keeps the model's own. Process-wide; opt in with care:
/// the model was trained on 30 s, so shorter contexts can cost accuracy.
static AUDIO_CTX: AtomicU64 = AtomicU64::new(0);

pub fn set_audio_ctx(positions: usize) {
    AUDIO_CTX.store(positions as u64, Ordering::Relaxed);
}

/// The context a call on `model` uses: the one set, within the model's own.
pub(crate) fn audio_ctx(model_ctx: usize) -> usize {
    match AUDIO_CTX.load(Ordering::Relaxed) as usize {
        0 => model_ctx,
        n => n.clamp(64, model_ctx),
    }
}
