//! NES music (NSF files): the tune's 6502 code on the same CPU as the SID
//! player (the 2A03 has no decimal mode), in a small NES (2 KB of RAM,
//! work RAM, the ROM in 4 KB banks with NSF bank switching), its play
//! routine called each frame; the APU's register writes followed by a
//! model of its channels (two pulses with their duty, envelopes, sweeps
//! and length counters, the triangle with its linear counter, the noise,
//! the DPCM sample channel) read frame by frame into [`super::voices`]:
//! pulse, triangle and noise patches, and the DPCM samples themselves on a
//! sampler.

use super::sid::cpu::{Bus, Cpu};
use super::tracker::arrange::SampleAsset;
use super::voices::{ChipVoice, Frame, Kind};
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct Header {
    pub songs: u8,
    pub start: u8,
    pub load: u16,
    pub init: u16,
    pub play: u16,
    pub name: String,
    pub artist: String,
    pub copyright: String,
    /// Microseconds a frame (NTSC).
    pub ntsc_speed: u16,
    pub pal_speed: u16,
    pub banks: [u8; 8],
    pub pal: bool,
    /// Expansion audio the tune uses (VRC6, VRC7, FDS, MMC5, N163, 5B).
    pub chips: u8,
}

pub fn is_nsf(bytes: &[u8]) -> bool {
    bytes.len() > 0x80 && &bytes[..5] == b"NESM\x1A"
}

pub fn is_nsfe(bytes: &[u8]) -> bool {
    bytes.len() > 8 && &bytes[..4] == b"NSFE"
}

/// An NSFe (chunks: INFO, DATA, BANK, auth, tlbl ...) as the NSF it
/// describes, and its track names.
pub fn from_nsfe(b: &[u8]) -> Result<(Vec<u8>, Vec<String>), String> {
    if !is_nsfe(b) {
        return Err("not an NSFe file".into());
    }
    let mut at = 4;
    let mut info: Option<&[u8]> = None;
    let mut data: &[u8] = &[];
    let mut bank = [0u8; 8];
    let mut auth: Vec<String> = Vec::new();
    let mut labels: Vec<String> = Vec::new();
    while at + 8 <= b.len() {
        let len = u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]) as usize;
        let id = &b[at + 4..at + 8];
        let body = b.get(at + 8..(at + 8 + len).min(b.len())).unwrap_or(&[]);
        match id {
            b"INFO" => info = Some(body),
            b"DATA" => data = body,
            b"BANK" => bank[..body.len().min(8)].copy_from_slice(&body[..body.len().min(8)]),
            b"auth" => auth = body.split(|c| *c == 0).map(|s| String::from_utf8_lossy(s).trim().to_string()).collect(),
            b"tlbl" => labels = body.split(|c| *c == 0).map(|s| String::from_utf8_lossy(s).trim().to_string()).collect(),
            b"NEND" => break,
            _ => {}
        }
        at += 8 + len;
    }
    let info = info.ok_or("NSFe: no INFO chunk")?;
    let get = |k: usize| info.get(k).copied().unwrap_or(0);
    let mut h = vec![0u8; 0x80];
    h[..5].copy_from_slice(b"NESM\x1A");
    h[5] = 1;
    h[6] = get(8).max(1);
    h[7] = get(9).saturating_add(1);
    h[8..14].copy_from_slice(&info[..6.min(info.len())]);
    let mut put = |at: usize, s: &str| {
        for (i, c) in s.bytes().take(31).enumerate() {
            h[at + i] = c;
        }
    };
    put(0x0E, auth.first().map_or("", |s| s.as_str()));
    put(0x2E, auth.get(1).map_or("", |s| s.as_str()));
    put(0x4E, auth.get(2).map_or("", |s| s.as_str()));
    h[0x6E..0x70].copy_from_slice(&16_639u16.to_le_bytes());
    h[0x70..0x78].copy_from_slice(&bank);
    h[0x78..0x7A].copy_from_slice(&19_997u16.to_le_bytes());
    h[0x7A] = get(6) & 3;
    h[0x7B] = get(7);
    h.extend_from_slice(data);
    Ok((h, labels))
}

