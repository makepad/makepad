//! # THE AUDIO TEXTURE — live sound as a sampleable picture
//!
//! One small float texture, rewritten every frame, that every effect shader
//! can read. It is the ONLY way audio waveform/spectrum data reaches the
//! GPU side of the effect system, and it is bound at the ENGINES layer (see
//! [`bind_audio`] and its call sites in `view.rs`), so a doc-authored hook
//! in ANY family samples it the same way — not a special case on one shader.
//!
//! ## THE SOURCE SEAM (one stream, no picker)
//!
//! The show's audio is **exactly what the beat-sync analysis is listening
//! to** — the mono capture ring in `main.rs` (`CaptureFeed`), fed by the
//! loopback audio-input callback. There is no source selector and no UI:
//! whatever beat sync hears, the visualisers see.
//!
//! The tap is READ-ONLY. [`crate::CaptureFeed::peek_since`] copies out of
//! the ring using its OWN monotonic cursor and never touches `tail`, so the
//! beat-sync worker stays the single consumer/authority over the ring; a
//! visualiser that falls behind silently skips forward instead of applying
//! backpressure to the detector.
//!
//! The seam for future sources is [`AudioTexBus::push_samples`]: anything
//! that can hand over mono f32 at a known rate (a mixer-master tap, a file
//! scrub, a test generator) feeds the same analyser. [`AudioTexBus::pump`]
//! is just "pull from the beat-sync feed, then push".
//!
//! ## THE TEXTURE CONTRACT (what a shader sees)
//!
//! Format `VecRf32` (one f32 per texel), **`AUDIO_TEX_W` x `AUDIO_TEX_H`**
//! = 256 x 320. All values are 0..1. Two stacked sections, both RINGS whose
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
//!                 value = the SIGNED sample, -1..1 (0 = silence —
//!                 which is also what an UNBOUND texture slot reads, so a
//!                 shader cannot tell "no analysis" from "silence", and
//!                 both are a flat line)
//!                 one row per HOP, 256 peak-decimated points per row
//!                 -> 64 * 1024 samples ~ 1.37 s of waveform history
//! ```
//!
//! Both rings advance together (one row of each per hop), so ONE hop clock
//! explains the whole picture. Uniforms published beside the texture:
//!
//! ```text
//!  audio_dim  = (bins, spec_rows, spec_cursor, wave_cursor)
//!  audio_meta = (tex_w, tex_h, wave_rows, hop_secs)
//!  audio_env  = (bass, mid, high, rms)   smoothed 0..1, no texture read
//! ```
//!
//! `spec_cursor` / `wave_cursor` are the row indices of the NEWEST row in
//! each ring — that is the unwrap key: row `(cursor - n) mod rows` is `n`
//! hops ago. The shader families wrap all of that in three helpers
//! (`audio_fft(f, age)`, `audio_wave(t)`, plus the raw `audio_env`), so a
//! preset never does ring math by hand. See CONTRACT.md, "THE AUDIO".
//!
//! ## SILENCE IS A VALID PICTURE
//!
//! With no capture device bound (MONITOR AUDIO off — the default) nothing
//! is pushed: the whole texture stays 0, which reads as "no spectrum, flat
//! waveform" — the same thing an unbound texture slot reads. That is a
//! legal, sane picture for every helper: no NaNs, no divide by zero, never
//! a false full-scale. Looks are expected to carry their own idle floor off
//! the beat clock so a silent rig still performs.

use makepad_widgets::*;

/// The app's linear-amplitude perceptual curve. Kept as a LOCAL constant
/// (same value as `wave_analysis::WAVE_CURVE`) because this module is also
/// compiled into the effect_gallery example, whose crate root has no
/// `wave_analysis` — the two must not drift.
const WAVE_CURVE: f32 = 0.62;
use std::sync::atomic::Ordering;

/// Texture width: log-spaced FFT bins per spectrogram row, and points per
/// waveform row.
pub const AUDIO_BINS: usize = 256;
/// Spectrogram history rows (one per hop).
pub const AUDIO_SPEC_ROWS: usize = 256;
/// Waveform history rows (one per hop).
pub const AUDIO_WAVE_ROWS: usize = 64;
/// Texture height: the two sections stacked.
pub const AUDIO_TEX_H: usize = AUDIO_SPEC_ROWS + AUDIO_WAVE_ROWS;
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

