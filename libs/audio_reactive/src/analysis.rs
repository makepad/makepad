//! The analysis: live sound as a sampleable picture plus the signals a
//! look or a game reads (levels, onsets, loudness drive, song form).
//!
//! ## THE SOURCE SEAM (one stream, no picker)
//!
//! The analyser does not own a capture ring. Callers hand it mono `f32`
//! samples at a known rate through [`AudioReactive::pump`] (`samples:
//! &[f32]`, `rate: f32`), or interleaved stereo through
//! [`AudioReactive::pump_stereo`]. Empty slices are idle: the texture keeps
//! its last state and envelopes decay. [`AudioReactive::push_samples`] is
//! the same sample seam without the per-frame upload.
//!
//! ## THE TEXTURE CONTRACT (what a shader sees)
//!
//! Format `VecRf32` (one f32 per texel), **`AUDIO_TEX_W` x `AUDIO_TEX_H`**
//! = 256 x 384. All values are 0..1 unless signed. Stacked RINGS whose
//! newest row is named by a uniform:
//!
//! ```text
//!  y = 0 .. 255   SPECTROGRAM ring  (AUDIO_SPEC_ROWS = 256 rows)
//!                 x = log-spaced FFT bin, 0 = 30 Hz .. 255 = 16 kHz
//!                 value = normalised magnitude, 0 = -72 dBFS, 1 = 0 dBFS
//!                 one row per HOP (1024 samples ~ 21.3 ms @48k)
//!                 -> 256 rows ~ 5.5 s of spectrum history
//!  y = 256 .. 319 WAVEFORM ring     (AUDIO_WAVE_ROWS = 64 rows)
//!                 x = time within the row, left = older
//!                 value = the SIGNED sample, -1..1 (0 = silence — which
//!                 is also what an UNBOUND texture slot reads)
//!                 one row per HOP, 256 peak-decimated points per row
//!                 -> 64 * 1024 samples ~ 1.37 s of waveform history
//!  y = 320 .. 383 SIDE ring         (AUDIO_SIDE_ROWS = 64 rows)
//!                 the stereo SIDE signal (L - R) / 2, signed, on the SAME
//!                 points and cursor as the waveform ring (which holds the
//!                 MID (L + R) / 2 when a stereo source feeds
//!                 `pump_stereo`). All 0 for a mono source: L = R = mid.
//! ```
//!
//! Uniforms published beside the texture ([`bind_audio`]):
//!
//! ```text
//!  audio_dim  = (bins, spec_rows, spec_cursor, wave_cursor)
//!  audio_meta = (tex_w, tex_h, wave_rows, hop_secs)
//!  audio_env  = (bass, mid, high, rms)   smoothed 0..1, no texture read
//!  audio_hit  = (kick, snare, hat, loud) onsets and loudness 0..1
//!  audio_norm = auto-gained (bass, mid, high, rms) 0..1
//!  audio_form = (drop, section, 0, 0)    song form
//! ```
//!
//! `spec_cursor` / `wave_cursor` are the row indices of the NEWEST row in
//! each ring — the unwrap key: row `(cursor - n) mod rows` is `n` hops ago.
//! [`crate::SPLASH`] wraps all of that in helpers (`audio_fft(f, age)`,
//! `audio_wave(t)`, `audio_wave2(t)`), so a shader never does ring math.
//!
//! ## SILENCE IS A VALID PICTURE
//!
//! With nothing pushed the whole texture stays 0, which reads as "no
//! spectrum, flat waveform" — the same thing an unbound texture slot reads.
//! That is a legal, sane picture for every helper: no NaNs, no divide by
//! zero, never a false full-scale. Looks carry their own idle floor off
//! their clock so a silent rig still performs.

use makepad_audio_picture::fft::Fft;
use makepad_draw::*;

/// The linear-amplitude perceptual curve of the rms level.
const WAVE_CURVE: f32 = 0.62;

/// Texture width: log-spaced FFT bins per spectrogram row, and points per
/// waveform row.
pub const AUDIO_BINS: usize = 256;
/// Spectrogram history rows (one per hop).
pub const AUDIO_SPEC_ROWS: usize = 256;
/// Waveform history rows (one per hop).
pub const AUDIO_WAVE_ROWS: usize = 64;
/// Texture height: the two sections stacked.
pub const AUDIO_TEX_H: usize = AUDIO_SPEC_ROWS + AUDIO_WAVE_ROWS + AUDIO_SIDE_ROWS;
/// Stereo side-signal history rows (same cursor as the waveform ring).
pub const AUDIO_SIDE_ROWS: usize = AUDIO_WAVE_ROWS;
/// Texture width.
pub const AUDIO_TEX_W: usize = AUDIO_BINS;

