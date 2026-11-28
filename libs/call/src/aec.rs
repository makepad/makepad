//! Acoustic echo cancellation at the call's mono voice rate.

use std::sync::atomic::{AtomicU8, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

pub const AEC_SAMPLE_RATE: usize = 16_000;
pub const AEC_BLOCK: usize = 64;
pub const AEC_FFT: usize = 2 * AEC_BLOCK;
pub const AEC_PARTITIONS: usize = 32;
pub const AEC_MAX_DELAY_MS: usize = 250;
pub const RENDER_RING_CAP: usize = 8192;
const _: () = assert!(RENDER_RING_CAP.is_power_of_two());
const RENDER_RING_MASK: usize = RENDER_RING_CAP - 1;
const CAPTURE_ACTIVE: u8 = 0;
const CAPTURE_MUTED: u8 = 1;
const CAPTURE_RESUMING: u8 = 2;

/// Wait-free SPSC ring from the render callback to the capture callback.
/// The producer owns the free slots and the consumer owns the published slots.
pub struct RenderRefRing {
    buf: Box<[AtomicU32]>,
    write: AtomicUsize,
    read: AtomicUsize,
    overflows: AtomicU64,
    paused_overflows: AtomicU64,
    capture_state: AtomicU8,
    reset_epoch: AtomicU64,
}

impl RenderRefRing {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            buf: (0..RENDER_RING_CAP).map(|_| AtomicU32::new(0)).collect(),
            write: AtomicUsize::new(0),
            read: AtomicUsize::new(0),
            overflows: AtomicU64::new(0),
            paused_overflows: AtomicU64::new(0),
            capture_state: AtomicU8::new(CAPTURE_ACTIVE),
            reset_epoch: AtomicU64::new(0),
        })
    }

    /// Producer only. Publishes the prefix that fits and counts an overflow if any tail was lost.
    pub fn push_mono(&self, samples: &[f32]) {
        self.push_mono_inner(samples, || {});
    }

    #[inline(always)]
    fn push_mono_inner(&self, samples: &[f32], before_publish: impl FnOnce()) {
        if samples.is_empty() {
            return;
        }
        // A consumer discards before publishing ACTIVE. Observe its state before
        // reading the cursor so an active overflow cannot use a pre-discard cursor.
        let state_before = self.capture_state.load(Ordering::Acquire);
        let w = self.write.load(Ordering::Relaxed);
        let r = self.read.load(Ordering::Acquire);
        let free = RENDER_RING_CAP.saturating_sub(w.wrapping_sub(r));
        let take = samples.len().min(free);
        for (i, s) in samples[..take].iter().enumerate() {
            self.buf[w.wrapping_add(i) & RENDER_RING_MASK].store(s.to_bits(), Ordering::Relaxed);
        }
        before_publish();
        if take > 0 {
            self.write.store(w.wrapping_add(take), Ordering::Release);
        }
        if take < samples.len() {
            if state_before == CAPTURE_ACTIVE && self.capture_state.load(Ordering::Acquire) == CAPTURE_ACTIVE {
                self.overflows.fetch_add(1, Ordering::Release);
            } else {
                self.paused_overflows.fetch_add(1, Ordering::Release);
            }
        }
    }

    /// Consumer only. Copies published samples in FIFO order and returns their slots.
    pub fn pop_into(&self, out: &mut [f32]) -> usize {
        let r = self.read.load(Ordering::Relaxed);
        let w = self.write.load(Ordering::Acquire);
        let n = out.len().min(w.wrapping_sub(r));
        for (i, dst) in out[..n].iter_mut().enumerate() {
            *dst = f32::from_bits(self.buf[r.wrapping_add(i) & RENDER_RING_MASK].load(Ordering::Relaxed));
        }
        if n > 0 {
            self.read.store(r.wrapping_add(n), Ordering::Release);
        }
        n
    }

    /// Consumer only. Flushes the published backlog after a gap, without copying it.
    pub fn discard(&self) -> usize {
        let r = self.read.load(Ordering::Relaxed);
        let w = self.write.load(Ordering::Acquire);
        let n = w.wrapping_sub(r);
        if n > 0 {
            self.read.store(w, Ordering::Release);
        }
        n
    }

    /// Consumer only. Published samples currently waiting for capture.
    pub fn available(&self) -> usize {
        let r = self.read.load(Ordering::Relaxed);
        let w = self.write.load(Ordering::Acquire);
        w.wrapping_sub(r)
    }

    /// Pushes that lost samples while capture was active.
    pub fn overflows(&self) -> u64 {
        self.overflows.load(Ordering::Acquire)
    }

    /// Pushes that lost samples while muted or waiting for capture to open.
    pub fn paused_overflows(&self) -> u64 {
        self.paused_overflows.load(Ordering::Acquire)
    }

    /// Number of explicit reset requests from the control thread.
    pub fn reset_epoch(&self) -> u64 {
        self.reset_epoch.load(Ordering::Acquire)
    }

    /// Control thread. Keep overflow reporting paused until capture actually resumes.
    pub fn set_capture_muted(&self, muted: bool) {
        if muted {
            self.capture_state.store(CAPTURE_MUTED, Ordering::Release);
        } else {
            let _ = self.capture_state.compare_exchange(CAPTURE_MUTED, CAPTURE_RESUMING, Ordering::AcqRel, Ordering::Relaxed);
        }
    }

    /// Control thread. Pause active overflow reporting during startup or a route change.
    pub fn set_capture_resuming(&self) {
        let _ = self.capture_state.compare_exchange(CAPTURE_ACTIVE, CAPTURE_RESUMING, Ordering::AcqRel, Ordering::Relaxed);
    }

    /// Control thread. Start the next capture callback with a fresh echo path.
    pub fn request_reset(&self) {
        self.reset_epoch.fetch_add(1, Ordering::Release);
        self.set_capture_resuming();
    }

    /// Consumer only. Finish resuming after gap handling, preserving a concurrent mute.
    pub fn note_capture_active(&self) {
        if self.capture_state.load(Ordering::Acquire) == CAPTURE_RESUMING {
            let _ = self.capture_state.compare_exchange(CAPTURE_RESUMING, CAPTURE_ACTIVE, Ordering::AcqRel, Ordering::Relaxed);
        }
    }
}

