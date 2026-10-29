//! Alias (type 11): a deliberately aliasing 8-bit oscillator. A 32-bit
//! phase's top byte is wrapped, masked and thresholded, optionally used to
//! index a 256-entry 8-bit table, then bit-crushed; unison and the
//! character filter.

use super::sine::shape as sine_shape;
use super::{detune_factor, freq, spread, unison_lr, CharFilter, DriftLfo, OscBlock};
use crate::dsp::Rng;
use crate::kit::core::{Lag, SlotDef, SlotUnit};
use std::f64::consts::PI;
use std::sync::OnceLock;

/// The reference converts `sample * (2^32 - 1)` to unsigned 32-bit in a
/// platform-defined way; its reference renders (arm64) saturate negative
/// samples to 0 (x86-64 would wrap). Mirrored: saturate.
const QUIRK_AUDIO_IN_UNSIGNED_SATURATES: bool = true;

/// One Audio In table byte: the top byte of the sample scaled to 32 bits.
fn audio_byte(x: f32) -> u8 {
    let v = x as f64 * 4_294_967_295.0;
    let u = if QUIRK_AUDIO_IN_UNSIGNED_SATURATES { v as u32 } else { v as i64 as u32 };
    (u >> 24) as u8
}

pub const SHAPE_NAMES: [&str; 18] = [
    "Sine",
    "Ramp",
    "Pulse",
    "Noise",
    "Alias Mem",
    "Osc Mem",
    "Scene Mem",
    "DAW Chunk Mem",
    "Step Seq Mem",
    "Audio In",
    "TX 2",
    "TX 3",
    "TX 4",
    "TX 5",
    "TX 6",
    "TX 7",
    "TX 8",
    "Additive",
];

pub const SINE: usize = 0;
pub const RAMP: usize = 1;
pub const PULSE: usize = 2;
pub const NOISE: usize = 3;
pub const MEM_FIRST: usize = 4;
pub const MEM_LAST: usize = 8;
pub const AUDIO_IN: usize = 9;
pub const TX_FIRST: usize = 10;
pub const ADDITIVE: usize = 17;

/// The memory shapes read the reference's own in-memory structures; we
/// substitute fixed pseudo-random tables (noise-like either way).
const MEM_SEEDS: [u32; 5] = [0x51A5_0001, 0x51A5_0002, 0x51A5_0003, 0x51A5_0004, 0x51A5_0005];

pub fn slot_defs() -> [SlotDef; 7] {
    [
        SlotDef::choice("Shape", &SHAPE_NAMES, 0.0),
        SlotDef { about: "wrap = 1 + 15 v", ..SlotDef::float("Wrap", 0.0, 1.0, 0.0, SlotUnit::Percent) },
        SlotDef { deform: &["Masked Ramp", "Unmasked Ramp After Threshold"], ..SlotDef::float("Mask", 0.0, 1.0, 0.0, SlotUnit::Percent) },
        SlotDef::float("Threshold", 0.0, 1.0, 0.5, SlotUnit::Percent),
        SlotDef { about: "bits of resolution", ..SlotDef::float("Bitcrush", 1.0, 8.0, 8.0, SlotUnit::None) },
        super::classic::unison_detune(0.2),
        super::classic::unison_voices(),
    ]
}

struct Tables {
    sine: [u8; 256],
    tx: [[u8; 256]; 7],
    mem: [[u8; 256]; 5],
}

fn tables() -> &'static Tables {
    static T: OnceLock<Tables> = OnceLock::new();
    T.get_or_init(|| {
        let mut t = Tables { sine: [0; 256], tx: [[0; 256]; 7], mem: [[0; 256]; 5] };
        for k in 0..256 {
            t.sine[k] = (127.5 + 127.5 * (2.0 * PI * k as f64 / 256.0).sin()).floor() as u8;
        }
        const MODES: [u8; 7] = [28, 2, 29, 4, 30, 6, 31];
        for (i, m) in MODES.iter().enumerate() {
            for k in 0..256 {
                let x = k as f64 * 2.0 * PI / 255.0;
                let v = sine_shape(*m, x.sin() as f32, x.cos() as f32);
                t.tx[i][k] = (255.0 * (v + 1.0) * 0.5).clamp(0.0, 255.0) as u8;
            }
        }
        for (i, seed) in MEM_SEEDS.iter().enumerate() {
            let mut r = Rng::new(*seed);
            for k in 0..256 {
                t.mem[i][k] = (r.next_u32() >> 24) as u8;
            }
        }
        t
    })
}

/// The 8-bit xorshift noise of the Noise shape.
#[derive(Clone, Copy, Debug)]
struct Noise8 {
    x: u8,
    y: u8,
    z: u8,
    a: u8,
    count: u8,
    value: u8,
}

