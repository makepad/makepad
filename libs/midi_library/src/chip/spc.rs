//! SNES music (SPC files): a snapshot of the sound CPU's 64 KB, its
//! registers and the DSP's. The driver runs on our SPC700 with the timers
//! it paces itself by; its DSP writes are followed by a model of the eight
//! voices (key on and off, pitch, the sample each plays, the ADSR or GAIN
//! envelope, volume). The samples are the cartridge's own: BRR-decoded
//! from the sample directory with their loops, each one's pitch found so
//! that notes are real notes, played on samplers.

use super::spc700::{Bus, Spc700};
use super::tracker::arrange::SampleAsset;
use super::voices::{ChipVoice, Frame, Kind};
use std::collections::HashMap;
use std::sync::Arc;

pub fn is_spc(bytes: &[u8]) -> bool {
    bytes.len() >= 0x10180 && bytes.starts_with(b"SNES-SPC700 Sound File Data")
}

#[derive(Clone, Debug, Default)]
pub struct Tag {
    pub song: String,
    pub game: String,
    pub dumper: String,
    pub comment: String,
    pub artist: String,
    pub seconds: f64,
    pub year: Option<u16>,
}

fn text(b: &[u8]) -> String {
    b.iter().take_while(|&&c| c != 0).map(|&c| if c < 32 { ' ' } else { c as char }).collect::<String>().trim().to_string()
}

pub fn tag(b: &[u8]) -> Tag {
    if b[0x23] != 26 {
        return Tag::default();
    }
    // The text form has digits (or nothing) where the binary form has its
    // length bytes.
    let len_field = &b[0xA9..0xAC];
    let textual = len_field.iter().all(|c| c.is_ascii_digit() || *c == 0) || b[0x9E..0xA9].iter().any(|c| *c == b'/');
    let (seconds, artist, year) = if textual {
        let secs = text(len_field).parse::<f64>().unwrap_or(0.0);
        let date = text(&b[0x9E..0xA9]);
        (secs, text(&b[0xB1..0xD1]), date.rsplit('/').next().and_then(|y| y.parse().ok()).filter(|y: &u16| *y > 1980))
    } else {
        let secs = u32::from_le_bytes([b[0xA9], b[0xAA], b[0xAB], 0]) as f64;
        (secs, text(&b[0xB0..0xD0]), None)
    };
    Tag { song: text(&b[0x2E..0x4E]), game: text(&b[0x4E..0x6E]), dumper: text(&b[0x6E..0x7E]), comment: text(&b[0x7E..0x9E]), artist, seconds, year }
}

#[derive(Clone, Copy, Default)]
struct Timer {
    stage: u32,
    target: u8,
    out: u8,
    on: bool,
}

struct Apu {
    ram: Vec<u8>,
    dsp: [u8; 128],
    dsp_addr: u8,
    timers: [Timer; 3],
    ports: [u8; 4],
    now: u64,
    writes: Vec<(f64, u8, u8)>,
    last_timer: u64,
}

const CLOCK: f64 = 1_024_000.0;

impl Apu {
    /// Bring the timers up to now (T0/T1 every 128 cycles, T2 every 16).
    fn tick(&mut self) {
        while self.last_timer + 16 <= self.now {
            self.last_timer += 16;
            for k in 0..3 {
                if k < 2 && self.last_timer % 128 != 0 {
                    continue;
                }
                let t = &mut self.timers[k];
                if !t.on {
                    continue;
                }
                t.stage += 1;
                let target = if t.target == 0 { 256 } else { t.target as u32 };
                if t.stage >= target {
                    t.stage = 0;
                    t.out = (t.out + 1) & 15;
                }
            }
        }
    }
}

impl Bus for Apu {
    fn read(&mut self, addr: u16) -> u8 {
        match addr {
            0xF3 => self.dsp[(self.dsp_addr & 0x7F) as usize],
            0xF4..=0xF7 => self.ports[(addr - 0xF4) as usize],
            0xFD..=0xFF => {
                self.tick();
                let t = &mut self.timers[(addr - 0xFD) as usize];
                let v = t.out;
                t.out = 0;
                v
            }
            _ => self.ram[addr as usize],
        }
    }