fn text(b: &[u8]) -> String {
    b.iter().take_while(|&&c| c != 0).map(|&c| c as char).collect::<String>().trim().to_string()
}

pub fn header(bytes: &[u8]) -> Result<Header, String> {
    if !is_nsf(bytes) {
        return Err("not an NSF file".into());
    }
    let u16le = |at: usize| u16::from_le_bytes([bytes[at], bytes[at + 1]]);
    let mut banks = [0u8; 8];
    banks.copy_from_slice(&bytes[0x70..0x78]);
    Ok(Header {
        songs: bytes[6].max(1),
        start: bytes[7].max(1),
        load: u16le(8),
        init: u16le(0x0A),
        play: u16le(0x0C),
        name: text(&bytes[0x0E..0x2E]),
        artist: text(&bytes[0x2E..0x4E]),
        copyright: text(&bytes[0x4E..0x6E]),
        ntsc_speed: u16le(0x6E),
        pal_speed: u16le(0x78),
        banks,
        pal: bytes[0x7A] & 3 == 1,
        chips: bytes[0x7B],
    })
}

/// A write to the APU.
#[derive(Clone, Copy, Debug)]
pub struct Write {
    pub t: f64,
    pub reg: u8,
    pub value: u8,
}

struct Nes {
    ram: [u8; 0x800],
    wram: Vec<u8>,
    rom: Vec<u8>,
    banked: bool,
    bank: [usize; 8],
    now: u64,
    clock: f64,
    writes: Vec<Write>,
}

impl Bus for Nes {
    fn read(&mut self, addr: u16) -> u8 {
        match addr {
            0x0000..=0x1FFF => self.ram[(addr & 0x7FF) as usize],
            0x6000..=0x7FFF => self.wram[(addr - 0x6000) as usize],
            0x8000..=0xFFFF => {
                let at = if self.banked { self.bank[((addr - 0x8000) >> 12) as usize] * 0x1000 + (addr as usize & 0xFFF) } else { addr as usize - 0x8000 };
                self.rom.get(at).copied().unwrap_or(0)
            }
            _ => 0,
        }
    }

    fn write(&mut self, addr: u16, value: u8) {
        match addr {
            0x0000..=0x1FFF => self.ram[(addr & 0x7FF) as usize] = value,
            0x4000..=0x4017 => self.writes.push(Write { t: self.now as f64 / self.clock, reg: (addr - 0x4000) as u8, value }),
            0x5FF8..=0x5FFF => self.bank[(addr - 0x5FF8) as usize] = value as usize,
            0x6000..=0x7FFF => self.wram[(addr - 0x6000) as usize] = value,
            _ => {}
        }
    }
}

fn call(cpu: &mut Cpu, m: &mut Nes, addr: u16, a: u8, x: u8) {
    let sp0 = cpu.sp;
    cpu.push(m, 0xFF);
    cpu.push(m, 0xFF);
    cpu.pc = addr;
    cpu.a = a;
    cpu.x = x;
    let start = cpu.cycles;
    loop {
        m.now = cpu.cycles;
        let op = m.read(cpu.pc);
        cpu.step(m);
        if op == 0x60 && cpu.sp == sp0 {
            return;
        }
        if cpu.jammed || cpu.cycles - start > 8_000_000 {
            cpu.sp = sp0;
            cpu.jammed = false;
            return;
        }
    }
}

pub struct Run {
    pub writes: Vec<Write>,
    pub rate: f64,
    pub clock: f64,
    pub seconds: f64,
    /// The CPU's memory at the end ($8000 up), where DPCM samples are read.
    pub rom: Box<dyn Fn(u16) -> u8>,
}

