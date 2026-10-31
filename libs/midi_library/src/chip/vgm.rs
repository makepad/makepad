//! VGM and VGZ: a game's sound chips as a log of register writes with
//! waits (no CPU to run). The chips the consoles and arcades used most are
//! read back into [`super::voices`]: the SN76489 PSG (Master System, Game
//! Gear, Mega Drive: three square tones and a noise), the YM2612 (Mega
//! Drive: six FM channels, key-on gates, frequency and the carriers'
//! level), the YM2151 (arcade boards: eight FM channels) and the AY-3-8910
//! (three squares and noise). Credits come from the file's GD3 tag (track,
//! game, system, composer, release date). FM patches are approximated: a
//! sine pair at the operators' frequency ratio and modulation depth.

use super::voices::{ChipVoice, Frame, Kind};

#[derive(Clone, Debug, Default)]
pub struct Gd3 {
    pub track: String,
    pub game: String,
    pub system: String,
    pub author: String,
    pub date: String,
    pub ripper: String,
}

pub fn is_vgm(bytes: &[u8]) -> bool {
    bytes.len() > 0x40 && &bytes[..4] == b"Vgm "
}

/// A .vgz is a gzipped VGM.
pub fn unpack(bytes: &[u8]) -> Result<Vec<u8>, String> {
    if is_vgm(bytes) {
        return Ok(bytes.to_vec());
    }
    if bytes.len() > 2 && bytes[0] == 0x1F && bytes[1] == 0x8B {
        let out = crate::tar::gunzip(bytes).map_err(|e| format!("vgz: {e}"))?;
        if is_vgm(&out) {
            return Ok(out);
        }
    }
    Err("not a VGM or VGZ file".into())
}

