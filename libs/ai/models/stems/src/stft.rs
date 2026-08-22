//! Torch-parity STFT / iSTFT for the BS-RoFormer stem separator.
//!
//! Dependency-free (`std` only). Everything here reproduces `torch.stft` /
//! `torch.istft` bit-for-bit-ish (f64 internals, so the residual difference is
//! torch's own f32 rounding, not ours) for the parameter set the model uses:
//!
//! ```text
//! n_fft = 2048, hop_length = 441, win_length = 2048
//! window = torch.hann_window(2048)   (periodic)
//! center = true, pad_mode = "reflect", onesided = true, normalized = false
//! ```
//!
//! Performance: a separation pass runs >11k transforms per 11-second chunk, so
//! the FFT is an iterative in-place radix-2 Cooley-Tukey over the *half-length*
//! complex spectrum (the real-FFT trick): a 2048-point real transform costs one
//! 1024-point complex FFT plus a linear post-twiddle pass. All twiddle factors
//! and the bit-reversal permutation are precomputed in [`Stft::new`]; the
//! per-call scratch buffers are allocated once and reused across every frame.

use std::f64::consts::PI;

pub const N_FFT: usize = 2048;
pub const HOP: usize = 441;
pub const WIN: usize = 2048;
pub const BINS: usize = N_FFT / 2 + 1; // 1025

/// Envelope values below this are treated as "no coverage" in [`Stft::inverse`]
/// (torch warns and divides anyway, producing inf/NaN; we emit silence).
const ENVELOPE_EPS: f64 = 1e-11;

/// Periodic Hann window, matching `torch.hann_window(n)` (`periodic=True`, the
/// default): `w[i] = 0.5 - 0.5*cos(2*pi*i / n)`, so `w[0] == 0` and the window
/// is *not* symmetric (that would divide by `n - 1`).
///
/// The arithmetic deliberately mirrors torch's, which builds the window in the
/// output dtype: `hann_window` delegates to `hamming_window(alpha=beta=0.5)`,
/// which does `arange(n+1).mul_(2*pi/n).cos_().mul_(-beta).add_(alpha)` on an
/// **f32** tensor and then narrows to `n`. Computing the phase in f64 instead
/// is *more* accurate but drifts from the fixture by ~2.1e-7, dominated by
/// torch rounding `2*pi/n` to f32 before the multiply.
pub fn hann_periodic(n: usize) -> Vec<f32> {
    if n == 0 {
        return Vec::new();
    }
    if n == 1 {
        // torch.hann_window(1) is [1.0] regardless of periodicity.
        return vec![1.0];
    }
    let step = (2.0 * PI / n as f64) as f32;
    (0..n)
        .map(|i| {
            let c = (i as f32 * step).cos();
            c * -0.5f32 + 0.5f32
        })
        .collect()
}

/// Number of frames `torch.stft(..., center=true, hop_length=HOP)` produces for
/// `samples` input samples.
///
/// With `center=true` the signal is reflect-padded by `n_fft/2` on both sides,
/// so the padded length is `samples + n_fft`, and
/// `frames = 1 + (samples + n_fft - n_fft) / hop = 1 + samples / hop`.
pub fn frame_count(samples: usize) -> usize {
    1 + samples / HOP
}

/// Map a (possibly out-of-range) index into `[0, len)` with numpy/torch
/// `"reflect"` semantics: the edge sample is *not* repeated, i.e. for
/// `[a, b, c]` a pad of 2 yields `[c, b, | a, b, c | b, a]`.
#[inline]
fn reflect_index(i: isize, len: usize) -> usize {
    debug_assert!(len > 0);
    if len == 1 {
        return 0;
    }
    let n = len as isize;
    let period = 2 * (n - 1);
    let mut k = i % period;
    if k < 0 {
        k += period;
    }
    if k >= n {
        k = period - k;
    }
    k as usize
}