/// Analysis window, samples (power of two — the FFT is radix-2).
const FFT_SIZE: usize = 2048;
/// Samples between rows. One hop = one spectrogram row + one waveform row.
const HOP: usize = 1024;
/// Waveform decimation: `HOP / AUDIO_BINS` samples per stored point, kept
/// as the peak of the group so a transient never disappears between rows.
const WAVE_DECIM: usize = HOP / AUDIO_BINS;

/// Lowest / highest frequency the log bin ladder spans.
const F_MIN: f32 = 30.0;
const F_MAX: f32 = 16_000.0;
/// Dynamic range of the normalised magnitude: 0 in the texture is this many
/// dB below full scale.
const RANGE_DB: f32 = 72.0;
/// Display gamma on the normalised magnitude (matches the offline
/// spectrogram's curve; the app's linear-amplitude curve is `WAVE_CURVE`,
/// used for the envelope row).
const SPEC_GAMMA: f32 = 0.85;

// ---------------------------------------------------------------------------
// THE HIT SIGNALS — onsets and loudness drive, computed per hop beside the
// spectrogram row (CPU, a few hundred flops per hop, no allocation).
//
// The smoothed band levels (`audio_env`) are dB-normalised peaks: on a
// mastered track a kick moves `bass` by a few hundredths, so they cannot
// drive a picture. Onsets can: per band, the SPECTRAL FLUX (the sum of
// positive bin rises since the previous row) is compared against its own
// running mean, and the excess is auto-gained against its recent peak, so
// a real kick reads ~1 whatever the master level. Each onset is held as a
// decaying envelope (kick ~120 ms, snare ~100 ms, hat ~70 ms).
//
// `loud` = short-term rms against its own recent maximum (~10 s release):
// a breakdown falls toward 0, a drop reads ~1. `lift` = short/(short+long)
// energy: 0.5 steady, above = building or dropping in.
//
// Published in `AudioFrame::hit` = (kick, snare, hat, loud) and
// `AudioFrame::lift`; every family shader reads them as `audio_hit` /
// `audio_drive` (see `bind_frame`), and the binding language as the
// signals `kick snare hat loud lift` (expr.rs).
// ---------------------------------------------------------------------------

/// Band edges for the onset detectors (Hz).
const KICK_HI: f32 = 150.0;
const SNARE_LO: f32 = 200.0;
const SNARE_HI: f32 = 2_800.0;
const HAT_LO: f32 = 6_000.0;
/// Onset envelope release times (s).
const HIT_RELEASE: [f32; 3] = [0.12, 0.10, 0.07];

/// One band's onset detector.
#[derive(Clone, Copy, Default)]
struct Onset {
    /// Running mean of the flux (the threshold's base).
    mean: f32,
    /// Recent peak of the above-threshold excess (the auto-gain).
    peak: f32,
    /// The output envelope 0..1.
    env: f32,
}

impl Onset {
    fn step(&mut self, flux: f32, hop_secs: f32, release: f32, gate: f32) {
        let excess = (flux - self.mean * 1.4).max(0.0);
        self.peak = excess.max(self.peak * (-hop_secs / 4.0).exp()).max(0.004);
        let strength = if gate > 0.0 { (excess / self.peak).clamp(0.0, 1.0) * gate } else { 0.0 };
        self.env = strength.max(self.env * (-hop_secs / release).exp());
        let a = (hop_secs / 0.45).clamp(0.0, 1.0);
        self.mean += (flux - self.mean) * a;
    }
}

/// One frame of the analysis: everything a draw call needs to bind the
/// audio texture, and the signals a host feeds its documents. Cheap to
/// clone (the texture is a handle).
#[derive(Clone)]
pub struct AudioFrame {
    pub tex: Texture,
    /// (bins, spec_rows, spec_cursor, wave_cursor)
    pub dim: Vec4f,
    /// (tex_w, tex_h, wave_rows, hop_secs)
    pub meta: Vec4f,
    /// (bass, mid, high, rms), smoothed 0..1
    pub env: Vec4f,
    /// The levels (energy, bass, mid, high) a host's signals read.
    pub levels: [f32; 4],
    /// THE HIT SIGNALS: (kick, snare, hat, loud), each 0..1.
    pub hit: Vec4f,
    /// Short/long energy ratio, 0.5 = steady.
    pub lift: f32,
    /// A stereo source feeds the side ring (`audio_wave2` is real).
    pub stereo: bool,
    /// AUTO-GAINED (bass, mid, high, rms) 0..1: the levels against their
    /// own recent floor/ceiling, so they swing on mastered music.
    pub norm: Vec4f,
    /// SONG FORM: (drop 0..1 — 1 on a re-entry after a breakdown, ~3 s
    /// decay; section — the ~6 s energy vs the recent max, 0..1; 0; 0).
    pub form: Vec4f,
    /// THE SPECTRUM BY BAND: the newest row folded into [`SPECTRUM_BANDS`]
    /// log-spaced bands (each its bins' peak, 0..1, lows first), for a
    /// kernel or a CPU look that reads levels per band.
    pub bands: [f32; SPECTRUM_BANDS],
}

