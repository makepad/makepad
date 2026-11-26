//! Synthetic sources for tests and the call probe: pictures and a voice that never touch a device.

/// Which test picture a synthetic camera shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Synth {
    /// Moving diagonal bands, a square crossing once a second, the picture's number along the top.
    Bands,
    /// A picture no encoder codes cheaply: scrolling noise, fresh noise across a sixth of the
    /// height, three fast squares, the frame number. Its cost has a floor above most asks (at 720p
    /// the NVIDIA encoders sent about 17 Mbit/s of it whatever they were asked), so it shows what
    /// the call does with an encoder that can't hold its rate.
    Noise,
}

/// A synthetic camera: fills an NV12 picture of any size for frame `n`.
pub struct SynthCamera {
    kind: Synth,
    field: Vec<u8>,
    field_size: (u32, u32),
    seed: u64,
}

impl SynthCamera {
    pub fn new(kind: Synth) -> Self {
        SynthCamera { kind, field: Vec::new(), field_size: (0, 0), seed: 0x2545_f491_4f6c_dd1d }
    }

    pub fn picture(&mut self, w: u32, h: u32, n: u64, nv12: &mut Vec<u8>) {
        match self.kind {
            Synth::Bands => bands_picture(w, h, n, nv12),
            Synth::Noise => {
                if self.field_size != (w, h) {
                    self.field.clear();
                    let seed = &mut self.seed;
                    self.field.resize_with(w as usize * h as usize, || xorshift(seed));
                    self.field_size = (w, h);
                }
                noise_picture(w, h, n, &self.field, nv12);
            }
        }
    }
}

fn bands_picture(w: u32, h: u32, n: u64, nv12: &mut Vec<u8>) {
    nv12.resize((w * h * 3 / 2) as usize, 0);
    let (sq, sx, sy) = (h / 4, ((n % 30) as u32 * (w - h / 4)) / 29, h / 2 - h / 8);
    for y in 0..h {
        for x in 0..w {
            let band = ((x + y + n as u32 * 4) / 16) % 2;
            let mut v = 60 + band * 40;
            if x >= sx && x < sx + sq && y >= sy && y < sy + sq {
                v = 230;
            }
            if y < 24 {
                let bit = x / (w / 32).max(1);
                v = if bit < 32 && (n >> (31 - bit)) & 1 == 1 { 235 } else { 16 };
            }
            nv12[(y * w + x) as usize] = v as u8;
        }
    }
    let c = (w * h) as usize;
    for y in 0..h / 2 {
        for x in 0..w / 2 {
            nv12[c + (y * w + 2 * x) as usize] = (96 + (x * 64 / (w / 2))) as u8;
            nv12[c + (y * w + 2 * x + 1) as usize] = (160 - (y * 64 / (h / 2))) as u8;
        }
    }
}

fn xorshift(s: &mut u64) -> u8 {
    *s ^= *s << 13;
    *s ^= *s >> 7;
    *s ^= *s << 17;
    (*s >> 56) as u8
}

fn noise_picture(w: u32, h: u32, n: u64, field: &[u8], nv12: &mut Vec<u8>) {
    let (w, h) = (w as usize, h as usize);
    nv12.resize(w * h * 3 / 2, 0);
    let (dx, dy) = ((n as usize * 14) % w, (n as usize * 6) % h);
    let mut fresh = 0x9e37_79b9_7f4a_7c15u64 ^ n.wrapping_mul(0x2545_f491_4f6c_dd1d);
    for y in 0..h {
        let row = &mut nv12[y * w..(y + 1) * w];
        if y >= h / 3 && y < h / 3 + h / 6 {
            for p in row.iter_mut() {
                *p = 16 + (xorshift(&mut fresh) as u32 * 219 / 255) as u8;
            }
        } else {
            let src = &field[((y + dy) % h) * w..((y + dy) % h + 1) * w];
            for (x, p) in row.iter_mut().enumerate() {
                *p = 16 + (src[(x + dx) % w] as u32 * 219 / 255) as u8;
            }
        }
    }
    let sq = h / 6;
    for k in 0..3usize {
        let sx = (n as usize * (37 + 11 * k)) % (w - sq);
        let sy = (k * h / 3 + n as usize * 7) % (h - sq);
        for y in sy..sy + sq {
            nv12[y * w + sx..y * w + sx + sq].fill(235 - 60 * k as u8);
        }
    }
    for y in 0..24.min(h) {
        for x in 0..w {
            let bit = x / (w / 32).max(1);
            nv12[y * w + x] = if bit < 32 && (n >> (31 - bit)) & 1 == 1 { 235 } else { 16 };
        }
    }
    let c = w * h;
    for y in 0..h / 2 {
        for x in 0..w / 2 {
            let v = field[((y * 2 + dy) % h) * w + (x * 2 + dx) % w];
            nv12[c + y * w + 2 * x] = 64 + v / 2;
            nv12[c + y * w + 2 * x + 1] = 192 - v / 2;
        }
    }
}

/// A synthetic microphone: a tone in talk spurts (1.5 s on, 0.5 s off), in device blocks of
/// 10 ms at 48 kHz.
pub struct SynthVoice {
    tone_hz: f32,
    phase: f32,
    block: u64,
}

pub const SYNTH_RATE: f64 = 48_000.0;
pub const SYNTH_BLOCK: usize = 480;

impl SynthVoice {
    pub fn new(tone_hz: f32) -> Self {
        SynthVoice { tone_hz, phase: 0.0, block: 0 }
    }

    /// The next 10 ms block.
    pub fn next_block(&mut self, out: &mut [f32; SYNTH_BLOCK]) {
        let talking = (self.block % 200) < 150;
        for s in out.iter_mut() {
            *s = if talking { 0.3 * (self.phase * std::f32::consts::TAU).sin() } else { 0.0 };
            self.phase = (self.phase + self.tone_hz / SYNTH_RATE as f32).fract();
        }
        self.block += 1;
    }
}
