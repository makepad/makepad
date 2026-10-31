//! How alike two renders sound, frame by frame: the pitch classes (chroma)
//! and the spectral envelope (log bands), each as the mean cosine between
//! the two over the frames where either sounds. A cover held against the
//! original it was made from.

/// An in-place radix-2 FFT of `re`/`im` (length a power of two).
fn fft(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
    let mut j = 0;
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
    let mut len = 2;
    while len <= n {
        let ang = -std::f32::consts::TAU / len as f32;
        for start in (0..n).step_by(len) {
            for k in 0..len / 2 {
                let (s, c) = (ang * k as f32).sin_cos();
                let (a, b) = (start + k, start + k + len / 2);
                let (tr, ti) = (re[b] * c - im[b] * s, re[b] * s + im[b] * c);
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
            }
        }
        len <<= 1;
    }
}

const N: usize = 4096;
const BANDS: usize = 32;

/// Per frame: (chroma, log bands, energy).
fn features(left: &[f32], right: &[f32], rate: f32, hop: usize) -> Vec<([f32; 12], [f32; BANDS], f32)> {
    let window: Vec<f32> = (0..N).map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / N as f32).cos()).collect();
    let mut out = Vec::new();
    let mut at = 0;
    let len = left.len().min(right.len());
    let (lo, hi) = (40.0f32, 14000.0f32);
    while at + N <= len {
        let mut re: Vec<f32> = (0..N).map(|i| (left[at + i] + right[at + i]) * 0.5 * window[i]).collect();
        let mut im = vec![0.0; N];
        fft(&mut re, &mut im);
        let mut chroma = [0f32; 12];
        let mut bands = [0f32; BANDS];
        let mut energy = 0.0;
        for k in 1..N / 2 {
            let hz = k as f32 * rate / N as f32;
            let p = re[k] * re[k] + im[k] * im[k];
            energy += p;
            if (lo..hi).contains(&hz) {
                let b = ((hz / lo).ln() / (hi / lo).ln() * BANDS as f32) as usize;
                bands[b.min(BANDS - 1)] += p;
                if hz < 5000.0 {
                    let midi = 69.0 + 12.0 * (hz / 440.0).log2();
                    chroma[(midi.round() as i32).rem_euclid(12) as usize] += p;
                }
            }
        }
        for b in bands.iter_mut() {
            *b = (*b + 1e-9).ln();
        }
        out.push((chroma, bands, energy));
        at += hop;
    }
    out
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let (mut ab, mut aa, mut bb) = (0.0, 0.0, 0.0);
    for (x, y) in a.iter().zip(b) {
        ab += x * y;
        aa += x * x;
        bb += y * y;
    }
    if aa <= 0.0 || bb <= 0.0 { 0.0 } else { ab / (aa.sqrt() * bb.sqrt()) }
}

#[derive(Clone, Copy, Debug)]
pub struct Likeness {
    /// 0..1: the same notes at the same times.
    pub chroma: f32,
    /// 0..1 (centred log bands): the same timbre and balance.
    pub spectrum: f32,
    pub frames: usize,
}

/// How alike `a` and `b` are (both at `rate`, aligned at their starts).
pub fn likeness(a: (&[f32], &[f32]), b: (&[f32], &[f32]), rate: f32) -> Likeness {
    let hop = N;
    let fa = features(a.0, a.1, rate, hop);
    let fb = features(b.0, b.1, rate, hop);
    let n = fa.len().min(fb.len());
    let loud = fa.iter().chain(fb.iter()).map(|f| f.2).fold(0.0f32, f32::max);
    let (mut chroma, mut spectrum, mut count) = (0.0, 0.0, 0);
    for i in 0..n {
        if fa[i].2.max(fb[i].2) < loud * 1e-4 {
            continue;
        }
        let c = cosine(&fa[i].0, &fb[i].0);
        chroma += c;
        if std::env::var("CHIP_TIMELINE").is_ok() && i % 24 == 0 {
            eprintln!("{:6.1} s chroma {c:.2}", (i * hop) as f32 / rate);
        }
        let ma = fa[i].1.iter().sum::<f32>() / BANDS as f32;
        let mb = fb[i].1.iter().sum::<f32>() / BANDS as f32;
        let ca: Vec<f32> = fa[i].1.iter().map(|v| v - ma).collect();
        let cb: Vec<f32> = fb[i].1.iter().map(|v| v - mb).collect();
        spectrum += cosine(&ca, &cb);
        count += 1;
    }
    let c = count.max(1) as f32;
    Likeness { chroma: chroma / c, spectrum: spectrum / c, frames: count }
}