/// How many bands [`AudioFrame::bands`] folds the spectrum into.
pub const SPECTRUM_BANDS: usize = 32;

/// Bind the audio texture and its uniforms (the picture's dims, the levels,
/// the onsets, the auto-gained levels, the song form) onto ONE draw call.
///
/// Resolution is BY NAME, not by slot index: the texture lands in whatever
/// slot the shader declared `audio_tex` in (families declare a different
/// number of textures before it), and a shader that declares none is a
/// silent no-op. That is what lets one call site serve every engine.
pub fn bind_audio(cx: &Cx, dv: &mut DrawVars, a: &AudioFrame) {
    if let Some(sid) = dv.draw_shader_id {
        let sh = &cx.draw_shaders[sid.index];
        if let Some(slot) = sh.mapping.textures.iter().position(|t| t.id == live_id!(audio_tex)) {
            dv.set_texture(slot, &a.tex);
        }
    }
    dv.set_uniform(cx, live_id!(audio_dim), &[a.dim.x, a.dim.y, a.dim.z, a.dim.w]);
    dv.set_uniform(cx, live_id!(audio_meta), &[a.meta.x, a.meta.y, a.meta.z, a.meta.w]);
    dv.set_uniform(cx, live_id!(audio_env), &[a.env.x, a.env.y, a.env.z, a.env.w]);
    dv.set_uniform(cx, live_id!(audio_hit), &[a.hit.x, a.hit.y, a.hit.z, a.hit.w]);
    dv.set_uniform(cx, live_id!(audio_norm), &[a.norm.x, a.norm.y, a.norm.z, a.norm.w]);
    dv.set_uniform(cx, live_id!(audio_form), &[a.form.x, a.form.y, a.form.z, a.form.w]);
}

/// The per-frame audio picture: analyser + texture + the publish surface.
///
/// One of these lives on the app (a show, a game); every consumer is handed
/// the same [`AudioFrame`], so everything reads one coherent analysis.
pub struct AudioReactive {
    tex: Option<Texture>,
    /// The texel staging buffer (`AUDIO_TEX_W * AUDIO_TEX_H`), swapped in
    /// and out of the texture so a frame allocates nothing.
    data: Vec<f32>,
    /// Rows written so far; `% ROWS` gives the newest row of each ring.
    rows_written: u64,
    /// Monotonic sample cursor into the capture ring (our own; the
    /// beat-sync worker's `tail` is never touched).
    cursor: usize,
    /// Samples pulled but not yet consumed by a hop.
    pending: Vec<f32>,
    /// The side signal beside `pending` (stereo sources only; kept the
    /// same length as `pending` while stereo).
    pending_side: Vec<f32>,
    /// Hops analysed from a stereo source recently (0 = mono now).
    stereo_hops: u32,
    /// The last `FFT_SIZE` samples, ring-free (rotated on each hop).
    window: Vec<f32>,
    scratch: Vec<f32>,
    /// The windowed transform (Hann, periodic) and its bin magnitudes.
    fft: Fft,
    mags: Vec<f32>,
    /// Per-output-bin FFT bin span, rebuilt when the sample rate changes.
    bin_lo: Vec<u16>,
    bin_hi: Vec<u16>,
    rate: f32,
    /// Smoothed (bass, mid, high, rms) — attack fast, release slow.
    env: [f32; 4],
    /// True once at least one hop has been analysed.
    seen_audio: bool,
    /// Onset detectors: kick, snare, hat.
    onsets: [Onset; 3],
    /// Loudness followers (linear rms): short (~0.8 s), long (~6 s), and
    /// the recent maximum (~10 s release).
    loud_short: f32,
    loud_long: f32,
    loud_max: f32,
    /// AUTO-GAIN of the four levels (bass, mid, high, rms): slow floor and
    /// ceiling followers, so `norm` swings ~0..1 on a mastered track.
    agc_lo: [f32; 4],
    agc_hi: [f32; 4],
    /// DROP detector: the recent low of `loud` (relaxes upward ~4 s), the
    /// drop envelope (1 on a re-entry, ~3 s decay) and a re-arm cooldown.
    loud_low: f32,
    drop_env: f32,
    drop_cool: f32,
}