/// Everything a draw call needs to bind the audio texture. Cheap to clone
/// (the texture is a handle).
#[derive(Clone)]
pub struct AudioBinding {
    pub tex: Texture,
    /// (bins, spec_rows, spec_cursor, wave_cursor)
    pub dim: Vec4f,
    /// (tex_w, tex_h, wave_rows, hop_secs)
    pub meta: Vec4f,
    /// (bass, mid, high, rms), smoothed 0..1
    pub env: Vec4f,
    /// The `Signals` levels the document binding language reads:
    /// (energy, bass, mid, high).
    pub levels: [f32; 4],
}

/// Bind the audio texture + its uniforms onto ONE draw call.
///
/// Resolution is BY NAME, not by slot index: the texture lands in whatever
/// slot the shader declared `audio_tex` in (families declare a different
/// number of textures before it), and a shader that declares none is a
/// silent no-op. That is what lets one call site serve every engine.
pub fn bind_audio(cx: &Cx, dv: &mut DrawVars, a: &AudioBinding) {
    if let Some(sid) = dv.draw_shader_id {
        let sh = &cx.draw_shaders[sid.index];
        if let Some(slot) = sh.mapping.textures.iter().position(|t| t.id == live_id!(audio_tex)) {
            dv.set_texture(slot, &a.tex);
        }
    }
    dv.set_uniform(cx, live_id!(audio_dim), &[a.dim.x, a.dim.y, a.dim.z, a.dim.w]);
    dv.set_uniform(cx, live_id!(audio_meta), &[a.meta.x, a.meta.y, a.meta.z, a.meta.w]);
    dv.set_uniform(cx, live_id!(audio_env), &[a.env.x, a.env.y, a.env.z, a.env.w]);
}

/// The per-frame audio picture: analyser + texture + the publish surface.
///
/// One of these lives on the app; every effect host is handed the same
/// [`AudioBinding`], so the whole show reads one coherent analysis.
pub struct AudioTexBus {
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
    /// The last `FFT_SIZE` samples, ring-free (rotated on each hop).
    window: Vec<f32>,
    scratch: Vec<f32>,
    fft_re: Vec<f32>,
    fft_im: Vec<f32>,
    hann: Vec<f32>,
    /// Per-output-bin FFT bin span, rebuilt when the sample rate changes.
    bin_lo: Vec<u16>,
    bin_hi: Vec<u16>,
    rate: f32,
    /// Smoothed (bass, mid, high, rms) — attack fast, release slow.
    env: [f32; 4],
    /// True once at least one hop has been analysed.
    seen_audio: bool,
}

impl Default for AudioTexBus {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioTexBus {
    pub fn new() -> AudioTexBus {
        // Zero is a legal reading everywhere: no spectrum, flat waveform.
        let data = vec![0.0f32; AUDIO_TEX_W * AUDIO_TEX_H];
        let hann = (0..FFT_SIZE)
            .map(|i| {
                // Periodic Hann, the same definition the WSOLA stretcher and
                // the offline spectrogram use.
                0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / FFT_SIZE as f32).cos()
            })
            .collect();
        AudioTexBus {
            tex: None,
            data,
            rows_written: 0,
            cursor: 0,
            pending: Vec::with_capacity(HOP * 4),
            window: vec![0.0; FFT_SIZE],
            scratch: Vec::with_capacity(HOP * 8),
            fft_re: vec![0.0; FFT_SIZE],
            fft_im: vec![0.0; FFT_SIZE],
            hann,
            bin_lo: vec![0; AUDIO_BINS],
            bin_hi: vec![0; AUDIO_BINS],
            rate: 0.0,
            env: [0.0; 4],
            seen_audio: false,
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
        if (self.rate - rate).abs() > 0.5 {
            self.rate = rate;
            self.rebuild_bins();
        }
        // Guard against a stalled consumer: never let `pending` grow past a
        // few hops (bounded work per frame is the law here).
        if self.pending.len() > HOP * 16 {
            let keep = self.pending.len() - HOP * 4;
            self.pending.drain(..keep);
        }
        for s in samples {
            self.pending.push(if s.is_finite() { *s } else { 0.0 });
        }
        while self.pending.len() >= HOP {
            let hop: Vec<f32> = self.pending.drain(..HOP).collect();
            self.analyse_hop(&hop);
        }
    }

