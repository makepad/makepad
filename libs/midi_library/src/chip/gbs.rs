//! Game Boy music (GBS files): the tune's code on our LR35902 in a small
//! Game Boy (ROM banks, work and high RAM), its play routine called at the
//! vertical blank or the timer rate the file asks for; the APU's writes
//! followed by a model of its four channels (two pulses with their duty,
//! envelopes and length, the wave channel with its 32-step wave table,
//! the noise) read frame by frame into [`super::voices`]. A wave table
//! becomes a one-cycle looped sample, so the wave channel sounds as it did.

use super::lr35902::{Bus, Lr35902};
use super::tracker::arrange::SampleAsset;
use super::voices::{ChipVoice, Frame, Kind};
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct Header {
    pub songs: u8,
    pub first: u8,
    pub load: u16,
    pub init: u16,
    pub play: u16,
    pub sp: u16,
    pub tma: u8,
    pub tac: u8,
    pub title: String,
    pub author: String,
    pub copyright: String,
}

pub fn is_gbs(b: &[u8]) -> bool {
    b.len() > 0x70 && &b[..3] == b"GBS"
}

fn text(b: &[u8]) -> String {
    b.iter().take_while(|&&c| c != 0).map(|&c| c as char).collect::<String>().trim().to_string()
}

pub fn header(b: &[u8]) -> Result<Header, String> {
    if !is_gbs(b) {
        return Err("not a GBS file".into());
    }
    let u16le = |at: usize| u16::from_le_bytes([b[at], b[at + 1]]);
    Ok(Header { songs: b[4].max(1), first: b[5].max(1), load: u16le(6), init: u16le(8), play: u16le(0x0A), sp: u16le(0x0C), tma: b[0x0E], tac: b[0x0F], title: text(&b[0x10..0x30]), author: text(&b[0x30..0x50]), copyright: text(&b[0x50..0x70]) })
}

struct Gb {
    rom: Vec<u8>,
    bank: usize,
    ram: Vec<u8>,
    io: [u8; 0x100],
    now: u64,
    writes: Vec<(f64, u8, u8)>,
}

const CLOCK: f64 = 4_194_304.0;

impl Bus for Gb {
    fn read(&mut self, a: u16) -> u8 {
        match a {
            0x0000..=0x3FFF => self.rom.get(a as usize).copied().unwrap_or(0xFF),
            0x4000..=0x7FFF => self.rom.get(self.bank.max(1) * 0x4000 + (a as usize - 0x4000)).copied().unwrap_or(0xFF),
            0xFF00..=0xFFFF => self.io[(a & 0xFF) as usize],
            0xE000..=0xFDFF => self.ram[(a - 0x2000) as usize],
            _ => self.ram[a as usize],
        }
    }

    fn write(&mut self, a: u16, v: u8) {
        match a {
            0x2000..=0x3FFF => self.bank = v as usize,
            0x0000..=0x7FFF => {}
            0xFF00..=0xFFFF => {
                let r = (a & 0xFF) as u8;
                self.io[r as usize] = v;
                if (0x10..=0x3F).contains(&r) {
                    self.writes.push((self.now as f64 / CLOCK, r, v));
                }
            }
            0xE000..=0xFDFF => self.ram[(a - 0x2000) as usize] = v,
            _ => self.ram[a as usize] = v,
        }
    }
}

fn call(cpu: &mut Lr35902, m: &mut Gb, addr: u16, a: u8) {
    let sp0 = cpu.sp;
    cpu.push(m, 0x0000);
    cpu.pc = addr;
    cpu.a = a;
    cpu.halted = false;
    let start = cpu.cycles;
    loop {
        m.now = cpu.cycles;
        let op = m.read(cpu.pc);
        cpu.step(m);
        let _ = op;
        if cpu.pc == 0 && cpu.sp == sp0 {
            return;
        }
        if cpu.halted || cpu.cycles - start > 20_000_000 {
            cpu.sp = sp0;
            return;
        }
    }
}

pub struct Run {
    pub writes: Vec<(f64, u8, u8)>,
    pub rate: f64,
    pub seconds: f64,
}