impl Default for AudioReactive {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioReactive {
    pub fn new() -> AudioReactive {
        // Zero is a legal reading everywhere: no spectrum, flat waveform.
        let data = vec![0.0f32; AUDIO_TEX_W * AUDIO_TEX_H];
        AudioReactive {
            tex: None,
            data,
            rows_written: 0,
            cursor: 0,
            pending: Vec::with_capacity(HOP * 4),
            pending_side: Vec::with_capacity(HOP * 4),
            stereo_hops: 0,
            window: vec![0.0; FFT_SIZE],
            scratch: Vec::with_capacity(HOP * 8),
            fft: Fft::new(FFT_SIZE),
            mags: vec![0.0; FFT_SIZE / 2],
            bin_lo: vec![0; AUDIO_BINS],
            bin_hi: vec![0; AUDIO_BINS],
            rate: 0.0,
            env: [0.0; 4],
            seen_audio: false,
            onsets: [Onset::default(); 3],
            loud_short: 0.0,
            loud_long: 0.0,
            loud_max: 0.0,
            agc_lo: [1.0; 4],
            agc_hi: [0.0; 4],
            loud_low: 0.0,
            drop_env: 0.0,
            drop_cool: 0.0,
        }
    }

    /// THE FRAME CALL, source-agnostic: the caller hands whatever mono
    /// samples its source gained since the last frame (empty while idle —
    /// the texture then holds its last state and the envelopes decay). The
    /// capture-ring tap itself lives with the caller (main.rs), so this
    /// module stays free of main-binary types and compiles inside the
    /// effect_gallery example too.
    pub fn pump(&mut self, cx: &mut Cx, samples: &[f32], rate: f32) {
        let before = self.rows_written;
        if !samples.is_empty() {
            self.push_samples(samples, rate);
        }
        if self.rows_written == before {
            self.decay_if_starved();
        }
        self.upload(cx);
    }

    /// The stereo frame call: `interleaved` L,R pairs. The analysis runs
    /// on the MID (so every mono consumer reads exactly what it did), and
    /// the SIDE lands in its own ring for `audio_wave2`.
    pub fn pump_stereo(&mut self, cx: &mut Cx, interleaved: &[f32], rate: f32) {
        let before = self.rows_written;
        if interleaved.len() >= 2 {
            self.push_samples_stereo(interleaved, rate);
        }
        if self.rows_written == before {
            self.decay_if_starved();
        }
        self.upload(cx);
    }

    /// [`Self::push_samples`] for interleaved stereo (L, R, L, R, ...).
    pub fn push_samples_stereo(&mut self, interleaved: &[f32], rate: f32) {
        if !(rate.is_finite() && rate >= 8_000.0) {
            return;
        }
        if self.pending_side.len() != self.pending.len() {
            self.pending_side.clear();
            self.pending_side.resize(self.pending.len(), 0.0);
        }
        let fin = |v: f32| if v.is_finite() { v } else { 0.0 };
        for pair in interleaved.chunks_exact(2) {
            let (l, r) = (fin(pair[0]), fin(pair[1]));
            self.pending.push((l + r) * 0.5);
            self.pending_side.push((l - r) * 0.5);
        }
        self.stereo_hops = 64;
        self.consume(rate);
    }

    /// The caller-owned tap cursor into whatever ring it reads (monotonic,
    /// starts at 0). Held here so the bus survives a source swap without
    /// the app growing bookkeeping fields.
    pub fn tap_cursor(&self) -> usize {
        self.cursor
    }

    pub fn set_tap_cursor(&mut self, cursor: usize) {
        self.cursor = cursor;
    }

    /// Scratch buffer loan for the caller's per-frame ring read — cleared
    /// on loan, returned via [`Self::return_scratch`].
    pub fn take_scratch(&mut self) -> Vec<f32> {
        let mut scratch = std::mem::take(&mut self.scratch);
        scratch.clear();
        scratch
    }

    pub fn return_scratch(&mut self, scratch: Vec<f32>) {
        self.scratch = scratch;
    }

    /// THE SOURCE SEAM: hand the analyser mono samples at a known rate.
    /// Everything above this line is just "where the samples came from".
    pub fn push_samples(&mut self, samples: &[f32], rate: f32) {
        if !(rate.is_finite() && rate >= 8_000.0) {
            return;
        }
        // (rate changes and the stalled-consumer guard live in `consume`:
        // bounded work per frame is the law here.)
        for s in samples {
            self.pending.push(if s.is_finite() { *s } else { 0.0 });
        }
        // A mono source: the side stays silent.
        self.pending_side.clear();
        self.consume(rate);
    }

