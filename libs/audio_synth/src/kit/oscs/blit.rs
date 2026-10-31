//! The band-limited step engine shared by Classic, Wavetable and S&H Noise:
//! each step of the waveform is a windowed-sinc impulse written at its exact
//! fractional time into a ring buffer, and a leaky integrator turns the
//! impulses back into steps. Classic adds ramps through a running slope
//! (the DC buffer) subtracted before integration.

use std::f64::consts::PI;
use std::sync::OnceLock;

/// Sub-sample rows of the kernel.
pub const ROWS: usize = 256;
/// Taps per impulse.
pub const TAPS: usize = 12;
/// Ring length: two blocks plus the kernel overlap.
const RING: usize = 128;
const BUF: usize = RING + TAPS;

/// A 12-tap windowed-sinc kernel at 257 sub-sample positions plus the
/// per-row difference for linear refinement inside a row.
pub struct Kernel {
    pub h: [[f32; TAPS]; ROWS + 1],
    pub dh: [[f32; TAPS]; ROWS],
}

fn blackman(t: f64, taps: f64) -> f64 {
    // Maximal at t = 0, period `taps` (centred on t - taps/2).
    let x = (t - taps * 0.5) / taps;
    0.42 - 0.5 * (2.0 * PI * x).cos() + 0.08 * (4.0 * PI * x).cos()
}

fn sinc(x: f64) -> f64 {
    if x.abs() < 1e-12 { 1.0 } else { (PI * x).sin() / (PI * x) }
}

impl Kernel {
    fn build(c: f64) -> Kernel {
        let mut k = Kernel { h: [[0.0; TAPS]; ROWS + 1], dh: [[0.0; TAPS]; ROWS] };
        for j in 0..=ROWS {
            for i in 0..TAPS {
                let t = 5.0 - i as f64 + j as f64 / ROWS as f64;
                // Blackman window over (t - 6) / 12: maximal at t = 0.
                let w = blackman(t, 12.0);
                k.h[j][i] = (w * c * sinc(c * t)) as f32;
            }
        }
        for j in 0..ROWS {
            for i in 0..TAPS {
                k.dh[j][i] = (k.h[j + 1][i] - k.h[j][i]) / 65536.0;
            }
        }
        k
    }
}

/// The step kernel (cutoff 0.455).
pub fn kernel() -> &'static Kernel {
    static K: OnceLock<Kernel> = OnceLock::new();
    K.get_or_init(|| Kernel::build(0.455))
}

/// The wider kernel (cutoff 0.85).
pub fn kernel_wide() -> &'static Kernel {
    static K: OnceLock<Kernel> = OnceLock::new();
    K.get_or_init(|| Kernel::build(0.85))
}

/// The 8-tap int16 interpolator (cutoff 1.0): row j tap i at t = 3 - i + j/256.
pub fn sinc8() -> &'static [[i32; 8]; 257] {
    static K: OnceLock<[[i32; 8]; 257]> = OnceLock::new();
    K.get_or_init(|| {
        let mut k = [[0i32; 8]; 257];
        for (j, row) in k.iter_mut().enumerate() {
            for (i, v) in row.iter_mut().enumerate() {
                let t = 3.0 - i as f64 + j as f64 / 256.0;
                let w = blackman(t, 8.0);
                *v = (w * sinc(t) * 16384.0) as i32;
            }
        }
        k
    })
}

/// The impulse ring (stereo) plus the slope (DC) ring and the integrators.
#[derive(Clone)]
pub struct BlitBuf {
    pub l: [f32; BUF],
    pub r: [f32; BUF],
    pub dc: [f32; BUF],
    pub read: usize,
    pub dcacc: f32,
    pub y: [f32; 2],
    pub leak: f32,
    pub leak_d: f32,
    leak_started: bool,
}

impl Default for BlitBuf {
    fn default() -> Self {
        BlitBuf { l: [0.0; BUF], r: [0.0; BUF], dc: [0.0; BUF], read: 0, dcacc: 0.0, y: [0.0; 2], leak: 0.0, leak_d: 0.0, leak_started: false }
    }
}

impl BlitBuf {
    pub fn reset(&mut self) {
        *self = BlitBuf::default();
    }

    /// Place a step of height (gl, gr) `pos` samples (0..64) after the read
    /// position. `stereo` false writes only the left ring.
    #[inline]
    pub fn place(&mut self, pos: f32, gl: f32, gr: f32, stereo: bool) -> usize {
        let ipos = ((pos.max(0.0) as f64) * 16_777_216.0) as i64;
        let d = ((ipos >> 24) & 63) as usize;
        let frac = (ipos & 0xFF_FFFF) as u32;
        self.place_at(d, frac, gl, gr, stereo);
        d
    }