impl Default for Noise8 {
    fn default() -> Self {
        Noise8 { x: 21, y: 229, z: 181, a: 51, count: 0, value: 127 }
    }
}

impl Noise8 {
    #[inline]
    fn step(&mut self) -> u8 {
        let t = self.x ^ (self.x << 3);
        self.x = self.y;
        self.y = self.z;
        self.z = self.a;
        self.a = self.a ^ (self.a >> 5) ^ (t ^ (t >> 2));
        self.a
    }
    fn advance_to(&mut self, upper: u8, every: u8) -> u8 {
        let every = every.max(1);
        while self.count != upper {
            self.count = self.count.wrapping_add(1);
            if self.count % every == 0 {
                self.value = self.step();
            }
        }
        self.value
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Voice {
    drift: DriftLfo,
    phase: u32,
    noise: Noise8,
    gl: f32,
    gr: f32,
}

pub struct Alias {
    rng: Rng,
    v: [Voice; 16],
    n: usize,
    fmd: Lag,
    additive: [u8; 256],
    additive_blocks: u32,
    audio: [u8; 256],
    chr: CharFilter,
    first: bool,
}

impl Alias {
    pub fn new(seed: u32) -> Alias {
        Alias {
            rng: Rng::new(seed),
            v: [Voice::default(); 16],
            n: 1,
            fmd: Lag::default(),
            additive: [127; 256],
            additive_blocks: 0,
            audio: [127; 256],
            chr: CharFilter::default(),
            first: true,
        }
    }

    pub fn init(&mut self, b: &OscBlock, nonzero_drift: bool) {
        self.n = b.unison();
        for u in 0..self.n {
            let mut v = Voice::default();
            v.drift.init(&mut self.rng, nonzero_drift);
            let (gl, gr) = unison_lr(self.n, u);
            v.gl = gl;
            v.gr = gr;
            v.phase = if b.retrigger { 0 } else { self.rng.next_u32() };
            self.v[u] = v;
        }
        self.fmd = Lag::default();
        self.additive_blocks = 0;
        self.build_additive(b.extra);
        self.chr.reset();
        self.first = false;
    }

    fn build_additive(&mut self, extra: &[f32]) {
        let t = tables();
        let mut a = [0.0f32; 16];
        for (h, v) in a.iter_mut().enumerate() {
            *v = extra.get(h).copied().unwrap_or(1.0 / (h as f32 + 1.0));
        }
        let ss: f32 = a.iter().map(|x| x * x).sum();
        let norm = if ss > 0.0 { 127.0 / ss.sqrt() } else { 0.0 };
        let mut ai = [0i32; 16];
        for h in 0..16 {
            ai[h] = ((a[h] * norm) as i32).clamp(-128, 127);
        }
        for s in 0..256usize {
            let mut sum: i16 = 0;
            for h in 0..16 {
                let tv = t.sine[(s * (h + 1)) & 255] as i32 - 127;
                sum = sum.wrapping_add((tv * ai[h]) as i16);
            }
            let x = ((sum >> 8) as i32).clamp(-127, 127);
            self.additive[s] = (x + 127) as u8;
        }
    }

    fn build_audio(&mut self, b: &OscBlock) {
        if let Some((il, ir)) = b.audio_in {
            for q in 0..64 {
                let (lb, rb) = (audio_byte(il[q]), audio_byte(ir[q]));
                // Each sample's L, R pair is written twice in a row.
                self.audio[4 * q..4 * q + 4].copy_from_slice(&[lb, rb, lb, rb]);
            }
        } else {
            self.audio = [127; 256];
        }
    }

    pub fn process(&mut self, b: &OscBlock, l: &mut [f32; 64], r: &mut [f32; 64]) {
        if self.first {
            self.init(b, false);
        }
        let shape = (b.slots[0].i().clamp(0, 17)) as usize;
        if shape == ADDITIVE {
            self.additive_blocks += 1;
            if self.additive_blocks >= 21 {
                self.additive_blocks = 0;
                self.build_additive(b.extra);
            }
        }
        if shape == AUDIO_IN {
            self.build_audio(b);
        }
        let t = tables();
        let table: Option<&[u8; 256]> = match shape {
            SINE => Some(&t.sine),
            MEM_FIRST..=MEM_LAST => Some(&t.mem[shape - MEM_FIRST]),
            AUDIO_IN => Some(&self.audio),
            TX_FIRST..=16 => Some(&t.tx[shape - TX_FIRST]),
            ADDITIVE => Some(&self.additive),
            _ => None,
        };
        let wrap = 1.0 + 15.0 * b.slots[1].v.clamp(0.0, 1.0);
        let mask = (255.0 * b.slots[2].v.clamp(0.0, 1.0)) as u8;
        let unmasked_ramp = b.slots[2].deform == 1;
        let thr = (255.0 * b.slots[3].v.clamp(0.0, 1.0)) as u8;
        let bits = b.slots[4].v.clamp(1.0, 8.0);
        let crush = bits < 8.0;
        let q = bits.exp2();
        self.fmd.set(if b.fm { 16.0 * b.fmdepth * b.fmdepth * b.fmdepth } else { 0.0 });
        let mut fmv = [0i64; 64];
        for k in 0..64 {
            fmv[k] = (self.fmd.process() as f64 * b.fm_buf[k] as f64 * 4_294_967_296.0) as i64;
        }
        let att = 1.0 / (self.n as f32).sqrt();
        let sp = spread(b, 5);
        let mut ol = [0.0f32; 64];
        let mut or = [0.0f32; 64];
        for u in 0..self.n {
            let d = self.v[u].drift.next(&mut self.rng);
            let df = detune_factor(self.n, u);
            let (note, hz) = if b.slots[5].absolute { (b.pitch + b.drift * d, 16.0 * sp * df) } else { (b.pitch + b.drift * d + sp * df, 0.0) };
            let f = (freq(note) + hz).max(0.0) as f64;
            let inc = ((f / b.fs_os as f64) * 4_294_967_296.0).round().clamp(0.0, u32::MAX as f64) as u32;
            let v = &mut self.v[u];
            for k in 0..64 {
                let upper = if shape == PULSE { (((v.phase as f64) * wrap as f64) as u64 as u32 >> 24) as u8 } else { (v.phase >> 24) as u8 };
                let masked = upper ^ mask;
                let mut res = masked as i32;
                match shape {
                    RAMP => {
                        if upper > thr {
                            res = 255 - if unmasked_ramp { upper as i32 } else { masked as i32 };
                        }
                    }
                    PULSE => res = if masked > thr { 255 } else { 0 },
                    NOISE => {
                        let val = v.noise.advance_to(upper, thr | 8) as i32;
                        res = ((val - 127) as f32 * wrap).clamp(-127.0, 126.0) as i32 + 127;
                    }
                    _ => {}
                }
                if shape != NOISE && shape != PULSE {
                    res = ((res as f32 * wrap) as i32).rem_euclid(256);
                }
                let mut out = match table {
                    Some(tb) => {
                        if res > thr as i32 {
                            res = (res + 127 - thr as i32).rem_euclid(256);
                        }
                        (tb[(255 - res) as usize] as f32 - 127.0) / 255.0
                    }
                    None => (res as f32 - 127.0) / 255.0,
                };
                if crush {
                    out = (out * q).trunc() / q;
                }
                ol[k] += out * v.gl * att;
                or[k] += out * v.gr * att;
                v.phase = v.phase.wrapping_add(inc).wrapping_add(fmv[k] as u32);
            }
        }
        if b.stereo {
            *l = ol;
            *r = or;
        } else {
            for k in 0..64 {
                l[k] = 0.5 * (ol[k] + or[k]);
            }
        }
        self.chr.process(b.character, b.fs(), b.stereo, l, r);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::core::SlotValue;

    #[test]
    fn audio_in_table_layout_and_saturation() {
        assert_eq!(audio_byte(0.5), 127);
        assert_eq!(audio_byte(0.999), 255);
        assert_eq!(audio_byte(1.5), 255);
        assert_eq!(audio_byte(-0.25), 0, "negative samples saturate to 0");
        assert_eq!(audio_byte(-1.0), 0);
        let mut il = [0f32; 64];
        let mut ir = [0f32; 64];
        for q in 0..64 {
            il[q] = q as f32 / 64.0;
            ir[q] = if q % 2 == 0 { -0.5 } else { 0.75 };
        }
        let slots = [SlotValue::default(); 7];
        let fm = [0f32; 64];
        let b = OscBlock {
            pitch: 60.0,
            drift: 0.0,
            stereo: false,
            fm: false,
            fmdepth: 0.0,
            fm_buf: &fm,
            slots: &slots,
            retrigger: true,
            character: 1,
            gate: true,
            table: None,
            extra: &[],
            audio_in: Some((&il, &ir)),
            fs_os: 96000.0,
            tsr: 1.0,
        };
        let mut a = Alias::new(1);
        a.build_audio(&b);
        for q in 0..64 {
            let (lb, rb) = (audio_byte(il[q]), audio_byte(ir[q]));
            assert_eq!(a.audio[4 * q..4 * q + 4], [lb, rb, lb, rb], "q {q}");
        }
        assert_eq!(a.audio[4], 3, "L[1] = 1/64 -> just under 2^26 -> top byte 3");
        assert_eq!(a.audio[1], 0);
        assert_eq!(a.audio[7], 191);
    }
}