    fn consume(&mut self, rate: f32) {
        if (self.rate - rate).abs() > 0.5 {
            self.rate = rate;
            self.rebuild_bins();
        }
        if self.pending.len() > HOP * 16 {
            let keep = self.pending.len() - HOP * 4;
            self.pending.drain(..keep);
            if self.pending_side.len() >= keep {
                self.pending_side.drain(..keep);
            }
        }
        while self.pending.len() >= HOP {
            let hop: Vec<f32> = self.pending.drain(..HOP).collect();
            let side: Option<Vec<f32>> = if self.pending_side.len() >= HOP {
                Some(self.pending_side.drain(..HOP).collect())
            } else {
                self.pending_side.clear();
                None
            };
            self.analyse_hop(&hop, side.as_deref());
        }
    }

    /// This frame's analysis. `None` before the texture exists (nothing has
    /// pumped yet).
    pub fn binding(&self) -> Option<AudioFrame> {
        let tex = self.tex.as_ref()?;
        // The cursor names the row LAST WRITTEN (the newest); before any
        // hop exists both rings are uniform anyway, so row 0 is honest.
        let newest = self.rows_written.saturating_sub(1);
        let spec_cursor = (newest % AUDIO_SPEC_ROWS as u64) as f32;
        let wave_cursor = (newest % AUDIO_WAVE_ROWS as u64) as f32;
        let hop_secs = if self.rate > 0.0 { HOP as f32 / self.rate } else { HOP as f32 / 48_000.0 };
        Some(AudioFrame {
            tex: tex.clone(),
            dim: vec4(AUDIO_BINS as f32, AUDIO_SPEC_ROWS as f32, spec_cursor, wave_cursor),
            meta: vec4(AUDIO_TEX_W as f32, AUDIO_TEX_H as f32, AUDIO_WAVE_ROWS as f32, hop_secs),
            env: vec4(self.env[0], self.env[1], self.env[2], self.env[3]),
            levels: [self.env[3], self.env[0], self.env[1], self.env[2]],
            hit: vec4(self.onsets[0].env, self.onsets[1].env, self.onsets[2].env, self.loud()),
            lift: self.lift(),
            stereo: self.is_stereo(),
            form: vec4(
                self.drop_env,
                if self.loud_max > 1.0e-4 { (self.loud_long / self.loud_max).clamp(0.0, 1.0) } else { 0.0 },
                0.0,
                0.0,
            ),
            norm: {
                let n = |k: usize| {
                    let span = (self.agc_hi[k] - self.agc_lo[k]).max(0.06);
                    ((self.env[k] - self.agc_lo[k]) / span).clamp(0.0, 1.0)
                };
                vec4(n(0), n(1), n(2), n(3))
            },
            bands: {
                let mut b = [0.0; SPECTRUM_BANDS];
                self.bands(&mut b);
                b
            },
        })
    }

    /// Loudness against its recent maximum, 0..1.
    fn loud(&self) -> f32 {
        if self.loud_max <= 1.0e-4 {
            return 0.0;
        }
        (self.loud_short / self.loud_max).clamp(0.0, 1.0)
    }

    /// True while a stereo source feeds the side ring.
    pub fn is_stereo(&self) -> bool {
        self.stereo_hops > 0
    }

    /// Short/(short+long) energy, 0.5 = steady.
    fn lift(&self) -> f32 {
        let sum = self.loud_short + self.loud_long;
        if sum <= 1.0e-5 {
            return 0.5;
        }
        (self.loud_short / sum).clamp(0.0, 1.0)
    }

    /// The newest spectrogram row: `AUDIO_BINS` log-spaced magnitudes 0..1
    /// (30 Hz .. 16 kHz), all 0 before any audio.
    pub fn spectrum(&self) -> &[f32] {
        let newest = self.rows_written.saturating_sub(1);
        let row = (newest % AUDIO_SPEC_ROWS as u64) as usize * AUDIO_TEX_W;
        &self.data[row..row + AUDIO_BINS]
    }

    /// The newest spectrum folded into `out.len()` log-spaced bands (each
    /// the peak of its bins), for a kernel, a Sim or a CPU look that reads
    /// levels per band rather than the texture.
    pub fn bands(&self, out: &mut [f32]) {
        let row = self.spectrum();
        let n = out.len().max(1);
        for (k, v) in out.iter_mut().enumerate() {
            let lo = k * AUDIO_BINS / n;
            let hi = ((k + 1) * AUDIO_BINS / n).max(lo + 1);
            *v = row[lo..hi].iter().cloned().fold(0.0, f32::max);
        }
    }