pub fn run(b: &[u8], song: u8, seconds: f64) -> Result<(Header, Run), String> {
    let h = header(b)?;
    let data = &b[0x70..];
    let mut rom = vec![0xFFu8; h.load as usize + data.len()];
    rom[h.load as usize..].copy_from_slice(data);
    // RST vectors jump into the load area as the format says.
    for k in 0..8u16 {
        let at = (k * 8) as usize;
        if at + 3 <= rom.len() && h.load as usize > at + 3 {
            let to = h.load + k * 8;
            rom[at] = 0xC3;
            rom[at + 1] = to as u8;
            rom[at + 2] = (to >> 8) as u8;
        }
    }
    let mut m = Gb { rom, bank: 1, ram: vec![0; 0x10000], io: [0; 0x100], now: 0, writes: Vec::new() };
    m.io[0x26] = 0x80;
    let mut cpu = Lr35902 { sp: h.sp, ..Default::default() };
    let song = song.clamp(1, h.songs);
    call(&mut cpu, &mut m, h.init, song - 1);
    let rate = if h.tac & 4 != 0 {
        let clocks = [4096.0, 262_144.0, 65_536.0, 16_384.0][(h.tac & 3) as usize] * if h.tac & 0x80 != 0 { 2.0 } else { 1.0 };
        clocks / (256.0 - h.tma as f64)
    } else {
        59.7275
    };
    let period = (CLOCK / rate) as u64;
    let total = (seconds * CLOCK) as u64;
    let mut next = cpu.cycles.max(1);
    while next < total {
        cpu.cycles = next;
        m.now = next;
        call(&mut cpu, &mut m, h.play, 0);
        next += period.max(1000);
    }
    Ok((h, Run { writes: m.writes, rate, seconds }))
}

#[derive(Clone, Copy, Default)]
struct Square {
    regs: [u8; 5],
    vol: u8,
    env_t: f64,
    on: bool,
    len: u16,
    retrig: bool,
}