    fn write(&mut self, addr: u16, value: u8) {
        match addr {
            0xF1 => {
                for k in 0..3 {
                    let on = value & (1 << k) != 0;
                    if on && !self.timers[k].on {
                        self.timers[k].stage = 0;
                        self.timers[k].out = 0;
                    }
                    self.timers[k].on = on;
                }
            }
            0xF2 => self.dsp_addr = value,
            0xF3 => {
                if self.dsp_addr < 0x80 {
                    self.dsp[self.dsp_addr as usize] = value;
                    self.writes.push((self.now as f64 / CLOCK, self.dsp_addr, value));
                }
            }
            0xFA..=0xFC => self.timers[(addr - 0xFA) as usize].target = value,
            _ => self.ram[addr as usize] = value,
        }
    }
}

pub struct Run {
    pub ram: Vec<u8>,
    /// The DSP at the start.
    pub dsp0: [u8; 128],
    pub writes: Vec<(f64, u8, u8)>,
    pub seconds: f64,
}

pub fn run(b: &[u8], seconds: f64) -> Result<Run, String> {
    if !is_spc(b) {
        return Err("not an SPC file".into());
    }
    let mut ram = b[0x100..0x10100].to_vec();
    let mut dsp = [0u8; 128];
    dsp.copy_from_slice(&b[0x10100..0x10180]);
    let control = ram[0xF1];
    let mut timers = [Timer::default(); 3];
    for (k, t) in timers.iter_mut().enumerate() {
        t.on = control & (1 << k) != 0;
        t.target = ram[0xFA + k];
        t.out = ram[0xFD + k] & 15;
    }
    let ports = [ram[0xF4], ram[0xF5], ram[0xF6], ram[0xF7]];
    ram[0xF1] &= 0x0F;
    let dsp0 = dsp;
    let mut apu = Apu { ram, dsp, dsp_addr: b[0x100 + 0xF2], timers, ports, now: 0, writes: Vec::new(), last_timer: 0 };
    let mut cpu = Spc700 { pc: u16::from_le_bytes([b[0x25], b[0x26]]), a: b[0x27], x: b[0x28], y: b[0x29], psw: b[0x2A], sp: b[0x2B], ..Default::default() };
    let total = (seconds * CLOCK) as u64;
    while cpu.cycles < total && !cpu.halted {
        apu.now = cpu.cycles;
        cpu.step(&mut apu);
        if cpu.cycles - apu.last_timer >= 128 {
            apu.now = cpu.cycles;
            apu.tick();
        }
    }
    Ok(Run { ram: apu.ram, dsp0, writes: apu.writes, seconds })
}

/// A sample (by its source number): decoded BRR from the directory, the
/// loop, and its pitch at 32 kHz (none for a noise or a drum).
pub struct Brr {
    pub pcm: Vec<f32>,
    pub looped: Option<(u32, u32)>,
    pub root: Option<f32>,
}

pub fn brr(ram: &[u8], dir: u8, srcn: u8) -> Brr {
    let entry = dir as usize * 0x100 + srcn as usize * 4;
    let rd = |at: usize| ram.get(at & 0xFFFF).copied().unwrap_or(0);
    let start = rd(entry) as usize | (rd(entry + 1) as usize) << 8;
    let lp = rd(entry + 2) as usize | (rd(entry + 3) as usize) << 8;
    let mut pcm: Vec<f32> = Vec::new();
    let (mut p1, mut p2) = (0i32, 0i32);
    let mut at = start;
    let mut looped = None;
    for _ in 0..4096 {
        let head = rd(at);
        let shift = head >> 4;
        let filter = (head >> 2) & 3;
        if at == lp && looped.is_none() {
            looped = Some(pcm.len() as u32);
        }
        for k in 0..16 {
            let byte = rd(at + 1 + k / 2);
            let nib = if k % 2 == 0 { byte >> 4 } else { byte & 15 } as i32;
            let nib = if nib >= 8 { nib - 16 } else { nib };
            let mut s = if shift <= 12 { (nib << shift) >> 1 } else { (nib >> 3) << 11 };
            s += match filter {
                1 => p1 + ((-p1) >> 4),
                2 => (p1 << 1) + ((-((p1 << 1) + p1)) >> 5) - p2 + (p2 >> 4),
                3 => (p1 << 1) + ((-(p1 + (p1 << 2) + (p1 << 3))) >> 6) - p2 + (((p2 << 1) + p2) >> 4),
                _ => 0,
            };
            // Clamped to 16 bits, then cut to 15 as the DSP keeps it.
            let s = s.clamp(-32768, 32767);
            let s = (((s << 1) as i16) as i32) >> 1;
            p2 = p1;
            p1 = s;
            pcm.push(s as f32 / 16384.0);
        }
        if head & 1 != 0 {
            break;
        }
        at += 9;
    }
    let loop_region = if pcm.len() > 16 { looped.filter(|_| true).map(|l| (l, pcm.len() as u32)) } else { None };
    let root = pitch_of(&pcm, loop_region);
    Brr { pcm, looped: loop_region, root }
}