    /// True once real audio has been analysed at least once (status/debug).
    pub fn has_audio(&self) -> bool {
        self.seen_audio
    }

    // -----------------------------------------------------------------
    // analysis
    // -----------------------------------------------------------------

    /// Log-spaced bin ladder: output bin j covers the FFT bins between the
    /// geometric centres of j-1..j and j..j+1, and takes their MAX (a peak
    /// hold, so a narrow tone never vanishes into a wide high-frequency
    /// band). Rebuilt only when the device rate changes.
    fn rebuild_bins(&mut self) {
        let nyq = self.rate * 0.5;
        let f_max = F_MAX.min(nyq * 0.98).max(F_MIN * 2.0);
        let half = FFT_SIZE / 2;
        let hz_per_bin = self.rate / FFT_SIZE as f32;
        let ratio = (f_max / F_MIN).ln();
        let centre = |j: f32| F_MIN * (ratio * j / (AUDIO_BINS as f32 - 1.0)).exp();
        for j in 0..AUDIO_BINS {
            let f = centre(j as f32);
            let lo_f = if j == 0 { F_MIN } else { (f * centre(j as f32 - 1.0)).sqrt() };
            let hi_f = if j + 1 == AUDIO_BINS {
                f_max
            } else {
                (f * centre(j as f32 + 1.0)).sqrt()
            };
            let lo = ((lo_f / hz_per_bin).floor() as isize).clamp(1, half as isize - 1) as u16;
            let hi = ((hi_f / hz_per_bin).ceil() as isize).clamp(lo as isize + 1, half as isize)
                as u16;
            self.bin_lo[j] = lo;
            self.bin_hi[j] = hi;
        }
    }