/// Precomputed real-FFT plan plus the analysis/synthesis window.
///
/// Construct once and reuse: `new` does all the trig.
pub struct Stft {
    n_fft: usize,
    hop: usize,
    win_length: usize,
    /// Size of the complex FFT that backs the real transform: `n_fft / 2`.
    m: usize,
    bins: usize,
    /// Window of length `n_fft` (zero-padded and centered if `win_length < n_fft`).
    window: Vec<f64>,
    /// `window[i]^2`, for the overlap-add normalisation in `inverse`.
    window_sq: Vec<f64>,
    /// Bit-reversal permutation for the `m`-point complex FFT.
    rev: Vec<u32>,
    /// Per-stage twiddles, interleaved `(re, im)`: `exp(-2*pi*i*j/len)`.
    stage_tw: Vec<Vec<f64>>,
    /// Conjugated per-stage twiddles, for the inverse FFT.
    stage_tw_inv: Vec<Vec<f64>>,
    /// Real-FFT post/pre twiddles, interleaved: `exp(-2*pi*i*k/n_fft)`, `k < m`.
    rtw: Vec<f64>,
}

impl Stft {
    pub fn new(n_fft: usize, hop: usize, win_length: usize) -> Self {
        assert!(
            n_fft >= 2 && n_fft.is_power_of_two(),
            "Stft: n_fft must be a power of two >= 2 (got {n_fft}); \
             the radix-2 real-FFT plan has no mixed-radix fallback"
        );
        assert!(hop >= 1, "Stft: hop must be >= 1 (got {hop})");
        assert!(
            win_length >= 1 && win_length <= n_fft,
            "Stft: win_length must be in 1..=n_fft (got {win_length}, n_fft {n_fft})"
        );

        let m = n_fft / 2;
        let bins = n_fft / 2 + 1;

        // Window, centered inside n_fft exactly like torch does when
        // win_length < n_fft.
        let mut window = vec![0.0f64; n_fft];
        let left = (n_fft - win_length) / 2;
        for (i, w) in hann_periodic(win_length).into_iter().enumerate() {
            window[left + i] = w as f64;
        }
        let window_sq: Vec<f64> = window.iter().map(|w| w * w).collect();

        // Bit-reversal permutation for m points.
        let bits = m.trailing_zeros();
        let mut rev = vec![0u32; m];
        for i in 1..m {
            rev[i] = (rev[i >> 1] >> 1) | (((i & 1) as u32) << (bits - 1));
        }

        // Per-stage twiddles. Stage `s` handles butterflies of span
        // `len = 2 << s`, and needs `len/2` factors.
        let mut stage_tw = Vec::new();
        let mut stage_tw_inv = Vec::new();
        let mut len = 2usize;
        while len <= m {
            let half = len >> 1;
            let mut fwd = Vec::with_capacity(half * 2);
            let mut inv = Vec::with_capacity(half * 2);
            for j in 0..half {
                let ang = -2.0 * PI * j as f64 / len as f64;
                let (s, c) = ang.sin_cos();
                fwd.push(c);
                fwd.push(s);
                inv.push(c);
                inv.push(-s);
            }
            stage_tw.push(fwd);
            stage_tw_inv.push(inv);
            len <<= 1;
        }

        // Real-FFT twiddles over the full transform length.
        let mut rtw = Vec::with_capacity(m * 2);
        for k in 0..m {
            let ang = -2.0 * PI * k as f64 / n_fft as f64;
            let (s, c) = ang.sin_cos();
            rtw.push(c);
            rtw.push(s);
        }

        Self { n_fft, hop, win_length, m, bins, window, window_sq, rev, stage_tw, stage_tw_inv, rtw }
    }

    /// The model's plan: `n_fft = 2048`, `hop = 441`, `win_length = 2048`.
    pub fn bs_roformer() -> Self {
        Self::new(N_FFT, HOP, WIN)
    }

    pub fn bins(&self) -> usize {
        self.bins
    }

    pub fn n_fft(&self) -> usize {
        self.n_fft
    }

    pub fn hop(&self) -> usize {
        self.hop
    }

    pub fn win_length(&self) -> usize {
        self.win_length
    }

    /// The analysis window, length `n_fft`.
    pub fn window(&self) -> &[f64] {
        &self.window
    }