/// Run `song` (1-based) for `seconds`.
pub fn run(bytes: &[u8], song: u8, seconds: f64) -> Result<(Header, Run), String> {
    let h = header(bytes)?;
    let data = &bytes[0x80..];
    let banked = h.banks.iter().any(|&b| b != 0);
    let mut rom = vec![0u8; if banked { (h.load as usize & 0xFFF) + data.len() } else { 0x8000 }];
    if banked {
        let pad = h.load as usize & 0xFFF;
        rom[pad..pad + data.len()].copy_from_slice(data);
    } else {
        let at = (h.load as usize).saturating_sub(0x8000);
        let n = data.len().min(0x8000 - at.min(0x8000));
        rom[at..at + n].copy_from_slice(&data[..n]);
    }
    let clock = if h.pal { 1_662_607.0 } else { 1_789_773.0 };
    let speed = if h.pal { h.pal_speed } else { h.ntsc_speed };
    let rate = if speed > 0 { 1_000_000.0 / speed as f64 } else if h.pal { 50.007 } else { 60.099 };
    let mut m = Nes { ram: [0; 0x800], wram: vec![0; 0x2000], rom, banked, bank: [0, 1, 2, 3, 4, 5, 6, 7], now: 0, clock, writes: Vec::new() };
    if banked {
        for (k, &b) in h.banks.iter().enumerate() {
            m.bank[k] = b as usize;
        }
    }
    let mut cpu = Cpu::new(false);
    // The APU as a player expects it: channels on, the frame counter set.
    for (reg, v) in [(0x15u16, 0x0F), (0x17, 0x40)] {
        m.write(0x4000 + reg, v);
    }
    let song = song.clamp(1, h.songs);
    call(&mut cpu, &mut m, h.init, song - 1, h.pal as u8);
    let period = (clock / rate) as u64;
    let total = (seconds * clock) as u64;
    let mut next = cpu.cycles.max(1);
    while next < total {
        cpu.cycles = next;
        m.now = next;
        call(&mut cpu, &mut m, h.play, 0, 0);
        next += period.max(1000);
    }
    let Nes { rom, banked, bank, writes, .. } = m;
    let reader = move |addr: u16| -> u8 {
        let at = if banked { bank[((addr.max(0x8000) - 0x8000) >> 12) as usize] * 0x1000 + (addr as usize & 0xFFF) } else { (addr as usize).saturating_sub(0x8000) };
        rom.get(at).copied().unwrap_or(0)
    };
    Ok((h, Run { writes, rate, clock, seconds, rom: Box::new(reader) }))
}

const LENGTHS: [u8; 32] = [10, 254, 20, 2, 40, 4, 80, 6, 160, 8, 60, 10, 14, 12, 26, 14, 12, 16, 24, 18, 48, 20, 96, 22, 192, 24, 72, 26, 16, 28, 32, 30];
const NOISE_PERIODS: [u16; 16] = [4, 8, 16, 32, 64, 96, 128, 160, 202, 254, 380, 508, 762, 1016, 2034, 4068];
const DMC_RATES: [u16; 16] = [428, 380, 340, 320, 286, 254, 226, 214, 190, 160, 142, 128, 106, 84, 72, 54];

#[derive(Clone, Copy, Default)]
struct Env {
    start: bool,
    divider: u8,
    decay: u8,
}

impl Env {
    fn clock(&mut self, reg: u8) {
        let period = reg & 15;
        if self.start {
            self.start = false;
            self.decay = 15;
            self.divider = period;
        } else if self.divider == 0 {
            self.divider = period;
            if self.decay > 0 {
                self.decay -= 1;
            } else if reg & 0x20 != 0 {
                self.decay = 15;
            }
        } else {
            self.divider -= 1;
        }
    }

    fn volume(&self, reg: u8) -> u8 {
        if reg & 0x10 != 0 { reg & 15 } else { self.decay }
    }
}

#[derive(Clone, Copy, Default)]
struct Pulse {
    regs: [u8; 4],
    timer: u16,
    length: u8,
    env: Env,
    sweep_div: u8,
    sweep_reload: bool,
    retrigger: bool,
}