    fn analyse_hop(&mut self, hop: &[f32], side: Option<&[f32]>) {
        // Slide the analysis window: drop the oldest HOP, append the new one.
        self.window.copy_within(HOP.., 0);
        self.window[FFT_SIZE - HOP..].copy_from_slice(hop);

        // ---- waveform row: peak-decimated, stored SIGNED ----
        let wrow = (self.rows_written % AUDIO_WAVE_ROWS as u64) as usize;
        let wbase = (AUDIO_SPEC_ROWS + wrow) * AUDIO_TEX_W;
        let sbase_side = (AUDIO_SPEC_ROWS + AUDIO_WAVE_ROWS + wrow) * AUDIO_TEX_W;
        for x in 0..AUDIO_BINS {
            let mut peak = 0.0f32;
            let mut at = x * WAVE_DECIM;
            for k in 0..WAVE_DECIM {
                let s = hop[x * WAVE_DECIM + k];
                if s.abs() > peak.abs() {
                    peak = s;
                    at = x * WAVE_DECIM + k;
                }
            }
            self.data[wbase + x] = peak.clamp(-1.0, 1.0);
            // The side sample at the SAME instant as the mid peak, so
            // (mid + side, mid - side) is a real (L, R) pair.
            self.data[sbase_side + x] = side.map(|sd| sd[at].clamp(-1.0, 1.0)).unwrap_or(0.0);
        }
        if side.is_none() {
            self.stereo_hops = self.stereo_hops.saturating_sub(1);
        }

        // ---- spectrogram row ----
        // Magnitudes relative to a full-scale sine (1.0 in its bin): the
        // row is an ABSOLUTE level.
        self.fft.magnitudes(&self.window, &mut self.mags);
        let srow = (self.rows_written % AUDIO_SPEC_ROWS as u64) as usize;
        let sbase = srow * AUDIO_TEX_W;
        // Band accumulators, split on the crossovers the beat detector uses.
        let hz_per_bin = if self.rate > 0.0 { self.rate / FFT_SIZE as f32 } else { 23.4 };
        let (mut bass, mut mid, mut high) = (0.0f32, 0.0f32, 0.0f32);
        // The previous row (the one written last hop) for the flux.
        let prow = ((self.rows_written + AUDIO_SPEC_ROWS as u64 - 1) % AUDIO_SPEC_ROWS as u64) as usize;
        let pbase = prow * AUDIO_TEX_W;
        let mut flux = [0.0f32; 3];
        let mut flux_n = [0u32; 3];
        for j in 0..AUDIO_BINS {
            let lo = self.bin_lo[j] as usize;
            let hi = (self.bin_hi[j] as usize).min(FFT_SIZE / 2);
            let mut mag = 0.0f32;
            for b in lo..hi.max(lo + 1) {
                mag = mag.max(self.mags[b.min(self.mags.len() - 1)]);
            }
            let norm = mag.max(1e-9);
            let db = 20.0 * norm.log10();
            let v = ((db + RANGE_DB) / RANGE_DB).clamp(0.0, 1.0).powf(SPEC_GAMMA);
            let rise = (v - self.data[pbase + j]).max(0.0);
            self.data[sbase + j] = v;
            let f = (lo as f32 + 0.5) * hz_per_bin;
            let band = if f < KICK_HI {
                Some(0)
            } else if (SNARE_LO..SNARE_HI).contains(&f) {
                Some(1)
            } else if f >= HAT_LO {
                Some(2)
            } else {
                None
            };
            if let Some(b) = band {
                flux[b] += rise;
                flux_n[b] += 1;
            }
            if f < 170.0 {
                bass = bass.max(v);
            } else if f < 2800.0 {
                mid = mid.max(v);
            } else {
                high = high.max(v);
            }
        }

        // ---- envelopes: attack fast, release slow (no accumulator ever
        // reaches a shader; these are bounded 0..1 followers) ----
        let mut sum = 0.0f64;
        for s in hop {
            sum += (*s as f64) * (*s as f64);
        }
        let rms = ((sum / hop.len() as f64).sqrt() as f32).clamp(0.0, 1.0).powf(WAVE_CURVE);
        for (slot, target) in [bass, mid, high, rms].into_iter().enumerate() {
            let cur = self.env[slot];
            let a = if target > cur { 0.55 } else { 0.12 };
            self.env[slot] = cur + (target - cur) * a;
        }

        // ---- the hit signals (see "THE HIT SIGNALS") ----
        let hop_secs = if self.rate > 0.0 { HOP as f32 / self.rate } else { HOP as f32 / 48_000.0 };
        let lin = ((sum / hop.len() as f64).sqrt() as f32).clamp(0.0, 1.0);
        // Below about -60 dBFS nothing counts as a hit (dither, room tone).
        let gate = ((lin - 0.001) / 0.004).clamp(0.0, 1.0);
        if self.rows_written > 0 {
            for b in 0..3 {
                let f = if flux_n[b] > 0 { flux[b] / flux_n[b] as f32 } else { 0.0 };
                self.onsets[b].step(f, hop_secs, HIT_RELEASE[b], gate);
            }
        }
        let a_short = (hop_secs / 0.8).clamp(0.0, 1.0);
        let a_long = (hop_secs / 6.0).clamp(0.0, 1.0);
        self.loud_short += (lin - self.loud_short) * a_short;
        self.loud_long += (lin - self.loud_long) * a_long;
        self.loud_max = self.loud_short.max(self.loud_max * (-hop_secs / 10.0).exp());
        // DROP: loudness jumping far above its recent low (a re-entry after
        // a breakdown) fires a 1 that decays over ~3 s; re-arms after 6 s.
        let loud_now = self.loud();
        self.loud_low = loud_now.min(self.loud_low + hop_secs / 4.0);
        self.drop_env *= (-hop_secs / 3.0).exp();
        self.drop_cool = (self.drop_cool - hop_secs).max(0.0);
        if self.drop_cool <= 0.0 && loud_now > 0.65 && loud_now - self.loud_low > 0.4 {
            self.drop_env = 1.0;
            self.drop_cool = 6.0;
            self.loud_low = loud_now;
        }
        // Level auto-gain: floor/ceiling snap outward instantly and relax
        // inward over ~8 s, so the normalised level uses the whole range
        // of what the music has recently done.
        let relax = (hop_secs / 8.0).clamp(0.0, 1.0);
        for k in 0..4 {
            let v = self.env[k];
            self.agc_lo[k] = if v < self.agc_lo[k] { v } else { self.agc_lo[k] + (v - self.agc_lo[k]) * relax };
            self.agc_hi[k] = if v > self.agc_hi[k] { v } else { self.agc_hi[k] + (v - self.agc_hi[k]) * relax };
        }

        self.rows_written = self.rows_written.wrapping_add(1);
        self.seen_audio = true;
    }

    /// No samples arrived this frame: let the envelopes fall so a stopped
    /// show does not sit on a stale "loud" reading forever. The rings keep
    /// their history — that IS the last few seconds of the show.
    fn decay_if_starved(&mut self) {
        for v in &mut self.env {
            *v *= 0.94;
            if *v < 1e-4 {
                *v = 0.0;
            }
        }
        for o in &mut self.onsets {
            o.env *= 0.8;
            if o.env < 1e-4 {
                o.env = 0.0;
            }
        }
        self.loud_short *= 0.94;
        self.loud_long *= 0.995;
        self.drop_env *= 0.97;
    }