#[derive(Clone, Copy, Default)]
struct Complex {
    re: f32,
    im: f32,
}

impl Complex {
    const ZERO: Self = Self { re: 0.0, im: 0.0 };

    fn real(re: f32) -> Self { Self { re, im: 0.0 } }
    fn conj(self) -> Self { Self { re: self.re, im: -self.im } }
    fn power(self) -> f32 { self.re * self.re + self.im * self.im }
    fn scale(self, a: f32) -> Self { Self { re: self.re * a, im: self.im * a } }
    fn mul(self, b: Self) -> Self {
        Self { re: self.re * b.re - self.im * b.im, im: self.im * b.re + self.re * b.im }
    }
    fn add(self, b: Self) -> Self { Self { re: self.re + b.re, im: self.im + b.im } }
    fn sub(self, b: Self) -> Self { Self { re: self.re - b.re, im: self.im - b.im } }
}

/// In-place radix-2 transform. Both sizes use the same butterflies and tables
/// built at construction. Forward transforms are unscaled, inverse transforms divide by N.
struct Fft {
    reversed: Box<[usize]>,
    roots: Box<[Complex]>,
}

impl Fft {
    fn new(n: usize) -> Self {
        let reversed = (0..n).map(|i| i.reverse_bits() >> (usize::BITS - n.ilog2())).collect();
        let roots = (0..n / 2).map(|i| {
            let (im, re) = (-std::f32::consts::TAU * i as f32 / n as f32).sin_cos();
            Complex { re, im }
        }).collect();
        Self { reversed, roots }
    }

    fn transform(&self, data: &mut [Complex], inverse: bool) {
        let n = self.reversed.len();
        for (i, &j) in self.reversed.iter().enumerate() {
            if i < j {
                data.swap(i, j);
            }
        }
        let mut width = 2;
        while width <= n {
            let step = n / width;
            for group in data.chunks_exact_mut(width) {
                let (left, right) = group.split_at_mut(width / 2);
                for (i, (a, b)) in left.iter_mut().zip(right).enumerate() {
                    let root = self.roots[i * step];
                    let odd = b.mul(if inverse { root.conj() } else { root });
                    *b = a.sub(odd);
                    *a = a.add(odd);
                }
            }
            width *= 2;
        }
        if inverse {
            let scale = 1.0 / n as f32;
            for x in data { *x = x.scale(scale); }
        }
    }
}

const DECIMATION: usize = 4;
const MAX_LAG: usize = AEC_MAX_DELAY_MS * AEC_SAMPLE_RATE / 1000 / DECIMATION;
const DELAY_BLOCKS: usize = 25;
/// Burst headroom in addition to the current render block (64 ms).
const FIFO_SLACK: usize = 64 * AEC_SAMPLE_RATE / 1000;
/// Observe the minimum after each capture pop over 100 ms.
const FIFO_WINDOW_BLOCKS: usize = DELAY_BLOCKS;
/// Keep one block of standing slack for small callback timing differences.
const FIFO_TARGET: usize = AEC_BLOCK;
const DELAY_WINDOW: usize = DELAY_BLOCKS * AEC_BLOCK / DECIMATION;
const DELAY_FFT: usize = 2048;
const _: () = assert!(DELAY_WINDOW + MAX_LAG <= DELAY_FFT);
const ACTIVE_POWER: f32 = 0.00031623; // -35 dBFS
const BINS: usize = AEC_FFT / 2 + 1;
type Spectrum = [Complex; BINS];

struct DelayEstimator {
    far: Box<[f32]>,
    mic: Box<[f32]>,
    pos: usize,
    fft: Fft,
    x: Box<[Complex]>,
    d: Box<[Complex]>,
}

impl DelayEstimator {
    fn new() -> Self {
        Self {
            far: vec![0.0; DELAY_FFT].into_boxed_slice(),
            mic: vec![0.0; DELAY_FFT].into_boxed_slice(),
            pos: 0,
            fft: Fft::new(DELAY_FFT),
            x: vec![Complex::ZERO; DELAY_FFT].into_boxed_slice(),
            d: vec![Complex::ZERO; DELAY_FFT].into_boxed_slice(),
        }
    }

    fn reset(&mut self) {
        self.far.fill(0.0);
        self.mic.fill(0.0);
        self.pos = 0;
    }

    fn observe(&mut self, far: &[f32], mic: &[f32]) {
        for (x, d) in far.chunks_exact(DECIMATION).zip(mic.chunks_exact(DECIMATION)) {
            self.far[self.pos] = x.iter().sum::<f32>() / DECIMATION as f32;
            self.mic[self.pos] = d.iter().sum::<f32>() / DECIMATION as f32;
            self.pos = (self.pos + 1) % DELAY_FFT;
        }
    }

    fn estimate(&mut self) -> Option<usize> {
        // The histories share an origin, so their circular correlation is valid
        // at every searched lag when the mic is windowed to the most recent 100 ms.
        let at = |age: usize| (self.pos + DELAY_FFT - 1 - age) % DELAY_FFT;
        let mut far_power = 0.0f64;
        let mut mic_power = 0.0f64;
        self.d.fill(Complex::ZERO);
        for age in 0..DELAY_WINDOW {
            let i = at(age);
            self.d[i] = Complex::real(self.mic[i]);
            mic_power += f64::from(self.mic[i]).powi(2);
            far_power += f64::from(self.far[i]).powi(2);
        }
        if mic_power < 1e-10 || far_power < f64::from(ACTIVE_POWER) * DELAY_WINDOW as f64 {
            return None;
        }
        for (z, &x) in self.x.iter_mut().zip(self.far.iter()) { *z = Complex::real(x); }
        self.fft.transform(&mut self.x, false);
        self.fft.transform(&mut self.d, false);
        for (d, &x) in self.d.iter_mut().zip(self.x.iter()) { *d = d.mul(x.conj()); }
        self.fft.transform(&mut self.d, true);
        let mut best = 0.25f64.powi(2);
        let mut delay = None;
        for lag in 0..=MAX_LAG {
            if far_power > 1e-8 {
                let score = f64::from(self.d[lag].re).powi(2) / (far_power * mic_power);
                if score > best {
                    best = score;
                    delay = Some((lag * DECIMATION).saturating_sub(2 * AEC_BLOCK));
                }
            }
            far_power += f64::from(self.far[at(lag + DELAY_WINDOW)]).powi(2)
                - f64::from(self.far[at(lag)]).powi(2);
        }
        delay
    }
}

