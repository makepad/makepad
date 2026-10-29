//! Wavetables as the Wavetable and Window oscillators read them: float frames
//! and an int16 copy (with 4 samples of wrap padding each side) per mip
//! level, built from a patch's embedded block or generated (the default
//! "sine to saw" table, the Window type's grain windows).

use std::f64::consts::PI;
use std::sync::{Arc, OnceLock};

pub const FLAG_SAMPLE: u16 = 1;
pub const FLAG_LOOP: u16 = 2;
pub const FLAG_INT16: u16 = 4;
pub const FLAG_FULL_RANGE: u16 = 8;
pub const FLAG_METADATA: u16 = 16;
pub const FLAG_RESLICED: u16 = 32;

const MAX_MIPS: usize = 16;
/// int16 padding before and after each frame.
pub const PAD: usize = 4;

/// One mip level: `len` samples per frame, frames back to back.
#[derive(Clone, Debug, Default)]
pub struct Mip {
    pub len: usize,
    pub f: Vec<f32>,
    /// `frames x (len + 2*PAD)`, padded copy.
    pub i: Vec<i16>,
}

impl Mip {
    #[inline]
    pub fn frame(&self, k: usize) -> &[f32] {
        &self.f[k * self.len..(k + 1) * self.len]
    }
    #[inline]
    pub fn frame_i16(&self, k: usize) -> &[i16] {
        let w = self.len + 2 * PAD;
        &self.i[k * w..(k + 1) * w]
    }
}

#[derive(Clone, Debug)]
pub struct RefTable {
    /// Samples per frame (level 0).
    pub size: usize,
    /// Frame count (incl. appended silent frames for samples).
    pub n: usize,
    pub flags: u16,
    /// Frames of real content (trailing all-zero frames excluded).
    pub content_frames: usize,
    pub mips: Vec<Mip>,
}

/// The 63-tap half-band low-pass of the mip chain (our own windowed-sinc
/// design, unity DC gain, -6 dB at half the previous level's Nyquist).
fn halfband63() -> &'static [f64; 63] {
    static H: OnceLock<[f64; 63]> = OnceLock::new();
    H.get_or_init(|| {
        let mut h = [0.0; 63];
        let fc = 0.225; // cycles per sample of the previous level
        for (a, v) in h.iter_mut().enumerate() {
            let t = a as f64 - 31.0;
            let s = if t == 0.0 { 2.0 * fc } else { (2.0 * PI * fc * t).sin() / (PI * t) };
            let x = a as f64 / 62.0;
            let w = 0.42 - 0.5 * (2.0 * PI * x).cos() + 0.08 * (4.0 * PI * x).cos();
            *v = s * w;
        }
        let sum: f64 = h.iter().sum();
        for v in h.iter_mut() {
            *v /= sum;
        }
        h
    })
}

impl RefTable {
    /// From a patch's embedded block, decoded to floats (int16 data already
    /// divided by 16384, or 32768 with the full-range flag).
    pub fn from_frames(samples_per_frame: usize, frames: usize, flags: u16, data: &[f32]) -> RefTable {
        let size = samples_per_frame.clamp(1, 4096);
        let mut n = frames.min(512);
        let mut f: Vec<f32> = vec![0.0; size * n];
        let m = data.len().min(f.len());
        f[..m].copy_from_slice(&data[..m]);
        let zero_frame = |f: &[f32], k: usize| f[k * size..(k + 1) * size].iter().all(|x| *x == 0.0);
        let mut trailing = 0;
        while trailing < n && zero_frame(&f, n - 1 - trailing) {
            trailing += 1;
        }
        let content = n - trailing;
        if flags & FLAG_SAMPLE != 0 && trailing < 3 {
            let add = 3 - trailing;
            f.resize(size * (n + add), 0.0);
            n += add;
        }
        Self::build(size, n, flags, f, content)
    }

    /// From int16 samples (the .wt / window path).
    pub fn from_i16(size: usize, frames: usize, flags: u16, data: &[i16]) -> RefTable {
        let scale = if flags & FLAG_FULL_RANGE != 0 { 1.0 / 32768.0 } else { 1.0 / 16384.0 };
        let f: Vec<f32> = data.iter().take(size * frames).map(|s| *s as f32 * scale).collect();
        Self::from_frames(size, frames, flags | FLAG_INT16, &f)
    }