    /// The binding every effect host is handed. `None` before the texture
    /// exists (nothing has pumped yet).
    pub fn binding(&self) -> Option<AudioBinding> {
        let tex = self.tex.as_ref()?;
        // The cursor names the row LAST WRITTEN (the newest); before any
        // hop exists both rings are uniform anyway, so row 0 is honest.
        let newest = self.rows_written.saturating_sub(1);
        let spec_cursor = (newest % AUDIO_SPEC_ROWS as u64) as f32;
        let wave_cursor = (newest % AUDIO_WAVE_ROWS as u64) as f32;
        let hop_secs = if self.rate > 0.0 { HOP as f32 / self.rate } else { HOP as f32 / 48_000.0 };
        Some(AudioBinding {
            tex: tex.clone(),
            dim: vec4(AUDIO_BINS as f32, AUDIO_SPEC_ROWS as f32, spec_cursor, wave_cursor),
            meta: vec4(AUDIO_TEX_W as f32, AUDIO_TEX_H as f32, AUDIO_WAVE_ROWS as f32, hop_secs),
            env: vec4(self.env[0], self.env[1], self.env[2], self.env[3]),
            levels: [self.env[3], self.env[0], self.env[1], self.env[2]],
        })
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

    fn analyse_hop(&mut self, hop: &[f32]) {
        // Slide the analysis window: drop the oldest HOP, append the new one.
        self.window.copy_within(HOP.., 0);
        self.window[FFT_SIZE - HOP..].copy_from_slice(hop);

        // ---- waveform row: peak-decimated, stored SIGNED ----
        let wrow = (self.rows_written % AUDIO_WAVE_ROWS as u64) as usize;
        let wbase = (AUDIO_SPEC_ROWS + wrow) * AUDIO_TEX_W;
        for x in 0..AUDIO_BINS {
            let mut peak = 0.0f32;
            for k in 0..WAVE_DECIM {
                let s = hop[x * WAVE_DECIM + k];
                if s.abs() > peak.abs() {
                    peak = s;
                }
            }
            self.data[wbase + x] = peak.clamp(-1.0, 1.0);
        }

        // ---- spectrogram row ----
        for i in 0..FFT_SIZE {
            self.fft_re[i] = self.window[i] * self.hann[i];
            self.fft_im[i] = 0.0;
        }
        fft_radix2(&mut self.fft_re, &mut self.fft_im);
        // A full-scale sine through a Hann window peaks at N/4 in one bin;
        // that is the 0 dBFS reference, so the row is an ABSOLUTE level.
        let inv_ref = 4.0 / FFT_SIZE as f32;
        let srow = (self.rows_written % AUDIO_SPEC_ROWS as u64) as usize;
        let sbase = srow * AUDIO_TEX_W;
        // Band accumulators, split on the crossovers the beat detector uses.
        let hz_per_bin = if self.rate > 0.0 { self.rate / FFT_SIZE as f32 } else { 23.4 };
        let (mut bass, mut mid, mut high) = (0.0f32, 0.0f32, 0.0f32);
        for j in 0..AUDIO_BINS {
            let lo = self.bin_lo[j] as usize;
            let hi = (self.bin_hi[j] as usize).min(FFT_SIZE / 2);
            let mut mag = 0.0f32;
            for b in lo..hi.max(lo + 1) {
                let m = (self.fft_re[b] * self.fft_re[b] + self.fft_im[b] * self.fft_im[b]).sqrt();
                if m > mag {
                    mag = m;
                }
            }
            let norm = (mag * inv_ref).max(1e-9);
            let db = 20.0 * norm.log10();
            let v = ((db + RANGE_DB) / RANGE_DB).clamp(0.0, 1.0).powf(SPEC_GAMMA);
            self.data[sbase + j] = v;
            let f = (lo as f32 + 0.5) * hz_per_bin;
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

/// In-place iterative radix-2 Cooley-Tukey FFT, forward, unnormalised.
///
/// Written here on purpose: the VJ crate has no runtime FFT to reuse —
/// `music_dsp.rs` is deliberately spectral-free (WSOLA + biquads),
/// `wave_analysis.rs` is time-domain one-poles, `beat_sync.rs` is a
/// time-domain band tracker, and the only other radix-2 in the crate lives
/// in `beat_eval.rs`, which is `#[cfg(test)]` judge code that must not
/// share code with the analysis it grades.
fn fft_radix2(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
    debug_assert!(n.is_power_of_two() && im.len() == n);
    // Bit-reversal permutation.
    let mut j = 0usize;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2usize;
    while len <= n {
        let half = len / 2;
        for k in 0..half {
            // Twiddles are evaluated, not stepped: an incremental rotation
            // drifts over 1024 butterflies and smears the high bins.
            let ang = -std::f32::consts::TAU * k as f32 / len as f32;
            let (cr, ci) = (ang.cos(), ang.sin());
            let mut i = 0usize;
            while i < n {
                let (ur, ui) = (re[i + k], im[i + k]);
                let (br, bi) = (re[i + k + half], im[i + k + half]);
                let (vr, vi) = (br * cr - bi * ci, br * ci + bi * cr);
                re[i + k] = ur + vr;
                im[i + k] = ui + vi;
                re[i + k + half] = ur - vr;
                im[i + k + half] = ui - vi;
                i += len;
            }
        }
        len <<= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The FFT against a naive DFT on a known signal — the analysis is only
    /// as trustworthy as this.
    #[test]
    fn fft_matches_dft() {
        const N: usize = 64;
        let sig: Vec<f32> =
            (0..N).map(|i| (std::f32::consts::TAU * 5.0 * i as f32 / N as f32).sin()).collect();
        let mut re = sig.clone();
        let mut im = vec![0.0f32; N];
        fft_radix2(&mut re, &mut im);
        for k in 0..N {
            let (mut dr, mut di) = (0.0f32, 0.0f32);
            for (n, s) in sig.iter().enumerate() {
                let a = -std::f32::consts::TAU * (k * n) as f32 / N as f32;
                dr += s * a.cos();
                di += s * a.sin();
            }
            assert!((re[k] - dr).abs() < 1e-2, "re[{k}] {} vs {dr}", re[k]);
            assert!((im[k] - di).abs() < 1e-2, "im[{k}] {} vs {di}", im[k]);
        }
    }

    /// A 1 kHz full-scale sine must land near the top of the normalised
    /// scale in the bin that covers it, and silence must land at the floor.
    #[test]
    fn spectrum_row_is_absolute() {
        let mut bus = AudioTexBus::new();
        let rate = 48_000.0f32;
        let sine: Vec<f32> = (0..HOP * 8)
            .map(|i| (std::f32::consts::TAU * 1000.0 * i as f32 / rate).sin())
            .collect();
        bus.push_samples(&sine, rate);
        let row = (bus.rows_written - 1) as usize % AUDIO_SPEC_ROWS;
        let base = row * AUDIO_TEX_W;
        let peak = bus.data[base..base + AUDIO_BINS].iter().cloned().fold(0.0f32, f32::max);
        assert!(peak > 0.9, "1 kHz sine peaked at {peak}");

        let mut quiet = AudioTexBus::new();
        quiet.push_samples(&vec![0.0f32; HOP * 4], rate);
        let row = (quiet.rows_written - 1) as usize % AUDIO_SPEC_ROWS;
        let base = row * AUDIO_TEX_W;
        let peak = quiet.data[base..base + AUDIO_BINS].iter().cloned().fold(0.0f32, f32::max);
        assert!(peak < 0.02, "silence peaked at {peak}");
    }

    /// The waveform section is signed, bounded, and silent-is-zero — the
    /// same reading an unbound texture slot gives, by design.
    #[test]
    fn wave_row_is_signed_and_bounded() {
        let mut bus = AudioTexBus::new();
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
        let idle = AudioTexBus::new();
        assert!(idle.data.iter().all(|v| *v == 0.0));
    }
}