/// The four channels every frame (`rate` a second), and the wave tables
/// as looped one-cycle samples.
pub fn voices(run: &Run, rate: f64, hash: &str) -> (Vec<ChipVoice>, Vec<SampleAsset>) {
    let mut sq = [Square::default(); 2];
    let mut wave = Square::default();
    let mut noise = Square::default();
    let mut wave_ram = [0u8; 16];
    let mut tables: Vec<[u8; 16]> = Vec::new();
    let mut wave_timbre = 0u16;
    let mut assets = Vec::new();
    let mut voices: Vec<ChipVoice> = [("pulse1", Kind::Tone), ("pulse2", Kind::Tone), ("wave", Kind::Sample), ("noise", Kind::Noise)].iter().map(|(n, k)| ChipVoice { name: n.to_string(), kind: *k, frames: Vec::new() }).collect();
    let frame = 1.0 / rate;
    let mut t = frame * 0.95;
    let mut w = 0;
    let mut last = 0.0;
    let table_of = |ram: &[u8; 16], tables: &mut Vec<[u8; 16]>, assets: &mut Vec<SampleAsset>| -> u16 {
        if let Some(k) = tables.iter().position(|x| x == ram) {
            return k as u16;
        }
        tables.push(*ram);
        // One cycle of 32 four-bit steps, at the rate that makes it C4.
        let pcm: Vec<f32> = (0..32).map(|i| {
            let b = ram[i / 2];
            let v = if i % 2 == 0 { b >> 4 } else { b & 15 };
            (v as f32 - 7.5) / 7.5 * 0.6
        }).collect();
        let hz_c4 = 261.6256;
        let sample = makepad_score_player::Sample::new(pcm.into(), None, (32.0 * hz_c4) as f32).with_loop(0, 32);
        let id = format!("gb-{hash}-w{}", tables.len());
        assets.push(SampleAsset { asset: makepad_score_live::AssetRef { id, rev: "1".into() }, sample: Arc::new(sample), name: format!("wave table {}", tables.len()) });
        (tables.len() - 1) as u16
    };
    while t < run.seconds {
        while w < run.writes.len() && run.writes[w].0 <= t {
            let (_, r, v) = run.writes[w];
            match r {
                0x10..=0x14 => {
                    sq[0].regs[(r - 0x10) as usize] = v;
                    if r == 0x14 && v & 0x80 != 0 {
                        sq[0].on = true;
                        sq[0].vol = sq[0].regs[2] >> 4;
                        sq[0].env_t = 0.0;
                        sq[0].len = 64 - (sq[0].regs[1] & 63) as u16;
                        sq[0].retrig = true;
                    }
                }
                0x15..=0x19 => {
                    sq[1].regs[(r - 0x15) as usize] = v;
                    if r == 0x19 && v & 0x80 != 0 {
                        sq[1].on = true;
                        sq[1].vol = sq[1].regs[2] >> 4;
                        sq[1].env_t = 0.0;
                        sq[1].len = 64 - (sq[1].regs[1] & 63) as u16;
                        sq[1].retrig = true;
                    }
                }
                0x1A..=0x1E => {
                    wave.regs[(r - 0x1A) as usize] = v;
                    if r == 0x1E && v & 0x80 != 0 {
                        wave.on = wave.regs[0] & 0x80 != 0;
                        wave.len = 256 - wave.regs[1] as u16;
                        wave.retrig = true;
                        wave_timbre = table_of(&wave_ram, &mut tables, &mut assets);
                    }
                }
                0x1F..=0x23 => {
                    noise.regs[(r - 0x1F) as usize] = v;
                    if r == 0x23 && v & 0x80 != 0 {
                        noise.on = true;
                        noise.vol = noise.regs[2] >> 4;
                        noise.env_t = 0.0;
                        noise.len = 64 - (noise.regs[1] & 63) as u16;
                        noise.retrig = true;
                    }
                }
                0x30..=0x3F => wave_ram[(r - 0x30) as usize] = v,
                _ => {}
            }
            w += 1;
        }
        // Envelopes (period/64 s a step) and lengths (256 Hz) since the last frame.
        let dt = t - last;
        last = t;
        for c in sq.iter_mut().chain(std::iter::once(&mut noise)) {
            let env = c.regs[2];
            let period = (env & 7) as f64 / 64.0;
            if c.on && period > 0.0 {
                c.env_t += dt;
                while c.env_t >= period {
                    c.env_t -= period;
                    if env & 8 != 0 {
                        c.vol = (c.vol + 1).min(15);
                    } else {
                        c.vol = c.vol.saturating_sub(1);
                    }
                }
            }
        }
        for c in sq.iter_mut() {
            if c.regs[4] & 0x40 != 0 && c.len > 0 {
                let steps = (dt * 256.0).round() as u16;
                c.len = c.len.saturating_sub(steps);
                if c.len == 0 {
                    c.on = false;
                }
            }
        }
        for (k, c) in sq.iter_mut().enumerate() {
            let x = c.regs[3] as u16 | ((c.regs[4] as u16 & 7) << 8);
            let hz = 131_072.0 / (2048.0 - x as f64);
            let level = if c.on && c.regs[2] & 0xF8 != 0 { c.vol as f32 / 15.0 } else { 0.0 };
            voices[k].frames.push(Frame { t, pitch: (69.0 + 12.0 * (hz / 440.0).log2()) as f32, level, retrigger: c.retrig, timbre: (c.regs[1] >> 6) as u16, cutoff: f32::NAN });
            c.retrig = false;
        }
        let x = wave.regs[3] as u16 | ((wave.regs[4] as u16 & 7) << 8);
        let hz = 65_536.0 / (2048.0 - x as f64);
        let out = [0.0, 1.0, 0.5, 0.25][((wave.regs[2] >> 5) & 3) as usize];
        let level = if wave.on && wave.regs[0] & 0x80 != 0 { out } else { 0.0 };
        voices[2].frames.push(Frame { t, pitch: (69.0 + 12.0 * (hz / 440.0).log2()) as f32, level, retrigger: wave.retrig, timbre: wave_timbre, cutoff: f32::NAN });
        wave.retrig = false;
        let nr43 = noise.regs[3];
        let r = (nr43 & 7) as f64;
        let s = (nr43 >> 4) as i32;
        let nhz = 524_288.0 / if r == 0.0 { 0.5 } else { r } / 2f64.powi(s + 1);
        let level = if noise.on && noise.regs[2] & 0xF8 != 0 { noise.vol as f32 / 15.0 } else { 0.0 };
        voices[3].frames.push(Frame { t, pitch: (72 - s.min(13) * 3) as f32, level, retrigger: noise.retrig, timbre: ((nr43 >> 3) & 1) as u16, cutoff: (nhz.clamp(100.0, 16_000.0)) as f32 });
        noise.retrig = false;
        t += frame;
    }
    voices.retain(|v| v.frames.iter().any(|f| f.level > 0.01));
    (voices, assets)
}

pub fn patch(voice: &ChipVoice, timbre: u16, wave_ids: &[String]) -> (String, bool) {
    let flat = "attack: 0.001 decay: 0 sustain: 1 release: 0.01";
    match voice.kind {
        Kind::Tone => (format!("Synth{{osc: @pulse width: {} {flat} gain: 0.4}}", [0.125, 0.25, 0.5, 0.75][timbre as usize & 3]), true),
        Kind::Sample => match wave_ids.get(timbre as usize) {
            Some(id) => (format!("Sampler{{src: asset(\"{id}\", \"1\") root: 60 attack: 0 release: 0.01}}"), true),
            None => (format!("Synth{{osc: @tri {flat}}}"), true),
        },
        Kind::Noise => (if timbre == 1 { format!("Synth{{noise: @white filter: @bp cutoff: 3000 res: 6 {flat} gain: 0.45}}") } else { format!("Synth{{noise: @white {flat} gain: 0.4}}") }, false),
    }
}