/// Real inverse FFT from the independent bins; restores the conjugate half in scratch.
fn inverse_spectrum(fft: &Fft, bins: &Spectrum, scratch: &mut [Complex; AEC_FFT]) {
    scratch[..BINS].copy_from_slice(bins);
    for k in 1..BINS - 1 {
        scratch[AEC_FFT - k] = bins[k].conj();
    }
    fft.transform(scratch, true);
}

/// Partitioned block frequency-domain adaptive filter. All storage, including both FFT
/// plans and delay-search scratch, is allocated before the first capture block.
pub struct AecProcessor {
    far_fifo: Box<[f32]>,
    fifo_read: usize,
    fifo_len: usize,
    fifo_min: usize,
    fifo_window: usize,
    history: Box<[f32]>,
    history_pos: usize,
    estimator: DelayEstimator,
    delay: usize,
    pending_delay: Option<usize>,
    fft: Fft,
    weights: Box<[Spectrum]>,
    inputs: Box<[Spectrum]>,
    newest: usize,
    valid_partitions: usize,
    power: [f32; BINS],
    scratch: [Complex; AEC_FFT],
    blocks: usize,
    estimate_blocks: usize,
    trained: bool,
    divergence: f32,
    mic_energy: f32,
    out_energy: f32,
}

impl AecProcessor {
    pub fn new() -> Self {
        Self {
            far_fifo: vec![0.0; RENDER_RING_CAP].into_boxed_slice(), fifo_read: 0, fifo_len: 0,
            fifo_min: usize::MAX, fifo_window: 0,
            history: vec![0.0; RENDER_RING_CAP].into_boxed_slice(), history_pos: 0,
            estimator: DelayEstimator::new(), delay: 0, pending_delay: None, fft: Fft::new(AEC_FFT),
            weights: vec![[Complex::ZERO; BINS]; AEC_PARTITIONS].into_boxed_slice(),
            inputs: vec![[Complex::ZERO; BINS]; AEC_PARTITIONS].into_boxed_slice(), newest: 0, valid_partitions: 0,
            power: [0.0; BINS], scratch: [Complex::ZERO; AEC_FFT], blocks: 0, estimate_blocks: 0,
            trained: false, divergence: 0.0, mic_energy: 0.0, out_energy: 0.0,
        }
    }

    /// Clear the adaptive path and its timelines in place after a capture discontinuity.
    pub fn reset(&mut self) {
        self.fifo_read = 0;
        self.fifo_len = 0;
        self.fifo_min = usize::MAX;
        self.fifo_window = 0;
        self.history.fill(0.0);
        self.history_pos = 0;
        self.estimator.reset();
        self.delay = 0;
        self.pending_delay = None;
        self.weights.fill([Complex::ZERO; BINS]);
        self.inputs.fill([Complex::ZERO; BINS]);
        self.newest = 0;
        self.valid_partitions = 0;
        self.power.fill(0.0);
        self.scratch.fill(Complex::ZERO);
        self.blocks = 0;
        self.estimate_blocks = 0;
        self.trained = false;
        self.divergence = 0.0;
        self.mic_energy = 0.0;
        self.out_energy = 0.0;
    }

    pub fn feed_render(&mut self, far: &[f32; AEC_BLOCK]) {
        let write = (self.fifo_read + self.fifo_len) % RENDER_RING_CAP;
        self.far_fifo[write..write + AEC_BLOCK].copy_from_slice(far);
        // Retain the current block plus 64 ms of render-ahead slack. All indices
        // advance by whole blocks and the FIFO capacity is a multiple of the block.
        self.fifo_len += AEC_BLOCK;
        if self.fifo_len > FIFO_SLACK + AEC_BLOCK {
            self.fifo_read = (self.fifo_read + AEC_BLOCK) % RENDER_RING_CAP;
            self.fifo_len -= AEC_BLOCK;
        }
    }