    /// Number of frames `torch.stft(center=true)` produces for `samples` samples.
    pub fn frame_count(&self, samples: usize) -> usize {
        1 + samples / self.hop
    }

    /// Forward STFT of one real channel.
    ///
    /// Output layout: interleaved complex at `(bin * frames + frame) * 2 + c`,
    /// `c = 0` real / `c = 1` imaginary — i.e. exactly
    /// `torch.view_as_real(torch.stft(x, ...))` for a `(bins, frames)` spectrum.
    /// Output length is `bins * frames * 2`.
    pub fn forward(&self, signal: &[f32]) -> (Vec<f32>, usize) {
        let samples = signal.len();
        let frames = self.frame_count(samples);
        let pad = self.n_fft / 2;
        let mut out = vec![0.0f32; self.bins * frames * 2];

        if samples == 0 {
            // torch errors here; an all-zero spectrum is the sane degenerate answer.
            return (out, frames);
        }

        // center=true: reflect-pad by n_fft/2 on both ends, once.
        let padded_len = samples + 2 * pad;
        let mut padded = vec![0.0f32; padded_len];
        for (i, p) in padded.iter_mut().enumerate() {
            *p = signal[reflect_index(i as isize - pad as isize, samples)];
        }

        // Scratch, allocated once and reused by every frame.
        let mut time = vec![0.0f64; self.n_fft];
        let mut buf = vec![0.0f64; self.m * 2];
        let mut spec = vec![0.0f64; self.bins * 2];

        for t in 0..frames {
            let off = t * self.hop;
            let src = &padded[off..off + self.n_fft];
            for ((dst, s), w) in time.iter_mut().zip(src.iter()).zip(self.window.iter()) {
                *dst = *s as f64 * *w;
            }
            self.rfft(&time, &mut buf, &mut spec);
            for b in 0..self.bins {
                let o = (b * frames + t) * 2;
                out[o] = spec[b * 2] as f32;
                out[o + 1] = spec[b * 2 + 1] as f32;
            }
        }
        (out, frames)
    }

    /// Inverse STFT (`torch.istft(..., length)`).
    ///
    /// `spec` uses the same layout [`Stft::forward`] returns. Each frame is
    /// inverse-transformed, multiplied by the window and overlap-added; the
    /// overlap-add of `window^2` is accumulated separately and divided out; then
    /// the leading `n_fft/2` samples are dropped and exactly `length` samples
    /// are returned (zero-filled if the frames do not reach that far).
    pub fn inverse(&self, spec: &[f32], frames: usize, length: usize) -> Vec<f32> {
        let mut out = vec![0.0f32; length];
        if frames == 0 || length == 0 {
            return out;
        }
        assert_eq!(
            spec.len(),
            self.bins * frames * 2,
            "Stft::inverse: spec length {} does not match bins {} * frames {} * 2",
            spec.len(),
            self.bins,
            frames
        );

        let pad = self.n_fft / 2;
        let span = (frames - 1) * self.hop + self.n_fft;
        let mut acc = vec![0.0f64; span];
        let mut env = vec![0.0f64; span];

        // Scratch, allocated once and reused by every frame.
        let mut half = vec![0.0f64; self.bins * 2];
        let mut buf = vec![0.0f64; self.m * 2];
        let mut time = vec![0.0f64; self.n_fft];

        for t in 0..frames {
            for b in 0..self.bins {
                let o = (b * frames + t) * 2;
                half[b * 2] = spec[o] as f64;
                half[b * 2 + 1] = spec[o + 1] as f64;
            }
            self.irfft(&half, &mut buf, &mut time);

            let off = t * self.hop;
            let a = &mut acc[off..off + self.n_fft];
            for ((dst, y), w) in a.iter_mut().zip(time.iter()).zip(self.window.iter()) {
                *dst += *y * *w;
            }
            let e = &mut env[off..off + self.n_fft];
            for (dst, w2) in e.iter_mut().zip(self.window_sq.iter()) {
                *dst += *w2;
            }
        }

        let avail = span.saturating_sub(pad).min(length);
        for i in 0..avail {
            let p = i + pad;
            let e = env[p];
            out[i] = if e.abs() > ENVELOPE_EPS { (acc[p] / e) as f32 } else { 0.0 };
        }
        out
    }