/// The APU's channels read every frame: pulse 1 and 2, triangle, noise,
/// and the DPCM starts (a sample by its address, length and rate).
pub fn voices(run: &Run) -> (Vec<ChipVoice>, Vec<(f64, u16, u16, u8)>) {
    let mut pulses = [Pulse::default(); 2];
    let mut tri_regs = [0u8; 4];
    let (mut tri_timer, mut tri_len, mut tri_lin, mut tri_reload, mut tri_retrig) = (0u16, 0u8, 0u8, false, false);
    let mut noise_regs = [0u8; 4];
    let (mut noise_len, mut noise_env, mut noise_retrig) = (0u8, Env::default(), false);
    let mut enabled = 0x0Fu8;
    let mut dmc = [0u8; 4];
    let mut dmc_starts: Vec<(f64, u16, u16, u8)> = Vec::new();
    let mut voices: Vec<ChipVoice> = ["pulse1", "pulse2", "triangle", "noise"].iter().enumerate().map(|(i, n)| ChipVoice { name: n.to_string(), kind: if i == 3 { Kind::Noise } else { Kind::Tone }, frames: Vec::new() }).collect();
    let quarter = 1.0 / 240.0;
    let frame = 1.0 / run.rate;
    let mut w = 0;
    let mut t = 0.0;
    let mut next_frame = frame * 0.95;
    let mut q = 0u64;
    let clock = run.clock;
    while t < run.seconds {
        // Writes up to now.
        while w < run.writes.len() && run.writes[w].t <= t {
            let wr = run.writes[w];
            let v = wr.value;
            match wr.reg {
                0x00..=0x07 => {
                    let p = &mut pulses[(wr.reg / 4) as usize];
                    let r = (wr.reg % 4) as usize;
                    p.regs[r] = v;
                    match r {
                        1 => p.sweep_reload = true,
                        2 => p.timer = (p.timer & 0x700) | v as u16,
                        3 => {
                            p.timer = (p.timer & 0xFF) | ((v as u16 & 7) << 8);
                            if enabled & (1 << (wr.reg / 4)) != 0 {
                                p.length = LENGTHS[(v >> 3) as usize];
                            }
                            p.env.start = true;
                            p.retrigger = true;
                        }
                        _ => {}
                    }
                }
                0x08 => tri_regs[0] = v,
                0x0A => tri_timer = (tri_timer & 0x700) | v as u16,
                0x0B => {
                    tri_timer = (tri_timer & 0xFF) | ((v as u16 & 7) << 8);
                    if enabled & 4 != 0 {
                        tri_len = LENGTHS[(v >> 3) as usize];
                    }
                    tri_reload = true;
                    tri_retrig = true;
                }
                0x0C => noise_regs[0] = v,
                0x0E => noise_regs[2] = v,
                0x0F => {
                    if enabled & 8 != 0 {
                        noise_len = LENGTHS[(v >> 3) as usize];
                    }
                    noise_env.start = true;
                    noise_retrig = true;
                }
                0x10..=0x13 => dmc[(wr.reg - 0x10) as usize] = v,
                0x15 => {
                    // DMC (re)start when enabled.
                    if v & 0x10 != 0 {
                        dmc_starts.push((wr.t, 0xC000 + dmc[2] as u16 * 64, dmc[3] as u16 * 16 + 1, dmc[0] & 15));
                    }
                    enabled = v;
                    if v & 1 == 0 {
                        pulses[0].length = 0;
                    }
                    if v & 2 == 0 {
                        pulses[1].length = 0;
                    }
                    if v & 4 == 0 {
                        tri_len = 0;
                    }
                    if v & 8 == 0 {
                        noise_len = 0;
                    }
                }
                _ => {}
            }
            w += 1;
        }
        // The frame sequencer: envelopes and the linear counter every
        // quarter, lengths and sweeps every half.
        for p in pulses.iter_mut() {
            p.env.clock(p.regs[0]);
        }
        noise_env.clock(noise_regs[0]);
        if tri_reload {
            tri_lin = tri_regs[0] & 0x7F;
        } else if tri_lin > 0 {
            tri_lin -= 1;
        }
        if tri_regs[0] & 0x80 == 0 {
            tri_reload = false;
        }
        if q % 2 == 1 {
            for (k, p) in pulses.iter_mut().enumerate() {
                if p.regs[0] & 0x20 == 0 && p.length > 0 {
                    p.length -= 1;
                }
                let s = p.regs[1];
                let shift = s & 7;
                let change = p.timer >> shift;
                let target = if s & 8 != 0 { p.timer.wrapping_sub(change).wrapping_sub((k == 0) as u16) } else { p.timer + change };
                if p.sweep_div == 0 && s & 0x80 != 0 && shift > 0 && p.timer >= 8 && target <= 0x7FF {
                    p.timer = target;
                }
                if p.sweep_div == 0 || p.sweep_reload {
                    p.sweep_div = (s >> 4) & 7;
                    p.sweep_reload = false;
                } else {
                    p.sweep_div -= 1;
                }
            }
            if tri_regs[0] & 0x80 == 0 && tri_len > 0 {
                tri_len -= 1;
            }
            if noise_regs[0] & 0x20 == 0 && noise_len > 0 {
                noise_len -= 1;
            }
        }
        q += 1;
        t += quarter;
        // A frame's look at the channels.
        while next_frame <= t {
            for (k, p) in pulses.iter_mut().enumerate() {
                let audible = p.length > 0 && p.timer >= 8;
                let vol = if audible { p.env.volume(p.regs[0]) } else { 0 };
                let hz = clock / (16.0 * (p.timer as f64 + 1.0));
                voices[k].frames.push(Frame { t: next_frame, pitch: (69.0 + 12.0 * (hz / 440.0).log2()) as f32, level: vol as f32 / 15.0, retrigger: p.retrigger, timbre: (p.regs[0] >> 6) as u16, cutoff: f32::NAN });
                p.retrigger = false;
            }
            let audible = tri_len > 0 && tri_lin > 0 && tri_timer >= 2;
            let hz = clock / (32.0 * (tri_timer as f64 + 1.0));
            voices[2].frames.push(Frame { t: next_frame, pitch: (69.0 + 12.0 * (hz / 440.0).log2()) as f32, level: if audible { 1.0 } else { 0.0 }, retrigger: tri_retrig, timbre: 0, cutoff: f32::NAN });
            tri_retrig = false;
            let vol = if noise_len > 0 { noise_env.volume(noise_regs[0]) } else { 0 };
            let period = NOISE_PERIODS[(noise_regs[2] & 15) as usize] as f64;
            // The noise's colour: its clock as a pitch and a cutoff.
            let hz = clock / period / 2.0;
            voices[3].frames.push(Frame { t: next_frame, pitch: (60.0 - (noise_regs[2] & 15) as f32 * 2.0), level: vol as f32 / 15.0, retrigger: noise_retrig, timbre: (noise_regs[2] >> 7) as u16, cutoff: hz.clamp(200.0, 16_000.0) as f32 });
            noise_retrig = false;
            next_frame += frame;
        }
    }
    (voices, dmc_starts)
}