    fn build(size: usize, n: usize, flags: u16, mut f0: Vec<f32>, content: usize) -> RefTable {
        // Frames 0..2 always exist.
        let stored = n.max(3);
        f0.resize(size * stored, 0.0);
        let sample = flags & FLAG_SAMPLE != 0;
        let mut mips = vec![Mip { len: size, f: f0, i: Vec::new() }];
        let hb = halfband63();
        let mut m = 1;
        while m < MAX_MIPS && (1usize << m) < size {
            let prev = &mips[m - 1];
            let pl = prev.len;
            let len = pl / 2;
            let mut f = vec![0.0f32; len * stored];
            for k in 0..stored {
                for i in 0..len {
                    let mut acc = 0.0f64;
                    for (a, h) in hb.iter().enumerate() {
                        let j = 2 * i as isize + a as isize - 31;
                        let x = if sample {
                            // Read across into the neighbouring frames.
                            let g = k as isize * pl as isize + j;
                            if g < 0 || g >= (stored * pl) as isize { 0.0 } else { prev.f[g as usize] }
                        } else {
                            prev.f[k * pl + j.rem_euclid(pl as isize) as usize]
                        };
                        acc += *h * x as f64;
                    }
                    f[k * len + i] = acc as f32;
                }
            }
            mips.push(Mip { len, f, i: Vec::new() });
            m += 1;
        }
        let iscale = if flags & FLAG_FULL_RANGE != 0 { 32768.0 } else { 16384.0 };
        for mip in mips.iter_mut() {
            let len = mip.len;
            let w = len + 2 * PAD;
            let mut iv = vec![0i16; w * stored];
            for k in 0..stored {
                let fr = &mip.f[k * len..(k + 1) * len];
                for p in 0..w {
                    let src = (p as isize - PAD as isize).rem_euclid(len as isize) as usize;
                    let x = (fr[src] * iscale).trunc();
                    iv[k * w + p] = x.clamp(-iscale, iscale - 1.0) as i16;
                }
            }
            mip.i = iv;
        }
        RefTable { size, n, flags, content_frames: content.max(1), mips }
    }

    /// "Sine to saw": 63 frames x 128, frame k = the first k+1 saw partials,
    /// every frame at the same RMS, peak 1.0; stored as int16.
    pub fn default_table() -> RefTable {
        let (size, n) = (128usize, 63usize);
        let mut f = vec![0.0f64; size * n];
        for k in 0..n {
            let fr = &mut f[k * size..(k + 1) * size];
            for (i, v) in fr.iter_mut().enumerate() {
                *v = (1..=k + 1).map(|h| (2.0 * PI * h as f64 * i as f64 / size as f64).sin() / h as f64).sum();
            }
            let rms = (fr.iter().map(|x| x * x).sum::<f64>() / size as f64).sqrt();
            for v in fr.iter_mut() {
                *v /= rms;
            }
        }
        let peak = f.iter().fold(0.0f64, |p, x| p.max(x.abs()));
        let data: Vec<i16> = f.iter().map(|x| ((x / peak) * 16384.0).trunc().clamp(-16384.0, 16383.0) as i16).collect();
        Self::from_i16(size, n, FLAG_INT16, &data)
    }

    /// The shared default table.
    pub fn shared_default() -> &'static Arc<RefTable> {
        static T: OnceLock<Arc<RefTable>> = OnceLock::new();
        T.get_or_init(|| Arc::new(RefTable::default_table()))
    }

    pub fn is_sample(&self) -> bool {
        self.flags & FLAG_SAMPLE != 0
    }
    pub fn is_loop(&self) -> bool {
        self.flags & FLAG_LOOP != 0
    }
    pub fn full_range(&self) -> bool {
        self.flags & FLAG_FULL_RANGE != 0
    }
    pub fn mip_count(&self) -> usize {
        self.mips.len()
    }
}

/// The Window oscillator's 9 grain windows (1024 samples, RMS 0.5, i15).
pub fn window_table() -> &'static RefTable {
    static T: OnceLock<RefTable> = OnceLock::new();
    T.get_or_init(|| {
        let n = 1024usize;
        let mut data = Vec::with_capacity(9 * n);
        for w in 0..9 {
            let mut v = vec![0.0f64; n];
            for (i, s) in v.iter_mut().enumerate() {
                let x = (i as f64 + 1.0) / n as f64;
                *s = match w {
                    0 => 1.0 - (2.0 * ((i as f64 + 0.5) / n as f64) - 1.0).abs(),
                    1 => (PI * x).sin(),
                    2 => (PI * x.powf(0.5)).sin(),
                    3 => (PI * x.powf(0.3)).sin(),
                    4 => (PI * x.powf(0.2)).sin(),
                    5 => 1.0 - x,
                    6 => (2.0 * PI * x).sin(),
                    7 => if i < 512 { 1.0 } else { -1.0 },
                    _ => 1.0,
                };
            }
            let rms = (v.iter().map(|x| x * x).sum::<f64>() / n as f64).sqrt();
            for s in v {
                data.push((s / rms * 8192.0).round().clamp(-32768.0, 32767.0) as i16);
            }
        }
        RefTable::from_i16(n, 9, FLAG_INT16, &data)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_table_shape() {
        let t = RefTable::default_table();
        assert_eq!((t.size, t.n), (128, 63));
        assert_eq!(t.mip_count(), 7);
        let peak = t.mips[0].f.iter().fold(0.0f32, |p, x| p.max(x.abs()));
        assert!((peak - 1.0).abs() < 1e-3, "{peak}");
        // Frame 0 is a sine: its level-1 mip keeps the amplitude.
        let p1 = t.mips[1].frame(0).iter().fold(0.0f32, |p, x| p.max(x.abs()));
        let p0 = t.mips[0].frame(0).iter().fold(0.0f32, |p, x| p.max(x.abs()));
        assert!((p1 / p0 - 1.0).abs() < 0.02, "{p0} {p1}");
    }

    #[test]
    fn sample_gets_silent_tail() {
        let t = RefTable::from_frames(8, 4, FLAG_SAMPLE, &[0.5; 32]);
        assert_eq!(t.n, 7);
        assert_eq!(t.content_frames, 4);
        let w = window_table();
        // Rectangle window: constant 8192.
        assert_eq!(w.mips[0].frame_i16(8)[PAD + 100], 8192);
    }
}