/// A sample's fundamental (MIDI, at 32 kHz), from its autocorrelation over
/// a steady stretch (the loop, or after the attack).
fn pitch_of(pcm: &[f32], looped: Option<(u32, u32)>) -> Option<f32> {
    let (a, b) = match looped {
        Some((a, b)) if b > a + 2 => (a as usize, b as usize),
        _ => (pcm.len() / 4, pcm.len()),
    };
    let seg: Vec<f32> = if b - a >= 2048 { pcm[a..a + 2048].to_vec() } else {
        // A short loop: repeat it to a window.
        let l = &pcm[a..b];
        if l.is_empty() {
            return None;
        }
        l.iter().cycle().take(2048).copied().collect()
    };
    let energy: f32 = seg.iter().map(|x| x * x).sum();
    if energy < 1e-4 {
        return None;
    }
    // The normalized autocorrelation at every lag; the period is the
    // first peak nearly as high as the highest (not a multiple of it),
    // placed between samples by the parabola through its neighbours.
    let r_at = |lag: usize| -> f32 {
        let (mut c, mut e1, mut e2) = (0.0f32, 0.0f32, 0.0f32);
        for i in 0..seg.len() - lag {
            c += seg[i] * seg[i + lag];
            e1 += seg[i] * seg[i];
            e2 += seg[i + lag] * seg[i + lag];
        }
        c / (e1 * e2).sqrt().max(1e-9)
    };
    // Lags from 4 samples (an 8 kHz tone: a short single-cycle loop).
    let r: Vec<f32> = (0..1000).map(|lag| if lag < 3 { 1.0 } else { r_at(lag) }).collect();
    let top = r[4..].iter().cloned().fold(f32::MIN, f32::max);
    if top < 0.6 {
        return None;
    }
    // Past the first dip (the waveform's own smoothness keeps the first
    // lags high), the first peak nearly as high as the highest.
    let Some(dip) = (3..999).find(|&l| r[l] < 0.5) else { return None };
    let top = r[dip..].iter().cloned().fold(f32::MIN, f32::max);
    if top < 0.6 {
        return None;
    }
    let Some(lag) = (dip.max(4)..999).find(|&l| r[l] >= top * 0.92 && r[l] >= r[l - 1] && r[l] >= r[l + 1]) else { return None };
    let (y0, y1, y2) = (r[lag - 1], r[lag], r[lag + 1]);
    let shift = if y0 - 2.0 * y1 + y2 != 0.0 { 0.5 * (y0 - y2) / (y0 - 2.0 * y1 + y2) } else { 0.0 };
    let period = lag as f32 + shift.clamp(-0.5, 0.5);
    let hz = 32000.0 / period;
    Some(69.0 + 12.0 * (hz / 440.0).log2())
}

const RATES: [u32; 32] = [0, 2048, 1536, 1280, 1024, 768, 640, 512, 384, 320, 256, 192, 160, 128, 96, 80, 64, 48, 40, 32, 24, 20, 16, 12, 10, 8, 6, 5, 4, 3, 2, 1];