    fn upload(&mut self, cx: &mut Cx) {
        match &self.tex {
            Some(tex) => {
                // The rings in `self.data` are authoritative; the texture
                // gets a copy. Take its buffer back, overwrite, hand it in —
                // one memcpy, no allocation per frame.
                let mut held = tex.take_vec_f32(cx);
                if held.len() != self.data.len() {
                    held.resize(self.data.len(), 0.0);
                }
                held.copy_from_slice(&self.data);
                tex.put_back_vec_f32(cx, held, None);
            }
            None => {
                self.tex = Some(Texture::new_with_format(
                    cx,
                    TextureFormat::VecRf32 {
                        width: AUDIO_TEX_W,
                        height: AUDIO_TEX_H,
                        data: Some(self.data.clone()),
                        updated: TextureUpdated::Full,
                    },
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 1 kHz full-scale sine must land near the top of the normalised
    /// scale in the bin that covers it, and silence must land at the floor.
    #[test]
    fn spectrum_row_is_absolute() {
        let mut bus = AudioReactive::new();
        let rate = 48_000.0f32;
        let sine: Vec<f32> = (0..HOP * 8)
            .map(|i| (std::f32::consts::TAU * 1000.0 * i as f32 / rate).sin())
            .collect();
        bus.push_samples(&sine, rate);
        let row = (bus.rows_written - 1) as usize % AUDIO_SPEC_ROWS;
        let base = row * AUDIO_TEX_W;
        let peak = bus.data[base..base + AUDIO_BINS].iter().cloned().fold(0.0f32, f32::max);
        assert!(peak > 0.9, "1 kHz sine peaked at {peak}");

        let mut quiet = AudioReactive::new();
        quiet.push_samples(&vec![0.0f32; HOP * 4], rate);
        let row = (quiet.rows_written - 1) as usize % AUDIO_SPEC_ROWS;
        let base = row * AUDIO_TEX_W;
        let peak = quiet.data[base..base + AUDIO_BINS].iter().cloned().fold(0.0f32, f32::max);
        assert!(peak < 0.02, "silence peaked at {peak}");
    }

    /// Bands fold the newest row: a 1 kHz tone lands in the band that
    /// holds 1 kHz and not far from it.
    #[test]
    fn bands_fold_the_newest_row() {
        let mut bus = AudioReactive::new();
        let rate = 48_000.0f32;
        let sine: Vec<f32> = (0..HOP * 8).map(|i| (std::f32::consts::TAU * 1000.0 * i as f32 / rate).sin()).collect();
        bus.push_samples(&sine, rate);
        let mut bands = [0.0f32; 8];
        bus.bands(&mut bands);
        let loudest = (0..8).max_by(|a, b| bands[*a].total_cmp(&bands[*b])).unwrap();
        // 1 kHz sits at (ln(1000/30) / ln(16000/30)) ~ 0.56 of the ladder.
        assert_eq!(loudest, 4, "{bands:?}");
        assert!(bands[0] < bands[4] * 0.5, "{bands:?}");
    }

    /// The waveform section is signed, bounded, and silent-is-zero — the
    /// same reading an unbound texture slot gives, by design.
    #[test]
    fn wave_row_is_signed_and_bounded() {
        let mut bus = AudioReactive::new();
        let rate = 48_000.0f32;
        let sine: Vec<f32> = (0..HOP * 2)
            .map(|i| (std::f32::consts::TAU * 200.0 * i as f32 / rate).sin())
            .collect();
        bus.push_samples(&sine, rate);
        let row = (bus.rows_written - 1) as usize % AUDIO_WAVE_ROWS;
        let base = (AUDIO_SPEC_ROWS + row) * AUDIO_TEX_W;
        let slice = &bus.data[base..base + AUDIO_BINS];
        assert!(slice.iter().all(|v| (-1.0..=1.0).contains(v)));
        let lo = slice.iter().cloned().fold(1.0f32, f32::min);
        let hi = slice.iter().cloned().fold(-1.0f32, f32::max);
        assert!(hi > 0.8 && lo < -0.8, "sine spanned {lo}..{hi}");
        // A fresh bus (nothing pushed) is a flat line at zero — exactly
        // what a shader reads when no audio texture is bound at all.
        let idle = AudioReactive::new();
        assert!(idle.data.iter().all(|v| *v == 0.0));
    }
}