    // ---- real FFT core -----------------------------------------------------

    /// Real forward FFT of `time` (`n_fft` samples) into `spec`
    /// (`bins` interleaved complex values). `buf` is `2 * m` scratch.
    ///
    /// The N-point real transform is one N/2-point complex FFT of
    /// `z[k] = x[2k] + i*x[2k+1]` followed by a post-twiddle split:
    /// `X[k] = E[k] + W_N^k * O[k]` with
    /// `E[k] = (Z[k] + conj(Z[M-k]))/2`, `O[k] = -i*(Z[k] - conj(Z[M-k]))/2`.
    fn rfft(&self, time: &[f64], buf: &mut [f64], spec: &mut [f64]) {
        let m = self.m;
        debug_assert_eq!(time.len(), self.n_fft);
        debug_assert_eq!(buf.len(), 2 * m);
        debug_assert_eq!(spec.len(), 2 * self.bins);

        for (dst, src) in buf.chunks_exact_mut(2).zip(time.chunks_exact(2)) {
            dst[0] = src[0];
            dst[1] = src[1];
        }
        self.fft_inplace(buf, false);

        // DC and Nyquist are real and fall out of the k=0 case directly.
        let z0r = buf[0];
        let z0i = buf[1];
        spec[0] = z0r + z0i;
        spec[1] = 0.0;
        spec[2 * m] = z0r - z0i;
        spec[2 * m + 1] = 0.0;

        for k in 1..m {
            let ar = buf[2 * k];
            let ai = buf[2 * k + 1];
            // B = conj(Z[m-k])
            let br = buf[2 * (m - k)];
            let bi = -buf[2 * (m - k) + 1];
            let er = 0.5 * (ar + br);
            let ei = 0.5 * (ai + bi);
            let dr = ar - br;
            let di = ai - bi;
            // O = -i*(A - B)/2
            let or = 0.5 * di;
            let oi = -0.5 * dr;
            let wr = self.rtw[2 * k];
            let wi = self.rtw[2 * k + 1];
            spec[2 * k] = er + (wr * or - wi * oi);
            spec[2 * k + 1] = ei + (wr * oi + wi * or);
        }
    }

    /// Inverse real FFT: `spec` (`bins` interleaved complex) into `time`
    /// (`n_fft` real samples), normalised by `1/n_fft` like `torch.fft.irfft`.
    /// The imaginary parts of DC and Nyquist are ignored (they must be zero for
    /// a Hermitian spectrum, and torch's C2R path ignores them too).
    fn irfft(&self, spec: &[f64], buf: &mut [f64], time: &mut [f64]) {
        let m = self.m;
        debug_assert_eq!(time.len(), self.n_fft);
        debug_assert_eq!(buf.len(), 2 * m);
        debug_assert_eq!(spec.len(), 2 * self.bins);

        let x0 = spec[0];
        let xm = spec[2 * m];
        buf[0] = 0.5 * (x0 + xm);
        buf[1] = 0.5 * (x0 - xm);

        for k in 1..m {
            let ar = spec[2 * k];
            let ai = spec[2 * k + 1];
            // B = conj(X[m-k])
            let br = spec[2 * (m - k)];
            let bi = -spec[2 * (m - k) + 1];
            let er = 0.5 * (ar + br);
            let ei = 0.5 * (ai + bi);
            let dr = 0.5 * (ar - br);
            let di = 0.5 * (ai - bi);
            // O = conj(W_N^k) * D
            let wr = self.rtw[2 * k];
            let wi = self.rtw[2 * k + 1];
            let or = wr * dr + wi * di;
            let oi = wr * di - wi * dr;
            // Z = E + i*O
            buf[2 * k] = er - oi;
            buf[2 * k + 1] = ei + or;
        }

        self.fft_inplace(buf, true);

        for (dst, src) in time.chunks_exact_mut(2).zip(buf.chunks_exact(2)) {
            dst[0] = src[0];
            dst[1] = src[1];
        }
    }