    fn advance_render(&mut self, mic: &[f32; AEC_BLOCK], estimate_delay: bool) -> f32 {
        let mut far = [0.0; AEC_BLOCK];
        if self.fifo_len > 0 {
            far.copy_from_slice(&self.far_fifo[self.fifo_read..self.fifo_read + AEC_BLOCK]);
            self.fifo_read = (self.fifo_read + AEC_BLOCK) % RENDER_RING_CAP;
            self.fifo_len -= AEC_BLOCK;
        }
        self.fifo_min = self.fifo_min.min(self.fifo_len);
        self.fifo_window += 1;
        let mut trimmed = 0;
        if self.fifo_window >= FIFO_WINDOW_BLOCKS {
            if self.fifo_min > FIFO_TARGET {
                trimmed = (self.fifo_min - FIFO_TARGET).min(self.fifo_len);
                self.fifo_read = (self.fifo_read + trimmed) % RENDER_RING_CAP;
                self.fifo_len -= trimmed;
            }
            self.fifo_min = usize::MAX;
            self.fifo_window = 0;
        }
        self.history[self.history_pos..self.history_pos + AEC_BLOCK].copy_from_slice(&far);
        self.history_pos = (self.history_pos + AEC_BLOCK) % RENDER_RING_CAP;
        self.estimator.observe(&far, mic);
        self.blocks = self.blocks.wrapping_add(1);
        if trimmed > 0 {
            // The next render block follows a skip. Correlate only continuous post-trim audio.
            self.estimator.reset();
            self.pending_delay = None;
            self.estimate_blocks = 0;
            if !self.trained && trimmed > AEC_BLOCK {
                self.delay = 0;
                self.weights.fill([Complex::ZERO; BINS]);
                self.inputs.fill([Complex::ZERO; BINS]);
                self.valid_partitions = 0;
                self.power.fill(0.0);
                self.divergence = 0.0;
            }
        } else if !estimate_delay {
            self.pending_delay = None;
            self.estimate_blocks = 0;
        } else {
            // Correlate only after a full window of live mic samples.
            self.estimate_blocks += 1;
            if self.estimate_blocks >= DELAY_BLOCKS {
                self.estimate_blocks = 0;
                let candidate = self.estimator.estimate().filter(|delay| delay.abs_diff(self.delay) > AEC_BLOCK);
                if let Some(delay) = candidate {
                    // During double-talk a reflection can briefly win the correlation
                    // search. Confirm a displacement before throwing away a trained path.
                    if !self.trained || self.pending_delay.is_some_and(|prev| prev.abs_diff(delay) <= AEC_BLOCK) {
                        self.delay = delay;
                        self.weights.fill([Complex::ZERO; BINS]);
                        self.inputs.fill([Complex::ZERO; BINS]);
                        self.valid_partitions = 0;
                        self.power.fill(0.0);
                        self.trained = false;
                        self.divergence = 0.0;
                        self.pending_delay = None;
                    } else {
                        self.pending_delay = Some(delay);
                    }
                } else {
                    self.pending_delay = None;
                }
            }
        }
        let start = self.history_pos.wrapping_sub(self.delay + AEC_FFT);
        let mut energy = 0.0;
        for (i, z) in self.scratch.iter_mut().enumerate() {
            let x = self.history[start.wrapping_add(i) & RENDER_RING_MASK];
            *z = Complex::real(x);
            energy += x * x;
        }
        self.fft.transform(&mut self.scratch, false);
        self.newest = (self.newest + AEC_PARTITIONS - 1) % AEC_PARTITIONS;
        self.inputs[self.newest].copy_from_slice(&self.scratch[..BINS]);
        self.valid_partitions = (self.valid_partitions + 1).min(AEC_PARTITIONS);
        energy / AEC_FFT as f32
    }

    /// Advance the render delay line and partition spectra for one muted capture
    /// block. The learned path, delay and ERLE stay frozen.
    pub fn process_muted(&mut self) {
        self.advance_render(&[0.0; AEC_BLOCK], false);
        let mut total_power = [0.0; BINS];
        for input in self.inputs.iter() {
            for k in 0..BINS {
                total_power[k] += input[k].power();
            }
        }
        for k in 0..BINS {
            self.power[k] = 0.8 * self.power[k] + 0.2 * total_power[k];
        }
    }

    /// Subtract the acoustic prediction and learn from the unsuppressed residual.
    pub fn process_capture(&mut self, near: &mut [f32; AEC_BLOCK]) {
        let far_active = self.advance_render(near, true) > ACTIVE_POWER;
        let mut prediction = [Complex::ZERO; BINS];
        let mut total_power = [0.0; BINS];
        for (p, weights) in self.weights.iter().enumerate() {
            let input = &self.inputs[(self.newest + p) % AEC_PARTITIONS];
            for k in 0..BINS {
                prediction[k] = prediction[k].add(weights[k].mul(input[k]));
                total_power[k] += input[k].power();
            }
        }
        inverse_spectrum(&self.fft, &prediction, &mut self.scratch);
        let mut residual = [0.0; AEC_BLOCK];
        let (mut ed, mut ey, mut ee, mut cross) = (0.0, 0.0, 0.0, 0.0);
        for i in 0..AEC_BLOCK {
            let d = near[i];
            let y = self.scratch[AEC_BLOCK + i].re;
            let e = d - y;
            residual[i] = e;
            ed += d * d;
            ey += y * y;
            ee += e * e;
            cross += d * y;
        }
        // A shifted path makes subtraction consistently louder than the mic across blocks
        // even when individual 4 ms low-frequency blocks dip below ed. Smooth the bounded
        // contrast so double-talk at any near-end level stays negative while a real shift
        // clears the stale weights without discarding the coarse delay or render spectra.
        if self.trained && far_active {
            let delta = (ee - ed) / (ee + ed + 1e-4);
            self.divergence = (0.96 * self.divergence + 0.04 * delta).max(0.0);
            if self.divergence >= 0.10 {
                self.weights.fill([Complex::ZERO; BINS]);
                self.trained = false;
                self.divergence = 0.0;
                residual.copy_from_slice(near);
                ey = 0.0;
                ee = ed;
                cross = 0.0;
            }
        }
        // Once the linear path explains most mic energy, a new uncorrelated
        // component indicates near-end speech. Keep subtracting the learned echo.
        let coherent = cross > 0.0 && cross * cross > 0.75 * ed * ey;
        let double_talk = self.trained && !coherent && ee > 0.12 * ed;
        if far_active && ee < 0.15 * ed { self.trained = true; }
        let suppress = !double_talk && far_active && coherent && ey > 0.05 * ed && ee < ed;
        self.scratch[..AEC_BLOCK].fill(Complex::ZERO);
        for (z, &e) in self.scratch[AEC_BLOCK..].iter_mut().zip(&residual) { *z = Complex::real(e); }
        self.fft.transform(&mut self.scratch, false);
        let mut gradient = [Complex::ZERO; BINS];
        for k in 0..BINS {
            self.power[k] = 0.8 * self.power[k] + 0.2 * total_power[k];
            // Bound the gain during a sudden onset while the smoother catches up.
            gradient[k] = self.scratch[k].scale(0.35 / (self.power[k].max(total_power[k]) + 1e-6));
        }
        // The mic already contains the full echo tail at a relock. Wait until
        // its render history is present before normalizing a gradient against it.
        let adapt = self.valid_partitions == AEC_PARTITIONS && far_active && !double_talk;
        if adapt {
            for (p, weights) in self.weights.iter_mut().enumerate() {
                let input = &self.inputs[(self.newest + p) % AEC_PARTITIONS];
                for k in 0..BINS {
                    weights[k] = weights[k].add(input[k].conj().mul(gradient[k]));
                }
            }
        }
        if ee <= ed {
            if suppress {
                let mut filtered = [Complex::ZERO; BINS];
                for k in 0..BINS {
                    let error = self.scratch[k];
                    let gain = (1.0 - prediction[k].power() / (error.power() + 1e-9)).max(0.12);
                    filtered[k] = error.scale(gain);
                }
                inverse_spectrum(&self.fft, &filtered, &mut self.scratch);
                for (e, z) in residual.iter_mut().zip(&self.scratch[AEC_BLOCK..]) { *e = z.re; }
            }
            near.copy_from_slice(&residual);
        }
        if far_active && !double_talk {
            let output = near.iter().map(|x| x * x).sum::<f32>();
            self.mic_energy = 0.96 * self.mic_energy + 0.04 * ed;
            self.out_energy = 0.96 * self.out_energy + 0.04 * output;
        }
        // Project two partitions per block onto causal 64-tap filters. A full
        // sweep takes 64 ms; spreading the transforms bounds the callback cost.
        if adapt {
            for i in 0..2 {
                let p = (self.blocks.wrapping_mul(2) + i) % AEC_PARTITIONS;
                inverse_spectrum(&self.fft, &self.weights[p], &mut self.scratch);
                self.scratch[AEC_BLOCK..].fill(Complex::ZERO);
                self.fft.transform(&mut self.scratch, false);
                self.weights[p].copy_from_slice(&self.scratch[..BINS]);
            }
        }
    }