#[derive(Clone, Copy, Default)]
struct DspVoice {
    env: i32,
    /// 0 attack, 1 decay, 2 sustain, 3 release, 4 off.
    stage: u8,
    counter: u32,
}

#[derive(Clone, Copy)]
enum Move {
    Add(i32),
    Exp,
    Bent,
}

impl DspVoice {
    fn step(&mut self, adsr1: u8, adsr2: u8, gain: u8) {
        self.counter += 1;
        let (rate, mv) = if self.stage == 3 {
            (31, Move::Add(-8))
        } else if adsr1 & 0x80 != 0 {
            match self.stage {
                0 => {
                    let r = (adsr1 & 15) as u32 * 2 + 1;
                    (r, Move::Add(if r == 31 { 1024 } else { 32 }))
                }
                1 => (((adsr1 >> 4) & 7) as u32 * 2 + 16, Move::Exp),
                _ => ((adsr2 & 31) as u32, Move::Exp),
            }
        } else if gain & 0x80 == 0 {
            self.env = (gain as i32 & 0x7F) * 16;
            return;
        } else {
            let r = (gain & 31) as u32;
            match (gain >> 5) & 3 {
                0 => (r, Move::Add(-32)),
                1 => (r, Move::Exp),
                2 => (r, Move::Add(32)),
                _ => (r, Move::Bent),
            }
        };
        let every = RATES[rate as usize & 31];
        if every == 0 || self.counter % every != 0 {
            return;
        }
        match mv {
            Move::Add(d) => self.env += d,
            Move::Exp => self.env -= ((self.env - 1) >> 8) + 1,
            Move::Bent => self.env += if self.env < 0x600 { 32 } else { 8 },
        }
        self.env = self.env.clamp(0, 0x7FF);
        if self.stage == 0 && adsr1 & 0x80 != 0 && self.env >= 0x7E0 {
            self.stage = 1;
        }
        if self.stage == 1 && self.env <= ((adsr2 >> 5) as i32 + 1) * 0x100 {
            self.stage = 2;
        }
    }
}

/// The voices frame by frame (`rate` a second), and the samples they play.
pub fn voices(run: &Run, rate: f64, hash: &str) -> (Vec<ChipVoice>, Vec<SampleAsset>, HashMap<u8, (String, f32)>) {
    let mut dsp = run.dsp0;
    let mut v: [DspVoice; 8] = [DspVoice { stage: 4, ..Default::default() }; 8];
    let mut voices: Vec<ChipVoice> = (0..8).map(|k| ChipVoice { name: format!("voice{}", k + 1), kind: Kind::Sample, frames: Vec::new() }).collect();
    let mut retrig = [false; 8];
    let mut used: HashMap<u8, (String, f32)> = HashMap::new();
    let mut assets = Vec::new();
    let mut w = 0;
    let frame = 1.0 / rate;
    let mut next_frame = frame;
    let samples_per = 32_000.0 / rate;
    let mut s = 0u64;
    let total = (run.seconds * 32_000.0) as u64;
    while s < total {
        let t = s as f64 / 32_000.0;
        while w < run.writes.len() && run.writes[w].0 <= t {
            let (_, reg, val) = run.writes[w];
            dsp[reg as usize] = val;
            match reg {
                0x4C => {
                    for k in 0..8 {
                        if val & (1 << k) != 0 {
                            v[k] = DspVoice { env: 0, stage: 0, counter: 0 };
                            retrig[k] = true;
                        }
                    }
                }
                0x5C => {
                    for (k, voice) in v.iter_mut().enumerate() {
                        if val & (1 << k) != 0 && voice.stage < 3 {
                            voice.stage = 3;
                        }
                    }
                }
                _ => {}
            }
            w += 1;
        }
        for (k, voice) in v.iter_mut().enumerate() {
            if voice.stage < 4 {
                let base = k * 16;
                voice.step(dsp[base + 5], dsp[base + 6], dsp[base + 7]);
                if voice.stage == 3 && voice.env == 0 {
                    voice.stage = 4;
                }
            }
        }
        s += 1;
        if t + 1.0 / 32_000.0 >= next_frame {
            let mvol = (dsp[0x0C] as i8 as f32).abs().max((dsp[0x1C] as i8 as f32).abs()).max(1.0) / 127.0;
            for k in 0..8 {
                let base = k * 16;
                let srcn = dsp[base + 4];
                let p = (dsp[base + 2] as u16 | ((dsp[base + 3] as u16 & 0x3F) << 8)).max(1);
                let vol = (dsp[base] as i8 as f32).abs().max((dsp[base + 1] as i8 as f32).abs()) / 127.0;
                let noise = dsp[0x3D] & (1 << k) != 0;
                let entry = used.entry(srcn).or_insert_with(|| {
                    let b = brr(&run.ram, dsp[0x5D], srcn);
                    let mut sample = makepad_score_player::Sample::new(b.pcm.clone().into(), None, 32_000.0);
                    if let Some((a, e)) = b.looped {
                        sample = sample.with_loop(a, e);
                    }
                    let id = format!("snes-{hash}-s{srcn}");
                    assets.push(SampleAsset { asset: makepad_score_live::AssetRef { id: id.clone(), rev: "1".into() }, sample: Arc::new(sample), name: format!("sample {srcn}") });
                    (id, b.root.unwrap_or(f32::NAN))
                });
                let root = if entry.1.is_finite() { entry.1 } else { 60.0 };
                let level = if v[k].stage < 4 { v[k].env as f32 / 0x7FF as f32 * vol * mvol } else { 0.0 };
                let pitch = root + 12.0 * (p as f32 / 4096.0).log2();
                voices[k].frames.push(Frame { t: next_frame, pitch: if noise { 60.0 } else { pitch }, level, retrigger: retrig[k], timbre: srcn as u16 | if noise { 0x100 } else { 0 }, cutoff: f32::NAN });
                retrig[k] = false;
            }
            next_frame += frame;
            let _ = samples_per;
        }
    }
    voices.retain(|v| v.frames.iter().any(|f| f.level > 0.01));
    (voices, assets, used)
}