    /// Iterative in-place radix-2 decimation-in-time complex FFT over `m`
    /// interleaved complex values. `inverse` conjugates the twiddles and applies
    /// the `1/m` scale. No allocation, no recursion.
    fn fft_inplace(&self, buf: &mut [f64], inverse: bool) {
        let m = self.m;
        debug_assert_eq!(buf.len(), 2 * m);

        for i in 0..m {
            let j = self.rev[i] as usize;
            if i < j {
                buf.swap(2 * i, 2 * j);
                buf.swap(2 * i + 1, 2 * j + 1);
            }
        }

        let mut len = 2usize;
        let mut stage = 0usize;
        while len <= m {
            let half = len >> 1;
            let tw: &[f64] =
                if inverse { &self.stage_tw_inv[stage] } else { &self.stage_tw[stage] };
            for block in buf.chunks_exact_mut(2 * len) {
                let (lo, hi) = block.split_at_mut(2 * half);
                for ((w, a), b) in
                    tw.chunks_exact(2).zip(lo.chunks_exact_mut(2)).zip(hi.chunks_exact_mut(2))
                {
                    let (wr, wi) = (w[0], w[1]);
                    let (br, bi) = (b[0], b[1]);
                    let tr = br * wr - bi * wi;
                    let ti = br * wi + bi * wr;
                    let (ar, ai) = (a[0], a[1]);
                    a[0] = ar + tr;
                    a[1] = ai + ti;
                    b[0] = ar - tr;
                    b[1] = ai - ti;
                }
            }
            len <<= 1;
            stage += 1;
        }

        if inverse {
            let s = 1.0 / m as f64;
            for v in buf.iter_mut() {
                *v *= s;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use std::time::Instant;

    /// `libs/ai/models/stems` -> up 4 -> repo root.
    const GOLDEN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../../local/stems_ref/golden");

    fn golden_dir() -> Option<PathBuf> {
        let p = PathBuf::from(GOLDEN);
        if p.is_dir() {
            Some(p)
        } else {
            None
        }
    }

    /// Minimal `.npy` reader: version 1/2 header, little-endian `<f4`, C order.
    fn read_npy(path: &Path) -> (Vec<f32>, Vec<usize>) {
        let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("read {path:?}: {e}"));
        assert!(bytes.len() > 10, "{path:?}: too short for an npy header");
        assert_eq!(&bytes[0..6], b"\x93NUMPY", "{path:?}: bad npy magic");
        let major = bytes[6];
        let (hlen, body) = match major {
            1 => (u16::from_le_bytes([bytes[8], bytes[9]]) as usize, 10usize),
            2 => (
                u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize,
                12usize,
            ),
            v => panic!("{path:?}: unsupported npy version {v}"),
        };
        let header = std::str::from_utf8(&bytes[body..body + hlen])
            .unwrap_or_else(|e| panic!("{path:?}: non-utf8 header: {e}"));
        assert!(header.contains("'<f4'"), "{path:?}: expected '<f4', header: {header}");
        assert!(
            header.contains("'fortran_order': False"),
            "{path:?}: expected C order, header: {header}"
        );

        let shape_start = header.find("'shape':").expect("npy header has no shape");
        let open = header[shape_start..].find('(').unwrap() + shape_start + 1;
        let close = header[open..].find(')').unwrap() + open;
        let shape: Vec<usize> = header[open..close]
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.parse().unwrap())
            .collect();

        let data = &bytes[body + hlen..];
        assert_eq!(data.len() % 4, 0, "{path:?}: payload is not a whole number of f32");
        let out: Vec<f32> = data
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect();
        let n: usize = shape.iter().product();
        assert_eq!(out.len(), n, "{path:?}: payload {} != shape product {n}", out.len());
        (out, shape)
    }

    fn max_abs_diff(a: &[f32], b: &[f32]) -> f64 {
        assert_eq!(a.len(), b.len(), "length mismatch: {} vs {}", a.len(), b.len());
        a.iter().zip(b.iter()).fold(0.0f64, |m, (x, y)| m.max((*x as f64 - *y as f64).abs()))
    }

    /// Deterministic test signal: a few sines plus a small LCG dither.
    fn synth_signal(n: usize) -> Vec<f32> {
        let mut state: u32 = 0x1234_5678;
        (0..n)
            .map(|i| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let noise = (state >> 8) as f32 / (1 << 24) as f32 - 0.5;
                let t = i as f32;
                0.6 * (t * 0.021).sin() + 0.3 * (t * 0.173 + 1.1).sin() + 0.1 * (t * 0.9).cos()
                    + 0.05 * noise
            })
            .collect()
    }