    /// Recent single-talk echo return loss enhancement, in decibels.
    pub fn erle_db(&self) -> f32 {
        10.0 * ((self.mic_energy + 1e-12) / (self.out_energy + 1e-12)).log10()
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    fn noise(seed: u32, len: usize) -> Vec<f32> {
        let mut state = seed;
        let mut speech = 0.0;
        (0..len).map(|_| {
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            speech = 0.85 * speech + 0.225 * (2.0 * (state >> 8) as f32 / 16777216.0 - 1.0);
            speech
        }).collect()
    }

    fn room(len: usize) -> (Vec<f32>, Vec<f32>) {
        let far = noise(42, len);
        let echo = room_echo(&far);
        (far, echo)
    }

    fn room_echo(far: &[f32]) -> Vec<f32> {
        room_echo_at(far, 1360)
    }

    fn room_echo_at(far: &[f32], direct_lag: usize) -> Vec<f32> {
        let mut echo = vec![0.0; far.len()];
        // Direct path followed by 47 reflections over the next 60 ms.
        let reflections = noise(123, 48);
        for (i, &reflection) in reflections.iter().enumerate() {
            let lag = direct_lag + i * 959 / 47;
            let gain = if i == 0 { 0.6 } else { reflection * (-3.0 * i as f32 / 47.0).exp() };
            for (out, &x) in echo[lag..].iter_mut().zip(far) { *out += gain * x; }
        }
        echo
    }

    fn energy(x: &[f32]) -> f64 { x.iter().map(|&s| f64::from(s).powi(2)).sum() }

    #[test]
    fn aec_drains_render_ahead_slack_for_low_latency_echo() {
        let far = noise(42, 525 * AEC_BLOCK);
        let echo = room_echo_at(&far, 320);
        let mut aec = AecProcessor::new();
        let mut residual = 0.0;
        for (i, (x, d)) in far.chunks_exact(AEC_BLOCK).zip(echo.chunks_exact(AEC_BLOCK)).enumerate() {
            aec.feed_render(x.try_into().unwrap());
            if i < 25 {
                if i == 24 {
                    assert_eq!(aec.fifo_len, FIFO_SLACK + AEC_BLOCK, "pre-render must fill the burst slack");
                }
                continue;
            }
            let mut block = d.try_into().unwrap();
            aec.process_capture(&mut block);
            if i == 49 {
                assert!(aec.fifo_len <= AEC_BLOCK, "standing render backlog after 25 capture blocks was {} samples", aec.fifo_len);
            }
            if i >= 400 { residual += energy(&block); }
        }
        let drop_db = 10.0 * (energy(&echo[400 * AEC_BLOCK..]) / residual).log10();
        assert!(drop_db >= 18.0, "low-latency echo dropped by {drop_db} dB");
        let erle = aec.erle_db();
        assert!(erle >= 18.0, "recent low-latency ERLE was {erle} dB");
    }

    #[test]
    fn aec_preserves_pipewire_sized_bursts_without_false_trim() {
        let burst = 6 * AEC_BLOCK;
        let (far, echo) = room(88 * burst);
        let mut aec = AecProcessor::new();
        let mut residual = 0.0;
        for (i, (render, mic)) in far.chunks_exact(burst).zip(echo.chunks_exact(burst)).enumerate() {
            for x in render.chunks_exact(AEC_BLOCK) { aec.feed_render(x.try_into().unwrap()); }
            for d in mic.chunks_exact(AEC_BLOCK) {
                let mut block = d.try_into().unwrap();
                aec.process_capture(&mut block);
                if i >= 67 { residual += energy(&block); }
            }
            assert_eq!(aec.fifo_len, 0, "capture must consume the entire render burst {i}");
        }
        assert!(aec.delay.abs_diff(1360 - 2 * AEC_BLOCK) <= AEC_BLOCK, "bursty render locked at the wrong delay: {}", aec.delay);
        let drop_db = 10.0 * (energy(&echo[67 * burst..]) / residual).log10();
        assert!(drop_db >= 18.0, "echo from 24 ms render bursts dropped by {drop_db} dB");
    }

    #[test]
    fn aec_reset_clears_state_and_reconverges_on_shifted_room() {
        let (far, echo) = room(375 * AEC_BLOCK);
        let mut aec = AecProcessor::new();
        for (x, d) in far.chunks_exact(AEC_BLOCK).zip(echo.chunks_exact(AEC_BLOCK)) {
            aec.feed_render(x.try_into().unwrap());
            aec.process_capture(&mut d.try_into().unwrap());
        }
        assert!(aec.trained && aec.delay > 0 && aec.erle_db() >= 18.0, "the original room must converge before reset");
        for x in far.chunks_exact(AEC_BLOCK).take(10) { aec.feed_render(x.try_into().unwrap()); }
        assert!(aec.fifo_len > 0, "the reset must start with queued render");
        aec.reset();
        assert_eq!((aec.fifo_read, aec.fifo_len), (0, 0), "reset must empty and rewind the FIFO");
        assert_eq!((aec.fifo_min, aec.fifo_window), (usize::MAX, 0), "reset must restart FIFO floor observation");
        assert_eq!((aec.delay, aec.pending_delay), (0, None), "reset must clear the locked and pending delays");
        assert!(!aec.trained && aec.valid_partitions == 0, "reset must invalidate the trained filter and spectra");
        assert_eq!((aec.blocks, aec.estimate_blocks), (0, 0), "reset must restart both block counters");
        assert_eq!(aec.divergence, 0.0, "reset must clear divergence");
        assert!(aec.erle_db().abs() < 1e-3, "reset must clear the ERLE history");

        let far = noise(99, 500 * AEC_BLOCK);
        let echo = room_echo_at(&far, 400);
        let mut residual = 0.0;
        for (i, (x, d)) in far.chunks_exact(AEC_BLOCK).zip(echo.chunks_exact(AEC_BLOCK)).enumerate() {
            let mut block = d.try_into().unwrap();
            aec.feed_render(x.try_into().unwrap());
            aec.process_capture(&mut block);
            if i >= 375 { residual += energy(&block); }
        }
        assert!(aec.delay.abs_diff(400 - 2 * AEC_BLOCK) <= AEC_BLOCK, "the new room locked at the wrong delay: {}", aec.delay);
        let drop_db = 10.0 * (energy(&echo[375 * AEC_BLOCK..]) / residual).log10();
        assert!(drop_db >= 18.0, "after reset the new room's echo dropped by {drop_db} dB");
    }

    #[test]
    fn render_ref_ring_suppresses_overflows_while_muted_or_resuming_and_reports_active_overflows() {
        let burst = [0.5; 12_000];
        let ring = RenderRefRing::new();
        ring.set_capture_muted(true);
        ring.push_mono(&burst);
        assert_eq!(ring.available(), RENDER_RING_CAP, "muted output must fill the render ring");
        assert_eq!((ring.overflows(), ring.paused_overflows()), (0, 1), "muted loss must count as paused");
        ring.set_capture_muted(false);
        ring.push_mono(&burst[..160]);
        assert_eq!((ring.overflows(), ring.paused_overflows()), (0, 2), "waiting for capture must count as paused");
        assert_eq!(ring.discard(), RENDER_RING_CAP, "resume must discard the full backlog");
        ring.note_capture_active();
        ring.push_mono(&burst);
        assert_eq!((ring.overflows(), ring.paused_overflows()), (1, 2), "live capture loss must count as active");
        ring.request_reset();
        assert_eq!(ring.reset_epoch(), 1, "a route change must publish a reset");
        ring.push_mono(&burst[..160]);
        assert_eq!((ring.overflows(), ring.paused_overflows()), (1, 3), "a route reopen must count as paused");
        ring.set_capture_muted(true);
        ring.note_capture_active();
        ring.push_mono(&burst[..160]);
        assert_eq!((ring.overflows(), ring.paused_overflows()), (1, 4), "a late capture must preserve a re-mute");

        let ring = RenderRefRing::new();
        ring.set_capture_resuming();
        ring.push_mono(&burst);
        assert_eq!((ring.overflows(), ring.paused_overflows()), (0, 1), "startup loss must count as paused");
        ring.discard();
        ring.note_capture_active();
        ring.push_mono(&burst);
        assert_eq!((ring.overflows(), ring.paused_overflows()), (1, 1), "first live capture must enable active overflow counting");
    }

    #[test]
    fn aec_converges_on_delayed_room_impulse_response() {
        let (far, echo) = room(2 * AEC_SAMPLE_RATE);
        let mut aec = AecProcessor::new();
        let mut residual = 0.0;
        for (i, (x, d)) in far.chunks_exact(AEC_BLOCK).zip(echo.chunks_exact(AEC_BLOCK)).enumerate() {
            let mut block = d.try_into().unwrap();
            aec.feed_render(x.try_into().unwrap());
            aec.process_capture(&mut block);
            if i >= 375 { residual += energy(&block); }
        }
        let drop_db = 10.0 * (energy(&echo[375 * AEC_BLOCK..]) / residual).log10();
        let erle = aec.erle_db();
        assert!(erle >= 18.0, "recent single-talk ERLE was {erle} dB");
        assert!(drop_db >= 18.0, "output over the final 500 ms dropped by {drop_db} dB");
    }

    #[test]
    fn aec_readapts_after_sub_block_echo_path_shift() {
        let (far, mut echo) = room(750 * AEC_BLOCK);
        let shift_at = 375 * AEC_BLOCK;
        let end = echo.len();
        // Delay every reflection by 1 ms from the midpoint, keeping the render clock steady.
        echo.copy_within(shift_at - 16..end - 16, shift_at);
        let mut aec = AecProcessor::new();
        let mut locked_delay = 0;
        let mut residual = 0.0;
        for (i, (x, d)) in far.chunks_exact(AEC_BLOCK).zip(echo.chunks_exact(AEC_BLOCK)).enumerate() {
            if i == 375 {
                assert!(aec.trained, "the original room path must be trained before it moves");
                locked_delay = aec.delay;
            }
            let mut block = d.try_into().unwrap();
            aec.feed_render(x.try_into().unwrap());
            aec.process_capture(&mut block);
            if i >= 625 { residual += energy(&block); }
        }
        let drop_db = 10.0 * (energy(&echo[625 * AEC_BLOCK..]) / residual).log10();
        assert_eq!(aec.delay, locked_delay, "sub-block shift must re-adapt without a coarse delay relock");
        assert!(drop_db >= 18.0, "after the 1 ms shift the final 500 ms of echo dropped by {drop_db} dB");
    }

    #[test]
    fn aec_preserves_near_end_speech_during_double_talk() {
        // One extra far-only probe block measures the learned path immediately after double-talk.
        let (far, echo) = room(2 * AEC_SAMPLE_RATE + AEC_BLOCK);
        let near_start = 375 * AEC_BLOCK;
        let near_end = 500 * AEC_BLOCK;
        let near = noise(301, near_end - near_start);
        let gain = (energy(&echo[near_start..near_end]) / energy(&near)).sqrt() as f32;
        let mut aec = AecProcessor::new();
        let (mut cross, mut clean_energy, mut out_energy) = (0.0, 0.0, 0.0);
        for (i, (x, d)) in far.chunks_exact(AEC_BLOCK).zip(echo.chunks_exact(AEC_BLOCK)).enumerate() {
            let mut block: [f32; AEC_BLOCK] = d.try_into().unwrap();
            let clean = (375..500).contains(&i).then(|| &near[(i - 375) * AEC_BLOCK..(i - 374) * AEC_BLOCK]);
            if let Some(clean) = clean {
                for (s, &v) in block.iter_mut().zip(clean) { *s += gain * v; }
            }
            aec.feed_render(x.try_into().unwrap());
            aec.process_capture(&mut block);
            if let Some(clean) = clean {
                for (&v, &s) in clean.iter().zip(&block) { cross += f64::from(gain * v) * f64::from(s); }
                clean_energy += f64::from(gain).powi(2) * energy(clean);
                out_energy += energy(&block);
            } else if i == 500 {
                assert!(energy(&block) < 0.1 * energy(d), "post-double-talk echo must remain at least 10 dB down");
            }
        }
        let correlation = cross / (clean_energy * out_energy).sqrt();
        assert!(correlation >= 0.90, "near-end speech correlation during double-talk was {correlation}");
    }

    #[test]
    fn aec_cancels_immediately_after_mute() {
        let (far, echo) = room(525 * AEC_BLOCK);
        let mut blocks = far.chunks_exact(AEC_BLOCK).zip(echo.chunks_exact(AEC_BLOCK));
        let mut aec = AecProcessor::new();
        for (x, d) in blocks.by_ref().take(375) {
            let mut mic = d.try_into().unwrap();
            aec.feed_render(x.try_into().unwrap());
            aec.process_capture(&mut mic);
        }
        assert!(aec.trained, "the echo path must be trained before muting");
        let locked_delay = aec.delay;
        let erle = aec.erle_db();
        for (x, _) in blocks.by_ref().take(125) {
            aec.feed_render(x.try_into().unwrap());
            aec.process_muted();
        }
        assert_eq!(aec.erle_db(), erle, "muted microphone blocks must leave ERLE frozen");
        assert!(aec.fifo_len <= AEC_BLOCK, "muted capture must keep consuming the render FIFO");
        let mut residual = 0.0;
        for (x, d) in blocks {
            let mut mic = d.try_into().unwrap();
            aec.feed_render(x.try_into().unwrap());
            aec.process_capture(&mut mic);
            residual += energy(&mic);
        }
        let drop_db = 10.0 * (energy(&echo[500 * AEC_BLOCK..]) / residual).log10();
        assert_eq!(aec.delay, locked_delay, "unmuting must preserve the learned delay");
        assert!(aec.fifo_len <= AEC_BLOCK, "the resumed capture must stay aligned with render");
        assert!(drop_db >= 18.0, "echo in the first 100 ms after unmute dropped by {drop_db} dB");
    }

    #[test]
    fn aec_unmute_off_cadence_waits_for_full_delay_window() {
        // Join muted. Remote speech starts 49 blocks before the first live mic block.
        let mut far = vec![0.0; 250 * AEC_BLOCK];
        far.extend(noise(42, 450 * AEC_BLOCK));
        let echo = room_echo(&far);
        let mut aec = AecProcessor::new();
        let mut blocks = far.chunks_exact(AEC_BLOCK).zip(echo.chunks_exact(AEC_BLOCK)).enumerate();
        for (_, (x, _)) in blocks.by_ref().take(299) {
            aec.feed_render(x.try_into().unwrap());
            aec.process_muted();
        }
        assert_eq!(aec.blocks, 299, "the first live block must be the 300th capture block");
        let mut residual = 0.0;
        for (i, (x, d)) in blocks {
            let mut mic = d.try_into().unwrap();
            aec.feed_render(x.try_into().unwrap());
            aec.process_capture(&mut mic);
            if i < 323 {
                assert_eq!(aec.delay, 0, "a partial live mic window must not lock the delay on block {i}");
            } else if i == 323 {
                assert!(aec.delay.abs_diff(1360 - 2 * AEC_BLOCK) <= AEC_BLOCK,
                    "the first full window must lock to the room delay on block {i}, got {}", aec.delay);
            }
            if i >= 575 { residual += energy(&mic); }
        }
        let drop_db = 10.0 * (energy(&echo[575 * AEC_BLOCK..]) / residual).log10();
        assert!(drop_db >= 18.0, "join-muted echo drop over the final 500 ms was {drop_db} dB");

        // Already trained. Remote speech pauses during mute, then resumes before unmute.
        let mut far = noise(42, 549 * AEC_BLOCK);
        far[375 * AEC_BLOCK..475 * AEC_BLOCK].fill(0.0);
        let echo = room_echo(&far);
        let mut aec = AecProcessor::new();
        let mut blocks = far.chunks_exact(AEC_BLOCK).zip(echo.chunks_exact(AEC_BLOCK)).enumerate();
        for (_, (x, d)) in blocks.by_ref().take(375) {
            let mut mic = d.try_into().unwrap();
            aec.feed_render(x.try_into().unwrap());
            aec.process_capture(&mut mic);
        }
        assert!(aec.trained, "the room path must be trained before the far-end pause");
        let locked_delay = aec.delay;
        for (_, (x, _)) in blocks.by_ref().take(149) {
            aec.feed_render(x.try_into().unwrap());
            aec.process_muted();
        }
        assert_eq!(aec.blocks, 524, "trained capture must resume one block before the old estimate cadence");
        let mut residual = 0.0;
        for (i, (x, d)) in blocks {
            let mut mic = d.try_into().unwrap();
            aec.feed_render(x.try_into().unwrap());
            aec.process_capture(&mut mic);
            assert_eq!(aec.delay, locked_delay, "trained delay changed after unmute on block {i}");
            assert_eq!(aec.pending_delay, None, "unmuting must not arm a re-lock on block {i}");
            residual += energy(&mic);
        }
        let drop_db = 10.0 * (energy(&echo[524 * AEC_BLOCK..]) / residual).log10();
        assert!(drop_db >= 18.0, "trained echo drop over the first 100 ms after unmute was {drop_db} dB");
    }

    #[test]
    fn render_ref_ring_spsc_never_allocates_and_handles_wraparound() {
        let ring = RenderRefRing::new();
        let mut out = [0.0; AEC_BLOCK];
        for block in 0..2000 {
            let samples = std::array::from_fn::<_, AEC_BLOCK, _>(|i| (block * AEC_BLOCK + i) as f32);
            ring.push_mono(&samples);
            assert_eq!(ring.pop_into(&mut out), AEC_BLOCK, "every block is published whole");
            assert_eq!(out, samples, "FIFO order across wrap at block {block}");
        }
        assert_eq!(ring.overflows(), 0, "lockstep transfers fit");
        let burst: Vec<_> = (0..12_000).map(|i| i as f32).collect();
        ring.push_mono(&burst);
        assert_eq!(ring.overflows(), 1, "one oversized push counts once");
        let mut retained = vec![0.0; RENDER_RING_CAP];
        assert_eq!(ring.pop_into(&mut retained), RENDER_RING_CAP, "retain the available capacity");
        assert_eq!(retained, burst[..RENDER_RING_CAP], "retain the ordered prefix");
        ring.push_mono(&burst);
        assert_eq!(ring.discard(), RENDER_RING_CAP, "flush the pre-gap backlog");
        ring.push_mono(&[9000.0, 9001.0]);
        assert_eq!(ring.pop_into(&mut out), 2, "fresh samples fit after discard");
        assert_eq!(out[..2], [9000.0, 9001.0], "fresh samples arrive in order");
    }

    #[test]
    fn render_ref_ring_mid_push_interleaving_preserves_order() {
        let sequence: Vec<_> = (0..8194).map(|i| i as f32).collect();
        let mut drain = vec![0.0; RENDER_RING_CAP];
        let mut small = [0.0; 4];

        let ring = RenderRefRing::new();
        ring.push_mono(&sequence[..8192]);
        ring.push_mono_inner(&sequence[8192..], || {
            assert_eq!(ring.pop_into(&mut small[..2]), 2, "full ring retains its front");
            assert_eq!(small[..2], sequence[..2], "producer cannot overwrite unread samples");
        });
        assert_eq!(ring.overflows(), 1, "full push counted once");
        assert_eq!(ring.pop_into(&mut drain), 8190, "unread suffix still present");
        assert_eq!(drain[..8190], sequence[2..8192], "full push preserves the suffix");

        let ring = RenderRefRing::new();
        ring.push_mono(&sequence[..8191]);
        assert_eq!(ring.pop_into(&mut drain[..8190]), 8190, "leave one sample before wrap");
        assert_eq!(drain[..8190], sequence[..8190], "initial prefix in order");
        ring.push_mono_inner(&sequence[8191..], || {
            assert_eq!(ring.pop_into(&mut small), 1, "only the published sample is visible");
            assert_eq!(small[0], 8190.0, "wrapped writes preserve the old front");
        });
        assert_eq!(ring.pop_into(&mut small), 3, "whole wrapped push becomes visible");
        assert_eq!(small[..3], sequence[8191..], "wrapped samples stay in order");
        assert_eq!(ring.overflows(), 0, "wrapped push fits");

        let ring = RenderRefRing::new();
        ring.push_mono(&sequence[..8192]);
        assert_eq!(ring.pop_into(&mut small[..1]), 1, "free one slot");
        assert_eq!(small[0], 0.0, "freed the oldest sample");
        ring.push_mono_inner(&sequence[8192..], || {
            assert_eq!(ring.pop_into(&mut small[..2]), 2, "partial push leaves old samples visible");
            assert_eq!(small[..2], sequence[1..3], "partial push leaves old samples intact");
        });
        assert_eq!(ring.overflows(), 1, "partial push counts once");
        assert_eq!(ring.pop_into(&mut drain), 8190, "only the fitting prefix was added");
        assert_eq!(drain[..8190], sequence[3..8193], "partial push extends the old suffix in order");

        ring.push_mono(&sequence);
        assert_eq!(ring.discard(), RENDER_RING_CAP, "discard a full pre-gap backlog");
        assert_eq!(ring.pop_into(&mut small), 0, "discard returns all published slots");
        ring.push_mono(&[9000.0, 9001.0]);
        assert_eq!(ring.pop_into(&mut small), 2, "fresh push fits");
        assert_eq!(small[..2], [9000.0, 9001.0], "fresh audio follows the gap");
        let ring = RenderRefRing::new();
        ring.push_mono(&[10.0, 11.0]);
        ring.push_mono_inner(&[12.0, 13.0], || {
            assert_eq!(ring.discard(), 2, "discard observes only published samples");
        });
        assert_eq!(ring.pop_into(&mut small), 2, "pending push survives discard");
        assert_eq!(small[..2], [12.0, 13.0], "pending push retains FIFO order");
    }
}