/// A voice's patch: its sample on a sampler at its found root.
pub fn patch(timbre: u16, used: &HashMap<u8, (String, f32)>) -> (String, bool) {
    if timbre & 0x100 != 0 {
        return ("Synth{noise: @white attack: 0.002 decay: 0 sustain: 1 release: 0.02 gain: 0.4}".into(), false);
    }
    match used.get(&(timbre as u8)) {
        Some((id, root)) => {
            let pitched = root.is_finite();
            let root = if pitched { *root } else { 60.0 };
            (format!("Sampler{{src: asset(\"{id}\", \"1\") root: {} attack: 0 release: 0.02}}", (root * 100.0).round() / 100.0), pitched)
        }
        None => ("Synth{osc: @square}".into(), true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sample_pitch_is_found() {
        // A4 (440 Hz) with a strong second harmonic, two seconds at 32 kHz.
        let tone = |hz: f32| -> Vec<f32> { (0..64000).map(|i| { let t = i as f32 / 32000.0; (std::f32::consts::TAU * hz * t).sin() + 0.6 * (std::f32::consts::TAU * 2.0 * hz * t).sin() }).collect() };
        let a4 = pitch_of(&tone(440.0), None).unwrap();
        assert!((a4 - 69.0).abs() < 0.1, "{a4}");
        let c2 = pitch_of(&tone(65.41), None).unwrap();
        assert!((c2 - 36.0).abs() < 0.1, "{c2}");
        // A single 16-sample cycle looped: 2 kHz.
        let cycle: Vec<f32> = (0..16).map(|i| if i < 8 { 0.5 } else { -0.5 }).collect();
        let b6 = pitch_of(&cycle, Some((0, 16))).unwrap();
        assert!((b6 - (69.0 + 12.0 * (2000.0f32 / 440.0).log2())).abs() < 0.1, "{b6}");
        // Noise has none.
        let mut x = 1u32;
        let noise: Vec<f32> = (0..8192).map(|_| { x ^= x << 13; x ^= x >> 17; x ^= x << 5; (x as f32 / u32::MAX as f32) - 0.5 }).collect();
        assert!(pitch_of(&noise, None).is_none());
    }
}