    /// Place at integer delay `d` and 24-bit sub-sample fraction `frac`.
    #[inline]
    pub fn place_at(&mut self, d: usize, frac: u32, gl: f32, gr: f32, stereo: bool) {
        let k = kernel();
        let j = ((frac >> 16) & 255) as usize;
        let lf = (frac & 0xFFFF) as f32;
        let base = self.read + d;
        let (h, dh) = (&k.h[j], &k.dh[j]);
        for i in 0..TAPS {
            let v = h[i] + lf * dh[i];
            self.l[base + i] += gl * v;
        }
        if stereo {
            for i in 0..TAPS {
                let v = h[i] + lf * dh[i];
                self.r[base + i] += gr * v;
            }
        }
    }

    /// A slope change, `d` = the impulse's integer delay.
    #[inline]
    pub fn add_slope(&mut self, d: usize, ds: f32) {
        self.dc[self.read + 6 + d] += ds;
    }

    /// Set the per-block leak target (block line across 64 samples).
    pub fn set_leak(&mut self, target: f32) {
        if !self.leak_started {
            self.leak = target;
            self.leak_d = 0.0;
            self.leak_started = true;
        } else {
            self.leak_d = (target - self.leak) / 64.0;
        }
    }

    /// The integrator leak for frequency `f`, host rate `fs`, `loss`.
    pub fn leak_target(f: f32, fs: f32, loss: f32) -> f32 {
        let base = (1.0 - 2.0 * 20.0 / fs).powi(2);
        let fs_os = 2.0 * fs;
        base.min(loss.powf(4.0 * (f / fs_os).min(1.0)))
    }

    /// Integrate one block out of the ring into `l`/`r`, `ramp` = the DC
    /// scale (unison gain x pitch mult; 0 for no ramps), then consume.
    pub fn integrate(&mut self, ramp: f32, stereo: bool, l: &mut [f32; 64], r: &mut [f32; 64]) {
        let rd = self.read;
        let mut hp = self.leak;
        for k in 0..64 {
            self.dcacc += self.dc[rd + k];
            let dcv = self.dcacc * ramp;
            self.y[0] = hp * self.y[0] + self.l[rd + k] - dcv;
            l[k] = self.y[0];
            if stereo {
                self.y[1] = hp * self.y[1] + self.r[rd + k] - dcv;
                r[k] = self.y[1];
            }
            hp += self.leak_d;
        }
        self.leak = hp;
        self.leak_d = 0.0;
        self.consume();
    }

    fn consume(&mut self) {
        let rd = self.read;
        self.l[rd..rd + 64].fill(0.0);
        self.r[rd..rd + 64].fill(0.0);
        self.dc[rd..rd + 64].fill(0.0);
        self.read = (rd + 64) % RING;
        if self.read == 0 {
            for b in [&mut self.l, &mut self.r, &mut self.dc] {
                let (head, tail) = b.split_at_mut(RING);
                head[..TAPS].copy_from_slice(tail);
                tail.fill(0.0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kernel_sums_to_dc_gain_and_peaks_at_centre() {
        let k = kernel();
        for j in [0usize, 64, 128, 255] {
            let s: f32 = k.h[j].iter().sum();
            // c * sum sinc(c t) ~ 1 for a band-limited unit step.
            assert!((s - 1.0).abs() < 0.02, "{j} {s}");
        }
        // Row 0: t = 5 - i, peak at i = 5.
        let row = &k.h[0];
        let imax = (0..TAPS).max_by(|a, b| row[*a].partial_cmp(&row[*b]).unwrap()).unwrap();
        assert_eq!(imax, 5);
        let s8 = sinc8();
        let s: i32 = s8[0].iter().sum();
        assert!((s - 16384).abs() < 200, "{s}");
    }

    #[test]
    fn step_integrates_to_unit_level() {
        let mut b = BlitBuf::default();
        b.set_leak(1.0);
        b.place(10.25, 1.0, 1.0, false);
        let (mut l, mut r) = ([0.0; 64], [0.0; 64]);
        b.integrate(0.0, false, &mut l, &mut r);
        assert!(l[5].abs() < 0.01);
        assert!((l[40] - 1.0).abs() < 0.01, "{}", l[40]);
    }
}