/// The DPCM samples played: each (address, length) decoded once, a part
/// whose notes are where it starts and its rate as the key.
pub fn dpcm(run: &Run, starts: &[(f64, u16, u16, u8)], hash: &str) -> (Option<ChipVoice>, Vec<SampleAsset>) {
    if starts.is_empty() {
        return (None, Vec::new());
    }
    let mut samples: Vec<(u16, u16)> = Vec::new();
    let mut assets = Vec::new();
    let mut frames = Vec::new();
    // The rate 15 (33 kHz) plays a sample at the root.
    let root_rate = run.clock / DMC_RATES[15] as f64;
    for &(t, addr, len, rate) in starts {
        let k = match samples.iter().position(|s| *s == (addr, len)) {
            Some(k) => k,
            None => {
                samples.push((addr, len));
                // 1-bit deltas from the middle level.
                let mut level: i32 = 64;
                let mut pcm = Vec::with_capacity(len as usize * 8);
                for i in 0..len {
                    let byte = (run.rom)(addr.wrapping_add(i).max(0x8000));
                    for b in 0..8 {
                        if byte >> b & 1 != 0 {
                            if level <= 125 {
                                level += 2;
                            }
                        } else if level >= 2 {
                            level -= 2;
                        }
                        pcm.push((level - 64) as f32 / 64.0);
                    }
                }
                let sample = makepad_score_player::Sample::new(pcm.into(), None, root_rate as f32);
                let asset = makepad_score_live::AssetRef { id: format!("nes-{hash}-d{}", samples.len()), rev: "1".into() };
                assets.push(SampleAsset { asset, sample: Arc::new(sample), name: format!("DPCM ${addr:04X}+{len}") });
                samples.len() - 1
            }
        };
        let hz_ratio = (DMC_RATES[15] as f64 / DMC_RATES[rate as usize] as f64).log2() * 12.0;
        frames.push(Frame { t, pitch: (60.0 + hz_ratio) as f32, level: 1.0, retrigger: true, timbre: k as u16, cutoff: f32::NAN });
        // Silent after its length.
        let secs = len as f64 * 8.0 / (run.clock / DMC_RATES[rate as usize] as f64);
        frames.push(Frame { t: t + secs, pitch: f32::NAN, level: 0.0, retrigger: false, timbre: k as u16, cutoff: f32::NAN });
    }
    frames.sort_by(|a, b| a.t.total_cmp(&b.t));
    (Some(ChipVoice { name: "dpcm".into(), kind: Kind::Sample, frames }), assets)
}