    #[test]
    fn hann_matches_torch() {
        let Some(dir) = golden_dir() else {
            eprintln!("hann_matches_torch: SKIP, {GOLDEN} not present");
            return;
        };
        let (reference, shape) = read_npy(&dir.join("hann2048.npy"));
        assert_eq!(shape, vec![2048]);
        let ours = hann_periodic(2048);
        let d = max_abs_diff(&ours, &reference);
        eprintln!("hann_matches_torch: max abs diff = {d:.3e}");
        assert_eq!(ours[0], 0.0, "periodic hann must start at 0");
        assert!(d < 1e-7, "hann max abs diff {d:.3e} >= 1e-7");
    }

    #[test]
    fn frame_count_matches_torch() {
        assert_eq!(frame_count(5000), 12);
        assert_eq!(Stft::bs_roformer().frame_count(5000), 12);
        // 11 s at 44.1 kHz, the chunk size the separator runs on.
        assert_eq!(frame_count(441 * 1000), 1001);
        if let Some(dir) = golden_dir() {
            let meta = std::fs::read_to_string(dir.join("stft_meta.json")).unwrap();
            assert!(meta.contains("\"frames\": 12"), "meta says: {meta}");
        }
    }

    #[test]
    fn forward_matches_torch() {
        let Some(dir) = golden_dir() else {
            eprintln!("forward_matches_torch: SKIP, {GOLDEN} not present");
            return;
        };
        let (signal, sshape) = read_npy(&dir.join("stft_in.npy"));
        assert_eq!(sshape, vec![5000]);
        let (reference, rshape) = read_npy(&dir.join("stft_out.npy"));
        assert_eq!(rshape, vec![BINS, 12, 2]);

        let stft = Stft::bs_roformer();
        let (ours, frames) = stft.forward(&signal);
        assert_eq!(frames, 12);
        assert_eq!(ours.len(), reference.len());

        let mut max_abs = 0.0f64;
        let mut max_rel = 0.0f64;
        let mut peak = 0.0f64;
        for (a, b) in ours.iter().zip(reference.iter()) {
            let (a, b) = (*a as f64, *b as f64);
            let d = (a - b).abs();
            max_abs = max_abs.max(d);
            peak = peak.max(b.abs());
            if b.abs() > 1.0 {
                max_rel = max_rel.max(d / b.abs());
            }
        }
        eprintln!(
            "forward_matches_torch: max abs diff = {max_abs:.3e}, \
             max rel diff (|ref|>1) = {max_rel:.3e}, ref peak = {peak:.3}"
        );
        assert!(max_abs < 2e-3, "forward max abs diff {max_abs:.3e} >= 2e-3");
        assert!(max_rel < 1e-4, "forward max rel diff {max_rel:.3e} >= 1e-4");
    }

    #[test]
    fn inverse_matches_torch() {
        let Some(dir) = golden_dir() else {
            eprintln!("inverse_matches_torch: SKIP, {GOLDEN} not present");
            return;
        };
        let (spec, sshape) = read_npy(&dir.join("stft_out.npy"));
        assert_eq!(sshape, vec![BINS, 12, 2]);
        let (reference, rshape) = read_npy(&dir.join("istft_out.npy"));
        assert_eq!(rshape, vec![5000]);

        let ours = Stft::bs_roformer().inverse(&spec, 12, 5000);
        let d = max_abs_diff(&ours, &reference);
        eprintln!("inverse_matches_torch: max abs diff = {d:.3e}");
        assert!(d < 1e-4, "inverse max abs diff {d:.3e} >= 1e-4");
    }