fn u32le(b: &[u8], at: usize) -> u32 {
    b.get(at..at + 4).map_or(0, |s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

pub fn gd3(b: &[u8]) -> Gd3 {
    let off = u32le(b, 0x14) as usize;
    if off == 0 {
        return Gd3::default();
    }
    let at = 0x14 + off;
    if b.get(at..at + 4) != Some(b"Gd3 ") {
        return Gd3::default();
    }
    let len = u32le(b, at + 8) as usize;
    let data = b.get(at + 12..(at + 12 + len).min(b.len())).unwrap_or(&[]);
    let units: Vec<u16> = data.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    let s: Vec<String> = units.split(|u| *u == 0).map(String::from_utf16_lossy).collect();
    let get = |i: usize| s.get(i).cloned().unwrap_or_default().trim().to_string();
    Gd3 { track: get(0), game: get(2), system: get(4), author: get(6), date: get(8), ripper: get(9) }
}

/// A chip write at a time.
#[derive(Clone, Copy)]
enum W {
    Sn(u8),
    Opn(u8, u8, u8),
    Opm(u8, u8),
    Ay(u8, u8),
}

/// A run of YM2612 DAC writes from the PCM bank: one drum hit (or a
/// longer sample), where it starts, and how fast it was played.
#[derive(Clone, Copy, Debug)]
pub struct DacRun {
    pub t: f64,
    pub offset: usize,
    pub len: usize,
    pub rate: f64,
}

pub struct Log {
    writes: Vec<(f64, W)>,
    /// The PCM data bank (data blocks of type 0) and the DAC's runs over it.
    pub pcm: Vec<u8>,
    pub dac: Vec<DacRun>,
    pub seconds: f64,
    pub sn_clock: f64,
    pub opn_clock: f64,
    pub opm_clock: f64,
    pub ay_clock: f64,
    /// Frames a second the music is driven at (60 or 50).
    pub rate: f64,
}

pub fn log(b: &[u8], max_seconds: f64) -> Result<Log, String> {
    let version = u32le(b, 8);
    let clock = |at: usize| if b.len() > at + 4 { (u32le(b, at) & 0x3FFF_FFFF) as f64 } else { 0.0 };
    let sn_clock = clock(0x0C);
    let opn_clock = if version >= 0x110 { clock(0x2C) } else { 0.0 };
    let opm_clock = if version >= 0x110 { clock(0x30) } else { 0.0 };
    let ay_clock = if version >= 0x151 { clock(0x74) } else { 0.0 };
    let data = if version >= 0x150 && u32le(b, 0x34) != 0 { 0x34 + u32le(b, 0x34) as usize } else { 0x40 };
    let mut at = data;
    let mut samples: u64 = 0;
    let mut writes = Vec::new();
    let (mut ntsc_waits, mut pal_waits) = (0, 0);
    let mut pcm: Vec<u8> = Vec::new();
    let mut pcm_at = 0usize;
    let mut dac: Vec<DacRun> = Vec::new();
    // The open run: its start time, offset, writes, and the last write's sample time.
    let mut run: Option<(u64, usize, usize, u64)> = None;
    let close = |run: &mut Option<(u64, usize, usize, u64)>, dac: &mut Vec<DacRun>| {
        if let Some((start, offset, len, last)) = run.take() {
            if len >= 64 {
                let secs = (last.saturating_sub(start)).max(1) as f64 / 44100.0;
                dac.push(DacRun { t: start as f64 / 44100.0, offset, len, rate: (len as f64 / secs).clamp(2000.0, 44100.0) });
            }
        }
    };
    let limit = (max_seconds * 44100.0) as u64;
    while at < b.len() && samples < limit {
        let t = samples as f64 / 44100.0;
        let op = b[at];
        let arg = |k: usize| b.get(at + k).copied().unwrap_or(0);
        at += match op {
            0x4F | 0x30..=0x3F => 2,
            0x50 => {
                writes.push((t, W::Sn(arg(1))));
                2
            }
            0x52 | 0x53 => {
                writes.push((t, W::Opn(op - 0x52, arg(1), arg(2))));
                3
            }
            0x54 => {
                writes.push((t, W::Opm(arg(1), arg(2))));
                3
            }
            0xA0 => {
                writes.push((t, W::Ay(arg(1), arg(2))));
                3
            }
            0x40..=0x4E | 0x51 | 0x55..=0x5F | 0xA1..=0xAF | 0xB0..=0xBF => 3,
            0xC0..=0xDF => 4,
            0xE0 => {
                pcm_at = u32le(b, at + 1) as usize;
                close(&mut run, &mut dac);
                5
            }
            0xE1..=0xFF => 5,
            0x61 => {
                samples += u16::from_le_bytes([arg(1), arg(2)]) as u64;
                3
            }
            0x62 => {
                samples += 735;
                ntsc_waits += 1;
                1
            }
            0x63 => {
                samples += 882;
                pal_waits += 1;
                1
            }
            0x66 => break,
            0x67 => {
                let size = (u32le(b, at + 3) & 0x7FFF_FFFF) as usize;
                if arg(2) == 0 {
                    pcm.extend_from_slice(b.get(at + 7..(at + 7 + size).min(b.len())).unwrap_or(&[]));
                }
                7 + size
            }
            0x70..=0x7F => {
                samples += (op & 15) as u64 + 1;
                1
            }
            0x80..=0x8F => {
                // A DAC write from the bank: a run while the writes follow
                // each other closely from consecutive bytes.
                match &mut run {
                    Some((_, offset, len, last)) if *offset + *len == pcm_at && samples - *last < 200 => {
                        *len += 1;
                        *last = samples;
                    }
                    _ => {
                        close(&mut run, &mut dac);
                        run = Some((samples, pcm_at, 1, samples));
                    }
                }
                pcm_at += 1;
                samples += (op & 15) as u64;
                1
            }
            0x90..=0x95 => match op {
                0x90 | 0x91 => 5,
                0x92 => 6,
                0x93 => 11,
                0x94 => 2,
                _ => 5,
            },
            _ => 1,
        };
    }
    close(&mut run, &mut dac);
    Ok(Log { writes, pcm, dac, seconds: samples as f64 / 44100.0, sn_clock, opn_clock, opm_clock, ay_clock, rate: if pal_waits > ntsc_waits { 50.0 } else { 60.0 } })
}

fn pitch(hz: f64) -> f32 {
    if hz > 8.0 { (69.0 + 12.0 * (hz / 440.0).log2()) as f32 } else { f32::NAN }
}

#[derive(Clone, Copy, Default)]
struct Opn {
    fnum: u16,
    block: u8,
    algorithm: u8,
    feedback: u8,
    tl: [u8; 4],
    mul: [u8; 4],
    on: bool,
    keyed: bool,
}

/// The operators that are carriers in each FM algorithm (slot order 1, 3, 2, 4 as the chips number them, here 0..4).
const CARRIERS: [&[usize]; 8] = [&[3], &[3], &[3], &[3], &[1, 3], &[1, 2, 3], &[1, 2, 3], &[0, 1, 2, 3]];

impl Opn {
    fn level(&self) -> f32 {
        if !self.on {
            return 0.0;
        }
        let c = CARRIERS[self.algorithm as usize & 7];
        let tl = c.iter().map(|&k| self.tl[k] as f32).fold(127.0, f32::min);
        10f32.powf(-tl * 0.75 / 20.0)
    }

    /// A timbre id: the algorithm and the modulator-to-carrier ratio.
    fn timbre(&self) -> u16 {
        (self.algorithm as u16) << 8 | (self.mul[0] as u16) << 4 | self.mul[3] as u16
    }
}

/// The DAC's hits as a sample voice, and its samples (each run of the
/// bank once, at the rate it was played).
pub fn dac(l: &Log, hash: &str) -> (Option<ChipVoice>, Vec<super::tracker::arrange::SampleAsset>) {
    if l.dac.is_empty() {
        return (None, Vec::new());
    }
    let mut keys: Vec<(usize, usize)> = Vec::new();
    let mut assets = Vec::new();
    let mut frames = Vec::new();
    for r in &l.dac {
        let k = match keys.iter().position(|&(o, n)| o == r.offset && (n as i64 - r.len as i64).abs() < 32) {
            Some(k) => k,
            None => {
                keys.push((r.offset, r.len));
                let bytes = l.pcm.get(r.offset..(r.offset + r.len).min(l.pcm.len())).unwrap_or(&[]);
                let pcm: Vec<f32> = bytes.iter().map(|&v| (v as f32 - 128.0) / 128.0).collect();
                let sample = makepad_score_player::Sample::new(pcm.into(), None, r.rate as f32);
                let id = format!("vgm-{hash}-d{}", keys.len());
                assets.push(super::tracker::arrange::SampleAsset { asset: makepad_score_live::AssetRef { id, rev: "1".into() }, sample: std::sync::Arc::new(sample), name: format!("DAC sample {}", keys.len()) });
                keys.len() - 1
            }
        };
        frames.push(Frame { t: r.t, pitch: 60.0, level: 1.0, retrigger: true, timbre: k as u16, cutoff: f32::NAN });
        frames.push(Frame { t: r.t + r.len as f64 / r.rate, pitch: f32::NAN, level: 0.0, retrigger: false, timbre: k as u16, cutoff: f32::NAN });
    }
    frames.sort_by(|a, b| a.t.total_cmp(&b.t));
    (Some(ChipVoice { name: "dac".into(), kind: Kind::Sample, frames }), assets)
}

/// The chips' voices read every frame.
pub fn voices(l: &Log) -> Vec<ChipVoice> {
    let mut out: Vec<ChipVoice> = Vec::new();
    let frame = 1.0 / l.rate;
    let mut t = frame * 0.95;
    let mut w = 0;
    // SN76489.
    let mut sn_tone = [0u16; 3];
    let mut sn_att = [15u8; 4];
    let mut sn_noise = 0u8;
    let mut sn_latch = (0usize, false);
    let mut sn_retrig = [false; 4];
    // YM2612 and YM2151.
    let mut opn = [Opn::default(); 6];
    let mut opn_regs = [[0u8; 256]; 2];
    let mut opm = [Opn::default(); 8];
    let mut opm_kc = [0u8; 8];
    // AY.
    let mut ay = [0u8; 16];
    let sn = l.sn_clock > 0.0;
    let opn_on = l.opn_clock > 0.0;
    let opm_on = l.opm_clock > 0.0;
    let ay_on = l.ay_clock > 0.0;
    let mut add = |name: &str, kind: Kind| {
        out.push(ChipVoice { name: name.into(), kind, frames: Vec::new() });
        out.len() - 1
    };
    let sn_v: Vec<usize> = if sn { vec![add("psg1", Kind::Tone), add("psg2", Kind::Tone), add("psg3", Kind::Tone), add("psg_noise", Kind::Noise)] } else { Vec::new() };
    let opn_v: Vec<usize> = if opn_on { (1..=6).map(|k| add(&format!("fm{k}"), Kind::Tone)).collect() } else { Vec::new() };
    let opm_v: Vec<usize> = if opm_on { (1..=8).map(|k| add(&format!("opm{k}"), Kind::Tone)).collect() } else { Vec::new() };
    let ay_v: Vec<usize> = if ay_on { vec![add("ay1", Kind::Tone), add("ay2", Kind::Tone), add("ay3", Kind::Tone)] } else { Vec::new() };
    while t < l.seconds {
        while w < l.writes.len() && l.writes[w].0 <= t {
            match l.writes[w].1 {
                W::Sn(b) => {
                    if b & 0x80 != 0 {
                        let ch = ((b >> 5) & 3) as usize;
                        let vol = b & 0x10 != 0;
                        sn_latch = (ch, vol);
                        if vol {
                            if sn_att[ch] == 15 && b & 15 != 15 {
                                sn_retrig[ch] = true;
                            }
                            sn_att[ch] = b & 15;
                        } else if ch == 3 {
                            sn_noise = b & 7;
                            sn_retrig[3] = true;
                        } else {
                            sn_tone[ch] = (sn_tone[ch] & 0x3F0) | (b & 15) as u16;
                        }
                    } else {
                        let (ch, vol) = sn_latch;
                        if vol {
                            sn_att[ch] = b & 15;
                        } else if ch < 3 {
                            sn_tone[ch] = (sn_tone[ch] & 15) | ((b & 0x3F) as u16) << 4;
                        }
                    }
                }
                W::Opn(port, reg, v) => {
                    opn_regs[port as usize][reg as usize] = v;
                    let base = port as usize * 3;
                    match reg {
                        0x28 if port == 0 => {
                            let ch = (v & 3) as usize + if v & 4 != 0 { 3 } else { 0 };
                            if ch < 6 {
                                let on = v & 0xF0 != 0;
                                if on && !opn[ch].on {
                                    opn[ch].keyed = true;
                                }
                                opn[ch].on = on;
                            }
                        }
                        0xA0..=0xA2 => opn[base + (reg - 0xA0) as usize].fnum = (opn[base + (reg - 0xA0) as usize].fnum & 0x700) | v as u16,
                        0xA4..=0xA6 => {
                            let c = &mut opn[base + (reg - 0xA4) as usize];
                            c.fnum = (c.fnum & 0xFF) | ((v as u16 & 7) << 8);
                            c.block = (v >> 3) & 7;
                        }
                        0xB0..=0xB2 => {
                            let c = &mut opn[base + (reg - 0xB0) as usize];
                            c.algorithm = v & 7;
                            c.feedback = (v >> 3) & 7;
                        }
                        0x40..=0x4F if reg & 3 != 3 => {
                            let slot = [0, 2, 1, 3][((reg - 0x40) >> 2) as usize];
                            opn[base + (reg & 3) as usize].tl[slot] = v & 0x7F;
                        }
                        0x30..=0x3F if reg & 3 != 3 => {
                            let slot = [0, 2, 1, 3][((reg - 0x30) >> 2) as usize];
                            opn[base + (reg & 3) as usize].mul[slot] = v & 15;
                        }
                        _ => {}
                    }
                }
                W::Opm(reg, v) => match reg {
                    0x08 => {
                        let ch = (v & 7) as usize;
                        let on = v & 0x78 != 0;
                        if on && !opm[ch].on {
                            opm[ch].keyed = true;
                        }
                        opm[ch].on = on;
                    }
                    0x20..=0x27 => opm[(reg & 7) as usize].algorithm = v & 7,
                    0x28..=0x2F => opm_kc[(reg & 7) as usize] = v,
                    0x60..=0x7F => {
                        let slot = [0, 2, 1, 3][((reg - 0x60) >> 3) as usize];
                        opm[(reg & 7) as usize].tl[slot] = v & 0x7F;
                    }
                    0x40..=0x5F => {
                        let slot = [0, 2, 1, 3][((reg - 0x40) >> 3) as usize];
                        opm[(reg & 7) as usize].mul[slot] = v & 15;
                    }
                    _ => {}
                },
                W::Ay(reg, v) => ay[(reg & 15) as usize] = v,
            }
            w += 1;
        }
        // A frame's look.
        for ch in 0..sn_v.len() {
            let att = sn_att[ch];
            let level = if att >= 15 { 0.0 } else { 10f32.powf(-(att as f32) * 2.0 / 20.0) };
            let (p, timbre, cutoff) = if ch < 3 {
                let n = sn_tone[ch].max(1) as f64;
                (pitch(l.sn_clock / (32.0 * n)), 0, f32::NAN)
            } else {
                let rate = [512.0, 1024.0, 2048.0, 32.0 * sn_tone[2].max(1) as f64][(sn_noise & 3) as usize];
                ((72 - (sn_noise & 3) as i32 * 6) as f32, ((sn_noise >> 2) & 1) as u16, (l.sn_clock / rate).clamp(200.0, 16000.0) as f32)
            };
            out[sn_v[ch]].frames.push(Frame { t, pitch: p, level, retrigger: sn_retrig[ch], timbre, cutoff });
            sn_retrig[ch] = false;
        }
        for (k, &v) in opn_v.iter().enumerate() {
            let c = &mut opn[k];
            let hz = c.fnum as f64 * 2f64.powi(c.block as i32 - 1) * l.opn_clock / (144.0 * 1_048_576.0);
            out[v].frames.push(Frame { t, pitch: pitch(hz), level: c.level(), retrigger: c.keyed, timbre: c.timbre(), cutoff: f32::NAN });
            c.keyed = false;
        }
        for (k, &v) in opm_v.iter().enumerate() {
            // Key code: octave and a note code that skips every fourth value.
            let kc = opm_kc[k];
            let note = [0, 1, 2, 2, 3, 4, 5, 5, 6, 7, 8, 8, 9, 10, 11, 11][(kc & 15) as usize];
            let midi = 13 + 12 * ((kc >> 4) & 7) as i32 + note;
            let c = &mut opm[k];
            let p = (midi as f64 + 12.0 * (l.opm_clock / 3_579_545.0).log2()) as f32;
            out[v].frames.push(Frame { t, pitch: p, level: c.level(), retrigger: c.keyed, timbre: c.timbre(), cutoff: f32::NAN });
            c.keyed = false;
        }
        for (k, &v) in ay_v.iter().enumerate() {
            let period = (ay[k * 2] as u16 | ((ay[k * 2 + 1] as u16 & 15) << 8)).max(1);
            let tone_on = ay[7] & (1 << k) == 0;
            let vol = ay[8 + k] & 15;
            let level = if tone_on && vol > 0 { 10f32.powf(-(15.0 - vol as f32) * 1.5 / 10.0) } else { 0.0 };
            out[v].frames.push(Frame { t, pitch: pitch(l.ay_clock / (16.0 * period as f64)), level, retrigger: false, timbre: 0, cutoff: f32::NAN });
        }
        t += frame;
    }
    out.retain(|v| v.frames.iter().any(|f| f.level > 0.004));
    out
}

/// A voice's patch, and whether it plays notes.
pub fn patch(voice: &ChipVoice, timbre: u16, dac_ids: &[String]) -> (String, bool) {
    if voice.kind == Kind::Sample {
        return match dac_ids.get(timbre as usize) {
            Some(id) => (format!("Sampler{{src: asset(\"{id}\", \"1\") root: 60 attack: 0 release: 0.005}}"), false),
            None => ("Synth{noise: @white decay: 0.1 sustain: 0}".into(), false),
        };
    }
    (patch_node(voice, timbre), voice.kind == Kind::Tone)
}

fn patch_node(voice: &ChipVoice, timbre: u16) -> String {
    let flat = "attack: 0.002 decay: 0 sustain: 1 release: 0.02";
    if voice.kind == Kind::Noise {
        return if timbre == 1 { format!("Synth{{noise: @white {flat} gain: 0.4}}") } else { format!("Synth{{noise: @white filter: @bp cutoff: 2500 res: 4 {flat} gain: 0.4}}") };
    }
    if voice.name.starts_with("fm") || voice.name.starts_with("opm") {
        let algorithm = timbre >> 8;
        let (m_mul, c_mul) = (((timbre >> 4) & 15).max(1) as f32 * if (timbre >> 4) & 15 == 0 { 0.5 } else { 1.0 }, (timbre & 15).max(1) as f32);
        let ratio = ((m_mul / c_mul) * 100.0).round() / 100.0;
        // All carriers: an organ; else a sine modulated at the ratio.
        if algorithm == 7 {
            return format!("Synth{{layers: [{{osc: @sine gain: 0.3}} {{osc: @sine octave: 1 gain: 0.15}}]  {flat}}}");
        }
        let index = if algorithm >= 4 { 1.5 } else { 3.0 };
        return format!("Synth{{osc: @sine fm: {ratio} index: {index} attack: 0.002 decay: 0.4 sustain: 0.6 release: 0.08 gain: 0.4}}");
    }
    format!("Synth{{osc: @square {flat} gain: 0.35}}")
}