/// A NES channel's patch.
pub fn patch(voice: &ChipVoice, timbre: u16, sample_ids: &[String]) -> (String, bool) {
    (patch_node(voice, timbre, sample_ids), voice.kind == Kind::Tone)
}

fn patch_node(voice: &ChipVoice, timbre: u16, sample_ids: &[String]) -> String {
    match (voice.kind, voice.name.as_str()) {
        (Kind::Tone, "triangle") => "Synth{osc: @tri attack: 0.001 decay: 0 sustain: 1 release: 0.01 gain: 0.55}".into(),
        (Kind::Tone, _) => {
            let width = [0.125, 0.25, 0.5, 0.75][timbre as usize & 3];
            format!("Synth{{osc: @pulse width: {width} attack: 0.001 decay: 0 sustain: 1 release: 0.01 gain: 0.4}}")
        }
        (Kind::Noise, _) => {
            // The short (metallic) mode rings; the long one hisses.
            if timbre == 1 { "Synth{noise: @white filter: @bp cutoff: 3000 res: 6 attack: 0.001 decay: 0 sustain: 1 release: 0.01 gain: 0.5}".into() } else { "Synth{noise: @white attack: 0.001 decay: 0 sustain: 1 release: 0.01 gain: 0.4}".into() }
        }
        (Kind::Sample, _) => match sample_ids.get(timbre as usize) {
            Some(id) => format!("Sampler{{src: asset(\"{id}\", \"1\") root: 60 attack: 0 release: 0.005}}"),
            None => "Synth{noise: @white decay: 0.1 sustain: 0}".into(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_nsfe_reads_as_its_nsf() {
        let chunk = |id: &[u8], body: &[u8]| -> Vec<u8> { [&(body.len() as u32).to_le_bytes()[..], id, body].concat() };
        let mut f = b"NSFE".to_vec();
        f.extend(chunk(b"INFO", &[0x00, 0x80, 0x03, 0x80, 0x06, 0x80, 0, 0, 5, 2]));
        f.extend(chunk(b"DATA", &[0x60, 0x60, 0x60, 0x60, 0x60, 0x60, 0x60]));
        f.extend(chunk(b"auth", b"Game\0Composer\0Company\0Ripper\0"));
        f.extend(chunk(b"tlbl", b"Intro\0Stage 1\0Boss\0\0\0"));
        f.extend(chunk(b"NEND", &[]));
        let (nsf, labels) = from_nsfe(&f).unwrap();
        let h = header(&nsf).unwrap();
        assert_eq!((h.load, h.init, h.play, h.songs, h.start, h.name.as_str(), h.artist.as_str()), (0x8000, 0x8003, 0x8006, 5, 3, "Game", "Composer"));
        assert_eq!(labels[2], "Boss");
        assert_eq!(&nsf[0x80..], &[0x60; 7]);
    }
}