    #[test]
    fn roundtrip() {
        let stft = Stft::bs_roformer();
        let signal = synth_signal(5000);
        let (spec, frames) = stft.forward(&signal);
        let back = stft.inverse(&spec, frames, signal.len());
        assert_eq!(back.len(), signal.len());

        // Skip the first/last n_fft samples: torch has the same edge behaviour
        // where the window-square envelope is only partially accumulated.
        let lo = N_FFT;
        let hi = signal.len() - N_FFT;
        let d = max_abs_diff(&signal[lo..hi], &back[lo..hi]);
        eprintln!("roundtrip: interior max abs diff = {d:.3e} over {} samples", hi - lo);
        assert!(d < 1e-4, "roundtrip interior max abs diff {d:.3e} >= 1e-4");
    }

    #[test]
    fn rfft_matches_naive_dft() {
        const N: usize = 64;
        let stft = Stft::new(N, 16, N);
        let x: Vec<f64> = synth_signal(N).into_iter().map(|v| v as f64).collect();

        let mut buf = vec![0.0f64; stft.m * 2];
        let mut spec = vec![0.0f64; stft.bins * 2];
        stft.rfft(&x, &mut buf, &mut spec);

        // Naive O(n^2) DFT, onesided.
        let mut max_fwd = 0.0f64;
        for k in 0..stft.bins {
            let (mut re, mut im) = (0.0f64, 0.0f64);
            for (n, v) in x.iter().enumerate() {
                let ang = -2.0 * PI * (k * n) as f64 / N as f64;
                re += v * ang.cos();
                im += v * ang.sin();
            }
            max_fwd = max_fwd.max((spec[2 * k] - re).abs()).max((spec[2 * k + 1] - im).abs());
        }
        eprintln!("rfft_matches_naive_dft: forward max abs diff = {max_fwd:.3e}");
        assert!(max_fwd < 1e-4, "rfft vs naive DFT: {max_fwd:.3e} >= 1e-4");

        // And the inverse must undo it.
        let mut back = vec![0.0f64; N];
        stft.irfft(&spec, &mut buf, &mut back);
        let max_inv =
            x.iter().zip(back.iter()).fold(0.0f64, |m, (a, b)| m.max((a - b).abs()));
        eprintln!("rfft_matches_naive_dft: irfft roundtrip max abs diff = {max_inv:.3e}");
        assert!(max_inv < 1e-4, "irfft roundtrip: {max_inv:.3e} >= 1e-4");
    }

    #[test]
    fn reflect_padding_matches_numpy() {
        // np.pad([0,1,2,3], 3, mode='reflect') == [3,2,1, 0,1,2,3, 2,1,0]
        let len = 4;
        let got: Vec<usize> = (-3..7).map(|i| reflect_index(i, len)).collect();
        assert_eq!(got, vec![3, 2, 1, 0, 1, 2, 3, 2, 1, 0]);
        assert_eq!(reflect_index(-5, 1), 0);
    }

    /// Not an assertion of speed, just a visible number: `inverse` over the
    /// frame count a full ~11k-transform separation pass produces.
    #[test]
    fn inverse_timing() {
        const FRAMES: usize = 8808;
        let stft = Stft::bs_roformer();
        let mut spec = vec![0.0f32; BINS * FRAMES * 2];
        let mut state: u32 = 0xC0FF_EE01;
        for v in spec.iter_mut() {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            *v = (state >> 8) as f32 / (1 << 24) as f32 - 0.5;
        }
        let t0 = Instant::now();
        let out = stft.inverse(&spec, FRAMES, (FRAMES - 1) * HOP);
        let dt = t0.elapsed();
        eprintln!("inverse_timing: {FRAMES} frames -> {} samples in {dt:?}", out.len());

        let t1 = Instant::now();
        let (fwd, f) = stft.forward(&out);
        let dt2 = t1.elapsed();
        eprintln!("inverse_timing: forward {f} frames in {dt2:?} ({} values)", fwd.len());
    }
}
